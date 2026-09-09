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
    #[serde(default, skip_serializing_if = "RemoteConfig::is_empty")]
    pub remote: RemoteConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            db_path: default_db(),
            debug_log: DebugLog::default(),
            proxy_pool: ProxyPool::default(),
            priority: Priority::default(),
            accounts: Vec::new(),
            remote: RemoteConfig::default(),
        }
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct RemoteConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

impl RemoteConfig {
    fn is_empty(&self) -> bool {
        self.endpoint.is_none() && self.api_key.is_none()
    }
}

impl std::fmt::Debug for RemoteConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteConfig")
            .field("endpoint", &self.endpoint)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
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

#[derive(Default, Deserialize, Serialize)]
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

impl std::fmt::Debug for ProxyPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyPool")
            .field(
                "webshare_url",
                &self.webshare_url.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "proxies",
                &format_args!("<redacted:{}>", self.proxies.len()),
            )
            .finish()
    }
}

#[derive(Clone, Deserialize, Serialize)]
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

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("provider", &self.provider)
            .field("label", &self.label)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("proxy", &self.proxy.as_ref().map(|_| "<redacted>"))
            .field("quota", &self.quota)
            .field("reset", &self.reset)
            .field("dr_quota", &self.dr_quota)
            .field("dr_reset", &self.dr_reset)
            .finish()
    }
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

/// Replace the config atomically; a failed write must preserve the previous credentials.
pub fn save(cfg: &Config, path: &Path) -> Result<()> {
    let txt = toml::to_string_pretty(cfg).map_err(|e| Error::Config(format!("serialize: {e}")))?;
    write_atomic(path, &txt, true)
}

/// Shared with MCP registration: protect Fetchira secrets, preserve existing client file modes.
pub(crate) fn write_atomic(path: &Path, text: &str, private: bool) -> Result<()> {
    use std::io::Write;
    // Preserve a user's config symlink while replacing its target.
    let target = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => path
            .canonicalize()
            .map_err(|e| Error::Config(format!("resolve {}: {e}", path.display())))?,
        Ok(_) => path.to_path_buf(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => path.to_path_buf(),
        Err(e) => return Err(Error::Config(format!("read {}: {e}", path.display()))),
    };
    let temp = target.with_file_name(format!(".fetchira-{}.tmp", rand::random::<u64>()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp)
        .map_err(|e| Error::Config(format!("create {}: {e}", temp.display())))?;
    let result = (|| -> std::io::Result<()> {
        if !private {
            match std::fs::metadata(&target) {
                Ok(metadata) => file.set_permissions(metadata.permissions())?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, &target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map_err(|e| Error::Config(format!("write {}: {e}", path.display())))
}

/// Encrypt literal provider credentials before a config is used by the hosted runtime. Existing
/// `env:` references remain references; encrypted values are idempotent. Local-only configs keep
/// their existing 0600 behavior and do not require a master key.
pub fn protect_hosted_secrets(cfg: &mut Config) -> Result<bool> {
    crate::secrets::require_master_key().map_err(|e| Error::Config(e.to_string()))?;
    protect_secrets_with(cfg, |value| {
        crate::secrets::encrypt(value).map_err(|e| Error::Config(e.to_string()))
    })
}

fn protect_secrets_with(
    cfg: &mut Config,
    encrypt: impl Fn(&str) -> Result<String>,
) -> Result<bool> {
    let mut changed = false;
    for account in &mut cfg.accounts {
        let Some(value) = account.api_key.as_mut() else {
            continue;
        };
        if value.starts_with("env:") || value.starts_with("enc:") {
            continue;
        }
        *value = encrypt(value)?;
        changed = true;
    }
    Ok(changed)
}

/// Load a hosted config, require encryption to be configured, and migrate legacy plaintext API
/// keys in place. This is the only config loader a hosted server should use.
pub fn load_hosted(path: &Path) -> Result<Config> {
    if !crate::secrets::configured() {
        return Err(Error::Config(
            "FETCHIRA_MASTER_KEY is required for hosted secrets".into(),
        ));
    }
    let mut cfg = if path.exists() {
        load(&path.to_string_lossy())?
    } else {
        Config::default()
    };
    if protect_hosted_secrets(&mut cfg)? {
        save(&cfg, path)?;
    }
    Ok(cfg)
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
        None => crate::secrets::resolve(s).map_err(|e| Error::Config(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_replaces_privately_without_truncating_previous_file() {
        let home = std::env::temp_dir().join(format!("fetchira-config-{}", rand::random::<u64>()));
        std::fs::create_dir(&home).unwrap();
        let path = home.join("fetchira.toml");
        let previous = home.join("previous.toml");
        std::fs::write(&path, "original credentials").unwrap();
        std::fs::hard_link(&path, &previous).unwrap();
        save(&Config::default(), &path).unwrap();
        assert_eq!(
            std::fs::read_to_string(&previous).unwrap(),
            "original credentials"
        );
        assert!(load(path.to_str().unwrap()).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            let link = home.join("linked.toml");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            save(&Config::default(), &link).unwrap();
            assert!(std::fs::symlink_metadata(link)
                .unwrap()
                .file_type()
                .is_symlink());
            let dangling = home.join("dangling.toml");
            std::os::unix::fs::symlink(home.join("missing.toml"), &dangling).unwrap();
            assert!(save(&Config::default(), &dangling).is_err());
            assert!(std::fs::symlink_metadata(dangling)
                .unwrap()
                .file_type()
                .is_symlink());
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
            write_atomic(&path, "client = true", false).unwrap();
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
        let blocked = home.join("directory.toml");
        std::fs::create_dir(&blocked).unwrap();
        assert!(save(&Config::default(), &blocked).is_err());
        assert!(blocked.is_dir());
        assert!(!std::fs::read_dir(&home).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn secret_debug_is_redacted() {
        let remote = RemoteConfig {
            endpoint: Some("https://example.test/mcp".into()),
            api_key: Some("fk_live_secret".into()),
        };
        let account = Account {
            provider: ProviderKind::Tavily,
            label: "primary".into(),
            api_key: Some("tvly-secret".into()),
            proxy: Some("http://user:password@proxy.test".into()),
            quota: None,
            reset: None,
            dr_quota: None,
            dr_reset: None,
        };
        assert!(!format!("{remote:?}").contains("fk_live_secret"));
        assert!(!format!("{account:?}").contains("tvly-secret"));
        assert!(!format!("{account:?}").contains("password"));
    }

    #[test]
    fn hosted_protection_migrates_only_literal_keys() {
        let account = |label: &str, key: &str| Account {
            provider: ProviderKind::Tavily,
            label: label.into(),
            api_key: Some(key.into()),
            proxy: None,
            quota: None,
            reset: None,
            dr_quota: None,
            dr_reset: None,
        };
        let mut cfg = Config {
            accounts: vec![
                account("plain", "secret"),
                account("environment", "env:TAVILY_KEY"),
                account("encrypted", "enc:already"),
            ],
            ..Config::default()
        };
        assert!(protect_secrets_with(&mut cfg, |value| Ok(format!("enc:{value}"))).unwrap());
        assert_eq!(cfg.accounts[0].api_key.as_deref(), Some("enc:secret"));
        assert_eq!(cfg.accounts[1].api_key.as_deref(), Some("env:TAVILY_KEY"));
        assert_eq!(cfg.accounts[2].api_key.as_deref(), Some("enc:already"));
    }
}
