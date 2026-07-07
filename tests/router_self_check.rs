use fetchira::config::{Priority, Reset};
use fetchira::providers::{Capability, Input, Provider, ProviderKind};
use fetchira::router::{Bucket, Conn, Router};
use fetchira::usage::{period_key, Store};
use serde_json::json;
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
    let path = std::env::temp_dir().join(format!("fetchira_test_{name}.db"));
    let _ = std::fs::remove_file(&path);
    Store::open(path.to_str().expect("temp path"))
        .await
        .expect("open store")
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

// (b) chosen provider returns 429 -> marked exhausted, fails over to next provider.
#[tokio::test]
async fn rate_limit_marks_exhausted_and_fails_over() {
    let tav = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(429).set_body_string("slow down"))
        .mount(&tav)
        .await;
    let exa = MockServer::start().await;
    mount_search(&exa, exa_body()).await;

    let store = fresh_store("b").await;
    let period = period_key(Reset::Monthly);
    store
        .record("tavily", "tavily-1", &period, 20)
        .await
        .unwrap();
    store.record("exa", "exa-1", &period, 80).await.unwrap();
    let probe = store.clone();

    let buckets = vec![
        bucket(ProviderKind::Tavily, &tav.uri(), "tavily-1"),
        bucket(ProviderKind::Exa, &exa.uri(), "exa-1"),
    ];
    let router = Router::from_parts(buckets, store);
    let out = router
        .call(Capability::Search, &query("hi"), None)
        .await
        .unwrap();
    assert!(
        out.text.contains("EXA_HIT"),
        "expected failover to exa, got: {}",
        out.text
    );
    assert!(
        probe
            .usage_for("tavily-1", &period)
            .await
            .unwrap()
            .exhausted
    );
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

// (d) forced provider that is exhausted -> error, no silent switch.
#[tokio::test]
async fn forced_exhausted_errors() {
    let tav = MockServer::start().await;
    mount_search(&tav, tavily_body()).await;
    let exa = MockServer::start().await;
    mount_search(&exa, exa_body()).await;

    let store = fresh_store("d").await;
    let period = period_key(Reset::Monthly);
    store.mark_exhausted("exa", "exa-1", &period).await.unwrap();

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
