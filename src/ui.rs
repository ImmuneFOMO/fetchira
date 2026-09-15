//! Local web dashboard (`fetchira ui`). Serves the embedded `webui/` design and a small
//! JSON API built from the live router, so a human can watch quota and manage accounts in a
//! browser. Loopback-only, token + Host (+ Origin on writes) guarded. Never runs unless asked
//! (explicit `ui`, or bare `fetchira` from a TTY) — the MCP stdio server is untouched.

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use axum::extract::{Path as AxumPath, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router as AxumRouter};
use chrono::{DateTime, Datelike, Utc};
use rand::{rngs::OsRng, RngCore};
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::cli;
use crate::config::{self, Config};
use crate::providers::{Capability, Input, ProviderKind};
use crate::router::Router;
use crate::usage::{DebugRow, RouteRow, Store};

#[derive(RustEmbed)]
#[folder = "webui/"]
#[exclude = "*.jsx"]
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
    update_result: RwLock<Value>,
}

pub async fn run(home: &Path) -> anyhow::Result<()> {
    let startup = crate::instances::admit_request(home)?;
    // A missing/empty config is fine: the dashboard opens in its onboarding state and the
    // first `POST /api/account/add` writes fetchira.toml.
    let cfg = cli::load_or_empty(home)?;
    let store = Store::open(&config::resolve_db(home, &cfg.db_path)).await?;
    let inner = build_inner(home, &store).await?;
    // A post-update re-exec passes the old token and port through so the open tab keeps working.
    let token = match std::env::var("FETCHIRA_UI_TOKEN") {
        Ok(token) if !token.is_empty() => token,
        _ => gen_token()?,
    };
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
        update_result: RwLock::new(json!({"status": "idle"})),
    });

    // Registry entry for the schema-aware updater (the dashboard holds the DB open too).
    let _run = crate::instances::register(home, "ui");
    drop(startup);

    // Best-effort: fill in account emails for already-logged-in accounts (otherwise missing until
    // their next login) so the dashboard shows them. Background — doesn't delay the first paint.
    {
        let home = home.to_path_buf();
        let store = state.store.clone();
        tokio::spawn(async move {
            if let Ok(_permit) = crate::instances::admit_request(&home) {
                tokio::select! {
                    _ = crate::instances::forced(&home) => {},
                    _ = cli::backfill_identities(&home, &store) => {},
                }
            }
        });
    }

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
                if let Ok(_permit) = crate::instances::admit_request(&st.home) {
                    let router = st.inner.read().await.router.clone();
                    tokio::select! {
                        _ = crate::instances::forced(&st.home) => {},
                        _ = router.warm() => {},
                    }
                }
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
        .route("/api/setup", get(api_setup).post(api_setup_save))
        .route("/api/install/targets", get(api_install_targets))
        .route("/api/install/refresh", post(api_refresh_skills))
        .route("/api/install", post(api_install))
        .route("/api/update", post(api_update))
        .route("/api/update/force", post(api_update_force))
        .route("/api/update/idle", post(api_update_idle))
        .route("/api/update/status", get(api_update_status))
        .fallback(static_handler)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            update_admission,
        ))
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

fn updating_response() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::RETRY_AFTER, "1")],
        Json(json!({"error": "Fetchira is updating; reconnect MCP and retry"})),
    )
        .into_response()
}

async fn update_admission(State(st): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if !path.starts_with("/api/") || path.starts_with("/api/update") || path == "/api/events" {
        return next.run(req).await;
    }
    let rejected = if req.method() == axum::http::Method::GET {
        guard(&st, req.headers())
    } else {
        guard_mut(&st, req.headers())
    };
    if let Some(response) = rejected {
        return response;
    }
    let Ok(_permit) = crate::instances::admit_request(&st.home) else {
        return updating_response();
    };
    tokio::select! {
        _ = crate::instances::forced(&st.home) => updating_response(),
        response = next.run(req) => response,
    }
}

/// (Re)build the router + per-account metadata from the on-disk config. Called at startup
/// and after every mutation so the dashboard reflects the new state immediately.
async fn build_inner(home: &Path, store: &Store) -> anyhow::Result<Inner> {
    let cfg = cli::load_or_empty(home)?;
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

async fn bind_local(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    // Bookmarkable default port, but never collide: fall back to an ephemeral one.
    match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => Ok(l),
        Err(_) => tokio::net::TcpListener::bind(("127.0.0.1", 0)).await,
    }
}

