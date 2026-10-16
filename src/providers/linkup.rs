use std::time::Duration;

use serde_json::{json, Value};

use super::{
    check, fmt_hits, niche, s, with_sources, Capability, Hit, Input, LiveBalance, Outcome,
    ProviderKind,
};
use crate::error::{Error, Result};

const POLL: Duration = Duration::from_secs(5);
const MAX_POLLS: u32 = 180;
const SNIPPET_CHARS: usize = 500;

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
        Capability::DeepResearch => research(base, key, client, input).await,
        _ => Err(Error::Unsupported("linkup")),
    }
}

// 429 also carries the out-of-credit codes; only the plain rate limit is temporary.
fn out_of_credit(e: Error) -> Error {
    match e {
        Error::RateLimit { message, .. }
            if message.contains("INSUFFICIENT_FUNDS_CREDITS")
                || message.contains("EXCEED_BUDGET_LIMIT") =>
        {
            Error::QuotaExceeded(message)
        }
        e => e,
    }
}

async fn search(base: &str, key: &str, client: &reqwest::Client, input: &Input) -> Result<Outcome> {
    let resp = crate::httptrace::send_traced(
        client
            .post(format!("{base}/v1/search"))
            .bearer_auth(key)
            .json(&search_body(input)?),
    )
    .await?;
    let v: Value = match check("linkup", resp).await.map_err(out_of_credit) {
        Err(Error::Provider {
            status: 400, body, ..
        }) if body.contains("SEARCH_QUERY_NO_RESULT") => return Ok(Outcome::new(String::new(), 1)),
        r => r?.json().await?,
    };
    Ok(Outcome::new(fmt_hits(&hits(&v)), 1))
}

fn search_body(input: &Input) -> Result<Value> {
    let mut b = json!({
        "q": input.need_query()?.to_string() + niche::topic_hint(input),
        "depth": "fast",
        "outputType": "searchResults",
        "maxResults": input.results(),
    });
    filters(&mut b, input);
    Ok(b)
}

