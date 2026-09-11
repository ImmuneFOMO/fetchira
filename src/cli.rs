use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use inquire::{Confirm, MultiSelect, Password, Select, Text};
use serde_json::{json, Value};

use crate::config::{self, Account, Config};
use crate::providers::{order_for, Capability, ProviderKind};
use crate::usage::{period_key, Store};
use crate::web;

/// fetchira's data dir (config, .env, usage.db) — independent of the working directory so an MCP
/// client can spawn the binary from anywhere. `FETCHIRA_HOME` overrides; default `~/.config/fetchira`.
pub fn home() -> PathBuf {
    if let Ok(h) = std::env::var("FETCHIRA_HOME") {
        return PathBuf::from(h);
    }
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("fetchira")
}

pub(crate) fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

/// Prefer the stable launcher visible to users on PATH (for example a Homebrew shim) over the
/// versioned path returned by `current_exe`; fall back to the running executable when no launcher
/// is available.
pub(crate) fn installation_binary() -> anyhow::Result<String> {
    let current = std::env::current_exe()?;
    let current_canonical = current.canonicalize().ok();
    if let Some(name) = current.file_name() {
        if let Some(path) = std::env::var_os("PATH") {
            for directory in std::env::split_paths(&path) {
                let candidate = directory.join(name);
                if candidate.is_file() && candidate.canonicalize().ok() == current_canonical {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if std::fs::metadata(&candidate)
                            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
                            .unwrap_or(false)
                        {
                            return Ok(candidate.to_string_lossy().into_owned());
                        }
                    }
                    #[cfg(not(unix))]
                    return Ok(candidate.to_string_lossy().into_owned());
                }
            }
        }
    }
    if let Some(stable) = brew_launcher_for(&current) {
        return Ok(stable.to_string_lossy().into_owned());
    }
    Ok(current.to_string_lossy().into_owned())
}

fn brew_launcher_for(current: &Path) -> Option<PathBuf> {
    let current = current.canonicalize().ok()?;
    let bin = current.parent()?;
    if bin.file_name()?.to_str()? != "bin" {
        return None;
    }
    let formula_version = bin.parent()?;
    let formula = formula_version.parent()?;
    if formula.file_name()?.to_str()? != "fetchira"
        || formula.parent()?.file_name()?.to_str()? != "Cellar"
    {
        return None;
    }
    let prefix = formula.parent()?.parent()?;
    let stable = prefix.join("bin").join(current.file_name()?);
    (stable.canonicalize().ok()? == current).then_some(stable)
}

fn cfg_path(home: &Path) -> PathBuf {
    home.join("fetchira.toml")
}

