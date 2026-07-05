use serde_json::{json, Value};

use super::{check, fmt_hits, niche, s, Capability, Hit, Input, LiveBalance, Outcome};
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
    let mut body = json!({ "query": search_query(input)?, "limit": input.results() });
    if let Some(tbs) = input.recency.as_deref().and_then(niche::recency_tbs) {
        body["tbs"] = json!(tbs);
    }
    let resp = client
        .post(format!("{base}/v1/search"))
        .bearer_auth(key)
        .json(&body)
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
    // firecrawl bills 2 credits per 10 results (2 minimum).
    Ok(Outcome::new(
        fmt_hits(&hits),
        (input.results().div_ceil(10) * 2) as i64,
    ))
}

// firecrawl search takes no domains param, so bake site:/-site: (and a topic hint) into the query.
fn search_query(input: &Input) -> Result<String> {
    let mut q = input.need_query()?.to_string();
    if let Some(t) = input.topic.as_deref() {
        match t {
            "academic" => q.push_str(" (peer-reviewed OR research paper)"),
            "news" => q.push_str(" latest news"),
            _ => {}
        }
    }
    if let Some(domains) = &input.domains {
        let (inc, exc) = niche::split_domains(domains);
        for d in inc {
            q.push_str(&format!(" site:{d}"));
        }
        for d in exc {
            q.push_str(&format!(" -site:{d}"));
        }
    }
    Ok(q)
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
    fn niche_maps_recency_and_domains() {
        let input = Input {
            query: Some("crispr".into()),
            recency: Some("week".into()),
            domains: Some(vec!["nature.com".into(), "-reddit.com".into()]),
            topic: Some("academic".into()),
            ..Default::default()
        };
        let q = search_query(&input).unwrap();
        assert!(q.contains("site:nature.com") && q.contains("-site:reddit.com"));
        assert!(q.contains("peer-reviewed"));
        assert_eq!(
            niche::recency_tbs(input.recency.as_deref().unwrap()),
            Some("qdr:w")
        );
    }

    #[test]
    fn plain_query_untouched() {
        let input = Input {
            query: Some("crispr".into()),
            ..Default::default()
        };
        assert_eq!(search_query(&input).unwrap(), "crispr");
    }

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
