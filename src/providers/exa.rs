use std::time::Duration;

use serde_json::{json, Value};

use super::{check, fmt_hits, s, value_to_text, Capability, Hit, Input, LiveBalance, Outcome};
use crate::error::{Error, Result};

const POLL: Duration = Duration::from_secs(3);
const MAX_POLLS: u32 = 60;

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
        .json(&json!({
            "query": input.need_query()?,
            "numResults": input.results(),
            "contents": { "text": { "maxCharacters": 500 } },
        }))
        .send()
        .await?;
    let v: Value = check("exa", resp).await?.json().await?;
    let hits = v["results"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|o| Hit {
                    title: s(o, "title"),
                    url: s(o, "url"),
                    snippet: s(o, "text"),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(Outcome::new(fmt_hits(&hits), 1))
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

async fn research(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    input: &Input,
) -> Result<Outcome> {
    let start = client
        .post(format!("{base}/research/v1"))
        .header("x-api-key", key)
        .json(&json!({ "instructions": input.need_query()?, "model": "exa-research" }))
        .send()
        .await?;
    let v: Value = check("exa", start).await?.json().await?;
    let id = v["researchId"]
        .as_str()
        .ok_or(Error::BadResponse("exa"))?
        .to_string();

    for _ in 0..MAX_POLLS {
        tokio::time::sleep(POLL).await;
        let resp = client
            .get(format!("{base}/research/v1/{id}"))
            .header("x-api-key", key)
            .send()
            .await?;
        let v: Value = check("exa", resp).await?.json().await?;
        match v["status"].as_str().unwrap_or_default() {
            "completed" => {
                let text = v
                    .get("output")
                    .or_else(|| v.get("data"))
                    .or_else(|| v.get("report"))
                    .map(value_to_text)
                    .unwrap_or_else(|| value_to_text(&v));
                return Ok(Outcome::new(text, 1));
            }
            "failed" | "canceled" | "cancelled" => return Err(Error::BadResponse("exa")),
            _ => continue,
        }
    }
    Err(Error::Timeout("exa"))
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
}
