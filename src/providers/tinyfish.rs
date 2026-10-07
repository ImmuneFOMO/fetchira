use std::time::Duration;

use serde_json::{json, Value};

use super::{check, fmt_hits, niche, s, with_sources, Capability, Hit, Input, Outcome};
use crate::error::{Error, Result};

const POLL: Duration = Duration::from_secs(10);
const MAX_POLLS: u32 = 150;

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
        _ => Err(Error::Unsupported("tinyfish")),
    }
}

// No result-count param: a page is always up to 10 hits, so trim client-side.
async fn search(base: &str, key: &str, client: &reqwest::Client, input: &Input) -> Result<Outcome> {
    let resp = crate::httptrace::send_traced(
        client
            .get(base)
            .header("X-API-Key", key)
            .query(&params(input)?),
    )
    .await?;
    let v: Value = check("tinyfish", resp).await?.json().await?;
    let hits = v["results"]
        .as_array()
        .map(|a| {
            a.iter()
                .take(input.results() as usize)
                .map(|o| Hit {
                    title: s(o, "title"),
                    url: s(o, "url"),
                    snippet: s(o, "snippet"),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // Searches draw on their own, much larger daily pool (enforced by its 429s); the local quota
    // tracks the 1,000/day fetch cap.
    Ok(Outcome::new(fmt_hits(&hits), 0))
}

fn domain_type(input: &Input) -> &'static str {
    match input.topic.as_deref() {
        Some("news") => "news",
        Some("academic") => "research_paper",
        _ => "web",
    }
}

fn params(input: &Input) -> Result<Vec<(&'static str, String)>> {
    let kind = domain_type(input);
    let mut p = vec![
        ("query", input.need_query()?.to_string()),
        ("domain_type", kind.into()),
    ];
    let since = input.recency.as_deref().and_then(niche::recency_date);
    if kind == "research_paper" {
        // Date filters 400 on papers; a publication-year floor is the closest native filter.
        if let Some(d) = since {
            p.push(("pub_year_min", d[..4].to_string()));
        }
    } else if let Some(days) = input.recency.as_deref().and_then(niche::recency_days) {
        p.push(("recency_minutes", (days * 1440).to_string()));
    } else if let Some(d) = since {
        p.push(("after_date", d));
    }
    if let Some(domains) = &input.domains {
        let (inc, exc) = niche::split_domains(domains);
        if !inc.is_empty() {
            p.push(("include_domains", inc.join(",")));
        }
        if !exc.is_empty() {
            p.push(("exclude_domains", exc.join(",")));
        }
    }
    Ok(p)
}

async fn fetch(base: &str, key: &str, client: &reqwest::Client, input: &Input) -> Result<Outcome> {
    let resp = crate::httptrace::send_traced(
        client
            .post(base.replace("api.search", "api.fetch"))
            .header("X-API-Key", key)
            .json(&json!({ "urls": [input.need_url()?], "format": "markdown", "ttl": 0 })),
    )
    .await?;
    let v: Value = check("tinyfish", resp).await?.json().await?;
    Ok(Outcome::new(page(&v)?, 1))
}

// A failed URL still answers 200; the status under `errors` is the target's, not a rejected key.
fn page(v: &Value) -> Result<String> {
    if let Some(text) = v["results"][0]["text"].as_str().filter(|t| !t.is_empty()) {
        return Ok(text.to_string());
    }
    let e = v["errors"].get(0).ok_or(Error::BadResponse("tinyfish"))?;
    Err(Error::Provider {
        provider: "tinyfish",
        status: 0,
        body: match e["status"].as_u64() {
            Some(code) => format!("{} ({code})", s(e, "error")),
            None => s(e, "error"),
        },
    })
}

async fn research(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    input: &Input,
) -> Result<Outcome> {
    let agent = base.replace("api.search", "agent");
    let start = crate::httptrace::send_traced(
        client
            .post(format!("{agent}/v1/automation/run-research-async"))
            .header("X-API-Key", key)
            .json(&research_body(input)?),
    )
    .await?;
    let v: Value = check("tinyfish", start)
        .await
        .map_err(daily_cap)?
        .json()
        .await?;
    let id = v["research_run_id"]
        .as_str()
        .ok_or(Error::BadResponse("tinyfish"))?
        .to_string();

    let deadline = tokio::time::Instant::now() + POLL * MAX_POLLS;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(POLL).await;
        let poll = async {
            let resp = crate::httptrace::send_traced(
                client
                    .get(format!("{agent}/v1/research-run/{id}"))
                    .header("X-API-Key", key),
            )
            .await?;
            Ok::<Value, Error>(check("tinyfish", resp).await?.json().await?)
        };
        let v = match poll.await {
            Err(e) if e.is_transient() => continue,
            r => r?,
        };
        match v["status"].as_str().unwrap_or_default() {
            "COMPLETED" => return Ok(Outcome::new(report(&v)?, 1)),
            "FAILED" | "CANCELLED" | "TIMED_OUT" => return Err(Error::BadResponse("tinyfish")),
            _ => continue,
        }
    }
    Err(Error::Timeout("tinyfish"))
}

fn research_body(input: &Input) -> Result<Value> {
    // An omitted mode runs the slow `deep` tier.
    let mode = match input.depth.as_deref() {
        Some("deep") => "deep",
        _ => "standard",
    };
    let mut b = json!({ "query": input.need_query()?, "mode": mode });
    let kind = domain_type(input);
    if kind != "web" {
        b["domain_type"] = json!(kind);
    }
    let since = input.recency.as_deref().and_then(niche::recency_date);
    if kind == "research_paper" {
        // Papers take no date filter, and research has no year floor to fall back on.
        if let Some(d) = since {
            b["query"] = json!(format!("{} after:{d}", input.need_query()?));
        }
    } else if let Some(days) = input.recency.as_deref().and_then(niche::recency_days) {
        b["recency_minutes"] = json!(days * 1440);
    } else if let Some(d) = since {
        b["after_date"] = json!(d);
    }
    if let Some(domains) = &input.domains {
        let (inc, exc) = niche::split_domains(domains);
        // `exclusive` keeps to the listed domains; `include` only ranks them first.
        b["domain_filter"] = json!({
            "mode": if inc.is_empty() { "include" } else { "exclusive" },
            "domains": inc,
            "block": exc,
        });
    }
    Ok(b)
}

// The research day cap shares 429 with the per-minute limit but holds until the day rolls over.
fn daily_cap(e: Error) -> Error {
    match e {
        Error::RateLimit { message, .. } if message.contains("DAILY_LIMIT_EXCEEDED") => {
            Error::QuotaExceeded(message)
        }
        e => e,
    }
}

// Standard runs report under `quick_result`, deep under `deep_result`; `[n]` index the citations.
fn report(v: &Value) -> Result<String> {
    let (r, field) = match &v["deep_result"] {
        d if d["result"].as_str().is_some_and(|t| !t.is_empty()) => (d, "result"),
        _ => (&v["quick_result"], "answer"),
    };
    let text = s(r, field);
    if text.is_empty() {
        return Err(Error::BadResponse("tinyfish"));
    }
    let sources: Vec<String> = r["citations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c.as_str().map(String::from))
        .collect();
    Ok(with_sources(text, &sources))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(p: &'a [(&str, String)], k: &str) -> Option<&'a str> {
        p.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn builds_niche_params() {
        let p = params(&Input {
            query: Some("tokio".into()),
            topic: Some("news".into()),
            recency: Some("week".into()),
            domains: Some(vec![
                "docs.rs".into(),
                "github.com".into(),
                "-reddit.com".into(),
            ]),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(get(&p, "domain_type"), Some("news"));
        assert_eq!(get(&p, "recency_minutes"), Some("10080"));
        assert_eq!(get(&p, "include_domains"), Some("docs.rs,github.com"));
        assert_eq!(get(&p, "exclude_domains"), Some("reddit.com"));

        let iso = params(&Input {
            query: Some("tokio".into()),
            recency: Some("2026-09-01".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(get(&iso, "domain_type"), Some("web"));
        assert_eq!(get(&iso, "after_date"), Some("2026-09-01"));
        assert_eq!(get(&iso, "recency_minutes"), None);

        let papers = params(&Input {
            query: Some("rag".into()),
            topic: Some("academic".into()),
            recency: Some("2024-03-01".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(get(&papers, "domain_type"), Some("research_paper"));
        assert_eq!(get(&papers, "pub_year_min"), Some("2024"));
        assert_eq!(get(&papers, "after_date"), None);
    }

    #[test]
    fn research_maps_the_niche() {
        let b = research_body(&Input {
            query: Some("EU AI act".into()),
            topic: Some("news".into()),
            recency: Some("week".into()),
            domains: Some(vec!["europa.eu".into(), "-reddit.com".into()]),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(b["mode"], "standard");
        assert_eq!(b["domain_type"], "news");
        assert_eq!(b["recency_minutes"], 10080);
        assert_eq!(
            b["domain_filter"],
            json!({"mode": "exclusive", "domains": ["europa.eu"], "block": ["reddit.com"]})
        );

        let papers = research_body(&Input {
            query: Some("rag".into()),
            topic: Some("academic".into()),
            recency: Some("2024-03-01".into()),
            domains: Some(vec!["-reddit.com".into()]),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(papers["domain_type"], "research_paper");
        assert_eq!(papers["query"], "rag after:2024-03-01");
        assert!(papers.get("after_date").is_none());
        assert_eq!(papers["domain_filter"]["mode"], "include");

        let plain = research_body(&Input {
            query: Some("rag".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(plain, json!({"query": "rag", "mode": "standard"}));
    }

    #[test]
    fn failed_fetch_surfaces_reason() {
        assert_eq!(
            page(&json!({"results": [{"text": "# Tokio"}], "errors": []})).unwrap(),
            "# Tokio"
        );
        let err = page(&json!({"results": [], "errors": [
            {"url": "https://httpbin.org/status/401", "error": "target_http_error", "status": 401}
        ]}));
        assert!(matches!(
            err,
            Err(Error::Provider { status: 0, body, .. }) if body == "target_http_error (401)"
        ));
        assert!(page(&json!({"results": [], "errors": []})).is_err());
    }

    #[test]
    fn report_reads_either_result_with_citations() {
        let deep = report(&json!({
            "quick_result": null,
            "deep_result": {"result": "Report [1]", "citations": ["https://a.b"]}
        }))
        .unwrap();
        assert_eq!(deep, "Report [1]\n\nSources:\n1. https://a.b");
        let quick = report(&json!({
            "quick_result": {"answer": "Brief", "citations": []},
            "deep_result": null
        }))
        .unwrap();
        assert_eq!(quick, "Brief");
        assert!(report(&json!({"quick_result": {"answer": ""}})).is_err());
    }

    #[test]
    fn daily_research_cap_is_quota() {
        let cap = daily_cap(Error::rate_limit(
            r#"tinyfish: {"error":{"code":"DAILY_LIMIT_EXCEEDED"}}"#,
        ));
        assert!(matches!(cap, Error::QuotaExceeded(_)));
        let burst = daily_cap(Error::rate_limit(
            r#"tinyfish: {"error":{"code":"RATE_LIMIT_EXCEEDED"}}"#,
        ));
        assert!(matches!(burst, Error::RateLimit { .. }));
    }
}
