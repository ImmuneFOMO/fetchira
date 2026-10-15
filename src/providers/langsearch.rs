use std::time::Duration;

use serde_json::{json, Value};

use super::{check, fmt_hits, niche, s, Capability, Hit, Input, Outcome};
use crate::error::{Error, Result};

pub async fn call(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    if cap != Capability::Search {
        return Err(Error::Unsupported("langsearch"));
    }
    let payload = body(input)?;
    let send = || {
        crate::httptrace::send_traced(
            client
                .post(format!("{base}/v1/web-search"))
                .bearer_auth(key)
                .json(&payload),
        )
    };
    let mut resp = send().await?;
    // One 429 covers rps, tpm and the daily token allowance, so even a repeat only backs off.
    if resp.status() == 429 && !resp.headers().contains_key(reqwest::header::RETRY_AFTER) {
        tokio::time::sleep(Duration::from_secs(2)).await;
        resp = send().await?;
        if resp.status() == 429 {
            let after = crate::error::parse_retry_after(
                resp.headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok()),
            );
            return Err(Error::rate_limit_after(
                format!("langsearch: {}", resp.text().await.unwrap_or_default()),
                Some(after.unwrap_or_default().max(Duration::from_secs(60))),
            ));
        }
    }
    let v: Value = check("langsearch", resp).await?.json().await?;
    // An HTTP 200 can still carry a failure `code` in the body.
    if let Some(code) = v["code"]
        .as_u64()
        .or_else(|| v["code"].as_str()?.parse().ok())
        .filter(|&c| c != 200)
    {
        return Err(Error::Provider {
            provider: "langsearch",
            status: code as u16,
            body: [&v["msg"], &v["message"]]
                .iter()
                .find_map(|m| m.as_str())
                .unwrap_or_default()
                .to_string(),
        });
    }
    let hits = v["data"]["webPages"]["value"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|o| Hit {
                    title: s(o, "name"),
                    url: s(o, "url"),
                    snippet: s(o, "text")
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(Outcome::new(fmt_hits(&hits), 1))
}

// The allowance is metered in returned tokens, and uncapped snippets run 3-7k chars per hit.
fn body(input: &Input) -> Result<Value> {
    let mut b = json!({
        "query": input.need_query()?.to_string() + niche::topic_hint(input),
        "count": input.results(),
        "contents": { "text": { "maxCharacters": 500 } },
    });
    if let Some(f) = input.recency.as_deref().and_then(freshness) {
        b["freshness"] = json!(f);
    }
    if let Some(domains) = &input.domains {
        let (inc, exc) = niche::split_domains(domains);
        if !inc.is_empty() {
            b["includeDomains"] = json!(inc);
        }
        if !exc.is_empty() {
            b["excludeDomains"] = json!(exc);
        }
    }
    Ok(b)
}

// A bare date matches only that UTC day; a `<from>..<to>` range filters from it.
fn freshness(recency: &str) -> Option<String> {
    match recency {
        "day" => Some("oneDay".into()),
        "week" => Some("oneWeek".into()),
        "month" => Some("oneMonth".into()),
        "year" => Some("oneYear".into()),
        iso => niche::recency_date(iso)
            .map(|d| format!("{d}..{}", chrono::Utc::now().format("%Y-%m-%d"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_text_and_maps_niche() {
        let b = body(&Input {
            query: Some("crispr".into()),
            topic: Some("academic".into()),
            recency: Some("week".into()),
            domains: Some(vec!["nature.com".into(), "-reddit.com".into()]),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(b["query"], "crispr (peer-reviewed OR research paper)");
        assert_eq!(b["count"], 5);
        assert_eq!(b["contents"]["text"]["maxCharacters"], 500);
        assert_eq!(b["freshness"], "oneWeek");
        assert_eq!(b["includeDomains"], json!(["nature.com"]));
        assert_eq!(b["excludeDomains"], json!(["reddit.com"]));

        let plain = body(&Input {
            query: Some("q".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(plain["query"], "q");
        assert!(plain.get("freshness").is_none() && plain.get("includeDomains").is_none());
    }

    #[test]
    fn iso_recency_is_a_range() {
        let f = freshness("2026-09-01").unwrap();
        assert!(f.starts_with("2026-09-01.."));
        assert_eq!(f.len(), "2026-09-01..2026-10-07".len());
        assert_eq!(freshness("bogus"), None);
    }

    #[tokio::test]
    async fn body_error_code_is_an_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/web-search"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"code": "403", "log_id": "abc", "message": "Access denied"}),
                ),
            )
            .mount(&server)
            .await;
        let input = Input {
            query: Some("q".into()),
            ..Default::default()
        };
        let err = call(
            &server.uri(),
            "k",
            &reqwest::Client::new(),
            Capability::Search,
            &input,
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(
            err,
            Error::Provider { status: 403, ref body, .. } if body == "Access denied"
        ));
    }

    #[tokio::test]
    async fn repeated_429_backs_off_a_minute() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let spent = json!({"code": "429", "message": "Rate limit or daily allowance exceeded"});
        let burst = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/web-search"))
            .respond_with(ResponseTemplate::new(429).set_body_json(&spent))
            .up_to_n_times(1)
            .mount(&burst)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/web-search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"code": 200})))
            .mount(&burst)
            .await;
        let day = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/web-search"))
            .respond_with(ResponseTemplate::new(429).set_body_json(&spent))
            .expect(2)
            .mount(&day)
            .await;
        let input = Input {
            query: Some("q".into()),
            ..Default::default()
        };
        let client = reqwest::Client::new();
        let (burst_uri, day_uri) = (burst.uri(), day.uri());
        let (burst, day) = tokio::join!(
            call(&burst_uri, "k", &client, Capability::Search, &input),
            call(&day_uri, "k", &client, Capability::Search, &input)
        );
        assert!(burst.is_ok());
        assert!(matches!(
            day,
            Err(Error::RateLimit { retry_after: Some(d), .. }) if d == Duration::from_secs(60)
        ));
    }
}
