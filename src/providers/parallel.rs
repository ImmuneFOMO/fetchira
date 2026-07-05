use std::time::Duration;

use serde_json::{json, Value};

use super::{check, fmt_hits, s, value_to_text, Capability, Hit, Input, LiveBalance, Outcome};
use crate::error::{Error, Result};

const POLL: Duration = Duration::from_secs(3);
const MAX_POLLS: u32 = 40;

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
    let resp = client
        .post(format!("{base}/v1beta/search"))
        .header("x-api-key", key)
        .header("parallel-beta", "search-extract-2025-10-10")
        .json(&search_body(input)?)
        .send()
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

// domains map to the native `source_policy`; topic/recency have no clean param, so they fold into
// the objective via `rewrite_query`. Common (no-niche) case builds the exact body as before.
fn search_body(input: &Input) -> Result<Value> {
    let q = input.need_query()?;
    if !super::niche::any(input) {
        return Ok(json!({
            "objective": q,
            "search_queries": [q],
            "max_results": input.results(),
        }));
    }
    let folded = super::niche::rewrite_query(input);
    let mut body = json!({
        "objective": folded,
        "search_queries": [folded],
        "max_results": input.results(),
    });
    if let Some(domains) = &input.domains {
        let (inc, exc) = super::niche::split_domains(domains);
        let mut sp = json!({});
        if !inc.is_empty() {
            sp["include_domains"] = json!(inc);
        }
        if !exc.is_empty() {
            sp["exclude_domains"] = json!(exc);
        }
        body["source_policy"] = sp;
    }
    Ok(body)
}

async fn research(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    input: &Input,
) -> Result<Outcome> {
    let start = client
        .post(format!("{base}/v1/tasks/runs"))
        .header("x-api-key", key)
        .json(&json!({ "input": input.need_query()?, "processor": "base" }))
        .send()
        .await?;
    let v: Value = check("parallel", start).await?.json().await?;
    let id = v["run_id"]
        .as_str()
        .ok_or(Error::BadResponse("parallel"))?
        .to_string();

    for _ in 0..MAX_POLLS {
        tokio::time::sleep(POLL).await;
        let resp = client
            .get(format!("{base}/v1/tasks/runs/{id}"))
            .header("x-api-key", key)
            .send()
            .await?;
        let v: Value = check("parallel", resp).await?.json().await?;
        match v["status"].as_str().unwrap_or_default() {
            "completed" => return result(base, key, client, &id).await,
            "failed" | "cancelled" | "canceled" => return Err(Error::BadResponse("parallel")),
            _ => continue,
        }
    }
    Err(Error::Timeout("parallel"))
}

async fn result(base: &str, key: &str, client: &reqwest::Client, id: &str) -> Result<Outcome> {
    let resp = client
        .get(format!("{base}/v1/tasks/runs/{id}/result"))
        .header("x-api-key", key)
        .send()
        .await?;
    let v: Value = check("parallel", resp).await?.json().await?;
    let content = &v["output"]["content"];
    let text = content["output"]
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| value_to_text(content));
    Ok(Outcome::new(text, 1))
}

/// Live $ balance via the dashboard's cookie session (the api-key can't read it — api.parallel.ai's
/// balance is OAuth-only). `billing_overview` → `{balance: <cents>}`; a search is $0.005, so cents·2
/// = searches.
pub async fn balance(client: &wreq::Client) -> Result<LiveBalance> {
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
    let v: Value = serde_json::from_str(&resp.text().await.unwrap_or_default())
        .map_err(|_| Error::BadResponse("parallel"))?;
    Ok(parse_balance(&v))
}

// No fixed ceiling exists for a top-up balance, so the gauge tracks the live figure itself.
// ponytail: total = remaining (bar full while funded); a stored high-water-mark would give a
// draining bar, add it if the flat gauge proves confusing.
fn parse_balance(v: &Value) -> LiveBalance {
    let searches = v["balance"].as_i64().unwrap_or(0) * 2;
    LiveBalance {
        remaining: searches,
        total: searches,
    }
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
        assert!(body["objective"].as_str().unwrap().contains("after:"));
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
    fn cents_to_searches() {
        let b = parse_balance(&json!({"balance": 1690, "balanceError": null}));
        assert_eq!(b.remaining, 3380); // $16.90 / $0.005
        assert_eq!(b.total, 3380);
    }
}
