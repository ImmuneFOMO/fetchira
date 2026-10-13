use serde_json::{json, Value};

use super::{check, fmt_hits, iso_ms, niche, s, Capability, Hit, Input, LiveBalance, Outcome};
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
        Capability::Read => fetch(base, key, client, input).await,
        _ => Err(Error::Unsupported("keenable")),
    }
}

// Keyless calls go to the `/public` twins, which require an app name.
fn request(
    client: &reqwest::Client,
    method: reqwest::Method,
    base: &str,
    endpoint: &str,
    key: &str,
) -> reqwest::RequestBuilder {
    if key.is_empty() {
        client
            .request(method, format!("{base}/v1/{endpoint}/public"))
            .header("X-Keenable-Title", "fetchira")
    } else {
        client
            .request(method, format!("{base}/v1/{endpoint}"))
            .header("X-API-Key", key)
    }
}

async fn search(base: &str, key: &str, client: &reqwest::Client, input: &Input) -> Result<Outcome> {
    let resp = crate::httptrace::send_traced(
        request(client, reqwest::Method::POST, base, "search", key).json(&body(input)?),
    )
    .await?;
    let resp = check("keenable", resp).await?;
    let live = hourly(resp.headers());
    let v: Value = resp.json().await?;
    let hits = v["results"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|o| Hit {
                    title: s(o, "title"),
                    url: s(o, "url"),
                    snippet: s(o, "snippet"),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut out = Outcome::new(fmt_hits(&hits), 1);
    out.live = live;
    Ok(out)
}

// Only keyless replies carry these: a fixed hour per IP, counted apart for search and fetch.
fn hourly(h: &reqwest::header::HeaderMap) -> Option<LiveBalance> {
    let get = |name: &str| h.get(name)?.to_str().ok();
    Some(LiveBalance {
        remaining: get("x-ratelimit-remaining")?.parse().ok()?,
        total: get("x-ratelimit-limit")?.parse().ok()?,
        usd: None,
        // A reading must expire, or a spent one would bench the account for good.
        reset_at: Some(
            get("x-ratelimit-reset")
                .and_then(iso_ms)
                .unwrap_or_else(|| {
                    (chrono::Utc::now() + chrono::TimeDelta::hours(1)).timestamp_millis()
                }),
        ),
    })
}

/// The one include-only domain keenable filters natively; any other domain mix goes into the query.
pub(super) fn site(input: &Input) -> Option<&str> {
    match input.domains.as_deref() {
        Some([d]) if !d.starts_with('-') => Some(d),
        _ => None,
    }
}

// Snippets default to whole-page excerpts, so cap them to SERP size.
fn body(input: &Input) -> Result<Value> {
    let mut q = input.need_query()?.to_string() + niche::topic_hint(input);
    let site = site(input);
    if site.is_none() {
        q.push_str(&niche::sites(input));
    }
    let mut b = json!({ "query": q, "max_results": input.results(), "snippet_max_length": 500 });
    if let Some(d) = site {
        b["site"] = json!(d);
    }
    if let Some(after) = input.recency.as_deref().and_then(niche::recency_date) {
        b["published_after"] = json!(after);
    }
    if let Some(mode) = input.mode.as_deref() {
        if !matches!(mode, "pro" | "realtime") {
            return Err(Error::Provider {
                provider: "keenable",
                status: 400,
                body: format!("unknown mode '{mode}'; valid modes: pro, realtime"),
            });
        }
        b["mode"] = json!(mode);
    }
    Ok(b)
}

// Without `live=true` keenable only serves its indexed copy and rejects unindexed URLs.
async fn fetch(base: &str, key: &str, client: &reqwest::Client, input: &Input) -> Result<Outcome> {
    let resp = crate::httptrace::send_traced(
        request(client, reqwest::Method::GET, base, "fetch", key)
            .query(&[("url", input.need_url()?), ("live", "true")]),
    )
    .await?;
    let resp = check("keenable", resp).await?;
    let live = hourly(resp.headers());
    let v: Value = resp.json().await?;
    let text = s(&v, "content");
    if text.is_empty() {
        return Err(Error::BadResponse("keenable"));
    }
    let mut out = Outcome::new(text, 1);
    out.live = live;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_request() {
        let b = body(&Input {
            query: Some("crispr".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(b["query"], "crispr");
        assert_eq!(b["max_results"], 5);
        assert!(b.get("site").is_none() && b.get("published_after").is_none());
    }

    #[test]
    fn mode_is_checked() {
        let input = Input {
            query: Some("crispr".into()),
            mode: Some("realtime".into()),
            ..Default::default()
        };
        assert_eq!(body(&input).unwrap()["mode"], "realtime");
        assert!(body(&Input {
            mode: Some("fast".into()),
            ..input
        })
        .is_err());
    }

    #[test]
    fn single_site_is_native_other_domains_fold_into_query() {
        let one = Input {
            query: Some("crispr".into()),
            domains: Some(vec!["nature.com".into()]),
            recency: Some("2024-03-01".into()),
            ..Default::default()
        };
        let b = body(&one).unwrap();
        assert_eq!(b["site"], "nature.com");
        assert_eq!(b["query"], "crispr");
        assert_eq!(b["published_after"], "2024-03-01");

        let mixed = Input {
            query: Some("crispr".into()),
            domains: Some(vec!["nature.com".into(), "-reddit.com".into()]),
            ..Default::default()
        };
        assert_eq!(site(&mixed), None);
        let b = body(&mixed).unwrap();
        assert!(b.get("site").is_none());
        assert_eq!(b["query"], "crispr site:nature.com -site:reddit.com");

        let two = Input {
            domains: Some(vec!["nature.com".into(), "cell.com".into()]),
            ..mixed
        };
        assert_eq!(
            body(&two).unwrap()["query"],
            "crispr site:nature.com OR site:cell.com"
        );
    }

    #[tokio::test]
    async fn hits_use_the_snippet() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [
                {"title": "t", "url": "https://a.dev", "description": "summary", "snippet": "excerpt"}
            ]})))
            .mount(&server)
            .await;
        let input = Input {
            query: Some("q".into()),
            ..Default::default()
        };
        let out = call(
            &server.uri(),
            "k",
            &reqwest::Client::new(),
            Capability::Search,
            &input,
        )
        .await
        .unwrap();
        assert!(out.text.contains("excerpt") && !out.text.contains("summary"));
    }

    #[tokio::test]
    async fn keyless_uses_public_endpoint() {
        use wiremock::matchers::{header, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/fetch/public"))
            .and(header("X-Keenable-Title", "fetchira"))
            .and(query_param("live", "true"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-ratelimit-limit", "1000")
                    .insert_header("x-ratelimit-remaining", "999")
                    .insert_header("x-ratelimit-reset", "2026-10-08T17:18:43.005Z")
                    .set_body_json(json!({ "content": "# Example" })),
            )
            .mount(&server)
            .await;
        let input = Input {
            url: Some("https://example.com".into()),
            ..Default::default()
        };
        let out = call(
            &server.uri(),
            "",
            &reqwest::Client::new(),
            Capability::Read,
            &input,
        )
        .await
        .unwrap();
        assert_eq!(out.text, "# Example");
        let live = out.live.unwrap();
        assert_eq!(
            (live.remaining, live.total, live.reset_at),
            (999, 1000, Some(1_791_479_923_005))
        );
    }

    #[test]
    fn reading_without_reset_expires_within_the_hour() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("x-ratelimit-limit", "1000".parse().unwrap());
        h.insert("x-ratelimit-remaining", "0".parse().unwrap());
        h.insert("x-ratelimit-reset", "1791479923".parse().unwrap());
        let now = chrono::Utc::now().timestamp_millis();
        let reset = hourly(&h).unwrap().reset_at.unwrap();
        assert!(reset > now && reset <= now + 3_600_000 + 1000);
    }
}
