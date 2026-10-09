use serde_json::{json, Value};

use super::{check, fmt_hits, iso_ms, niche, s, Capability, Hit, Input, LiveBalance, Outcome};
use crate::error::{Error, Result};

const SNIPPET_CHARS: usize = 500;

pub async fn call(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    if cap != Capability::Search {
        return Err(Error::Unsupported("nimble"));
    }
    let resp = crate::httptrace::send_traced(
        client
            .post(format!("{base}/v2/search"))
            .bearer_auth(key)
            .json(&body(input)?),
    )
    .await?;
    let v: Value = check("nimble", resp).await?.json().await?;
    Ok(Outcome::new(fmt_hits(&hits(&v)), 1))
}

// `focus` only works on lite depth and the news focus 500s, so both fall back to a query hint.
fn body(input: &Input) -> Result<Value> {
    let standard = match input.mode.as_deref() {
        None => false,
        Some("standard") => true,
        Some(other) => {
            return Err(Error::Provider {
                provider: "nimble",
                status: 400,
                body: format!("unknown mode '{other}'; valid modes: standard"),
            })
        }
    };
    let mut q = input.need_query()?.to_string();
    let mut focus = None;
    match input.topic.as_deref() {
        Some("academic") if !standard => focus = Some("academic"),
        _ => q.push_str(niche::topic_hint(input)),
    }
    let mut b = json!({
        "query": q,
        "max_results": input.results(),
        "search_depth": if standard { "standard" } else { "lite" },
    });
    if let Some(f) = focus {
        b["focus"] = json!(f);
    }
    // time_range and start_date together are a 422.
    match input.recency.as_deref() {
        Some(r @ ("day" | "week" | "month" | "year")) => b["time_range"] = json!(r),
        Some(iso) => {
            if let Some(d) = niche::recency_date(iso) {
                b["start_date"] = json!(d);
            }
        }
        None => {}
    }
    if let Some(domains) = &input.domains {
        let (inc, exc) = niche::split_domains(domains);
        if !inc.is_empty() {
            b["include_domains"] = json!(inc);
        }
        if !exc.is_empty() {
            b["exclude_domains"] = json!(exc);
        }
    }
    Ok(b)
}

// Standard depth puts 10k+ chars of page markdown in `description`.
fn hits(v: &Value) -> Vec<Hit> {
    v["results"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|o| Hit {
            title: s(o, "title"),
            url: s(o, "url"),
            snippet: s(o, "description")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(SNIPPET_CHARS)
                .collect(),
        })
        .collect()
}

// Account details live on the platform host, not the search one.
fn api(base: &str) -> String {
    base.replace("sdk.nimbleway.com", "api.nimbleway.com")
}

/// `GET /api/v1/account/details` is free; the free tier is a one-time 12-month trial.
pub async fn balance(base: &str, key: &str, client: &reqwest::Client) -> Result<LiveBalance> {
    let resp = client
        .get(format!("{}/api/v1/account/details", api(base)))
        .bearer_auth(key)
        .send()
        .await?;
    parse_balance(&check("nimble", resp).await?.json().await?)
}

fn parse_balance(v: &Value) -> Result<LiveBalance> {
    let trial = &v["services"]["data"]["trial"];
    let remaining = trial["remaining_quota"]
        .as_i64()
        .ok_or(Error::BadResponse("nimble"))?;
    // An ended trial keeps its unused quota, but every search 402s.
    let ended = trial["status"].as_str().is_some_and(|s| s != "active")
        || trial["trial_period_end"]
            .as_str()
            .and_then(iso_ms)
            .is_some_and(|end| end <= chrono::Utc::now().timestamp_millis());
    Ok(LiveBalance {
        remaining: if ended { 0 } else { remaining },
        total: trial["quota_granted"].as_i64().unwrap_or(remaining),
        usd: None,
        reset_at: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lite_by_default_with_native_filters() {
        let b = body(&Input {
            query: Some("crispr".into()),
            topic: Some("academic".into()),
            recency: Some("2026-01-01".into()),
            domains: Some(vec!["nature.com".into(), "-reddit.com".into()]),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(b["query"], "crispr");
        assert_eq!(b["search_depth"], "lite");
        assert_eq!(b["focus"], "academic");
        assert_eq!(b["max_results"], 5);
        assert_eq!(b["start_date"], "2026-01-01");
        assert_eq!(b["include_domains"], json!(["nature.com"]));
        assert_eq!(b["exclude_domains"], json!(["reddit.com"]));
        assert!(b.get("time_range").is_none());
    }

    #[test]
    fn news_and_standard_fold_topic_into_query() {
        let b = body(&Input {
            query: Some("ecb rates".into()),
            topic: Some("news".into()),
            recency: Some("week".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(b["query"], "ecb rates latest news");
        assert_eq!(b["time_range"], "week");
        assert!(b.get("focus").is_none() && b.get("start_date").is_none());

        let b = body(&Input {
            query: Some("crispr".into()),
            topic: Some("academic".into()),
            mode: Some("standard".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(b["search_depth"], "standard");
        assert_eq!(b["query"], "crispr (peer-reviewed OR research paper)");
        assert!(b.get("focus").is_none());

        assert!(body(&Input {
            query: Some("q".into()),
            mode: Some("deep".into()),
            ..Default::default()
        })
        .is_err());
    }

    #[test]
    fn parses_trial_quota() {
        let b = parse_balance(&json!({"services": {"data": {"trial": {
            "status": "active", "quota_used": 14, "remaining_quota": 4986, "quota_granted": 5000,
            "trial_period_end": "2027-10-07T09:19:48.261Z"
        }}}}))
        .unwrap();
        assert_eq!((b.remaining, b.total, b.reset_at), (4986, 5000, None));
        for (status, end) in [
            ("expired", "2027-10-07T09:19:48.261Z"),
            ("active", "2025-10-07T09:19:48.261Z"),
        ] {
            let b = parse_balance(&json!({"services": {"data": {"trial": {
                "status": status, "remaining_quota": 4986, "quota_granted": 5000,
                "trial_period_end": end
            }}}}))
            .unwrap();
            assert_eq!((b.remaining, b.total), (0, 5000));
        }
        assert!(parse_balance(&json!({"detail": "Unauthorized"})).is_err());
        assert_eq!(
            api("https://sdk.nimbleway.com"),
            "https://api.nimbleway.com"
        );
    }

    #[test]
    fn standard_description_is_trimmed() {
        let long = format!("# tokio::select!\n{}", "x ".repeat(5000));
        let h = hits(&json!({"results": [
            {"title": "select", "url": "https://docs.rs/tokio", "description": long}
        ]}));
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].snippet.chars().count(), SNIPPET_CHARS);
        assert!(h[0].snippet.starts_with("# tokio::select! x x"));
    }
}
