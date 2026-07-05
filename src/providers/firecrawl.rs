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
        Capability::Search => search(base, key, client, input).await,
        Capability::Read => scrape(base, key, client, input).await,
        _ => Err(Error::Unsupported("firecrawl")),
    }
}

async fn search(base: &str, key: &str, client: &reqwest::Client, input: &Input) -> Result<Outcome> {
    let resp = client
        .post(format!("{base}/v1/search"))
        .bearer_auth(key)
        .json(&json!({ "query": input.need_query()?, "limit": input.results() }))
        .send()
        .await?;
    let v: Value = check("firecrawl", resp).await?.json().await?;
    let hits = v["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|o| Hit {
                    title: s(o, "title"),
                    url: s(o, "url"),
                    snippet: s(o, "description"),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(Outcome::new(fmt_hits(&hits), 1))
}

async fn scrape(base: &str, key: &str, client: &reqwest::Client, input: &Input) -> Result<Outcome> {
    let resp = client
        .post(format!("{base}/v1/scrape"))
        .bearer_auth(key)
        .json(&json!({ "url": input.need_url()?, "formats": ["markdown"] }))
        .send()
        .await?;
    let v: Value = check("firecrawl", resp).await?.json().await?;
    let text = v["data"]["markdown"]
        .as_str()
        .filter(|t| !t.is_empty())
        .ok_or(Error::BadResponse("firecrawl"))?
        .to_string();
    let cost = v["data"]["metadata"]["creditsUsed"].as_i64().unwrap_or(1);
    Ok(Outcome::new(text, cost))
}

/// `GET /v1/team/credit-usage` → monthly credit balance. Free read, reflects paid plans + top-ups.
pub async fn balance(base: &str, key: &str, client: &reqwest::Client) -> Result<LiveBalance> {
    let resp = client
        .get(format!("{base}/v1/team/credit-usage"))
        .bearer_auth(key)
        .send()
        .await?;
    Ok(parse_balance(
        &check("firecrawl", resp).await?.json().await?,
    ))
}

// remaining_credits is the real spendable balance (includes coupons/packs/auto-recharge);
// plan_credits is only the monthly base, so it seeds the gauge ceiling but can't cap remaining.
fn parse_balance(v: &Value) -> LiveBalance {
    let d = &v["data"];
    let remaining = d["remaining_credits"].as_i64().unwrap_or(0);
    LiveBalance {
        remaining,
        total: d["plan_credits"].as_i64().unwrap_or(0).max(remaining),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_credit_usage() {
        let b = parse_balance(&json!({
            "success": true,
            "data": {"remaining_credits": 1357, "plan_credits": 1000},
        }));
        assert_eq!(b.remaining, 1357);
        assert_eq!(b.total, 1357); // top-ups exceed the plan base
    }
}
