use std::time::Duration;

use serde_json::{json, Value};

use super::{
    check, fmt_hits, niche, s, with_sources, Capability, Hit, Input, LiveBalance, Outcome,
    ProviderKind,
};
use crate::error::{Error, Result};

pub async fn call(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    match cap {
        Capability::Search => search(base, key, client, input).await,
        Capability::Read => contents(base, key, client, input).await,
        Capability::DeepResearch => research(base, key, client, input).await,
        _ => Err(Error::Unsupported("you")),
    }
}

// Research and billing live on api.you.com, not the search index host.
fn api(base: &str) -> String {
    base.replace("ydc-index.io", "api.you.com")
}

async fn search(base: &str, key: &str, client: &reqwest::Client, input: &Input) -> Result<Outcome> {
    let resp = crate::httptrace::send_traced(
        client
            .post(format!("{base}/v1/search"))
            .header("X-API-Key", key)
            .json(&search_body(input)?),
    )
    .await?;
    let v: Value = check("you", resp).await.map_err(bad_key)?.json().await?;
    let hits = v["results"]["web"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|o| {
                    let desc = s(o, "description");
                    Hit {
                        title: s(o, "title"),
                        url: s(o, "url"),
                        snippet: if desc.is_empty() {
                            o["snippets"][0].as_str().unwrap_or_default().to_string()
                        } else {
                            desc
                        },
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(Outcome::new(fmt_hits(&hits), 1))
}

fn search_body(input: &Input) -> Result<Value> {
    let mut b = filters(input);
    b["query"] = json!(input.need_query()?.to_string() + niche::topic_hint(input));
    b["count"] = json!(input.results());
    Ok(b)
}

// Research takes the same filters, under `source_control`.
fn filters(input: &Input) -> Value {
    let mut b = json!({});
    if let Some(f) = input.recency.as_deref().and_then(freshness) {
        b["freshness"] = json!(f);
    }
    // include + exclude together is a 422; the allowlist already keeps excluded domains out.
    if let Some(domains) = &input.domains {
        let (inc, exc) = niche::split_domains(domains);
        if !inc.is_empty() {
            b["include_domains"] = json!(inc);
        } else if !exc.is_empty() {
            b["exclude_domains"] = json!(exc);
        }
    }
    b
}

// A bare date is silently ignored; only a `<from>to<to>` range filters.
fn freshness(recency: &str) -> Option<String> {
    match recency {
        "day" | "week" | "month" | "year" => Some(recency.to_string()),
        iso => niche::recency_date(iso)
            .map(|d| format!("{d}to{}", chrono::Utc::now().format("%Y-%m-%d"))),
    }
}

// A failed page still answers 200, just with empty markdown.
async fn contents(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    input: &Input,
) -> Result<Outcome> {
    let resp = crate::httptrace::send_traced(
        client
            .post(format!("{base}/v1/contents"))
            .header("X-API-Key", key)
            .json(&json!({ "urls": [input.need_url()?], "formats": ["markdown"] })),
    )
    .await?;
    let v: Value = check("you", resp).await.map_err(bad_key)?.json().await?;
    let text = s(&v[0], "markdown");
    if text.is_empty() {
        return Err(Error::BadResponse("you"));
    }
    Ok(Outcome::new(text, 1))
}

// ydc-index.io answers a bad key with a bare 403 Forbidden where api.you.com says 401.
fn bad_key(e: Error) -> Error {
    match e {
        Error::Provider {
            provider,
            status: 403,
            body,
        } if body.contains("Forbidden") => Error::Provider {
            provider,
            status: 401,
            body,
        },
        e => e,
    }
}

// Synchronous; a deep run holds the connection for up to two minutes.
async fn research(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    input: &Input,
) -> Result<Outcome> {
    let (body, cost) = research_body(input)?;
    let resp = crate::httptrace::send_traced(
        client
            .post(format!("{}/v1/research", api(base)))
            .header("X-API-Key", key)
            .timeout(Duration::from_secs(300))
            .json(&body),
    )
    .await?;
    let v: Value = check("you", resp).await?.json().await?;
    Ok(Outcome::new(report(&v["output"])?, cost))
}

fn research_body(input: &Input) -> Result<(Value, i64)> {
    let (effort, cost) = match input.depth.as_deref() {
        Some("deep") => ("deep", 20),
        _ => ("lite", 3),
    };
    let mut b = json!({
        "input": input.need_query()?.to_string() + niche::topic_hint(input),
        "research_effort": effort,
    });
    let sc = filters(input);
    if sc != json!({}) {
        b["source_control"] = sc;
    }
    Ok((b, cost))
}

// `[[n]]` citations index `sources`, 1-based.
fn report(out: &Value) -> Result<String> {
    let text = s(out, "content");
    if text.is_empty() {
        return Err(Error::BadResponse("you"));
    }
    let sources: Vec<String> = out["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|o| s(o, "url"))
        .collect();
    Ok(with_sources(text, &sources))
}

/// Balance in cents; lags real usage by minutes.
pub async fn balance(base: &str, key: &str, client: &reqwest::Client) -> Result<LiveBalance> {
    let resp = client
        .get(format!("{}/v1/billing/account_balance", api(base)))
        .header("X-API-Key", key)
        .send()
        .await?;
    parse_balance(&check("you", resp).await?.json().await?)
}

fn parse_balance(v: &Value) -> Result<LiveBalance> {
    let cents = v["data"]["attributes"]["balance"]
        .as_f64()
        .ok_or(Error::BadResponse("you"))?;
    Ok(LiveBalance {
        remaining: (cents * 2.0) as i64,
        total: ProviderKind::You.default_quota(),
        usd: Some(cents / 100.0),
        reset_at: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn niche_maps_to_native_params() {
        let b = search_body(&Input {
            query: Some("tokio".into()),
            recency: Some("week".into()),
            domains: Some(vec!["github.com".into(), "-reddit.com".into()]),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(b["count"], 5);
        assert_eq!(b["freshness"], "week");
        assert_eq!(b["include_domains"], json!(["github.com"]));
        assert!(b.get("exclude_domains").is_none());

        let b = search_body(&Input {
            query: Some("tokio".into()),
            domains: Some(vec!["-reddit.com".into()]),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(b["exclude_domains"], json!(["reddit.com"]));
        assert!(b.get("include_domains").is_none() && b.get("freshness").is_none());
    }

    #[test]
    fn topic_folds_into_the_query() {
        let b = search_body(&Input {
            query: Some("ecb rates".into()),
            topic: Some("news".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(b["query"], "ecb rates latest news");
    }

    #[test]
    fn research_filters_go_under_source_control() {
        let (b, cost) = research_body(&Input {
            query: Some("crispr".into()),
            recency: Some("week".into()),
            domains: Some(vec!["nature.com".into()]),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(cost, 3);
        assert_eq!(b["research_effort"], "lite");
        assert_eq!(
            b["source_control"],
            json!({"freshness": "week", "include_domains": ["nature.com"]})
        );

        let (plain, _) = research_body(&Input {
            query: Some("crispr".into()),
            ..Default::default()
        })
        .unwrap();
        assert!(plain.get("source_control").is_none());
    }

    #[test]
    fn iso_recency_becomes_a_range() {
        let f = freshness("2025-01-01").unwrap();
        assert!(f.starts_with("2025-01-01to") && f.len() == 22);
        assert_eq!(freshness("bogus"), None);
    }

    #[test]
    fn report_appends_numbered_sources() {
        let text = report(&json!({
            "content": "Rust 1.99 [[1]]",
            "content_type": "text",
            "sources": [{"url": "https://blog.rust-lang.org", "title": "Rust Blog"}]
        }))
        .unwrap();
        assert_eq!(
            text,
            "Rust 1.99 [[1]]\n\nSources:\n1. https://blog.rust-lang.org"
        );
        assert!(report(&json!(null)).is_err());
    }

    #[test]
    fn cents_to_searches() {
        let b = parse_balance(&json!({"data": {"attributes": {"balance": 9996.0}}})).unwrap();
        assert_eq!(b.remaining, 19_992);
        assert_eq!(b.usd, Some(99.96));
        assert!(parse_balance(&json!({"data": {}})).is_err());
    }

    #[tokio::test]
    async fn index_forbidden_is_a_rejected_key() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({"message": "Forbidden"})))
            .mount(&server)
            .await;
        let input = Input {
            query: Some("tokio".into()),
            url: Some("https://example.com".into()),
            ..Default::default()
        };
        for cap in [Capability::Search, Capability::Read] {
            let out = call(&server.uri(), "k", &reqwest::Client::new(), cap, &input).await;
            assert!(matches!(out, Err(Error::Provider { status: 401, .. })));
        }
    }

    #[tokio::test]
    async fn research_outlasts_a_short_client_timeout() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/research"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(500))
                    .set_body_json(json!({"output": {"content": "Report", "sources": []}})),
            )
            .mount(&server)
            .await;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(100))
            .build()
            .unwrap();
        let input = Input {
            query: Some("q".into()),
            ..Default::default()
        };
        let out = call(
            &server.uri(),
            "k",
            &client,
            Capability::DeepResearch,
            &input,
        )
        .await
        .unwrap();
        assert_eq!(out.text, "Report");
    }

    #[tokio::test]
    async fn empty_markdown_is_an_error() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/contents"))
            .and(header("X-API-Key", "k"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"url": "https://httpbin.org/status/404", "markdown": ""}
            ])))
            .mount(&server)
            .await;
        let input = Input {
            url: Some("https://httpbin.org/status/404".into()),
            ..Default::default()
        };
        let out = call(
            &server.uri(),
            "k",
            &reqwest::Client::new(),
            Capability::Read,
            &input,
        )
        .await;
        assert!(matches!(out, Err(Error::BadResponse("you"))));
    }
}
