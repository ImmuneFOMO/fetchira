use fetchira::config::{Account, Config, Priority, Reset};
use fetchira::providers::{Capability, Input, LiveBalance, Provider, ProviderKind};
use fetchira::router::{compact_usage, provider_sheet, Bucket, Conn, Router};
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
        pinned: true,
        dr_pinned: true,
        proxy: None,
        balance_conn: None,
    }
}

fn unpinned(kind: ProviderKind, base: &str, label: &str) -> Bucket {
    Bucket {
        pinned: false,
        ..bucket(kind, base, label)
    }
}

fn live(remaining: i64) -> LiveBalance {
    LiveBalance {
        remaining,
        total: 1000,
        usd: None,
        reset_at: None,
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
    store.record("exa", "exa-1", &period, 100).await.unwrap();
    store
        .record("tavily", "tavily-1", &period, 20)
        .await
        .unwrap();
    store
        .record("serper", "serper-1", &period, 80)
        .await
        .unwrap();

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
    let exa = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "37")
                .set_body_string("slow down"),
        )
        .expect(1)
        .mount(&exa)
        .await;
    let tav = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tavily_body()))
        .expect(2)
        .mount(&tav)
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
        out.text.contains("TAVILY_HIT"),
        "expected failover to tavily, got: {}",
        out.text
    );
    let usage = probe.usage_for("exa-1", &period).await.unwrap();
    assert_eq!(usage.used, 0, "failed reservation must be refunded");
    assert!(!usage.exhausted, "temporary 429 must not exhaust the month");
    let wait = probe
        .cooldown_remaining("exa-1")
        .await
        .unwrap()
        .expect("Retry-After cooldown");
    assert!(
        (35_000..=37_000).contains(&wait.as_millis()),
        "got {wait:?}"
    );

    let router = Router::from_parts(make_buckets(), probe);
    let out = router
        .call(Capability::Search, &query("again"), None)
        .await
        .unwrap();
    assert!(out.text.contains("TAVILY_HIT"));
}

#[tokio::test]
async fn legacy_firecrawl_429_recovers_once_from_live_balance() {
    let firecrawl = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/team/credit-usage"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "success": true,
                    "data": {"remainingCredits": 1515, "planCredits": 3000}
                }))
                .set_delay(std::time::Duration::from_millis(100)),
        )
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
    let recovery_router = router.clone();
    let first = tokio::spawn(async move {
        recovery_router
            .call(Capability::Read, &input_a, Some(ProviderKind::Firecrawl))
            .await
    });
    // During the balance probe only its owner may proceed. Wait for its persisted lease,
    // rather than assuming which of two concurrently scheduled calls reaches it first.
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while probe
            .cooldown_remaining("firecrawl-1")
            .await
            .unwrap()
            .is_none()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let second = router
        .call(Capability::Read, &input_b, Some(ProviderKind::Firecrawl))
        .await;
    assert!(matches!(
        second,
        Err(fetchira::error::Error::RateLimit { .. })
    ));
    assert!(first.await.unwrap().unwrap().text.contains("FIRECRAWL_OK"));
    assert!(probe
        .cooldown_remaining("firecrawl-1")
        .await
        .unwrap()
        .is_none());
    assert!(router
        .call(Capability::Read, &input_b, Some(ProviderKind::Firecrawl))
        .await
        .unwrap()
        .text
        .contains("FIRECRAWL_OK"));
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

#[tokio::test]
async fn payment_required_probe_runs_past_a_spent_estimate() {
    let exa = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(exa_body()))
        .expect(1)
        .mount(&exa)
        .await;
    let store = fresh_store("real_402_past_estimate").await;
    let period = period_key(Reset::Monthly);
    store.save_live("exa-1", &period, &live(500)).await.unwrap();
    store.record("exa", "exa-1", &period, 150).await.unwrap();
    store.mark_exhausted("exa", "exa-1", &period).await.unwrap();
    store.drain_live("exa-1", &period).await.unwrap();
    let probe = Router::from_parts(
        vec![unpinned(ProviderKind::Exa, &exa.uri(), "exa-1")],
        store,
    )
    .call(Capability::Search, &query("probe"), Some(ProviderKind::Exa))
    .await
    .unwrap();
    assert!(probe.text.contains("EXA_HIT"));
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