pub(crate) fn load_or_empty(home: &Path) -> anyhow::Result<Config> {
    let path = cfg_path(home);
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).with_context(|| format!("parse {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

async fn open_store(home: &Path, cfg: &Config) -> anyhow::Result<Store> {
    Ok(Store::open(&config::resolve_db(home, &cfg.db_path)).await?)
}

fn parse_provider(s: &str) -> anyhow::Result<ProviderKind> {
    serde_json::from_value::<ProviderKind>(serde_json::Value::String(s.to_string()))
        .with_context(|| format!("unknown provider '{s}' (try `fetchira providers`)"))
}

pub fn flag_value(args: &mut impl Iterator<Item = String>, flag: &str) -> anyhow::Result<String> {
    args.next()
        .filter(|s| !s.starts_with("--") && !s.is_empty())
        .with_context(|| format!("missing value for {flag}"))
}

fn prompt(msg: &str) -> String {
    print!("{msg}");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
    s.trim().to_string()
}

fn pause(msg: &str) {
    if !msg.is_empty() {
        println!("{msg}");
    }
    prompt("(press Enter to continue) ");
}

/// Next free `provider-N` label (web providers drop the `_web` suffix: gemini-1, grok-2, …).
fn default_label(cfg: &Config, kind: ProviderKind) -> String {
    let base = kind.as_str().trim_end_matches("_web");
    (1..)
        .map(|n| format!("{base}-{n}"))
        .find(|l| !cfg.accounts.iter().any(|a| &a.label == l))
        .unwrap()
}

pub fn providers() {
    println!("Available providers:\n");
    for k in ProviderKind::all() {
        let cred = if k.is_web() {
            "browser login"
        } else {
            "API key"
        };
        println!("  {:15} {:14} {}", k.as_str(), cred, k.blurb());
    }
    println!("\nAdd one with:  fetchira add <provider>");
    println!("Guided setup:  fetchira setup");
}

pub async fn list(home: &Path) -> anyhow::Result<()> {
    let cfg = load_or_empty(home)?;
    if cfg.accounts.is_empty() {
        println!("No accounts yet. Run `fetchira` to set up in the dashboard.");
        return Ok(());
    }
    let store = open_store(home, &cfg).await?;
    // Render REMAINING/RESEARCH from the same live snapshot the dashboard uses, so a lapsed sub
    // reads 0/0 here too (a bucket the router couldn't build — no session/key — is absent = "-").
    let accounts = cfg.accounts.clone();
    let router = crate::router::Router::build(cfg, store).await?;
    let views = router.usage_snapshot().await?;
    let mut mains: HashMap<&str, &crate::router::UsageView> = HashMap::new();
    let mut drs: HashMap<&str, &crate::router::UsageView> = HashMap::new();
    for v in &views {
        match v.label.strip_suffix("#dr") {
            Some(base) => drs.insert(base, v),
            None => mains.insert(v.label.as_str(), v),
        };
    }

    println!(
        "{:15} {:14} {:12} {:10} {:>10} {:>13}  PROXY",
        "PROVIDER", "LABEL", "CRED", "PLAN", "REMAINING", "RESEARCH"
    );
    for a in &accounts {
        let main = mains.get(a.label.as_str());
        let ready = main.is_some();
        let cred = if a.provider.is_web() {
            if ready {
                "session"
            } else {
                "NEEDS LOGIN"
            }
        } else if ready {
            "key"
        } else {
            "NO KEY"
        };
        // Subscription badge for web providers that report it (chatgpt Plus/Max/free, grok pro/free);
        // "-" until the live limits land or for providers without a plan concept.
        let plan = main
            .and_then(|v| v.limits.as_ref())
            .and_then(|l| l.tier.as_deref())
            .unwrap_or("-");
        let live = main.and_then(|v| v.limits.as_ref()).is_some();
        let remaining = list_remaining(a.provider.is_web(), live, main.map(|v| v.remaining));
        let research = list_research(
            a.provider.is_web(),
            live,
            drs.get(a.label.as_str()).map(|v| (v.remaining, v.quota)),
        );
        println!(
            "{:15} {:14} {:12} {:10} {:>10} {:>13}  {}",
            a.provider.as_str(),
            a.label,
            cred,
            plan,
            remaining,
            research,
            a.proxy.as_deref().unwrap_or("direct"),
        );
    }

    // Live per-tier tool limits + model catalog, fetched straight from the providers that report
    // them (chatgpt tier/features, grok per-mode limits, gemini model list).
    for v in &views {
        let Some(ll) = &v.limits else { continue };
        println!(
            "\nlive limits — {} ({})",
            v.label,
            ll.tier.as_deref().unwrap_or("?")
        );
        for f in &ll.features {
            let reset = f
                .reset_after
                .as_deref()
                .map(|s| format!("  · resets {}", &s[..s.len().min(10)]))
                .unwrap_or_default();
            println!("  {:20} {:>5}{}", f.feature, f.remaining, reset);
        }
        for m in &ll.models {
            let cap = match (m.remaining, m.total) {
                (Some(r), Some(t)) => format!("{r}/{t}"),
                (Some(r), None) => r.to_string(),
                _ => "—".to_string(),
            };
            let levels = if m.levels.is_empty() {
                String::new()
            } else {
                format!(" [{}]", m.levels.join("/"))
            };
            let window = m
                .window_secs
                .map(|w| format!("  · {}", dur(w)))
                .unwrap_or_default();
            let lock = if m.locked { "  (locked)" } else { "" };
            println!("  · {:12} {:>7}{levels}{window}{lock}", m.name, cap);
        }
    }
    Ok(())
}

/// Coarse duration for a rolling window ("24h", "2h").
fn dur(secs: i64) -> String {
    if secs >= 3600 {
        format!("{}h", secs / 3600)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

fn list_remaining(web: bool, live: bool, remaining: Option<i64>) -> String {
    if web && !live {
        return "-".into();
    }
    remaining
        .map(|n| n.to_string())
        .unwrap_or_else(|| "-".into())
}

fn list_research(web: bool, live: bool, dr: Option<(i64, i64)>) -> String {
    if !web || !live {
        return "-".into();
    }
    match dr {
        Some((rem, quota)) => format!("{rem}/{quota}/day"),
        None => "-".into(),
    }
}

/// True once the account is usable: a key that resolves to a non-empty secret, or a captured web
/// session. Until then its quota is meaningless, so the listings show "-" instead of a number.
async fn account_ready(store: &Store, a: &Account) -> bool {
    if a.provider.is_web() {
        matches!(store.load_session(&a.label).await, Ok(Some(_)))
    } else {
        a.api_key
            .as_deref()
            .and_then(|s| config::resolve_secret(s).ok())
            .is_some_and(|k| !k.trim().is_empty())
    }
}

/// Remaining chat/search quota for this period, or "-" until the account is authorized.
async fn remaining_cell(store: &Store, a: &Account, ready: bool) -> String {
    if !ready {
        return "-".to_string();
    }
    let quota = a.quota.unwrap_or_else(|| a.provider.default_quota());
    let period = period_key(a.reset.unwrap_or_else(|| a.provider.default_reset()));
    store
        .remaining(&a.label, quota, &period)
        .await
        .unwrap_or(quota)
        .to_string()
}

/// Deep-research budget with its configured reset period, or "-" until authorized.
async fn research_cell(store: &Store, a: &Account, ready: bool) -> String {
    if !ready || !a.provider.is_web() {
        return "-".to_string();
    }
    let dq = a.dr_quota.unwrap_or_else(|| a.provider.dr_quota());
    let reset = a.dr_reset.unwrap_or_else(|| a.provider.dr_reset());
    let period = period_key(reset);
    let dr = store
        .remaining(&format!("{}#dr", a.label), dq, &period)
        .await
        .unwrap_or(dq);
    let unit = match reset {
        crate::config::Reset::Daily => "day",
        crate::config::Reset::Monthly => "month",
        crate::config::Reset::Once => "one-time",
    };
    format!("{dr}/{dq}/{unit}")
}

pub async fn add(home: &Path, mut args: impl Iterator<Item = String>) -> anyhow::Result<()> {
    let provider = args
        .next()
        .context("usage: fetchira add <provider> [--label L] [--key K] [--proxy pool|URL]")?;
    let kind = parse_provider(&provider)?;
    let (mut label, mut key, mut proxy) = (None, None, None);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--label" => label = Some(flag_value(&mut args, &flag)?),
            "--key" => key = Some(flag_value(&mut args, &flag)?),
            "--proxy" => proxy = Some(flag_value(&mut args, &flag)?),
            other => bail!("unknown flag '{other}'"),
        }
    }

    if !kind.is_web() && key.is_none() {
        if !kind.signup().is_empty() {
            println!("Get a key: {}", kind.signup());
        }
        let k = prompt(&format!("{} API key: ", kind.as_str()));
        if k.is_empty() {
            bail!("no key given");
        }
        key = Some(k);
    }

    let label = add_account(home, kind, label.as_deref(), key, proxy)?;
    println!("added {} account '{label}'", kind.as_str());

    if kind.is_web() {
        let cfg = load_or_empty(home)?;
        // Best-effort: the account is saved either way. On a headless box with no browser,
        // tell the user how to attach the session by hand.
        match do_login(home, &cfg, kind, &label, None).await {
            Ok(()) => {
                if let Ok(Some((other, id))) = identity_dup(home, kind, &label).await {
                    let _ = remove_account(home, &label).await;
                    bail!(
                        "that account ({id}) is already added as '{other}' — log in with a different one"
                    );
                }
            }
            Err(e) => {
                println!("login skipped: {e}");
                println!("attach a session manually:  fetchira session {label} < session.json");
            }
        }
    }
    Ok(())
}

/// Write a new account to the config (non-interactive). Shared by the CLI and the web UI.
/// Returns the resolved label.
pub fn add_account(
    home: &Path,
    kind: ProviderKind,
    label: Option<&str>,
    key: Option<String>,
    proxy: Option<String>,
) -> anyhow::Result<String> {
    let proxy = proxy.and_then(|p| parse_proxy_arg(&p));
    validate_proxy(&proxy)?;
    let mut cfg = load_or_empty(home)?;
    let label = match label {
        Some(l) if !l.trim().is_empty() => l.trim().to_string(),
        _ => default_label(&cfg, kind),
    };
    if cfg.accounts.iter().any(|a| a.label == label) {
        bail!("an account labelled '{label}' already exists");
    }
    if !kind.is_web() && key.as_deref().unwrap_or("").trim().is_empty() {
        bail!("{} needs an API key", kind.as_str());
    }
    // Reject re-adding the same API key under a new label — it's the same account/quota pool.
    if let Some(nk) = key.as_deref().and_then(|k| config::resolve_secret(k).ok()) {
        if let Some(dup) = cfg.accounts.iter().find(|a| {
            a.provider == kind
                && a.api_key
                    .as_deref()
                    .and_then(|k| config::resolve_secret(k).ok())
                    .as_deref()
                    == Some(nk.as_str())
        }) {
            bail!(
                "that {} key is already used by '{}'",
                kind.as_str(),
                dup.label
            );
        }
    }
    cfg.accounts.push(Account {
        provider: kind,
        label: label.clone(),
        api_key: key,
        proxy,
        quota: None,
        reset: None,
        dr_quota: None,
        dr_reset: None,
    });
    config::save(&cfg, &cfg_path(home))?;
    Ok(label)
}

pub async fn remove(home: &Path, label: Option<String>) -> anyhow::Result<()> {
    let label = label.context("usage: fetchira remove <label>")?;
    remove_account(home, &label).await?;
    println!("removed account '{label}'");
    Ok(())
}

/// Delete an account (config + its DB rows). Shared by the CLI and the web UI.
pub async fn remove_account(home: &Path, label: &str) -> anyhow::Result<()> {
    let mut cfg = load_or_empty(home)?;
    let before = cfg.accounts.len();
    cfg.accounts.retain(|a| a.label != label);
    if cfg.accounts.len() == before {
        bail!("no account labelled '{label}'");
    }
    config::save(&cfg, &cfg_path(home))?;
    open_store(home, &cfg).await?.delete_account(label).await?;
    Ok(())
}

/// Rename an account. The label is its identity across config + the DB, so migrate both. Shared
/// by the CLI and web UI.
pub async fn rename_account(home: &Path, old: &str, new: &str) -> anyhow::Result<()> {
    let new = new.trim();
    if new.is_empty() {
        bail!("new label is empty");
    }
    if new == old {
        return Ok(());
    }
    let mut cfg = load_or_empty(home)?;
    if !cfg.accounts.iter().any(|a| a.label == old) {
        bail!("no account labelled '{old}'");
    }
    if cfg.accounts.iter().any(|a| a.label == new) {
        bail!("an account labelled '{new}' already exists");
    }
    for a in cfg.accounts.iter_mut().filter(|a| a.label == old) {
        a.label = new.to_string();
    }
    config::save(&cfg, &cfg_path(home))?;
    open_store(home, &cfg)
        .await?
        .rename_account(old, new)
        .await?;
    Ok(())
}

/// Interpret a proxy string the user typed: "" / "direct" / "none" / "off" → direct (None),
/// "pool" → a sticky proxy from the pool, anything else → that specific proxy URL.
pub fn parse_proxy_arg(s: &str) -> Option<String> {
    match s.trim() {
        "" | "direct" | "none" | "off" => None,
        "pool" => Some("pool".to_string()),
        url => Some(crate::proxy::normalize(url)),
    }
}

/// Reject a malformed specific-proxy URL before it reaches the config. "pool"/direct pass through.
fn validate_proxy(proxy: &Option<String>) -> anyhow::Result<()> {
    if let Some(p) = proxy {
        if p != "pool" {
            crate::proxy::validate(p).with_context(|| format!("not a usable proxy: '{p}'"))?;
        }
    }
    Ok(())
}

/// Change (or clear) an existing account's proxy: `None` = direct, `Some("pool")` = sticky from the
/// pool, `Some(url)` = a specific proxy. Drops any sticky assignment so the change takes effect on
/// the next call. Shared by the CLI and web UI.
pub async fn set_proxy(home: &Path, label: &str, proxy: Option<String>) -> anyhow::Result<()> {
    validate_proxy(&proxy)?;
    let mut cfg = load_or_empty(home)?;
    let acc = cfg
        .accounts
        .iter_mut()
        .find(|a| a.label == label)
        .with_context(|| format!("no account labelled '{label}'"))?;
    acc.proxy = proxy;
    config::save(&cfg, &cfg_path(home))?;
    open_store(home, &cfg).await?.clear_proxy(label).await?;
    Ok(())
}

/// `fetchira proxy <label> <direct|pool|URL>` — change an account's proxy.
pub async fn proxy(home: &Path, mut args: impl Iterator<Item = String>) -> anyhow::Result<()> {
    let label = args
        .next()
        .context("usage: fetchira proxy <label> <direct|pool|URL>")?;
    let val = args
        .next()
        .context("usage: fetchira proxy <label> <direct|pool|URL>")?;
    if args.next().is_some() {
        bail!("usage: fetchira proxy <label> <direct|pool|URL>");
    }
    let proxy = parse_proxy_arg(&val);
    set_proxy(home, &label, proxy.clone()).await?;
    println!(
        "proxy for '{label}' → {}",
        proxy.as_deref().unwrap_or("direct")
    );
    Ok(())
}

/// The capabilities whose provider order is user-tunable (browser has a single backend).
pub const PRIORITY_CAPS: [Capability; 4] = [
    Capability::Search,
    Capability::Read,
    Capability::DeepResearch,
    Capability::Image,
];

/// Set (or clear, with an empty list) the provider order for one capability. Rejects providers
/// that don't serve it. Shared by the CLI and web UI. Routers read this at startup, so running
/// MCP servers keep the old order until restarted.
pub fn set_priority(home: &Path, cap: Capability, list: Vec<ProviderKind>) -> anyhow::Result<()> {
    if let Some(p) = list.iter().find(|p| !p.supports(cap)) {
        bail!("{} does not serve {}", p.as_str(), cap.as_str());
    }
    let mut cfg = load_or_empty(home)?;
    cfg.priority.set(cap, list);
    config::save(&cfg, &cfg_path(home))?;
    Ok(())
}

fn print_priority(cfg: &Config) {
    for cap in PRIORITY_CAPS {
        let custom = cfg.priority.for_cap(cap);
        let eff: Vec<&str> = order_for(cap, None, custom)
            .iter()
            .map(|p| p.as_str())
            .collect();
        let mark = if custom.is_empty() { ' ' } else { '*' };
        println!("{:<14}{mark} {}", cap.as_str(), eff.join(" → "));
    }
    println!("\n* custom order — set: fetchira priority <capability> <provider,provider,…>   reset: fetchira priority <capability> reset");
}

/// `fetchira priority` — show the per-capability provider order; `<cap> <p1,p2,…>` sets it,
/// `<cap> reset` restores the default.
pub fn priority(home: &Path, args: impl Iterator<Item = String>) -> anyhow::Result<()> {
    let mut vals = args.flat_map(|a| {
        a.split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>()
    });
    let Some(cap_arg) = vals.next() else {
        print_priority(&load_or_empty(home)?);
        return Ok(());
    };
    let cap = Capability::parse(&cap_arg)
        .filter(|c| PRIORITY_CAPS.contains(c))
        .with_context(|| {
            format!("unknown capability '{cap_arg}' (search, read, deep_research, image)")
        })?;
    let vals: Vec<String> = vals.collect();
    if vals.is_empty() {
        bail!(
            "usage: fetchira priority {0} <provider,provider,…>   or: fetchira priority {0} reset",
            cap.as_str()
        );
    }
    let list = match vals.as_slice() {
        [one] if one == "reset" || one == "default" => Vec::new(),
        _ => vals
            .iter()
            .map(|s| parse_provider(s))
            .collect::<anyhow::Result<Vec<_>>>()?,
    };
    set_priority(home, cap, list)?;
    print_priority(&load_or_empty(home)?);
    println!("\nsaved — applies to newly started fetchira processes (restart running MCP servers to pick it up)");
    Ok(())
}

/// If a web account's captured identity (email) already belongs to another account of the same
/// provider, returns (that other label, the identity) — so the add flow can reject a duplicate.
pub async fn identity_dup(
    home: &Path,
    kind: ProviderKind,
    label: &str,
) -> anyhow::Result<Option<(String, String)>> {
    let store = open_store(home, &load_or_empty(home)?).await?;
    let Some(id) = store.load_identity(label).await? else {
        return Ok(None);
    };
    Ok(store
        .identity_conflict(kind.as_str(), &id, label)
        .await?
        .map(|other| (other, id)))
}

/// Best-effort: record the account email (dashboard display + dup detection). Direct egress is
/// fine for this one-off identity read; the router uses the sticky proxy for real calls.
pub async fn record_identity(
    store: &Store,
    kind: ProviderKind,
    label: &str,
    raw_session: &str,
    proxy: Option<&str>,
) {
    let session = web::parse_session(raw_session);
    let proxy = proxy.filter(|p| p.starts_with("http"));
    if let Ok(client) = web::build_client(&session.cookies, &session.headers, proxy) {
        let p = crate::providers::Provider::new(kind);
        if let Some(id) = p.account_identity(&client).await {
            let _ = store.set_identity(label, &id).await;
        }
        if let Some(ll) = p.live_limits(&client, label, &session.cookies, false).await {
            if let Some(tier) = ll.tier.as_deref().filter(|t| *t != "free") {
                let _ = store.set_plan(label, tier).await;
            }
            if ll.has_quotas() {
                if let Ok(raw) = serde_json::to_string(&ll) {
                    let _ = store.set_limits(label, &raw).await;
                }
            }
        }
    }
}

/// Fill in missing emails for already-logged-in web/dashboard accounts so the UI shows them
/// without a forced re-login.
pub async fn backfill_identities(home: &Path, store: &Store) {
    let Ok(cfg) = load_or_empty(home) else { return };
    for a in &cfg.accounts {
        if !(a.provider.is_web() || a.provider.balance_session()) {
            continue;
        }
        if matches!(store.load_identity(&a.label).await, Ok(Some(_))) {
            continue;
        }
        let Ok(Some(raw)) = store.load_session(&a.label).await else {
            continue;
        };
        record_identity(store, a.provider, &a.label, &raw, a.proxy.as_deref()).await;
    }
}

/// Capture (or re-capture) a web session for an existing account. Shared by CLI + web UI.
pub async fn capture_login(
    home: &Path,
    label: &str,
    browser: Option<String>,
) -> anyhow::Result<()> {
    let cfg = load_or_empty(home)?;
    let acc = cfg
        .accounts
        .iter()
        .find(|a| a.label == label)
        .with_context(|| format!("no account labelled '{label}'"))?;
    if !acc.provider.is_web() {
        bail!("'{}' is not a web-session provider", acc.provider.as_str());
    }
    let kind = acc.provider;
    do_login(home, &cfg, kind, label, browser).await
}

/// Validate a session JSON captured elsewhere (cookies exported from any browser) and store it
/// for an existing web account. Returns the cookie count. Shared by the CLI and web UI.
pub async fn set_session(home: &Path, label: &str, raw: &str) -> anyhow::Result<usize> {
    let cfg = load_or_empty(home)?;
    let acc = cfg
        .accounts
        .iter()
        .find(|a| a.label == label)
        .with_context(|| {
            format!("no account labelled '{label}' — add one first with `fetchira add <provider> --label {label}`")
        })?;
    if !acc.provider.is_web() {
        bail!("'{}' is not a web-session provider", acc.provider.as_str());
    }
    let session = web::parse_session(raw);
    if session.cookies.is_empty() {
        bail!("no cookies found — expected a cookie array or {{\"cookies\":[…]}}");
    }
    let n = session.cookies.len();
    open_store(home, &cfg)
        .await?
        .save_session(
            label,
            acc.provider.as_str(),
            &serde_json::to_string(&session)?,
        )
        .await?;
    Ok(n)
}

/// `fetchira session <label> [--file PATH]` — attach a web session by hand (JSON on stdin if no
/// `--file`). The escape hatch when no browser is available, e.g. a headless server.
pub async fn session(home: &Path, mut args: impl Iterator<Item = String>) -> anyhow::Result<()> {
    let label = args.next().context(
        "usage: fetchira session <label> [--file PATH]   (reads JSON from stdin otherwise)",
    )?;
    let mut file = None;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--file" | "-f" => file = Some(flag_value(&mut args, &flag)?),
            other => bail!("unknown flag '{other}'"),
        }
    }
    let raw = match file {
        Some(p) => std::fs::read_to_string(&p).with_context(|| format!("read {p}"))?,
        None => {
            std::io::read_to_string(std::io::stdin()).context("read session JSON from stdin")?
        }
    };
    let n = set_session(home, &label, &raw).await?;
    println!("session '{label}' set ({n} cookies)");
    Ok(())
}

