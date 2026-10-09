use serde_json::{json, Value};

use super::{check, iso_ms, Capability, Input, LiveBalance, Outcome};
use crate::error::{Error, Result};

pub async fn call(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    let resp = crate::httptrace::send_traced(
        client
            .post(format!("{base}/api/web/fetch"))
            .header("x-api-key", key)
            .json(&body(cap, input.need_url()?)),
    )
    .await?;
    let v: Value = check("hyperbrowser", resp).await?.json().await?;
    Ok(Outcome::new(page(&v)?, 1))
}

// Every fetch loads in a cloud browser; `browser` also lets the page settle and skips the cache.
fn body(cap: Capability, url: &str) -> Value {
    let mut b = json!({ "url": url, "outputs": { "formats": ["markdown"] } });
    if cap == Capability::Browser {
        b["navigation"] = json!({ "waitUntil": "networkidle", "waitFor": 1500 });
        b["cache"] = json!({ "maxAgeSeconds": 0 });
    }
    b
}

// A failed load still answers 200, with the reason under `error`.
fn page(v: &Value) -> Result<String> {
    if let Some(text) = v["data"]["markdown"].as_str().filter(|t| !t.is_empty()) {
        return Ok(text.to_string());
    }
    let e = v["error"]
        .as_str()
        .ok_or(Error::BadResponse("hyperbrowser"))?;
    Err(Error::Provider {
        provider: "hyperbrowser",
        status: 0,
        body: e.to_string(),
    })
}

/// Credits from the free `/api/team/credit-info`; the renewal date only comes from `/api/me`.
pub async fn balance(base: &str, key: &str, client: &reqwest::Client) -> Result<LiveBalance> {
    let resp = client
        .get(format!("{base}/api/team/credit-info"))
        .header("x-api-key", key)
        .send()
        .await?;
    let credits: Value = check("hyperbrowser", resp).await?.json().await?;
    let me = async {
        let resp = client
            .get(format!("{base}/api/me"))
            .header("x-api-key", key)
            .send()
            .await?;
        Ok::<Value, Error>(check("hyperbrowser", resp).await?.json().await?)
    };
    parse_balance(&credits, &me.await.unwrap_or_default())
}

fn parse_balance(v: &Value, me: &Value) -> Result<LiveBalance> {
    let remaining = v["remaining"]
        .as_i64()
        .ok_or(Error::BadResponse("hyperbrowser"))?;
    Ok(LiveBalance {
        remaining,
        total: v["limit"].as_i64().unwrap_or(remaining),
        usd: None,
        reset_at: me["user"]["personalTeam"]["billingEnd"]
            .as_str()
            .and_then(iso_ms),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_settles_and_skips_cache() {
        let read = body(Capability::Read, "https://example.com");
        assert_eq!(read["outputs"]["formats"], json!(["markdown"]));
        assert!(read.get("navigation").is_none() && read.get("cache").is_none());
        let b = body(Capability::Browser, "https://example.com");
        assert_eq!(b["navigation"]["waitUntil"], "networkidle");
        assert_eq!(b["navigation"]["waitFor"], 1500);
        assert_eq!(b["cache"]["maxAgeSeconds"], 0);
        assert!(b.get("stealth").is_none() && b.get("browser").is_none());
    }

    #[test]
    fn failed_load_is_a_provider_error() {
        let ok = json!({"status": "completed", "data": {"markdown": "# Example"}});
        assert_eq!(page(&ok).unwrap(), "# Example");
        let failed = json!({"status": "failed", "data": {}, "error": "net::ERR_NAME_NOT_RESOLVED"});
        assert!(matches!(
            page(&failed),
            Err(Error::Provider { body, .. }) if body == "net::ERR_NAME_NOT_RESOLVED"
        ));
        assert!(matches!(
            page(&json!({"data": {"markdown": ""}})),
            Err(Error::BadResponse(_))
        ));
    }

    #[test]
    fn parses_credit_info() {
        let me = json!({"user": {"personalTeam": {"billingEnd": "2026-11-07T00:00:00.000Z"}}});
        let b =
            parse_balance(&json!({"usage": 27, "limit": 5000, "remaining": 4973}), &me).unwrap();
        assert_eq!(
            (b.remaining, b.total, b.reset_at),
            (4973, 5000, Some(1_794_009_600_000))
        );
        assert!(parse_balance(&json!({"error": "NOT AUTHENTICATED"}), &me).is_err());
    }

    #[tokio::test]
    async fn failed_renewal_lookup_keeps_the_balance() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/team/credit-info"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"usage": 27, "limit": 5000, "remaining": 4973})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/me"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let b = balance(&server.uri(), "k", &reqwest::Client::new())
            .await
            .unwrap();
        assert_eq!((b.remaining, b.total, b.reset_at), (4973, 5000, None));
    }
}
