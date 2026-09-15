use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tokio_util::sync::CancellationToken;

pub const CAPACITY: u32 = 64;
pub const UPDATE_MESSAGE: &str = "Fetchira is updating; reconnect MCP and retry";

#[derive(Clone, Default)]
pub struct UpdateState {
    info: Arc<RwLock<UpdateInfo>>,
    path: Option<PathBuf>,
    cancellation: Arc<RwLock<CancellationToken>>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct UpdateInfo {
    pub status: String,
    pub message: Option<String>,
    pub requested_at: Option<String>,
    pub target: Option<String>,
    pub backup: Option<PathBuf>,
    pub previous_binary: Option<PathBuf>,
}
impl UpdateState {
    pub fn load(home: &Path) -> anyhow::Result<Self> {
        let path = home.join("hosted-update.json");
        let info = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => UpdateInfo {
                status: "idle".into(),
                ..Default::default()
            },
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            info: Arc::new(RwLock::new(info)),
            path: Some(path),
            cancellation: Default::default(),
        })
    }

    fn persist(&self, info: &UpdateInfo) -> anyhow::Result<()> {
        if let Some(path) = &self.path {
            crate::config::write_atomic(path, &serde_json::to_string(info)?, true)?;
        }
        Ok(())
    }

    pub async fn get(&self) -> UpdateInfo {
        self.info.read().await.clone()
    }

    pub async fn set(&self, status: &str, message: Option<String>) -> anyhow::Result<()> {
        let mut info = self.info.write().await;
        let mut next = info.clone();
        next.status = status.into();
        next.message = message;
        self.persist(&next)?;
        if status == "failed" {
            *self.cancellation.write().await = CancellationToken::new();
        }
        *info = next;
        Ok(())
    }

    pub async fn start(&self, _mode: &str) -> anyhow::Result<bool> {
        let mut info = self.info.write().await;
        if matches!(
            info.status.as_str(),
            "queued"
                | "draining"
                | "forcing"
                | "backing_up"
                | "updating"
                | "restart_required"
                | "startup_failed"
                | "recovery_required"
        ) {
            return Ok(false);
        }
        let next = UpdateInfo {
            status: "queued".into(),
            requested_at: Some(chrono::Utc::now().to_rfc3339()),
            ..Default::default()
        };
        self.persist(&next)?;
        *info = next;
        Ok(true)
    }

    pub async fn finish(&self, status: &str, message: Option<String>) {
        let _ = self.set(status, message).await;
    }

    pub async fn rejecting_requests(&self) -> bool {
        matches!(
            self.info.read().await.status.as_str(),
            "draining"
                | "forcing"
                | "backing_up"
                | "updating"
                | "restart_required"
                | "startup_failed"
                | "recovery_required"
        )
    }

    pub async fn token(&self) -> CancellationToken {
        self.cancellation.read().await.clone()
    }
    pub async fn cancel(&self) {
        self.token().await.cancel();
    }

    async fn checkpoint(
        &self,
        target: &str,
        backup: PathBuf,
        previous_binary: PathBuf,
    ) -> anyhow::Result<()> {
        let mut info = self.info.write().await;
        let mut next = info.clone();
        next.target = Some(target.into());
        next.backup = Some(backup);
        next.previous_binary = Some(previous_binary);
        next.status = "updating".into();
        self.persist(&next)?;
        *info = next;
        Ok(())
    }

    pub async fn validate_startup(&self) -> anyhow::Result<()> {
        let info = self.get().await;
        if matches!(
            info.status.as_str(),
            "updating" | "restart_required" | "startup_failed" | "recovery_required"
        ) {
            anyhow::ensure!(
                info.target.as_deref() == Some(env!("CARGO_PKG_VERSION")),
                "hosted update requires recovery: expected version {:?}, running {}; backup {:?}",
                info.target,
                env!("CARGO_PKG_VERSION"),
                info.backup
            );
        }
        Ok(())
    }

    pub async fn startup_complete(&self) -> anyhow::Result<()> {
        let info = self.get().await;
        if info.status == "idle" || info.status.is_empty() {
            return Ok(());
        }
        if matches!(
            info.status.as_str(),
            "updating" | "restart_required" | "startup_failed" | "recovery_required"
        ) {
            if info.target.as_deref() == Some(env!("CARGO_PKG_VERSION")) {
                self.set(
                    "succeeded",
                    Some(format!("Updated to {}", env!("CARGO_PKG_VERSION"))),
                )
                .await
            } else {
                anyhow::bail!("hosted update requires recovery: expected version {:?}, running {}; backup {:?}", info.target, env!("CARGO_PKG_VERSION"), info.backup)
            }
        } else if matches!(
            info.status.as_str(),
            "queued" | "draining" | "forcing" | "backing_up"
        ) {
            self.set(
                "failed",
                Some("Update interrupted before installation; current version is running".into()),
            )
            .await
        } else {
            Ok(())
        }
    }
}

pub async fn drain(
    active: Arc<Semaphore>,
    updates: &UpdateState,
    force: bool,
) -> anyhow::Result<tokio::sync::OwnedSemaphorePermit> {
    if force {
        updates.cancel().await;
    }
    let timeout = if force { 5 } else { 3600 };
    tokio::time::timeout(
        std::time::Duration::from_secs(timeout),
        active.acquire_many_owned(CAPACITY),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "active work did not stop; update aborted without changing the binary or database"
        )
    })?
    .map_err(|_| anyhow::anyhow!("server is shutting down"))
}

