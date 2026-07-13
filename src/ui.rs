//! Local web dashboard (`fetchira ui`). Serves the embedded `webui/` design and a small
//! JSON API built from the live router, so a human can watch quota and manage accounts in a
//! browser. Loopback-only, token + Host (+ Origin on writes) guarded. Never runs unless asked
//! (explicit `ui`, or bare `fetchira` from a TTY) — the MCP stdio server is untouched.

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router as AxumRouter};
use chrono::{DateTime, Datelike, Utc};
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::cli;
use crate::config;
use crate::providers::{Capability, Input, ProviderKind};
use crate::router::Router;
use crate::usage::{DebugRow, RouteRow, Store};

#[derive(RustEmbed)]
#[folder = "webui/"]
struct Assets;

struct AcctMeta {
    kind: ProviderKind,
    has_key: bool,
    is_web: bool,
    logged_in: bool,
    /// Configured proxy intent ("pool" | url | absent → direct), as opposed to the resolved sticky
    /// URL the router routes through — so the dashboard shows and edits the setting, not the pick.
    proxy: Option<String>,
}

/// The parts that a config mutation rebuilds; behind an RwLock so add/remove/login take effect live.
struct Inner {
    router: Arc<Router>,
    meta: HashMap<String, AcctMeta>,
    priority: config::Priority,
}

struct AppState {
    home: PathBuf,
    token: String,
    port: u16,
    store: Store,
    inner: RwLock<Inner>,
}

pub async fn run(home: &Path) -> anyhow::Result<()> {
    // A missing/empty config is fine: the dashboard opens in its onboarding state and the
    // first `POST /api/account/add` writes fetchira.toml.
    let cfg = cli::load_or_empty(home);
    let store = Store::open(&config::resolve_db(home, &cfg.db_path)).await?;
    let inner = build_inner(home, &store).await?;
    // A post-update re-exec passes the old token and port through so the open tab keeps working.
    let token = std::env::var("FETCHIRA_UI_TOKEN").unwrap_or_else(|_| gen_token());
    let port = std::env::var("FETCHIRA_UI_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(7878);
    let listener = bind_local(port).await?;
    let addr = listener.local_addr()?;
    let state = Arc::new(AppState {
        home: home.to_path_buf(),
        token: token.clone(),
        port: addr.port(),
        store,
        inner: RwLock::new(inner),
    });

    // Best-effort: fill in account emails for already-logged-in accounts (otherwise missing until
    // their next login) so the dashboard shows them. Background — doesn't delay the first paint.
    tokio::spawn(backfill_identities(home.to_path_buf(), state.store.clone()));

    // Fresh update check at launch, then every 15 min while the UI runs — the daily throttle
    // would otherwise hide a release published after the last passive check.
    {
        let h = home.to_path_buf();
        tokio::spawn(async move {
            loop {
                crate::update::refresh(&h).await;
                tokio::time::sleep(Duration::from_secs(15 * 60)).await;
            }
        });
    }

    // Keep the live-limit/balance caches warm in the background so the dashboard's cached snapshot
    // paints instantly and fills in per provider as each fetch lands — never blocking on a cold
    // fan-out. Clone the Arc out from under the lock so `warm`'s network I/O holds no guard.
    {
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                let router = st.inner.read().await.router.clone();
                router.warm().await;
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
        });
    }

    let app = AxumRouter::new()
        .route("/api/state", get(api_state))
        .route("/api/events", get(api_events))
        .route("/api/debug", get(api_debug))
        .route("/api/debug/{id}", get(api_debug_one))
        .route("/api/account/add", post(api_add))
        .route("/api/account/remove", post(api_remove))
        .route("/api/account/login", post(api_login))
        .route("/api/account/session", post(api_session))
        .route("/api/account/rename", post(api_rename))
        .route("/api/account/proxy", post(api_proxy))
        .route("/api/account/test", post(api_test))
        .route("/api/priority", post(api_priority))
        .route("/api/try", post(api_try))
        .route("/api/install/targets", get(api_install_targets))
        .route("/api/install", post(api_install))
        .route("/api/update", post(api_update))
        .fallback(static_handler)
        .with_state(state);

    let url = format!("http://{addr}/ui_kits/dashboard/index.html?token={token}");
    eprintln!("fetchira ui — open {url}");
    // Over SSH there's no local browser to open, and the loopback bind isn't reachable
    // remotely — print the tunnel command instead of silently failing to open.
    let ssh = std::env::var("SSH_CONNECTION").is_ok() || std::env::var("SSH_TTY").is_ok();
    if ssh {
        let port = addr.port();
        eprintln!("  remote shell detected — tunnel it from your machine, then open the URL:");
        eprintln!("  ssh -L {port}:127.0.0.1:{port} <this-host>");
    } else if std::env::var("FETCHIRA_NO_OPEN").is_err() {
        let _ = open::that(&url);
    }
    axum::serve(listener, app).await?;
    Ok(())
}

/// (Re)build the router + per-account metadata from the on-disk config. Called at startup
/// and after every mutation so the dashboard reflects the new state immediately.
async fn build_inner(home: &Path, store: &Store) -> anyhow::Result<Inner> {
    let cfg = cli::load_or_empty(home);
    let mut meta = HashMap::new();
    for a in &cfg.accounts {
        let logged_in = a.provider.is_web() && store.load_session(&a.label).await?.is_some();
        meta.insert(
            a.label.clone(),
            AcctMeta {
                kind: a.provider,
                has_key: a.api_key.is_some(),
                is_web: a.provider.is_web(),
                logged_in,
                proxy: a.proxy.clone(),
            },
        );
    }
    let priority = cfg.priority.clone();
    let router = Router::build(cfg, store.clone()).await?;
    Ok(Inner {
        router: Arc::new(router),
        meta,
        priority,
    })
}

async fn rebuild(st: &AppState) {
    if let Ok(inner) = build_inner(&st.home, &st.store).await {
        *st.inner.write().await = inner;
    }
}