#[tokio::test]
async fn academic_domains_skip_openalex() {
    let oa = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/works"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "results": [{ "id": "https://openalex.org/W1", "display_name": "OPENALEX_HIT" }] }),
        ))
        .mount(&oa)
        .await;
    let exa = MockServer::start().await;
    mount_search(&exa, exa_body()).await;

    let store = fresh_store("academic_domains").await;
    let probe = store.clone();
    let buckets = vec![
        bucket(ProviderKind::Openalex, &oa.uri(), "openalex-1"),
        bucket(ProviderKind::Exa, &exa.uri(), "exa-1"),
    ];
    let router = Router::from_parts(buckets, store);
    let input = Input {
        topic: Some("academic".into()),
        domains: Some(vec!["nature.com".into()]),
        ..query("crispr")
    };
    let out = router.call(Capability::Search, &input, None).await.unwrap();
    assert!(out.text.contains("EXA_HIT"), "got: {}", out.text);
    assert_eq!(probe.recent_routes(1).await.unwrap()[0].fail_from, None);
    let plain = Input {
        domains: None,
        ..input
    };
    let out = router.call(Capability::Search, &plain, None).await.unwrap();
    assert!(out.text.contains("OPENALEX_HIT"), "got: {}", out.text);
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

#[tokio::test]
async fn live_snapshot_moves_the_routing_ceiling() {
    let tav = MockServer::start().await;
    mount_search(&tav, tavily_body()).await;
    let store = fresh_store("live_ceiling").await;
    let probe = store.clone();
    let period = period_key(Reset::Monthly);
    store
        .record("tavily", "tavily-1", &period, 100)
        .await
        .unwrap();
    store
        .save_live("tavily-1", &period, &live(50))
        .await
        .unwrap();
    let router = Router::from_parts(
        vec![unpinned(ProviderKind::Tavily, &tav.uri(), "tavily-1")],
        store,
    );
    let forced = Some(ProviderKind::Tavily);
    assert!(router
        .call(Capability::Search, &query("up"), forced)
        .await
        .is_ok());

    probe
        .save_live("tavily-1", &period, &live(0))
        .await
        .unwrap();
    assert!(router
        .call(Capability::Search, &query("down"), forced)
        .await
        .is_err());
}

#[tokio::test]
async fn pinned_quota_caps_live_snapshot() {
    let tav = MockServer::start().await;
    mount_search(&tav, tavily_body()).await;
    let store = fresh_store("live_pinned").await;
    let period = period_key(Reset::Monthly);
    store
        .record("tavily", "tavily-1", &period, 100)
        .await
        .unwrap();
    store
        .save_live("tavily-1", &period, &live(50))
        .await
        .unwrap();
    let router = Router::from_parts(
        vec![bucket(ProviderKind::Tavily, &tav.uri(), "tavily-1")],
        store,
    );
    assert!(router
        .call(Capability::Search, &query("hi"), Some(ProviderKind::Tavily))
        .await
        .is_err());
}

#[tokio::test]
async fn pinned_cap_skips_the_balance_refresh() {
    let tav = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/usage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "account": {"plan_usage": 0, "plan_limit": 1000}
        })))
        .expect(0)
        .mount(&tav)
        .await;
    let store = fresh_store("live_pinned_spent").await;
    store
        .record("tavily", "tavily-1", &period_key(Reset::Monthly), 100)
        .await
        .unwrap();
    let res = Router::from_parts(
        vec![bucket(ProviderKind::Tavily, &tav.uri(), "tavily-1")],
        store,
    )
    .call(Capability::Search, &query("q"), Some(ProviderKind::Tavily))
    .await;
    assert!(matches!(res, Err(fetchira::Error::ProviderForced(_))));
}

#[tokio::test]
async fn spent_counter_refreshes_live_balance_once() {
    let tav = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/usage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "account": {"plan_usage": 100, "plan_limit": 1000}
        })))
        .expect(1)
        .mount(&tav)
        .await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tavily_body()))
        .expect(2)
        .mount(&tav)
        .await;
    let store = fresh_store("live_refresh").await;
    let probe = store.clone();
    let period = period_key(Reset::Monthly);
    store
        .record("tavily", "tavily-1", &period, 100)
        .await
        .unwrap();
    let router = Router::from_parts(
        vec![unpinned(ProviderKind::Tavily, &tav.uri(), "tavily-1")],
        store,
    );
    for q in ["first", "second"] {
        let out = router
            .call(Capability::Search, &query(q), Some(ProviderKind::Tavily))
            .await
            .unwrap();
        assert!(out.text.contains("TAVILY_HIT"));
    }
    let snap = probe.load_live("tavily-1").await.unwrap().unwrap();
    assert_eq!((snap.used_at_fetch, snap.remaining), (100, 900));
}

