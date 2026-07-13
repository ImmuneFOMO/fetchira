//! Self-update for the prebuilt binary: `fetchira update` pulls the latest GitHub release
//! and replaces the running binary in place. Also a passive "new version available" check,
//! surfaced in the CLI (stderr, TTY only — never in MCP stdio) and in the web dashboard.
//!
//! A Homebrew-managed copy is left alone: we detect it and point at `brew upgrade` instead.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use semver::Version;

const REPO: &str = "ImmuneFOMO/fetchira";
const CHECK_INTERVAL: u64 = 24 * 60 * 60; // throttle the passive check to once a day

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
    if is_brew_managed() {
        "brew upgrade fetchira"
    } else {
        "fetchira update"
    }
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
    Brew,
    UpToDate,
    Updated(String),
}

/// Download the latest release for this platform and replace the binary in place.
/// Shared by `fetchira update` and the dashboard's Update button.
pub async fn perform(home: &Path) -> anyhow::Result<Outcome> {
    if is_brew_managed() {
        return Ok(Outcome::Brew);
    }
    let client = client(Duration::from_secs(60)).context("could not build http client")?;
    let (tag, latest) = fetch_latest(&client)
        .await
        .context("could not check the latest release")?;
    write_cache(home, &latest.to_string());
    if latest <= current() {
        return Ok(Outcome::UpToDate);
    }
    let triple = target_triple().context(
        "no prebuilt binary for this platform — reinstall via install.sh or `cargo install`",
    )?;
    let dir = std::env::temp_dir().join(format!("fetchira-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let res = download_and_swap(&client, &tag, triple, &dir).await;
    let _ = std::fs::remove_dir_all(&dir); // clean up on success and failure alike
    res?;
    Ok(Outcome::Updated(latest.to_string()))
}

/// `fetchira update` — the CLI face of `perform`.
pub async fn run(home: &Path) -> anyhow::Result<()> {
    println!("fetchira {}", current());
    match perform(home).await? {
        Outcome::Brew => println!("installed via Homebrew — run `brew upgrade fetchira`"),
        Outcome::UpToDate => println!("already up to date"),
        Outcome::Updated(v) => println!("updated fetchira to {v} — restart any running instances"),
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
    if let Ok(resp) = client
        .get(format!("{base}/fetchira-{triple}.tar.xz.sha256"))
        .send()
        .await
        .and_then(|r| r.error_for_status())
    {
        verify_sha256(&tarball, &resp.text().await?)?;
    }

    let archive = dir.join("fetchira.tar.xz");
    std::fs::write(&archive, &tarball)?;
    let extracted = extract(&archive, dir)?;
    finalize(&extracted)?;
    self_replace::self_replace(&extracted)?;
    Ok(())
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
