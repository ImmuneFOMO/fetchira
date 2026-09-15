//! Registry of running fetchira processes (`<home>/run/<pid>.json` + a `ps` sweep), so a
//! schema-changing update can refuse to swap while old-version MCP servers are still alive.

use std::path::{Path, PathBuf};

use anyhow::Context;
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
    #[serde(default)]
    pub supports_update_control: bool,
    #[serde(default)]
    pub process_start: Option<String>,
    #[serde(default)]
    pub binary: Option<PathBuf>,
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
        supports_update_control: matches!(mode, "mcp" | "ui"),
        process_start: process_start(pid),
        binary: std::env::current_exe()
            .ok()
            .and_then(|p| p.canonicalize().ok()),
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
        if !table.iter().any(|r| {
            r.pid == inst.pid
                && is_fetchira(&r.args)
                && inst
                    .process_start
                    .as_ref()
                    .is_none_or(|start| process_start(r.pid).as_ref() == Some(start))
        }) {
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
            supports_update_control: false,
            process_start: None,
            binary: None,
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

pub const UPDATING: &str =
    "Fetchira is updating. Reconnect the Fetchira MCP server in your coding tool and retry.";

fn lock_file(home: &Path, name: &str) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(home.join(name))
}

pub fn admit_request(home: &Path) -> std::io::Result<std::fs::File> {
    let file = lock_file(home, "update-admission.lock")?;
    file.try_lock_shared().map_err(std::io::Error::from)?;
    if update_mode(home).is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            UPDATING,
        ));
    }
    Ok(file)
}

pub fn update_mode(home: &Path) -> Option<String> {
    if home.join("update-recovery.json").exists() {
        return Some("recovery".into());
    }
    let bytes = std::fs::read(home.join("update-control.json")).ok()?;
    // An updater that crashed before writing its recovery journal never replaced the binary.
    // Ignore its stale marker only after proving no owner holds the OS lock.
    let owner = lock_file(home, "update-owner.lock").ok()?;
    if owner.try_lock().is_ok() {
        return None;
    }
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return Some("recovery".into()),
    };
    Some(
        value
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("recovery")
            .to_owned(),
    )
}
pub fn update_token(home: &Path) -> Option<String> {
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(home.join("update-control.json")).ok()?).ok()?;
    value
        .get("token")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

/// Only the updater's authenticated child may migrate while the parent holds admission closed.
pub fn migration_child(home: &Path) -> bool {
    let Some(token) = update_token(home) else {
        return false;
    };
    let owner = std::fs::read(home.join("update-control.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    owner
        .as_ref()
        .and_then(|v| v.get("pid"))
        .and_then(|v| v.as_u64())
        == Some(std::os::unix::process::parent_id() as u64)
        && std::env::var("FETCHIRA_UPDATE_TOKEN").ok().as_deref() == Some(token.as_str())
}

/// Restore the saved binary/database pair while all request and startup admission is closed.
/// Every step is repeatable: leave the journal/backups intact until both replacements succeed.
pub async fn recovery(home: &Path) -> anyhow::Result<()> {
    let owner = lock_file(home, "update-owner.lock")?;
    owner
        .try_lock()
        .map_err(|_| anyhow::anyhow!("another Fetchira update is running"))?;
    let path = home.join("update-recovery.json");
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    let field = |key| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("recovery state is missing {key}"))
    };
    let previous = Path::new(field("previous")?);
    let target = Path::new(field("binary")?);
    let db = Path::new(field("database")?);
    let backup = value.get("backup").and_then(|v| v.as_str()).map(Path::new);
    anyhow::ensure!(
        previous.is_file(),
        "saved binary is missing; recovery remains paused"
    );
    // Check the snapshot before changing either half of the pair.
    if let Some(backup) = backup {
        use sqlx::Connection;
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(backup)
            .read_only(true);
        let mut connection = sqlx::SqliteConnection::connect_with(&options).await?;
        let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&mut connection)
            .await?;
        connection.close().await?;
        anyhow::ensure!(
            integrity == "ok",
            "database backup failed integrity_check; recovery remains paused"
        );
    } else {
        anyhow::ensure!(
            !db.exists(),
            "no database snapshot is available; refusing binary-only recovery"
        );
    }
    crate::config::write_atomic(
        &home.join("update-control.json"),
        r#"{"mode":"recovery"}"#,
        true,
    )?;
    let admission = lock_file(home, "update-admission.lock")?;
    admission.try_lock().map_err(|_| {
        anyhow::anyhow!("Fetchira requests are still running; close them before recovery")
    })?;
    anyhow::ensure!(
        running_in_home(home, &[std::process::id()]).is_empty(),
        "close Fetchira MCP servers and dashboards before recovery"
    );
    if let Some(backup) = backup {
        // No live Fetchira connection remains. Remove sidecars before replacing the snapshot;
        // on an interruption the journal keeps admission closed and retry repeats this restore.
        for suffix in ["-wal", "-shm"] {
            let sidecar = PathBuf::from(format!("{}{suffix}", db.display()));
            match std::fs::remove_file(sidecar) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        replace_file(backup, db)?;
    }
    replace_file(previous, target)?;
    std::fs::remove_file(home.join("update-control.json"))?;
    std::fs::remove_file(path)?;
    Ok(())
}

