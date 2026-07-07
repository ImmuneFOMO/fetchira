use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::providers::{Capability, ProviderKind};

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_db")]
    pub db_path: String,
    #[serde(default, skip_serializing_if = "DebugLog::is_default")]
    pub debug_log: DebugLog,
    #[serde(default, skip_serializing_if = "ProxyPool::is_empty")]
    pub proxy_pool: ProxyPool,
    #[serde(default, skip_serializing_if = "Priority::is_empty")]
    pub priority: Priority,
    #[serde(default, rename = "account")]
    pub accounts: Vec<Account>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            db_path: default_db(),
            debug_log: DebugLog::default(),
            proxy_pool: ProxyPool::default(),
            priority: Priority::default(),
            accounts: Vec::new(),
        }
    }
}

/// User override of the per-capability provider order (`fetchira priority`, UI Routing panel).
/// Listed providers are tried first, in this order; unlisted ones follow in the built-in order.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct Priority {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search: Vec<ProviderKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read: Vec<ProviderKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deep_research: Vec<ProviderKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image: Vec<ProviderKind>,
}

impl Priority {
    pub fn is_empty(&self) -> bool {
        self.search.is_empty()
            && self.read.is_empty()
            && self.deep_research.is_empty()
            && self.image.is_empty()
    }

    pub fn for_cap(&self, cap: Capability) -> &[ProviderKind] {
        match cap {
            Capability::Search => &self.search,
            Capability::Read => &self.read,
            Capability::DeepResearch => &self.deep_research,
            Capability::Image => &self.image,
            Capability::Browser => &[],
        }
    }

    pub fn set(&mut self, cap: Capability, list: Vec<ProviderKind>) {
        match cap {
            Capability::Search => self.search = list,
            Capability::Read => self.read = list,
            Capability::DeepResearch => self.deep_research = list,
            Capability::Image => self.image = list,
            Capability::Browser => {}
        }
    }
}

fn default_db() -> String {
    "usage.db".into()
}

/// Full request/response capture for debugging, kept in the `debug_log` table. Bounded by
/// `retention_hours` plus a fixed row + per-entry size cap (see `usage`), so it can't run away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct DebugLog {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_retention")]
    pub retention_hours: i64,
}

impl Default for DebugLog {
    fn default() -> Self {
        Self {
            enabled: true,
            retention_hours: default_retention(),
        }
    }
}

impl DebugLog {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

fn default_true() -> bool {
    true
}

fn default_retention() -> i64 {
    24
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ProxyPool {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webshare_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proxies: Vec<String>,
}

impl ProxyPool {
    fn is_empty(&self) -> bool {
        self.webshare_url.is_none() && self.proxies.is_empty()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Account {
    pub provider: ProviderKind,
    pub label: String,
    /// Absent for web-session providers, whose credential is a captured cookie session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// "pool" | "http://user:pass@host:port" | omitted (direct)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset: Option<Reset>,
    /// Separate budget for deep_research (web providers track it apart from chat, since the
    /// real per-tier limit is much smaller). Defaults per provider; tune to your subscription.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dr_quota: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dr_reset: Option<Reset>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Reset {
    Monthly,
    Once,
    Daily,
}

pub fn load(path: &str) -> Result<Config> {
    let txt =
        std::fs::read_to_string(path).map_err(|e| Error::Config(format!("read {path}: {e}")))?;
    toml::from_str(&txt).map_err(|e| Error::Config(format!("parse {path}: {e}")))
}

/// Write the config back (it can hold API keys, so restrict perms to 0600 on unix).
pub fn save(cfg: &Config, path: &Path) -> Result<()> {
    let txt = toml::to_string_pretty(cfg).map_err(|e| Error::Config(format!("serialize: {e}")))?;
    std::fs::write(path, txt)
        .map_err(|e| Error::Config(format!("write {}: {e}", path.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Resolve a possibly-relative `db_path` against the fetchira home dir.
pub fn resolve_db(home: &Path, db_path: &str) -> String {
    if Path::new(db_path).is_relative() {
        home.join(db_path).to_string_lossy().into_owned()
    } else {
        db_path.to_string()
    }
}

/// Resolve `"env:VAR"` against the environment; anything else is returned verbatim.
pub fn resolve_secret(s: &str) -> Result<String> {
    match s.strip_prefix("env:") {
        Some(var) => {
            std::env::var(var).map_err(|_| Error::Config(format!("missing env var {var}")))
        }
        None => Ok(s.to_string()),
    }
}
