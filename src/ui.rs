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
}

/// The parts that a config mutation rebuilds; behind an RwLock so add/remove/login take effect live.
struct Inner {
    router: Router,
    meta: HashMap<String, AcctMeta>,
}

struct AppState {
    home: PathBuf,
    token: String,
    store: Store,
    inner: RwLock<Inner>,
}

pub async fn run(home: &Path) -> anyhow::Result<()> {
    let cfg_path = home.join("fetchira.toml");
    let cfg = config::load(cfg_path.to_str().unwrap_or("fetchira.toml"))
        .map_err(|e| anyhow::anyhow!("{e}. Run `fetchira setup` first."))?;
    let store = Store::open(&config::resolve_db(home, &cfg.db_path)).await?;
    let inner = build_inner(home, &store).await?;
    let token = gen_token();
    let state = Arc::new(AppState {
        home: home.to_path_buf(),
        token: token.clone(),
        store,
        inner: RwLock::new(inner),
    });

    let app = AxumRouter::new()
        .route("/api/state", get(api_state))
        .route("/api/events", get(api_events))
        .route("/api/debug", get(api_debug))
        .route("/api/debug/{id}", get(api_debug_one))
        .route("/api/account/add", post(api_add))
        .route("/api/account/remove", post(api_remove))
        .route("/api/account/login", post(api_login))
        .route("/api/account/session", post(api_session))
        .route("/api/account/test", post(api_test))
        .fallback(static_handler)
        .with_state(state);

    let listener = bind_local(7878).await?;
    let addr = listener.local_addr()?;
    let url = format!("http://{addr}/ui_kits/dashboard/index.html?token={token}");
    eprintln!("fetchira ui — open {url}");
    if std::env::var("FETCHIRA_NO_OPEN").is_err() {
        let _ = open::that(&url);
    }
    axum::serve(listener, app).await?;
    Ok(())
}

/// (Re)build the router + per-account metadata from the on-disk config. Called at startup
/// and after every mutation so the dashboard reflects the new state immediately.
async fn build_inner(home: &Path, store: &Store) -> anyhow::Result<Inner> {
    let cfg_path = home.join("fetchira.toml");
    let cfg = config::load(cfg_path.to_str().unwrap_or("fetchira.toml"))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
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
            },
        );
    }
    let router = Router::build(cfg, store.clone()).await?;
    Ok(Inner { router, meta })
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
        Ok(Some(r)) => Json(json!({
            "id": r.id, "ts": r.ts, "capability": r.capability, "provider": r.provider,
            "label": r.label, "status": r.status, "latencyMs": r.latency_ms,
            "request": r.request, "response": r.response, "error": r.error,
        }))
        .into_response(),
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
}

#[derive(Deserialize)]
struct LabelReq {
    label: String,
}

#[derive(Deserialize)]
struct SessionReq {
    label: String,
    session: String,
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
            None => cli::capture_login(&st.home, &label).await,
        };
        if let Err(e) = outcome {
            rebuild(&st).await;
            return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
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
    match cli::capture_login(&st.home, &req.label).await {
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

/// A capability-appropriate probe call for a provider (real request → counts against quota,
/// and shows up in the route log).
fn test_call(kind: ProviderKind) -> (Capability, Input) {
    match kind {
        ProviderKind::Jina | ProviderKind::Firecrawl => (
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
    if let Some(from) = &r.fail_from {
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
    }
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
        Some(c) => (
            [(header::CONTENT_TYPE, mime_for(&path))],
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
async fn build_state(inner: &Inner, store: &Store) -> crate::Result<Value> {
    let views = inner.router.usage_snapshot().await?;

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
    let routes = store.recent_routes(120).await.unwrap_or_default();
    let mut last_seen: HashMap<&str, &RouteRow> = HashMap::new();
    for r in &routes {
        last_seen.insert(r.label.as_str(), r);
        if let Some(f) = &r.fail_from {
            last_seen.insert(f.as_str(), r);
        }
    }
    let start = routes.len().saturating_sub(50);
    let log: Vec<Value> = routes[start..].iter().map(route_to_entry).collect();

    // Accounts table + status counts for the summary pills.
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
                "proxy": mask_proxy(&v.proxy),
                "status": status,
                "key": key,
                "web": web,
                "loggedIn": logged,
                "limits": v.limits.as_ref().map(|ll| json!({
                    "tier": ll.tier,
                    "features": ll.features.iter().map(|f| json!({
                        "feature": f.feature,
                        "remaining": f.remaining,
                        "resetAfter": f.reset_after,
                    })).collect::<Vec<_>>(),
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
        e.window_secs = e.window_secs.or(v.window_secs);
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
                });
                if a.web {
                    tile["webSession"] = json!(true);
                    tile["loggedIn"] = json!(a.logged);
                }
                if a.has_dr {
                    tile["dr"] = json!({ "used": a.dr_used, "quota": a.dr_quota });
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

    Ok(json!({
        "groups": groups,
        "accounts": accounts,
        "health": health,
        "log": log,
        "stream": [],
        "usage": usage,
        "summary": { "accounts": mains.len(), "healthy": healthy, "needsLogin": needs_login, "exhausted": exhausted },
        "totalRemaining": total_remaining,
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
        "jina" | "firecrawl" => ("read", "Read / scrape"),
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

fn desc_of(provider: &str) -> &'static str {
    match provider {
        "serper" => "Web search API",
        "tavily" => "Search + extract API",
        "exa" => "Neural search API",
        "parallel" => "Search API",
        "jina" => "Reader — URL → markdown",
        "firecrawl" => "Crawl + scrape API",
        "steel" => "Headless browser sessions",
        "perplexity_web" => "Browser session · search + deep research",
        "gemini_web" => "Browser session · search + #dr",
        "grok_web" => "Browser session · search + #dr",
        _ => "",
    }
}