#[tokio::test]
async fn sibling_with_room_skips_a_spent_balance_reread() {
    let spent = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/usage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "account": {"plan_usage": 0, "plan_limit": 1000}
        })))
        .expect(0)
        .mount(&spent)
        .await;
    let tav = MockServer::start().await;
    mount_search(&tav, tavily_body()).await;
    let store = fresh_store("sibling_room").await;
    store
        .record("tavily", "tavily-1", &period_key(Reset::Monthly), 100)
        .await
        .unwrap();
    let router = Router::from_parts(
        vec![
            unpinned(ProviderKind::Tavily, &spent.uri(), "tavily-1"),
            unpinned(ProviderKind::Tavily, &tav.uri(), "tavily-2"),
        ],
        store,
    );
    let out = router
        .call(Capability::Search, &query("q"), Some(ProviderKind::Tavily))
        .await
        .unwrap();
    assert!(out.text.contains("TAVILY_HIT"));
}

#[tokio::test]
async fn failed_balance_reread_hides_the_usd() {
    let tav = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/usage"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&tav)
        .await;
    let store = fresh_store("live_miss_view").await;
    let period = period_key(Reset::Monthly);
    let reading = LiveBalance {
        usd: Some(10.0),
        ..live(900)
    };
    store
        .save_live("tavily-1", &period, &reading)
        .await
        .unwrap();
    store
        .record("tavily", "tavily-1", &period, 10)
        .await
        .unwrap();
    let router = Router::from_parts(
        vec![unpinned(ProviderKind::Tavily, &tav.uri(), "tavily-1")],
        store,
    );
    router.usage_snapshot().await.unwrap();
    let views = router.usage_snapshot_cached().await.unwrap();
    assert_eq!(
        (views[0].remaining, views[0].quota, views[0].usd),
        (890, 1000, None)
    );
}

#[tokio::test]
async fn exhausted_account_without_reading_shows_spent() {
    let store = fresh_store("exhausted_view").await;
    let period = period_key(Reset::Once);
    store
        .record("octen", "octen-1", &period, 4200)
        .await
        .unwrap();
    store
        .mark_exhausted("octen", "octen-1", &period)
        .await
        .unwrap();
    let b = Bucket {
        quota: 5000,
        reset: Reset::Once,
        ..unpinned(ProviderKind::Octen, "http://127.0.0.1:9", "octen-1")
    };
    let views = Router::from_parts(vec![b], store)
        .usage_snapshot_cached()
        .await
        .unwrap();
    assert_eq!((views[0].quota - views[0].used, views[0].remaining), (0, 0));
}

#[tokio::test]
async fn spent_estimate_without_balance_shows_exhausted() {
    let store = fresh_store("spent_estimate_view").await;
    store
        .record("octen", "octen-1", &period_key(Reset::Once), 5000)
        .await
        .unwrap();
    let b = Bucket {
        quota: 5000,
        reset: Reset::Once,
        ..unpinned(ProviderKind::Octen, "http://127.0.0.1:9", "octen-1")
    };
    let views = Router::from_parts(vec![b], store)
        .usage_snapshot_cached()
        .await
        .unwrap();
    assert!(views[0].exhausted);
}

#[tokio::test]
async fn exhausted_account_with_positive_reading_shows_spent() {
    let store = fresh_store("exhausted_reading_view").await;
    let period = period_key(Reset::Monthly);
    store
        .record("tavily", "tavily-1", &period, 10)
        .await
        .unwrap();
    store
        .mark_exhausted("tavily", "tavily-1", &period)
        .await
        .unwrap();
    store
        .save_live("tavily-1", &period, &live(900))
        .await
        .unwrap();
    let b = unpinned(ProviderKind::Tavily, "http://127.0.0.1:9", "tavily-1");
    let views = Router::from_parts(vec![b], store)
        .usage_snapshot_cached()
        .await
        .unwrap();
    assert_eq!((views[0].exhausted, views[0].remaining), (true, 0));
}

