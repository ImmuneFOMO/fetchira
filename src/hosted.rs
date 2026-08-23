//! Hosted HTTP runtime. The same Fetchira service is exposed over MCP Streamable HTTP;
//! Embedded hosted assets are rebuilt with the server whenever the UI bundle changes.
//! authentication and quota middleware are layered here as the hosted control plane grows.
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
use std::{collections::HashMap, path::PathBuf, sync::Arc};

use axum::{
    extract::State,
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router as AxumRouter,
};
use base64::Engine;
use chrono::{DateTime, Utc};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rust_embed::RustEmbed;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::{auth, cli, config, mcp::Fetchira, router::Router, usage::Store};

pub const PROTOCOL_VERSION: &str = "1";

#[derive(RustEmbed)]
#[folder = "webui/"]
#[exclude = "*.jsx"]
struct HostedAssets;

#[derive(Clone)]
struct HostedState {
    router: Arc<tokio::sync::RwLock<Arc<Router>>>,
    store: Store,
    active: Arc<tokio::sync::Semaphore>,
    key_active: Arc<tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>>,
    updates: crate::hosted_update::UpdateState,
    home: PathBuf,
    db_path: String,
}

pub async fn run(home: &std::path::Path, bind: &str) -> anyhow::Result<()> {
    crate::web::require_browser()?;
    let config_path = home.join("fetchira.toml");
    let cfg = config::load_hosted(&config_path)?;
    let db_path = config::resolve_db(home, &cfg.db_path);
    let store = Store::open(&db_path).await?;
    let router = Arc::new(Router::build(cfg, store.clone()).await?);
    let shared_router = Arc::new(tokio::sync::RwLock::new(router.clone()));
    let state = HostedState {
        router: shared_router.clone(),
        store: store.clone(),
        active: Arc::new(tokio::sync::Semaphore::new(64)),
        key_active: Default::default(),
        updates: Default::default(),
        home: home.to_path_buf(),
        db_path,
    };

    // Keep provider limits and balances warm so the hosted dashboard's cached snapshot
    // resolves its loading state without blocking every poll on provider network calls.
    {
        let shared_router = shared_router.clone();
        tokio::spawn(async move {
            loop {
                let router = shared_router.read().await.clone();
                router.warm().await;
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
        });
    }
    let cancellation = CancellationToken::new();
    let mut http_config = StreamableHttpServerConfig::default();
    http_config.cancellation_token = cancellation.clone();
    http_config.allowed_hosts.clear();
    let mcp = StreamableHttpService::new(
        {
            let shared_router = shared_router.clone();
            move || Ok(Fetchira::new_shared(shared_router.clone()))
        },
        Arc::new(LocalSessionManager::default()),
        http_config,
    );
    let version = env!("CARGO_PKG_VERSION");
    let app = AxumRouter::new()
        .route("/", get(|| async {
            axum::response::Redirect::temporary("/admin")
        }))
        .route("/healthz", get(|| async { Json(json!({"ok": true})) }))
        .route("/readyz", get(|| async { Json(json!({"ready": true})) }))
        .route("/version", get(move || async move {
            Json(json!({"server_version": version, "protocol_version": PROTOCOL_VERSION, "schema_version": crate::usage::SCHEMA, "min_client_schema_version": 1, "max_client_schema_version": crate::usage::SCHEMA}))
        }))
        .route("/admin/login", axum::routing::post(admin_login))
        .route("/auth/check", get(auth_check))
        .route("/remote/check", get(remote_check))
        .route("/admin/keys", get(admin_keys).post(admin_create_key))
        .route("/admin/keys/{id}/revoke", axum::routing::post(admin_revoke_key))
        .route("/admin/usage", get(admin_usage))
        .route("/admin/debug", get(admin_debug))
        .route("/admin/debug/{id}", get(admin_debug_one))
        .route("/admin/state", get(admin_state))
        .route("/admin/usage/{id}/attempts", get(admin_attempts))
        .route("/admin/audit", get(admin_audit))
        .route("/admin/providers", get(admin_providers))
        .route("/admin/providers/key", axum::routing::post(admin_provider_key))
        .route(
            "/admin/providers/session",
            axum::routing::post(admin_provider_session),
        )
        .route("/admin/account/remove", axum::routing::post(admin_account_remove))
        .route("/admin/account/rename", axum::routing::post(admin_account_rename))
        .route("/admin/account/proxy", axum::routing::post(admin_account_proxy))
        .route("/admin/account/test", axum::routing::post(admin_account_test))
        .route("/admin/priority", axum::routing::post(admin_priority))
        .route("/admin/try", axum::routing::post(admin_try))
        .route(
            "/admin/login-challenges",
            axum::routing::post(admin_create_challenge),
        )
        .route(
            "/login-challenges/{id}",
            get(challenge_get).post(challenge_upload),
        )
        .route("/admin/login-challenges/{id}", get(admin_challenge_get))
        .route(
            "/admin/update",
            get(admin_update_status).post(admin_start_update),
        )
        .route("/usage", get(key_usage))
        .nest_service("/mcp", mcp)
        .fallback(hosted_static)
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    eprintln!(
        "fetchira hosted server listening on http://{}",
        listener.local_addr()?
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { cancellation.cancelled().await })
        .await?;
    Ok(())
}

async fn current_router(st: &HostedState) -> Arc<Router> {
    st.router.read().await.clone()
}

async fn rebuild_router(st: &HostedState) {
    let Ok(cfg) = config::load_hosted(&st.home.join("fetchira.toml")) else {
        return;
    };
    if let Ok(router) = Router::build(cfg, st.store.clone()).await {
        *st.router.write().await = Arc::new(router);
    }
}

async fn admin_state(State(st): State<HostedState>, headers: axum::http::HeaderMap) -> Response {
    if !require_admin(&st, &headers).await {
        return (StatusCode::UNAUTHORIZED, "admin login required").into_response();
    }
    let router = current_router(&st).await;
    let views = match router.usage_snapshot_cached().await {
        Ok(v) => v,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let cfg = config::load_hosted(&st.home.join("fetchira.toml")).ok();
    let mut logged = std::collections::HashSet::new();
    if let Some(cfg) = &cfg {
        for a in &cfg.accounts {
            if !a.provider.is_web()
                || st
                    .store
                    .load_session(&a.label)
                    .await
                    .ok()
                    .flatten()
                    .is_some()
            {
                logged.insert(a.label.clone());
            }
        }
    }
    let mains: Vec<_> = views.iter().filter(|v| !v.label.ends_with("#dr")).collect();
    let account_json = |v: &&crate::router::UsageView| {
        let kind = provider_kind(v.provider);
        let web = kind.is_some_and(|p| p.is_web());
        let ready = logged.contains(&v.label);
        json!({
            "provider": v.provider, "label": v.label, "used": v.used, "quota": v.quota,
            "remaining": v.remaining, "resetWindow": v.period, "proxy": v.proxy,
            "status": if v.exhausted { "exhausted" } else if !ready { "needs-login" } else { "healthy" },
            "key": !web, "web": web, "loggedIn": ready, "pending": v.pending,
            "limits": v.limits.as_ref().map(limits_json), "usd": v.usd
        })
    };
    let seen_labels: std::collections::HashSet<&str> =
        mains.iter().map(|v| v.label.as_str()).collect();
    let mut accounts: Vec<_> = mains.iter().map(account_json).collect();
    // Router buckets intentionally omit web accounts without cookies. Keep those configured
    // accounts visible so hosted users can reconnect them instead of seeing them disappear.
    if let Some(cfg) = &cfg {
        for account in &cfg.accounts {
            if account.provider.is_web() && !seen_labels.contains(account.label.as_str()) {
                accounts.push(json!({
                    "provider": account.provider.as_str(), "label": account.label,
                    "used": 0, "quota": 0, "remaining": 0, "resetWindow": null,
                    "proxy": account.proxy.as_deref().unwrap_or("direct"),
                    "status": "needs-login", "key": false, "web": true,
                    "loggedIn": false, "pending": false, "limits": null, "usd": null
                }));
            }
        }
    }
    let mut groups = Vec::new();
    for (id, label) in [
        ("search", "Search"),
        ("read", "Read / scrape"),
        ("browser", "Browser"),
        ("web", "Web sessions"),
    ] {
        let mut providers = Vec::new();
        for v in mains.iter().filter(|v| provider_group(v.provider) == id) {
            let web = provider_kind(v.provider).is_some_and(|p| p.is_web());
            if let Some(existing) = providers
                .iter_mut()
                .find(|p: &&mut serde_json::Value| p["name"] == v.provider)
            {
                existing["accounts"] = json!(existing["accounts"].as_i64().unwrap_or(0) + 1);
                existing["used"] = json!(existing["used"].as_i64().unwrap_or(0) + v.used);
                existing["quota"] = json!(existing["quota"].as_i64().unwrap_or(0) + v.quota);
                existing["loggedIn"] = json!(
                    existing["loggedIn"].as_bool().unwrap_or(false) || logged.contains(&v.label)
                );
                existing["pending"] =
                    json!(existing["pending"].as_bool().unwrap_or(false) || v.pending);
            } else {
                providers.push(json!({"name":v.provider,"desc":provider_desc(v.provider),"used":v.used,"quota":v.quota,"accounts":1,"resetWindow":v.period,"pending":v.pending,"key":!web,"webSession":web,"loggedIn":logged.contains(&v.label),"limits":provider_limit_rows(v),"features":provider_feature_rows(v),"catalog":provider_catalog(v)}));
            }
        }
        if let Some(cfg) = &cfg {
            for account in &cfg.accounts {
                if account.provider.is_web()
                    && provider_group(account.provider.as_str()) == id
                    && !seen_labels.contains(account.label.as_str())
                {
                    if let Some(existing) = providers
                        .iter_mut()
                        .find(|p: &&mut serde_json::Value| p["name"] == account.provider.as_str())
                    {
                        existing["accounts"] =
                            json!(existing["accounts"].as_i64().unwrap_or(0) + 1);
                    } else {
                        providers.push(json!({"name":account.provider.as_str(),"desc":provider_desc(account.provider.as_str()),"used":0,"quota":0,"accounts":1,"resetWindow":null,"pending":false,"key":false,"webSession":true,"loggedIn":false,"limits":[],"features":[],"catalog":[]}));
                    }
                }
            }
        }
        if !providers.is_empty() {
            groups.push(json!({"id":id,"label":label,"providers":providers}));
        }
    }
    let catalog: Vec<_> = crate::providers::ProviderKind::all().iter().map(|p| json!({"id":p.as_str(),"web":p.is_web(),"blurb":p.blurb(),"signup":p.signup(),"caps":[],"group":provider_group(p.as_str())})).collect();
    let routes = st.store.recent_routes(1000).await.unwrap_or_default();
    let log: Vec<_> = routes
        .iter()
        .rev()
        .take(50)
        .map(hosted_route_entry)
        .collect();
    let health: Vec<_> = accounts.iter().map(|a| {
        let label = a["label"].as_str().unwrap_or("");
        let last_success = routes.iter().rev().find(|r| r.label == label && r.status == 200).map(|r| hosted_ago(&r.ts)).unwrap_or_else(|| "—".into());
        let last_error = routes.iter().rev().find(|r| r.label == label && r.status >= 400).map(|r| format!("{} returned {}", r.provider, r.status));
        json!({"provider":a["label"],"state":a["status"],"lastSuccess":last_success,"lastError":last_error})
    }).collect();
    let today = Utc::now().date_naive();
    let mut usage_by_label: HashMap<String, [i64; 14]> = HashMap::new();
    for route in &routes {
        let Ok(ts) = DateTime::parse_from_rfc3339(&route.ts) else {
            continue;
        };
        let age = (today - ts.with_timezone(&Utc).date_naive()).num_days();
        if (0..14).contains(&age) {
            usage_by_label.entry(route.label.clone()).or_default()[13 - age as usize] += 1;
        }
    }
    let usage = usage_by_label
        .into_iter()
        .map(|(label, series)| {
            let provider = accounts
                .iter()
                .find(|a| a["label"].as_str() == Some(label.as_str()))
                .and_then(|a| a["provider"].as_str())
                .unwrap_or("router");
            json!({"provider":label,"color":hosted_group_color(provider),"series":series})
        })
        .collect::<Vec<_>>();
    let mut burn = accounts.iter().filter_map(|a| {
        let quota = a["quota"].as_i64().unwrap_or(0);
        (quota > 0).then(|| json!({"provider":a["provider"],"label":a["label"],"remaining":a["remaining"],"resetWindow":a["resetWindow"],"ratePerHour":0}))
    }).collect::<Vec<_>>();
    burn.sort_by_key(|a| a["remaining"].as_i64().unwrap_or(0));
    burn.truncate(5);
    let healthy = accounts.iter().filter(|a| a["status"] == "healthy").count();
    let priority = cli::PRIORITY_CAPS.iter().map(|cap| {
        let custom = cfg.as_ref().map(|c| c.priority.for_cap(*cap)).unwrap_or(&[]);
        let order = crate::providers::order_for(*cap, None, custom);
        json!({"capability":cap.as_str(),"order":order.iter().map(|p|p.as_str()).collect::<Vec<_>>(),"custom":!custom.is_empty(),"available":[]})
    }).collect::<Vec<_>>();
    let capabilities = mains.iter().filter_map(|v| provider_kind(v.provider)).map(|p| { let x = crate::providers::extras(p); json!({"provider":p.as_str(),"niches":x.niches,"modes":x.modes.iter().map(|(m,d)|json!([m,d])).collect::<Vec<_>>()}) }).collect::<Vec<_>>();
    let total_remaining: i64 = mains.iter().map(|v| v.remaining).sum();
    Json(json!({"accounts":accounts,"groups":groups,"catalog":catalog,"log":log,"stream":[],"health":health,"usage":usage,"burn":burn,"capabilities":capabilities,"priority":priority,"totalRemaining":total_remaining,"summary":{"accounts":accounts.len(),"healthy":healthy,"needsLogin":accounts.iter().filter(|a|a["status"]=="needs-login").count(),"exhausted":accounts.iter().filter(|a|a["status"]=="exhausted").count()}})).into_response()
}

fn provider_kind(name: &str) -> Option<crate::providers::ProviderKind> {
    crate::providers::ProviderKind::all()
        .iter()
        .copied()
        .find(|p| p.as_str() == name)
}
fn hosted_route_entry(r: &crate::usage::RouteRow) -> serde_json::Value {
    let mut entry = if let Some(from) = &r.fail_from {
        json!({"time":r.ts.get(11..19).unwrap_or(""),"capability":r.capability,"failover":{"from":from,"code":r.fail_code,"to":r.label},"status":r.status,"latencyMs":r.latency_ms})
    } else {
        json!({"time":r.ts.get(11..19).unwrap_or(""),"capability":r.capability,"provider":r.provider,"account":r.label.rsplit('-').next().and_then(|n| n.parse::<i64>().ok()).unwrap_or(1),"status":r.status,"latencyMs":r.latency_ms})
    };
    if !r.niche.is_empty() {
        entry["niche"] = json!(r.niche);
    }
    if let Some(id) = r.debug_id {
        entry["debugId"] = json!(id);
    }
    entry
}
fn hosted_ago(ts: &str) -> String {
    let Ok(then) = DateTime::parse_from_rfc3339(ts) else {
        return "—".into();
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
fn hosted_group_color(provider: &str) -> &'static str {
    match provider_group(provider) {
        "search" => "var(--lime-500)",
        "read" => "var(--cyan-500)",
        "browser" => "var(--amber-500)",
        _ => "#C792EA",
    }
}
fn provider_group(name: &str) -> &'static str {
    match name {
        "serper" | "tavily" | "exa" | "parallel" => "search",
        "firecrawl" => "read",
        "steel" => "browser",
        _ => "web",
    }
}
fn provider_desc(name: &str) -> &'static str {
    match name {
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
fn limits_json(l: &crate::providers::LiveLimits) -> serde_json::Value {
    json!({"tier":l.tier,"features":l.features,"models":l.models})
}
fn provider_limit_rows(v: &crate::router::UsageView) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    if v.quota > 0 {
        out.push(json!({"label":"router quota","used":v.used,"quota":v.quota,"window":v.period,"locked":false,"usd":v.usd}));
    }
    out.extend(v.limits.as_ref().map(|l| l.features.iter().filter_map(|f| f.total.map(|total| json!({"label":f.feature,"used":(total-f.remaining).max(0),"quota":total,"resetAt":f.reset_after,"locked":total == 0}))).collect::<Vec<_>>()).unwrap_or_default());
    if out.is_empty() && !provider_kind(v.provider).is_some_and(|p| p.is_web()) {
        out.push(json!({"label":"quota","used":v.used,"quota":v.quota,"window":v.period,"locked":false,"usd":v.usd}));
    }
    out
}
fn provider_feature_rows(v: &crate::router::UsageView) -> Vec<serde_json::Value> {
    v.limits
        .as_ref()
        .map(|l| {
            l.features
                .iter()
                .filter(|f| f.total.is_none())
                .map(|f| json!({"label":f.feature,"remaining":f.remaining,"resetAt":f.reset_after}))
                .collect()
        })
        .unwrap_or_default()
}
fn provider_catalog(v: &crate::router::UsageView) -> Vec<serde_json::Value> {
    v.limits
        .as_ref()
        .map(|l| {
            l.models
                .iter()
                .filter(|m| m.total.is_none())
                .map(|m| json!({"name":m.name,"levels":m.levels}))
                .collect()
        })
        .unwrap_or_default()
}

async fn hosted_static(uri: axum::http::Uri) -> Response {
    let path = match uri.path() {
        "/admin" | "/admin/" => "hosted/index.html".to_string(),
        "/404" => "hosted/not-found.html".to_string(),
        p if p.starts_with("/admin/assets/") => p.trim_start_matches("/admin/assets/").to_string(),
        _ => return render_hosted_not_found(),
    };
    match HostedAssets::get(&path) {
        Some(file) => (
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    match path.rsplit('.').next() {
                        Some("html") => "text/html; charset=utf-8",
                        Some("css") => "text/css; charset=utf-8",
                        Some("js") | Some("jsx") => "application/javascript; charset=utf-8",
                        Some("svg") => "image/svg+xml",
                        _ => "application/octet-stream",
                    },
                ),
                (axum::http::header::CACHE_CONTROL, "no-cache"),
            ],
            file.data.into_owned(),
        )
            .into_response(),
        None => render_hosted_not_found(),
    }
}

