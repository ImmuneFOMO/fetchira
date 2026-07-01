use serde_json::{json, Value};

use super::{check, fmt_hits, s, Capability, Hit, Input, Outcome};
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
        .json(&json!({
            "query": input.need_query()?,
            "max_results": input.results(),
            "search_depth": depth,
        }))
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
    let cost = if depth == "advanced" { 2 } else { 1 };
    Ok(Outcome::new(fmt_hits(&hits), cost))
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
        .json(&json!({
            "query": input.need_query()?,
            "search_depth": "advanced",
            "include_answer": true,
            "max_results": input.results(),
        }))
        .send()
        .await?;
    let v: Value = check("tavily", resp).await?.json().await?;
    let answer = v["answer"].as_str().unwrap_or_default().to_string();
    Ok(Outcome::new(answer, 2))
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