fn gen_token() -> anyhow::Result<String> {
    let mut buf = [0u8; 16];
    OsRng
        .try_fill_bytes(&mut buf)
        .context("generate dashboard token")?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

fn is_loopback_authority(raw: &str) -> bool {
    if raw.is_empty() || raw.chars().any(char::is_whitespace) {
        return false;
    }
    let (host, port) = if let Some(rest) = raw.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return false;
        };
        let suffix = &rest[end + 1..];
        let port = match suffix {
            "" => None,
            suffix => suffix.strip_prefix(':').filter(|port| !port.is_empty()),
        };
        if !suffix.is_empty() && port.is_none() {
            return false;
        }
        (Some(&rest[..end]), port)
    } else if raw.matches(':').count() == 1 {
        let (host, port) = raw.rsplit_once(':').unwrap();
        (Some(host), Some(port))
    } else {
        (Some(raw), None)
    };
    if let Some(port) = port {
        if port.is_empty() || port.parse::<u16>().is_err() {
            return false;
        }
    }
    host.is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    })
}

fn is_loopback_origin(raw: &str) -> bool {
    let Ok(origin) = reqwest::Url::parse(raw) else {
        return false;
    };
    origin.scheme() == "http"
        && origin.username().is_empty()
        && origin.password().is_none()
        && origin.query().is_none()
        && origin.fragment().is_none()
        && (origin.path().is_empty() || origin.path() == "/")
        && origin
            .host_str()
            .is_some_and(|host| is_loopback_authority(&format_host(host, origin.port())))
}

fn format_host(host: &str, port: Option<u16>) -> String {
    let host = host.trim_matches(['[', ']']);
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    port.map_or(host.clone(), |port| format!("{host}:{port}"))
}

