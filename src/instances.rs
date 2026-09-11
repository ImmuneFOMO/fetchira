//! Registry of running fetchira processes (`<home>/run/<pid>.json` + a `ps` sweep), so a
//! schema-changing update can refuse to swap while old-version MCP servers are still alive.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct Instance {
    pub pid: u32,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Older dashboards would silently drop the new `[remote]` section on their next save.
    #[serde(default)]
    pub supports_remote_config: bool,
}

/// Removes this process's registry entry on drop (kernel cleanup isn't needed — a stale
/// file is GC'd by the next `running()` sweep when its pid is gone from `ps`).
pub struct RunGuard {
    path: PathBuf,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Record this process in the registry for its lifetime. `mode` is "mcp" or "ui".
pub fn register(home: &Path, mode: &str) -> RunGuard {
    let dir = home.join("run");
    let _ = std::fs::create_dir_all(&dir);
    let pid = std::process::id();
    let host = if mode == "ui" {
        Some("ui".to_string())
    } else {
        host_of_parent()
    };
    let inst = Instance {
        pid,
        mode: mode.to_string(),
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
        hint: host.as_deref().map(|h| restart_hint(h).to_string()),
        host,
        supports_remote_config: true,
    };
    let path = dir.join(format!("{pid}.json"));
    if let Ok(s) = serde_json::to_string(&inst) {
        let _ = std::fs::write(&path, s);
    }
    RunGuard { path }
}

/// Every live fetchira process except `exclude`: registry entries verified against `ps`,
/// plus unregistered fetchira processes (pre-registry versions) found by the same sweep.
pub fn running(home: &Path, exclude: &[u32]) -> Vec<Instance> {
    running_with_scope(home, exclude, true)
}

/// Since 0.1.13, processes register their config home. Data migrations must not be blocked by
/// unrelated installations; the binary updater retains its broader pre-registry process sweep.
pub(crate) fn running_in_home(home: &Path, exclude: &[u32]) -> Vec<Instance> {
    running_with_scope(home, exclude, false)
}

fn running_with_scope(home: &Path, exclude: &[u32], include_unregistered: bool) -> Vec<Instance> {
    let table = ps_table();
    let dir = home.join("run");
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let Some(inst) = std::fs::read_to_string(e.path())
            .ok()
            .and_then(|s| serde_json::from_str::<Instance>(&s).ok())
        else {
            continue;
        };
        // GC: pid gone, or reused by something that isn't fetchira.
        if !table
            .iter()
            .any(|r| r.pid == inst.pid && is_fetchira(&r.args))
        {
            let _ = std::fs::remove_file(e.path());
            continue;
        }
        seen.push(inst.pid);
        if !exclude.contains(&inst.pid) {
            out.push(inst);
        }
    }
    for r in &table {
        if !include_unregistered
            || !is_fetchira(&r.args)
            || seen.contains(&r.pid)
            || exclude.contains(&r.pid)
            || r.args.contains("--when-idle")
        {
            continue;
        }
        let host = table
            .iter()
            .find(|p| p.pid == r.ppid)
            .map(|p| host_key(p.args.split_whitespace().next().unwrap_or("")));
        out.push(Instance {
            pid: r.pid,
            mode: if r.args.contains(" ui") { "ui" } else { "mcp" }.to_string(),
            version: None,
            hint: host.as_deref().map(|h| restart_hint(h).to_string()),
            host,
            supports_remote_config: false,
        });
    }
    out.sort_by_key(|i| i.pid);
    out
}

pub(crate) fn ensure_remote_config_compatible(home: &Path) -> crate::Result<()> {
    // A legacy UI may still be starting or may have failed to write its registry entry.
    // Conservatively include unregistered dashboards here; silently losing credentials is worse
    // than asking the user to close an unrelated old dashboard. DB migration remains home-scoped.
    let unsafe_peers: Vec<_> = running(home, &[std::process::id()])
        .into_iter()
        .filter(|instance| instance.mode == "ui" && !instance.supports_remote_config)
        .map(|instance| instance.pid.to_string())
        .collect();
    if !unsafe_peers.is_empty() {
        return Err(crate::Error::Config(format!("restart older Fetchira dashboards (PID {}) before saving a hosted connection; they could erase the server settings", unsafe_peers.join(", "))));
    }
    Ok(())
}

/// Whether `pid` is a live fetchira process (used to spot stale idle-update markers).
pub fn alive(pid: u32) -> bool {
    ps_table()
        .iter()
        .any(|r| r.pid == pid && is_fetchira(&r.args))
}

/// How to restart fetchira inside the tool that spawned it (stdio MCP servers are never
/// respawned mid-session by any client — the user has to do it in the tool).
pub fn restart_hint(host: &str) -> &'static str {
    match host {
        "claude" => "Claude Code: /mcp → fetchira → reconnect (or restart the session)",
        "cursor" => "Cursor: Settings → MCP → toggle fetchira off/on",
        "codex" => "Codex CLI: restart codex",
        "vscode" => "VS Code: command palette → MCP: List Servers → fetchira → restart",
        "ui" => "close the fetchira dashboard (Ctrl-C in its terminal)",
        _ => "restart the tool — MCP servers respawn on launch",
    }
}

struct PsRow {
    pid: u32,
    ppid: u32,
    args: String,
}

fn ps_table() -> Vec<PsRow> {
    let Ok(out) = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid=,args="])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some(PsRow {
                pid: it.next()?.parse().ok()?,
                ppid: it.next()?.parse().ok()?,
                args: it.collect::<Vec<_>>().join(" "),
            })
        })
        .collect()
}

fn is_fetchira(args: &str) -> bool {
    Path::new(args.split_whitespace().next().unwrap_or(""))
        .file_name()
        .is_some_and(|n| n == "fetchira")
}

/// Friendly key for the tool that spawned us, from its executable name.
fn host_key(comm: &str) -> String {
    let name = Path::new(comm)
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    for k in ["claude", "cursor", "codex", "windsurf", "zed"] {
        if name.contains(k) {
            return k.to_string();
        }
    }
    if name.contains("code") {
        return "vscode".to_string();
    }
    name
}

fn host_of_parent() -> Option<String> {
    if std::env::var("CLAUDECODE").is_ok() {
        return Some("claude".to_string());
    }
    let ppid = std::os::unix::process::parent_id();
    let out = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &ppid.to_string()])
        .output()
        .ok()?;
    let comm = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!comm.is_empty()).then(|| host_key(&comm))
}
