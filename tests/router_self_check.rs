use fetchira::config::{Priority, Reset};
use fetchira::providers::{Capability, Input, Provider, ProviderKind};
use fetchira::router::{Bucket, Conn, Router};
use fetchira::usage::{period_key, Store};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn bucket(kind: ProviderKind, base: &str, label: &str) -> Bucket {
    Bucket {
        provider: Provider::with_base(kind, base),
        conn: Conn::Api(reqwest::Client::new()),
        key: "testkey".into(),
        label: label.into(),
        quota: 100,
        reset: Reset::Monthly,
        dr_quota: 100,
        dr_reset: Reset::Monthly,
        proxy: None,
        balance_conn: None,
    }
}

fn query(q: &str) -> Input {
    Input {
        query: Some(q.into()),
        ..Default::default()
    }
}

async fn mount_search(m: &MockServer, body: serde_json::Value) {
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(m)
        .await;
}

async fn fresh_store(name: &str) -> Store {
    let path = temp_db(name);
    let _ = std::fs::remove_file(&path);
    Store::open(path.to_str().expect("temp path"))
        .await
        .expect("open store")
}

fn temp_db(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "fetchira_test_{name}_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ))
}

async fn legacy_exhausted_store(name: &str, label: &str, period: &str) -> Store {
    let path = temp_db(name);
    let _ = std::fs::remove_file(&path);
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true),
    )
    .await
    .expect("open legacy db");
    sqlx::query(
        "CREATE TABLE usage (
            provider TEXT NOT NULL, label TEXT NOT NULL, period TEXT NOT NULL,
            used INTEGER NOT NULL DEFAULT 0, exhausted INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (label, period)
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO usage (provider, label, period, used, exhausted)
         VALUES ('firecrawl', ?, ?, 0, 1)",
    )
    .bind(label)
    .bind(period)
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    Store::open(path.to_str().expect("temp path"))
        .await
        .expect("open store")
}

async fn expire_cooldown(path: &PathBuf, label: &str) {
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(false),
    )
    .await
    .expect("open cooldown db");
    sqlx::query("UPDATE provider_cooldown SET until_ms = 0 WHERE label = ?")
        .bind(label)
        .execute(&pool)
        .await
        .expect("expire cooldown");
    pool.close().await;
}

fn tavily_body() -> serde_json::Value {
    json!({ "results": [{ "title": "TAVILY_HIT", "url": "https://t.example", "content": "snip" }] })
}

fn exa_body() -> serde_json::Value {
    json!({ "results": [{ "title": "EXA_HIT", "url": "https://e.example", "text": "snip" }] })
}

// (a) serper exhausted, tavily 80% / exa 20% remaining -> most-preferred available (tavily) wins.
#[tokio::test]
async fn picks_preferred_available() {
    let tav = MockServer::start().await;
    mount_search(&tav, tavily_body()).await;
    let exa = MockServer::start().await;
    mount_search(&exa, exa_body()).await;

    let store = fresh_store("a").await;
    let period = period_key(Reset::Monthly);
    store
        .record("serper", "serper-1", &period, 100)
        .await
        .unwrap();
    store
        .record("tavily", "tavily-1", &period, 20)
        .await
        .unwrap();
    store.record("exa", "exa-1", &period, 80).await.unwrap();

    let buckets = vec![
        bucket(ProviderKind::Serper, "http://127.0.0.1:9/dead", "serper-1"),
        bucket(ProviderKind::Tavily, &tav.uri(), "tavily-1"),
        bucket(ProviderKind::Exa, &exa.uri(), "exa-1"),
    ];
    let router = Router::from_parts(buckets, store);
    let out = router
        .call(Capability::Search, &query("hi"), None)
        .await
        .unwrap();
    assert!(
        out.text.contains("TAVILY_HIT"),
        "expected tavily, got: {}",
        out.text
    );
}

