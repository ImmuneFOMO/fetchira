use serde_json::{json, Value};

use super::{check, Capability, Input, LiveBalance, Outcome};
use crate::error::{Error, Result};

// ponytail: v1 drives Steel's REST /v1/scrape only (clean markdown of one page). Multi-step
// CDP/Playwright automation (the `actions` arg) is deferred to v2. /v1/scrape is a flat charge
// ($0.005 = 1 credit), not metered session time.
pub async fn call(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    _cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    let resp = client
        .post(format!("{base}/v1/scrape"))
        .header("steel-api-key", key)
        .json(&json!({ "url": input.need_url()?, "format": ["markdown"] }))
        .send()
        .await?;
    let v: Value = check("steel", resp).await?.json().await?;
    let text = v["content"]["markdown"]
        .as_str()
        .filter(|t| !t.is_empty())
        .ok_or(Error::BadResponse("steel"))?
        .to_string();
    Ok(Outcome::new(text, 1))
}

/// Live credit balance via `POST /v1/usage-details` with the api-key (the gateway routes it as
/// POST, not GET — a GET returns "no route"). The response carries a Stripe `creditBalanceSummary`;
/// sum the available grants (cents). A scrape is $0.005, so cents·2 = scrapes.
pub async fn balance(base: &str, key: &str, client: &reqwest::Client) -> Result<LiveBalance> {
    let resp = client
        .post(format!("{base}/v1/usage-details"))
        .header("steel-api-key", key)
        .header("content-type", "application/json")
        .send()
        .await?;
    let v: Value = check("steel", resp).await?.json().await?;
    Ok(parse_balance(&v))
}

// Sum the available credit across Stripe credit-balance grants (`value` is in cents). No fixed
// ceiling exists for a top-up balance, so the gauge tracks the live figure itself.
// ponytail: total = remaining (bar full while funded); a stored high-water-mark would give a
// draining bar, add it if the flat gauge proves confusing.
fn parse_balance(v: &Value) -> LiveBalance {
    let cents: i64 = v["creditBalanceSummary"]["balances"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|b| {
                    b["available_balance"]["monetary"]["value"]
                        .as_i64()
                        .unwrap_or(0)
                })
                .sum()
        })
        .unwrap_or(0);
    let scrapes = cents * 2;
    LiveBalance {
        remaining: scrapes,
        total: scrapes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credit_summary_to_scrapes() {
        let v = json!({"creditBalanceSummary": {"object": "billing.credit_balance_summary", "balances": [
            {"available_balance": {"monetary": {"currency": "usd", "value": 1000}, "type": "monetary"}}
        ]}});
        assert_eq!(parse_balance(&v).remaining, 2000); // $10.00 / $0.005
    }
}
