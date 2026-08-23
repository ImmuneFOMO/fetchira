use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone, Default)]
pub struct UpdateState(Arc<RwLock<UpdateInfo>>);

#[derive(Clone, Debug, serde::Serialize)]
pub struct UpdateInfo {
    pub status: String,
    pub message: Option<String>,
    pub requested_at: Option<String>,
}
impl Default for UpdateInfo {
    fn default() -> Self {
        Self {
            status: "idle".into(),
            message: None,
            requested_at: None,
        }
    }
}
impl UpdateState {
    pub async fn get(&self) -> UpdateInfo {
        self.0.read().await.clone()
    }
    pub async fn set(&self, status: &str, message: Option<String>) {
        let mut v = self.0.write().await;
        v.status = status.into();
        v.message = message;
        v.requested_at = Some(chrono::Utc::now().to_rfc3339());
    }

    pub async fn start(&self) -> Option<()> {
        let mut v = self.0.write().await;
        if matches!(v.status.as_str(), "queued" | "backing_up" | "updating") {
            return None;
        }
        v.status = "queued".into();
        v.message = None;
        v.requested_at = Some(chrono::Utc::now().to_rfc3339());
        Some(())
    }

    pub async fn finish(&self, status: &str, message: Option<String>) {
        let mut v = self.0.write().await;
        v.status = status.into();
        v.message = message;
    }
}

pub async fn perform(
    home: &Path,
    db_path: &str,
    active: Arc<tokio::sync::Semaphore>,
    when_idle: bool,
) -> anyhow::Result<String> {
    let _drained = if when_idle {
        tokio::time::timeout(
            std::time::Duration::from_secs(3600),
            active.acquire_many_owned(64),
        )
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for active requests to drain"))?
        .map_err(|_| anyhow::anyhow!("server is shutting down"))?
    } else {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            active.acquire_many_owned(64),
        )
        .await
        .map_err(|_| anyhow::anyhow!("timed out draining active requests"))?
        .map_err(|_| anyhow::anyhow!("server is shutting down"))?
    };
    let backup = backup_db(home, db_path).await?;
    match crate::update::perform(home, true).await? {
        crate::update::Outcome::Updated(version) => {
            let message = format!(
                "updated to {version}; backup {}; restarting service",
                backup.display()
            );
            #[cfg(unix)]
            {
                restart_self(&message);
            }
            #[cfg(not(unix))]
            {
                Ok(message)
            }
        }
        crate::update::Outcome::UpToDate => {
            Ok(format!("already up to date; backup {}", backup.display()))
        }
        crate::update::Outcome::Brew => anyhow::bail!("Homebrew-managed server: use brew upgrade"),
        crate::update::Outcome::Blocked { latest, .. } => {
            anyhow::bail!("update to {latest} is blocked by running instances")
        }
    }
}

#[cfg(unix)]
fn restart_self(message: &str) -> ! {
    use std::os::unix::process::CommandExt;
    let Ok(exe) = std::env::current_exe() else {
        eprintln!("{message}");
        std::process::exit(1);
    };
    eprintln!("{message}");
    let err = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!("restart after hosted update failed: {err}");
    std::process::exit(1);
}

pub async fn backup_db(home: &Path, db_path: &str) -> anyhow::Result<PathBuf> {
    let src = Path::new(db_path);
    if !src.exists() {
        anyhow::bail!("database does not exist: {}", src.display());
    }
    // SQLite is in WAL mode. Checkpoint before copying so committed rows in the WAL are
    // included in the snapshot; copying only usage.db is not a consistent backup.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(src)
                .journal_mode(SqliteJournalMode::Wal),
        )
        .await?;
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&pool)
        .await?;
    pool.close().await;
    let dir = home.join("backups");
    std::fs::create_dir_all(&dir)?;
    let dst = dir.join(format!(
        "usage-{}.db",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    ));
    std::fs::copy(src, &dst)?;
    Ok(dst)
}
pub fn verify_release_bytes(bytes: &[u8], expected_sha256: &str) -> bool {
    use sha2::{Digest, Sha256};
    let got: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    got.eq_ignore_ascii_case(expected_sha256.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checksum_rejects_bad() {
        assert!(!verify_release_bytes(b"x", "bad"));
    }
}