async fn stale_reading(
    name: &str,
    remaining: i64,
    used: i64,
    pinned: bool,
    usage: ResponseTemplate,
) -> (MockServer, Router) {
    let tav = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/usage"))
        .respond_with(usage)
        .expect(1)
        .mount(&tav)
        .await;
    let db = temp_db(name);
    let _ = std::fs::remove_file(&db);
    let store = Store::open(db.to_str().expect("temp path")).await.unwrap();
    let period = period_key(Reset::Monthly);
    store
        .save_live("tavily-1", &period, &live(remaining))
        .await
        .unwrap();
    store
        .record("tavily", "tavily-1", &period, used)
        .await
        .unwrap();
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&db))
            .await
            .unwrap();
    sqlx::query("UPDATE live_quota SET fetched_at = 0")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let router = Router::from_parts(
        vec![if pinned {
            bucket(ProviderKind::Tavily, &tav.uri(), "tavily-1")
        } else {
            unpinned(ProviderKind::Tavily, &tav.uri(), "tavily-1")
        }],
        store,
    );
    (tav, router)
}

#[tokio::test]
async fn unreadable_spent_reading_keeps_routing() {
    let (tav, router) =
        stale_reading("live_miss_route", 2, 2, false, ResponseTemplate::new(500)).await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tavily_body()))
        .expect(2)
        .mount(&tav)
        .await;
    for q in ["first", "second"] {
        assert!(router
            .call(Capability::Search, &query(q), Some(ProviderKind::Tavily))
            .await
            .is_ok());
    }
    assert!(!router.usage_snapshot_cached().await.unwrap()[0].exhausted);
}

#[tokio::test]
async fn unreadable_reported_zero_stays_spent() {
    let (tav, router) =
        stale_reading("live_miss_zero", 0, 0, false, ResponseTemplate::new(500)).await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tavily_body()))
        .expect(0)
        .mount(&tav)
        .await;
    assert!(router
        .call(Capability::Search, &query("q"), Some(ProviderKind::Tavily))
        .await
        .is_err());
}

#[tokio::test]
async fn unreadable_spent_reading_keeps_a_pinned_account_under_its_cap() {
    let unread = ResponseTemplate::new(500).set_delay(std::time::Duration::from_millis(300));
    let (tav, router) = stale_reading("live_miss_pinned", 99, 99, true, unread).await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tavily_body()))
        .expect(1)
        .mount(&tav)
        .await;
    let q = query("q");
    let calls = (0..3).map(|_| router.call(Capability::Search, &q, Some(ProviderKind::Tavily)));
    let res = futures_util::future::join_all(calls).await;
    assert_eq!(res.iter().filter(|r| r.is_ok()).count(), 1);
}

#[tokio::test]
async fn unreadable_balance_routes_past_a_spent_estimate() {
    let tav = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/usage"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&tav)
        .await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tavily_body()))
        .expect(2)
        .mount(&tav)
        .await;
    let store = fresh_store("live_miss_estimate").await;
    store
        .record("tavily", "tavily-1", &period_key(Reset::Monthly), 100)
        .await
        .unwrap();
    let router = Router::from_parts(
        vec![unpinned(ProviderKind::Tavily, &tav.uri(), "tavily-1")],
        store,
    );
    for q in ["first", "second"] {
        assert!(router
            .call(Capability::Search, &query(q), Some(ProviderKind::Tavily))
            .await
            .is_ok());
    }
}

#[tokio::test]
async fn balance_reread_in_flight_holds_parallel_callers() {
    let spent = ResponseTemplate::new(200)
        .set_body_json(json!({"account": {"plan_usage": 1000, "plan_limit": 1000}}))
        .set_delay(std::time::Duration::from_millis(800));
    let (tav, router) = stale_reading("live_in_flight", 0, 0, false, spent).await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tavily_body()))
        .expect(0)
        .mount(&tav)
        .await;
    let q = query("q");
    let calls = (0..5).map(|_| router.call(Capability::Search, &q, Some(ProviderKind::Tavily)));
    let res = futures_util::future::join_all(calls).await;
    assert!(res.iter().all(|r| r.is_err()));
}

#[tokio::test]
async fn parallel_callers_follow_a_positive_reread_in_flight() {
    let fresh = ResponseTemplate::new(200)
        .set_body_json(json!({"account": {"plan_usage": 0, "plan_limit": 1000}}))
        .set_delay(std::time::Duration::from_millis(800));
    let (tav, router) = stale_reading("live_in_flight_ok", 0, 0, false, fresh).await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tavily_body()))
        .expect(5)
        .mount(&tav)
        .await;
    let q = query("q");
    let calls = (0..5).map(|_| router.call(Capability::Search, &q, Some(ProviderKind::Tavily)));
    let res = futures_util::future::join_all(calls).await;
    assert!(res.iter().all(|r| r.is_ok()));
}