// (b) 429 is temporary: refund, persist Retry-After, and fail over without poisoning the month.
#[tokio::test]
async fn rate_limit_cools_down_refunds_and_fails_over() {
    let tav = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "37")
                .set_body_string("slow down"),
        )
        .expect(1)
        .mount(&tav)
        .await;
    let exa = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(exa_body()))
        .expect(2)
        .mount(&exa)
        .await;

    let store = fresh_store("b").await;
    let period = period_key(Reset::Monthly);
    let probe = store.clone();

    let make_buckets = || {
        vec![
            bucket(ProviderKind::Tavily, &tav.uri(), "tavily-1"),
            bucket(ProviderKind::Exa, &exa.uri(), "exa-1"),
        ]
    };
    let router = Router::from_parts(make_buckets(), store);
    let out = router
        .call(Capability::Search, &query("hi"), None)
        .await
        .unwrap();
    assert!(
        out.text.contains("EXA_HIT"),
        "expected failover to exa, got: {}",
        out.text
    );
    let usage = probe.usage_for("tavily-1", &period).await.unwrap();
    assert_eq!(usage.used, 0, "failed reservation must be refunded");
    assert!(!usage.exhausted, "temporary 429 must not exhaust the month");
    let wait = probe
        .cooldown_remaining("tavily-1")
        .await
        .unwrap()
        .expect("Retry-After cooldown");
    assert!(
        (35_000..=37_000).contains(&wait.as_millis()),
        "got {wait:?}"
    );

    // A fresh router (the next one-shot CLI process) reads the same cooldown and skips Tavily.
    let router = Router::from_parts(make_buckets(), probe);
    let out = router
        .call(Capability::Search, &query("again"), None)
        .await
        .unwrap();
    assert!(out.text.contains("EXA_HIT"));
}

#[tokio::test]
async fn legacy_firecrawl_429_recovers_once_from_live_balance() {
    let firecrawl = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/team/credit-usage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "data": {"remainingCredits": 1515, "planCredits": 3000}
        })))
        .expect(1)
        .mount(&firecrawl)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/scrape"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {"markdown": "FIRECRAWL_OK", "metadata": {"creditsUsed": 1}}
        })))
        .expect(2)
        .mount(&firecrawl)
        .await;

    let period = period_key(Reset::Monthly);
    let store = legacy_exhausted_store("legacy_positive", "firecrawl-1", &period).await;
    let probe = store.clone();
    let router = Arc::new(Router::from_parts(
        vec![bucket(
            ProviderKind::Firecrawl,
            &firecrawl.uri(),
            "firecrawl-1",
        )],
        store,
    ));
    let input_a = Input {
        url: Some("https://example.com/a".into()),
        ..Default::default()
    };
    let input_b = Input {
        url: Some("https://example.com/b".into()),
        ..Default::default()
    };
    let (a, b) = tokio::join!(
        router.call(Capability::Read, &input_a, Some(ProviderKind::Firecrawl)),
        router.call(Capability::Read, &input_b, Some(ProviderKind::Firecrawl))
    );
    assert!(a.unwrap().text.contains("FIRECRAWL_OK"));
    assert!(b.unwrap().text.contains("FIRECRAWL_OK"));
    let usage = probe.usage_for("firecrawl-1", &period).await.unwrap();
    assert_eq!(usage.used, 2);
    assert!(!usage.exhausted);
}

#[tokio::test]
async fn legacy_429_without_balance_gets_one_successful_probe() {
    let exa = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_millis(100))
                .set_body_json(exa_body()),
        )
        .expect(2)
        .mount(&exa)
        .await;

    let period = period_key(Reset::Monthly);
    let store = legacy_exhausted_store("legacy_no_balance", "exa-1", &period).await;
    let probe = store.clone();
    let router = Arc::new(Router::from_parts(
        vec![bucket(ProviderKind::Exa, &exa.uri(), "exa-1")],
        store,
    ));
    let input_a = query("probe a");
    let input_b = query("probe b");
    let (a, b) = tokio::join!(
        router.call(Capability::Search, &input_a, Some(ProviderKind::Exa)),
        router.call(Capability::Search, &input_b, Some(ProviderKind::Exa))
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let out = router
        .call(
            Capability::Search,
            &query("normal route"),
            Some(ProviderKind::Exa),
        )
        .await
        .unwrap();
    assert!(out.text.contains("EXA_HIT"));
    assert!(!probe.usage_for("exa-1", &period).await.unwrap().exhausted);
    assert!(probe.cooldown_remaining("exa-1").await.unwrap().is_none());
}

#[tokio::test]
async fn legacy_exhaustion_with_zero_live_balance_stays_gated() {
    let firecrawl = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/team/credit-usage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "data": {"remainingCredits": 0, "planCredits": 3000}
        })))
        .expect(1)
        .mount(&firecrawl)
        .await;

    let period = period_key(Reset::Monthly);
    let store = legacy_exhausted_store("legacy_zero", "firecrawl-1", &period).await;
    let probe = store.clone();
    let router = Router::from_parts(
        vec![bucket(
            ProviderKind::Firecrawl,
            &firecrawl.uri(),
            "firecrawl-1",
        )],
        store,
    );
    let result = router
        .call(
            Capability::Read,
            &Input {
                url: Some("https://example.com".into()),
                ..Default::default()
            },
            Some(ProviderKind::Firecrawl),
        )
        .await;
    assert!(matches!(result, Err(fetchira::Error::ProviderForced(_))));
    assert!(
        probe
            .usage_for("firecrawl-1", &period)
            .await
            .unwrap()
            .exhausted
    );
}

