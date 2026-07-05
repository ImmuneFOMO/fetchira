use serde_json::{json, Value};

use super::{check, fmt_hits, s, Capability, Hit, Input, LiveBalance, Outcome};
use crate::error::Result;

pub async fn call(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    _cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    let q = input.need_query()?;
    let resp = client
        .post(format!("{base}/search"))
        .header("X-API-KEY", key)
        .json(&json!({ "q": q, "num": input.results() }))
        .send()
        .await?;
    let v: Value = check("serper", resp).await?.json().await?;
    let hits = v["organic"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|o| Hit {
                    title: s(o, "title"),
                    url: s(o, "link"),
                    snippet: s(o, "snippet"),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(Outcome::new(fmt_hits(&hits), 1))
}

/// `GET /account` → `{balance, rateLimit}`. Free, read-only, authoritative (the dashboard's own
/// endpoint). `balance` = remaining credits, 1 per query.
pub async fn balance(base: &str, key: &str, client: &reqwest::Client) -> Result<LiveBalance> {
    let resp = client
        .get(format!("{base}/account"))
        .header("X-API-KEY", key)
        .send()
        .await?;
    Ok(parse_balance(&check("serper", resp).await?.json().await?))
}

// One-time 2,500-credit grant; bought packs accumulate into the same `balance`, so the gauge ceiling
// tracks the larger of the grant and the current balance.
fn parse_balance(v: &Value) -> LiveBalance {
    let remaining = v["balance"].as_i64().unwrap_or(0);
    LiveBalance {
        remaining,
        total: remaining.max(2500),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_account_balance() {
        let b = parse_balance(&json!({"balance": 1629, "rateLimit": 5}));
        assert_eq!(b.remaining, 1629);
        assert_eq!(b.total, 2500);
    }
}