/// `fetchira login <provider|label>` — capture (or re-capture) a web session.
pub async fn login(home: &Path, who: Option<String>) -> anyhow::Result<()> {
    let who = who.context("usage: fetchira login <provider|label>")?;
    let cfg = load_or_empty(home)?;
    let acc = cfg
        .accounts
        .iter()
        .find(|a| a.label == who)
        .or_else(|| {
            cfg.accounts.iter().find(|a| {
                (a.provider.is_web() || a.provider.balance_session()) && a.provider.as_str() == who
            })
        })
        .with_context(|| {
            format!("no web account matching '{who}' — add one with `fetchira add {who}`")
        })?;
    if !acc.provider.is_web() && !acc.provider.balance_session() {
        bail!("'{}' is not a web-session provider", acc.provider.as_str());
    }
    do_login(home, &cfg, acc.provider, &acc.label, None).await
}

async fn do_login(
    home: &Path,
    cfg: &Config,
    kind: ProviderKind,
    label: &str,
    browser: Option<String>,
) -> anyhow::Result<()> {
    let store = open_store(home, cfg).await?;
    let seed = match store.load_session(label).await {
        Ok(Some(raw)) => Some(web::parse_session(&raw)),
        _ => None,
    };
    if kind == ProviderKind::GrokWeb {
        println!(
            "opening a browser to log into {} ({label}) — complete Apple passkey/Touch ID; the window closes once the grok.com session is stored…",
            kind.as_str()
        );
    } else {
        println!(
            "opening a browser to log into {} ({label}) — finish login; the window closes itself once you're in…",
            kind.as_str()
        );
    }
    let session = web::login(
        home,
        kind,
        label,
        browser,
        seed.as_ref().map(|s| s.cookies.as_slice()),
    )
    .await?;
    let raw = serde_json::to_string(&session)?;
    store.save_session(label, kind.as_str(), &raw).await?;
    let proxy = cfg
        .accounts
        .iter()
        .find(|a| a.label == label)
        .and_then(|a| a.proxy.as_deref());
    record_identity(&store, kind, label, &raw, proxy).await;
    println!(
        "captured {} cookies; session '{label}' ready",
        session.cookies.len()
    );
    Ok(())
}

/// A provider menu entry that renders nicely but carries the kind.
struct PChoice(ProviderKind);
impl std::fmt::Display for PChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tag = if self.0.is_web() { "login" } else { " key " };
        write!(f, "{:14} [{tag}]  {}", self.0.as_str(), self.0.blurb())
    }
}

/// Interactive TUI: a menu of arrow-key actions over a live status board. Esc / Quit exits.
pub async fn setup(home: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(home).ok();
    let mode = match Select::new(
        "Where should Fetchira run? (Esc to exit)",
        vec![SetupMode::Local, SetupMode::Hosted],
    )
    .prompt()
    {
        Ok(mode) => mode,
        Err(_) => return Ok(()),
    };
    if mode == SetupMode::Hosted {
        setup_hosted(home).await?;
        install_tools(home)?;
        return Ok(());
    }
    let cfg = load_or_empty(home)?;
    if cfg.remote.endpoint.is_some() || cfg.remote.api_key.is_some() {
        let confirmed =
            Confirm::new("Clear the saved hosted server URL and API key and use local accounts?")
                .with_default(false)
                .prompt()
                .unwrap_or(false);
        if !confirmed {
            println!("Kept the hosted connection; setup was not changed.");
            return Ok(());
        }
    }
    use_local_config(home)?;
    loop {
        print_status(home).await?;
        let action = Select::new(
            "Manage providers — ↑/↓ to move, Enter to pick, Esc to exit:",
            vec![
                "Add an account",
                "Log in / re-login a web provider",
                "Change an account's proxy",
                "Remove an account",
                "Quit",
            ],
        )
        .prompt();
        match action {
            Ok("Add an account") => add_flow(home).await?,
            Ok("Log in / re-login a web provider") => login_flow(home).await?,
            Ok("Change an account's proxy") => proxy_flow(home).await?,
            Ok("Remove an account") => remove_flow(home).await?,
            _ => break, // Quit or Esc
        }
    }
    if Confirm::new("\nRegister fetchira into your coding tools now?")
        .with_default(true)
        .prompt()
        .unwrap_or(false)
    {
        install_tools(home)?;
    }
    println!(
        "\nConfigure anytime with `fetchira setup`.  Config: {}",
        home.display()
    );
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SetupMode {
    Local,
    Hosted,
}

impl std::fmt::Display for SetupMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local => write!(
                f,
                "On this computer \x1b[2m— use local provider accounts\x1b[0m"
            ),
            Self::Hosted => write!(
                f,
                "Connect to a server \x1b[2m— use its URL and API key\x1b[0m"
            ),
        }
    }
}

fn use_local_config(home: &Path) -> anyhow::Result<()> {
    let mut cfg = load_or_empty(home)?;
    if cfg.remote.endpoint.is_some() || cfg.remote.api_key.is_some() {
        cfg.remote = Default::default();
        config::save(&cfg, &cfg_path(home))?;
        println!("Using local provider accounts; hosted connection cleared.");
    }
    Ok(())
}

async fn setup_hosted(home: &Path) -> anyhow::Result<()> {
    let endpoint = Text::new("Hosted MCP endpoint (for example, https://host.example/mcp):")
        .prompt()
        .context("hosted endpoint is required")?;
    let api_key = Password::new("Hosted API key:")
        .without_confirmation()
        .prompt()
        .context("hosted API key is required")?;
    let message = configure_remote(home, &endpoint, &api_key).await?;
    println!("✓ {message}");
    Ok(())
}

/// Verify a hosted endpoint and key before replacing the remote connection in the config.
/// Provider accounts stay untouched so switching between local and hosted is reversible.
pub async fn configure_remote(
    home: &Path,
    endpoint: &str,
    api_key: &str,
) -> anyhow::Result<String> {
    let endpoint = endpoint.trim();
    let api_key = api_key.trim();
    if endpoint.is_empty() {
        bail!("hosted endpoint is empty");
    }
    crate::remote::validate_api_key(api_key)?;
    let mut cfg = load_or_empty(home)?;
    cfg.remote.endpoint = Some(endpoint.to_string());
    cfg.remote.api_key = Some(api_key.to_string());
    let message = crate::remote::verify(&cfg).await?;
    // `remote::set` normalizes the endpoint using the same parser as the transport and reloads
    // the existing config, preserving all local accounts and unrelated settings.
    crate::remote::set(home, endpoint.to_string(), Some(api_key.to_string()))?;
    Ok(message)
}

