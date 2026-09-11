//! Self-update for the prebuilt binary: `fetchira update` pulls the latest GitHub release
//! and replaces the running binary in place. Also a passive "new version available" check,
//! surfaced in the CLI (stderr, TTY only — never in MCP stdio) and in the web dashboard.
//!
//! Homebrew-managed copies update through Homebrew; standalone copies use release archives.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use semver::Version;

const REPO: &str = "ImmuneFOMO/fetchira";
const CHECK_INTERVAL: u64 = 24 * 60 * 60; // throttle the passive check to once a day

pub(crate) fn refresh_integrations(
    agent_home: &Path,
    home: &Path,
    bin: &str,
) -> Vec<crate::cli::IntegrationResult> {
    let mut results = crate::skills::refresh_existing(agent_home, home, Path::new(bin), true)
        .into_iter()
        .map(|result| crate::cli::IntegrationResult {
            name: result.name.into(),
            ok: result.ok,
            msg: result.msg,
        })
        .collect::<Vec<_>>();
    results.extend(crate::cli::repair_mcp_launchers(home, bin));
    results
}

pub fn refresh_skills(home: &Path) -> anyhow::Result<()> {
    let agent_home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("HOME is required to refresh agent skills")?;
    let bin = crate::cli::installation_binary()?;
    let results = refresh_integrations(&agent_home, home, &bin);
    if results.is_empty() {
        println!("Your existing agent integrations are up to date.");
    }
    let failed = results.iter().any(|result| !result.ok);
    for result in results {
        println!("{}: {}", result.name, result.msg);
    }
    if failed {
        anyhow::bail!("some skills could not be refreshed; existing integrations were preserved where refresh failed");
    }
    println!("Restart your agents to load updated skills and launchers. Your integration choices are unchanged.");
    Ok(())
}

/// A local, once-per-version notice also reaches users who upgraded through Homebrew.
pub fn integration_nudge(home: &Path) {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return;
    }
    let marker = home.join("agent-upgrade-notice");
    if std::fs::read_to_string(&marker).ok().as_deref() == Some(env!("CARGO_PKG_VERSION")) {
        return;
    }
    let agent_home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let Ok(bin) = crate::cli::installation_binary() else {
        return;
    };
    let skills = crate::skills::refresh_existing(&agent_home, home, Path::new(&bin), false);
    let targets: Vec<_> = crate::cli::mcp_target_list()
        .into_iter()
        .filter(|target| target.installed)
        .map(|target| target.name)
        .collect();
    if skills.is_empty() && targets.is_empty() {
        return;
    }
    if let Err(error) = finish_upgrade(home) {
        eprintln!("Agent upgrade: {error}");
    }
}

/// Executed by the newly installed binary, so embedded skills come from the new release.
pub fn finish_upgrade(home: &Path) -> anyhow::Result<()> {
    refresh_skills(home)?;
    let targets: Vec<_> = crate::cli::mcp_target_list()
        .into_iter()
        .filter(|target| target.installed)
        .map(|target| target.name)
        .collect();
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() && !targets.is_empty() {
        println!(
            "Fetchira MCP is installed in {}. CLI-only provides the same operations.",
            targets.join(", ")
        );
        if inquire::Confirm::new("Review agent setup or switch to CLI-only now?")
            .with_default(false)
            .prompt()
            .unwrap_or(false)
        {
            crate::cli::install_tools(home)?;
        }
    }
    crate::config::write_atomic(
        &home.join("agent-upgrade-notice"),
        env!("CARGO_PKG_VERSION"),
        false,
    )?;
    Ok(())
}

async fn finish_in_new_binary(home: &Path, binary: &str) -> anyhow::Result<()> {
    let status = tokio::process::Command::new(binary)
        .arg("--finish-upgrade")
        .env("FETCHIRA_HOME", crate::cli::absolute_path(home))
        .status()
        .await
        .context("updated binary could not finish agent setup")?;
    anyhow::ensure!(
        status.success(),
        "binary updated, but agent setup did not finish"
    );
    Ok(())
}

fn current() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or_else(|_| Version::new(0, 0, 0))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The prebuilt target triple for this platform, or None where we don't ship one.
fn target_triple() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        _ => None,
    }
}

/// True when this binary lives inside a Homebrew prefix — then self-update must defer to brew.
fn is_brew_managed() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let exe = exe.canonicalize().unwrap_or(exe);
    let p = exe.to_string_lossy();
    p.contains("/Cellar/")
        || p.contains("/homebrew/")
        || std::env::var("HOMEBREW_PREFIX")
            .map(|pre| !pre.is_empty() && p.starts_with(&pre))
            .unwrap_or(false)
}