/// Loopback dashboard holds quota data → require a per-session token and a loopback Host
/// (defeats DNS-rebinding). Applies to the API; static assets carry no secrets.
fn guard(st: &AppState, headers: &HeaderMap) -> Option<Response> {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !is_loopback_authority(host) {
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
        if !is_loopback_origin(origin) {
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
            if let Ok(cfg) = cli::load_or_empty(&st.home) {
                v["setup"] = setup_snapshot(&cfg);
            }
            if let Some(u) = crate::update::ui_banner(&st.home).await {
                v["update"] = u;
            }
            if let Some(p) = crate::update::pending(&st.home) {
                v["update_pending"] = p;
            }
            v["instances"] = json!(crate::instances::running(&st.home, &[std::process::id()]));
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
    if !is_loopback_authority(host) {
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

#[derive(Deserialize)]
struct SetupReq {
    mode: String,
    endpoint: Option<String>,
    #[serde(alias = "apiKey")]
    api_key: Option<String>,
    #[serde(default)]
    clear_remote: bool,
}

fn setup_snapshot(cfg: &Config) -> Value {
    let endpoint = cfg.remote.endpoint.clone();
    json!({
        "mode": if endpoint.is_some() { "hosted" } else { "local" },
        "configured": endpoint.is_some() && cfg.remote.api_key.is_some(),
        "endpoint": endpoint,
    })
}

async fn api_setup(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(resp) = guard(&st, &headers) {
        return resp;
    }
    match cli::load_or_empty(&st.home) {
        Ok(cfg) => Json(setup_snapshot(&cfg)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn api_setup_save(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<SetupReq>,
) -> Response {
    if let Some(resp) = guard_mut(&st, &headers) {
        return resp;
    }
    match req.mode.as_str() {
        "local" => {
            let cfg = match cli::load_or_empty(&st.home) {
                Ok(cfg) => cfg,
                Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
            };
            if (cfg.remote.endpoint.is_some() || cfg.remote.api_key.is_some()) && !req.clear_remote
            {
                return (
                    StatusCode::BAD_REQUEST,
                    "confirm clearing the saved hosted server URL and API key",
                )
                    .into_response();
            }
            if let Err(e) = cli::use_local(&st.home) {
                return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
            }
            rebuild(&st).await;
            match cli::load_or_empty(&st.home) {
                Ok(cfg) => Json(json!({
                    "ok": true,
                    "message": "using local provider accounts",
                    "setup": setup_snapshot(&cfg),
                }))
                .into_response(),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            }
        }
        "hosted" => {
            let Some(endpoint) = req
                .endpoint
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                return (StatusCode::BAD_REQUEST, "hosted endpoint is required").into_response();
            };
            let current = match cli::load_or_empty(&st.home) {
                Ok(cfg) => cfg,
                Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
            };
            let reuse = req
                .api_key
                .as_deref()
                .map(str::trim)
                .is_none_or(str::is_empty)
                && current.remote.endpoint.as_deref().map(str::trim) == Some(endpoint);
            let message = if reuse {
                if current.remote.api_key.is_none() {
                    return (StatusCode::BAD_REQUEST, "hosted API key is required").into_response();
                }
                // Verify the stored reference in place. Passing env:SECRET through the literal
                // API-key path would reject a valid reference before it can be resolved.
                match crate::remote::verify(&current).await {
                    Ok(message) => message,
                    Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
                }
            } else {
                let Some(api_key) = req
                    .api_key
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    return (
                        StatusCode::BAD_REQUEST,
                        "hosted API key is required when changing the endpoint",
                    )
                        .into_response();
                };
                match cli::configure_remote(&st.home, endpoint, api_key).await {
                    Ok(message) => message,
                    Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
                }
            };
            rebuild(&st).await;
            match cli::load_or_empty(&st.home) {
                Ok(cfg) => Json(json!({
                    "ok": true,
                    "message": message,
                    "setup": setup_snapshot(&cfg),
                }))
                .into_response(),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            }
        }
        _ => (StatusCode::BAD_REQUEST, "mode must be local or hosted").into_response(),
    }
}

async fn api_install_targets(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(r) = guard(&st, &headers) {
        return r;
    }
    let targets = cli::mcp_target_list();
    let skill_destinations = crate::skills::skill_destinations(&user_home());
    let agents: Vec<Value> = targets
        .iter()
        .map(|target| {
            let skill_supported = agent_skill_supported(target.name);
            let skill_installed =
                skill_supported && agent_skill_installed(target.name, &skill_destinations);
            json!({
                "name": target.name,
                "present": target.present,
                "mcp": true,
                "skill": skill_supported,
                "skillSupported": skill_supported,
                "skillInstalled": skill_installed,
                "mcpInstalled": target.installed,
                "installed": target.installed || skill_installed,
            })
        })
        .collect();
    let targets: Vec<Value> = targets
        .iter()
        .map(|t| json!({ "name": t.name, "present": t.present, "installed": t.installed }))
        .collect();
    let (skill, installed_skills) = installed_skill_status(&user_home());
    let outdated = cli::installation_binary()
        .map(|bin| crate::skills::refresh_existing(&user_home(), &st.home, Path::new(&bin), false))
        .unwrap_or_default();
    let upgrade_pending = std::fs::read_to_string(st.home.join("agent-upgrade-complete"))
        .ok()
        .as_deref()
        != Some(env!("CARGO_PKG_VERSION"))
        && (!outdated.is_empty() || targets.iter().any(|target| target["installed"] == true));
    Json(json!({
        "targets": targets,
        "agents": agents,
        "skill": skill,
        "skills": installed_skills,
        "upgradePending": upgrade_pending,
        "outdatedSkills": outdated.iter().map(|result| json!({"name": result.name, "ok": result.ok, "msg": result.msg})).collect::<Vec<_>>(),
        "version": env!("CARGO_PKG_VERSION"),
    }))
    .into_response()
}

async fn api_refresh_skills(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(response) = guard_mut(&st, &headers) {
        return response;
    }
    let bin = match cli::installation_binary() {
        Ok(bin) => bin,
        Err(error) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response()
        }
    };
    let results = crate::update::refresh_integrations(&user_home(), &st.home, &bin);
    if results.iter().all(|result| result.ok) {
        let _ = config::write_atomic(
            &st.home.join("agent-upgrade-complete"),
            env!("CARGO_PKG_VERSION"),
            false,
        );
    }
    Json(json!({"results": results.into_iter().map(|result| json!({"name": result.name, "ok": result.ok, "msg": result.msg})).collect::<Vec<_>>()})).into_response()
}

fn agent_skill_installed(name: &str, destinations: &[crate::skills::SkillDestination]) -> bool {
    let home = user_home();
    let shared = home.canonicalize().unwrap_or(home).join(".agents");
    destinations.iter().any(|destination| {
        !destination.variants.is_empty()
            && match name {
                "Claude Code" => destination.name == "Claude",
                "Codex CLI" => destination.name == "Codex",
                "Gemini CLI" => {
                    destination.name == "Gemini"
                        || (destination.name == "Codex" && destination.parent == shared)
                }
                "Cursor" => destination.name == "Cursor",
                _ => false,
            }
    })
}

fn agent_skill_supported(name: &str) -> bool {
    matches!(name, "Claude Code" | "Codex CLI" | "Gemini CLI" | "Cursor")
}

#[derive(Deserialize)]
struct InstallReq {
    targets: Vec<String>,
    /// Omitted keeps the old API behavior: register MCP targets only.
    skill: Option<String>,
    #[serde(default)]
    remove_mcp: bool,
}

async fn api_install(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<InstallReq>,
) -> Response {
    if let Some(r) = guard_mut(&st, &headers) {
        return r;
    }
    let variant = match parse_install_skill(req.skill.as_deref()) {
        Ok(variant) => variant,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    if variant == Some(crate::skills::SkillVariant::Skip) {
        return Json(json!({ "results": [], "skill": req.skill })).into_response();
    }
    let bin = match cli::installation_binary() {
        Ok(path) => path,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let results = match cli::install_integrations_with_options(
        &user_home(),
        &st.home,
        &bin,
        &req.targets,
        variant,
        req.remove_mcp,
    ) {
        Ok(results) => results
            .into_iter()
            .map(|result| json!({ "name": result.name, "ok": result.ok, "msg": result.msg }))
            .collect::<Vec<_>>(),
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    Json(json!({ "results": results, "skill": req.skill })).into_response()
}

fn parse_install_skill(value: Option<&str>) -> Result<Option<crate::skills::SkillVariant>, String> {
    value.map_or(Ok(None), |value| {
        crate::skills::SkillVariant::parse(value)
            .map(Some)
            .ok_or_else(|| format!("unknown skill '{value}' (expected both, mcp, cli, or skip)"))
    })
}

fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn installed_skill_status(home: &Path) -> (Option<&'static str>, Vec<&'static str>) {
    let destinations = crate::skills::skill_destinations(home);
    let mut names = Vec::new();
    for variant in [
        crate::skills::SkillVariant::Both,
        crate::skills::SkillVariant::Mcp,
        crate::skills::SkillVariant::Cli,
    ] {
        if destinations
            .iter()
            .any(|destination| destination.variants.contains(&variant))
        {
            names.push(variant.as_str());
        }
    }
    let selected = (names.len() == 1).then(|| names[0]);
    (selected, names)
}

/// Compatibility route: use the same managed graceful update as the main button.
async fn api_update_idle(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    api_update_impl(st, headers, false).await
}

#[cfg(unix)]
fn restart_self(exe: &str, token: &str, port: u16) {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env("FETCHIRA_UI_TOKEN", token)
        .env("FETCHIRA_UI_PORT", port.to_string())
        .env("FETCHIRA_NO_OPEN", "1")
        .exec();
    eprintln!("restart after update failed: {err}");
}

#[cfg(not(unix))]
fn restart_self(_exe: &str, _token: &str, _port: u16) {}

async fn api_update_force(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    api_update_impl(st, headers, true).await
}

async fn api_update(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    api_update_impl(st, headers, false).await
}

async fn api_update_status(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(r) = guard(&st, &headers) {
        return r;
    }
    Json(st.update_result.read().await.clone()).into_response()
}

async fn api_update_impl(st: Arc<AppState>, headers: HeaderMap, force: bool) -> Response {
    if let Some(r) = guard_mut(&st, &headers) {
        return r;
    }
    let binary = match cli::installation_binary() {
        Ok(binary) => binary,
        Err(error) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };
    let mut status = st.update_result.write().await;
    if matches!(status["status"].as_str(), Some("running" | "restarting")) {
        return (StatusCode::CONFLICT, "An update is already running").into_response();
    }
    *status = json!({"status": "running", "ok": true, "msg": "Preparing update"});
    drop(status);
    // The updater must survive an HTTP disconnect while downloading or draining work.
    tokio::spawn(async move {
        let result = match crate::update::perform_with_store(&st.home, force, Some(&st.store)).await
        {
            Ok(crate::update::Outcome::UpToDate) => {
                json!({"status": "finished", "ok": true, "msg": "Already up to date"})
            }
            Ok(crate::update::Outcome::Blocked { latest, instances }) => json!({
                "status": "finished", "ok": false, "blocked": true,
                "latest": latest, "instances": instances,
            }),
            Ok(crate::update::Outcome::Updated(version)) => {
                *st.update_result.write().await = json!({
                    "status": "restarting", "ok": true, "restarted": true,
                    "msg": format!("Updated to {version}; restarting dashboard"),
                });
                tokio::time::sleep(Duration::from_secs(2)).await;
                restart_self(&binary, &st.token, st.port);
                json!({"status": "finished", "ok": false,
                    "msg": "Update installed, but dashboard restart failed. Restart Fetchira."})
            }
            Err(error) => json!({"status": "finished", "ok": false, "msg": error.to_string()}),
        };
        *st.update_result.write().await = result;
    });
    (
        StatusCode::ACCEPTED,
        Json(json!({"ok": true, "status": "running"})),
    )
        .into_response()
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
    let path = uri.path().trim_start_matches('/');
    if path.is_empty() {
        let mut location = String::from("/ui_kits/dashboard/index.html");
        if let Some(query) = uri.query() {
            location.push('?');
            location.push_str(query);
        }
        return axum::response::Redirect::temporary(&location).into_response();
    }
    match Assets::get(path) {
        // no-cache: a restarted/upgraded server must not run against stale cached JSX.
        Some(c) => (
            [
                (header::CONTENT_TYPE, mime_for(path)),
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
            window_secs: None,
            reset_after: None,
            all_locked: true,
        }
    }

    fn merge(&mut self, m: &crate::providers::ModelInfo) {
        self.remaining = add_opt(self.remaining, m.remaining);
        self.total = add_opt(self.total, m.total);
        self.window_secs = min_opt(self.window_secs, m.window_secs);
        self.reset_after = sooner_reset(self.reset_after.take(), m.reset_after.as_deref());
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

fn min_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (a, b) => a.or(b),
    }
}

/// Earliest ISO-8601 reset among the accounts that reported one.
fn sooner_reset(a: Option<String>, b: Option<&str>) -> Option<String> {
    match (a, b) {
        (None, None) => None,
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y.to_string()),
        (Some(x), Some(y)) => {
            let tx = DateTime::parse_from_rfc3339(&x).ok();
            let ty = DateTime::parse_from_rfc3339(y).ok();
            match (tx, ty) {
                (Some(tx), Some(ty)) if ty < tx => Some(y.to_string()),
                (None, Some(_)) => Some(y.to_string()),
                _ => Some(x),
            }
        }
    }
}

struct FeatBar {
    label: String,
    used: i64,
    quota: i64,
    window_secs: Option<i64>,
    reset_after: Option<String>,
    locked: bool,
}

impl FeatBar {
    fn to_bar(&self) -> Value {
        limit_bar(
            &self.label,
            self.used,
            self.quota,
            self.window_secs,
            None,
            self.reset_after.as_deref(),
            self.locked,
        )
    }
}

fn feat_label(name: &str) -> String {
    match name {
        "image_gen" => "create image".into(),
        "file_upload" => "file upload".into(),
        "deep_research" => "deep research".into(),
        _ => name.replace('_', " "),
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

    let groups = overview_groups(&mains, |label| {
        inner.meta.get(label).is_some_and(|m| m.logged_in)
    });

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
    usage_rows.sort_by_key(|a| std::cmp::Reverse(a.1));
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
    let mut cap_order: Vec<&str> = Vec::new();
    for v in &mains {
        if !cap_order.contains(&v.provider) {
            cap_order.push(v.provider);
        }
    }
    let capabilities: Vec<Value> = cap_order
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

/// Provider tiles, aggregated across each provider's accounts, grouped by capability.
pub(crate) fn overview_groups(
    mains: &[&crate::router::UsageView],
    logged_in: impl Fn(&str) -> bool,
) -> Vec<Value> {
    let mut order: Vec<&str> = Vec::new();
    let mut aggs: HashMap<&str, Agg> = HashMap::new();
    for v in mains {
        let e = aggs.entry(v.provider).or_insert_with(|| {
            order.push(v.provider);
            Agg::new(v.period.clone())
        });
        e.used += v.used;
        e.quota += v.quota;
        e.accounts += 1;
        e.pending |= v.pending;
        e.window_secs = min_opt(e.window_secs, v.window_secs);
        if let Some(u) = v.usd {
            e.usd = Some(e.usd.unwrap_or(0.0) + u);
        }
        if v.provider.ends_with("_web") {
            e.web = true;
            e.logged |= logged_in(&v.label);
        }
    }

    // Per-provider model catalog, SUMMED across the provider's accounts (like the quota tiles):
    // per model id remaining/total add up, and it stays locked only if every account has it locked.
    let mut cat_by_provider: HashMap<&str, (Vec<String>, HashMap<String, AggModel>)> =
        HashMap::new();
    for v in mains {
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

    // Live feature limits (chatgpt remaining+reset, grok deep research remaining+total). Summed
    // by feature id; reset is the soonest ISO among accounts.
    let mut feats_by_provider: HashMap<&str, (Vec<String>, HashMap<String, FeatBar>)> =
        HashMap::new();
    for v in mains {
        let Some(ll) = &v.limits else { continue };
        let (ford, by) = feats_by_provider.entry(v.provider).or_default();
        for f in &ll.features {
            if f.feature.starts_with("model:") {
                continue;
            }
            let e = by.entry(f.feature.clone()).or_insert_with(|| {
                ford.push(f.feature.clone());
                FeatBar {
                    label: feat_label(&f.feature),
                    used: 0,
                    quota: 0,
                    window_secs: None,
                    reset_after: None,
                    locked: true,
                }
            });
            if let Some(total) = f.total {
                e.used += (total - f.remaining).max(0);
                e.quota += total;
                e.locked &= total == 0;
                e.window_secs = min_opt(e.window_secs, f.window_secs);
            } else {
                e.quota += f.remaining.max(0);
                e.locked = false;
            }
            e.reset_after = sooner_reset(e.reset_after.take(), f.reset_after.as_deref());
        }
    }

    [
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
                            m["resetAfter"].as_str(),
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
                if let Some((ford, by)) = feats_by_provider.get(name) {
                    for id in ford {
                        if let Some(f) = by.get(id) {
                            bars.push(f.to_bar());
                        }
                    }
                }
                tile["limits"] = json!(bars);
                tile["catalog"] = json!(catalog);
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
    .collect()
}

struct Agg {
    used: i64,
    quota: i64,
    accounts: i64,
    period: String,
    window_secs: Option<i64>,
    web: bool,
    logged: bool,
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
/// (`http://user:pass@192.0.2.123:6184` -> `192.0.2.x:6184`).
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
        "chatgpt_web" => "Browser session · search + deep research",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ModelInfo;
    use std::fs;

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

    #[tokio::test]
    async fn root_redirect_keeps_token_and_asset_base() {
        let response = static_handler("/?token=example".parse().unwrap()).await;
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            response.headers()[header::LOCATION],
            "/ui_kits/dashboard/index.html?token=example"
        );
    }

    #[test]
    fn install_skill_is_optional_and_validated() {
        let omitted: InstallReq = serde_json::from_str(r#"{"targets":[]}"#).unwrap();
        assert_eq!(parse_install_skill(omitted.skill.as_deref()).unwrap(), None);

        let cli: InstallReq = serde_json::from_str(r#"{"targets":[],"skill":"cli"}"#).unwrap();
        assert_eq!(
            parse_install_skill(cli.skill.as_deref()).unwrap(),
            Some(crate::skills::SkillVariant::Cli)
        );
        assert!(parse_install_skill(Some("unknown")).is_err());
    }

    #[test]
    fn dashboard_accepts_only_exact_loopback_hosts_and_origins() {
        for host in [
            "localhost",
            "localhost:7878",
            "127.0.0.1",
            "127.0.0.1:7878",
            "[::1]:7878",
        ] {
            assert!(is_loopback_authority(host), "{host}");
        }
        for host in [
            "localhost.attacker.test",
            "localhost:bad",
            "127.0.0.1.attacker.test",
            "[::1]attacker.test",
            "[::1]evil",
            "[::1]@attacker.test",
            "[2001:db8::1]:7878",
        ] {
            assert!(!is_loopback_authority(host), "{host}");
        }
        assert!(is_loopback_origin("http://localhost:7878"));
        assert!(is_loopback_origin("http://[::1]:7878/"));
        for origin in [
            "http://localhost.attacker.test:7878",
            "http://127.0.0.1.attacker.test:7878",
            "http://localhost:7878/evil",
            "https://localhost:7878",
        ] {
            assert!(!is_loopback_origin(origin), "{origin}");
        }
    }

    #[test]
    fn dashboard_token_uses_os_randomness() {
        let first = gen_token().unwrap();
        let second = gen_token().unwrap();
        assert_eq!(first.len(), 32);
        assert_ne!(first, second);
    }

    #[test]
    fn install_target_capabilities_distinguish_skills_from_mcp() {
        assert!(agent_skill_supported("Codex CLI"));
        assert!(agent_skill_supported("Gemini CLI"));
        assert!(!agent_skill_supported("Claude Desktop"));
        assert!(!agent_skill_supported("VS Code"));
    }

    #[test]
    fn installed_skill_status_recognizes_cli_only() {
        let home = std::env::temp_dir().join(format!(
            "fetchira-ui-skill-status-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(home.join(".agents/skills/fetchira-cli")).unwrap();
        fs::write(
            home.join(".agents/skills/fetchira-cli/SKILL.md"),
            "name: fetchira-cli",
        )
        .unwrap();
        fs::write(
            home.join(".agents/skills/fetchira-cli/references.md"),
            "reference",
        )
        .unwrap();
        let (selected, installed) = installed_skill_status(&home);
        assert_eq!(selected, Some("cli"));
        assert_eq!(installed, vec!["cli"]);
        fs::remove_dir_all(home).unwrap();
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

    #[test]
    fn overview_omits_soft_dr_bar_without_live_total() {
        let v = view("chatgpt_web", "a", 0, 100, None);
        let groups = overview_groups(&[&v], |_| true);
        let p = tile(&groups, "chatgpt_web");
        let limits = p["limits"].as_array().unwrap();
        assert!(limits.iter().all(|l| l["label"] != "deep research"));
    }

    fn view(
        provider: &'static str,
        label: &str,
        used: i64,
        quota: i64,
        limits: Option<crate::providers::LiveLimits>,
    ) -> crate::router::UsageView {
        crate::router::UsageView {
            provider,
            label: label.into(),
            period: "monthly".into(),
            quota,
            used,
            remaining: (quota - used).max(0),
            exhausted: false,
            proxy: "direct".into(),
            window_secs: None,
            limits,
            usd: None,
            pending: false,
        }
    }

    fn tile<'a>(groups: &'a [Value], name: &str) -> &'a Value {
        groups
            .iter()
            .flat_map(|g| g["providers"].as_array().into_iter().flatten())
            .find(|p| p["name"] == name)
            .expect("tile")
    }

    #[test]
    fn sooner_reset_picks_earliest_iso() {
        assert_eq!(
            sooner_reset(
                Some("2026-10-02T02:00:00Z".into()),
                Some("2026-09-20T02:00:00Z"),
            ),
            Some("2026-09-20T02:00:00Z".into())
        );
        assert_eq!(
            sooner_reset(None, Some("2026-09-20T02:00:00Z")),
            Some("2026-09-20T02:00:00Z".into())
        );
    }

    #[test]
    fn overview_sums_chatgpt_features_and_soonest_reset() {
        let a = view(
            "chatgpt_web",
            "a",
            0,
            100,
            Some(crate::providers::LiveLimits {
                features: vec![
                    crate::providers::FeatureLimit::simple(
                        "deep_research",
                        10,
                        Some("2026-10-02T02:00:00Z".into()),
                    ),
                    crate::providers::FeatureLimit::simple(
                        "image_gen",
                        2,
                        Some("2026-10-05T02:00:00Z".into()),
                    ),
                    crate::providers::FeatureLimit::simple(
                        "paste_text_to_file",
                        40,
                        Some("2026-10-08T02:00:00Z".into()),
                    ),
                ],
                ..Default::default()
            }),
        );
        let b = view(
            "chatgpt_web",
            "b",
            0,
            100,
            Some(crate::providers::LiveLimits {
                features: vec![
                    crate::providers::FeatureLimit::simple(
                        "deep_research",
                        15,
                        Some("2026-09-20T02:00:00Z".into()),
                    ),
                    crate::providers::FeatureLimit::simple("image_gen", 1, None),
                    crate::providers::FeatureLimit::simple(
                        "paste_text_to_file",
                        80,
                        Some("2026-10-01T02:00:00Z".into()),
                    ),
                ],
                ..Default::default()
            }),
        );
        let groups = overview_groups(&[&a, &b], |_| true);
        let p = tile(&groups, "chatgpt_web");
        assert_eq!(p["accounts"], 2);
        let limits = p["limits"].as_array().expect("limits");
        let dr = limits
            .iter()
            .find(|l| l["label"] == "deep research")
            .unwrap();
        assert_eq!(dr["quota"], 25);
        assert_eq!(dr["used"], 0);
        assert_eq!(dr["resetAt"], "2026-09-20T02:00:00Z");
        let img = limits
            .iter()
            .find(|l| l["label"] == "create image")
            .unwrap();
        assert_eq!(img["quota"], 3);
        let paste = limits
            .iter()
            .find(|l| l["label"] == "paste text to file")
            .unwrap();
        assert_eq!(paste["quota"], 120);
        assert_eq!(paste["resetAt"], "2026-10-01T02:00:00Z");
    }

    #[test]
    fn overview_sums_api_quota_and_grok_models() {
        let a = view("exa", "e1", 10, 100, None);
        let b = view("exa", "e2", 5, 50, None);
        let groups = overview_groups(&[&a, &b], |_| false);
        let p = tile(&groups, "exa");
        assert_eq!(p["accounts"], 2);
        assert_eq!(p["used"], 15);
        assert_eq!(p["quota"], 150);
        let q = p["limits"].as_array().unwrap()[0].clone();
        assert_eq!(q["label"], "quota");
        assert_eq!(q["used"], 15);
        assert_eq!(q["quota"], 150);

        let g1 = view(
            "grok_web",
            "g1",
            0,
            100,
            Some(crate::providers::LiveLimits {
                models: vec![mi("expert", Some(5), Some(20), false)],
                features: vec![crate::providers::FeatureLimit {
                    feature: "deep_research".into(),
                    remaining: 5,
                    total: Some(20),
                    window_secs: Some(7200),
                    reset_after: None,
                }],
                ..Default::default()
            }),
        );
        let g2 = view(
            "grok_web",
            "g2",
            0,
            100,
            Some(crate::providers::LiveLimits {
                models: vec![mi("expert", Some(0), Some(0), true)],
                features: vec![crate::providers::FeatureLimit {
                    feature: "deep_research".into(),
                    remaining: 0,
                    total: Some(0),
                    window_secs: None,
                    reset_after: None,
                }],
                ..Default::default()
            }),
        );
        let groups = overview_groups(&[&g1, &g2], |_| true);
        let p = tile(&groups, "grok_web");
        let limits = p["limits"].as_array().unwrap();
        let expert = limits.iter().find(|l| l["label"] == "expert").unwrap();
        assert_eq!(expert["quota"], 20);
        assert_eq!(expert["used"], 15);
        assert!(!expert["locked"].as_bool().unwrap());
        let dr = limits
            .iter()
            .find(|l| l["label"] == "deep research")
            .unwrap();
        assert_eq!(dr["quota"], 20);
        assert_eq!(dr["used"], 15);
        assert_eq!(dr["window"], "2h");
    }
}
