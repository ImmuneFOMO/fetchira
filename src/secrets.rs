use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use base64::Engine;
use sha2::{Digest, Sha256};

const PREFIX: &str = "enc:";
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

pub fn configured() -> bool {
    master_key().is_ok()
}

pub fn require_master_key() -> anyhow::Result<()> {
    master_key().map(|_| ())
}

fn master_key() -> anyhow::Result<String> {
    let value = std::env::var("FETCHIRA_MASTER_KEY").ok().or_else(|| {
        std::env::var("FETCHIRA_MASTER_KEY_FILE")
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok())
    });
    value
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "FETCHIRA_MASTER_KEY is required for hosted secrets; generate one with `openssl rand -base64 32`"
            )
        })
}

fn key(master: &str) -> [u8; 32] {
    Sha256::digest(master.as_bytes()).into()
}

pub fn encrypt(value: &str) -> anyhow::Result<String> {
    encrypt_with_master(value, &master_key()?)
}

pub(crate) fn encrypt_with_master(value: &str, master: &str) -> anyhow::Result<String> {
    if value.starts_with(PREFIX) {
        return Ok(value.to_string());
    }
    if master.is_empty() {
        anyhow::bail!("FETCHIRA_MASTER_KEY cannot be empty");
    }
    let cipher = Aes256Gcm::new_from_slice(&key(master))
        .map_err(|_| anyhow::anyhow!("invalid master key"))?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let mut out = nonce.to_vec();
    out.extend(
        cipher
            .encrypt(&nonce, value.as_bytes())
            .map_err(|_| anyhow::anyhow!("secret encryption failed"))?,
    );
    Ok(format!(
        "{PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(out)
    ))
}

pub fn decrypt(value: &str) -> anyhow::Result<String> {
    decrypt_with_master(value, &master_key()?)
}

pub(crate) fn decrypt_with_master(value: &str, master: &str) -> anyhow::Result<String> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(
        value
            .strip_prefix(PREFIX)
            .ok_or_else(|| anyhow::anyhow!("not encrypted"))?,
    )?;
    if raw.len() < NONCE_LEN + TAG_LEN {
        anyhow::bail!("invalid encrypted secret");
    }
    let (nonce, data) = raw.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new_from_slice(&key(master))
        .map_err(|_| anyhow::anyhow!("invalid master key"))?;
    #[allow(deprecated)]
    let nonce = Nonce::clone_from_slice(nonce);
    Ok(String::from_utf8(cipher.decrypt(&nonce, data).map_err(
        |_| anyhow::anyhow!("secret decryption failed"),
    )?)?)
}

pub fn resolve(value: &str) -> anyhow::Result<String> {
    value
        .strip_prefix("env:")
        .map(|name| std::env::var(name).map_err(Into::into))
        .unwrap_or_else(|| {
            if value.starts_with(PREFIX) {
                decrypt(value)
            } else {
                Ok(value.to_string())
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let e = encrypt_with_master("secret", "test-master").unwrap();
        assert_ne!(e, "secret");
        assert_eq!(decrypt_with_master(&e, "test-master").unwrap(), "secret");
        assert_eq!(encrypt_with_master(&e, "test-master").unwrap(), e);
    }

    #[test]
    fn malformed_ciphertext_is_rejected_without_panicking() {
        assert!(decrypt_with_master("enc:eA", "test-master").is_err());
        let encrypted = encrypt_with_master("secret", "right-master").unwrap();
        assert!(decrypt_with_master(&encrypted, "wrong-master").is_err());
    }
}