fn filters(b: &mut Value, input: &Input) {
    if let Some(d) = input.recency.as_deref().and_then(niche::recency_date) {
        b["fromDate"] = json!(d);
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
}

// `content` is multi-line markdown up to several KB, and titles keep raw HTML entities.
fn hits(v: &Value) -> Vec<Hit> {
    v["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|o| o["type"] == "text")
        .map(|o| Hit {
            title: unescape(&s(o, "name")),
            url: s(o, "url"),
            snippet: s(o, "content")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(SNIPPET_CHARS)
                .collect(),
        })
        .collect()
}

fn unescape(t: &str) -> String {
    t.replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

async fn fetch(base: &str, key: &str, client: &reqwest::Client, input: &Input) -> Result<Outcome> {
    let mut body = json!({ "url": input.need_url()? });
    match input.mode.as_deref() {
        None => {}
        Some("js") => body["renderJs"] = json!(true),
        Some(other) => {
            return Err(Error::Provider {
                provider: "linkup",
                status: 400,
                body: format!("unknown mode '{other}'; valid modes: js"),
            })
        }
    }
    let resp = crate::httptrace::send_traced(
        client
            .post(format!("{base}/v1/fetch"))
            .bearer_auth(key)
            .json(&body),
    )
    .await?;
    let v: Value = check("linkup", resp)
        .await
        .map_err(out_of_credit)?
        .json()
        .await?;
    let text = s(&v, "markdown");
    if text.is_empty() {
        return Err(Error::BadResponse("linkup"));
    }
    Ok(Outcome::new(text, 1))
}

// Omitting `reasoningDepth` runs the priciest `L` tier.
fn research_body(input: &Input) -> Result<(Value, i64)> {
    let (depth, cost) = match input.depth.as_deref() {
        Some("deep") => ("M", 100),
        _ => ("S", 50),
    };
    let mut b = json!({
        "q": input.need_query()?.to_string() + niche::topic_hint(input),
        "outputType": "sourcedAnswer",
        "reasoningDepth": depth,
    });
    filters(&mut b, input);
    Ok((b, cost))
}

async fn research(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    input: &Input,
) -> Result<Outcome> {
    let (body, cost) = research_body(input)?;
    let start = crate::httptrace::send_traced(
        client
            .post(format!("{base}/v1/research"))
            .bearer_auth(key)
            .json(&body),
    )
    .await?;
    let v: Value = check("linkup", start)
        .await
        .map_err(out_of_credit)?
        .json()
        .await?;
    let id = v["id"]
        .as_str()
        .ok_or(Error::BadResponse("linkup"))?
        .to_string();

    let deadline = tokio::time::Instant::now() + POLL * MAX_POLLS;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(POLL).await;
        let poll = async {
            let resp = crate::httptrace::send_traced(
                client
                    .get(format!("{base}/v1/research/{id}"))
                    .bearer_auth(key),
            )
            .await?;
            Ok::<Value, Error>(check("linkup", resp).await?.json().await?)
        };
        let v = match poll.await {
            Err(e) if e.is_transient() => continue,
            r => r?,
        };
        match v["status"].as_str().unwrap_or_default() {
            "completed" => return Ok(Outcome::new(report(&v["output"])?, cost)),
            "failed" => return Err(Error::BadResponse("linkup")),
            _ => continue,
        }
    }
    Err(Error::Timeout("linkup"))
}

fn report(out: &Value) -> Result<String> {
    let text = s(out, "answer");
    if text.is_empty() {
        return Err(Error::BadResponse("linkup"));
    }
    let sources: Vec<String> = out["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|o| s(o, "url"))
        .collect();
    Ok(with_sources(text, &sources))
}

pub async fn balance(base: &str, key: &str, client: &reqwest::Client) -> Result<LiveBalance> {
    let resp = client
        .get(format!("{base}/v1/credits/balance"))
        .bearer_auth(key)
        .send()
        .await?;
    parse_balance(&check("linkup", resp).await?.json().await?)
}

fn parse_balance(v: &Value) -> Result<LiveBalance> {
    let usd = v["balance"].as_f64().ok_or(Error::BadResponse("linkup"))?;
    Ok(LiveBalance {
        remaining: (usd * 200.0) as i64,
        total: ProviderKind::Linkup.default_quota(),
        usd: Some(usd),
        reset_at: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn niche_maps_to_native_params() {
        let b = search_body(&Input {
            query: Some("rust release".into()),
            recency: Some("2026-09-01".into()),
            domains: Some(vec!["blog.rust-lang.org".into(), "-reddit.com".into()]),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(b["depth"], "fast");
        assert_eq!(b["outputType"], "searchResults");
        assert_eq!(b["maxResults"], 5);
        assert_eq!(b["fromDate"], "2026-09-01");
        assert_eq!(b["includeDomains"], json!(["blog.rust-lang.org"]));
        assert_eq!(b["excludeDomains"], json!(["reddit.com"]));

        let plain = search_body(&Input {
            query: Some("q".into()),
            ..Default::default()
        })
        .unwrap();
        assert!(plain.get("fromDate").is_none() && plain.get("includeDomains").is_none());
    }

    #[test]
    fn topic_folds_into_the_query() {
        let input = Input {
            query: Some("crispr".into()),
            topic: Some("academic".into()),
            ..Default::default()
        };
        let q = "crispr (peer-reviewed OR research paper)";
        assert_eq!(search_body(&input).unwrap()["q"], q);
        assert_eq!(research_body(&input).unwrap().0["q"], q);
    }

    #[test]
    fn parses_text_hits() {
        let long = "x".repeat(2000);
        let h = hits(&json!({"results": [
            {"name": "l&#39;Europe &amp; AI", "url": "https://a.b", "content": long, "type": "text"},
            {"name": "img", "url": "https://a.b/i.png", "type": "image"}
        ]}));
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].title, "l'Europe & AI");
        assert_eq!(h[0].snippet.len(), SNIPPET_CHARS);
    }

    #[test]
    fn research_always_sends_depth() {
        let (b, cost) = research_body(&Input {
            query: Some("q".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(b["reasoningDepth"], "S");
        assert_eq!(b["outputType"], "sourcedAnswer");
        assert_eq!(cost, 50);
        let (b, cost) = research_body(&Input {
            query: Some("q".into()),
            depth: Some("deep".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(b["reasoningDepth"], "M");
        assert_eq!(cost, 100);
    }

    #[test]
    fn credit_codes_are_quota() {
        let body = |code: &str| format!(r#"linkup: {{"error":{{"code":"{code}"}}}}"#);
        for code in ["INSUFFICIENT_FUNDS_CREDITS", "EXCEED_BUDGET_LIMIT"] {
            assert!(matches!(
                out_of_credit(Error::rate_limit(body(code))),
                Error::QuotaExceeded(_)
            ));
        }
        assert!(matches!(
            out_of_credit(Error::rate_limit(body("TOO_MANY_REQUESTS"))),
            Error::RateLimit { .. }
        ));
    }

    #[test]
    fn usd_to_searches() {
        let b = parse_balance(&json!({"balance": 19.729})).unwrap();
        assert_eq!(b.remaining, 3945);
        assert_eq!(b.usd, Some(19.729));
        let dust = parse_balance(&json!({"balance": 0.0049})).unwrap();
        assert_eq!(dust.remaining, 0);
        assert!(parse_balance(&json!({})).is_err());
    }

    #[tokio::test]
    async fn research_polls_through_a_rate_limit() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/research"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "r1"})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/research/r1"))
            .respond_with(
                ResponseTemplate::new(429).set_body_json(
                    json!({"error": {"code": "TOO_MANY_REQUESTS"}, "statusCode": 429}),
                ),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/research/r1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"status": "completed", "output": {
                    "answer": "Report",
                    "sources": [{"name": "Tokio", "url": "https://tokio.rs", "snippet": "async"}]
                }}),
            ))
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
            Capability::DeepResearch,
            &input,
        )
        .await
        .unwrap();
        assert_eq!(
            (out.text.as_str(), out.cost),
            ("Report\n\nSources:\n1. https://tokio.rs", 50)
        );
    }

    #[tokio::test]
    async fn no_result_is_empty_not_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "statusCode": 400,
                "error": {"code": "SEARCH_QUERY_NO_RESULT", "message": "no result", "details": []}
            })))
            .mount(&server)
            .await;
        let input = Input {
            query: Some("zzzz".into()),
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
        assert!(out.text.is_empty());
    }
}