#[tokio::test]
async fn a_cancelled_reread_frees_the_account() {
    let slow = ResponseTemplate::new(200)
        .set_body_json(json!({"account": {"plan_usage": 0, "plan_limit": 1000}}))
        .set_delay(std::time::Duration::from_secs(5));
    let (tav, router) = stale_reading("live_cancelled", 0, 0, false, slow).await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tavily_body()))
        .expect(1)
        .mount(&tav)
        .await;
    let q = query("q");
    let first = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        router.call(Capability::Search, &q, Some(ProviderKind::Tavily)),
    )
    .await;
    assert!(first.is_err());
    Mock::given(method("GET"))
        .and(path("/usage"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"account": {"plan_usage": 0, "plan_limit": 1000}})),
        )
        .with_priority(1)
        .expect(1)
        .mount(&tav)
        .await;
    let second = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        router.call(Capability::Search, &q, Some(ProviderKind::Tavily)),
    )
    .await;
    assert!(second.is_ok_and(|r| r.is_ok()));
}

#[tokio::test]
async fn slow_research_reread_still_holds_waiters_at_zero() {
    let tf = MockServer::start().await;
    let slow = |body| {
        ResponseTemplate::new(200)
            .set_body_json(body)
            .set_delay(std::time::Duration::from_secs(9))
    };
    Mock::given(method("GET"))
        .and(path("/usage"))
        .respond_with(slow(json!({"total": 0})))
        .mount(&tf)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/research-run"))
        .respond_with(slow(json!({"pagination": {"total": 5}})))
        .mount(&tf)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/automation/run-research-async"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&tf)
        .await;
    let db = temp_db("slow_dr_reread");
    let _ = std::fs::remove_file(&db);
    let store = Store::open(db.to_str().expect("temp path")).await.unwrap();
    let period = period_key(Reset::Monthly);
    store.save_live("tf-1#dr", &period, &live(2)).await.unwrap();
    store
        .record("tinyfish", "tf-1#dr", &period, 2)
        .await
        .unwrap();
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&db))
            .await
            .unwrap();
    sqlx::query("UPDATE live_quota SET fetched_at = 0")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let router = Router::from_parts(
        vec![bucket(ProviderKind::Tinyfish, &tf.uri(), "tf-1")],
        store,
    );
    let q = query("q");
    let calls =
        (0..3).map(|_| router.call(Capability::DeepResearch, &q, Some(ProviderKind::Tinyfish)));
    let res = futures_util::future::join_all(calls).await;
    assert!(res.iter().all(|r| r.is_err()));
}

#[tokio::test]
async fn rate_limited_providers_are_unmetered() {
    let account = |provider, label: &str, quota| Account {
        provider,
        label: label.into(),
        api_key: None,
        proxy: None,
        quota,
        reset: None,
        dr_quota: None,
        dr_reset: None,
    };
    let cfg = Config {
        accounts: vec![
            account(ProviderKind::Gdelt, "gdelt-1", None),
            account(ProviderKind::Jina, "jina-1", None),
            account(ProviderKind::Gdelt, "gdelt-2", Some(5)),
        ],
        ..Default::default()
    };
    let router = Router::build(cfg, fresh_store("unmetered").await)
        .await
        .unwrap();
    let views = router.usage_snapshot().await.unwrap();
    let unmetered: Vec<_> = views
        .iter()
        .map(|v| (v.label.as_str(), v.unmetered))
        .collect();
    assert_eq!(
        unmetered,
        [("gdelt-1", true), ("jina-1", true), ("gdelt-2", false)]
    );
    assert!(compact_usage(&views).contains("gdelt ∞ (rate-limited)"));
}

#[tokio::test]
async fn unresolved_key_reference_skips_a_keyless_provider() {
    let cfg = Config {
        accounts: vec![Account {
            provider: ProviderKind::Jina,
            label: "jina-1".into(),
            api_key: Some("env:FETCHIRA_TEST_UNSET_JINA_KEY".into()),
            proxy: None,
            quota: None,
            reset: None,
            dr_quota: None,
            dr_reset: None,
        }],
        ..Default::default()
    };
    let router = Router::build(cfg, fresh_store("unresolved_key").await)
        .await
        .unwrap();
    assert!(router.usage_snapshot_cached().await.unwrap().is_empty());
}