pub fn use_local(home: &Path) -> anyhow::Result<()> {
    use_local_config(home)
}

/// Clear the screen and print the current accounts + remaining quota (the TUI's status board).
async fn print_status(home: &Path) -> anyhow::Result<()> {
    print!("\x1B[2J\x1B[H");
    let _ = std::io::stdout().flush();
    println!("fetchira — your providers\n");
    let cfg = load_or_empty(home)?;
    if cfg.accounts.is_empty() {
        println!("  (nothing configured yet — pick \"Add an account\" below)\n");
        return Ok(());
    }
    let store = open_store(home, &cfg).await?;
    println!(
        "  {:14} {:14} {:12} {:>10} {:>13}",
        "PROVIDER", "LABEL", "STATUS", "REMAINING", "RESEARCH"
    );
    for a in &cfg.accounts {
        let ready = account_ready(&store, a).await;
        let status = if a.provider.is_web() {
            if ready {
                "logged in"
            } else {
                "needs login"
            }
        } else if ready {
            "key set"
        } else {
            "no key"
        };
        println!(
            "  {:14} {:14} {:12} {:>10} {:>13}",
            a.provider.as_str(),
            a.label,
            status,
            remaining_cell(&store, a, ready).await,
            research_cell(&store, a, ready).await
        );
    }
    println!();
    Ok(())
}

async fn add_flow(home: &Path) -> anyhow::Result<()> {
    let mut cfg = load_or_empty(home)?;
    let choices: Vec<PChoice> = ProviderKind::all().iter().map(|&k| PChoice(k)).collect();
    let kind = match Select::new("Add which provider? (Esc to cancel)", choices).prompt() {
        Ok(c) => c.0,
        Err(_) => return Ok(()),
    };
    let label = Text::new("Label:")
        .with_default(&default_label(&cfg, kind))
        .prompt()
        .unwrap_or_else(|_| default_label(&cfg, kind));
    if cfg.accounts.iter().any(|a| a.label == label) {
        pause(&format!("'{label}' already exists."));
        return Ok(());
    }
    let mut api_key = None;
    if !kind.is_web() {
        if !kind.signup().is_empty() {
            println!("  get a key: {}", kind.signup());
        }
        match Text::new(&format!("{} API key:", kind.as_str())).prompt() {
            Ok(k) if !k.trim().is_empty() => api_key = Some(k.trim().to_string()),
            _ => return Ok(()),
        }
    }
    let proxy = choose_proxy(&cfg).await;
    if let Err(e) = validate_proxy(&proxy) {
        pause(&format!("✗ {e}"));
        return Ok(());
    }
    cfg.accounts.push(Account {
        provider: kind,
        label: label.clone(),
        api_key,
        proxy,
        quota: None,
        reset: None,
        dr_quota: None,
        dr_reset: None,
    });
    config::save(&cfg, &cfg_path(home))?;
    if kind.is_web()
        && Confirm::new(&format!(
            "Open a browser to log into {} now?",
            kind.as_str()
        ))
        .with_default(true)
        .prompt()
        .unwrap_or(false)
    {
        do_login(home, &cfg, kind, &label, None).await?;
        pause("");
    } else {
        pause(&format!("✓ added '{label}'"));
    }
    Ok(())
}

/// Arrow-key proxy picker: direct, sticky-from-pool, or a specific URL (typed or chosen from the pool).
async fn choose_proxy(cfg: &Config) -> Option<String> {
    match Select::new(
        "Proxy for this account:",
        vec![
            "Direct — no proxy",
            "Sticky proxy from the pool (recommended for multi-account)",
            "A specific proxy",
        ],
    )
    .prompt()
    {
        Ok("Sticky proxy from the pool (recommended for multi-account)") => Some("pool".into()),
        Ok("A specific proxy") => specific_proxy(cfg).await,
        _ => None, // Direct or Esc
    }
}

async fn specific_proxy(cfg: &Config) -> Option<String> {
    let pool = pool_proxies(cfg).await;
    let mut opts = vec!["Enter a URL manually".to_string()];
    opts.extend(pool.iter().map(|p| host_port(p)));
    let sel = match Select::new("Which proxy?", opts.clone()).prompt() {
        Ok(s) => s,
        Err(_) => return None,
    };
    let idx = opts.iter().position(|o| o == &sel).unwrap_or(0);
    if idx == 0 {
        Text::new("Proxy (ip:port:user:pass or http://user:pass@host:port):")
            .prompt()
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    } else {
        pool.get(idx - 1).cloned()
    }
}

async fn pool_proxies(cfg: &Config) -> Vec<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .unwrap_or_default();
    crate::proxy::resolve_pool(&cfg.proxy_pool, &client)
        .await
        .unwrap_or_default()
}

/// Strip credentials for display: "http://u:p@1.2.3.4:8080" -> "1.2.3.4:8080".
fn host_port(url: &str) -> String {
    url.rsplit('@').next().unwrap_or(url).to_string()
}

/// Pick an account, then a new proxy (direct / sticky pool / specific). Esc keeps the current one.
async fn proxy_flow(home: &Path) -> anyhow::Result<()> {
    let cfg = load_or_empty(home)?;
    if cfg.accounts.is_empty() {
        pause("No accounts yet.");
        return Ok(());
    }
    let labels: Vec<String> = cfg.accounts.iter().map(|a| a.label.clone()).collect();
    let label =
        match Select::new("Change proxy for which account? (Esc to cancel)", labels).prompt() {
            Ok(l) => l,
            Err(_) => return Ok(()),
        };
    let proxy = match Select::new(
        "New proxy: (Esc to keep current)",
        vec![
            "Direct — no proxy",
            "Sticky proxy from the pool",
            "A specific proxy",
        ],
    )
    .prompt()
    {
        Ok("Sticky proxy from the pool") => Some("pool".to_string()),
        Ok("A specific proxy") => specific_proxy(&cfg).await,
        Ok(_) => None,           // Direct
        Err(_) => return Ok(()), // Esc → keep current
    };
    match set_proxy(home, &label, proxy.clone()).await {
        Ok(()) => pause(&format!(
            "✓ proxy for '{label}' → {}",
            proxy.as_deref().unwrap_or("direct")
        )),
        Err(e) => pause(&format!("✗ {e}")),
    }
    Ok(())
}

async fn login_flow(home: &Path) -> anyhow::Result<()> {
    let cfg = load_or_empty(home)?;
    let web: Vec<(ProviderKind, String)> = cfg
        .accounts
        .iter()
        .filter(|a| a.provider.is_web())
        .map(|a| (a.provider, a.label.clone()))
        .collect();
    if web.is_empty() {
        pause("No web accounts yet — add gemini_web / grok_web / chatgpt_web first.");
        return Ok(());
    }
    let labels: Vec<String> = web
        .iter()
        .map(|(k, l)| format!("{l}  ({})", k.as_str()))
        .collect();
    let sel = match Select::new("Log into which? (Esc to cancel)", labels.clone()).prompt() {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };
    let idx = labels.iter().position(|l| l == &sel).unwrap_or(0);
    let (kind, label) = web[idx].clone();
    do_login(home, &cfg, kind, &label, None).await?;
    pause("");
    Ok(())
}

async fn remove_flow(home: &Path) -> anyhow::Result<()> {
    let mut cfg = load_or_empty(home)?;
    if cfg.accounts.is_empty() {
        pause("Nothing to remove.");
        return Ok(());
    }
    let labels: Vec<String> = cfg
        .accounts
        .iter()
        .map(|a| format!("{}  ({})", a.label, a.provider.as_str()))
        .collect();
    let sel = match Select::new("Remove which? (Esc to cancel)", labels.clone()).prompt() {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };
    let idx = labels.iter().position(|l| l == &sel).unwrap_or(0);
    let label = cfg.accounts[idx].label.clone();
    if !Confirm::new(&format!("Remove '{label}'?"))
        .with_default(false)
        .prompt()
        .unwrap_or(false)
    {
        return Ok(());
    }
    cfg.accounts.retain(|a| a.label != label);
    config::save(&cfg, &cfg_path(home))?;
    open_store(home, &cfg).await?.delete_account(&label).await?;
    pause(&format!("✓ removed '{label}'"));
    Ok(())
}

// ── Register the MCP server into coding tools ──────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Integration {
    Both,
    Mcp,
    Cli,
    Skip,
}

impl std::fmt::Display for Integration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Both => write!(
                f,
                "MCP with CLI fallback \x1b[2m— register MCP and install both transports\x1b[0m"
            ),
            Self::Mcp => write!(
                f,
                "MCP \x1b[2m— register MCP and install the MCP skill\x1b[0m"
            ),
            Self::Cli => write!(
                f,
                "CLI only \x1b[2m— install the shell skill without MCP\x1b[0m"
            ),
            Self::Skip => write!(f, "Later \x1b[2m— leave integrations unchanged\x1b[0m"),
        }
    }
}

pub(crate) struct IntegrationResult {
    pub(crate) name: String,
    pub(crate) ok: bool,
    pub(crate) msg: String,
}

/// Install the selected skill and, when explicitly requested for CLI-only setup, remove the
/// selected agents' existing Fetchira MCP registrations after their skill installation succeeds.
pub(crate) fn install_integrations_with_options(
    agent_home: &Path,
    config_home: &Path,
    bin: &str,
    target_names: &[String],
    skill: Option<crate::skills::SkillVariant>,
    remove_mcp: bool,
) -> anyhow::Result<Vec<IntegrationResult>> {
    if skill == Some(crate::skills::SkillVariant::Skip) {
        return Ok(Vec::new());
    }
    if remove_mcp && skill != Some(crate::skills::SkillVariant::Cli) {
        bail!("MCP conversion is available only with the CLI-only skill");
    }
    let all = mcp_target_list();
    let mut targets = Vec::with_capacity(target_names.len());
    for name in target_names {
        let target = all
            .iter()
            .find(|target| target.name == name)
            .with_context(|| format!("unknown install target '{name}'"))?;
        if remove_mcp && !agent_skill_supported(target.name) {
            bail!("MCP conversion is unavailable for {}", target.name);
        }
        targets.push(target);
    }
    if let Some(variant) = skill {
        crate::skills::preflight_skills_for_agents(agent_home, variant, target_names)?;
    }
    let mut results = Vec::new();
    if !matches!(skill, Some(crate::skills::SkillVariant::Cli)) {
        for target in &targets {
            match (target.run)(config_home, bin) {
                Ok(msg) => results.push(IntegrationResult {
                    name: target.name.to_string(),
                    ok: true,
                    msg,
                }),
                Err(e) => results.push(IntegrationResult {
                    name: target.name.to_string(),
                    ok: false,
                    msg: e.to_string(),
                }),
            }
        }
    }
    let skill_results = if let Some(variant) = skill {
        let skill_results = crate::skills::install_skills_for_agents(
            agent_home,
            config_home,
            Path::new(bin),
            variant,
            target_names,
        );
        results.extend(skill_results.iter().map(|result| IntegrationResult {
            name: format!("skill:{}", result.name),
            ok: result.ok,
            msg: result.msg.clone(),
        }));
        skill_results
    } else {
        Vec::new()
    };
    if remove_mcp {
        let shared_root = agent_home
            .canonicalize()
            .unwrap_or_else(|_| agent_home.to_path_buf())
            .join(".agents");
        let shared_codex = crate::skills::skill_destinations(agent_home)
            .iter()
            .any(|destination| destination.name == "Codex" && destination.parent == shared_root);
        for target in targets {
            if !target.installed {
                continue;
            }
            if !skill_succeeded_for_target(target.name, &skill_results, shared_codex) {
                results.push(IntegrationResult {
                    name: format!("mcp:{}", target.name),
                    ok: false,
                    msg: "kept because the CLI skill was not installed".into(),
                });
                continue;
            }
            let Some(remove) = target.remove.as_ref() else {
                results.push(IntegrationResult {
                    name: format!("mcp:{}", target.name),
                    ok: false,
                    msg: "MCP removal is unavailable for this agent".into(),
                });
                continue;
            };
            match remove(agent_home) {
                Ok(msg) => results.push(IntegrationResult {
                    name: format!("mcp:{}", target.name),
                    ok: true,
                    msg,
                }),
                Err(e) => results.push(IntegrationResult {
                    name: format!("mcp:{}", target.name),
                    ok: false,
                    msg: e.to_string(),
                }),
            }
        }
    }
    Ok(results)
}

