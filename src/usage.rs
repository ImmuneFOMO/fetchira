use chrono::{Datelike, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqliteSynchronous};
use sqlx::Row;
use std::time::Duration;

use crate::auth;
use crate::config::Reset;
use crate::error::{Error, Result};

tokio::task_local! { pub static HOSTED_REQUEST_ID: String; }

/// Keep provider recovery hints useful in both the local dashboard and hosted MCP responses.
pub fn provider_login_hint(provider: &str) -> String {
    if HOSTED_REQUEST_ID.try_with(|_| ()).is_ok() {
        format!("re-authenticate {provider} in the hosted dashboard")
    } else {
        format!("run `fetchira login {provider}`")
    }
}

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

pub struct UsageRow {
    pub used: i64,
    pub exhausted: bool,
}

#[derive(Debug, Clone)]
pub struct ApiKeyRow {
    pub id: String,
    pub name: String,
    pub secret_hash: String,
    pub scopes: Vec<String>,
    pub revoked: bool,
    pub expires_at: Option<String>,
    pub rpm: i64,
    pub daily_limit: i64,
    pub monthly_limit: i64,
    pub concurrency_limit: i64,
    pub last_used_at: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct QuotaReservation {
    pub limit: i64,
    pub remaining: i64,
    pub reset_at: i64,
}

/// One recorded router decision (for the dashboard's live route log + history).
pub struct RouteRow {
    pub id: i64,
    pub ts: String,
    pub capability: String,
    pub provider: String,
    pub label: String,
    pub status: i64,
    pub latency_ms: i64,
    pub fail_from: Option<String>,
    pub fail_code: Option<i64>,
    pub niche: String,
    pub debug_id: Option<i64>,
}

/// What the router hands to `log_route` after each call (success, with optional failover origin).
pub struct RouteLog<'a> {
    pub capability: &'a str,
    pub provider: &'a str,
    pub label: &'a str,
    pub status: i64,
    pub latency_ms: i64,
    pub fail_from: Option<&'a str>,
    pub fail_code: Option<i64>,
    /// `native`/`rewrite`/`` — how the chosen provider served the request's niche knobs.
    pub niche: &'a str,
    /// The debug_log row for this route's winning attempt, for drill-down from the Activity tab.
    pub debug_id: Option<i64>,
}

/// One full request/response capture (the debug firehose — every attempt, success or failure).
pub struct DebugRow {
    pub id: i64,
    pub ts: String,
    pub capability: String,
    pub provider: String,
    pub label: String,
    pub status: i64,
    pub latency_ms: i64,
    pub request: String,
    pub response: Option<String>,
    pub error: Option<String>,
    /// Raw HTTP round-trip(s) for the attempt, as a JSON array string (api-key providers only).
    pub http_trace: Option<String>,
}

/// What the router hands to `log_debug` after every provider attempt.
pub struct DebugLog<'a> {
    pub capability: &'a str,
    pub provider: &'a str,
    pub label: &'a str,
    pub status: i64,
    pub latency_ms: i64,
    pub request: &'a str,
    pub response: Option<&'a str>,
    pub error: Option<&'a str>,
    pub http_trace: Option<&'a str>,
}

/// Per-field char cap and row cap for the debug log. The product (~0.5 GB) is the hard ceiling
/// regardless of `retention_hours`, so a burst can't fill the disk before the time sweep runs.
const DEBUG_BODY_CAP: usize = 128 * 1024;
const DEBUG_MAX_ROWS: i64 = 4000;

/// Bump ONLY on a breaking schema change (additive `IF NOT EXISTS`/`ADD COLUMN` stays free).
/// Must match the repo-root `schema-version` file, which ships as a release asset so the
/// updater can refuse a breaking swap while old-version MCP servers are still running.
pub const SCHEMA: i64 = 2;

/// Additive `CREATE TABLE IF NOT EXISTS` is safe under older binaries. Only stamp `user_version`
/// when no other fetchira process is using the file (or this is a brand-new db).
fn should_bump_user_version(current: i64, peer_count: usize) -> bool {
    current < SCHEMA && (current == 0 || peer_count == 0)
}

async fn usage_has_exhausted_kind(pool: &SqlitePool) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM pragma_table_info('usage') WHERE name = 'exhausted_kind'",
    )
    .fetch_one(pool)
    .await?
        != 0)
}

