use serde_json::Value;

use super::{check, header_cost, iso_ms, s, Capability, Input, LiveBalance, Outcome};
use crate::error::{Error, Result};

pub async fn call(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    _cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    let resp = crate::httptrace::send_traced(
        client
            .get(format!("{base}/v2/markdown"))
            .header("x-api-key", key)
            .query(&[("url", input.need_url()?), ("browser", browser(input)?)]),
    )
    .await?;
    let resp = check("scrapingant", resp).await.map_err(classify)?;
    let cost = header_cost(&resp, "ant-credits-cost");
    let v: Value = resp.json().await?;
    let text = s(&v, "markdown");
    if text.trim().is_empty() {
        return Err(Error::BadResponse("scrapingant"));
    }
    Ok(Outcome::new(text, cost))
}

// `browser` defaults to true upstream, so a plain read has to turn it off.
fn browser(input: &Input) -> Result<&'static str> {
    match input.mode.as_deref() {
        None => Ok("false"),
        Some("browser") => Ok("true"),
        Some(other) => Err(Error::Provider {
            provider: "scrapingant",
            status: 400,
            body: format!("unknown mode '{other}'; valid modes: browser"),
        }),
    }
}

// 403 is both a wrong key and a spent plan; 409 is the free plan's one-request concurrency cap.
fn classify(e: Error) -> Error {
    match e {
        Error::Provider {
            provider,
            status: 403,
            body,
        } if body.contains("token is wrong") => Error::Provider {
            provider,
            status: 401,
            body,
        },
        Error::Provider {
            status: 403, body, ..
        } => Error::QuotaExceeded(format!("scrapingant: {body}")),
        Error::Provider {
            status: 409, body, ..
        } => Error::rate_limit(format!("scrapingant: {body}")),
        e => e,
    }
}

/// `GET /v2/usage` is free; the plan renews 30 days after signup, not on the calendar month.
pub async fn balance(base: &str, key: &str, client: &reqwest::Client) -> Result<LiveBalance> {
    let resp = client
        .get(format!("{base}/v2/usage"))
        .header("x-api-key", key)
        .send()
        .await?;
    parse_balance(&check("scrapingant", resp).await?.json().await?)
}

fn parse_balance(v: &Value) -> Result<LiveBalance> {
    let remaining = v["remained_credits"]
        .as_i64()
        .ok_or(Error::BadResponse("scrapingant"))?;
    Ok(LiveBalance {
        remaining,
        total: v["plan_total_credits"].as_i64().unwrap_or(remaining),
        usd: None,
        reset_at: v["end_date"].as_str().and_then(iso_ms),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn mode_picks_browser() {
        let mut input = Input::default();
        assert_eq!(browser(&input).unwrap(), "false");
        input.mode = Some("browser".into());
        assert_eq!(browser(&input).unwrap(), "true");
        input.mode = Some("js".into());
        assert!(browser(&input).is_err());
    }

    #[test]
    fn any_403_but_a_bad_token_is_quota() {
        let err = |status, body: &str| Error::Provider {
            provider: "scrapingant",
            status,
            body: body.into(),
        };
        assert!(matches!(
            classify(err(403, r#"{"detail":"API credits limit exceeded"}"#)),
            Error::QuotaExceeded(_)
        ));
        assert!(matches!(
            classify(err(403, r#"{"detail":"Plan limit reached"}"#)),
            Error::QuotaExceeded(_)
        ));
        assert!(matches!(
            classify(err(403, r#"{"detail":"API token is wrong"}"#)),
            Error::Provider { status: 401, .. }
        ));
        assert!(matches!(
            classify(err(409, "concurrency")),
            Error::RateLimit { .. }
        ));
        assert!(matches!(
            classify(err(423, "anti-bot")),
            Error::Provider { status: 423, .. }
        ));
    }

    #[test]
    fn parses_usage() {
        let b = parse_balance(&json!({
            "plan_total_credits": 10000, "remained_credits": 9986,
            "end_date": "2026-11-06T09:21:38.100056"
        }))
        .unwrap();
        assert_eq!(
            (b.remaining, b.total, b.reset_at),
            (9986, 10000, Some(1_793_956_898_100))
        );
        assert!(parse_balance(&json!({"detail": "x"})).is_err());
    }

    #[tokio::test]
    async fn render_bills_header_cost() {
        use wiremock::matchers::{header, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/markdown"))
            .and(header("x-api-key", "k"))
            .and(query_param("url", "https://example.com"))
            .and(query_param("browser", "true"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("ant-credits-cost", "10")
                    .set_body_json(json!({"url": "https://example.com", "markdown": "# Example"})),
            )
            .mount(&server)
            .await;
        let input = Input {
            url: Some("https://example.com".into()),
            mode: Some("browser".into()),
            ..Default::default()
        };
        let out = call(
            &server.uri(),
            "k",
            &reqwest::Client::new(),
            Capability::Read,
            &input,
        )
        .await
        .unwrap();
        assert_eq!((out.text.as_str(), out.cost), ("# Example", 10));
    }
}