#[tokio::test]
async fn payment_required_rechecks_fresh_balance_after_cooldown() {
    let tav = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(402).set_body_string("credits exhausted"))
        .expect(1)
        .mount(&tav)
        .await;
    let db = temp_db("real_402_topup");
    let _ = std::fs::remove_file(&db);
    let store = Store::open(db.to_str().expect("temp path")).await.unwrap();
    let probe = store.clone();
    let router = Router::from_parts(
        vec![bucket(ProviderKind::Tavily, &tav.uri(), "tavily-1")],
        store,
    );
    let first = router
        .call(
            Capability::Search,
            &query("first"),
            Some(ProviderKind::Tavily),
        )
        .await;
    assert!(matches!(first, Err(fetchira::Error::QuotaExceeded(_))));
    let second = router
        .call(
            Capability::Search,
            &query("second"),
            Some(ProviderKind::Tavily),
        )
        .await;
    assert!(matches!(second, Err(fetchira::Error::RateLimit { .. })));

    // Simulate the one-minute probe interval and a user top-up. The recovery fetch bypasses the
    // router's balance cache, clears only the provider-side marker, then performs the request.
    tav.reset().await;
    Mock::given(method("GET"))
        .and(path("/usage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "account": {"plan_usage": 0, "plan_limit": 1000}
        })))
        .expect(1)
        .mount(&tav)
        .await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tavily_body()))
        .expect(1)
        .mount(&tav)
        .await;
    expire_cooldown(&db, "tavily-1").await;
    let recovered = router
        .call(
            Capability::Search,
            &query("after top-up"),
            Some(ProviderKind::Tavily),
        )
        .await
        .unwrap();
    assert!(recovered.text.contains("TAVILY_HIT"));
    let usage = probe
        .usage_for("tavily-1", &period_key(Reset::Monthly))
        .await
        .unwrap();
    assert_eq!(usage.used, 1);
    assert!(!usage.exhausted);
    assert!(probe
        .cooldown_remaining("tavily-1")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn payment_required_without_balance_endpoint_recovers_after_successful_probe() {
    let tav = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(402).set_body_string("credits exhausted"))
        .expect(1)
        .mount(&tav)
        .await;

    let db = temp_db("real_402_probe");
    let _ = std::fs::remove_file(&db);
    let store = Store::open(db.to_str().expect("temp path")).await.unwrap();
    let router = Router::from_parts(
        vec![bucket(ProviderKind::Tavily, &tav.uri(), "tavily-1")],
        store,
    );
    assert!(matches!(
        router
            .call(
                Capability::Search,
                &query("first"),
                Some(ProviderKind::Tavily)
            )
            .await,
        Err(fetchira::Error::QuotaExceeded(_))
    ));

    tav.reset().await;
    Mock::given(method("GET"))
        .and(path("/usage"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&tav)
        .await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tavily_body()))
        .expect(2)
        .mount(&tav)
        .await;
    expire_cooldown(&db, "tavily-1").await;
    let recovered = router
        .call(
            Capability::Search,
            &query("probe"),
            Some(ProviderKind::Tavily),
        )
        .await
        .unwrap();
    assert!(recovered.text.contains("TAVILY_HIT"));
    let next = router
        .call(
            Capability::Search,
            &query("normal route"),
            Some(ProviderKind::Tavily),
        )
        .await
        .unwrap();
    assert!(next.text.contains("TAVILY_HIT"));
}

// (c) usage() reports correct remaining after recorded calls.
#[tokio::test]
async fn usage_reports_remaining() {
    let tav = MockServer::start().await;
    mount_search(&tav, tavily_body()).await;

    let store = fresh_store("c").await;
    let buckets = vec![bucket(ProviderKind::Tavily, &tav.uri(), "tavily-1")];
    let router = Router::from_parts(buckets, store);
    router
        .call(Capability::Search, &query("a"), None)
        .await
        .unwrap();
    router
        .call(Capability::Search, &query("b"), None)
        .await
        .unwrap();

    let views = router.usage_snapshot().await.unwrap();
    let tv = views
        .iter()
        .find(|v| v.label == "tavily-1")
        .expect("tavily row");
    assert_eq!(tv.used, 2);
    assert_eq!(tv.remaining, 98);
    assert_eq!(tv.proxy, "direct");
}