/// One-shot best-effort: capture the account email for every logged-in web/dashboard account that
/// doesn't have one yet, so upgraded configs show emails without a forced re-login.
async fn backfill_identities(home: PathBuf, store: Store) {
    let Ok(cfg) = config::load(home.join("fetchira.toml").to_str().unwrap_or("")) else {
        return;
    };
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
        let session = crate::web::parse_session(&raw);
        let proxy = a.proxy.as_deref().filter(|p| p.starts_with("http"));
        if let Ok(client) = crate::web::build_client(&session.cookies, &session.headers, proxy) {
            if let Some(id) = crate::providers::Provider::new(a.provider)
                .account_identity(&client)
                .await
            {
                let _ = store.set_identity(&a.label, &id).await;
            }
        }
    }
}

async fn bind_local(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    // Bookmarkable default port, but never collide: fall back to an ephemeral one.
    match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => Ok(l),
        Err(_) => tokio::net::TcpListener::bind(("127.0.0.1", 0)).await,
    }
}

fn gen_token() -> String {
    let mut buf = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut buf);
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Loopback dashboard holds quota data → require a per-session token and a loopback Host
/// (defeats DNS-rebinding). Applies to the API; static assets carry no secrets.
fn guard(st: &AppState, headers: &HeaderMap) -> Option<Response> {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !(host.starts_with("127.0.0.1") || host.starts_with("localhost")) {
        return Some((StatusCode::FORBIDDEN, "bad host").into_response());
    }
    let tok = headers
        .get("x-fetchira-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if tok != st.token {
        return Some((StatusCode::UNAUTHORIZED, "bad token").into_response());
    }
    None
}

/// Mutating endpoints additionally reject any non-loopback Origin (CSRF defense on top of the
/// token, which already can't be forged cross-site since it rides a custom header).
fn guard_mut(st: &AppState, headers: &HeaderMap) -> Option<Response> {
    if let Some(r) = guard(st, headers) {
        return Some(r);
    }
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|h| h.to_str().ok()) {
        if !(origin.starts_with("http://127.0.0.1") || origin.starts_with("http://localhost")) {
            return Some((StatusCode::FORBIDDEN, "bad origin").into_response());
        }
    }
    None
}