fn render_hosted_not_found() -> Response {
    match HostedAssets::get("hosted/not-found.html") {
        Some(file) => (
            StatusCode::NOT_FOUND,
            [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
            file.data.into_owned(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn admin_usage(State(st): State<HostedState>, headers: axum::http::HeaderMap) -> Response {
    if !require_admin(&st, &headers).await {
        return (StatusCode::UNAUTHORIZED, "admin login required").into_response();
    }
    match st.store.recent_requests(None, 500).await {
        Ok(rows) => Json(json!({"ok":true,"rows":rows})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

fn hosted_debug_entry(r: &crate::usage::DebugRow) -> serde_json::Value {
    let body = r.response.as_deref().or(r.error.as_deref()).unwrap_or("");
    json!({"id":r.id,"time":r.ts.get(11..19).unwrap_or(""),"capability":r.capability,"provider":r.provider,"account":1,"status":r.status,"latencyMs":r.latency_ms,"ok":r.status == 200,"request":r.request,"preview":body.chars().take(180).collect::<String>().replace(['\n','\r']," ")})
}

async fn admin_debug(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    if !require_admin(&st, &headers).await {
        return (StatusCode::UNAUTHORIZED, "admin login required").into_response();
    }
    let after = q.get("after").and_then(|v| v.parse().ok()).unwrap_or(0);
    let limit = q
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(100)
        .clamp(1, 500);
    let rows = if after > 0 {
        st.store.debug_since(after, limit).await
    } else {
        st.store.recent_debug(limit).await
    };
    match rows {
        Ok(rows) => {
            let max_id = rows.iter().map(|r| r.id).max().unwrap_or(after);
            Json(json!({"rows":rows.iter().map(hosted_debug_entry).collect::<Vec<_>>(),"maxId":max_id})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn admin_debug_one(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Response {
    if !require_admin(&st, &headers).await {
        return (StatusCode::UNAUTHORIZED, "admin login required").into_response();
    }
    match st.store.debug_get(id).await {
        Ok(Some(r)) => {
            let trace = r
                .http_trace
                .as_deref()
                .and_then(|v| serde_json::from_str::<serde_json::Value>(v).ok());
            Json(json!({"id":r.id,"ts":r.ts,"capability":r.capability,"provider":r.provider,"label":r.label,"status":r.status,"latencyMs":r.latency_ms,"request":r.request,"response":r.response,"error":r.error,"httpTrace":trace})).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn admin_attempts(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    if !require_admin(&st, &headers).await {
        return (StatusCode::UNAUTHORIZED, "admin login required").into_response();
    }
    match st.store.attempts_for(&id).await {
        Ok(rows) => Json(json!({"ok":true,"requestId":id,"rows":rows})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn admin_audit(State(st): State<HostedState>, headers: axum::http::HeaderMap) -> Response {
    if !require_admin(&st, &headers).await {
        return (StatusCode::UNAUTHORIZED, "admin login required").into_response();
    }
    match st.store.recent_audit(500).await {
        Ok(rows) => Json(json!({"ok":true,"rows":rows})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn admin_providers(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
) -> Response {
    if !require_admin(&st, &headers).await {
        return (StatusCode::UNAUTHORIZED, "admin login required").into_response();
    }
    let cfg = match config::load_hosted(&st.home.join("fetchira.toml")) {
        Ok(cfg) => cfg,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let identities = st.store.all_identities().await.unwrap_or_default();
    let mut rows = Vec::with_capacity(cfg.accounts.len());
    for account in cfg.accounts {
        let logged_in = st
            .store
            .load_session(&account.label)
            .await
            .ok()
            .flatten()
            .is_some()
            || account.api_key.is_some();
        rows.push(json!({"provider":account.provider.as_str(),"label":account.label,"identity":identities.get(&account.label),"loggedIn":logged_in,"ready":logged_in}));
    }
    Json(json!({"ok":true,"providers":rows})).into_response()
}

#[derive(serde::Deserialize)]
struct ProviderKeyRequest {
    provider: crate::providers::ProviderKind,
    label: Option<String>,
    key: Option<String>,
    proxy: Option<String>,
}

async fn admin_provider_key(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ProviderKeyRequest>,
) -> Response {
    if !require_admin_or_scope(&st, &headers, auth::Scope::AccountsManage).await {
        return (
            StatusCode::UNAUTHORIZED,
            "admin login and CSRF token required",
        )
            .into_response();
    }
    if req.provider.is_web() {
        return (
            StatusCode::BAD_REQUEST,
            "browser providers require a session login",
        )
            .into_response();
    }
    match cli::add_account(
        &st.home,
        req.provider,
        req.label.as_deref(),
        req.key,
        req.proxy,
    ) {
        Ok(label) => {
            if let Err(e) = config::load_hosted(&st.home.join("fetchira.toml")) {
                let _ = cli::remove_account(&st.home, &label).await;
                return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
            }
            rebuild_router(&st).await;
            let _ = st
                .store
                .log_audit("admin", "provider.add", Some(&label), None)
                .await;
            Json(json!({"ok": true, "label": label})).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct ProviderSessionRequest {
    provider: Option<crate::providers::ProviderKind>,
    label: String,
    session: String,
    proxy: Option<String>,
}

async fn admin_provider_session(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ProviderSessionRequest>,
) -> Response {
    if !require_admin_or_scope(&st, &headers, auth::Scope::AccountsManage).await {
        return (
            StatusCode::UNAUTHORIZED,
            "admin login and CSRF token required",
        )
            .into_response();
    }
    let cfg = config::load_hosted(&st.home.join("fetchira.toml"));
    let existing = cfg.as_ref().ok().and_then(|c| {
        c.accounts
            .iter()
            .find(|a| !req.label.trim().is_empty() && a.label == req.label)
    });
    let provider = req.provider.or_else(|| existing.map(|a| a.provider));
    let Some(provider) = provider else {
        return (StatusCode::BAD_REQUEST, "provider is required").into_response();
    };
    if req.label.len() > 80 || req.session.len() > 1024 * 1024 {
        return (StatusCode::BAD_REQUEST, "invalid label or session size").into_response();
    }
    if crate::web::parse_session(&req.session).cookies.is_empty() {
        return (StatusCode::BAD_REQUEST, "session contains no cookies").into_response();
    }
    let label = if let Some(account) = existing {
        if account.provider != provider {
            return (StatusCode::BAD_REQUEST, "provider does not match account").into_response();
        }
        req.label.clone()
    } else {
        // Empty labels are allowed for new accounts; the CLI resolves the provider-based name.
        match cli::add_account(&st.home, provider, Some(&req.label), None, req.proxy) {
            Ok(label) => label,
            Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        }
    };
    if label.is_empty() {
        return (StatusCode::BAD_REQUEST, "invalid label").into_response();
    }
    if let Ok(cfg) = config::load_hosted(&st.home.join("fetchira.toml")) {
        if let Some(account) = cfg.accounts.iter().find(|a| a.label == label) {
            if !account.provider.is_web() {
                return (
                    StatusCode::BAD_REQUEST,
                    "provider account must support browser sessions",
                )
                    .into_response();
            }
        }
    }
    match st
        .store
        .save_session(&label, provider.as_str(), &req.session)
        .await
    {
        Ok(()) => {
            rebuild_router(&st).await;
            let _ = st
                .store
                .log_audit("admin", "provider.session", Some(&label), None)
                .await;
            Json(json!({"ok":true})).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct HostedLabelRequest {
    label: String,
}

async fn admin_account_remove(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<HostedLabelRequest>,
) -> Response {
    if !require_admin_or_scope(&st, &headers, auth::Scope::AccountsManage).await {
        return (
            StatusCode::UNAUTHORIZED,
            "admin login and CSRF token required",
        )
            .into_response();
    }
    match cli::remove_account(&st.home, &req.label).await {
        Ok(()) => {
            rebuild_router(&st).await;
            Json(json!({"ok":true})).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct HostedRenameRequest {
    label: String,
    new_label: String,
}

async fn admin_account_rename(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<HostedRenameRequest>,
) -> Response {
    if !require_admin_or_scope(&st, &headers, auth::Scope::AccountsManage).await {
        return (
            StatusCode::UNAUTHORIZED,
            "admin login and CSRF token required",
        )
            .into_response();
    }
    match cli::rename_account(&st.home, &req.label, &req.new_label).await {
        Ok(()) => {
            rebuild_router(&st).await;
            Json(json!({"ok":true})).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct HostedProxyRequest {
    label: String,
    proxy: String,
}

async fn admin_account_proxy(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<HostedProxyRequest>,
) -> Response {
    if !require_admin_or_scope(&st, &headers, auth::Scope::AccountsManage).await {
        return (
            StatusCode::UNAUTHORIZED,
            "admin login and CSRF token required",
        )
            .into_response();
    }
    match cli::set_proxy(&st.home, &req.label, cli::parse_proxy_arg(&req.proxy)).await {
        Ok(()) => {
            rebuild_router(&st).await;
            Json(json!({"ok":true})).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn admin_account_test(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<HostedLabelRequest>,
) -> Response {
    if !require_admin_or_scope(&st, &headers, auth::Scope::AccountsManage).await {
        return (
            StatusCode::UNAUTHORIZED,
            "admin login and CSRF token required",
        )
            .into_response();
    }
    let Some(kind) = config::load_hosted(&st.home.join("fetchira.toml"))
        .ok()
        .and_then(|c| {
            c.accounts
                .into_iter()
                .find(|a| a.label == req.label)
                .map(|a| a.provider)
        })
    else {
        return (StatusCode::BAD_REQUEST, "unknown account").into_response();
    };
    let (cap, input) = match kind {
        crate::providers::ProviderKind::Firecrawl => (
            crate::providers::Capability::Read,
            crate::providers::Input {
                url: Some("https://example.com".into()),
                ..Default::default()
            },
        ),
        crate::providers::ProviderKind::Steel => (
            crate::providers::Capability::Browser,
            crate::providers::Input {
                url: Some("https://example.com".into()),
                ..Default::default()
            },
        ),
        _ => (
            crate::providers::Capability::Search,
            crate::providers::Input {
                query: Some("fetchira connectivity test".into()),
                ..Default::default()
            },
        ),
    };
    let t0 = Instant::now();
    let router = current_router(&st).await;
    let result = crate::usage::HOSTED_REQUEST_ID
        .scope(
            "admin-ui".to_string(),
            router.call_account(cap, &input, kind, &req.label),
        )
        .await;
    match result {
        Ok(_) => {
            Json(json!({"ok":true,"latencyMs":t0.elapsed().as_millis() as i64})).into_response()
        }
        Err(e) => Json(
            json!({"ok":false,"latencyMs":t0.elapsed().as_millis() as i64,"error":e.to_string()}),
        )
        .into_response(),
    }
}

#[derive(serde::Deserialize)]
struct HostedPriorityRequest {
    capability: String,
    #[serde(default)]
    order: Vec<String>,
}

async fn admin_priority(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<HostedPriorityRequest>,
) -> Response {
    if !require_admin_or_scope(&st, &headers, auth::Scope::AccountsManage).await {
        return (
            StatusCode::UNAUTHORIZED,
            "admin login and CSRF token required",
        )
            .into_response();
    }
    let Some(cap) = crate::providers::Capability::parse(&req.capability) else {
        return (StatusCode::BAD_REQUEST, "unknown capability").into_response();
    };
    let kinds = req
        .order
        .iter()
        .filter_map(|p| serde_json::from_value(serde_json::Value::String(p.clone())).ok())
        .collect();
    match cli::set_priority(&st.home, cap, kinds) {
        Ok(()) => {
            rebuild_router(&st).await;
            Json(json!({"ok":true})).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct HostedTryRequest {
    q: String,
}

async fn admin_try(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<HostedTryRequest>,
) -> Response {
    if !require_admin_mut(&st, &headers).await {
        return (
            StatusCode::UNAUTHORIZED,
            "admin login and CSRF token required",
        )
            .into_response();
    }
    let q = req.q.trim();
    if q.is_empty() {
        return (StatusCode::BAD_REQUEST, "empty query").into_response();
    }
    let input = crate::providers::Input {
        query: Some(q.to_string()),
        ..Default::default()
    };
    let router = current_router(&st).await;
    let result = crate::usage::HOSTED_REQUEST_ID
        .scope(
            "admin-ui".to_string(),
            router.call(crate::providers::Capability::Search, &input, None),
        )
        .await;
    match result {
        Ok(reply) => Json(json!({"ok":true,"text":reply.text})).into_response(),
        Err(e) => Json(json!({"ok":false,"error":e.to_string()})).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct ChallengeRequest {
    provider: Option<crate::providers::ProviderKind>,
    label: String,
}

async fn admin_create_challenge(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ChallengeRequest>,
) -> Response {
    if !require_admin_or_scope(&st, &headers, auth::Scope::AccountsManage).await {
        return (
            StatusCode::UNAUTHORIZED,
            "admin login and CSRF token required",
        )
            .into_response();
    }
    let provider = req.provider.or_else(|| {
        config::load_hosted(&st.home.join("fetchira.toml"))
            .ok()
            .and_then(|c| {
                c.accounts
                    .iter()
                    .find(|a| a.label == req.label)
                    .map(|a| a.provider)
            })
    });
    let Some(provider) = provider else {
        return (StatusCode::BAD_REQUEST, "provider is required").into_response();
    };
    let label = if req.label.trim().is_empty() {
        match cli::add_account(&st.home, provider, None, None, None) {
            Ok(label) => label,
            Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        }
    } else {
        req.label.trim().to_string()
    };
    if label.len() > 80 {
        return (StatusCode::BAD_REQUEST, "invalid label").into_response();
    }
    let mut raw = [0_u8; 24];
    OsRng.fill_bytes(&mut raw);
    let id = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    let expires = (Utc::now() + chrono::Duration::minutes(10)).to_rfc3339();
    match st
        .store
        .save_login_challenge(&id, provider.as_str(), &label, "*", &expires)
        .await
    {
        Ok(()) => Json(json!({"ok":true,"challenge":id,"label":label,"provider":provider.as_str(),"expiresAt":expires})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

fn bearer(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .or_else(|| {
            headers
                .get(axum::http::header::AUTHORIZATION)?
                .to_str()
                .ok()?
                .strip_prefix("bearer ")
        })
}

async fn challenge_key(st: &HostedState, headers: &axum::http::HeaderMap) -> Option<String> {
    let token = bearer(headers)?;
    let key = st
        .store
        .find_api_key_by_hash(&auth::hash_key(token))
        .await
        .ok()??;
    (!key.revoked
        && key
            .expires_at
            .as_deref()
            .and_then(|v| v.parse::<chrono::DateTime<Utc>>().ok())
            .is_none_or(|expires| expires > Utc::now())
        && auth::has_scope(&key.scopes, auth::Scope::AccountsManage))
    .then_some(key.id)
}

async fn challenge_get(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let Some(key_id) = challenge_key(&st, &headers).await else {
        return (StatusCode::FORBIDDEN, "accounts:manage scope required").into_response();
    };
    challenge_response(&st, &id, &key_id).await
}

async fn admin_challenge_get(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    if !require_admin(&st, &headers).await {
        return (StatusCode::UNAUTHORIZED, "admin login required").into_response();
    }
    match st.store.login_challenge_any(&id).await {
        Ok(Some((provider, label, expires, consumed))) => {
            if expires <= Utc::now().to_rfc3339() {
                return (StatusCode::GONE, "challenge expired").into_response();
            }
            Json(json!({"provider":provider,"label":label,"expires_at":expires,"expiresAt":expires,"status":if consumed {"complete"} else {"pending"},"consumed":consumed})).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "challenge not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn challenge_response(st: &HostedState, id: &str, key_id: &str) -> Response {
    match st.store.login_challenge(id, key_id).await {
        Ok(Some((provider, label, expires, consumed))) => {
            if expires <= Utc::now().to_rfc3339() {
                return (StatusCode::GONE, "challenge expired").into_response();
            }
            Json(json!({"provider":provider,"label":label,"expires_at":expires,"expiresAt":expires,"status":if consumed {"complete"} else {"pending"},"consumed":consumed})).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "challenge not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct ChallengeUpload {
    session: String,
}

async fn challenge_upload(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<ChallengeUpload>,
) -> Response {
    let Some(key_id) = challenge_key(&st, &headers).await else {
        return (StatusCode::FORBIDDEN, "accounts:manage scope required").into_response();
    };
    if req.session.len() > 1024 * 1024 {
        return (StatusCode::PAYLOAD_TOO_LARGE, "session too large").into_response();
    }
    let Ok(Some((provider, label, expires, consumed))) =
        st.store.login_challenge(&id, &key_id).await
    else {
        return (StatusCode::NOT_FOUND, "challenge not found").into_response();
    };
    if consumed || expires <= Utc::now().to_rfc3339() {
        return (StatusCode::GONE, "challenge expired or already consumed").into_response();
    }
    if crate::web::parse_session(&req.session).cookies.is_empty() {
        return (StatusCode::BAD_REQUEST, "session contains no cookies").into_response();
    }
    match st
        .store
        .consume_challenge_and_save_session(&id, &key_id, &provider, &label, &req.session)
        .await
    {
        Ok(true) => {
            rebuild_router(&st).await;
            let _ = st
                .store
                .log_audit(&key_id, "provider.session", Some(&label), None)
                .await;
            Json(json!({"ok":true})).into_response()
        }
        Ok(false) => (StatusCode::CONFLICT, "challenge already consumed").into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn key_usage(State(st): State<HostedState>, headers: axum::http::HeaderMap) -> Response {
    let Some(value) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return (StatusCode::UNAUTHORIZED, "missing bearer token").into_response();
    };
    let Some(token) = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
    else {
        return (StatusCode::UNAUTHORIZED, "expected bearer token").into_response();
    };
    let Ok(Some(key)) = st.store.find_api_key_by_hash(&auth::hash_key(token)).await else {
        return (StatusCode::UNAUTHORIZED, "invalid API key").into_response();
    };
    if key.revoked
        || key
            .expires_at
            .as_deref()
            .and_then(|v| v.parse::<chrono::DateTime<Utc>>().ok())
            .is_some_and(|expires| expires <= Utc::now())
        || !auth::has_scope(&key.scopes, auth::Scope::UsageRead)
    {
        return (StatusCode::FORBIDDEN, "usage scope required").into_response();
    }
    match st.store.recent_requests(Some(&key.id), 200).await {
        Ok(rows) => Json(json!({"ok":true,"keyId":key.id,"rows":rows})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct LoginRequest {
    password: String,
}
#[derive(serde::Deserialize)]
struct CreateKeyRequest {
    id: String,
    name: String,
    #[serde(default = "default_scopes")]
    scopes: Vec<String>,
    #[serde(default = "default_rpm")]
    rpm: i64,
    #[serde(default)]
    daily_limit: i64,
    #[serde(default)]
    monthly_limit: i64,
    #[serde(default = "default_concurrency")]
    concurrency_limit: i64,
    expires_at: Option<String>,
}
fn default_scopes() -> Vec<String> {
    vec!["mcp".into(), "usage:read".into()]
}
fn default_rpm() -> i64 {
    60
}
fn default_concurrency() -> i64 {
    4
}
fn cookie_value(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get("cookie")?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|p| p.trim().strip_prefix(&format!("{name}=")))
        .map(str::to_string)
}
fn admin_token(headers: &axum::http::HeaderMap) -> Option<String> {
    cookie_value(headers, "fetchira_admin")
}
async fn admin_login(State(st): State<HostedState>, Json(req): Json<LoginRequest>) -> Response {
    let expected = std::env::var("FETCHIRA_ADMIN_PASSWORD")
        .ok()
        .or_else(|| {
            std::env::var("FETCHIRA_ADMIN_PASSWORD_FILE")
                .ok()
                .and_then(|p| std::fs::read_to_string(p).ok())
        })
        .unwrap_or_default()
        .trim()
        .to_string();
    if expected.is_empty() || !auth::verify_password(&req.password, &expected) {
        return (StatusCode::UNAUTHORIZED, "invalid credentials").into_response();
    }
    let mut raw = [0u8; 32];
    OsRng.fill_bytes(&mut raw);
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    OsRng.fill_bytes(&mut raw);
    let csrf = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    let hash =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()));
    let expires = (Utc::now() + chrono::Duration::hours(12)).to_rfc3339();
    let _ = st.store.save_admin_session(&hash, &expires).await;
    let mut out = Json(json!({"ok":true})).into_response();
    out.headers_mut().append(
        axum::http::header::SET_COOKIE,
        format!("fetchira_admin={token}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=43200")
            .parse()
            .unwrap(),
    );
    out.headers_mut().append(
        axum::http::header::SET_COOKIE,
        format!("fetchira_csrf={csrf}; Secure; SameSite=Lax; Path=/; Max-Age=43200")
            .parse()
            .unwrap(),
    );
    let _ = st.store.log_audit("admin", "login", None, None).await;
    out
}
async fn require_admin(st: &HostedState, headers: &axum::http::HeaderMap) -> bool {
    let Some(t) = admin_token(headers) else {
        return false;
    };
    let h = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(t.as_bytes()));
    st.store.valid_admin_session(&h).await.unwrap_or(false)
}
async fn require_admin_mut(st: &HostedState, headers: &axum::http::HeaderMap) -> bool {
    if !require_admin(st, headers).await {
        return false;
    }
    let Some(expected) = cookie_value(headers, "fetchira_csrf") else {
        return false;
    };
    headers.get("x-csrf-token").and_then(|v| v.to_str().ok()) == Some(expected.as_str())
}

async fn require_admin_or_scope(
    st: &HostedState,
    headers: &axum::http::HeaderMap,
    scope: auth::Scope,
) -> bool {
    if require_admin_mut(st, headers).await {
        return true;
    }
    let Some(token) = bearer(headers) else {
        return false;
    };
    let Ok(Some(key)) = st.store.find_api_key_by_hash(&auth::hash_key(token)).await else {
        return false;
    };
    !key.revoked
        && key
            .expires_at
            .as_deref()
            .and_then(|v| v.parse::<chrono::DateTime<Utc>>().ok())
            .is_none_or(|expires| expires > Utc::now())
        && auth::has_scope(&key.scopes, scope)
}
async fn admin_keys(State(st): State<HostedState>, headers: axum::http::HeaderMap) -> Response {
    if !require_admin(&st, &headers).await {
        return (StatusCode::UNAUTHORIZED, "admin login required").into_response();
    }
    match st.store.list_api_keys().await {
        Ok(keys) => {
            let mut out = Vec::with_capacity(keys.len());
            for k in keys {
                let checked = st.store.key_was_checked(&k.id).await.unwrap_or(false);
                out.push(json!({"id":k.id,"name":k.name,"scopes":k.scopes,"revoked":k.revoked,"expiresAt":k.expires_at,"rpm":k.rpm,"dailyLimit":k.daily_limit,"monthlyLimit":k.monthly_limit,"concurrencyLimit":k.concurrency_limit,"lastUsedAt":k.last_used_at,"remoteChecked":checked}));
            }
            Json(json!({"ok":true,"keys":out})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
async fn admin_create_key(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<CreateKeyRequest>,
) -> Response {
    if !require_admin_mut(&st, &headers).await {
        return (StatusCode::UNAUTHORIZED, "admin login required").into_response();
    };
    let allowed = ["mcp", "usage:read", "accounts:manage", "server:update"];
    if req.scopes.is_empty()
        || req.scopes.iter().any(|s| !allowed.contains(&s.as_str()))
        || req.rpm < 0
        || req.daily_limit < 0
        || req.monthly_limit < 0
        || req.concurrency_limit < 1
        || req.concurrency_limit > 64
        || req
            .expires_at
            .as_deref()
            .is_some_and(|v| v.parse::<chrono::DateTime<Utc>>().is_err())
    {
        return (StatusCode::BAD_REQUEST, "invalid scopes, limits, or expiry").into_response();
    }
    let scopes = req.scopes.iter().filter_map(|scope| match scope.as_str() {
        "mcp" => Some(auth::Scope::Mcp),
        "usage:read" => Some(auth::Scope::UsageRead),
        "accounts:manage" => Some(auth::Scope::AccountsManage),
        "server:update" => Some(auth::Scope::ServerUpdate),
        _ => None,
    });
    match auth::generate_key(req.id, scopes) {
        Ok(key) => match st
            .store
            .save_api_key_with_limits(
                &key,
                &req.name,
                req.rpm,
                req.daily_limit,
                req.monthly_limit,
                req.concurrency_limit,
                req.expires_at.as_deref(),
            )
            .await
        {
            Ok(()) => {
                let _ = st
                    .store
                    .log_audit("admin", "api_key.create", Some(&key.id), None)
                    .await;
                Json(json!({"ok":true,"key":key.plaintext,"id":key.id})).into_response()
            }
            Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        },
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}
async fn admin_revoke_key(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    if !require_admin_mut(&st, &headers).await {
        return (StatusCode::UNAUTHORIZED, "admin login required").into_response();
    };
    let ok = st.store.revoke_api_key(&id).await.unwrap_or(false);
    if ok {
        let _ = st
            .store
            .log_audit("admin", "api_key.revoke", Some(&id), None)
            .await;
    }
    Json(json!({"ok":ok})).into_response()
}

#[derive(serde::Deserialize)]
struct UpdateRequest {
    #[serde(default)]
    when_idle: bool,
    mode: Option<String>,
}

async fn admin_update_status(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
) -> Response {
    if !require_admin(&st, &headers).await {
        return (StatusCode::UNAUTHORIZED, "admin login required").into_response();
    }
    Json(json!({"ok":true,"job":st.updates.get().await})).into_response()
}

async fn admin_start_update(
    State(st): State<HostedState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<UpdateRequest>,
) -> Response {
    if !require_admin_or_scope(&st, &headers, auth::Scope::ServerUpdate).await {
        return (
            StatusCode::UNAUTHORIZED,
            "admin login and CSRF token required",
        )
            .into_response();
    }
    if st.updates.start().await.is_none() {
        return (StatusCode::CONFLICT, "update already running").into_response();
    }
    let when_idle = req.when_idle || req.mode.as_deref() == Some("idle");
    if req
        .mode
        .as_deref()
        .is_some_and(|mode| !matches!(mode, "idle" | "now"))
    {
        return (StatusCode::BAD_REQUEST, "mode must be now or idle").into_response();
    }
    let _ = st
        .store
        .log_audit(
            "admin",
            "server.update",
            None,
            Some(if when_idle { "when_idle" } else { "now" }),
        )
        .await;
    let state = st.clone();
    tokio::spawn(async move {
        let result = crate::hosted_update::perform(
            &state.home,
            &state.db_path,
            state.active.clone(),
            when_idle,
        )
        .await;
        match result {
            Ok(message) => {
                state
                    .updates
                    .finish("restart_required", Some(message))
                    .await;
                // Docker/systemd owns the process lifecycle. Exit after the response has had a
                // chance to reach the admin client so the supervisor starts the replaced binary.
                tokio::time::sleep(Duration::from_secs(1)).await;
                std::process::exit(75);
            }
            Err(e) => state.updates.finish("failed", Some(e.to_string())).await,
        }
    });
    (
        StatusCode::ACCEPTED,
        Json(json!({"ok":true,"status":"queued"})),
    )
        .into_response()
}

async fn auth_check(State(st): State<HostedState>, headers: axum::http::HeaderMap) -> Response {
    let Some(value) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
    else {
        return (StatusCode::UNAUTHORIZED, "missing bearer token").into_response();
    };
    let Ok(Some(key)) = st.store.find_api_key_by_hash(&auth::hash_key(value)).await else {
        return (StatusCode::UNAUTHORIZED, "invalid API key").into_response();
    };
    if key.revoked
        || key
            .expires_at
            .as_deref()
            .and_then(|v| v.parse::<chrono::DateTime<Utc>>().ok())
            .is_some_and(|v| v <= Utc::now())
        || !auth::has_scope(&key.scopes, auth::Scope::Mcp)
    {
        return (
            StatusCode::FORBIDDEN,
            "API key is revoked, expired, or lacks mcp scope",
        )
            .into_response();
    }
    Json(json!({"ok":true,"keyId":key.id,"scopes":key.scopes})).into_response()
}

async fn remote_check(State(st): State<HostedState>, headers: axum::http::HeaderMap) -> Response {
    let Some(value) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
    else {
        return (StatusCode::UNAUTHORIZED, "missing bearer token").into_response();
    };
    let Ok(Some(key)) = st.store.find_api_key_by_hash(&auth::hash_key(value)).await else {
        return (StatusCode::UNAUTHORIZED, "invalid API key").into_response();
    };
    if key.revoked
        || key
            .expires_at
            .as_deref()
            .and_then(|v| v.parse::<DateTime<Utc>>().ok())
            .is_some_and(|v| v <= Utc::now())
        || !auth::has_scope(&key.scopes, auth::Scope::Mcp)
    {
        return (
            StatusCode::FORBIDDEN,
            "API key is revoked, expired, or lacks mcp scope",
        )
            .into_response();
    }
    let now = Utc::now().to_rfc3339();
    let _ = st.store.touch_api_key(&key.id).await;
    let _ = st
        .store
        .log_audit(&key.id, "remote.check", None, None)
        .await;
    Json(json!({"ok":true,"keyId":key.id,"checkedAt":now,"serverVersion":env!("CARGO_PKG_VERSION"),"protocolVersion":PROTOCOL_VERSION,"schemaVersion":crate::usage::SCHEMA})).into_response()
}

pub async fn create_key(home: &std::path::Path, id: String, name: String) -> anyhow::Result<()> {
    let cfg = cli::load_or_empty(home);
    let store = Store::open(&config::resolve_db(home, &cfg.db_path)).await?;
    let key = auth::generate_key(id, [auth::Scope::Mcp, auth::Scope::UsageRead])?;
    store.save_api_key(&key, &name).await?;
    println!("{}", key.plaintext);
    eprintln!("Save this key now; it cannot be shown again.");
    Ok(())
}

async fn authenticate(
    State(state): State<HostedState>,
    mut req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    if !req.uri().path().starts_with("/mcp") {
        return next.run(req).await;
    }
    let Some(value) = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return (StatusCode::UNAUTHORIZED, "missing bearer token").into_response();
    };
    let Some(token) = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
    else {
        return (StatusCode::UNAUTHORIZED, "expected bearer token").into_response();
    };
    let Some(key_id) = token
        .strip_prefix("fk_live_")
        .and_then(|s| s.split('_').next())
    else {
        return (StatusCode::UNAUTHORIZED, "invalid API key").into_response();
    };
    let Ok(Some(key)) = state
        .store
        .find_api_key_by_hash(&auth::hash_key(token))
        .await
    else {
        return (StatusCode::UNAUTHORIZED, "invalid API key").into_response();
    };
    if key.id != key_id
        || key.revoked
        || key
            .expires_at
            .as_deref()
            .and_then(|v| v.parse::<chrono::DateTime<Utc>>().ok())
            .is_some_and(|expires| expires <= Utc::now())
        || !auth::has_scope(&key.scopes, auth::Scope::Mcp)
    {
        return (
            StatusCode::FORBIDDEN,
            "API key is revoked or lacks mcp scope",
        )
            .into_response();
    }
    let request_id = format!(
        "{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    req.headers_mut().insert(
        "x-fetchira-request-id",
        axum::http::HeaderValue::from_str(&request_id).unwrap(),
    );
    let quota = match state
        .store
        .reserve_request(
            &request_id,
            &key.id,
            key.rpm,
            key.daily_limit,
            key.monthly_limit,
        )
        .await
    {
        Ok(Some(q)) => q,
        Ok(None) => return rate_limited("quota exceeded", 60, None),
        Err(_) => {
            return (StatusCode::SERVICE_UNAVAILABLE, "quota store unavailable").into_response()
        }
    };
    let global_permit = match tokio::time::timeout(
        std::time::Duration::from_secs(1),
        state.active.clone().acquire_owned(),
    )
    .await
    {
        Ok(Ok(p)) => p,
        _ => {
            let _ = state.store.cancel_request(&request_id, &key.id).await;
            return rate_limited("server concurrency limit exceeded", 1, Some(quota));
        }
    };
    let key_sem = {
        let mut active = state.key_active.lock().await;
        active
            .entry(key.id.clone())
            .or_insert_with(|| {
                Arc::new(tokio::sync::Semaphore::new(
                    key.concurrency_limit.max(1) as usize
                ))
            })
            .clone()
    };
    let key_permit = match key_sem.try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            drop(global_permit);
            let _ = state.store.cancel_request(&request_id, &key.id).await;
            return rate_limited("API key concurrency limit exceeded", 1, Some(quota));
        }
    };
    let _ = state.store.touch_api_key(&key.id).await;
    let started = Instant::now();
    let response = crate::usage::HOSTED_REQUEST_ID
        .scope(request_id.clone(), next.run(req))
        .await;
    drop(key_permit);
    drop(global_permit);
    let _ = state
        .store
        .log_request(
            &request_id,
            &key.id,
            response.status().as_u16() as i64,
            started.elapsed().as_millis() as i64,
        )
        .await;
    with_rate_limit_headers(response, quota)
}

fn rate_limited(
    message: &'static str,
    retry: u64,
    quota: Option<crate::usage::QuotaReservation>,
) -> Response {
    let mut response = (StatusCode::TOO_MANY_REQUESTS, message).into_response();
    response.headers_mut().insert(
        "retry-after",
        axum::http::HeaderValue::from_str(&retry.to_string()).unwrap(),
    );
    response.headers_mut().insert(
        "x-ratelimit-remaining",
        axum::http::HeaderValue::from_static("0"),
    );
    if let Some(q) = quota {
        response = with_rate_limit_headers(response, q);
    }
    response
}

fn with_rate_limit_headers(mut response: Response, q: crate::usage::QuotaReservation) -> Response {
    for (name, value) in [
        ("x-ratelimit-limit", q.limit),
        ("x-ratelimit-remaining", q.remaining),
        ("x-ratelimit-reset", q.reset_at),
    ] {
        response.headers_mut().insert(
            axum::http::HeaderName::from_static(name),
            axum::http::HeaderValue::from_str(&value.to_string()).unwrap(),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth;
    use crate::router::Router;
    use crate::usage::Store;
    use axum::http::StatusCode;
    use axum::middleware;
    use axum::routing::get;
    use chrono::{Duration as ChronoDuration, FixedOffset};
    use serde_json::json;
    use std::sync::Arc;

    async fn fresh_store(name: &str) -> Store {
        let path = std::env::temp_dir().join(format!(
            "fetchira_hosted_{name}_{}_{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        Store::open(path.to_str().expect("temp path"))
            .await
            .expect("open store")
    }

    async fn test_app(store: Store, key: &auth::ApiKey, concurrency_limit: i64) -> axum::Router {
        store
            .save_api_key_with_limits(key, "test", 10, 0, 0, concurrency_limit, None)
            .await
            .expect("save key");
        test_app_with_state(store)
    }

    fn test_app_with_state(store: Store) -> axum::Router {
        let state = HostedState {
            router: Arc::new(tokio::sync::RwLock::new(Arc::new(Router::from_parts(
                vec![],
                store.clone(),
            )))),
            store,
            active: Arc::new(tokio::sync::Semaphore::new(64)),
            key_active: Default::default(),
            updates: Default::default(),
            home: std::env::temp_dir(),
            db_path: String::new(),
        };
        axum::Router::new()
            .route(
                "/mcp/slow",
                get(|| async {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    Json(json!({"ok": true}))
                }),
            )
            .route("/auth/check", get(auth_check))
            .route("/remote/check", get(remote_check))
            .route("/usage", get(key_usage))
            .layer(middleware::from_fn_with_state(state.clone(), authenticate))
            .with_state(state)
    }

    #[test]
    fn protocol_is_stable() {
        assert_eq!(PROTOCOL_VERSION, "1");
    }

    #[tokio::test]
    async fn key_concurrency_rejection_does_not_consume_quota() {
        let store = fresh_store("key_concurrency").await;
        let key = auth::generate_key(
            "key-concurrency",
            [auth::Scope::Mcp, auth::Scope::UsageRead],
        )
        .expect("key");
        let app = test_app(store.clone(), &key, 1).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test app");
        });
        let client = reqwest::Client::new();
        let url = format!("http://{addr}/mcp/slow");
        let auth = format!("Bearer {}", key.plaintext);

        let first = client
            .get(&url)
            .header("authorization", auth.clone())
            .send();
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        let second = client.get(&url).header("authorization", auth).send();
        let (r1, r2) = tokio::join!(first, second);
        let r1 = r1.expect("first response");
        let r2 = r2.expect("second response");
        server.abort();

        assert_eq!(r1.status(), StatusCode::OK);
        assert_eq!(r2.status(), StatusCode::TOO_MANY_REQUESTS);

        let rows = store
            .recent_requests(Some(&key.id), 10)
            .await
            .expect("recent requests");
        assert_eq!(
            rows.len(),
            1,
            "429 concurrency rejection must not spend quota"
        );
        assert_eq!(rows[0]["status"], 200);
    }

    #[tokio::test]
    async fn mcp_auth_rejects_expired_key_even_with_offset_timestamp() {
        let store = fresh_store("expired_offset").await;
        let key = auth::generate_key("expired-offset", [auth::Scope::Mcp]).expect("key");
        let expired = (Utc::now() - ChronoDuration::minutes(30))
            .with_timezone(&FixedOffset::east_opt(2 * 3600).expect("offset"))
            .to_rfc3339();
        store
            .save_api_key_with_limits(&key, "expired", 10, 0, 0, 1, Some(&expired))
            .await
            .expect("save key");
        let app = test_app_with_state(store);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test app");
        });
        let client = reqwest::Client::new();
        let url = format!("http://{addr}/mcp/slow");
        let response = client
            .get(&url)
            .header("authorization", format!("Bearer {}", key.plaintext))
            .send()
            .await
            .expect("response");
        server.abort();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn scope_matrix_matches_endpoints() {
        let store = fresh_store("scope_matrix").await;
        let mcp_key = auth::generate_key("mcp-only", [auth::Scope::Mcp]).expect("mcp key");
        let usage_key =
            auth::generate_key("usage-only", [auth::Scope::UsageRead]).expect("usage key");
        store
            .save_api_key_with_limits(&mcp_key, "mcp", 10, 0, 0, 1, None)
            .await
            .expect("save mcp key");
        store
            .save_api_key_with_limits(&usage_key, "usage", 10, 0, 0, 1, None)
            .await
            .expect("save usage key");
        let app = test_app_with_state(store);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test app");
        });
        let client = reqwest::Client::new();
        let auth_url = format!("http://{addr}/auth/check");
        let remote_url = format!("http://{addr}/remote/check");
        let usage_url = format!("http://{addr}/usage");

        let auth_ok = client
            .get(&auth_url)
            .header("authorization", format!("Bearer {}", mcp_key.plaintext))
            .send()
            .await
            .expect("auth ok");
        let remote_ok = client
            .get(&remote_url)
            .header("authorization", format!("Bearer {}", mcp_key.plaintext))
            .send()
            .await
            .expect("remote ok");
        let usage_forbidden = client
            .get(&usage_url)
            .header("authorization", format!("Bearer {}", mcp_key.plaintext))
            .send()
            .await
            .expect("usage forbidden");
        let auth_forbidden = client
            .get(&auth_url)
            .header("authorization", format!("Bearer {}", usage_key.plaintext))
            .send()
            .await
            .expect("auth forbidden");
        let remote_forbidden = client
            .get(&remote_url)
            .header("authorization", format!("Bearer {}", usage_key.plaintext))
            .send()
            .await
            .expect("remote forbidden");
        let usage_ok = client
            .get(&usage_url)
            .header("authorization", format!("Bearer {}", usage_key.plaintext))
            .send()
            .await
            .expect("usage ok");
        server.abort();

        assert_eq!(auth_ok.status(), StatusCode::OK);
        assert_eq!(remote_ok.status(), StatusCode::OK);
        assert_eq!(usage_forbidden.status(), StatusCode::FORBIDDEN);
        assert_eq!(auth_forbidden.status(), StatusCode::FORBIDDEN);
        assert_eq!(remote_forbidden.status(), StatusCode::FORBIDDEN);
        assert_eq!(usage_ok.status(), StatusCode::OK);
    }
}
