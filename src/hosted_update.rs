use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
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
    let binary = crate::cli::installation_binary()?;
    let backup = backup_db(home, db_path).await?;
    match crate::update::perform(home, true).await? {
        crate::update::Outcome::Updated(version) => {
            let message = format!(
                "updated to {version}; backup {}; restarting service",
                backup.display()
            );
            #[cfg(unix)]
            {
                restart_self(&binary, &message);
            }
            #[cfg(not(unix))]
            {
                Ok(message)
            }
        }
        crate::update::Outcome::UpToDate => {
            Ok(format!("already up to date; backup {}", backup.display()))
        }
        crate::update::Outcome::Blocked { latest, .. } => {
            anyhow::bail!("update to {latest} is blocked by running instances")
        }
    }
}

#[cfg(unix)]
fn restart_self(exe: &str, message: &str) -> ! {
    use std::os::unix::process::CommandExt;
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
    let dir = home.join("backups");
    std::fs::create_dir_all(&dir)?;
    let dst = dir.join(format!(
        "usage-{}.db",
        chrono::Utc::now().format("%Y%m%d-%H%M%S-%f")
    ));
    // VACUUM INTO reads one transactionally-consistent snapshot while WAL writers continue.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().filename(src))
        .await?;
    let dst_sql = dst.to_string_lossy().into_owned();
    sqlx::query("VACUUM INTO ?")
        .bind(dst_sql)
        .execute(&pool)
        .await?;
    pool.close().await;
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[test]
    fn checksum_rejects_bad() {
        assert!(!verify_release_bytes(b"x", "bad"));
    }

    #[tokio::test]
    async fn backup_is_consistent_while_wal_is_being_written() {
        let home = std::env::temp_dir().join(format!(
            "fetchira-backup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&home).expect("test home");
        let source = home.join("usage.db");
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&source)
                    .create_if_missing(true)
                    .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal),
            )
            .await
            .expect("source database");
        sqlx::query("CREATE TABLE probe(id INTEGER PRIMARY KEY, payload BLOB NOT NULL)")
            .execute(&pool)
            .await
            .expect("create probe");
        sqlx::query("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x < 2000) INSERT INTO probe(payload) SELECT randomblob(2048) FROM n")
            .execute(&pool)
            .await
            .expect("seed probe");

        let stop = Arc::new(AtomicBool::new(false));
        let writes = Arc::new(AtomicUsize::new(0));
        let writer = {
            let pool = pool.clone();
            let stop = stop.clone();
            let writes = writes.clone();
            tokio::spawn(async move {
                while !stop.load(Ordering::Relaxed) {
                    sqlx::query("INSERT INTO probe(payload) VALUES(randomblob(256))")
                        .execute(&pool)
                        .await
                        .expect("concurrent write");
                    writes.fetch_add(1, Ordering::Relaxed);
                    tokio::task::yield_now().await;
                }
            })
        };
        while writes.load(Ordering::Relaxed) == 0 {
            tokio::task::yield_now().await;
        }
        let backup = backup_db(&home, source.to_str().expect("source path"))
            .await
            .expect("backup");
        stop.store(true, Ordering::Relaxed);
        writer.await.expect("writer task");

        let backup_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::new().filename(&backup))
            .await
            .expect("open backup");
        let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&backup_pool)
            .await
            .expect("integrity check");
        let backup_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM probe")
            .fetch_one(&backup_pool)
            .await
            .expect("backup rows");
        let source_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM probe")
            .fetch_one(&pool)
            .await
            .expect("source rows");
        assert_eq!(integrity, "ok");
        assert!(backup_rows >= 2000 && backup_rows <= source_rows);
        assert!(writes.load(Ordering::Relaxed) > 0);
        backup_pool.close().await;
        pool.close().await;
        std::fs::remove_dir_all(home).expect("remove test home");
    }
}