fn update_cmd() -> &'static str {
    "fetchira update"
}

async fn upgrade_with_brew() -> anyhow::Result<Outcome> {
    let binary = crate::cli::installation_binary()?;
    let sibling = Path::new(&binary)
        .parent()
        .unwrap_or(Path::new("."))
        .join("brew");
    let brew = if sibling.is_file() {
        sibling.as_path()
    } else {
        Path::new("brew")
    };
    run_brew_upgrade(brew, Path::new(&binary)).await
}

async fn run_brew_upgrade(brew: &Path, binary: &Path) -> anyhow::Result<Outcome> {
    let output = tokio::time::timeout(
        Duration::from_secs(600),
        tokio::process::Command::new(brew)
            .args(["upgrade", "ImmuneFOMO/tap/fetchira"])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("Homebrew update timed out after 10 minutes")?
    .context("could not start Homebrew to update Fetchira")?;
    anyhow::ensure!(
        output.status.success(),
        "Homebrew update failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let checked = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new(binary)
            .arg("--version")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("updated Homebrew binary did not respond")??;
    anyhow::ensure!(
        checked.status.success(),
        "Homebrew binary failed its version check"
    );
    let version = String::from_utf8_lossy(&checked.stdout);
    let version = version
        .split_whitespace()
        .last()
        .context("Homebrew binary returned no version")?;
    let version = Version::parse(version).context("Homebrew binary returned an invalid version")?;
    if version <= current() {
        anyhow::bail!("Homebrew has not installed a newer Fetchira yet. Your current version is intact; retry Update when the tap has the release.");
    }
    Ok(Outcome::Updated(version.to_string()))
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Cache {
    checked_at: u64,
    latest: String,
}

fn cache_path(home: &Path) -> PathBuf {
    home.join("update-check.json")
}

fn read_cache(home: &Path) -> Cache {
    std::fs::read_to_string(cache_path(home))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_cache(home: &Path, latest: &str) {
    let c = Cache {
        checked_at: now_secs(),
        latest: latest.to_string(),
    };
    if let Ok(s) = serde_json::to_string(&c) {
        let _ = std::fs::write(cache_path(home), s);
    }
}

fn client(timeout: Duration) -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(concat!("fetchira/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()
}

/// (tag, version) of the latest GitHub release.
async fn fetch_latest(client: &reqwest::Client) -> anyhow::Result<(String, Version)> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let v: serde_json::Value = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let tag = v
        .get("tag_name")
        .and_then(|t| t.as_str())
        .context("release has no tag_name")?
        .to_string();
    let ver = Version::parse(tag.trim_start_matches('v'))?;
    Ok((tag, ver))
}

/// Latest version if it's newer than the running one, throttled to one network check a day
/// (falls back to the cached value on a fresh window or a network error).
async fn newer_available(home: &Path) -> Option<Version> {
    let cache = read_cache(home);
    let fresh = now_secs().saturating_sub(cache.checked_at) < CHECK_INTERVAL;
    let latest = if fresh {
        Version::parse(&cache.latest).ok()
    } else {
        match fetch_latest(&client(Duration::from_secs(3))?).await {
            Ok((_, v)) => {
                write_cache(home, &v.to_string());
                Some(v)
            }
            Err(_) => {
                // Throttle failures too — else an offline machine re-hits the network every run.
                write_cache(home, &cache.latest);
                Version::parse(&cache.latest).ok()
            }
        }
    };
    latest.filter(|l| *l > current())
}

/// CLI nudge: stderr, terminal only. Never fires in MCP stdio mode (stdout is the protocol
/// channel, and a piped stderr means we're being driven by a client).
pub async fn nudge_if_stale(home: &Path) {
    if !std::io::stderr().is_terminal() {
        return;
    }
    if let Some(latest) = newer_available(home).await {
        eprintln!("→ fetchira {latest} is available — run `{}`", update_cmd());
    }
}

/// Dashboard banner payload, or None when up to date.
pub async fn ui_banner(home: &Path) -> Option<serde_json::Value> {
    let latest = newer_available(home).await?;
    Some(serde_json::json!({
        "latest": latest.to_string(),
        "current": env!("CARGO_PKG_VERSION"),
        "command": update_cmd(),
    }))
}

/// Force a fresh check (UI startup) so a same-day release shows without waiting out the throttle.
pub async fn refresh(home: &Path) {
    let Some(c) = client(Duration::from_secs(5)) else {
        return;
    };
    if let Ok((_, v)) = fetch_latest(&c).await {
        write_cache(home, &v.to_string());
    }
}

pub enum Outcome {
    UpToDate,
    Updated(String),
    /// The release changes the DB schema and old-version fetchira processes are still
    /// running — swapping now would break them on their next DB open.
    Blocked {
        latest: String,
        instances: Vec<crate::instances::Instance>,
    },
}

/// The `schema-version` release asset, or None when the release predates it (= no break).
async fn fetch_schema(client: &reqwest::Client, tag: &str) -> Option<i64> {
    // Test hook: lets the blocked flow be exercised before a real schema bump exists.
    if let Ok(v) = std::env::var("FETCHIRA_REMOTE_SCHEMA") {
        return v.parse().ok();
    }
    let url = format!("https://github.com/{REPO}/releases/download/{tag}/schema-version");
    let resp = client.get(url).send().await.ok()?.error_for_status().ok()?;
    resp.text().await.ok()?.trim().parse().ok()
}

/// Download the latest release for this platform and replace the binary in place.
/// Shared by `fetchira update`, the dashboard's Update button, and the idle waiter
/// (`force` skips the schema gate — the waiter only runs once every process has exited).
pub async fn perform(home: &Path, force: bool) -> anyhow::Result<Outcome> {
    let client = client(Duration::from_secs(60)).context("could not build http client")?;
    let (tag, latest) = fetch_latest(&client)
        .await
        .context("could not check the latest release")?;
    write_cache(home, &latest.to_string());
    if latest <= current() {
        return Ok(Outcome::UpToDate);
    }
    if !force {
        if let Some(remote) = fetch_schema(&client, &tag).await {
            if remote > crate::usage::SCHEMA {
                let instances = crate::instances::running(home, &[std::process::id()]);
                if !instances.is_empty() {
                    return Ok(Outcome::Blocked {
                        latest: latest.to_string(),
                        instances,
                    });
                }
            }
        }
    }
    if is_brew_managed() {
        return upgrade_with_brew().await;
    }
    let triple = target_triple().context(
        "no prebuilt binary for this platform — reinstall via install.sh or `cargo install`",
    )?;
    let dir = create_update_dir()?;
    let res = download_and_swap(&client, &tag, triple, &dir).await;
    let _ = std::fs::remove_dir_all(&dir); // clean up on success and failure alike
    res?;
    Ok(Outcome::Updated(latest.to_string()))
}

fn create_update_dir() -> anyhow::Result<PathBuf> {
    let root = std::env::temp_dir();
    for _ in 0..3 {
        let dir = root.join(format!("fetchira-update-{:032x}", rand::random::<u128>()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&dir) {
            Ok(()) => return Ok(dir),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("could not create update directory {}", dir.display())
                });
            }
        }
    }
    anyhow::bail!("could not create a unique update directory")
}

fn marker_path(home: &Path) -> PathBuf {
    home.join("update-pending.json")
}

/// The live idle-wait marker for the dashboard, or None (a dead waiter or an already-applied
/// target drops the marker).
pub fn pending(home: &Path) -> Option<serde_json::Value> {
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(marker_path(home)).ok()?).ok()?;
    let pid = v.get("pid").and_then(|p| p.as_u64()).unwrap_or(0) as u32;
    let target = Version::parse(v.get("target").and_then(|t| t.as_str()).unwrap_or("")).ok();
    if !crate::instances::alive(pid) || target.is_some_and(|t| t <= current()) {
        let _ = std::fs::remove_file(marker_path(home));
        return None;
    }
    Some(v)
}

/// `fetchira update --when-idle`: wait until every other fetchira process has exited, then
/// update. Spawned detached by the dashboard; runs in the foreground from a terminal.
pub async fn wait_idle(home: &Path) -> anyhow::Result<()> {
    let binary = crate::cli::installation_binary()?;
    if pending(home).is_some() {
        println!("an idle update is already pending");
        return Ok(());
    }
    let client = client(Duration::from_secs(10)).context("could not build http client")?;
    let (_, latest) = fetch_latest(&client)
        .await
        .context("could not check the latest release")?;
    if latest <= current() {
        println!("already up to date");
        return Ok(());
    }
    let me = std::process::id();
    std::fs::write(
        marker_path(home),
        serde_json::json!({ "pid": me, "target": latest.to_string() }).to_string(),
    )?;
    println!("waiting to update to {latest} until all fetchira processes exit…");
    let mut last = usize::MAX;
    let res = loop {
        let alive = crate::instances::running(home, &[me]);
        if alive.is_empty() {
            break perform(home, true).await;
        }
        if alive.len() != last {
            last = alive.len();
            println!("  {} process(es) still running", alive.len());
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    };
    let _ = std::fs::remove_file(marker_path(home));
    match res? {
        Outcome::Updated(v) => {
            println!("updated fetchira to {v}");
            finish_in_new_binary(home, &binary).await?;
        }
        Outcome::UpToDate => println!("already up to date"),
        _ => {}
    }
    Ok(())
}

/// `fetchira update [--when-idle]` — the CLI face of `perform`.
pub async fn run(home: &Path, mut args: impl Iterator<Item = String>) -> anyhow::Result<()> {
    let when_idle = match args.next().as_deref() {
        None => false,
        Some("--when-idle") => true,
        Some(_) => anyhow::bail!("usage: fetchira update [--when-idle]"),
    };
    anyhow::ensure!(
        args.next().is_none(),
        "usage: fetchira update [--when-idle]"
    );
    if when_idle {
        return wait_idle(home).await;
    }
    let binary = crate::cli::installation_binary()?;
    println!("fetchira {}", current());
    loop {
        match perform(home, false).await? {
            Outcome::UpToDate => {
                println!("already up to date");
                finish_upgrade(home)?;
            }
            Outcome::Updated(v) => {
                println!("updated fetchira to {v}");
                finish_in_new_binary(home, &binary).await?;
            }
            Outcome::Blocked { latest, instances } => {
                println!(
                "fetchira {latest} changes the database format; {} fetchira process(es) are still running:",
                instances.len()
            );
                for i in &instances {
                    println!(
                        "  {:>7}  {:<4} {:<9} {}",
                        i.pid,
                        i.mode,
                        i.version.as_deref().unwrap_or("?"),
                        i.hint
                            .as_deref()
                            .unwrap_or("restart the tool — MCP servers respawn on launch"),
                    );
                }
                if std::io::stdin().is_terminal()
                    && std::io::stdout().is_terminal()
                    && inquire::Confirm::new(
                        "Close or restart the listed processes, then retry the update here?",
                    )
                    .with_default(true)
                    .prompt()
                    .unwrap_or(false)
                {
                    continue;
                }
                println!("Update paused; existing data and integrations are unchanged.");
            }
        }
        break;
    }
    Ok(())
}

/// Download the release tarball for `triple`, verify it, and replace the running binary.
async fn download_and_swap(
    client: &reqwest::Client,
    tag: &str,
    triple: &str,
    dir: &Path,
) -> anyhow::Result<()> {
    let base = format!("https://github.com/{REPO}/releases/download/{tag}");
    let tarball = client
        .get(format!("{base}/fetchira-{triple}.tar.xz"))
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    // Verify the checksum dist publishes alongside the archive (catches truncated downloads).
    let checksum = client
        .get(format!("{base}/fetchira-{triple}.tar.xz.sha256"))
        .send()
        .await
        .context("could not download release checksum")?
        .error_for_status()
        .context("release checksum request failed")?
        .text()
        .await
        .context("could not read release checksum")?;
    verify_sha256(&tarball, &checksum)?;

    let archive = dir.join("fetchira.tar.xz");
    std::fs::write(&archive, &tarball)?;
    let extracted = extract(&archive, dir)?;
    finalize(&extracted)?;
    launch_check(
        &extracted,
        &dir.join("verify-home"),
        dir,
        Duration::from_secs(10),
    )
    .await?;
    self_replace::self_replace(&extracted)?;
    Ok(())
}

/// Launch the downloaded binary before replacing the current one. The isolated home and working
/// directory keep startup checks from reading or writing the user's real config.
async fn launch_check(
    bin: &Path,
    home: &Path,
    cwd: &Path,
    timeout: Duration,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(home)?;
    let output = tokio::time::timeout(
        timeout,
        tokio::process::Command::new(bin)
            .arg("--version")
            .env("FETCHIRA_HOME", home)
            .current_dir(cwd)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .with_context(|| format!("downloaded binary did not finish `--version` within {timeout:?}"))?
    .with_context(|| format!("launching downloaded binary {}", bin.display()))?;
    if output.status.success() {
        return Ok(());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");
    anyhow::bail!(
        "downloaded binary failed `--version` ({}){}",
        output.status,
        if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        }
    );
}

fn verify_sha256(bytes: &[u8], want_line: &str) -> anyhow::Result<()> {
    use sha2::{Digest, Sha256};
    let want = want_line
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase();
    // The sidecar was fetched (HTTP 200) — an empty/garbled body must fail, not silently skip.
    if want.is_empty() {
        anyhow::bail!("checksum file is empty — refusing to install");
    }
    let got: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if want != got {
        anyhow::bail!("checksum mismatch — refusing to install");
    }
    Ok(())
}

/// Unpack the .tar.xz and return the path of the extracted `fetchira` binary.
fn extract(archive: &Path, dir: &Path) -> anyhow::Result<PathBuf> {
    let ok = std::process::Command::new("tar")
        .arg("-xJf")
        .arg(archive)
        .arg("-C")
        .arg(dir)
        .status()
        .context("running tar")?
        .success();
    if !ok {
        anyhow::bail!("failed to extract update archive");
    }
    find_bin(dir).context("update archive had no fetchira binary")
}

fn find_bin(dir: &Path) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(found) = find_bin(&p) {
                return Some(found);
            }
        } else if p.file_name().is_some_and(|n| n == "fetchira") {
            return Some(p);
        }
    }
    None
}

