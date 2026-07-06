use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use inquire::{Confirm, MultiSelect, Select, Text};
use serde_json::{json, Value};

use crate::config::{self, Account, Config};
use crate::providers::ProviderKind;
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

fn cfg_path(home: &Path) -> PathBuf {
    home.join("fetchira.toml")
}

fn load_or_empty(home: &Path) -> Config {
    config::load(cfg_path(home).to_str().unwrap_or_default()).unwrap_or_default()
}

async fn open_store(home: &Path, cfg: &Config) -> anyhow::Result<Store> {
    Ok(Store::open(&config::resolve_db(home, &cfg.db_path)).await?)
}

fn parse_provider(s: &str) -> anyhow::Result<ProviderKind> {
    serde_json::from_value::<ProviderKind>(serde_json::Value::String(s.to_string()))
        .with_context(|| format!("unknown provider '{s}' (try `fetchira providers`)"))
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
    let cfg = load_or_empty(home);
    if cfg.accounts.is_empty() {
        println!("No accounts yet. Run `fetchira setup`.");
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
        "{:15} {:14} {:12} {:>10} {:>13}  PROXY",
        "PROVIDER", "LABEL", "CRED", "REMAINING", "RESEARCH"
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
        let remaining = main
            .map(|v| v.remaining.to_string())
            .unwrap_or_else(|| "-".into());
        let research = match (a.provider.is_web().then_some(()), drs.get(a.label.as_str())) {
            (Some(_), Some(v)) => format!("{}/{}/day", v.remaining, v.quota),
            _ => "-".to_string(),
        };
        println!(
            "{:15} {:14} {:12} {:>10} {:>13}  {}",
            a.provider.as_str(),
            a.label,
            cred,
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

/// "{remaining}/{quota}/day" deep-research budget for authorized web providers, "-" otherwise.
async fn research_cell(store: &Store, a: &Account, ready: bool) -> String {
    if !ready || !a.provider.is_web() {
        return "-".to_string();
    }
    let dq = a.dr_quota.unwrap_or_else(|| a.provider.dr_quota());
    let period = period_key(a.dr_reset.unwrap_or_else(|| a.provider.dr_reset()));
    let dr = store
        .remaining(&format!("{}#dr", a.label), dq, &period)
        .await
        .unwrap_or(dq);
    format!("{dr}/{dq}/day")
}

pub async fn add(home: &Path, mut args: impl Iterator<Item = String>) -> anyhow::Result<()> {
    let provider = args
        .next()
        .context("usage: fetchira add <provider> [--label L] [--key K] [--proxy pool|URL]")?;
    let kind = parse_provider(&provider)?;
    let (mut label, mut key, mut proxy) = (None, None, None);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--label" => label = args.next(),
            "--key" => key = args.next(),
            "--proxy" => proxy = args.next(),
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
        let cfg = load_or_empty(home);
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
    let mut cfg = load_or_empty(home);
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
    let mut cfg = load_or_empty(home);
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
    let mut cfg = load_or_empty(home);
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

/// If a web account's captured identity (email) already belongs to another account of the same
/// provider, returns (that other label, the identity) — so the add flow can reject a duplicate.
pub async fn identity_dup(
    home: &Path,
    kind: ProviderKind,
    label: &str,
) -> anyhow::Result<Option<(String, String)>> {
    let store = open_store(home, &load_or_empty(home)).await?;
    let Some(id) = store.load_identity(label).await? else {
        return Ok(None);
    };
    Ok(store
        .identity_conflict(kind.as_str(), &id, label)
        .await?
        .map(|other| (other, id)))
}

/// Capture (or re-capture) a web session for an existing account. Shared by CLI + web UI.
pub async fn capture_login(
    home: &Path,
    label: &str,
    browser: Option<String>,
) -> anyhow::Result<()> {
    let cfg = load_or_empty(home);
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
    let cfg = load_or_empty(home);
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
            "--file" | "-f" => file = args.next(),
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
    let cfg = load_or_empty(home);
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
    println!(
        "opening a browser to log into {} ({label}) — finish login; the window closes itself once you're in…",
        kind.as_str()
    );
    let session = web::login(home, kind, label, browser).await?;
    let store = open_store(home, cfg).await?;
    store
        .save_session(label, kind.as_str(), &serde_json::to_string(&session)?)
        .await?;
    // Best-effort: record the account email (dashboard display + dup detection). Direct egress is
    // fine for this one-off identity read; the router uses the sticky proxy for real calls.
    let proxy = cfg
        .accounts
        .iter()
        .find(|a| a.label == label)
        .and_then(|a| a.proxy.as_deref())
        .filter(|p| p.starts_with("http"));
    if let Ok(client) = web::build_client(&session.cookies, &session.headers, proxy) {
        if let Some(id) = crate::providers::Provider::new(kind)
            .account_identity(&client)
            .await
        {
            let _ = store.set_identity(label, &id).await;
        }
    }
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
    loop {
        print_status(home).await?;
        let action = Select::new(
            "Manage providers — ↑/↓ to move, Enter to pick, Esc to exit:",
            vec![
                "Add an account",
                "Log in / re-login a web provider",
                "Remove an account",
                "Quit",
            ],
        )
        .prompt();
        match action {
            Ok("Add an account") => add_flow(home).await?,
            Ok("Log in / re-login a web provider") => login_flow(home).await?,
            Ok("Remove an account") => remove_flow(home).await?,
            _ => break, // Quit or Esc
        }
    }
    if Confirm::new("\nRegister fetchira into your coding tools now?")
        .with_default(true)
        .prompt()
        .unwrap_or(false)
    {
        let _ = install_tools();
    }
    println!(
        "\nConfigure anytime with `fetchira setup`.  Config: {}",
        home.display()
    );
    Ok(())
}

/// Clear the screen and print the current accounts + remaining quota (the TUI's status board).
async fn print_status(home: &Path) -> anyhow::Result<()> {
    print!("\x1B[2J\x1B[H");
    let _ = std::io::stdout().flush();
    println!("fetchira — your providers\n");
    let cfg = load_or_empty(home);
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
    let mut cfg = load_or_empty(home);
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
        Text::new("Proxy URL (http://user:pass@host:port):")
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

async fn login_flow(home: &Path) -> anyhow::Result<()> {
    let cfg = load_or_empty(home);
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
    let mut cfg = load_or_empty(home);
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

/// `fetchira install` — pick coding tools and write fetchira's MCP-server registration into each.
pub fn install_tools() -> anyhow::Result<()> {
    let bin = std::env::current_exe()?.to_string_lossy().into_owned();
    let h = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let appsup = h.join("Library/Application Support");

    let targets = mcp_targets(&h, &appsup);
    let opts: Vec<String> = targets
        .iter()
        .map(|t| format!("{}{}", t.name, if t.present { "  (detected)" } else { "" }))
        .collect();
    let preselect: Vec<usize> = targets
        .iter()
        .enumerate()
        .filter(|(_, t)| t.present)
        .map(|(i, _)| i)
        .collect();

    let chosen = match MultiSelect::new(
        "Register fetchira's MCP server into which tools? (Space toggles, Enter confirms)",
        opts.clone(),
    )
    .with_default(&preselect)
    .prompt()
    {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };

    println!();
    for label in &chosen {
        let idx = opts.iter().position(|o| o == label).unwrap_or(0);
        match (targets[idx].run)(&bin) {
            Ok(msg) => println!("  ✓ {:14} {msg}", targets[idx].name),
            Err(e) => println!("  ✗ {:14} {e}", targets[idx].name),
        }
    }
    println!("\nRestart the tool (or reload its MCP servers) to pick up fetchira.");
    Ok(())
}

type RunFn = Box<dyn Fn(&str) -> anyhow::Result<String>>;

struct McpTarget {
    name: &'static str,
    present: bool,
    run: RunFn,
}

fn mcp_targets(h: &Path, appsup: &Path) -> Vec<McpTarget> {
    let p = |rel: &str| h.join(rel);
    vec![
        McpTarget {
            name: "Claude Code",
            present: which("claude"),
            run: Box::new(reg_claude_code),
        },
        McpTarget {
            name: "Codex CLI",
            present: p(".codex").exists() || which("codex"),
            run: boxed(p(".codex/config.toml"), reg_codex),
        },
        McpTarget {
            name: "OpenCode",
            present: p(".config/opencode").exists() || which("opencode"),
            run: boxed(p(".config/opencode/opencode.json"), reg_opencode),
        },
        McpTarget {
            name: "Gemini CLI",
            present: p(".gemini").exists() || which("gemini"),
            run: boxed(p(".gemini/settings.json"), reg_mcp_servers),
        },
        McpTarget {
            name: "Cursor",
            present: p(".cursor").exists(),
            run: boxed(p(".cursor/mcp.json"), reg_mcp_servers),
        },
        McpTarget {
            name: "Windsurf",
            present: p(".codeium/windsurf").exists(),
            run: boxed(p(".codeium/windsurf/mcp_config.json"), reg_mcp_servers),
        },
        McpTarget {
            name: "Claude Desktop",
            present: appsup.join("Claude").exists(),
            run: boxed(
                appsup.join("Claude/claude_desktop_config.json"),
                reg_mcp_servers,
            ),
        },
        McpTarget {
            name: "VS Code",
            present: appsup.join("Code").exists(),
            run: boxed(appsup.join("Code/User/mcp.json"), reg_vscode),
        },
    ]
}

fn boxed(path: PathBuf, f: fn(&Path, &str) -> anyhow::Result<String>) -> RunFn {
    Box::new(move |bin| f(&path, bin))
}

fn which(cmd: &str) -> bool {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|d| Path::new(d).join(cmd).exists())
}

fn read_obj(path: &Path) -> serde_json::Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn write_obj(path: &Path, obj: &serde_json::Map<String, Value>) -> anyhow::Result<String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(&Value::Object(obj.clone()))?,
    )?;
    Ok(format!("wrote {}", path.display()))
}

/// The common `{ "mcpServers": { "fetchira": { "command": … } } }` shape (Cursor, Windsurf,
/// Gemini CLI, Claude Desktop).
fn reg_mcp_servers(path: &Path, bin: &str) -> anyhow::Result<String> {
    let mut obj = read_obj(path);
    let servers = obj.entry("mcpServers").or_insert_with(|| json!({}));
    if let Some(m) = servers.as_object_mut() {
        m.insert("fetchira".into(), json!({ "command": bin }));
    }
    write_obj(path, &obj)
}

/// VS Code: `{ "servers": { "fetchira": { "type": "stdio", "command": … } } }`.
fn reg_vscode(path: &Path, bin: &str) -> anyhow::Result<String> {
    let mut obj = read_obj(path);
    let servers = obj.entry("servers").or_insert_with(|| json!({}));
    if let Some(m) = servers.as_object_mut() {
        m.insert(
            "fetchira".into(),
            json!({ "type": "stdio", "command": bin }),
        );
    }
    write_obj(path, &obj)
}

/// OpenCode: `{ "mcp": { "fetchira": { "type": "local", "command": [bin], "enabled": true } } }`.
fn reg_opencode(path: &Path, bin: &str) -> anyhow::Result<String> {
    let mut obj = read_obj(path);
    obj.entry("$schema")
        .or_insert_with(|| json!("https://opencode.ai/config.json"));
    let mcp = obj.entry("mcp").or_insert_with(|| json!({}));
    if let Some(m) = mcp.as_object_mut() {
        m.insert(
            "fetchira".into(),
            json!({ "type": "local", "command": [bin], "enabled": true }),
        );
    }
    write_obj(path, &obj)
}

/// Codex CLI: TOML `[mcp_servers.fetchira] command = … args = []`.
fn reg_codex(path: &Path, bin: &str) -> anyhow::Result<String> {
    let mut doc: toml::Table = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default();
    let servers = doc
        .entry("mcp_servers")
        .or_insert_with(|| toml::Value::Table(Default::default()));
    if let Some(t) = servers.as_table_mut() {
        let mut e = toml::Table::new();
        e.insert("command".into(), toml::Value::String(bin.to_string()));
        e.insert("args".into(), toml::Value::Array(vec![]));
        t.insert("fetchira".into(), toml::Value::Table(e));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, toml::to_string_pretty(&doc)?)?;
    Ok(format!("wrote {}", path.display()))
}

/// Claude Code: use its CLI so the config + health check are handled natively.
fn reg_claude_code(bin: &str) -> anyhow::Result<String> {
    let out = std::process::Command::new("claude")
        .args(["mcp", "add", "fetchira", "-s", "user", "--", bin])
        .output();
    match out {
        Ok(o) if o.status.success() => Ok("claude mcp add fetchira (user scope)".into()),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            if err.contains("already exists") {
                Ok("already registered".into())
            } else {
                bail!("claude mcp add failed: {}", err.trim())
            }
        }
        Err(_) => bail!("`claude` CLI not on PATH"),
    }
}

pub fn help() {
    println!(
        "fetchira — quota-aware web search/scrape MCP server + CLI\n\n\
         USAGE:\n  \
           fetchira [serve]              run the MCP server (stdio) — the default when piped\n  \
           fetchira ui                  open the local web dashboard (live quota, accounts); also the default in a terminal\n  \
           fetchira setup               guided setup: pick providers, enter keys, log in\n  \
           fetchira providers           list all available providers\n  \
           fetchira list                show your accounts + remaining quota\n  \
           fetchira install             register the MCP server into your coding tools (Claude Code, Codex, …)\n  \
           fetchira add <provider>      add an account  [--label L] [--key K] [--proxy pool|URL]\n  \
           fetchira remove <label>      delete an account\n  \
           fetchira login <provider>    (re)capture a web-session login (gemini_web/grok_web/chatgpt_web)\n  \
           fetchira session <label>     attach a web session by hand (cookies JSON on stdin or --file) — for headless boxes\n  \
           fetchira update              download & install the latest release\n  \
           fetchira --version           print the installed version\n  \
           fetchira help                this message\n\n\
         Config lives in $FETCHIRA_HOME or ~/.config/fetchira (fetchira.toml + usage.db)."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_merge_preserves_existing() {
        let path = std::env::temp_dir().join("fetchira_install_test.json");
        std::fs::write(
            &path,
            r#"{"mcpServers":{"other":{"command":"x"}},"theme":"dark"}"#,
        )
        .unwrap();
        reg_mcp_servers(&path, "/bin/fetchira").unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["fetchira"]["command"], "/bin/fetchira");
        assert_eq!(v["mcpServers"]["other"]["command"], "x"); // untouched
        assert_eq!(v["theme"], "dark"); // untouched
        let _ = std::fs::remove_file(&path);
    }
}
