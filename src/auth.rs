use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use base64::Engine;
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Mcp,
    UsageRead,
    AccountsManage,
    ServerUpdate,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::UsageRead => "usage:read",
            Self::AccountsManage => "accounts:manage",
            Self::ServerUpdate => "server:update",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKey {
    pub id: String,
    pub plaintext: String,
    pub hash: String,
    pub scopes: Vec<String>,
}

pub fn generate_key(
    id: impl Into<String>,
    scopes: impl IntoIterator<Item = Scope>,
) -> anyhow::Result<ApiKey> {
    let id = id.into();
    if id.is_empty() || id.len() > 40 || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
    {
        anyhow::bail!("API key id must be 1-40 ASCII letters, digits, or hyphens");
    }
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let plaintext = format!("fk_live_{id}_{secret}");
    Ok(ApiKey {
        id,
        hash: hash_key(&plaintext),
        plaintext,
        scopes: scopes
            .into_iter()
            .map(Scope::as_str)
            .map(str::to_string)
            .collect(),
    })
}

pub fn hash_key(key: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(key.as_bytes()))
}
pub fn verify_key(key: &str, hash: &str) -> bool {
    hash_key(key) == hash
}
pub fn has_scope(scopes: &[String], needed: Scope) -> bool {
    scopes
        .iter()
        .any(|s| s == "*" || s == needed.as_str() || (needed == Scope::Mcp && s == "mcp:*"))
}

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!(e.to_string()))
}
pub fn verify_password(password: &str, encoded: &str) -> bool {
    PasswordHash::new(encoded).ok().is_some_and(|h| {
        Argon2::default()
            .verify_password(password.as_bytes(), &h)
            .is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_roundtrip() {
        let k = generate_key("one", [Scope::Mcp]).unwrap();
        assert!(k.plaintext.starts_with("fk_live_one_"));
        assert!(verify_key(&k.plaintext, &k.hash));
        assert!(!verify_key("test", &hash_key("test").to_ascii_uppercase()));
        assert!(!verify_key("fk_live_one_bad", &k.hash));
        assert!(has_scope(&k.scopes, Scope::Mcp));
    }
    #[test]
    fn password_roundtrip() {
        let h = hash_password("correct horse").unwrap();
        assert!(verify_password("correct horse", &h));
        assert!(!verify_password("wrong", &h));
    }
}
