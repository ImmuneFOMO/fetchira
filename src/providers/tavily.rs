use serde_json::{json, Value};

use super::{check, fmt_hits, s, Capability, Hit, Input, LiveBalance, Outcome};
use crate::error::{Error, Result};

pub async fn call(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    match cap {
        Capability::Search => search(base, key, client, input, "basic").await,
        Capability::DeepResearch => research(base, key, client, input).await,
        Capability::Read => extract(base, key, client, input).await,
        _ => Err(Error::Unsupported("tavily")),
    }
}

async fn search(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    input: &Input,
    depth: &str,
) -> Result<Outcome> {
    let resp = client
        .post(format!("{base}/search"))
        .bearer_auth(key)
        .json(&body(input, depth, false)?)
        .send()
        .await?;
    let v: Value = check("tavily", resp).await?.json().await?;
    let hits = v["results"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|o| Hit {
                    title: s(o, "title"),
                    url: s(o, "url"),
                    snippet: s(o, "content"),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(Outcome::new(fmt_hits(&hits), 1))
}

async fn research(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    input: &Input,
) -> Result<Outcome> {
    let resp = client
        .post(format!("{base}/search"))
        .bearer_auth(key)
        .json(&body(input, "advanced", true)?)
        .send()
        .await?;
    let v: Value = check("tavily", resp).await?.json().await?;
    let answer = v["answer"].as_str().unwrap_or_default().to_string();
    Ok(Outcome::new(answer, 2))
}

// Shared /search body. Maps the niche knobs to Tavily's native params: topic=news, `days`
// window, and include/exclude domains. Only set when the corresponding field is present.
fn body(input: &Input, depth: &str, include_answer: bool) -> Result<Value> {
    let mut b = serde_json::Map::new();
    b.insert("query".into(), json!(input.need_query()?));
    b.insert("max_results".into(), json!(input.results()));
    b.insert("search_depth".into(), json!(depth));
    if include_answer {
        b.insert("include_answer".into(), json!(true));
    }
    if input.topic.as_deref() == Some("news") {
        b.insert("topic".into(), json!("news"));
    }
    if let Some(days) = input
        .recency
        .as_deref()
        .and_then(super::niche::recency_days)
    {
        b.insert("days".into(), json!(days));
    }
    if let Some(domains) = &input.domains {
        let (inc, exc) = super::niche::split_domains(domains);
        if !inc.is_empty() {
            b.insert("include_domains".into(), json!(inc));
        }
        if !exc.is_empty() {
            b.insert("exclude_domains".into(), json!(exc));
        }
    }
    Ok(Value::Object(b))
}

async fn extract(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    input: &Input,
) -> Result<Outcome> {
    let resp = client
        .post(format!("{base}/extract"))
        .bearer_auth(key)
        .json(&json!({ "urls": [input.need_url()?] }))
        .send()
        .await?;
    let v: Value = check("tavily", resp).await?.json().await?;
    let text = v["results"]
        .as_array()
        .and_then(|a| a.first())
        .map(|r| s(r, "raw_content"))
        .filter(|t| !t.is_empty())
        .ok_or(Error::BadResponse("tavily"))?;
    Ok(Outcome::new(text, 1))
}

/// `GET /usage` → account-wide monthly credits. Free read, reflects paid plans automatically.
pub async fn balance(base: &str, key: &str, client: &reqwest::Client) -> Result<LiveBalance> {
    let resp = client
        .get(format!("{base}/usage"))
        .bearer_auth(key)
        .send()
        .await?;
    Ok(parse_balance(&check("tavily", resp).await?.json().await?))
}

// plan_limit is the monthly ceiling, plan_usage what's spent (basic search 1, advanced 2). Paygo
// overage can push usage past the plan, so remaining floors at 0.
fn parse_balance(v: &Value) -> LiveBalance {
    let a = &v["account"];
    let total = a["plan_limit"].as_i64().unwrap_or(0);
    let used = a["plan_usage"].as_i64().unwrap_or(0);
    LiveBalance {
        remaining: (total - used).max(0),
        total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn niche_maps_native_params() {
        let input = Input {
            query: Some("crispr trials".into()),
            topic: Some("news".into()),
            recency: Some("week".into()),
            domains: Some(vec!["nature.com".into(), "-reddit.com".into()]),
            ..Default::default()
        };
        let b = body(&input, "advanced", true).unwrap();
        assert_eq!(b["topic"], "news");
        assert_eq!(b["days"], 7);
        assert_eq!(b["include_domains"], json!(["nature.com"]));
        assert_eq!(b["exclude_domains"], json!(["reddit.com"]));
        assert_eq!(b["include_answer"], true);
    }

    #[test]
    fn plain_search_body_unchanged() {
        let input = Input {
            query: Some("hello".into()),
            ..Default::default()
        };
        let b = body(&input, "basic", false).unwrap();
        assert_eq!(b["search_depth"], "basic");
        assert!(b.get("topic").is_none());
        assert!(b.get("days").is_none());
        assert!(b.get("include_domains").is_none());
        assert!(b.get("include_answer").is_none());
    }

    #[test]
    fn parses_usage() {
        let b = parse_balance(&json!({
            "key": {"usage": 20},
            "account": {"current_plan": "Researcher", "plan_usage": 20, "plan_limit": 1000},
        }));
        assert_eq!(b.remaining, 980);
        assert_eq!(b.total, 1000);
    }
}