#[tokio::test]
async fn tinyfish_reads_and_research_come_live() {
    let tf = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/usage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"total": 4})))
        .mount(&tf)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/research-run"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data": [], "pagination": {"total": 1}})),
        )
        .mount(&tf)
        .await;
    let store = fresh_store("tinyfish_live").await;
    let probe = store.clone();
    let b = Bucket {
        quota: 1000,
        reset: Reset::Daily,
        dr_quota: 5,
        dr_reset: Reset::Daily,
        pinned: false,
        dr_pinned: false,
        ..bucket(ProviderKind::Tinyfish, &tf.uri(), "tinyfish-1")
    };
    let views = Router::from_parts(vec![b], store)
        .usage_snapshot()
        .await
        .unwrap();
    let left: Vec<_> = views
        .iter()
        .map(|v| (v.label.as_str(), v.remaining, v.quota))
        .collect();
    assert_eq!(left, [("tinyfish-1", 996, 1000), ("tinyfish-1#dr", 2, 5)]);
    let dr = probe.load_live("tinyfish-1#dr").await.unwrap().unwrap();
    assert!(dr.reset_at.is_some());
    assert!(compact_usage(&views).contains("tinyfish 996 (search 12000/12000, deep_research 2/5)"));
    assert!(provider_sheet(ProviderKind::Tinyfish, &views).contains("deep_research 2/5"));
}

#[tokio::test]
async fn tinyfish_spent_reads_leave_search_open() {
    let tf = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{"title": "TINYFISH_HIT", "url": "https://t.example", "snippet": "s"}]
        })))
        .expect(1)
        .mount(&tf)
        .await;
    let store = fresh_store("tinyfish_search_pool").await;
    store
        .save_live("tinyfish-1", &period_key(Reset::Daily), &live(0))
        .await
        .unwrap();
    let b = Bucket {
        quota: 1000,
        reset: Reset::Daily,
        pinned: false,
        ..bucket(ProviderKind::Tinyfish, &tf.uri(), "tinyfish-1")
    };
    let router = Router::from_parts(vec![b], store);
    let forced = Some(ProviderKind::Tinyfish);
    let hit = router
        .call(Capability::Search, &query("q"), forced)
        .await
        .unwrap();
    assert!(hit.text.contains("TINYFISH_HIT"));
    let read = Input {
        url: Some("https://example.com".into()),
        ..Default::default()
    };
    assert!(matches!(
        router.call(Capability::Read, &read, forced).await,
        Err(fetchira::Error::ProviderForced(_))
    ));
    let views = router.usage_snapshot_cached().await.unwrap();
    assert!(!views[0].exhausted);
    assert!(compact_usage(&views).contains("tinyfish 0 (search 11999/12000"));
}

#[tokio::test]
async fn tinyfish_search_outlasts_the_fetch_allowance() {
    let tf = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{"title": "TINYFISH_HIT", "url": "https://t.example", "snippet": "s"}]
        })))
        .expect(1)
        .mount(&tf)
        .await;
    let store = fresh_store("tinyfish_search_quota").await;
    let probe = store.clone();
    let period = period_key(Reset::Daily);
    store
        .record("tinyfish", "tinyfish-1#search", &period, 1000)
        .await
        .unwrap();
    let b = Bucket {
        quota: 1000,
        reset: Reset::Daily,
        pinned: false,
        ..bucket(ProviderKind::Tinyfish, &tf.uri(), "tinyfish-1")
    };
    let hit = Router::from_parts(vec![b], store)
        .call(
            Capability::Search,
            &query("q"),
            Some(ProviderKind::Tinyfish),
        )
        .await
        .unwrap();
    assert!(hit.text.contains("TINYFISH_HIT"));
    let used = probe.usage_for("tinyfish-1#search", &period).await.unwrap();
    assert_eq!(used.used, 1001);
}