async fn api_state(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(resp) = guard(&st, &headers) {
        return resp;
    }
    let built = {
        let inner = st.inner.read().await;
        build_state(&inner, &st.store).await
    };
    match built {
        Ok(mut v) => {
            if let Some(u) = crate::update::ui_banner(&st.home).await {
                v["update"] = u;
            }
            Json(v).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// SSE stream of new route-log rows for the live feed. EventSource can't set headers,
/// so the token rides the query string (`/api/events?token=…`).
async fn api_events(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !(host.starts_with("127.0.0.1") || host.starts_with("localhost")) {
        return (StatusCode::FORBIDDEN, "bad host").into_response();
    }
    if q.get("token").map(String::as_str) != Some(st.token.as_str()) {
        return (StatusCode::UNAUTHORIZED, "bad token").into_response();
    }
    let store = st.store.clone();
    let start = store.max_route_id().await.unwrap_or(0);
    let stream = futures_util::stream::unfold((store, start), |(store, last)| async move {
        loop {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            if let Ok(rows) = store.routes_since(last, 100).await {
                if let Some(newest) = rows.last() {
                    let next = newest.id;
                    let entries: Vec<Value> = rows.iter().map(route_to_entry).collect();
                    let data = serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string());
                    return Some((
                        Ok::<Event, Infallible>(Event::default().data(data)),
                        (store, next),
                    ));
                }
            }
        }
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// Debug firehose feed: recent attempts (`after=0`) or just new ones (`after=<id>`), each with a
/// body preview. The full request/response/error is fetched per-row from `/api/debug/{id}`.
async fn api_debug(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if let Some(resp) = guard(&st, &headers) {
        return resp;
    }
    let after: i64 = q.get("after").and_then(|s| s.parse().ok()).unwrap_or(0);
    let limit: i64 = q
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(100)
        .clamp(1, 500);
    let rows = if after > 0 {
        st.store.debug_since(after, limit).await
    } else {
        st.store.recent_debug(limit).await.map(|mut r| {
            r.reverse(); // ascending, so the client appends and tracks the max id
            r
        })
    };
    match rows {
        Ok(rows) => {
            let max_id = rows.last().map(|r| r.id).unwrap_or(after);
            let entries: Vec<Value> = rows.iter().map(debug_to_entry).collect();
            Json(json!({ "rows": entries, "maxId": max_id })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn api_debug_one(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<i64>,
) -> Response {
    if let Some(resp) = guard(&st, &headers) {
        return resp;
    }
    match st.store.debug_get(id).await {
        Ok(Some(r)) => {
            let trace = r.http_trace.as_deref().map(|t| {
                serde_json::from_str::<Value>(t).unwrap_or_else(|_| Value::String(t.to_string()))
            });
            Json(json!({
                "id": r.id, "ts": r.ts, "capability": r.capability, "provider": r.provider,
                "label": r.label, "status": r.status, "latencyMs": r.latency_ms,
                "request": r.request, "response": r.response, "error": r.error,
                "httpTrace": trace,
            }))
            .into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct AddReq {
    provider: String,
    label: Option<String>,
    key: Option<String>,
    proxy: Option<String>,
    /// Web providers only: a session pasted by hand instead of the guided browser login.
    session: Option<String>,
    /// Web providers only: "chrome" | "firefox" — which browser to open for the guided login.
    browser: Option<String>,
}

#[derive(Deserialize)]
struct LabelReq {
    label: String,
    /// Login only: "chrome" | "firefox". Ignored by remove/test.
    browser: Option<String>,
}

#[derive(Deserialize)]
struct SessionReq {
    label: String,
    session: String,
}

#[derive(Deserialize)]
struct RenameReq {
    label: String,
    new_label: String,
}

#[derive(Deserialize)]
struct ProxyReq {
    label: String,
    /// Raw user input: "" / "direct" → direct, "pool" → sticky pool, else a proxy URL.
    proxy: String,
}

async fn api_add(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<AddReq>,
) -> Response {
    if let Some(r) = guard_mut(&st, &headers) {
        return r;
    }
    let Some(kind) = parse_kind(&req.provider) else {
        return (StatusCode::BAD_REQUEST, "unknown provider").into_response();
    };
    let proxy = req.proxy.filter(|s| !s.trim().is_empty());
    let label = match cli::add_account(&st.home, kind, req.label.as_deref(), req.key, proxy) {
        Ok(l) => l,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    // Web providers also need a session; the account is written either way. A pasted session
    // skips the browser entirely (the only path that works on a headless box).
    if kind.is_web() {
        let outcome = match req.session.filter(|s| !s.trim().is_empty()) {
            Some(raw) => cli::set_session(&st.home, &label, &raw).await.map(|_| ()),
            None => cli::capture_login(&st.home, &label, req.browser).await,
        };
        if let Err(e) = outcome {
            // A brand-new account whose first login failed: drop it so it doesn't linger
            // session-less and consume the auto-name (gemini-2 -> gap on the next add).
            let _ = cli::remove_account(&st.home, &label).await;
            rebuild(&st).await;
            return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
        }
        // Reject if this login duplicates an account you already have (same identity).
        if let Ok(Some((other, id))) = cli::identity_dup(&st.home, kind, &label).await {
            let _ = cli::remove_account(&st.home, &label).await;
            rebuild(&st).await;
            return (
                StatusCode::BAD_REQUEST,
                format!("that account ({id}) is already added as '{other}' — log in with a different one"),
            )
                .into_response();
        }
    }
    rebuild(&st).await;
    Json(json!({ "ok": true, "label": label })).into_response()
}

async fn api_session(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<SessionReq>,
) -> Response {
    if let Some(r) = guard_mut(&st, &headers) {
        return r;
    }
    match cli::set_session(&st.home, &req.label, &req.session).await {
        Ok(n) => {
            rebuild(&st).await;
            Json(json!({ "ok": true, "cookies": n })).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn api_remove(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<LabelReq>,
) -> Response {
    if let Some(r) = guard_mut(&st, &headers) {
        return r;
    }
    match cli::remove_account(&st.home, &req.label).await {
        Ok(()) => {
            rebuild(&st).await;
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn api_login(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<LabelReq>,
) -> Response {
    if let Some(r) = guard_mut(&st, &headers) {
        return r;
    }
    match cli::capture_login(&st.home, &req.label, req.browser).await {
        Ok(()) => {
            rebuild(&st).await;
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn api_rename(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<RenameReq>,
) -> Response {
    if let Some(r) = guard_mut(&st, &headers) {
        return r;
    }
    match cli::rename_account(&st.home, &req.label, &req.new_label).await {
        Ok(()) => {
            rebuild(&st).await;
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn api_proxy(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ProxyReq>,
) -> Response {
    if let Some(r) = guard_mut(&st, &headers) {
        return r;
    }
    match cli::set_proxy(&st.home, &req.label, cli::parse_proxy_arg(&req.proxy)).await {
        Ok(()) => {
            rebuild(&st).await;
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct PriorityReq {
    capability: String,
    /// The full provider order for the capability; empty = back to the built-in default.
    #[serde(default)]
    order: Vec<String>,
}

async fn api_priority(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<PriorityReq>,
) -> Response {
    if let Some(r) = guard_mut(&st, &headers) {
        return r;
    }
    let Some(cap) = Capability::parse(&req.capability).filter(|c| cli::PRIORITY_CAPS.contains(c))
    else {
        return (StatusCode::BAD_REQUEST, "unknown capability").into_response();
    };
    let mut kinds = Vec::with_capacity(req.order.len());
    for s in &req.order {
        let Some(k) = parse_kind(s) else {
            return (StatusCode::BAD_REQUEST, format!("unknown provider '{s}'")).into_response();
        };
        kinds.push(k);
    }
    match cli::set_priority(&st.home, cap, kinds) {
        Ok(()) => {
            rebuild(&st).await;
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn api_test(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<LabelReq>,
) -> Response {
    if let Some(r) = guard_mut(&st, &headers) {
        return r;
    }
    let inner = st.inner.read().await;
    let Some(meta) = inner.meta.get(&req.label) else {
        return (StatusCode::BAD_REQUEST, "unknown account").into_response();
    };
    let kind = meta.kind;
    let (cap, input) = test_call(kind);
    let t0 = Instant::now();
    let res = inner.router.call(cap, &input, Some(kind)).await;
    let ms = t0.elapsed().as_millis() as i64;
    match res {
        Ok(_) => Json(json!({ "ok": true, "latencyMs": ms })).into_response(),
        Err(e) => {
            Json(json!({ "ok": false, "latencyMs": ms, "error": e.to_string() })).into_response()
        }
    }
}

#[derive(Deserialize)]
struct TryReq {
    q: String,
}

/// Onboarding's "try it": one real routed search, answering with the text plus which
/// provider/account served it (read back from the route row the call just logged).
async fn api_try(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<TryReq>,
) -> Response {
    if let Some(r) = guard_mut(&st, &headers) {
        return r;
    }
    let q = req.q.trim();
    if q.is_empty() {
        return (StatusCode::BAD_REQUEST, "empty query").into_response();
    }
    let router = st.inner.read().await.router.clone();
    let input = Input {
        query: Some(q.to_string()),
        ..Default::default()
    };
    let before = st.store.max_route_id().await.unwrap_or(0);
    let t0 = Instant::now();
    let res = router.call(Capability::Search, &input, None).await;
    let ms = t0.elapsed().as_millis() as i64;
    match res {
        Ok(reply) => {
            let served = st
                .store
                .routes_since(before, 10)
                .await
                .ok()
                .and_then(|rows| rows.into_iter().rev().find(|r| r.status == 200));
            Json(json!({
                "ok": true,
                "latencyMs": ms,
                "provider": served.as_ref().map(|r| r.provider.clone()),
                "label": served.as_ref().map(|r| r.label.clone()),
                "text": reply.text.chars().take(2000).collect::<String>(),
            }))
            .into_response()
        }
        Err(e) => {
            Json(json!({ "ok": false, "latencyMs": ms, "error": e.to_string() })).into_response()
        }
    }
}

async fn api_install_targets(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(r) = guard(&st, &headers) {
        return r;
    }
    let targets: Vec<Value> = cli::mcp_target_list()
        .iter()
        .map(|t| json!({ "name": t.name, "present": t.present, "installed": t.installed }))
        .collect();
    Json(json!({ "targets": targets })).into_response()
}

#[derive(Deserialize)]
struct InstallReq {
    targets: Vec<String>,
}

async fn api_install(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<InstallReq>,
) -> Response {
    if let Some(r) = guard_mut(&st, &headers) {
        return r;
    }
    let bin = match std::env::current_exe() {
        Ok(p) => p.to_string_lossy().into_owned(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let all = cli::mcp_target_list();
    let results: Vec<Value> = req
        .targets
        .iter()
        .map(|name| match all.iter().find(|t| t.name == name) {
            Some(t) => match (t.run)(&bin) {
                Ok(msg) => json!({ "name": name, "ok": true, "msg": msg }),
                Err(e) => json!({ "name": name, "ok": false, "msg": e.to_string() }),
            },
            None => json!({ "name": name, "ok": false, "msg": "unknown target" }),
        })
        .collect();
    Json(json!({ "results": results })).into_response()
}

/// The dashboard's Update button: self-update in place, then tell the user to restart.
/// Brew-managed installs can't self-replace — the response says to `brew upgrade` instead.
#[cfg(unix)]
fn restart_self(token: &str, port: u16) {
    use std::os::unix::process::CommandExt;
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let err = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env("FETCHIRA_UI_TOKEN", token)
        .env("FETCHIRA_UI_PORT", port.to_string())
        .env("FETCHIRA_NO_OPEN", "1")
        .exec();
    eprintln!("restart after update failed: {err}");
}

#[cfg(not(unix))]
fn restart_self(_token: &str, _port: u16) {}

async fn api_update(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(r) = guard_mut(&st, &headers) {
        return r;
    }
    match crate::update::perform(&st.home).await {
        Ok(crate::update::Outcome::Brew) => Json(json!({
            "ok": false,
            "msg": "installed via Homebrew — run `brew upgrade fetchira` in a terminal",
        }))
        .into_response(),
        Ok(crate::update::Outcome::UpToDate) => {
            Json(json!({ "ok": true, "msg": "already up to date" })).into_response()
        }
        Ok(crate::update::Outcome::Updated(v)) => {
            // Flush the response, then re-exec the new binary. Running MCP servers keep the
            // old inode (in-flight work is safe) and pick the update on their next spawn.
            let (token, port) = (st.token.clone(), st.port);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(500)).await;
                restart_self(&token, port);
            });
            Json(json!({
                "ok": true,
                "restarted": true,
                "msg": format!("updated to {v} — restarting the dashboard…"),
            }))
            .into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

/// A capability-appropriate probe call for a provider (real request → counts against quota,
/// and shows up in the route log).
fn test_call(kind: ProviderKind) -> (Capability, Input) {
    match kind {
        ProviderKind::Firecrawl => (
            Capability::Read,
            Input {
                url: Some("https://example.com".to_string()),
                ..Default::default()
            },
        ),
        ProviderKind::Steel => (
            Capability::Browser,
            Input {
                url: Some("https://example.com".to_string()),
                ..Default::default()
            },
        ),
        _ => (
            Capability::Search,
            Input {
                query: Some("fetchira connectivity test".to_string()),
                ..Default::default()
            },
        ),
    }
}

fn parse_kind(s: &str) -> Option<ProviderKind> {
    serde_json::from_value(Value::String(s.to_string())).ok()
}

fn route_to_entry(r: &RouteRow) -> Value {
    let time = r.ts.get(11..19).unwrap_or("").to_string();
    let mut entry = if let Some(from) = &r.fail_from {
        json!({
            "time": time, "capability": r.capability,
            "failover": { "from": from, "code": r.fail_code, "to": r.label },
            "status": r.status, "latencyMs": r.latency_ms,
        })
    } else {
        json!({
            "time": time, "capability": r.capability, "provider": r.provider,
            "account": acct_num(&r.label), "status": r.status, "latencyMs": r.latency_ms,
        })
    };
    if !r.niche.is_empty() {
        entry["niche"] = json!(r.niche);
    }
    if let Some(id) = r.debug_id {
        entry["debugId"] = json!(id);
    }
    entry
}

/// A debug-feed row: metadata + small request inline, plus a one-line preview of the body. The
/// full response/error is loaded lazily when a row is expanded.
fn debug_to_entry(r: &DebugRow) -> Value {
    let body = r.response.as_deref().or(r.error.as_deref()).unwrap_or("");
    json!({
        "id": r.id,
        "time": r.ts.get(11..19).unwrap_or("").to_string(),
        "capability": r.capability,
        "provider": r.provider,
        "account": acct_num(&r.label),
        "status": r.status,
        "latencyMs": r.latency_ms,
        "ok": r.status == 200,
        "request": r.request,
        "preview": preview(body),
    })
}

fn preview(s: &str) -> String {
    s.chars()
        .take(180)
        .collect::<String>()
        .replace(['\n', '\r'], " ")
}

fn acct_num(label: &str) -> i64 {
    label
        .rsplit('-')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
}

fn ago(ts: &str) -> String {
    let Ok(then) = DateTime::parse_from_rfc3339(ts) else {
        return "—".to_string();
    };
    let secs = (Utc::now() - then.with_timezone(&Utc)).num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

async fn static_handler(uri: Uri) -> Response {
    let mut path = uri.path().trim_start_matches('/').to_string();
    if path.is_empty() {
        path = "ui_kits/dashboard/index.html".to_string();
    }
    match Assets::get(&path) {
        // no-cache: a restarted/upgraded server must not run against stale cached JSX.
        Some(c) => (
            [
                (header::CONTENT_TYPE, mime_for(&path)),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            c.data.into_owned(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("jsx") => "application/javascript; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("json") => "application/json",
        Some("md") => "text/markdown; charset=utf-8",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// Build the `window.FX` shape the dashboard expects, from the live usage + route log.
/// One model/mode summed across a provider's accounts for the Overview tile.
struct AggModel {
    id: String,
    name: String,
    levels: Vec<String>,
    remaining: Option<i64>,
    total: Option<i64>,
    window_secs: Option<i64>,
    reset_after: Option<String>,
    // Locked only if EVERY account has it locked; one unlocked account makes it usable.
    all_locked: bool,
}

impl AggModel {
    fn seed(m: &crate::providers::ModelInfo) -> Self {
        Self {
            id: m.id.clone(),
            name: m.name.clone(),
            levels: m.levels.clone(),
            remaining: None,
            total: None,
            window_secs: m.window_secs,
            reset_after: m.reset_after.clone(),
            all_locked: true,
        }
    }

    fn merge(&mut self, m: &crate::providers::ModelInfo) {
        self.remaining = add_opt(self.remaining, m.remaining);
        self.total = add_opt(self.total, m.total);
        self.window_secs = self.window_secs.or(m.window_secs);
        if self.reset_after.is_none() {
            self.reset_after = m.reset_after.clone();
        }
        if !m.locked {
            self.all_locked = false;
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "levels": self.levels,
            "remaining": self.remaining,
            "total": self.total,
            "windowSecs": self.window_secs,
            "resetAfter": self.reset_after,
            "locked": self.all_locked,
        })
    }
}

/// Sum two optional counts, treating `None` as "no number" (not zero).
fn add_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + y),
        (Some(x), None) => Some(x),
        (None, y) => y,
    }
}

/// Serialize a provider's live model catalog for the dashboard (camelCase keys, matching the
/// hand-built limits JSON). A locked entry (`total: 0`) renders as 0/0.
fn models_json(models: &[crate::providers::ModelInfo]) -> Vec<Value> {
    models
        .iter()
        .map(|m| {
            json!({
                "id": m.id,
                "name": m.name,
                "levels": m.levels,
                "remaining": m.remaining,
                "total": m.total,
                "windowSecs": m.window_secs,
                "resetAfter": m.reset_after,
                "locked": m.locked,
            })
        })
        .collect()
}

async fn build_state(inner: &Inner, store: &Store) -> crate::Result<Value> {
    let views = inner.router.usage_snapshot_cached().await?;

    let mut dr: HashMap<&str, &crate::router::UsageView> = HashMap::new();
    for v in &views {
        if let Some(base) = v.label.strip_suffix("#dr") {
            dr.insert(base, v);
        }
    }
    let mains: Vec<&crate::router::UsageView> =
        views.iter().filter(|v| !v.label.ends_with("#dr")).collect();

    let total_remaining: i64 = mains.iter().map(|v| v.remaining).sum();

    // Recent route history (live log + per-account "last seen" + sparklines). Empty until an
    // MCP server built from this code records calls into the shared usage.db.
    let routes = store.recent_routes(1000).await.unwrap_or_default();
    let mut last_seen: HashMap<&str, &RouteRow> = HashMap::new();
    for r in &routes {
        last_seen.insert(r.label.as_str(), r);
        if let Some(f) = &r.fail_from {
            last_seen.insert(f.as_str(), r);
        }
    }
    // Newest first: the freshest route sits at the top so new activity shows without scrolling
    // (`recent_routes` returns oldest-first, so take the last 50 and reverse).
    let start = routes.len().saturating_sub(50);
    let log: Vec<Value> = routes[start..].iter().rev().map(route_to_entry).collect();

    // Accounts table + status counts for the summary pills.
    let idents = store.all_identities().await.unwrap_or_default();
    let (mut healthy, mut needs_login, mut exhausted) = (0i64, 0i64, 0i64);
    let accounts: Vec<Value> = mains
        .iter()
        .map(|v| {
            let m = inner.meta.get(&v.label);
            let web = m.map(|x| x.is_web).unwrap_or(false);
            let key = m.map(|x| x.has_key).unwrap_or(false);
            let logged = m.map(|x| x.logged_in).unwrap_or(false);
            let status = status_of(v.exhausted, web, logged);
            match status {
                "exhausted" => exhausted += 1,
                "needs-login" => needs_login += 1,
                _ => healthy += 1,
            }
            json!({
                "provider": v.provider,
                "label": v.label,
                "used": v.used,
                "quota": v.quota,
                "resetWindow": window_or_period(v.window_secs, &v.period),
                "proxy": mask_proxy(m.and_then(|x| x.proxy.as_deref()).unwrap_or("direct")),
                "status": status,
                "key": key,
                "web": web,
                "loggedIn": logged,
                "email": idents.get(v.label.as_str()),
                "pending": v.pending,
                "limits": v.limits.as_ref().map(|ll| json!({
                    "tier": ll.tier,
                    "features": ll.features.iter().map(|f| json!({
                        "feature": f.feature,
                        "remaining": f.remaining,
                        "total": f.total,
                        "windowSecs": f.window_secs,
                        "resetAfter": f.reset_after,
                    })).collect::<Vec<_>>(),
                    "models": models_json(&ll.models),
                })),
            })
        })
        .collect();

    // Provider tiles, aggregated across each provider's accounts, grouped by capability.
    let mut order: Vec<&str> = Vec::new();
    let mut aggs: HashMap<&str, Agg> = HashMap::new();
    for v in &mains {
        let e = aggs.entry(v.provider).or_insert_with(|| {
            order.push(v.provider);
            Agg::new(v.period.clone())
        });
        e.used += v.used;
        e.quota += v.quota;
        e.accounts += 1;
        e.pending |= v.pending;
        e.window_secs = e.window_secs.or(v.window_secs);
        if let Some(u) = v.usd {
            e.usd = Some(e.usd.unwrap_or(0.0) + u);
        }
        if let Some(m) = inner.meta.get(&v.label) {
            if m.is_web {
                e.web = true;
                e.logged |= m.logged_in;
            }
        }
        if let Some(d) = dr.get(v.label.as_str()) {
            e.has_dr = true;
            e.dr_used += d.used;
            e.dr_quota += d.quota;
            e.dr_window_secs = e.dr_window_secs.or(d.window_secs);
            if e.dr_period.is_empty() {
                e.dr_period = d.period.clone();
            }
        }
        // The absolute deep-research reset (chatgpt reports one); grok is a rolling window instead.
        if e.dr_reset_after.is_none() {
            e.dr_reset_after = v
                .limits
                .as_ref()
                .and_then(|l| l.feature("deep_research"))
                .and_then(|f| f.reset_after.clone());
        }
    }

    // Per-provider model catalog, SUMMED across the provider's accounts (like the quota tiles):
    // per model id remaining/total add up, and it stays locked only if every account has it locked.
    let mut cat_by_provider: HashMap<&str, (Vec<String>, HashMap<String, AggModel>)> =
        HashMap::new();
    for v in &mains {
        let Some(ll) = &v.limits else { continue };
        let (order, by_id) = cat_by_provider.entry(v.provider).or_default();
        for m in &ll.models {
            by_id
                .entry(m.id.clone())
                .or_insert_with(|| {
                    order.push(m.id.clone());
                    AggModel::seed(m)
                })
                .merge(m);
        }
    }
    let catalogs: HashMap<&str, Vec<Value>> = cat_by_provider
        .iter()
        .map(|(prov, (order, by_id))| {
            let models = order
                .iter()
                .filter_map(|id| by_id.get(id))
                .map(AggModel::to_json)
                .collect();
            (*prov, models)
        })
        .collect();

    // Other capability limits worth surfacing (create image, file upload). These report a remaining
    // count + reset but no ceiling, so they render as info rows, not fuel-gauge bars. Summed by name.
    let mut feats_by_provider: HashMap<&str, Vec<Value>> = HashMap::new();
    for v in &mains {
        let Some(ll) = &v.limits else { continue };
        let e = feats_by_provider.entry(v.provider).or_default();
        for (name, label) in [
            ("image_gen", "create image"),
            ("file_upload", "file upload"),
        ] {
            let Some(f) = ll.feature(name) else { continue };
            match e.iter_mut().find(|r| r["label"] == label) {
                Some(row) => {
                    row["remaining"] =
                        json!(row["remaining"].as_i64().unwrap_or(0) + f.remaining.max(0));
                }
                None => e.push(json!({
                    "label": label,
                    "remaining": f.remaining,
                    "resetAt": f.reset_after,
                })),
            }
        }
    }

    let groups: Vec<Value> = [
        ("search", "Search"),
        ("read", "Read / scrape"),
        ("browser", "Browser"),
        ("web", "Web sessions"),
    ]
    .iter()
    .map(|(gid, glabel)| {
        let providers: Vec<Value> = order
            .iter()
            .filter(|name| group_of(name).0 == *gid)
            .map(|&name| {
                let a = &aggs[name];
                let resets_in = if a.window_secs.is_some() {
                    Value::Null
                } else {
                    json!(resets_in(&a.period))
                };
                let mut tile = json!({
                    "name": name,
                    "desc": desc_of(name),
                    "used": a.used,
                    "quota": a.quota,
                    "resetWindow": window_or_period(a.window_secs, &a.period),
                    "resetsIn": resets_in,
                    "accounts": a.accounts,
                    "key": !a.web,
                    "pending": a.pending,
                });
                if a.web {
                    tile["webSession"] = json!(true);
                    tile["loggedIn"] = json!(a.logged);
                }
                // Each limit becomes its own cube bar (with its real window + reset); count-less
                // models (chatgpt/gemini) fall to a text catalog line.
                let models = catalogs.get(name).cloned().unwrap_or_default();
                let mut bars: Vec<Value> = Vec::new();
                let mut catalog: Vec<Value> = Vec::new();
                let mut has_model_bar = false;
                for m in &models {
                    if m["total"].is_i64() && m["remaining"].is_i64() {
                        has_model_bar = true;
                        let total = m["total"].as_i64().unwrap_or(0);
                        let rem = m["remaining"].as_i64().unwrap_or(0);
                        bars.push(limit_bar(
                            m["name"].as_str().unwrap_or(""),
                            (total - rem).max(0),
                            total,
                            m["windowSecs"].as_i64(),
                            None,
                            None,
                            m["locked"].as_bool().unwrap_or(false),
                        ));
                    } else {
                        catalog.push(json!({ "name": m["name"], "levels": m["levels"] }));
                    }
                }
                // Account-level quota bar for API providers only — that's their real key quota. A web
                // provider's account counter is just a soft failover placeholder; showing it as a
                // "messages/search" limit misleads (chatgpt caps are per-model, gemini has none), so
                // web cards show only real live limits (grok modes, deep research) + the model catalog.
                if !has_model_bar && !a.web {
                    let mut q = limit_bar(
                        "quota",
                        a.used,
                        a.quota,
                        a.window_secs,
                        Some(&a.period),
                        None,
                        false,
                    );
                    // Estimate providers (a $/token→ops conversion) show "≈" — the count isn't exact.
                    if approx_quota(name) {
                        q["approx"] = json!(true);
                        if let Some(usd) = a.usd {
                            q["usd"] = json!(usd);
                        }
                    }
                    bars.push(q);
                }
                if a.has_dr {
                    bars.push(limit_bar(
                        "deep research",
                        a.dr_used,
                        a.dr_quota,
                        a.dr_window_secs,
                        Some(&a.dr_period),
                        a.dr_reset_after.as_deref(),
                        a.dr_quota == 0,
                    ));
                }
                tile["limits"] = json!(bars);
                tile["catalog"] = json!(catalog);
                if let Some(fs) = feats_by_provider.get(name) {
                    tile["features"] = json!(fs);
                }
                tile
            })
            .collect();
        json!({ "id": gid, "label": glabel, "providers": providers })
    })
    .filter(|g| {
        !g["providers"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true)
    })
    .collect();

    // Provider health: quota state + last-seen time from the route log.
    let health: Vec<Value> = mains
        .iter()
        .map(|v| {
            let m = inner.meta.get(&v.label);
            let web = m.map(|x| x.is_web).unwrap_or(false);
            let logged = m.map(|x| x.logged_in).unwrap_or(false);
            let state = status_of(v.exhausted, web, logged);
            let last_success = last_seen
                .get(v.label.as_str())
                .map(|r| ago(&r.ts))
                .unwrap_or_else(|| "—".to_string());
            let last_error = if v.exhausted {
                Some(format!(
                    "quota exhausted — {}/{} {}",
                    v.used,
                    v.quota,
                    window_or_period(v.window_secs, &v.period)
                ))
            } else if state == "needs-login" {
                Some("session expired — browser login required".to_string())
            } else {
                None
            };
            json!({ "provider": v.label, "state": state, "lastSuccess": last_success, "lastError": last_error })
        })
        .collect();

    // Per-account calls-per-day from the recent route log (Activity sparklines).
    let today = Utc::now().date_naive();
    let mut by_label: HashMap<&str, BTreeMap<i64, i64>> = HashMap::new();
    for r in &routes {
        if let Ok(d) = DateTime::parse_from_rfc3339(&r.ts) {
            let off = (d.with_timezone(&Utc).date_naive() - today).num_days();
            if off > -14 {
                *by_label
                    .entry(r.label.as_str())
                    .or_default()
                    .entry(off)
                    .or_insert(0) += 1;
            }
        }
    }
    let mut usage_rows: Vec<(&str, i64, Vec<i64>)> = by_label
        .iter()
        .map(|(label, days)| {
            let series: Vec<i64> = (-13..=0).map(|off| *days.get(&off).unwrap_or(&0)).collect();
            let total: i64 = series.iter().sum();
            (*label, total, series)
        })
        .collect();
    usage_rows.sort_by(|a, b| b.1.cmp(&a.1));
    usage_rows.truncate(6);
    let usage: Vec<Value> = usage_rows
        .iter()
        .map(|(label, _t, series)| {
            let color = inner
                .meta
                .get(*label)
                .map(|m| group_color(m.kind.as_str()))
                .unwrap_or("var(--lime-500)");
            json!({ "provider": label, "color": color, "series": series })
        })
        .collect();

    // One pass over the route log: dead-end tally (ops that ran out with no success — ~0 by design)
    // and per-account op rate for the burn radar.
    let mut ran_out = 0i64;
    let mut op_rate: HashMap<&str, (i64, i64, i64)> = HashMap::new(); // count, first_epoch, last_epoch
    for r in &routes {
        if r.status == 429 || r.status == 402 {
            ran_out += 1;
        }
        if let Ok(d) = DateTime::parse_from_rfc3339(&r.ts) {
            let secs = d.timestamp();
            let e = op_rate.entry(r.label.as_str()).or_insert((0, secs, secs));
            e.0 += 1;
            e.1 = e.1.min(secs);
            e.2 = e.2.max(secs);
        }
    }

    // Burn radar: the accounts closest to empty, with their recent op/hr slope. Web-session
    // placeholders (no real ceiling) count as full so they don't crowd out real low balances.
    let empty_frac = |v: &&crate::router::UsageView| -> f64 {
        if v.quota > 0 {
            v.remaining as f64 / v.quota as f64
        } else {
            1.0
        }
    };
    let mut burn_ord = mains.clone();
    burn_ord.sort_by(|a, b| {
        empty_frac(a)
            .partial_cmp(&empty_frac(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let burn: Vec<Value> = burn_ord
        .iter()
        .take(5)
        .map(|v| {
            let rate = op_rate
                .get(v.label.as_str())
                .map(|(c, first, last)| {
                    let span_h = (last - first) as f64 / 3600.0;
                    if span_h >= 0.1 {
                        (*c as f64 / span_h * 10.0).round() / 10.0
                    } else {
                        0.0
                    }
                })
                .unwrap_or(0.0);
            json!({
                "provider": v.provider,
                "label": v.label,
                "remaining": v.remaining,
                "resetWindow": window_or_period(v.window_secs, &v.period),
                "ratePerHour": rate,
            })
        })
        .collect();

    // Capability matrix: each configured provider's native niches + escape-hatch modes.
    let capabilities: Vec<Value> = order
        .iter()
        .map(|&name| match parse_kind(name) {
            Some(kind) => {
                let ex = crate::providers::extras(kind);
                json!({
                    "provider": name,
                    "niches": ex.niches,
                    "modes": ex.modes.iter().map(|(m, d)| json!([m, d])).collect::<Vec<_>>(),
                })
            }
            None => json!({ "provider": name, "niches": [], "modes": [] }),
        })
        .collect();

    // Routing priority per capability: the effective order (user custom applied) plus the
    // supported-but-unrouted providers the user could float in.
    let priority: Vec<Value> = cli::PRIORITY_CAPS
        .iter()
        .map(|&cap| {
            let custom = inner.priority.for_cap(cap);
            let eff = crate::providers::order_for(cap, None, custom);
            let avail: Vec<&str> = ProviderKind::all()
                .iter()
                .filter(|p| p.supports(cap) && !eff.contains(p))
                .map(|p| p.as_str())
                .collect();
            json!({
                "capability": cap.as_str(),
                "order": eff.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
                "custom": !custom.is_empty(),
                "available": avail,
            })
        })
        .collect();

    // Static per-provider metadata (auth type, blurb, key signup URL, capabilities) so the
    // add-account and onboarding screens don't hardcode the provider list.
    let catalog: Vec<Value> = ProviderKind::all()
        .iter()
        .map(|&k| {
            use Capability::*;
            let caps: Vec<&str> = [Search, Read, DeepResearch, Browser, Image]
                .iter()
                .filter(|&&c| k.supports(c))
                .map(|c| c.as_str())
                .collect();
            json!({
                "id": k.as_str(),
                "web": k.is_web(),
                "blurb": k.blurb(),
                "signup": k.signup(),
                "caps": caps,
                "free": k.free_tier(),
                "group": group_of(k.as_str()).0,
            })
        })
        .collect();

    Ok(json!({
        "groups": groups,
        "accounts": accounts,
        "catalog": catalog,
        "health": health,
        "log": log,
        "stream": [],
        "usage": usage,
        "summary": { "accounts": mains.len(), "healthy": healthy, "needsLogin": needs_login, "exhausted": exhausted },
        "totalRemaining": total_remaining,
        "deadEnds": { "routed": routes.len(), "ranOut": ran_out },
        "burn": burn,
        "capabilities": capabilities,
        "priority": priority,
    }))
}

struct Agg {
    used: i64,
    quota: i64,
    accounts: i64,
    period: String,
    window_secs: Option<i64>,
    web: bool,
    logged: bool,
    has_dr: bool,
    dr_used: i64,
    dr_quota: i64,
    dr_window_secs: Option<i64>,
    dr_period: String,
    dr_reset_after: Option<String>,
    /// Summed real $ balance for top-up providers (exa/parallel/steel); None for credit providers.
    usd: Option<f64>,
    /// Any account still awaiting its first live figure (cached snapshot) → the card shows a loader.
    pending: bool,
}

impl Agg {
    fn new(period: String) -> Self {
        Self {
            used: 0,
            quota: 0,
            accounts: 0,
            period,
            window_secs: None,
            web: false,
            logged: false,
            has_dr: false,
            dr_used: 0,
            dr_quota: 0,
            dr_window_secs: None,
            dr_period: String::new(),
            dr_reset_after: None,
            usd: None,
            pending: false,
        }
    }
}

fn status_of(exhausted: bool, web: bool, logged: bool) -> &'static str {
    if exhausted {
        "exhausted"
    } else if web && !logged {
        "needs-login"
    } else {
        "healthy"
    }
}

/// A live rolling window (grok's `windowSizeSeconds`) wins over the calendar period label.
fn window_or_period(window_secs: Option<i64>, period: &str) -> String {
    match window_secs {
        Some(s) => window_label(s),
        None => reset_window(period).to_string(),
    }
}

fn window_label(secs: i64) -> String {
    if secs % 3600 == 0 {
        format!("{}h", secs / 3600)
    } else if secs % 60 == 0 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

fn reset_window(period: &str) -> &'static str {
    if period == "lifetime" {
        "lifetime"
    } else if period.len() == 10 {
        "daily"
    } else {
        "monthly"
    }
}

/// One limit as a cube-bar descriptor for the dashboard: value + its own window + reset date.
fn limit_bar(
    label: &str,
    used: i64,
    quota: i64,
    window_secs: Option<i64>,
    period: Option<&str>,
    reset_after: Option<&str>,
    locked: bool,
) -> Value {
    let window = match window_secs {
        Some(s) => window_label(s),
        None => period.map(reset_window).unwrap_or("").to_string(),
    };
    json!({
        "label": label,
        "used": used,
        "quota": quota,
        "window": window,
        "resetAt": reset_at(window_secs, period, reset_after),
        "locked": locked,
    })
}

/// The absolute reset *instant* as an RFC3339 timestamp (UTC) — the browser renders it in the
/// viewer's own timezone. From an ISO `reset_after` (chatgpt, exact time), else the next period
/// boundary (midnight UTC). A rolling window (grok) has no fixed instant — the window label carries it.
fn reset_at(
    window_secs: Option<i64>,
    period: Option<&str>,
    reset_after: Option<&str>,
) -> Option<String> {
    if window_secs.is_some() {
        return None;
    }
    if let Some(iso) = reset_after {
        return Some(iso.to_string());
    }
    let now = chrono::Utc::now().date_naive();
    let boundary = match period.map(reset_window) {
        Some("daily") => now.succ_opt(),
        Some("monthly") => {
            let (y, m) = if now.month() == 12 {
                (now.year() + 1, 1)
            } else {
                (now.year(), now.month() + 1)
            };
            chrono::NaiveDate::from_ymd_opt(y, m, 1)
        }
        _ => None,
    };
    boundary
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|dt| dt.and_utc().to_rfc3339())
}

fn resets_in(period: &str) -> Option<String> {
    match reset_window(period) {
        "lifetime" => None,
        "daily" => Some("1d".to_string()),
        _ => {
            let now = chrono::Utc::now().date_naive();
            let (ny, nm) = if now.month() == 12 {
                (now.year() + 1, 1)
            } else {
                (now.year(), now.month() + 1)
            };
            chrono::NaiveDate::from_ymd_opt(ny, nm, 1)
                .map(|first_next| format!("{}d", (first_next - now).num_days().max(0)))
        }
    }
}

/// Hide proxy credentials and the last IP octet before sending to the browser
/// (`http://user:pass@45.38.78.247:6184` -> `45.38.78.x:6184`).
fn mask_proxy(proxy: &str) -> String {
    if proxy == "direct" || proxy == "pool" {
        return proxy.to_string();
    }
    let after_scheme = proxy.split_once("://").map(|(_, r)| r).unwrap_or(proxy);
    let host_port = after_scheme.rsplit('@').next().unwrap_or(after_scheme);
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (host_port, None),
    };
    let octets: Vec<&str> = host.split('.').collect();
    let host = if octets.len() == 4 {
        format!("{}.{}.{}.x", octets[0], octets[1], octets[2])
    } else {
        host.to_string()
    };
    match port {
        Some(p) => format!("{host}:{p}"),
        None => host,
    }
}

fn group_of(provider: &str) -> (&'static str, &'static str) {
    match provider {
        "serper" | "tavily" | "exa" | "parallel" => ("search", "Search"),
        "firecrawl" => ("read", "Read / scrape"),
        "steel" => ("browser", "Browser"),
        _ => ("web", "Web sessions"),
    }
}

fn group_color(provider: &str) -> &'static str {
    match group_of(provider).0 {
        "search" => "var(--lime-500)",
        "read" => "var(--cyan-500)",
        "browser" => "var(--green-500)",
        _ => "var(--amber-500)",
    }
}

/// Providers whose op-count is a $/token→ops conversion (exa/parallel/steel = $ balance) rather
/// than an exact request count — shown with a leading "≈".
fn approx_quota(provider: &str) -> bool {
    matches!(provider, "parallel" | "exa" | "steel")
}

fn desc_of(provider: &str) -> &'static str {
    match provider {
        "serper" => "Web search API",
        "tavily" => "Search + extract API",
        "exa" => "Neural search API",
        "parallel" => "Search API",
        "firecrawl" => "Crawl + scrape API",
        "steel" => "Headless browser sessions",
        "gemini_web" => "Browser session · search + deep research",
        "grok_web" => "Browser session · search + deep research",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ModelInfo;

    fn mi(id: &str, remaining: Option<i64>, total: Option<i64>, locked: bool) -> ModelInfo {
        ModelInfo {
            id: id.into(),
            name: id.into(),
            levels: Vec::new(),
            remaining,
            total,
            window_secs: None,
            reset_after: None,
            locked,
        }
    }

    #[test]
    fn agg_sums_and_unlocks_if_any_account_can() {
        // Same mode across two accounts: one paid (5/20), one free+locked (0/0).
        let paid = mi("expert", Some(5), Some(20), false);
        let free = mi("expert", Some(0), Some(0), true);
        let mut agg = AggModel::seed(&paid);
        agg.merge(&paid);
        agg.merge(&free);
        assert_eq!(agg.remaining, Some(25 - 20)); // 5 + 0
        assert_eq!(agg.total, Some(20)); // 20 + 0
        assert!(!agg.all_locked); // one account can use it
    }

    #[test]
    fn agg_locked_when_all_locked_and_none_stays_none() {
        let locked = mi("heavy", Some(0), Some(0), true);
        let mut h = AggModel::seed(&locked);
        h.merge(&locked);
        h.merge(&locked);
        assert!(h.all_locked);
        assert_eq!(h.remaining, Some(0));

        // gemini reports no count on any account -> stays None (not summed to 0).
        let no_count = mi("pro", None, None, false);
        let mut p = AggModel::seed(&no_count);
        p.merge(&no_count);
        p.merge(&no_count);
        assert_eq!(p.remaining, None);
        assert!(!p.all_locked);
    }
}
