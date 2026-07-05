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
        Capability::Read => contents(base, key, client, input).await,
        Capability::DeepResearch => research(base, key, client, input).await,
        _ => Err(Error::Unsupported("exa")),
    }
}

async fn search(base: &str, key: &str, client: &reqwest::Client, input: &Input) -> Result<Outcome> {
    let resp = client
        .post(format!("{base}/search"))
        .header("x-api-key", key)
        .json(&search_body(input, 500)?)
        .send()
        .await?;
    let v: Value = check("exa", resp).await?.json().await?;
    Ok(Outcome::new(fmt_hits(&hits(&v)), 1))
}

fn search_body(input: &Input, max_chars: u32) -> Result<Value> {
    let mut body = json!({
        "query": input.need_query()?,
        "numResults": input.results(),
        "contents": { "text": { "maxCharacters": max_chars } },
    });
    if let Some(cat) = input.topic.as_deref().and_then(|t| match t {
        "news" => Some("news"),
        "academic" => Some("research paper"),
        _ => None,
    }) {
        body["category"] = json!(cat);
    }
    if let Some(d) = input
        .recency
        .as_deref()
        .and_then(super::niche::recency_date)
    {
        body["startPublishedDate"] = json!(d);
    }
    if let Some(domains) = &input.domains {
        let (inc, exc) = super::niche::split_domains(domains);
        if !inc.is_empty() {
            body["includeDomains"] = json!(inc);
        }
        if !exc.is_empty() {
            body["excludeDomains"] = json!(exc);
        }
    }
    Ok(body)
}

fn hits(v: &Value) -> Vec<Hit> {
    v["results"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|o| Hit {
                    title: s(o, "title"),
                    url: s(o, "url"),
                    snippet: s(o, "text"),
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn contents(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    input: &Input,
) -> Result<Outcome> {
    let resp = client
        .post(format!("{base}/contents"))
        .header("x-api-key", key)
        .json(&json!({
            "urls": [input.need_url()?],
            "text": { "maxCharacters": 8000 },
        }))
        .send()
        .await?;
    let v: Value = check("exa", resp).await?.json().await?;
    let text = v["results"]
        .as_array()
        .and_then(|a| a.first())
        .map(|r| s(r, "text"))
        .filter(|t| !t.is_empty())
        .ok_or(Error::BadResponse("exa"))?;
    Ok(Outcome::new(text, 1))
}

// The async `/research/v1` job endpoint is EOL; deep research now runs synchronously through
// `/search` with the `deep-reasoning` type (also the `depth=deep` path).
async fn research(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    input: &Input,
) -> Result<Outcome> {
    let resp = client
        .post(format!("{base}/search"))
        .header("x-api-key", key)
        .json(&research_body(input)?)
        .send()
        .await?;
    let v: Value = check("exa", resp).await?.json().await?;
    Ok(Outcome::new(fmt_hits(&hits(&v)), 1))
}

fn research_body(input: &Input) -> Result<Value> {
    let mut body = search_body(input, 8000)?;
    body["type"] = json!("deep-reasoning");
    Ok(body)
}

/// Live $ balance via the dashboard's cookie session (the api-key has no balance endpoint — exa is a
/// PAYG $ balance, not a request quota). `get-credits` → `{orbCreditsInCents}`; a search is $0.007,
/// so cents/0.7 = searches.
pub async fn balance(client: &wreq::Client) -> Result<LiveBalance> {
    let resp = client
        .get("https://dashboard.exa.ai/api/get-credits")
        .header("referer", "https://dashboard.exa.ai/billing")
        .send()
        .await?;
    if resp.status().as_u16() != 200 {
        return Err(Error::Provider {
            provider: "exa",
            status: resp.status().as_u16(),
            body: resp.text().await.unwrap_or_default(),
        });
    }
    let v: Value = serde_json::from_str(&resp.text().await.unwrap_or_default())
        .map_err(|_| Error::BadResponse("exa"))?;
    Ok(parse_balance(&v))
}

// No fixed ceiling exists for a top-up balance, so the gauge tracks the live figure itself.
// ponytail: total = remaining (bar full while funded); a stored high-water-mark would give a
// draining bar, add it if the flat gauge proves confusing.
fn parse_balance(v: &Value) -> LiveBalance {
    let searches = (v["orbCreditsInCents"].as_f64().unwrap_or(0.0) / 0.7) as i64;
    LiveBalance {
        remaining: searches,
        total: searches,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cents_to_searches() {
        let b = parse_balance(&json!({"orbCreditsInCents": 1766.99, "orbInvoiceDebt": 0}));
        assert_eq!(b.remaining, 2524); // $17.67 / $0.007
        assert_eq!(b.total, 2524);
    }

    #[test]
    fn plain_search_body_unchanged() {
        let b = search_body(
            &Input {
                query: Some("crispr".into()),
                ..Default::default()
            },
            500,
        )
        .unwrap();
        assert_eq!(b["query"], "crispr");
        assert!(b.get("category").is_none());
        assert!(b.get("startPublishedDate").is_none());
        assert!(b.get("includeDomains").is_none());
    }

    #[test]
    fn niche_maps_to_native_params() {
        let input = Input {
            query: Some("crispr".into()),
            topic: Some("academic".into()),
            recency: Some("2024-01-01".into()),
            domains: Some(vec!["nature.com".into(), "-reddit.com".into()]),
            ..Default::default()
        };
        let b = search_body(&input, 500).unwrap();
        assert_eq!(b["category"], "research paper");
        assert_eq!(b["startPublishedDate"], "2024-01-01");
        assert_eq!(b["includeDomains"], json!(["nature.com"]));
        assert_eq!(b["excludeDomains"], json!(["reddit.com"]));
    }

    #[test]
    fn research_is_synchronous_deep() {
        let b = research_body(&Input {
            query: Some("q".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(b["type"], "deep-reasoning");
        assert_eq!(b["contents"]["text"]["maxCharacters"], 8000);
    }
}