fn skill_succeeded_for_target(
    target: &str,
    results: &[crate::skills::SkillInstallResult],
    shared_codex: bool,
) -> bool {
    let destinations: &[&str] = match target {
        "Claude Code" => &["Claude"],
        "Codex CLI" => &["Codex"],
        "Gemini CLI" if shared_codex => &["Codex"],
        "Gemini CLI" => &["Gemini"],
        "Cursor" => &["Cursor"],
        _ => &[],
    };
    results.iter().any(|result| {
        destinations.contains(&result.name) && result.ok && !result.msg.starts_with("skipped:")
    })
}

/// `fetchira install` — choose one agent integration, then select detected MCP targets if needed.
pub fn install_tools(config_home: &Path) -> anyhow::Result<()> {
    let user_home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("HOME is required to install agent skills")?;
    let mut variants: Vec<_> = crate::skills::skill_destinations(&user_home)
        .into_iter()
        .flat_map(|destination| destination.variants)
        .collect();
    variants.sort_by_key(|variant| variant.as_str());
    variants.dedup();
    let starting_cursor = match variants.as_slice() {
        [crate::skills::SkillVariant::Mcp] => 1,
        [crate::skills::SkillVariant::Cli] => 2,
        _ => 0,
    };
    let integration = match Select::new(
        "How should your agents use Fetchira? (Esc to exit)",
        vec![
            Integration::Both,
            Integration::Mcp,
            Integration::Cli,
            Integration::Skip,
        ],
    )
    .with_starting_cursor(starting_cursor)
    .prompt()
    {
        Ok(choice) => choice,
        Err(_) => return Ok(()),
    };
    if matches!(integration, Integration::Skip) {
        println!("Skipped; no MCP registrations or skills were changed.");
        return Ok(());
    }

    let skill = match integration {
        Integration::Both => crate::skills::SkillVariant::Both,
        Integration::Mcp => crate::skills::SkillVariant::Mcp,
        Integration::Cli => crate::skills::SkillVariant::Cli,
        Integration::Skip => unreachable!(),
    };
    let bin = installation_binary()?;
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("HOME is required to install agent skills")?;
    let cli_only = matches!(integration, Integration::Cli);
    let targets = choose_install_targets(cli_only)?;
    let remove_mcp = cli_only && confirm_mcp_conversion(&targets);
    let results = install_integrations_with_options(
        &home,
        config_home,
        &bin,
        &targets,
        Some(skill),
        remove_mcp,
    )?;
    let mut changed = false;
    for result in results {
        println!(
            "  {} {:14} {}",
            if result.ok { "✓" } else { "✗" },
            result.name,
            result.msg
        );
        changed |= result.ok && !result.msg.starts_with("skipped:");
    }
    if changed {
        println!("\nRestart the agent to load the selected integrations.");
    }
    Ok(())
}

fn choose_install_targets(skill_only: bool) -> anyhow::Result<Vec<String>> {
    let targets = mcp_target_list();
    let targets: Vec<_> = targets
        .into_iter()
        .filter(|target| !skill_only || agent_skill_supported(target.name))
        .collect();
    let opts: Vec<String> = targets
        .iter()
        .map(|t| format!("{}{}", t.name, if t.present { "  (detected)" } else { "" }))
        .collect();
    let has_installed = targets.iter().any(|target| target.installed);
    let preselect: Vec<usize> = targets
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            if has_installed {
                t.installed
            } else {
                t.present
            }
        })
        .map(|(i, _)| i)
        .collect();

    let chosen = match MultiSelect::new(
        if skill_only {
            "Install the skill for which agents? (Space toggles)"
        } else {
            "Register MCP in which tools? (Space toggles)"
        },
        opts.clone(),
    )
    .with_default(&preselect)
    .prompt()
    {
        Ok(c) => c,
        Err(_) => return Ok(Vec::new()),
    };
    Ok(chosen
        .iter()
        .filter_map(|label| opts.iter().position(|option| option == label))
        .map(|index| targets[index].name.to_string())
        .collect())
}

fn confirm_mcp_conversion(target_names: &[String]) -> bool {
    let existing: Vec<&str> = mcp_target_list()
        .iter()
        .filter(|target| target_names.iter().any(|name| name == target.name))
        .filter(|target| target.installed)
        .map(|target| target.name)
        .collect();
    if existing.is_empty() {
        return false;
    }
    println!(
        "\nFetchira MCP registrations already exist for: {}.",
        existing.join(", ")
    );
    println!("Keeping them leaves those MCP registrations callable alongside the CLI skill.");
    Confirm::new("Remove these MCP registrations after the CLI skill is installed?")
        .with_default(false)
        .prompt()
        .unwrap_or(false)
}

fn agent_skill_supported(name: &str) -> bool {
    matches!(name, "Claude Code" | "Codex CLI" | "Gemini CLI" | "Cursor")
}

type RunFn = Box<dyn Fn(&Path, &str) -> anyhow::Result<String> + Send>;
type RemoveFn = Box<dyn Fn(&Path) -> anyhow::Result<String> + Send>;
type RepairFn = Box<dyn Fn(&Path, &str) -> anyhow::Result<String> + Send>;

pub(crate) struct McpTarget {
    pub(crate) name: &'static str,
    pub(crate) present: bool,
    /// This tool's config already registers a "fetchira" server (checklist done-state in the UI).
    pub(crate) installed: bool,
    pub(crate) run: RunFn,
    pub(crate) remove: Option<RemoveFn>,
    pub(crate) repair: Option<RepairFn>,
}

/// Detected coding tools + registration actions. Shared by `fetchira install` and the web UI.
pub(crate) fn mcp_target_list() -> Vec<McpTarget> {
    let h = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let appsup = h.join("Library/Application Support");
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| h.join(".config"));
    let codex = std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| h.join(".codex"));
    mcp_targets(&h, &appsup, &xdg, &codex)
}

fn has_fetchira(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    if path.extension().and_then(|ext| ext.to_str()) == Some("toml") {
        let Ok(doc) = toml::from_str::<toml::Value>(&text) else {
            return false;
        };
        return doc
            .get("mcp_servers")
            .and_then(|servers| servers.get("fetchira"))
            .is_some();
    }
    let Ok(doc) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    ["mcpServers", "servers", "mcp"].iter().any(|section| {
        doc.get(*section)
            .and_then(Value::as_object)
            .is_some_and(|servers| servers.contains_key("fetchira"))
    })
}

fn mac_or_xdg(mac: PathBuf, xdg: PathBuf) -> PathBuf {
    if cfg!(target_os = "macos") {
        mac
    } else {
        xdg
    }
}

fn mcp_targets(h: &Path, appsup: &Path, xdg: &Path, codex: &Path) -> Vec<McpTarget> {
    let p = |rel: &str| h.join(rel);
    let claude_dir = mac_or_xdg(appsup.join("Claude"), xdg.join("Claude"));
    let code_dir = mac_or_xdg(appsup.join("Code"), xdg.join("Code"));
    vec![
        McpTarget {
            name: "Claude Code",
            present: which("claude"),
            installed: has_fetchira(&p(".claude.json")),
            run: Box::new(reg_claude_code),
            remove: Some(boxed_remove(p(".claude.json"), remove_json_mcp_servers)),
            repair: Some(boxed_repair(p(".claude.json"), repair_json_mcp_servers)),
        },
        McpTarget {
            name: "Codex CLI",
            present: codex.exists() || which("codex"),
            installed: has_fetchira(&codex.join("config.toml")),
            run: boxed(codex.join("config.toml"), reg_codex),
            remove: Some(boxed_remove(
                codex.join("config.toml"),
                remove_codex_registration,
            )),
            repair: Some(boxed_repair(
                codex.join("config.toml"),
                repair_codex_registration,
            )),
        },
        McpTarget {
            name: "OpenCode",
            present: xdg.join("opencode").exists() || which("opencode"),
            installed: has_fetchira(&xdg.join("opencode/opencode.json")),
            run: boxed(xdg.join("opencode/opencode.json"), reg_opencode),
            remove: None,
            repair: Some(boxed_repair(
                xdg.join("opencode/opencode.json"),
                repair_opencode_registration,
            )),
        },
        McpTarget {
            name: "Gemini CLI",
            present: p(".gemini").exists() || which("gemini"),
            installed: has_fetchira(&p(".gemini/settings.json")),
            run: boxed(p(".gemini/settings.json"), reg_mcp_servers),
            remove: Some(boxed_remove(
                p(".gemini/settings.json"),
                remove_json_mcp_servers,
            )),
            repair: Some(boxed_repair(
                p(".gemini/settings.json"),
                repair_json_mcp_servers,
            )),
        },
        McpTarget {
            name: "Cursor",
            present: p(".cursor").exists(),
            installed: has_fetchira(&p(".cursor/mcp.json")),
            run: boxed(p(".cursor/mcp.json"), reg_mcp_servers),
            remove: Some(boxed_remove(p(".cursor/mcp.json"), remove_json_mcp_servers)),
            repair: Some(boxed_repair(p(".cursor/mcp.json"), repair_json_mcp_servers)),
        },
        McpTarget {
            name: "Windsurf",
            present: p(".codeium/windsurf").exists(),
            installed: has_fetchira(&p(".codeium/windsurf/mcp_config.json")),
            run: boxed(p(".codeium/windsurf/mcp_config.json"), reg_mcp_servers),
            remove: None,
            repair: Some(boxed_repair(
                p(".codeium/windsurf/mcp_config.json"),
                repair_json_mcp_servers,
            )),
        },
        McpTarget {
            name: "Claude Desktop",
            present: claude_dir.exists(),
            installed: has_fetchira(&claude_dir.join("claude_desktop_config.json")),
            run: boxed(
                claude_dir.join("claude_desktop_config.json"),
                reg_mcp_servers,
            ),
            remove: None,
            repair: Some(boxed_repair(
                claude_dir.join("claude_desktop_config.json"),
                repair_json_mcp_servers,
            )),
        },
        McpTarget {
            name: "VS Code",
            present: code_dir.exists(),
            installed: has_fetchira(&code_dir.join("User/mcp.json")),
            run: boxed(code_dir.join("User/mcp.json"), reg_vscode),
            remove: None,
            repair: Some(boxed_repair(
                code_dir.join("User/mcp.json"),
                repair_vscode_registration,
            )),
        },
    ]
}

