use serde_json::{json, Value};

use super::{check, fmt_hits, niche, s, Capability, Hit, Input, LiveBalance, Outcome};
use crate::error::Result;

// Maps the niche knobs onto serper's native shape: topic → endpoint (/news, /scholar), recency →
// `tbs`, and domains → `site:`/`-site:` on `q` (serper has no domains param). Returns the path, body
// and the result-array key (news lands under `news`, search/scholar under `organic`).
fn build_req(input: &Input) -> Result<(&'static str, Value, &'static str)> {
    let mut q = input.need_query()?.to_string();
    if let Some(domains) = &input.domains {
        let (inc, exc) = niche::split_domains(domains);
        for d in inc {
            q.push_str(&format!(" site:{d}"));
        }
        for d in exc {
            q.push_str(&format!(" -site:{d}"));
        }
    }
    let (path, arr) = match input.topic.as_deref() {
        Some("news") => ("news", "news"),
        Some("academic") => ("scholar", "organic"),
        _ => ("search", "organic"),
    };
    let mut body = json!({ "q": q, "num": input.results() });
    if let Some(tbs) = input.recency.as_deref().and_then(niche::recency_tbs) {
        body["tbs"] = json!(tbs);
    }
    Ok((path, body, arr))
}

pub async fn call(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    _cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    let (path, body, arr) = build_req(input)?;
    let resp = client
        .post(format!("{base}/{path}"))
        .header("X-API-KEY", key)
        .json(&body)
        .send()
        .await?;
    let v: Value = check("serper", resp).await?.json().await?;
    let hits = v[arr]
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
    fn builds_niche_request() {
        let input = Input {
            query: Some("crispr".into()),
            topic: Some("news".into()),
            recency: Some("week".into()),
            domains: Some(vec!["nature.com".into(), "-reddit.com".into()]),
            ..Default::default()
        };
        let (path, body, arr) = build_req(&input).unwrap();
        assert_eq!(path, "news");
        assert_eq!(arr, "news");
        assert_eq!(body["tbs"], "qdr:w");
        let q = body["q"].as_str().unwrap();
        assert!(q.contains("site:nature.com") && q.contains("-site:reddit.com"));
    }

    #[test]
    fn plain_request_untouched() {
        let input = Input {
            query: Some("crispr".into()),
            ..Default::default()
        };
        let (path, body, arr) = build_req(&input).unwrap();
        assert_eq!(path, "search");
        assert_eq!(arr, "organic");
        assert!(body.get("tbs").is_none());
        assert_eq!(body["q"], "crispr");
    }

    #[test]
    fn parses_account_balance() {
        let b = parse_balance(&json!({"balance": 1629, "rateLimit": 5}));
        assert_eq!(b.remaining, 1629);
        assert_eq!(b.total, 2500);
    }
}
