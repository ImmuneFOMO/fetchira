use std::time::Duration;

use serde_json::{json, Value};

use super::{check, fmt_hits, s, value_to_text, Capability, Hit, Input, LiveBalance, Outcome};
use crate::error::{Error, Result};

const POLL: Duration = Duration::from_secs(3);
const MAX_POLLS: u32 = 120;

pub async fn call(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    match cap {
        Capability::Search => search(base, key, client, input).await,
        Capability::DeepResearch => research(base, key, client, input).await,
        _ => Err(Error::Unsupported("parallel")),
    }
}

async fn search(base: &str, key: &str, client: &reqwest::Client, input: &Input) -> Result<Outcome> {
    let resp = crate::httptrace::send_traced(
        client
            .post(format!("{base}/v1beta/search"))
            .header("x-api-key", key)
            .header("parallel-beta", "search-extract-2025-10-10")
            .json(&search_body(input)?),
    )
    .await?;
    let v: Value = check("parallel", resp).await?.json().await?;
    let hits = v["results"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|o| Hit {
                    title: s(o, "title"),
                    url: s(o, "url"),
                    snippet: o["excerpts"]
                        .as_array()
                        .map(|e| {
                            e.iter()
                                .filter_map(|x| x.as_str())
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .unwrap_or_default(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(Outcome::new(fmt_hits(&hits), 1))
}

fn search_body(input: &Input) -> Result<Value> {
    let q = input.need_query()?.to_string() + super::niche::topic_hint(input);
    let mut body = json!({
        "objective": q,
        "search_queries": [q],
        "max_results": input.results(),
    });
    if let Some(sp) = source_policy(input) {
        body["source_policy"] = sp;
    }
    if let Some(d) = input
        .recency
        .as_deref()
        .and_then(super::niche::recency_date)
    {
        body["source_policy"]["after_date"] = json!(d);
    }
    Ok(body)
}

// `after_date` only filters search, so task runs fold recency into the input.
fn research_body(input: &Input) -> Result<Value> {
    input.need_query()?;
    let mut body = json!({
        "input": super::niche::rewrite_query(input),
        "processor": processor(input),
    });
    if let Some(sp) = source_policy(input) {
        body["source_policy"] = sp;
    }
    Ok(body)
}

fn source_policy(input: &Input) -> Option<Value> {
    let (inc, exc) = super::niche::split_domains(input.domains.as_deref()?);
    let mut sp = json!({});
    if !inc.is_empty() {
        sp["include_domains"] = json!(inc);
    }
    if !exc.is_empty() {
        sp["exclude_domains"] = json!(exc);
    }
    Some(sp)
}

fn processor(input: &Input) -> &'static str {
    match input.depth.as_deref() {
        Some("deep") => "core",
        _ => "base",
    }
}

async fn research(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    input: &Input,
) -> Result<Outcome> {
    let start = crate::httptrace::send_traced(
        client
            .post(format!("{base}/v1/tasks/runs"))
            .header("x-api-key", key)
            .json(&research_body(input)?),
    )
    .await?;
    let v: Value = check("parallel", start).await?.json().await?;
    let id = v["run_id"]
        .as_str()
        .ok_or(Error::BadResponse("parallel"))?
        .to_string();

    let deadline = tokio::time::Instant::now() + POLL * MAX_POLLS;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(POLL).await;
        let poll = async {
            let resp = crate::httptrace::send_traced(
                client
                    .get(format!("{base}/v1/tasks/runs/{id}"))
                    .header("x-api-key", key),
            )
            .await?;
            Ok::<Value, Error>(check("parallel", resp).await?.json().await?)
        };
        let v = match poll.await {
            Err(e) if e.is_transient() => continue,
            r => r?,
        };
        match v["status"].as_str().unwrap_or_default() {
            "completed" => return result(base, key, client, &id).await,
            "failed" | "cancelled" | "canceled" => return Err(Error::BadResponse("parallel")),
            _ => continue,
        }
    }
    Err(Error::Timeout("parallel"))
}

async fn result(base: &str, key: &str, client: &reqwest::Client, id: &str) -> Result<Outcome> {
    let resp = crate::httptrace::send_traced(
        client
            .get(format!("{base}/v1/tasks/runs/{id}/result"))
            .header("x-api-key", key),
    )
    .await?;
    let v: Value = check("parallel", resp).await?.json().await?;
    let content = &v["output"]["content"];
    let text = content["output"]
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| value_to_text(content));
    Ok(Outcome::new(text, 1))
}

/// Live $ balance in cents via the dashboard cookie session; the api-key can't read it.
pub async fn balance(client: &wreq::Client) -> Result<(LiveBalance, Vec<(String, String)>)> {
    let resp = client
        .post("https://platform.parallel.ai/api/acc_svc/billing_overview")
        .header("content-type", "application/json")
        .header("origin", "https://platform.parallel.ai")
        .header(
            "referer",
            "https://platform.parallel.ai/settings?tab=billing",
        )
        .body("{}")
        .send()
        .await?;
    if resp.status().as_u16() != 200 {
        return Err(Error::Provider {
            provider: "parallel",
            status: resp.status().as_u16(),
            body: resp.text().await.unwrap_or_default(),
        });
    }
    let updates = crate::web::set_cookie_updates(resp.headers());
    let v: Value = serde_json::from_str(&resp.text().await.unwrap_or_default())
        .map_err(|_| Error::BadResponse("parallel"))?;
    Ok((parse_balance(&v)?, updates))
}

fn parse_balance(v: &Value) -> Result<LiveBalance> {
    if !v["balanceError"].is_null() {
        return Err(Error::BadResponse("parallel"));
    }
    let cents = v["balance"]
        .as_i64()
        .ok_or(Error::BadResponse("parallel"))?;
    let searches = cents.saturating_mul(2);
    Ok(LiveBalance {
        remaining: searches,
        total: searches,
        usd: Some(cents as f64 / 100.0),
        reset_at: None,
    })
}

/// Account email via the dashboard cookie session, like `balance`.
pub async fn identity(client: &wreq::Client) -> Result<Option<String>> {
    let resp = client
        .post("https://platform.parallel.ai/api/rpc/auth/get-my-api-key-and-session-details")
        .header("content-type", "application/json")
        .header("origin", "https://platform.parallel.ai")
        .header(
            "referer",
            "https://platform.parallel.ai/settings?tab=billing",
        )
        .body("{}")
        .send()
        .await?;
    if resp.status().as_u16() != 200 {
        return Err(Error::Provider {
            provider: "parallel",
            status: resp.status().as_u16(),
            body: resp.text().await.unwrap_or_default(),
        });
    }
    let v: Value = serde_json::from_str(&resp.text().await.unwrap_or_default())
        .map_err(|_| Error::BadResponse("parallel"))?;
    Ok(v["data"]["userEmail"].as_str().map(String::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn niche_domains_to_source_policy() {
        let input = Input {
            query: Some("crispr".into()),
            domains: Some(vec!["nature.com".into(), "-reddit.com".into()]),
            recency: Some("week".into()),
            ..Default::default()
        };
        let body = search_body(&input).unwrap();
        assert_eq!(
            body["source_policy"]["include_domains"],
            json!(["nature.com"])
        );
        assert_eq!(
            body["source_policy"]["exclude_domains"],
            json!(["reddit.com"])
        );
        assert_eq!(
            body["source_policy"]["after_date"].as_str(),
            super::super::niche::recency_date("week").as_deref()
        );
        assert_eq!(body["objective"], "crispr");
    }

    #[test]
    fn plain_search_body_unchanged() {
        let input = Input {
            query: Some("crispr".into()),
            ..Default::default()
        };
        let body = search_body(&input).unwrap();
        assert_eq!(body["objective"], "crispr");
        assert!(body.get("source_policy").is_none());
    }

    #[test]
    fn research_body_carries_the_niche() {
        let input = Input {
            query: Some("crispr".into()),
            domains: Some(vec!["nature.com".into()]),
            ..Default::default()
        };
        let body = research_body(&input).unwrap();
        assert_eq!(
            body["source_policy"]["include_domains"],
            json!(["nature.com"])
        );
        assert_eq!(body["input"], "crispr");
        assert_eq!(body["processor"], "base");
        let plain = research_body(&Input {
            query: Some("crispr".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(plain, json!({ "input": "crispr", "processor": "base" }));
        let recent = research_body(&Input {
            query: Some("crispr".into()),
            recency: Some("2024-03-01".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(recent["input"], "crispr after:2024-03-01");
    }

    #[test]
    fn depth_selects_processor() {
        assert_eq!(processor(&Input::default()), "base");
        assert_eq!(
            processor(&Input {
                depth: Some("deep".into()),
                ..Default::default()
            }),
            "core"
        );
        assert_eq!(
            processor(&Input {
                depth: Some("standard".into()),
                ..Default::default()
            }),
            "base"
        );
    }

    #[test]
    fn cents_to_searches() {
        let b = parse_balance(&json!({"balance": 1690, "balanceError": null})).unwrap();
        assert_eq!(b.remaining, 3380);
        assert_eq!(b.total, 3380);
        assert!(parse_balance(&json!({"balanceError": null})).is_err());
        assert!(parse_balance(&json!({"balance": 1690, "balanceError": "stale"})).is_err());
    }
}