fn boxed(path: PathBuf, f: fn(&Path, &Path, &str) -> anyhow::Result<String>) -> RunFn {
    Box::new(move |config_home, bin| f(&path, config_home, bin))
}

fn boxed_remove(path: PathBuf, f: fn(&Path, &Path) -> anyhow::Result<String>) -> RemoveFn {
    Box::new(move |backup_root| f(&path, backup_root))
}

fn boxed_repair(path: PathBuf, f: fn(&Path, &Path, &str) -> anyhow::Result<String>) -> RepairFn {
    Box::new(move |backup_root, bin| f(&path, backup_root, bin))
}

fn which(cmd: &str) -> bool {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|d| Path::new(d).join(cmd).exists())
}

fn read_obj(path: &Path) -> anyhow::Result<serde_json::Map<String, Value>> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .with_context(|| format!("parse {} (expected a JSON object)", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

fn write_obj(path: &Path, obj: &serde_json::Map<String, Value>) -> anyhow::Result<String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    config::write_atomic(
        path,
        &serde_json::to_string_pretty(&Value::Object(obj.clone()))?,
        false,
    )?;
    Ok(format!("wrote {}", path.display()))
}

fn readable_config(path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("refusing to modify symlink {}", path.display())
        }
        Ok(metadata) if !metadata.is_file() => {
            bail!("refusing to modify non-file {}", path.display())
        }
        Ok(_) => Ok(Some(
            std::fs::read(path).with_context(|| format!("read {}", path.display()))?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn backup_config(path: &Path, backup_root: &Path, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    match std::fs::symlink_metadata(backup_root) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("refusing symlink backup root {}", backup_root.display())
        }
        Ok(metadata) if !metadata.is_dir() => {
            bail!("backup root {} is not a directory", backup_root.display())
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(backup_root)
                .with_context(|| format!("create backup root {}", backup_root.display()))?;
        }
        Err(error) => {
            return Err(error).with_context(|| format!("inspect {}", backup_root.display()));
        }
    }
    let dir = backup_root.join(".fetchira-mcp-backups");
    match std::fs::symlink_metadata(&dir) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("refusing symlink backup directory {}", dir.display())
        }
        Ok(metadata) if !metadata.is_dir() => {
            bail!("backup directory {} is not a directory", dir.display())
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&dir)
                .with_context(|| format!("create backup directory {}", dir.display()))?;
        }
        Err(error) => return Err(error).with_context(|| format!("inspect {}", dir.display())),
    }

    let basename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config")
        .replace(
            |c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-',
            "_",
        );
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    for attempt in 0..100u32 {
        let destination = dir.join(format!(
            "{basename}-{}-{stamp}-{attempt}.bak",
            std::process::id()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&destination) {
            Ok(mut file) => {
                if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
                    let _ = std::fs::remove_file(&destination);
                    return Err(error)
                        .with_context(|| format!("write backup {}", destination.display()));
                }
                return Ok(destination);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("create backup {}", destination.display()));
            }
        }
    }
    bail!(
        "could not allocate a unique MCP backup name in {}",
        dir.display()
    )
}

fn removed_message(path: &Path, backup: &Path) -> String {
    format!(
        "removed Fetchira MCP registration from {}; backup: {}",
        path.display(),
        backup.display()
    )
}

fn remove_json_registration_in(
    path: &Path,
    backup_root: &Path,
    section: &str,
) -> anyhow::Result<String> {
    let Some(bytes) = readable_config(path)? else {
        return Ok(format!(
            "MCP registration already absent from {}",
            path.display()
        ));
    };
    let mut obj: serde_json::Map<String, Value> = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse {} (expected a JSON object)", path.display()))?;
    let Some(servers) = obj.get_mut(section) else {
        return Ok(format!(
            "MCP registration already absent from {}",
            path.display()
        ));
    };
    let servers = servers
        .as_object_mut()
        .with_context(|| format!("{section} must be a JSON object in {}", path.display()))?;
    if servers.remove("fetchira").is_none() {
        return Ok(format!(
            "MCP registration already absent from {}",
            path.display()
        ));
    }
    let backup = backup_config(path, backup_root, &bytes)?;
    write_obj(path, &obj)?;
    Ok(removed_message(path, &backup))
}

fn remove_json_mcp_servers(path: &Path, backup_root: &Path) -> anyhow::Result<String> {
    remove_json_registration_in(path, backup_root, "mcpServers")
}

fn remove_codex_registration(path: &Path, backup_root: &Path) -> anyhow::Result<String> {
    let Some(bytes) = readable_config(path)? else {
        return Ok(format!(
            "MCP registration already absent from {}",
            path.display()
        ));
    };
    let text = std::str::from_utf8(&bytes)
        .with_context(|| format!("parse {} as UTF-8 TOML", path.display()))?;
    let mut doc: toml::Table =
        toml::from_str(text).with_context(|| format!("parse {}", path.display()))?;
    let Some(servers) = doc.get_mut("mcp_servers") else {
        return Ok(format!(
            "MCP registration already absent from {}",
            path.display()
        ));
    };
    let servers = servers
        .as_table_mut()
        .with_context(|| format!("mcp_servers must be a TOML table in {}", path.display()))?;
    if servers.remove("fetchira").is_none() {
        return Ok(format!(
            "MCP registration already absent from {}",
            path.display()
        ));
    }
    let backup = backup_config(path, backup_root, &bytes)?;
    config::write_atomic(path, &toml::to_string_pretty(&doc)?, false)?;
    Ok(removed_message(path, &backup))
}

/// Repair only versioned launchers from this Homebrew installation, preserving all other options.
pub(crate) fn repair_mcp_launchers(backup_root: &Path, bin: &str) -> Vec<IntegrationResult> {
    mcp_target_list()
        .into_iter()
        .filter(|target| target.installed)
        .filter_map(|target| {
            let repair = target.repair?;
            let result = repair(backup_root, bin);
            if result.as_ref().is_ok_and(|message| message == "unchanged") {
                return None;
            }
            Some(IntegrationResult {
                name: target.name.into(),
                ok: result.is_ok(),
                msg: match result {
                    Ok(message) => message,
                    Err(error) => format!("{error:#}"),
                },
            })
        })
        .collect()
}

fn cellar_prefix(path: &Path) -> Option<&Path> {
    if path.file_name()? != "fetchira" || path.parent()?.file_name()? != "bin" {
        return None;
    }
    let formula = path.parent()?.parent()?.parent()?;
    if formula.file_name()? != "fetchira" || formula.parent()?.file_name()? != "Cellar" {
        return None;
    }
    formula.parent()?.parent()
}

fn needs_launcher_repair(command: &str, bin: &str) -> bool {
    let Ok(current) = Path::new(bin).canonicalize() else {
        return false;
    };
    let Some(old_prefix) =
        cellar_prefix(Path::new(command)).and_then(|path| path.canonicalize().ok())
    else {
        return false;
    };
    command != bin && Some(old_prefix.as_path()) == cellar_prefix(&current)
}

fn repair_json_registration(
    path: &Path,
    backup_root: &Path,
    bin: &str,
    section: &str,
) -> anyhow::Result<String> {
    let Some(bytes) = readable_config(path)? else {
        return Ok("unchanged".into());
    };
    let mut doc: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    let Some(server) = doc
        .get_mut(section)
        .and_then(|servers| servers.get_mut("fetchira"))
    else {
        return Ok("unchanged".into());
    };
    if server.get("url").is_some() {
        return Ok("unchanged".into());
    }
    let Some(command) = server.get_mut("command") else {
        return Ok("unchanged".into());
    };
    let command = if command.is_array() {
        let Some(first) = command.get_mut(0) else {
            return Ok("unchanged".into());
        };
        first
    } else {
        command
    };
    if !command
        .as_str()
        .is_some_and(|value| needs_launcher_repair(value, bin))
    {
        return Ok("unchanged".into());
    }
    *command = json!(bin);
    let backup = backup_config(path, backup_root, &bytes)?;
    config::write_atomic(path, &serde_json::to_string_pretty(&doc)?, false)?;
    Ok(format!(
        "updated Fetchira launcher in {}; backup: {}",
        path.display(),
        backup.display()
    ))
}

fn repair_json_mcp_servers(path: &Path, backup_root: &Path, bin: &str) -> anyhow::Result<String> {
    repair_json_registration(path, backup_root, bin, "mcpServers")
}
fn repair_opencode_registration(
    path: &Path,
    backup_root: &Path,
    bin: &str,
) -> anyhow::Result<String> {
    repair_json_registration(path, backup_root, bin, "mcp")
}
fn repair_vscode_registration(
    path: &Path,
    backup_root: &Path,
    bin: &str,
) -> anyhow::Result<String> {
    repair_json_registration(path, backup_root, bin, "servers")
}
fn repair_codex_registration(path: &Path, backup_root: &Path, bin: &str) -> anyhow::Result<String> {
    let Some(bytes) = readable_config(path)? else {
        return Ok("unchanged".into());
    };
    let mut doc: toml::Table = toml::from_str(std::str::from_utf8(&bytes)?)?;
    let Some(server) = doc
        .get_mut("mcp_servers")
        .and_then(|servers| servers.get_mut("fetchira"))
    else {
        return Ok("unchanged".into());
    };
    if server.get("url").is_some() {
        return Ok("unchanged".into());
    }
    let Some(command) = server.get_mut("command") else {
        return Ok("unchanged".into());
    };
    if !command
        .as_str()
        .is_some_and(|value| needs_launcher_repair(value, bin))
    {
        return Ok("unchanged".into());
    }
    *command = toml::Value::String(bin.into());
    let backup = backup_config(path, backup_root, &bytes)?;
    config::write_atomic(path, &toml::to_string_pretty(&doc)?, false)?;
    Ok(format!(
        "updated Fetchira launcher in {}; backup: {}",
        path.display(),
        backup.display()
    ))
}