/// Returns without restarting only when the installed version is current. A failed update
/// after pool closure stays in maintenance so no handler uses a closed/partially migrated DB.
pub async fn perform(
    home: &Path,
    db_path: &str,
    active: Arc<Semaphore>,
    updates: &UpdateState,
    store: &crate::usage::Store,
    force: bool,
) -> anyhow::Result<()> {
    let Some(staged) = crate::update::stage(home).await? else {
        updates
            .set("succeeded", Some("Already up to date".into()))
            .await?;
        return Ok(());
    };
    let binary = crate::cli::installation_binary()?;
    updates
        .set(
            if force { "forcing" } else { "draining" },
            Some(UPDATE_MESSAGE.into()),
        )
        .await?;
    let _drained = drain(active, updates, force).await?;
    updates
        .set("backing_up", Some("Creating a database backup".into()))
        .await?;
    // Closing the shared pool waits for SQLite operations already submitted to its worker.
    store.close().await;
    let result = async {
        let backup = backup_db(home, db_path).await?;
        let previous_binary = backup.with_extension("fetchira");
        std::fs::copy(&binary, &previous_binary)?;
        updates
            .checkpoint(&staged.version, backup, previous_binary)
            .await?;
        crate::update::install(&staged)?;
        updates
            .set(
                "restart_required",
                Some(format!("Installed {}; restarting service", staged.version)),
            )
            .await?;
        restart_self(&binary)?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if let Err(error) = result {
        updates
            .set("recovery_required", Some(error.to_string()))
            .await?;
        return Err(error);
    }
    Ok(())
}

#[cfg(unix)]
fn restart_self(exe: &str) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;
    // exec retains args, FETCHIRA_HOME, encryption key environment, and the container PID.
    Err(std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .exec()
        .into())
}
#[cfg(not(unix))]
fn restart_self(_exe: &str) -> anyhow::Result<()> {
    anyhow::bail!(
        "hosted self-update requires Unix; restart the service to load the installed binary"
    )
}

pub async fn check_database(db_path: &str) -> anyhow::Result<()> {
    if !Path::new(db_path).exists() {
        return Ok(());
    }
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(db_path)
                .read_only(true),
        )
        .await?;
    let result: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await?;
    pool.close().await;
    anyhow::ensure!(result == "ok", "database integrity check failed: {result}");
    Ok(())
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

    #[tokio::test]
    async fn maintenance_modes_reject_duplicate_and_new_requests() {
        let state = UpdateState::default();
        assert!(state.start("drain").await.unwrap());
        assert!(!state.rejecting_requests().await);
        state.set("draining", None).await.unwrap();
        assert!(state.rejecting_requests().await);
        assert!(!state.start("force").await.unwrap());
        state.finish("failed", Some("network".into())).await;
        assert!(!state.rejecting_requests().await);
        assert!(state.start("force").await.unwrap());
    }

    #[tokio::test]
    async fn update_state_survives_restart() {
        let home =
            std::env::temp_dir().join(format!("fetchira-hosted-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let state = UpdateState::load(&home).unwrap();
        state
            .set("recovery_required", Some("backup is available".into()))
            .await
            .unwrap();
        let restored = UpdateState::load(&home).unwrap();
        assert_eq!(restored.get().await.status, "recovery_required");
        assert_eq!(
            restored.get().await.message.as_deref(),
            Some("backup is available")
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[tokio::test]
    async fn drain_waits_for_work_and_force_cancels_preexisting_requests() {
        let state = UpdateState::default();
        let active = Arc::new(Semaphore::new(CAPACITY as usize));
        let permit = active.clone().acquire_owned().await.unwrap();
        let drained = drain(active.clone(), &state, false);
        tokio::pin!(drained);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut drained)
                .await
                .is_err()
        );
        drop(permit);
        drop(drained.await.unwrap());

        // A request started before the update was queued must receive the same token.
        let token = state.token().await;
        let permit = active.clone().acquire_owned().await.unwrap();
        state.start("force").await.unwrap();
        let work = tokio::spawn(async move {
            let _permit = permit;
            token.cancelled().await;
            UPDATE_MESSAGE
        });
        let barrier = drain(active.clone(), &state, true).await.unwrap();
        assert_eq!(work.await.unwrap(), UPDATE_MESSAGE);
        assert_eq!(active.available_permits(), 0);
        drop(barrier);
        state
            .set("failed", Some("test failure".into()))
            .await
            .unwrap();
        assert!(
            !state.token().await.is_cancelled(),
            "failed updates must allow new work"
        );
    }

    #[tokio::test]
    async fn startup_validation_does_not_claim_migration_success() {
        let state = UpdateState::default();
        state
            .checkpoint(
                env!("CARGO_PKG_VERSION"),
                "backup.db".into(),
                "previous".into(),
            )
            .await
            .unwrap();
        state.validate_startup().await.unwrap();
        assert_eq!(state.get().await.status, "updating");
        state
            .set("startup_failed", Some("migration failed".into()))
            .await
            .unwrap();
        assert!(state.rejecting_requests().await);
        state.startup_complete().await.unwrap();
        assert_eq!(state.get().await.status, "succeeded");
    }
}