/// Make the freshly extracted binary runnable: executable bit, and an ad-hoc signature on
/// macOS (Apple Silicon SIGKILLs unsigned binaries).
fn finalize(bin: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("codesign")
            .args(["--force", "--sign", "-"])
            .arg(bin)
            .status();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn brew_upgrade_checks_formula_result_and_new_binary() {
        use std::os::unix::fs::PermissionsExt;
        let root = create_update_dir().unwrap();
        let brew = root.join("brew");
        let binary = root.join("fetchira");
        let script = |path: &Path, body: &str| {
            std::fs::write(path, body).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        };
        script(
            &brew,
            "#!/bin/sh\n[ \"$1\" = upgrade ] && [ \"$2\" = ImmuneFOMO/tap/fetchira ]\n",
        );
        script(
            &binary,
            "#!/bin/sh\n[ \"$1\" = --version ] || exit 1\nprintf 'fetchira 99.0.0\\n'\n",
        );
        assert!(
            matches!(run_brew_upgrade(&brew, &binary).await.unwrap(), Outcome::Updated(version) if version == "99.0.0")
        );
        script(&binary, "#!/bin/sh\nprintf 'fetchira 0.0.1\\n'\n");
        assert!(run_brew_upgrade(&brew, &binary)
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("not installed a newer"));
        script(
            &brew,
            "#!/bin/sh\necho 'fixture brew failure' >&2\nexit 1\n",
        );
        assert!(run_brew_upgrade(&brew, &binary)
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("fixture brew failure"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn launch_check_accepts_startable_binary_and_rejects_failed_start() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "fetchira-update-check-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let make_executable = |path: &Path, body: &str| {
            std::fs::write(path, body).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };

        let good = root.join("good");
        make_executable(
            &good,
            "#!/bin/sh\n[ -n \"$FETCHIRA_HOME\" ] || exit 10\nprintf '%s\\n' \"$PWD\" > \"$FETCHIRA_HOME/cwd\"\nprintf '%s\\n' 'fetchira test'\n",
        );
        let good_home = root.join("good-home");
        launch_check(&good, &good_home, &root, Duration::from_secs(2))
            .await
            .unwrap();
        let expected_cwd = std::fs::canonicalize(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(good_home.join("cwd"))
                .unwrap()
                .trim(),
            expected_cwd.to_string_lossy()
        );

        let bad = root.join("bad");
        make_executable(
            &bad,
            "#!/bin/sh\nprintf '%s\\n' 'incompatible test binary' >&2\nexit 1\n",
        );
        let bad_home = root.join("bad-home");
        let error = launch_check(&bad, &bad_home, &root, Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("incompatible test binary"));

        let hanging = root.join("hanging");
        make_executable(&hanging, "#!/bin/sh\nexec sleep 60\n");
        let error = launch_check(
            &hanging,
            &root.join("hanging-home"),
            &root,
            Duration::from_millis(100),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("did not finish `--version`"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn update_dir_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = create_update_dir().unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        std::fs::remove_dir(dir).unwrap();
    }
}