#[tokio::test]
async fn pinned_tinyfish_reads_leave_the_search_pool_whole() {
    let tf = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{"title": "TINYFISH_HIT", "url": "https://t.example", "snippet": "s"}]
        })))
        .expect(1)
        .mount(&tf)
        .await;
    let store = fresh_store("tinyfish_pinned_reads").await;
    store
        .record(
            "tinyfish",
            "tinyfish-1#search",
            &period_key(Reset::Daily),
            600,
        )
        .await
        .unwrap();
    let b = Bucket {
        quota: 500,
        reset: Reset::Daily,
        ..bucket(ProviderKind::Tinyfish, &tf.uri(), "tinyfish-1")
    };
    let hit = Router::from_parts(vec![b], store)
        .call(
            Capability::Search,
            &query("q"),
            Some(ProviderKind::Tinyfish),
        )
        .await
        .unwrap();
    assert!(hit.text.contains("TINYFISH_HIT"));
}

#[tokio::test]
async fn tinyfish_search_recovery_ignores_reads_and_research_readings() {
    let tf = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/usage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"total": 1000})))
        .expect(0)
        .mount(&tf)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/research-run"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data": [], "pagination": {"total": 5}})),
        )
        .expect(0)
        .mount(&tf)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{"title": "TINYFISH_HIT", "url": "https://t.example", "snippet": "s"}]
        })))
        .expect(1)
        .mount(&tf)
        .await;
    let store = fresh_store("tinyfish_search_recover").await;
    store
        .mark_exhausted("tinyfish", "tinyfish-1#search", &period_key(Reset::Daily))
        .await
        .unwrap();
    let b = Bucket {
        quota: 1000,
        reset: Reset::Daily,
        dr_quota: 5,
        dr_reset: Reset::Daily,
        pinned: false,
        dr_pinned: false,
        ..bucket(ProviderKind::Tinyfish, &tf.uri(), "tinyfish-1")
    };
    let hit = Router::from_parts(vec![b], store)
        .call(
            Capability::Search,
            &query("q"),
            Some(ProviderKind::Tinyfish),
        )
        .await
        .unwrap();
    assert!(hit.text.contains("TINYFISH_HIT"));
}

#[tokio::test]
async fn research_reading_keeps_runs_from_the_previous_utc_day() {
    let store = fresh_store("tinyfish_dr_rollover").await;
    let reset_at = Some(chrono::Utc::now().timestamp_millis() + 3_600_000);
    let dr = LiveBalance {
        remaining: 5,
        total: 5,
        usd: None,
        reset_at,
    };
    store
        .save_live("tinyfish-1#dr", "2000-01-01", &dr)
        .await
        .unwrap();
    store
        .record("tinyfish", "tinyfish-1#dr", "2000-01-01", 4)
        .await
        .unwrap();
    let b = Bucket {
        dr_quota: 5,
        dr_reset: Reset::Daily,
        dr_pinned: false,
        ..bucket(ProviderKind::Tinyfish, "http://127.0.0.1:9", "tinyfish-1")
    };
    let views = Router::from_parts(vec![b], store)
        .usage_snapshot_cached()
        .await
        .unwrap();
    let dr = views.iter().find(|v| v.label == "tinyfish-1#dr").unwrap();
    assert_eq!((dr.remaining, dr.quota), (1, 5));
}

#[tokio::test]
async fn tinyfish_research_recovers_from_its_own_reading() {
    let tf = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/usage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"total": 1000})))
        .mount(&tf)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/research-run"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data": [], "pagination": {"total": 0}})),
        )
        .mount(&tf)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/automation/run-research-async"))
        .respond_with(ResponseTemplate::new(400).set_body_string("probe"))
        .expect(1)
        .mount(&tf)
        .await;
    let store = fresh_store("tinyfish_dr_recover").await;
    let probe = store.clone();
    let period = period_key(Reset::Daily);
    store
        .mark_exhausted("tinyfish", "tinyfish-1#dr", &period)
        .await
        .unwrap();
    let b = Bucket {
        quota: 1000,
        reset: Reset::Daily,
        dr_quota: 5,
        dr_reset: Reset::Daily,
        pinned: false,
        dr_pinned: false,
        ..bucket(ProviderKind::Tinyfish, &tf.uri(), "tinyfish-1")
    };
    let res = Router::from_parts(vec![b], store)
        .call(
            Capability::DeepResearch,
            &query("q"),
            Some(ProviderKind::Tinyfish),
        )
        .await;
    assert!(matches!(
        res,
        Err(fetchira::Error::Provider { status: 400, .. })
    ));
    assert!(
        !probe
            .usage_for("tinyfish-1#dr", &period)
            .await
            .unwrap()
            .exhausted
    );
}
