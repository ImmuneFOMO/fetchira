use serde_json::{json, Value};

use super::{check, fmt_hits, niche, s, Capability, Hit, Input, Outcome};
use crate::error::{Error, Result};

// Read and research need a payment method on the account, so only search is wired.
pub async fn call(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    if cap != Capability::Search {
        return Err(Error::Unsupported("octen"));
    }
    let (path, body, cost) = build_req(input)?;
    let resp = crate::httptrace::send_traced(
        client
            .post(format!("{base}/{path}"))
            .header("x-api-key", key)
            .json(&body),
    )
    .await?;
    let v: Value = check("octen", resp)
        .await
        .map_err(no_balance)?
        .json()
        .await?;
    let hits = v["data"]["results"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|o| Hit {
                    title: s(o, "title"),
                    url: s(o, "url"),
                    snippet: s(o, "highlight")
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(Outcome::new(fmt_hits(&hits), cost))
}

// The default 512-token highlight runs to ~2 KB, so cap it to snippet size.
fn build_req(input: &Input) -> Result<(&'static str, Value, i64)> {
    let mut b = json!({
        "query": input.need_query()?,
        "count": input.results(),
        "highlight": { "enable": true, "max_tokens": 200 },
    });
    if let Some(domains) = &input.domains {
        let (inc, exc) = niche::split_domains(domains);
        if !inc.is_empty() {
            b["include_domains"] = json!(inc);
        }
        if !exc.is_empty() {
            b["exclude_domains"] = json!(exc);
        }
    }
    match input.recency.as_deref() {
        Some(r @ ("day" | "week" | "month" | "year")) => b["time_range"] = json!(r),
        // A date-only start_time is a 400.
        Some(iso) => {
            if let Some(d) = niche::recency_date(iso) {
                b["start_time"] = json!(format!("{d}T00:00:00Z"));
            }
        }
        None => {}
    }
    if input.topic.as_deref() == Some("news") {
        // Subject clustering is on by default and bills an extra query.
        b["subjects"] = json!({ "enable": false });
        return Ok(("news-search", b, 3));
    }
    b["query"] = json!(input.need_query()?.to_string() + niche::topic_hint(input));
    Ok(("search", b, 1))
}

// 403 doubles as the payment-method gate; only the balance message means the credit is spent.
fn no_balance(e: Error) -> Error {
    match e {
        Error::Provider {
            status: 403, body, ..
        } if body.contains("Insufficient balance") => {
            Error::QuotaExceeded(format!("octen: {body}"))
        }
        e => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_web_request() {
        let (path, b, cost) = build_req(&Input {
            query: Some("tokio".into()),
            recency: Some("2026-09-01".into()),
            domains: Some(vec!["tokio.rs".into(), "-reddit.com".into()]),
            ..Default::default()
        })
        .unwrap();
        assert_eq!((path, cost), ("search", 1));
        assert_eq!(b["count"], 5);
        assert_eq!(b["highlight"]["max_tokens"], 200);
        assert_eq!(b["start_time"], "2026-09-01T00:00:00Z");
        assert_eq!(b["include_domains"], json!(["tokio.rs"]));
        assert_eq!(b["exclude_domains"], json!(["reddit.com"]));
        assert!(b.get("subjects").is_none() && b.get("time_range").is_none());
    }

    #[test]
    fn news_goes_to_news_search() {
        let (path, b, cost) = build_req(&Input {
            query: Some("fed rates".into()),
            topic: Some("news".into()),
            recency: Some("week".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!((path, cost), ("news-search", 3));
        assert_eq!(b["query"], "fed rates");
        assert_eq!(b["time_range"], "week");
        assert_eq!(b["subjects"]["enable"], false);
    }

    #[test]
    fn academic_folds_into_the_query() {
        let (path, b, _) = build_req(&Input {
            query: Some("crispr".into()),
            topic: Some("academic".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(path, "search");
        assert_eq!(b["query"], "crispr (peer-reviewed OR research paper)");
    }

    #[test]
    fn only_insufficient_balance_is_quota() {
        let forbidden = |body: &str| Error::Provider {
            provider: "octen",
            status: 403,
            body: body.into(),
        };
        assert!(matches!(
            no_balance(forbidden(
                r#"{"code":403,"msg":"Insufficient balance in account"}"#
            )),
            Error::QuotaExceeded(_)
        ));
        assert!(matches!(
            no_balance(forbidden(
                r#"{"code":403,"msg":"Payment method required."}"#
            )),
            Error::Provider { status: 403, .. }
        ));
    }
}