fn registered_env(config_home: &Path) -> Value {
    json!({ "FETCHIRA_HOME": absolute_path(config_home) })
}

/// The common `{ "mcpServers": { "fetchira": { "command": … } } }` shape (Cursor, Windsurf,
/// Gemini CLI, Claude Desktop).
fn reg_mcp_servers(path: &Path, config_home: &Path, bin: &str) -> anyhow::Result<String> {
    let mut obj = read_obj(path)?;
    let servers = obj.entry("mcpServers").or_insert_with(|| json!({}));
    let m = servers
        .as_object_mut()
        .context("mcpServers must be a JSON object")?;
    let mut server = json!({ "command": bin });
    server["env"] = registered_env(config_home);
    m.insert("fetchira".into(), server);
    write_obj(path, &obj)
}

/// VS Code: `{ "servers": { "fetchira": { "type": "stdio", "command": … } } }`.
fn reg_vscode(path: &Path, config_home: &Path, bin: &str) -> anyhow::Result<String> {
    let mut obj = read_obj(path)?;
    let servers = obj.entry("servers").or_insert_with(|| json!({}));
    let m = servers
        .as_object_mut()
        .context("servers must be a JSON object")?;
    let mut server = json!({ "type": "stdio", "command": bin });
    server["env"] = registered_env(config_home);
    m.insert("fetchira".into(), server);
    write_obj(path, &obj)
}

/// OpenCode: `{ "mcp": { "fetchira": { "type": "local", "command": [bin], "enabled": true } } }`.
fn reg_opencode(path: &Path, config_home: &Path, bin: &str) -> anyhow::Result<String> {
    let mut obj = read_obj(path)?;
    obj.entry("$schema")
        .or_insert_with(|| json!("https://opencode.ai/config.json"));
    let mcp = obj.entry("mcp").or_insert_with(|| json!({}));
    let m = mcp.as_object_mut().context("mcp must be a JSON object")?;
    let mut server = json!({ "type": "local", "command": [bin], "enabled": true });
    server["environment"] = registered_env(config_home);
    m.insert("fetchira".into(), server);
    write_obj(path, &obj)
}

/// Codex CLI: TOML `[mcp_servers.fetchira] command = … args = []`.
fn reg_codex(path: &Path, config_home: &Path, bin: &str) -> anyhow::Result<String> {
    let mut doc: toml::Table = match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    let servers = doc
        .entry("mcp_servers")
        .or_insert_with(|| toml::Value::Table(Default::default()));
    let t = servers
        .as_table_mut()
        .context("mcp_servers must be a TOML table")?;
    let mut e = toml::Table::new();
    e.insert("command".into(), toml::Value::String(bin.to_string()));
    e.insert("args".into(), toml::Value::Array(vec![]));
    let mut env = toml::Table::new();
    env.insert(
        "FETCHIRA_HOME".into(),
        toml::Value::String(absolute_path(config_home).to_string_lossy().into_owned()),
    );
    e.insert("env".into(), toml::Value::Table(env));
    t.insert("fetchira".into(), toml::Value::Table(e));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    config::write_atomic(path, &toml::to_string_pretty(&doc)?, false)?;
    Ok(format!("wrote {}", path.display()))
}

/// Claude Code: use its CLI so the config + health check are handled natively.
fn reg_claude_code(config_home: &Path, bin: &str) -> anyhow::Result<String> {
    // Claude Code stores user-scoped MCP servers in ~/.claude.json. Update only Fetchira's entry
    // when the file exists; refusing an invalid file keeps unrelated or malformed user state
    // intact instead of handing it to a merge command that may rewrite the whole file.
    if let Some(path) = user_claude_config() {
        if path.is_file() {
            let mut obj = read_obj(&path).with_context(|| {
                format!(
                    "cannot refresh Claude MCP config {}; repair the JSON first",
                    path.display()
                )
            })?;
            let servers = obj
                .entry("mcpServers")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .with_context(|| {
                    format!(
                        "cannot refresh Claude MCP config {}; mcpServers must be an object",
                        path.display()
                    )
                })?;
            servers.insert(
                "fetchira".into(),
                json!({
                    "type": "stdio",
                    "command": bin,
                    "args": [],
                    "env": registered_env(config_home),
                }),
            );
            return write_obj(&path, &obj);
        }
    }

    // Fall back to Claude's own commands when its user config is absent or has an unknown shape.
    // Remove first so an existing entry cannot leave a stale executable behind. Keep a byte-for-
    // byte snapshot and restore it if the replacement fails.
    let config_path = user_claude_config();
    let original = config_path
        .as_ref()
        .filter(|path| path.is_file())
        .map(std::fs::read)
        .transpose()?;
    let existing = std::process::Command::new("claude")
        .args(["mcp", "get", "fetchira"])
        .output()
        .is_ok_and(|output| output.status.success());
    if existing {
        let removed = std::process::Command::new("claude")
            .args(["mcp", "remove", "fetchira", "-s", "user"])
            .output()?;
        if !removed.status.success() {
            bail!("claude mcp remove fetchira failed");
        }
    }
    let mut command = std::process::Command::new("claude");
    command.args(["mcp", "add", "fetchira", "-s", "user"]);
    let env = format!(
        "FETCHIRA_HOME={}",
        absolute_path(config_home).to_string_lossy()
    );
    command.args(["-e", &env]);
    let out = command.args(["--", bin]).output();
    match out {
        Ok(o) if o.status.success() => Ok("claude mcp add fetchira (user scope)".into()),
        Ok(o) => {
            if let (Some(path), Some(contents)) = (config_path, original) {
                let _ = std::fs::write(path, contents);
            }
            let err = String::from_utf8_lossy(&o.stderr);
            bail!("claude mcp add failed: {}", err.trim())
        }
        Err(error) => {
            if let (Some(path), Some(contents)) = (config_path, original) {
                let _ = std::fs::write(path, contents);
            }
            bail!("cannot run `claude mcp add`: {error}");
        }
    }
}

fn user_claude_config() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| PathBuf::from(home).join(".claude.json"))
}