/// Copy to a fresh inode beside the destination, then rename. This also works when the
/// destination is the running executable (overwriting its inode would fail with ETXTBSY).
fn replace_file(source: &Path, destination: &Path) -> anyhow::Result<()> {
    let parent = destination
        .parent()
        .context("replacement path has no parent")?;
    let temporary = parent.join(format!(".fetchira-restore-{:032x}", rand::random::<u128>()));
    let result = (|| -> anyhow::Result<()> {
        let mut input = std::fs::File::open(source)?;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        std::io::copy(&mut input, &mut output)?;
        output.set_permissions(input.metadata()?.permissions())?;
        output.sync_all()?;
        std::fs::rename(&temporary, destination)?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub async fn forced(home: &Path) {
    loop {
        if update_mode(home).as_deref() == Some("force") {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

pub struct UpdateGuard {
    home: PathBuf,
    _owner: std::fs::File,
    admission: Option<std::fs::File>,
    keep_closed: bool,
}

impl UpdateGuard {
    pub fn acquire(home: &Path, force: bool) -> anyhow::Result<Self> {
        let owner = lock_file(home, "update-owner.lock")?;
        owner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("another Fetchira update is running"))?;
        anyhow::ensure!(
            !home.join("update-recovery.json").exists(),
            "an interrupted update needs recovery before another update"
        );
        crate::config::write_atomic(&home.join("update-control.json"), &serde_json::json!({"mode": if force {"force"} else {"drain"}, "pid": std::process::id(), "token": format!("{:032x}", rand::random::<u128>())}).to_string(), true)?;
        Ok(Self {
            home: home.to_owned(),
            _owner: owner,
            admission: None,
            keep_closed: false,
        })
    }

    pub async fn drain(&mut self, timeout: std::time::Duration) -> anyhow::Result<()> {
        if self.admission.is_some() {
            return Ok(());
        }
        let file = lock_file(&self.home, "update-admission.lock")?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match file.try_lock() {
                Ok(()) => {
                    self.admission = Some(file);
                    return Ok(());
                }
                Err(std::fs::TryLockError::WouldBlock) => {}
                Err(error) => return Err(std::io::Error::from(error).into()),
            }
            anyhow::ensure!(tokio::time::Instant::now() < deadline, "timed out waiting for Fetchira requests to finish; retry with --force to interrupt them");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
    pub fn reopen(&mut self) {
        self.keep_closed = false;
    }
    pub fn keep_closed(&mut self) {
        self.keep_closed = true;
    }
}

impl Drop for UpdateGuard {
    fn drop(&mut self) {
        if !self.keep_closed {
            let _ = std::fs::remove_file(self.home.join("update-control.json"));
        }
    }
}

fn process_start(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let value = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn process_binary(pid: u32) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/{pid}/exe"))
            .ok()?
            .canonicalize()
            .ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let out = std::process::Command::new("ps")
            .args(["-o", "comm=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        Path::new(String::from_utf8_lossy(&out.stdout).trim())
            .canonicalize()
            .ok()
    }
}

/// Signal only registered peers whose executable and start fingerprint still match.
/// Legacy entries require an open file under this home to positively identify their scope.
pub async fn stop_for_update(home: &Path, exclude: &[u32]) -> anyhow::Result<()> {
    let expected = std::env::current_exe()?.canonicalize()?;
    let mut peers = Vec::new();
    for peer in running_in_home(home, exclude) {
        let start = process_start(peer.pid)
            .ok_or_else(|| anyhow::anyhow!("cannot identify Fetchira PID {}", peer.pid))?;
        anyhow::ensure!(
            process_binary(peer.pid).as_ref() == Some(&expected),
            "Fetchira PID {} uses another binary; close it before updating",
            peer.pid
        );
        if let Some(stamp) = &peer.process_start {
            anyhow::ensure!(
                stamp == &start && peer.binary.as_ref() == Some(&expected),
                "Fetchira PID {} identity changed",
                peer.pid
            );
        } else {
            let out = std::process::Command::new("lsof")
                .args(["-a", "-p", &peer.pid.to_string(), "-Fn"])
                .output()?;
            let prefix = format!("n{}/", home.canonicalize()?.display());
            anyhow::ensure!(
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .any(|line| line.starts_with(&prefix)),
                "cannot safely identify legacy Fetchira PID {}; close this MCP manually",
                peer.pid
            );
        }
        peers.push((peer.pid, start));
    }
    for signal in ["-TERM", "-KILL"] {
        for (pid, start) in &peers {
            if process_start(*pid).as_ref() == Some(start)
                && process_binary(*pid).as_ref() == Some(&expected)
            {
                let status = std::process::Command::new("kill")
                    .args([signal, &pid.to_string()])
                    .status()?;
                anyhow::ensure!(
                    status.success() || process_start(*pid).is_none(),
                    "could not stop Fetchira PID {pid}"
                );
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    for (pid, start) in peers {
        anyhow::ensure!(
            process_start(pid).as_ref() != Some(&start),
            "Fetchira PID {pid} did not exit"
        );
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_home() -> PathBuf {
        std::env::temp_dir().join(format!(
            "fetchira-update-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn admission_rejects_new_work_during_update() {
        let home = temp_home();
        std::fs::create_dir_all(&home).unwrap();
        let permit = admit_request(&home).unwrap();
        let guard = UpdateGuard::acquire(&home, true).unwrap();
        assert!(admit_request(&home).is_err());
        drop(permit);
        drop(guard);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn restore_replaces_destination_by_rename() {
        let root = temp_home();
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("previous");
        let target = root.join("binary");
        std::fs::write(&source, b"old").unwrap();
        std::fs::write(&target, b"new").unwrap();
        replace_file(&source, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        let _ = std::fs::remove_dir_all(root);
    }
}
