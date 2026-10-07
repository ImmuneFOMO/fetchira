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
    let v: Value = check("keenable", resp).await?.json().await?;
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
    Ok(Outcome::new(fmt_hits(&hits), 1))
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
    if let (None, Some(domains)) = (site, &input.domains) {
        let (inc, exc) = niche::split_domains(domains);
        for d in inc {
            q.push_str(&format!(" site:{d}"));
        }
        for d in exc {
            q.push_str(&format!(" -site:{d}"));
        }
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
    let v: Value = check("keenable", resp).await?.json().await?;
    let text = s(&v, "content");
    if text.is_empty() {
        return Err(Error::BadResponse("keenable"));
    }
    Ok(Outcome::new(text, 1))
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
                ResponseTemplate::new(200).set_body_json(json!({ "content": "# Example" })),
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
    }
}