pub fn help() {
    println!(
        "fetchira — quota-aware web search/scrape MCP server + CLI\n\n\
         USAGE:\n  \
           fetchira [serve]              run the MCP server (stdio) — the default when piped\n  \
           fetchira ui                  open the local web dashboard (http://127.0.0.1:7878); also the default in a terminal\n  \
           fetchira search QUERY...     one-shot search; MCP optional\n  \
           fetchira read URL            read a page as markdown\n  \
           fetchira deep_research QUERY...  deep research (alias dr)\n  \
           fetchira browser URL         read a JS-heavy page\n  \
           fetchira create_image PROMPT...  generate/edit an image; prints saved file path\n  \
           fetchira usage [PROVIDER]    compact quota snapshot or full provider sheet\n  \
           fetchira setup               guided setup: pick providers, enter keys, log in\n  \
           fetchira providers           list all available providers\n  \
           fetchira list                show your accounts + remaining quota\n  \
         fetchira install             choose agent integration (--refresh: update existing skills and launchers)\n  \
           fetchira add <provider>      add an account  [--label L] [--key K] [--proxy pool|URL]\n  \
           fetchira remove <label>      delete an account\n  \
           fetchira proxy <label>       set an account's proxy  (direct | pool | http://user:pass@host:port)\n  \
           fetchira priority [cap]      show or set the provider order per capability (search/read/deep_research/image)\n  \
           fetchira login <provider>    (re)capture a web-session login (gemini_web/grok_web/chatgpt_web)\n  \
           fetchira session <label>     attach a web session by hand (cookies JSON on stdin or --file) — for headless boxes\n  \
           fetchira remote set URL      connect stdio to hosted Fetchira [--key fk_live_*]\n  \
           fetchira remote check        verify endpoint, API key, and version compatibility\n  \
           fetchira remote login CHALLENGE  capture/upload a hosted login [--browser chrome|firefox] [--file session.json]\n  \
           fetchira remote disconnect   clear the saved hosted endpoint and API key\n  \
           fetchira server              hosted Streamable HTTP at /mcp (alias serve-http; FETCHIRA_MASTER_KEY required; FETCHIRA_BIND default 127.0.0.1:7879)\n  \
           fetchira server key create ID [NAME] [--accounts-manage]  mint a key (mcp + usage:read; opt in to account management/remote login; prints fk_live_* once)\n  \
           fetchira server password hash  Argon2id of a password on stdin (not a TTY) — for FETCHIRA_ADMIN_PASSWORD, never plaintext\n  \
           fetchira update              update and finish agent setup (alias: upgrade; --when-idle waits for active instances)\n  \
           fetchira --version           print the installed version\n  \
           fetchira help                this message\n\n\
         Tool flags: fetchira <command> --help. Flags may mix with words; -- ends flags.\n\
         Quote queries, file paths, and session tokens. ChatGPT research/image poll sessions may omit text.\n\
         CLI tools use local accounts or the configured hosted endpoint.\n\n\
         Config lives in $FETCHIRA_HOME or ~/.config/fetchira (fetchira.toml + usage.db)."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn brew_launcher_is_used_only_for_the_matching_cellar_binary() {
        let home = std::env::temp_dir().join(format!(
            "fetchira-brew-launcher-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let versioned = home.join("Cellar/fetchira/0.1.14/bin/fetchira");
        std::fs::create_dir_all(versioned.parent().unwrap()).unwrap();
        std::fs::write(&versioned, "binary").unwrap();
        std::fs::create_dir_all(home.join("bin")).unwrap();
        std::os::unix::fs::symlink(&versioned, home.join("bin/fetchira")).unwrap();
        assert_eq!(
            brew_launcher_for(&versioned),
            Some(home.canonicalize().unwrap().join("bin/fetchira"))
        );
        let old = home.join("Cellar/fetchira/0.1.13/bin/fetchira");
        let stable = home.join("bin/fetchira");
        let stable = stable.to_str().unwrap();
        let config_path = home.join("mcp.json");
        let original = json!({"mcpServers": {
            "fetchira": {"command": old, "args": ["serve"], "env": {"FETCHIRA_HOME": "/custom/home"}},
            "other": {"command": "keep"}
        }});
        std::fs::write(&config_path, original.to_string()).unwrap();
        repair_json_mcp_servers(&config_path, &home, stable).unwrap();
        let repaired: Value =
            serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
        let mut expected = original;
        expected["mcpServers"]["fetchira"]["command"] = json!(stable);
        assert_eq!(repaired, expected);
        assert_eq!(
            repair_json_mcp_servers(&config_path, &home, stable).unwrap(),
            "unchanged"
        );
        let codex = home.join("config.toml");
        let source = format!("[mcp_servers.fetchira]\ncommand = {}\nargs = [\"serve\"]\n[mcp_servers.other]\ncommand = \"keep\"\n", json!(old.to_str().unwrap()));
        std::fs::write(&codex, source).unwrap();
        repair_codex_registration(&codex, &home, stable).unwrap();
        let repaired: toml::Table =
            toml::from_str(&std::fs::read_to_string(&codex).unwrap()).unwrap();
        assert_eq!(
            repaired["mcp_servers"]["fetchira"]["command"].as_str(),
            Some(stable)
        );
        assert_eq!(
            repaired["mcp_servers"]["other"]["command"].as_str(),
            Some("keep")
        );
        assert!(!needs_launcher_repair(
            "/another/Cellar/fetchira/0.1.13/bin/fetchira",
            stable
        ));
        assert!(!needs_launcher_repair("my-custom-wrapper", stable));

        let unrelated = home.join("other/bin/fetchira");
        std::fs::create_dir_all(unrelated.parent().unwrap()).unwrap();
        std::fs::write(&unrelated, "binary").unwrap();
        assert_eq!(brew_launcher_for(&unrelated), None);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn mcp_registration_uses_native_config_paths() {
        let home = std::env::temp_dir().join(format!(
            "fetchira_mcp_paths_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let appsup = home.join("Library/Application Support");
        let xdg = home.join("custom-config");
        for base in [&appsup, &xdg] {
            for app in ["Claude", "Code"] {
                std::fs::create_dir_all(base.join(app)).unwrap();
            }
        }
        let native = if cfg!(target_os = "macos") {
            &appsup
        } else {
            &xdg
        };
        for (name, path) in [
            (
                "Claude Desktop",
                native.join("Claude/claude_desktop_config.json"),
            ),
            ("VS Code", native.join("Code/User/mcp.json")),
            ("OpenCode", xdg.join("opencode/opencode.json")),
            ("Codex CLI", home.join("custom-codex/config.toml")),
        ] {
            let targets = mcp_targets(&home, &appsup, &xdg, &home.join("custom-codex"));
            let target = targets.iter().find(|t| t.name == name).unwrap();
            assert!(!target.installed);
            (target.run)(Path::new("/tmp/fetchira-config"), "/bin/fetchira").unwrap();
            assert!(has_fetchira(&path), "wrong config path for {name}");
            let targets = mcp_targets(&home, &appsup, &xdg, &home.join("custom-codex"));
            let target = targets.iter().find(|t| t.name == name).unwrap();
            assert!(target.present && target.installed);
        }
        assert!(!home.join(".codex/config.toml").exists());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn mcp_merge_preserves_existing() {
        let path = std::env::temp_dir().join("fetchira_install_test.json");
        std::fs::write(
            &path,
            r#"{"mcpServers":{"other":{"command":"x"}},"theme":"dark"}"#,
        )
        .unwrap();
        reg_mcp_servers(&path, Path::new("/tmp/fetchira-config"), "/bin/fetchira").unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["fetchira"]["command"], "/bin/fetchira");
        assert_eq!(v["mcpServers"]["other"]["command"], "x"); // untouched
        assert_eq!(v["theme"], "dark"); // untouched
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn install_detection_checks_the_client_server_map() {
        let home = std::env::temp_dir().join(format!(
            "fetchira-detection-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&home).unwrap();
        let json_path = home.join("mcp.json");
        std::fs::write(&json_path, r#"{"note":"fetchira","mcpServers":{}}"#).unwrap();
        assert!(!has_fetchira(&json_path));
        std::fs::write(
            &json_path,
            r#"{"mcpServers":{"fetchira":{"command":"/bin/fetchira"}}}"#,
        )
        .unwrap();
        assert!(has_fetchira(&json_path));

        let toml_path = home.join("config.toml");
        std::fs::write(&toml_path, "note = 'fetchira'\n[mcp_servers.other]\n").unwrap();
        assert!(!has_fetchira(&toml_path));
        std::fs::write(
            &toml_path,
            "[mcp_servers.fetchira]\ncommand = '/bin/fetchira'\n",
        )
        .unwrap();
        assert!(has_fetchira(&toml_path));
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn claude_config_refresh_preserves_other_servers() {
        let home = std::env::temp_dir().join(format!(
            "fetchira-claude-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&home).unwrap();
        let path = home.join(".claude.json");
        let mut obj = serde_json::Map::new();
        obj.insert(
            "mcpServers".into(),
            json!({
                "other": { "command": "other" },
                "fetchira": { "command": "/old/fetchira", "args": [] }
            }),
        );
        std::fs::write(&path, serde_json::to_string(&obj).unwrap()).unwrap();
        let mut parsed = read_obj(&path).unwrap();
        let servers = parsed
            .get_mut("mcpServers")
            .and_then(Value::as_object_mut)
            .unwrap();
        let entry = servers
            .get_mut("fetchira")
            .unwrap()
            .as_object_mut()
            .unwrap();
        entry.insert("command".into(), json!("/new/fetchira"));
        entry.insert("args".into(), json!([]));
        entry.insert("env".into(), registered_env(&home));
        write_obj(&path, &parsed).unwrap();
        let updated = read_obj(&path).unwrap();
        assert_eq!(
            updated["mcpServers"]["fetchira"]["command"],
            "/new/fetchira"
        );
        assert_eq!(updated["mcpServers"]["other"]["command"], "other");
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn invalid_configs_are_reported_and_preserved() {
        let home =
            std::env::temp_dir().join(format!("fetchira-config-errors-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let path = home.join("fetchira.toml");
        std::fs::write(&path, "broken = [").unwrap();
        assert!(add_account(
            &home,
            ProviderKind::Serper,
            None,
            Some("test-key".into()),
            None
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken = [");
        for (register, contents) in [
            (
                reg_mcp_servers as fn(&Path, &Path, &str) -> anyhow::Result<String>,
                "{invalid",
            ),
            (reg_mcp_servers, "{\"mcpServers\":[]}"),
            (reg_vscode, "{\"servers\":null}"),
            (reg_opencode, "{\"mcp\":false}"),
            (reg_codex, "broken = ["),
            (reg_codex, "mcp_servers = 42"),
        ] {
            std::fs::write(&path, contents).unwrap();
            assert!(
                register(&path, &home, "/bin/fetchira").is_err(),
                "{contents}"
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
        }
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn removing_mcp_registration_preserves_other_servers_and_backups_original() {
        let home = std::env::temp_dir().join(format!(
            "fetchira-mcp-remove-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&home).unwrap();
        let path = home.join("mcp.json");
        let original = br#"{"mcpServers":{"other":{"command":"other"},"fetchira":{"command":"/old/fetchira"}},"theme":"dark"}"#;
        std::fs::write(&path, original).unwrap();

        let message = remove_json_mcp_servers(&path, &home).unwrap();
        let updated: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(updated["mcpServers"].get("fetchira").is_none());
        assert_eq!(updated["mcpServers"]["other"]["command"], "other");
        assert_eq!(updated["theme"], "dark");
        assert!(message.contains("backup:"));

        let backups = home.join(".fetchira-mcp-backups");
        let backup = std::fs::read_dir(backups)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(std::fs::read(backup).unwrap(), original);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn removing_codex_registration_preserves_other_servers_and_backups_original() {
        let home = std::env::temp_dir().join(format!(
            "fetchira-codex-remove-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&home).unwrap();
        let path = home.join("config.toml");
        let original = b"title = 'keep'\n[mcp_servers.other]\ncommand = 'other'\n[mcp_servers.fetchira]\ncommand = '/old/fetchira'\n";
        std::fs::write(&path, original).unwrap();

        remove_codex_registration(&path, &home).unwrap();
        let updated: toml::Table =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(updated["mcp_servers"].get("fetchira").is_none());
        assert_eq!(
            updated["mcp_servers"]["other"]["command"].as_str(),
            Some("other")
        );
        assert_eq!(updated["title"].as_str(), Some("keep"));
        let backup = std::fs::read_dir(home.join(".fetchira-mcp-backups"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(std::fs::read(backup).unwrap(), original);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn removing_mcp_refuses_malformed_and_symlink_configs() {
        let home = std::env::temp_dir().join(format!(
            "fetchira-mcp-remove-errors-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&home).unwrap();
        let malformed = home.join("malformed.json");
        std::fs::write(&malformed, b"{broken").unwrap();
        assert!(remove_json_mcp_servers(&malformed, &home).is_err());
        assert_eq!(std::fs::read(&malformed).unwrap(), b"{broken");

        #[cfg(unix)]
        {
            let target = home.join("target.json");
            let link = home.join("link.json");
            let original = br#"{"mcpServers":{"fetchira":{"command":"/old/fetchira"}}}"#;
            std::fs::write(&target, original).unwrap();
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(remove_json_mcp_servers(&link, &home).is_err());
            assert_eq!(std::fs::read(&target).unwrap(), original);
            assert!(!home.join(".fetchira-mcp-backups").exists());
        }
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn mcp_conversion_only_accepts_a_successful_selected_skill() {
        let skipped = crate::skills::SkillInstallResult {
            name: "Cursor",
            ok: true,
            msg: "skipped: Cursor was not selected".into(),
        };
        assert!(!skill_succeeded_for_target("Cursor", &[skipped], false));
        let installed = crate::skills::SkillInstallResult {
            name: "Cursor",
            ok: true,
            msg: "installed /tmp/.cursor/skills/fetchira-cli".into(),
        };
        assert!(skill_succeeded_for_target("Cursor", &[installed], false));
        let shared = crate::skills::SkillInstallResult {
            name: "Codex",
            ok: true,
            msg: "installed shared skill".into(),
        };
        assert!(!skill_succeeded_for_target(
            "Gemini CLI",
            std::slice::from_ref(&shared),
            false
        ));
        assert!(skill_succeeded_for_target("Gemini CLI", &[shared], true));
        assert!(!agent_skill_supported("OpenCode"));
        assert!(agent_skill_supported("Gemini CLI"));
    }

    #[test]
    fn list_hides_soft_web_quota_when_live_poll_misses() {
        assert_eq!(list_remaining(true, false, Some(100)), "-");
        assert_eq!(list_research(true, false, Some((3, 3))), "-");
        assert_eq!(list_remaining(true, true, Some(12)), "12");
        assert_eq!(list_research(true, true, Some((3, 3))), "3/3/day");
        assert_eq!(list_remaining(false, false, Some(2500)), "2500");
    }
}
