use std::time::Duration;

use serde_json::Value;

use super::{check, header_cost, Capability, Input, LiveBalance, Outcome};
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
            .get(format!("{base}/v1/"))
            .header("X-API-Key", key)
            .query(&[("url", input.need_url()?), ("response_type", "markdown")])
            .query(&params(input)?),
    )
    .await?;
    let resp = check("zenrows", resp).await.map_err(banned)?;
    let cost = header_cost(&resp, "X-Request-Credits");
    let text = resp.text().await?;
    if text.trim().is_empty() {
        return Err(Error::BadResponse("zenrows"));
    }
    Ok(Outcome::new(text, cost))
}

// BLK0001 is a few-minute IP ban for too many errors, not a dead key.
fn banned(e: Error) -> Error {
    match e {
        Error::Provider {
            status: 403, body, ..
        } if body.contains("BLK0001") => {
            Error::rate_limit_after(format!("zenrows: {body}"), Some(Duration::from_secs(300)))
        }
        e => e,
    }
}

// A bare `js_render` can return the pre-hydration DOM, so it always waits.
fn params(input: &Input) -> Result<&'static [(&'static str, &'static str)]> {
    match input.mode.as_deref() {
        None => Ok(&[("mode", "auto")]),
        Some("js") => Ok(&[("js_render", "true"), ("wait", "1500")]),
        Some(other) => Err(Error::Provider {
            provider: "zenrows",
            status: 400,
            body: format!("unknown mode '{other}'; valid modes: js"),
        }),
    }
}

/// `GET /v1/subscriptions/self/details` is free; the period renews a month after signup.
pub async fn balance(base: &str, key: &str, client: &reqwest::Client) -> Result<LiveBalance> {
    let resp = client
        .get(format!("{base}/v1/subscriptions/self/details"))
        .header("X-API-Key", key)
        .send()
        .await?;
    parse_balance(&check("zenrows", resp).await?.json().await?)
}

fn parse_balance(v: &Value) -> Result<LiveBalance> {
    let total = v["credit_limit"]
        .as_i64()
        .ok_or(Error::BadResponse("zenrows"))?;
    Ok(LiveBalance {
        remaining: (total - v["usage_credits"].as_i64().unwrap_or(0)).max(0),
        total,
        usd: None,
        reset_at: None,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn mode_picks_render() {
        let mut input = Input::default();
        assert_eq!(params(&input).unwrap(), &[("mode", "auto")]);
        input.mode = Some("js".into());
        assert_eq!(
            params(&input).unwrap(),
            &[("js_render", "true"), ("wait", "1500")]
        );
        input.mode = Some("browser".into());
        assert!(params(&input).is_err());
    }

    #[test]
    fn ip_ban_cools_down() {
        let forbidden = |body: &str| Error::Provider {
            provider: "zenrows",
            status: 403,
            body: body.into(),
        };
        assert!(matches!(
            banned(forbidden(r#"{"code":"BLK0001","status":403}"#)),
            Error::RateLimit { retry_after: Some(d), .. } if d == Duration::from_secs(300)
        ));
        assert!(matches!(
            banned(forbidden(r#"{"code":"AUTH009","status":403}"#)),
            Error::Provider { status: 403, .. }
        ));
    }

    #[test]
    fn parses_subscription() {
        let b = parse_balance(&json!({"credit_limit": 5000, "usage_credits": 17})).unwrap();
        assert_eq!((b.remaining, b.total), (4983, 5000));
        assert!(parse_balance(&json!({"code": "AUTH002"})).is_err());
    }

    #[tokio::test]
    async fn js_read_bills_header_cost() {
        use wiremock::matchers::{header, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/"))
            .and(header("X-API-Key", "k"))
            .and(query_param("response_type", "markdown"))
            .and(query_param("js_render", "true"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("X-Request-Credits", "5")
                    .set_body_string("# Rendered"),
            )
            .mount(&server)
            .await;
        let input = Input {
            url: Some("https://example.com".into()),
            mode: Some("js".into()),
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
        assert_eq!((out.text.as_str(), out.cost), ("# Rendered", 5));
    }
}