impl Store {
    pub async fn open(path: &str) -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            // WAL so readers don't block the writer when several processes share this file.
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePool::connect_with(opts).await?;
        let v: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&pool)
            .await?;
        if v > SCHEMA {
            return Err(Error::Schema(format!(
                "database schema v{v} is newer than this fetchira (schema v{SCHEMA}) — \
                 restart this MCP server (or update this tool's fetchira) to pick up the new binary"
            )));
        }
        let peers = if v < SCHEMA && v > 0 {
            crate::instances::running(&crate::cli::home(), &[std::process::id()]).len()
        } else {
            0
        };
        let bump = should_bump_user_version(v, peers);
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS usage (
                provider  TEXT    NOT NULL,
                label     TEXT    NOT NULL,
                period    TEXT    NOT NULL,
                used      INTEGER NOT NULL DEFAULT 0,
                exhausted INTEGER NOT NULL DEFAULT 0,
                exhausted_kind TEXT,
                PRIMARY KEY (label, period)
            )",
        )
        .execute(&pool)
        .await?;
        // Old versions marked both temporary 429s and real 402s as exhausted. New durable marks
        // carry `quota`; a NULL legacy mark may be cleared only after a fresh positive balance.
        if !usage_has_exhausted_kind(&pool).await? {
            if let Err(err) = sqlx::query("ALTER TABLE usage ADD COLUMN exhausted_kind TEXT")
                .execute(&pool)
                .await
            {
                // Two fresh CLI/MCP processes can both observe the legacy schema. The loser of
                // the ALTER race is healthy only when the winner really added the column.
                if !usage_has_exhausted_kind(&pool).await? {
                    return Err(err.into());
                }
            }
        }
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS provider_cooldown (
                label TEXT PRIMARY KEY,
                until_ms INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS admin_session (token_hash TEXT PRIMARY KEY, created_at TEXT NOT NULL, expires_at TEXT NOT NULL)")
            .execute(&pool).await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS proxy_assignment (
                label TEXT PRIMARY KEY,
                proxy TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS web_session (
                label    TEXT PRIMARY KEY,
                provider TEXT NOT NULL,
                cookies  TEXT NOT NULL,
                updated  TEXT NOT NULL,
                identity TEXT,
                plan     TEXT,
                limits   TEXT
            )",
        )
        .execute(&pool)
        .await?;
        // Existing DBs predate `identity` (account email, for the dashboard + dup detection);
        // add it idempotently (ignore "duplicate column name").
        sqlx::query("ALTER TABLE web_session ADD COLUMN identity TEXT")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("ALTER TABLE web_session ADD COLUMN plan TEXT")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("ALTER TABLE web_session ADD COLUMN limits TEXT")
            .execute(&pool)
            .await
            .ok();
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS route_log (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                ts         TEXT    NOT NULL,
                capability TEXT    NOT NULL,
                provider   TEXT    NOT NULL,
                label      TEXT    NOT NULL,
                status     INTEGER NOT NULL,
                latency_ms INTEGER NOT NULL,
                fail_from  TEXT,
                fail_code  INTEGER,
                niche      TEXT    NOT NULL DEFAULT '',
                debug_id   INTEGER
            )",
        )
        .execute(&pool)
        .await?;
        // Existing DBs predate `niche`; add it idempotently (ignore "duplicate column name").
        sqlx::query("ALTER TABLE route_log ADD COLUMN niche TEXT NOT NULL DEFAULT ''")
            .execute(&pool)
            .await
            .ok();
        // Same for `debug_id` — links each route to its debug_log capture for drill-down.
        sqlx::query("ALTER TABLE route_log ADD COLUMN debug_id INTEGER")
            .execute(&pool)
            .await
            .ok();
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS debug_log (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                ts         TEXT    NOT NULL,
                capability TEXT    NOT NULL,
                provider   TEXT    NOT NULL,
                label      TEXT    NOT NULL,
                status     INTEGER NOT NULL,
                latency_ms INTEGER NOT NULL,
                request    TEXT    NOT NULL,
                response   TEXT,
                error      TEXT,
                http_trace TEXT
            )",
        )
        .execute(&pool)
        .await?;
        // Existing DBs predate `http_trace`; add it idempotently (ignore "duplicate column name").
        sqlx::query("ALTER TABLE debug_log ADD COLUMN http_trace TEXT")
            .execute(&pool)
            .await
            .ok();
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS api_key (
            id TEXT PRIMARY KEY, name TEXT NOT NULL, secret_hash TEXT NOT NULL UNIQUE,
            scopes TEXT NOT NULL DEFAULT 'mcp', rpm INTEGER NOT NULL DEFAULT 60,
            daily_limit INTEGER NOT NULL DEFAULT 0, monthly_limit INTEGER NOT NULL DEFAULT 0,
            revoked INTEGER NOT NULL DEFAULT 0, expires_at TEXT, created_at TEXT NOT NULL,
            last_used_at TEXT
        )",
        )
        .execute(&pool)
        .await?;
        sqlx::query("ALTER TABLE api_key ADD COLUMN daily_limit INTEGER NOT NULL DEFAULT 0")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("ALTER TABLE api_key ADD COLUMN monthly_limit INTEGER NOT NULL DEFAULT 0")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("ALTER TABLE api_key ADD COLUMN concurrency_limit INTEGER NOT NULL DEFAULT 4")
            .execute(&pool)
            .await
            .ok();
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS request_log (
            id TEXT PRIMARY KEY, created_at TEXT NOT NULL, api_key_id TEXT,
            capability TEXT NOT NULL, status INTEGER NOT NULL, latency_ms INTEGER NOT NULL,
            provider TEXT, attempts INTEGER NOT NULL DEFAULT 0, error TEXT,
            query_hash TEXT, query_preview TEXT
        )",
        )
        .execute(&pool)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS request_log_key_time ON request_log(api_key_id, created_at)").execute(&pool).await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS request_attempt (
            id INTEGER PRIMARY KEY AUTOINCREMENT, request_id TEXT NOT NULL,
            provider TEXT NOT NULL, account_label TEXT NOT NULL, status INTEGER NOT NULL,
            latency_ms INTEGER NOT NULL, failure TEXT, winner INTEGER NOT NULL DEFAULT 0
        )",
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS request_attempt_request ON request_attempt(request_id)",
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS audit_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT, created_at TEXT NOT NULL,
            actor TEXT NOT NULL, action TEXT NOT NULL, target TEXT, detail TEXT
        )",
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS login_challenge (
            id TEXT PRIMARY KEY, provider TEXT NOT NULL, label TEXT NOT NULL,
            api_key_id TEXT NOT NULL, expires_at TEXT NOT NULL, consumed_at TEXT
        )",
        )
        .execute(&pool)
        .await?;
        if bump {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "PRAGMA user_version = {SCHEMA}"
            )))
            .execute(&pool)
            .await?;
        }
        Ok(Self { pool })
    }

    pub async fn save_api_key(&self, key: &auth::ApiKey, name: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO api_key (id,name,secret_hash,scopes,created_at) VALUES (?,?,?,?,?)",
        )
        .bind(&key.id)
        .bind(name)
        .bind(&key.hash)
        .bind(key.scopes.join(","))
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn save_api_key_with_limits(
        &self,
        key: &auth::ApiKey,
        name: &str,
        rpm: i64,
        daily_limit: i64,
        monthly_limit: i64,
        concurrency_limit: i64,
        expires_at: Option<&str>,
    ) -> Result<()> {
        sqlx::query("INSERT INTO api_key (id,name,secret_hash,scopes,rpm,daily_limit,monthly_limit,concurrency_limit,expires_at,created_at) VALUES (?,?,?,?,?,?,?,?,?,?)")
            .bind(&key.id).bind(name).bind(&key.hash).bind(key.scopes.join(","))
            .bind(rpm).bind(daily_limit).bind(monthly_limit).bind(concurrency_limit)
            .bind(expires_at).bind(Utc::now().to_rfc3339()).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn save_admin_session(&self, token_hash: &str, expires_at: &str) -> Result<()> {
        sqlx::query("INSERT INTO admin_session (token_hash,created_at,expires_at) VALUES (?,?,?)")
            .bind(token_hash)
            .bind(Utc::now().to_rfc3339())
            .bind(expires_at)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn valid_admin_session(&self, token_hash: &str) -> Result<bool> {
        let row = sqlx::query("SELECT expires_at FROM admin_session WHERE token_hash = ?")
            .bind(token_hash)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row
            .and_then(|r| {
                r.get::<String, _>("expires_at")
                    .parse::<chrono::DateTime<Utc>>()
                    .ok()
            })
            .is_some_and(|e| e > Utc::now()))
    }

    pub async fn api_key(&self, id: &str) -> Result<Option<ApiKeyRow>> {
        let row = sqlx::query(
            "SELECT id,name,secret_hash,scopes,revoked,expires_at,rpm,daily_limit,monthly_limit,concurrency_limit,last_used_at FROM api_key WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| ApiKeyRow {
            id: r.get("id"),
            name: r.get("name"),
            secret_hash: r.get("secret_hash"),
            scopes: r
                .get::<String, _>("scopes")
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
            revoked: r.get::<i64, _>("revoked") != 0,
            expires_at: r.get("expires_at"),
            rpm: r.get("rpm"),
            daily_limit: r.get("daily_limit"),
            monthly_limit: r.get("monthly_limit"),
            concurrency_limit: r.get("concurrency_limit"),
            last_used_at: r.get("last_used_at"),
        }))
    }

    pub async fn list_api_keys(&self) -> Result<Vec<ApiKeyRow>> {
        let rows = sqlx::query("SELECT id,name,secret_hash,scopes,revoked,expires_at,rpm,daily_limit,monthly_limit,concurrency_limit,last_used_at FROM api_key ORDER BY created_at")
            .fetch_all(&self.pool).await?;
        Ok(rows
            .into_iter()
            .map(|r| ApiKeyRow {
                id: r.get("id"),
                name: r.get("name"),
                secret_hash: r.get("secret_hash"),
                scopes: r
                    .get::<String, _>("scopes")
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect(),
                revoked: r.get::<i64, _>("revoked") != 0,
                expires_at: r.get("expires_at"),
                rpm: r.get("rpm"),
                daily_limit: r.get("daily_limit"),
                monthly_limit: r.get("monthly_limit"),
                concurrency_limit: r.get("concurrency_limit"),
                last_used_at: r.get("last_used_at"),
            })
            .collect())
    }

    pub async fn revoke_api_key(&self, id: &str) -> Result<bool> {
        let n = sqlx::query("UPDATE api_key SET revoked = 1 WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(n != 0)
    }

    pub async fn touch_api_key(&self, id: &str) -> Result<()> {
        sqlx::query("UPDATE api_key SET last_used_at = ? WHERE id = ?")
            .bind(Utc::now().to_rfc3339())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn count_key_since(&self, id: &str, cutoff: &str) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "SELECT COUNT(*) FROM request_log WHERE api_key_id = ? AND created_at >= ?",
        )
        .bind(id)
        .bind(cutoff)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn count_key_period(&self, id: &str, start: &str) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "SELECT COUNT(*) FROM request_log WHERE api_key_id = ? AND created_at >= ?",
        )
        .bind(id)
        .bind(start)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn log_request(
        &self,
        id: &str,
        key_id: &str,
        status: i64,
        latency_ms: i64,
    ) -> Result<()> {
        sqlx::query("UPDATE request_log SET status = ?, latency_ms = ?, attempts = 1 WHERE id = ? AND api_key_id = ?")
            .bind(status).bind(latency_ms).bind(id).bind(key_id).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn cancel_request(&self, id: &str, key_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM request_log WHERE id = ? AND api_key_id = ?")
            .bind(id)
            .bind(key_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Atomically reserve a user request before provider work starts. This closes the race where
    /// concurrent requests could all pass a read-only quota check.
    pub async fn reserve_request(
        &self,
        id: &str,
        key_id: &str,
        rpm: i64,
        daily_limit: i64,
        monthly_limit: i64,
    ) -> Result<Option<QuotaReservation>> {
        let mut tx = self
            .pool
            .begin_with(sqlx::AssertSqlSafe(String::from("BEGIN IMMEDIATE")))
            .await?;
        let now = Utc::now();
        let minute = (now - chrono::Duration::minutes(1)).to_rfc3339();
        let day = (now - chrono::Duration::days(1)).to_rfc3339();
        let month = now
            .with_day(1)
            .unwrap_or(now)
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .to_rfc3339();
        let counts: (i64, i64, i64) = (
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM request_log WHERE api_key_id = ? AND created_at >= ?",
            )
            .bind(key_id)
            .bind(&minute)
            .fetch_one(&mut *tx)
            .await?,
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM request_log WHERE api_key_id = ? AND created_at >= ?",
            )
            .bind(key_id)
            .bind(&day)
            .fetch_one(&mut *tx)
            .await?,
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM request_log WHERE api_key_id = ? AND created_at >= ?",
            )
            .bind(key_id)
            .bind(&month)
            .fetch_one(&mut *tx)
            .await?,
        );
        let allowed = (rpm <= 0 || counts.0 < rpm)
            && (daily_limit <= 0 || counts.1 < daily_limit)
            && (monthly_limit <= 0 || counts.2 < monthly_limit);
        if allowed {
            sqlx::query("INSERT INTO request_log (id,created_at,api_key_id,capability,status,latency_ms,attempts) VALUES (?,?,?,?,?,?,?)")
                .bind(id).bind(now.to_rfc3339()).bind(key_id).bind("mcp").bind(0_i64).bind(0_i64).bind(1_i64).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        if !allowed {
            return Ok(None);
        }
        let constrained = [(rpm, counts.0, 60_i64), (daily_limit, counts.1, 86_400)]
            .into_iter()
            .chain(std::iter::once((
                monthly_limit,
                counts.2,
                (month.parse::<chrono::DateTime<Utc>>().unwrap_or(now) + chrono::Months::new(1)
                    - now)
                    .num_seconds()
                    .max(1),
            )))
            .filter(|(limit, _, _)| *limit > 0)
            .min_by_key(|(limit, used, _)| limit - used);
        let (limit, used, reset) = constrained.unwrap_or((0, 0, 60));
        Ok(Some(QuotaReservation {
            limit,
            remaining: if limit > 0 {
                (limit - used - 1).max(0)
            } else {
                -1
            },
            reset_at: now.timestamp() + reset,
        }))
    }

    pub async fn recent_requests(
        &self,
        key_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<serde_json::Value>> {
        let rows = if let Some(key) = key_id {
            sqlx::query("SELECT id,created_at,api_key_id,capability,status,latency_ms,attempts,error FROM request_log WHERE api_key_id = ? ORDER BY created_at DESC LIMIT ?").bind(key).bind(limit).fetch_all(&self.pool).await?
        } else {
            sqlx::query("SELECT id,created_at,api_key_id,capability,status,latency_ms,attempts,error FROM request_log ORDER BY created_at DESC LIMIT ?").bind(limit).fetch_all(&self.pool).await?
        };
        Ok(rows.into_iter().map(|r| serde_json::json!({"id":r.get::<String,_>("id"),"createdAt":r.get::<String,_>("created_at"),"apiKeyId":r.get::<Option<String>,_>("api_key_id"),"capability":r.get::<String,_>("capability"),"status":r.get::<i64,_>("status"),"latencyMs":r.get::<i64,_>("latency_ms"),"attempts":r.get::<i64,_>("attempts"),"error":r.get::<Option<String>,_>("error")})).collect())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn log_attempt(
        &self,
        request_id: &str,
        provider: &str,
        account: &str,
        status: i64,
        latency_ms: i64,
        failure: Option<&str>,
        winner: bool,
    ) -> Result<()> {
        sqlx::query("INSERT INTO request_attempt (request_id,provider,account_label,status,latency_ms,failure,winner) VALUES (?,?,?,?,?,?,?)")
            .bind(request_id).bind(provider).bind(account).bind(status).bind(latency_ms).bind(failure).bind(winner as i64).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn log_audit(
        &self,
        actor: &str,
        action: &str,
        target: Option<&str>,
        detail: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO audit_log (created_at,actor,action,target,detail) VALUES (?,?,?,?,?)",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(actor)
        .bind(action)
        .bind(target)
        .bind(detail)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn attempts_for(&self, request_id: &str) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query("SELECT provider,account_label,status,latency_ms,failure,winner FROM request_attempt WHERE request_id = ? ORDER BY id")
            .bind(request_id).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|r| serde_json::json!({"provider":r.get::<String,_>("provider"),"account":r.get::<String,_>("account_label"),"status":r.get::<i64,_>("status"),"latencyMs":r.get::<i64,_>("latency_ms"),"failure":r.get::<Option<String>,_>("failure"),"winner":r.get::<i64,_>("winner") != 0})).collect())
    }

    pub async fn recent_audit(&self, limit: i64) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query("SELECT id,created_at,actor,action,target,detail FROM audit_log ORDER BY id DESC LIMIT ?")
            .bind(limit.clamp(1, 1000)).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|r| serde_json::json!({"id":r.get::<i64,_>("id"),"createdAt":r.get::<String,_>("created_at"),"actor":r.get::<String,_>("actor"),"action":r.get::<String,_>("action"),"target":r.get::<Option<String>,_>("target"),"detail":r.get::<Option<String>,_>("detail")})).collect())
    }

    pub async fn last_audit_at(&self, actor: &str, action: &str) -> Result<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT created_at FROM audit_log WHERE actor = ? AND action = ? ORDER BY id DESC LIMIT 1",
        )
        .bind(actor)
        .bind(action)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn key_was_checked(&self, id: &str) -> Result<bool> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM audit_log WHERE actor = ? AND action = 'remote.check'",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?
            > 0)
    }

    pub async fn save_login_challenge(
        &self,
        id: &str,
        provider: &str,
        label: &str,
        key_id: &str,
        expires_at: &str,
    ) -> Result<()> {
        sqlx::query("INSERT INTO login_challenge(id,provider,label,api_key_id,expires_at) VALUES(?,?,?,?,?)")
            .bind(id).bind(provider).bind(label).bind(key_id).bind(expires_at)
            .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn login_challenge(
        &self,
        id: &str,
        key_id: &str,
    ) -> Result<Option<(String, String, String, bool)>> {
        let row = sqlx::query("SELECT provider,label,expires_at,consumed_at FROM login_challenge WHERE id = ? AND (api_key_id = ? OR api_key_id = '*')")
            .bind(id).bind(key_id).fetch_optional(&self.pool).await?;
        Ok(row.map(|r| {
            (
                r.get("provider"),
                r.get("label"),
                r.get("expires_at"),
                r.get::<Option<String>, _>("consumed_at").is_some(),
            )
        }))
    }

    pub async fn login_challenge_any(
        &self,
        id: &str,
    ) -> Result<Option<(String, String, String, bool)>> {
        let row = sqlx::query(
            "SELECT provider,label,expires_at,consumed_at FROM login_challenge WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| {
            (
                r.get("provider"),
                r.get("label"),
                r.get("expires_at"),
                r.get::<Option<String>, _>("consumed_at").is_some(),
            )
        }))
    }

    pub async fn consume_challenge_and_save_session(
        &self,
        id: &str,
        key_id: &str,
        provider: &str,
        label: &str,
        session: &str,
    ) -> Result<bool> {
        let encrypted =
            crate::secrets::encrypt(session).map_err(|e| Error::Config(e.to_string()))?;
        let mut tx = self.pool.begin().await?;
        let now = Utc::now().to_rfc3339();
        let changed = sqlx::query("UPDATE login_challenge SET consumed_at = ? WHERE id = ? AND (api_key_id = ? OR api_key_id = '*') AND consumed_at IS NULL AND expires_at > ?")
            .bind(&now).bind(id).bind(key_id).bind(&now).execute(&mut *tx).await?.rows_affected() == 1;
        if !changed {
            tx.rollback().await?;
            return Ok(false);
        }
        sqlx::query("INSERT INTO web_session(label,provider,cookies,updated) VALUES(?,?,?,?) ON CONFLICT(label) DO UPDATE SET provider=excluded.provider,cookies=excluded.cookies,updated=excluded.updated")
            .bind(label).bind(provider).bind(encrypted).bind(&now).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn find_api_key_by_hash(&self, hash: &str) -> Result<Option<ApiKeyRow>> {
        let row = sqlx::query("SELECT id,name,secret_hash,scopes,revoked,expires_at,rpm,daily_limit,monthly_limit,concurrency_limit,last_used_at FROM api_key WHERE secret_hash = ?")
            .bind(hash).fetch_optional(&self.pool).await?;
        Ok(row.map(|r| ApiKeyRow {
            id: r.get("id"),
            name: r.get("name"),
            secret_hash: r.get("secret_hash"),
            scopes: r
                .get::<String, _>("scopes")
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
            revoked: r.get::<i64, _>("revoked") != 0,
            expires_at: r.get("expires_at"),
            rpm: r.get("rpm"),
            daily_limit: r.get("daily_limit"),
            monthly_limit: r.get("monthly_limit"),
            concurrency_limit: r.get("concurrency_limit"),
            last_used_at: r.get("last_used_at"),
        }))
    }

    pub async fn save_session(&self, label: &str, provider: &str, cookies: &str) -> Result<()> {
        let stored = if crate::secrets::configured() {
            crate::secrets::encrypt(cookies).map_err(|e| Error::Config(e.to_string()))?
        } else {
            cookies.to_string()
        };
        sqlx::query(
            "INSERT INTO web_session (label, provider, cookies, updated) VALUES (?, ?, ?, ?)
             ON CONFLICT(label) DO UPDATE SET cookies = excluded.cookies, updated = excluded.updated",
        )
        .bind(label)
        .bind(provider)
        .bind(stored)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn load_session(&self, label: &str) -> Result<Option<String>> {
        let row = sqlx::query("SELECT cookies FROM web_session WHERE label = ?")
            .bind(label)
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let stored: String = row.get("cookies");
        if stored.starts_with("enc:") {
            return crate::secrets::decrypt(&stored)
                .map(Some)
                .map_err(|e| Error::Config(e.to_string()));
        }
        if crate::secrets::configured() {
            let encrypted =
                crate::secrets::encrypt(&stored).map_err(|e| Error::Config(e.to_string()))?;
            sqlx::query("UPDATE web_session SET cookies = ? WHERE label = ? AND cookies = ?")
                .bind(encrypted)
                .bind(label)
                .bind(&stored)
                .execute(&self.pool)
                .await?;
        }
        Ok(Some(stored))
    }

    /// Record the account identity (email) captured at login — for the dashboard + dup detection.
    pub async fn set_identity(&self, label: &str, identity: &str) -> Result<()> {
        sqlx::query("UPDATE web_session SET identity = ? WHERE label = ?")
            .bind(identity)
            .bind(label)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn load_identity(&self, label: &str) -> Result<Option<String>> {
        let row = sqlx::query("SELECT identity FROM web_session WHERE label = ?")
            .bind(label)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.and_then(|r| r.get::<Option<String>, _>("identity")))
    }

    /// Another account of the same provider already logged into this identity → its label. Lets the
    /// add flow reject re-adding an account you already have.
    pub async fn identity_conflict(
        &self,
        provider: &str,
        identity: &str,
        exclude: &str,
    ) -> Result<Option<String>> {
        let row = sqlx::query(
            "SELECT label FROM web_session WHERE provider = ? AND identity = ? AND label != ? LIMIT 1",
        )
        .bind(provider)
        .bind(identity)
        .bind(exclude)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.get("label")))
    }

    pub async fn set_plan(&self, label: &str, plan: &str) -> Result<()> {
        sqlx::query("UPDATE web_session SET plan = ? WHERE label = ?")
            .bind(plan)
            .bind(label)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn load_plan(&self, label: &str) -> Result<Option<String>> {
        let row = sqlx::query("SELECT plan FROM web_session WHERE label = ?")
            .bind(label)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.and_then(|r| r.get::<Option<String>, _>("plan")))
    }

    pub async fn set_limits(&self, label: &str, limits: &str) -> Result<()> {
        sqlx::query("UPDATE web_session SET limits = ? WHERE label = ?")
            .bind(limits)
            .bind(label)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn load_limits(&self, label: &str) -> Result<Option<String>> {
        let row = sqlx::query("SELECT limits FROM web_session WHERE label = ?")
            .bind(label)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.and_then(|r| r.get::<Option<String>, _>("limits")))
    }

    /// All captured account identities (label -> email), for the dashboard rows.
    pub async fn all_identities(&self) -> Result<std::collections::HashMap<String, String>> {
        let rows =
            sqlx::query("SELECT label, identity FROM web_session WHERE identity IS NOT NULL")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get("label"), r.get("identity")))
            .collect())
    }

    /// Remove all rows for an account (usage, cooldown, proxy assignment, web session).
    pub async fn delete_account(&self, label: &str) -> Result<()> {
        for owned in [
            label.to_string(),
            format!("{label}#dr"),
            format!("{label}#image"),
        ] {
            sqlx::query("DELETE FROM usage WHERE label = ?")
                .bind(&owned)
                .execute(&self.pool)
                .await?;
            sqlx::query("DELETE FROM provider_cooldown WHERE label = ?")
                .bind(&owned)
                .execute(&self.pool)
                .await?;
        }
        sqlx::query("DELETE FROM proxy_assignment WHERE label = ?")
            .bind(label)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM web_session WHERE label = ?")
            .bind(label)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Drop an account's sticky pool assignment so its next call re-resolves — used after its proxy
    /// setting changes, so a fresh proxy takes effect instead of the old pinned one.
    pub async fn clear_proxy(&self, label: &str) -> Result<()> {
        sqlx::query("DELETE FROM proxy_assignment WHERE label = ?")
            .bind(label)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Move an account's rows to a new label — usage (incl. feature budgets), cooldown, proxy
    /// assignment, and web session. The label is the identity key, so a rename must carry these or
    /// the session/quota are orphaned. Mirrors `delete_account`.
    pub async fn rename_account(&self, old: &str, new: &str) -> Result<()> {
        for (from, to) in [
            (old.to_string(), new.to_string()),
            (format!("{old}#dr"), format!("{new}#dr")),
            (format!("{old}#image"), format!("{new}#image")),
        ] {
            sqlx::query("UPDATE usage SET label = ? WHERE label = ?")
                .bind(&to)
                .bind(&from)
                .execute(&self.pool)
                .await?;
            sqlx::query("UPDATE provider_cooldown SET label = ? WHERE label = ?")
                .bind(&to)
                .bind(&from)
                .execute(&self.pool)
                .await?;
        }
        sqlx::query("UPDATE proxy_assignment SET label = ? WHERE label = ?")
            .bind(new)
            .bind(old)
            .execute(&self.pool)
            .await?;
        sqlx::query("UPDATE web_session SET label = ? WHERE label = ?")
            .bind(new)
            .bind(old)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn remaining(&self, label: &str, quota: i64, period: &str) -> Result<i64> {
        let u = self.usage_for(label, period).await?;
        Ok(if u.exhausted {
            0
        } else {
            (quota - u.used).max(0)
        })
    }

    pub async fn usage_for(&self, label: &str, period: &str) -> Result<UsageRow> {
        let row = sqlx::query("SELECT used, exhausted FROM usage WHERE label = ? AND period = ?")
            .bind(label)
            .bind(period)
            .fetch_optional(&self.pool)
            .await?;
        Ok(match row {
            Some(r) => UsageRow {
                used: r.get("used"),
                exhausted: r.get::<i64, _>("exhausted") != 0,
            },
            None => UsageRow {
                used: 0,
                exhausted: false,
            },
        })
    }

    pub async fn legacy_exhausted(&self, label: &str, period: &str) -> Result<bool> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM usage
             WHERE label = ? AND period = ? AND exhausted = 1 AND exhausted_kind IS NULL)",
        )
        .bind(label)
        .bind(period)
        .fetch_one(&self.pool)
        .await?
            != 0)
    }

    pub async fn quota_exhausted(&self, label: &str, period: &str) -> Result<bool> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM usage
             WHERE label = ? AND period = ? AND exhausted = 1 AND exhausted_kind = 'quota')",
        )
        .bind(label)
        .bind(period)
        .fetch_one(&self.pool)
        .await?
            != 0)
    }

    /// Clear only a pre-upgrade exhaustion mark. The condition prevents a concurrent real 402
    /// from being undone after the balance request started.
    pub async fn clear_legacy_exhausted(&self, label: &str, period: &str) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE usage SET exhausted = 0
             WHERE label = ? AND period = ? AND exhausted = 1 AND exhausted_kind IS NULL",
        )
        .bind(label)
        .bind(period)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn clear_quota_exhausted(&self, label: &str, period: &str) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE usage SET exhausted = 0, exhausted_kind = NULL
             WHERE label = ? AND period = ? AND exhausted = 1 AND exhausted_kind = 'quota'",
        )
        .bind(label)
        .bind(period)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Persist a provider-request cooldown so CLI invocations and separate MCP processes honor it.
    pub async fn set_cooldown(&self, label: &str, delay: Duration) -> Result<()> {
        let millis = delay.as_millis().max(1).min(i64::MAX as u128) as i64;
        let until = Utc::now().timestamp_millis().saturating_add(millis);
        sqlx::query(
            "INSERT INTO provider_cooldown (label, until_ms) VALUES (?, ?)
             ON CONFLICT(label) DO UPDATE SET
                until_ms = max(provider_cooldown.until_ms, excluded.until_ms)",
        )
        .bind(label)
        .bind(until)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn cooldown_remaining(&self, label: &str) -> Result<Option<Duration>> {
        let now = Utc::now().timestamp_millis();
        let until =
            sqlx::query_scalar::<_, i64>("SELECT until_ms FROM provider_cooldown WHERE label = ?")
                .bind(label)
                .fetch_optional(&self.pool)
                .await?;
        let Some(until) = until else { return Ok(None) };
        if until > now {
            return Ok(Some(Duration::from_millis((until - now) as u64)));
        }
        Ok(None)
    }

    /// Claim the next probe window after a persisted cooldown. Keeping expired rows makes this
    /// atomic across independent CLI, MCP, and hosted processes sharing the database.
    pub async fn claim_cooldown(&self, label: &str, delay: Duration) -> Result<Option<i64>> {
        let now = Utc::now().timestamp_millis();
        let millis = delay.as_millis().max(1).min(i64::MAX as u128) as i64;
        let until = now.saturating_add(millis);
        let result = sqlx::query(
            "INSERT INTO provider_cooldown (label, until_ms) VALUES (?, ?)
             ON CONFLICT(label) DO UPDATE SET until_ms = excluded.until_ms
             WHERE provider_cooldown.until_ms <= ?",
        )
        .bind(label)
        .bind(until)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok((result.rows_affected() == 1).then_some(until))
    }

    /// Release only the probe lease this caller acquired. A newer 429/402 cooldown wins the race.
    pub async fn release_cooldown(&self, label: &str, until: i64) -> Result<()> {
        sqlx::query("DELETE FROM provider_cooldown WHERE label = ? AND until_ms = ?")
            .bind(label)
            .bind(until)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Atomically claim `cost` units, or return `false` if granting it would exceed `quota` (or the
    /// account is exhausted) — closes the read-then-write race where concurrent callers all clear
    /// the same `remaining > 0` gate. Caller must `refund` if the reserved attempt then fails.
    pub async fn reserve(
        &self,
        provider: &str,
        label: &str,
        quota: i64,
        period: &str,
        cost: i64,
    ) -> Result<bool> {
        let res = sqlx::query(
            "INSERT INTO usage (provider, label, period, used) VALUES (?, ?, ?, ?)
             ON CONFLICT(label, period) DO UPDATE SET used = used + excluded.used
                WHERE exhausted = 0 AND used + excluded.used <= ?",
        )
        .bind(provider)
        .bind(label)
        .bind(period)
        .bind(cost)
        .bind(quota)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Hand back a reserved `cost` after the attempt it was claimed for failed.
    pub async fn refund(&self, label: &str, period: &str, cost: i64) -> Result<()> {
        sqlx::query("UPDATE usage SET used = max(used - ?, 0) WHERE label = ? AND period = ?")
            .bind(cost)
            .bind(label)
            .bind(period)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn record(&self, provider: &str, label: &str, period: &str, cost: i64) -> Result<()> {
        sqlx::query(
            "INSERT INTO usage (provider, label, period, used) VALUES (?, ?, ?, ?)
             ON CONFLICT(label, period) DO UPDATE SET used = used + excluded.used",
        )
        .bind(provider)
        .bind(label)
        .bind(period)
        .bind(cost)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_exhausted(&self, provider: &str, label: &str, period: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO usage (provider, label, period, used, exhausted, exhausted_kind)
             VALUES (?, ?, ?, 0, 1, 'quota')
             ON CONFLICT(label, period) DO UPDATE SET exhausted = 1, exhausted_kind = 'quota'",
        )
        .bind(provider)
        .bind(label)
        .bind(period)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn assign_proxy(&self, label: &str, proxy: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO proxy_assignment (label, proxy) VALUES (?, ?)
             ON CONFLICT(label) DO NOTHING",
        )
        .bind(label)
        .bind(proxy)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn assignment_count(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) AS n FROM proxy_assignment")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get("n"))
    }

    pub async fn proxy_for(&self, label: &str) -> Result<Option<String>> {
        let row = sqlx::query("SELECT proxy FROM proxy_assignment WHERE label = ?")
            .bind(label)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.get("proxy")))
    }

    pub async fn log_route(&self, e: &RouteLog<'_>) -> Result<()> {
        let res = sqlx::query(
            "INSERT INTO route_log
                (ts, capability, provider, label, status, latency_ms, fail_from, fail_code, niche, debug_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(e.capability)
        .bind(e.provider)
        .bind(e.label)
        .bind(e.status)
        .bind(e.latency_ms)
        .bind(e.fail_from)
        .bind(e.fail_code)
        .bind(e.niche)
        .bind(e.debug_id)
        .execute(&self.pool)
        .await?;
        // Keep the table bounded (~last 1000 rows), amortized so it isn't run every call.
        if res.last_insert_rowid() % 200 == 0 {
            sqlx::query("DELETE FROM route_log WHERE id <= ?")
                .bind(res.last_insert_rowid() - 1000)
                .execute(&self.pool)
                .await
                .ok();
        }
        Ok(())
    }

    pub async fn recent_routes(&self, limit: i64) -> Result<Vec<RouteRow>> {
        let rows = sqlx::query(
            "SELECT id, ts, capability, provider, label, status, latency_ms, fail_from, fail_code, niche, debug_id
             FROM route_log ORDER BY id DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        let mut out: Vec<RouteRow> = rows.iter().map(route_row).collect();
        out.reverse(); // chronological (oldest first)
        Ok(out)
    }

    pub async fn routes_since(&self, after_id: i64, limit: i64) -> Result<Vec<RouteRow>> {
        let rows = sqlx::query(
            "SELECT id, ts, capability, provider, label, status, latency_ms, fail_from, fail_code, niche, debug_id
             FROM route_log WHERE id > ? ORDER BY id ASC LIMIT ?",
        )
        .bind(after_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(route_row).collect())
    }

    pub async fn max_route_id(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COALESCE(MAX(id), 0) AS m FROM route_log")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get("m"))
    }

    pub async fn log_debug(&self, e: &DebugLog<'_>, retention_hours: i64) -> Result<i64> {
        let res = sqlx::query(
            "INSERT INTO debug_log
                (ts, capability, provider, label, status, latency_ms, request, response, error, http_trace)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(e.capability)
        .bind(e.provider)
        .bind(e.label)
        .bind(e.status)
        .bind(e.latency_ms)
        .bind(clip(e.request))
        .bind(e.response.map(clip))
        .bind(e.error.map(clip))
        .bind(e.http_trace.map(clip))
        .execute(&self.pool)
        .await?;
        // Amortized eviction: drop rows past the row cap or older than the retention window.
        let id = res.last_insert_rowid();
        if id % 50 == 0 {
            let cutoff =
                (Utc::now() - chrono::Duration::hours(retention_hours.max(0))).to_rfc3339();
            sqlx::query("DELETE FROM debug_log WHERE id <= ? OR ts < ?")
                .bind(id - DEBUG_MAX_ROWS)
                .bind(cutoff)
                .execute(&self.pool)
                .await
                .ok();
        }
        Ok(id)
    }

    /// Newest-first page for the Debug tab's initial load. Bodies are returned in full; the
    /// handler trims them to previews and serves the full row from `debug_get`.
    pub async fn recent_debug(&self, limit: i64) -> Result<Vec<DebugRow>> {
        let rows = sqlx::query(
            "SELECT id, ts, capability, provider, label, status, latency_ms, request, response, error, http_trace
             FROM debug_log ORDER BY id DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(debug_row).collect())
    }

    /// New rows since `after_id`, oldest-first, for incremental polling.
    pub async fn debug_since(&self, after_id: i64, limit: i64) -> Result<Vec<DebugRow>> {
        let rows = sqlx::query(
            "SELECT id, ts, capability, provider, label, status, latency_ms, request, response, error, http_trace
             FROM debug_log WHERE id > ? ORDER BY id ASC LIMIT ?",
        )
        .bind(after_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(debug_row).collect())
    }

    pub async fn debug_get(&self, id: i64) -> Result<Option<DebugRow>> {
        let row = sqlx::query(
            "SELECT id, ts, capability, provider, label, status, latency_ms, request, response, error, http_trace
             FROM debug_log WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.as_ref().map(debug_row))
    }

    pub async fn max_debug_id(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COALESCE(MAX(id), 0) AS m FROM debug_log")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get("m"))
    }
}

/// Truncate at a char boundary so a giant scrape can't bloat one row, marking what was dropped.
fn clip(s: &str) -> String {
    if s.len() <= DEBUG_BODY_CAP {
        return s.to_string();
    }
    let mut end = DEBUG_BODY_CAP;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[+{} bytes truncated]", &s[..end], s.len() - end)
}

fn debug_row(r: &sqlx::sqlite::SqliteRow) -> DebugRow {
    DebugRow {
        id: r.get("id"),
        ts: r.get("ts"),
        capability: r.get("capability"),
        provider: r.get("provider"),
        label: r.get("label"),
        status: r.get("status"),
        latency_ms: r.get("latency_ms"),
        request: r.get("request"),
        response: r.get("response"),
        error: r.get("error"),
        http_trace: r.get("http_trace"),
    }
}

fn route_row(r: &sqlx::sqlite::SqliteRow) -> RouteRow {
    RouteRow {
        id: r.get("id"),
        ts: r.get("ts"),
        capability: r.get("capability"),
        provider: r.get("provider"),
        label: r.get("label"),
        status: r.get("status"),
        latency_ms: r.get("latency_ms"),
        fail_from: r.get("fail_from"),
        fail_code: r.get("fail_code"),
        niche: r.get("niche"),
        debug_id: r.get("debug_id"),
    }
}

pub fn period_key(reset: Reset) -> String {
    match reset {
        Reset::Monthly => Utc::now().format("%Y-%m").to_string(),
        Reset::Daily => Utc::now().format("%Y-%m-%d").to_string(),
        Reset::Once => "lifetime".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex, OnceLock};
    use tokio::sync::Barrier;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn schema_version_asset_matches_const() {
        let file: i64 = include_str!("../schema-version").trim().parse().unwrap();
        assert_eq!(file, SCHEMA);
    }

    #[test]
    fn old_schema_bumps_only_without_peers() {
        assert!(should_bump_user_version(0, 5));
        assert!(should_bump_user_version(1, 0));
        assert!(!should_bump_user_version(1, 3));
        assert!(!should_bump_user_version(SCHEMA, 0));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_legacy_open_adds_exhaustion_kind_once() {
        let path = std::env::temp_dir().join(format!(
            "fetchira_migration_race_{}_{}.db",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_file(&path);
        let legacy = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE usage (
                provider TEXT NOT NULL, label TEXT NOT NULL, period TEXT NOT NULL,
                used INTEGER NOT NULL DEFAULT 0, exhausted INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (label, period)
            )",
        )
        .execute(&legacy)
        .await
        .unwrap();
        legacy.close().await;

        let path = Arc::new(path.to_string_lossy().into_owned());
        let barrier = Arc::new(Barrier::new(8));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let path = Arc::clone(&path);
            let barrier = Arc::clone(&barrier);
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                Store::open(&path).await
            }));
        }
        let mut stores = Vec::new();
        for task in tasks {
            stores.push(task.await.unwrap().expect("concurrent Store::open"));
        }
        assert!(usage_has_exhausted_kind(&stores[0].pool).await.unwrap());
        for store in stores {
            store.pool.close().await;
        }
        let _ = std::fs::remove_file(path.as_str());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cooldown_probe_has_one_winner() {
        let path = std::env::temp_dir().join(format!(
            "fetchira_probe_race_{}_{}.db",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_file(&path);
        let store = Store::open(path.to_str().unwrap()).await.unwrap();
        sqlx::query("INSERT INTO provider_cooldown(label, until_ms) VALUES ('account-1', 0)")
            .execute(&store.pool)
            .await
            .unwrap();

        let barrier = Arc::new(Barrier::new(8));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let store = store.clone();
            let barrier = Arc::clone(&barrier);
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                store
                    .claim_cooldown("account-1", Duration::from_secs(60))
                    .await
                    .unwrap()
                    .is_some()
            }));
        }
        let mut winners = 0;
        for task in tasks {
            winners += usize::from(task.await.unwrap());
        }
        assert_eq!(winners, 1);
        store.pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn hosted_provider_hint_changes_only_inside_hosted_request() {
        assert!(provider_login_hint("grok_web").contains("fetchira login"));
        let hint = HOSTED_REQUEST_ID
            .scope("test".into(), async { provider_login_hint("grok_web") })
            .await;
        assert!(hint.contains("hosted dashboard"));
    }

    #[tokio::test]
    async fn too_new_schema_is_refused() {
        let path = std::env::temp_dir().join(format!("fetchira_schema_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = Store::open(path.to_str().unwrap()).await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "PRAGMA user_version = {}",
            SCHEMA + 1
        )))
        .execute(&store.pool)
        .await
        .unwrap();
        drop(store);
        let Err(err) = Store::open(path.to_str().unwrap()).await else {
            panic!("too-new schema must be refused");
        };
        assert!(err.to_string().contains("newer than this fetchira"));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn route_log_roundtrips() {
        // A temp file, not ":memory:" — each pooled connection gets its own in-memory db.
        let path = std::env::temp_dir().join(format!("fetchira_route_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = Store::open(path.to_str().unwrap()).await.unwrap();
        assert_eq!(store.max_route_id().await.unwrap(), 0);

        store
            .log_route(&RouteLog {
                capability: "search",
                provider: "serper",
                label: "serper-1",
                status: 200,
                latency_ms: 198,
                fail_from: None,
                fail_code: None,
                niche: "",
                debug_id: None,
            })
            .await
            .unwrap();
        store
            .log_route(&RouteLog {
                capability: "search",
                provider: "tavily",
                label: "tavily-1",
                status: 200,
                latency_ms: 312,
                fail_from: Some("exa-1"),
                fail_code: Some(429),
                niche: "native",
                debug_id: Some(7),
            })
            .await
            .unwrap();

        let recent = store.recent_routes(10).await.unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].label, "serper-1"); // chronological: oldest first
        assert_eq!(recent[1].fail_from.as_deref(), Some("exa-1"));
        assert_eq!(recent[1].niche, "native");
        assert_eq!(recent[1].debug_id, Some(7));

        let since = store.routes_since(recent[0].id, 10).await.unwrap();
        assert_eq!(since.len(), 1);
        assert_eq!(since[0].label, "tavily-1");
        assert_eq!(store.max_route_id().await.unwrap(), recent[1].id);

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn debug_log_roundtrips() {
        let path = std::env::temp_dir().join(format!("fetchira_debug_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = Store::open(path.to_str().unwrap()).await.unwrap();

        store
            .log_debug(
                &DebugLog {
                    capability: "search",
                    provider: "grok_web",
                    label: "grok-1",
                    status: 403,
                    latency_ms: 812,
                    request: r#"{"query":"hi"}"#,
                    response: None,
                    error: Some("grok anti-bot rejected this request"),
                    http_trace: Some(r#"[{"method":"POST","status":403}]"#),
                },
                24,
            )
            .await
            .unwrap();

        let recent = store.recent_debug(10).await.unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].status, 403);
        assert_eq!(
            recent[0].error.as_deref(),
            Some("grok anti-bot rejected this request")
        );
        assert!(recent[0].response.is_none());
        assert_eq!(
            recent[0].http_trace.as_deref(),
            Some(r#"[{"method":"POST","status":403}]"#)
        );

        let id = recent[0].id;
        assert_eq!(store.max_debug_id().await.unwrap(), id);
        assert!(store.debug_since(id, 10).await.unwrap().is_empty());
        assert_eq!(store.debug_get(id).await.unwrap().unwrap().label, "grok-1");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn clip_truncates_on_char_boundary() {
        let short = "é".repeat(10);
        assert_eq!(clip(&short), short);
        // A 4-byte char repeated past the cap: clipping must not split it mid-codepoint.
        let big = "𝓍".repeat(DEBUG_BODY_CAP); // 4 bytes each
        let out = clip(&big);
        assert!(out.contains("truncated"));
        assert!(out.starts_with('𝓍'));
    }

    #[tokio::test]
    async fn reserve_gates_quota() {
        let path = std::env::temp_dir().join(format!("fetchira_reserve_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = Store::open(path.to_str().unwrap()).await.unwrap();

        // Quota 3: the first three reservations win, the fourth is denied.
        for _ in 0..3 {
            assert!(store
                .reserve("grok_web", "grok-1#dr", 3, "d", 1)
                .await
                .unwrap());
        }
        assert!(!store
            .reserve("grok_web", "grok-1#dr", 3, "d", 1)
            .await
            .unwrap());
        assert_eq!(store.remaining("grok-1#dr", 3, "d").await.unwrap(), 0);

        // A refund frees exactly one slot back.
        store.refund("grok-1#dr", "d", 1).await.unwrap();
        assert!(store
            .reserve("grok_web", "grok-1#dr", 3, "d", 1)
            .await
            .unwrap());

        // Once exhausted, no reservation succeeds even with budget refunded.
        store.refund("grok-1#dr", "d", 1).await.unwrap();
        store
            .mark_exhausted("grok_web", "grok-1#dr", "d")
            .await
            .unwrap();
        assert!(!store
            .reserve("grok_web", "grok-1#dr", 3, "d", 1)
            .await
            .unwrap());

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn hosted_sessions_are_encrypted_and_legacy_rows_migrate() {
        let _guard = env_lock();
        let path =
            std::env::temp_dir().join(format!("fetchira_session_crypto_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = Store::open(path.to_str().unwrap()).await.unwrap();
        std::env::set_var("FETCHIRA_MASTER_KEY", "session-test-master");
        store
            .save_session("encrypted", "grok_web", "super-secret-cookie")
            .await
            .unwrap();
        let raw: String = sqlx::query_scalar("SELECT cookies FROM web_session WHERE label = ?")
            .bind("encrypted")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert!(raw.starts_with("enc:"));
        assert!(!raw.contains("super-secret-cookie"));
        assert_eq!(
            store.load_session("encrypted").await.unwrap().as_deref(),
            Some("super-secret-cookie")
        );

        sqlx::query("INSERT INTO web_session(label,provider,cookies,updated) VALUES(?,?,?,?)")
            .bind("legacy")
            .bind("grok_web")
            .bind("legacy-cookie")
            .bind(Utc::now().to_rfc3339())
            .execute(&store.pool)
            .await
            .unwrap();
        assert_eq!(
            store.load_session("legacy").await.unwrap().as_deref(),
            Some("legacy-cookie")
        );
        let migrated: String =
            sqlx::query_scalar("SELECT cookies FROM web_session WHERE label = ?")
                .bind("legacy")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert!(migrated.starts_with("enc:"));
        std::env::remove_var("FETCHIRA_MASTER_KEY");
        drop(store);
        let _ = std::fs::remove_file(&path);
    }
}
