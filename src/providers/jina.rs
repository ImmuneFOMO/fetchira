use serde_json::Value;

use super::{check, Capability, Input, LiveBalance, Outcome};
use crate::error::{Error, Result};

// A Reader call is billed the tokens in the returned markdown; ~4k tokens is a typical article — the
// heuristic that turns the token wallet into a "≈N reads" figure. ponytail: crude but honest, the
// desc says it's a reader; refine only if users complain the estimate is off.
const TOKENS_PER_READ: i64 = 4000;

pub async fn call(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    _cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    let url = input.need_url()?;
    let resp = client
        .get(format!("{base}/{url}"))
        .bearer_auth(key)
        .header("Accept", "application/json")
        .send()
        .await?;
    let v: Value = check("jina", resp).await?.json().await?;
    let text = v["data"]["content"]
        .as_str()
        .filter(|t| !t.is_empty())
        .ok_or(Error::BadResponse("jina"))?
        .to_string();
    Ok(Outcome::new(text, 1))
}

/// Token wallet, shown as "≈N reads". Lives on the dashboard-API host with query-param auth (the
/// reader host has no balance route). Reflects paid top-ups (they roll into `total_balance`).
pub async fn balance(key: &str, client: &reqwest::Client) -> Result<LiveBalance> {
    let resp = client
        .get(format!(
            "https://embeddings-dashboard-api.jina.ai/api/v1/api_key/user?api_key={key}"
        ))
        .send()
        .await?;
    Ok(parse_balance(&check("jina", resp).await?.json().await?))
}

// Jina lets the balance go negative rather than hard-blocking, so floor at 0 reads.
fn parse_balance(v: &Value) -> LiveBalance {
    let tokens = v["wallet"]["total_balance"].as_i64().unwrap_or(0).max(0);
    LiveBalance {
        remaining: tokens / TOKENS_PER_READ,
        total: 10_000_000 / TOKENS_PER_READ,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wallet_and_floors_negative() {
        let full = parse_balance(&serde_json::json!({"wallet": {"total_balance": 10_000_000}}));
        assert_eq!(full.remaining, 2500);
        assert_eq!(full.total, 2500);
        let overdrawn = parse_balance(&serde_json::json!({"wallet": {"total_balance": -467579}}));
        assert_eq!(overdrawn.remaining, 0);
    }
}
