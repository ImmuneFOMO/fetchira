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
    let resp = crate::httptrace::send_traced(
        client
            .post(format!("{base}/v1/search"))
            .bearer_auth(key)
            .json(&body),
    )
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
    match input.mode.as_deref() {
        None => scrape_markdown(base, key, client, input).await,
        Some("extract") => extract(base, key, client, input).await,
        Some("crawl") => crawl(base, key, client, input).await,
        Some(other) => Err(Error::Provider {
            provider: "firecrawl",
            status: 400,
            body: format!("unknown mode '{other}'; valid modes: extract, crawl"),
        }),
    }
}

async fn scrape_markdown(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    input: &Input,
) -> Result<Outcome> {
    let resp = crate::httptrace::send_traced(
        client
            .post(format!("{base}/v1/scrape"))
            .bearer_auth(key)
            .json(&json!({ "url": input.need_url()?, "formats": ["markdown"] })),
    )
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

// LLM-structured extraction over a single url; query doubles as the extraction prompt.
async fn extract(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    input: &Input,
) -> Result<Outcome> {
    let resp = crate::httptrace::send_traced(
        client
            .post(format!("{base}/v1/extract"))
            .bearer_auth(key)
            .json(&extract_body(input)?),
    )
    .await?;
    let v: Value = check("firecrawl", resp).await?.json().await?;
    let cost = v["data"]["metadata"]["creditsUsed"].as_i64().unwrap_or(1);
    Ok(Outcome::new(v["data"].to_string(), cost))
}

fn extract_body(input: &Input) -> Result<Value> {
    Ok(json!({
        "urls": [input.need_url()?],
        "prompt": input.query.as_deref().unwrap_or("Extract the main content of the page as structured data."),
    }))
}

// crawl is async on firecrawl's side; kick it off and hand back the job id to poll.
async fn crawl(base: &str, key: &str, client: &reqwest::Client, input: &Input) -> Result<Outcome> {
    let resp = crate::httptrace::send_traced(
        client
            .post(format!("{base}/v1/crawl"))
            .bearer_auth(key)
            .json(&json!({ "url": input.need_url()? })),
    )
    .await?;
    let v: Value = check("firecrawl", resp).await?.json().await?;
    let text = match v["id"].as_str() {
        Some(id) => format!("crawl started, job id: {id}"),
        None => v.to_string(),
    };
    Ok(Outcome::new(text, 1))
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
    fn extract_body_uses_url_and_query_prompt() {
        let input = Input {
            url: Some("https://example.com".into()),
            query: Some("list the pricing tiers".into()),
            mode: Some("extract".into()),
            ..Default::default()
        };
        let body = extract_body(&input).unwrap();
        assert_eq!(body["urls"], json!(["https://example.com"]));
        assert_eq!(body["prompt"], "list the pricing tiers");
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