// (e) a custom priority beats the built-in order (and the most-remaining account).
#[tokio::test]
async fn custom_priority_reorders() {
    let tav = MockServer::start().await;
    mount_search(&tav, tavily_body()).await;
    let exa = MockServer::start().await;
    mount_search(&exa, exa_body()).await;

    let store = fresh_store("e").await;
    let period = period_key(Reset::Monthly);
    // tavily has more left than exa — without a custom priority it would win.
    store.record("exa", "exa-1", &period, 50).await.unwrap();

    let buckets = vec![
        bucket(ProviderKind::Tavily, &tav.uri(), "tavily-1"),
        bucket(ProviderKind::Exa, &exa.uri(), "exa-1"),
    ];
    let router = Router::from_parts(buckets, store).with_priority(Priority {
        search: vec![ProviderKind::Exa],
        ..Default::default()
    });
    let out = router
        .call(Capability::Search, &query("hi"), None)
        .await
        .unwrap();
    assert!(
        out.text.contains("EXA_HIT"),
        "expected exa first, got: {}",
        out.text
    );
}

// (f) forcing a provider outside the capability's auto-route order still works (read via serper's
// scrape endpoint, which the built-in read order doesn't route).
#[tokio::test]
async fn forced_off_route_provider_works() {
    let srp = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "text": "SCRAPED_OK" })))
        .mount(&srp)
        .await;

    let store = fresh_store("f").await;
    let buckets = vec![bucket(ProviderKind::Serper, &srp.uri(), "serper-1")];
    let router = Router::from_parts(buckets, store);
    let out = router
        .call(
            Capability::Read,
            &Input {
                url: Some("https://example.com".into()),
                ..Default::default()
            },
            Some(ProviderKind::Serper),
        )
        .await
        .unwrap();
    assert!(out.text.contains("SCRAPED_OK"), "got: {}", out.text);
}

#[tokio::test]
async fn image_only_read_is_a_success() {
    let steel = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/screenshot"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"PNG"))
        .expect(1)
        .mount(&steel)
        .await;

    let store = fresh_store("image_read").await;
    let probe = store.clone();
    let router = Router::from_parts(
        vec![bucket(ProviderKind::Steel, &steel.uri(), "steel-1")],
        store,
    );
    let out = router
        .call(
            Capability::Read,
            &Input {
                url: Some("https://example.com".into()),
                mode: Some("screenshot".into()),
                ..Default::default()
            },
            Some(ProviderKind::Steel),
        )
        .await
        .unwrap();
    assert!(out.text.is_empty());
    assert_eq!(out.image.unwrap().mime, "image/png");
    assert_eq!(
        probe
            .usage_for("steel-1", &period_key(Reset::Monthly))
            .await
            .unwrap()
            .used,
        1
    );
}

// (g) forcing a provider that can't serve the capability -> clear error, no dial-out.
#[tokio::test]
async fn forced_unsupported_errors() {
    let store = fresh_store("g").await;
    let router = Router::from_parts(vec![], store);
    let res = router
        .call(
            Capability::DeepResearch,
            &query("hi"),
            Some(ProviderKind::Serper),
        )
        .await;
    assert!(
        matches!(res, Err(fetchira::Error::Unsupported(_))),
        "got: {res:?}"
    );
}

// (h) a balance endpoint that accepts the connection and never responds must not hang usage.
#[tokio::test]
async fn usage_snapshot_survives_stalled_provider() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let mut open = Vec::new();
        loop {
            if let Ok((sock, _)) = listener.accept().await {
                open.push(sock); // hold the socket open, never respond
            }
        }
    });

    let store = fresh_store("h").await;
    let buckets = vec![bucket(ProviderKind::Serper, &base, "serper-1")];
    let router = Router::from_parts(buckets, store);
    let views = tokio::time::timeout(std::time::Duration::from_secs(15), router.usage_snapshot())
        .await
        .expect("usage_snapshot hung on a stalled provider")
        .unwrap();
    // The live fetch timed out, so the soft counter stands in.
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].remaining, 100);
}

// (d) forced provider that is exhausted -> error, no silent switch.
#[tokio::test]
async fn forced_exhausted_errors() {
    let tav = MockServer::start().await;
    mount_search(&tav, tavily_body()).await;
    let exa = MockServer::start().await;
    mount_search(&exa, exa_body()).await;

    let store = fresh_store("d").await;
    let period = period_key(Reset::Monthly);
    // The configured ceiling is permanent for this period and must not be cleared by the
    // provider-side 402 recovery path.
    store.record("exa", "exa-1", &period, 100).await.unwrap();

    let buckets = vec![
        bucket(ProviderKind::Tavily, &tav.uri(), "tavily-1"),
        bucket(ProviderKind::Exa, &exa.uri(), "exa-1"),
    ];
    let router = Router::from_parts(buckets, store);
    let res = router
        .call(Capability::Search, &query("hi"), Some(ProviderKind::Exa))
        .await;
    assert!(
        matches!(res, Err(fetchira::Error::ProviderForced(_))),
        "got: {res:?}"
    );
}
