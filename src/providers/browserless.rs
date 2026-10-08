use serde_json::{json, Value};

use super::{check, header_cost, Capability, Input, LiveBalance, Outcome};
use crate::error::{Error, Result};

pub async fn call(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    // The token rides in the query string, so keep it out of the stored trace and error text.
    let resp = client
        .post(format!("{base}/stealth/bql"))
        .query(&[("token", key)])
        .json(&body(cap, input.need_url()?))
        .send()
        .await
        .map_err(reqwest::Error::without_url)?;
    let resp = check("browserless", resp).await?;
    let cost = header_cost(&resp, "X-Browserless-Units");
    let v: Value = resp.json().await.map_err(reqwest::Error::without_url)?;
    Ok(Outcome::new(page(&v)?, cost))
}

// `browser` waits for the network to idle plus 1.5 s; a read takes the DOM once it parses.
fn body(cap: Capability, url: &str) -> Value {
    let (wait_until, settle) = if cap == Capability::Browser {
        ("networkIdle", " waitForTimeout(time: 1500) { time }")
    } else {
        ("domContentLoaded", "")
    };
    json!({
        "query": format!(
            "mutation Read($url: String!) {{ goto(url: $url, waitUntil: {wait_until}, timeout: 30000) {{ status }}{settle} markdown {{ markdown }} }}"
        ),
        "variables": { "url": url },
        "operationName": "Read",
    })
}

// A dead domain answers 200 with no status; `goto.status` is the target's, not a rejected token.
fn page(v: &Value) -> Result<String> {
    let status = v["data"]["goto"]["status"].as_u64().unwrap_or(0);
    if !(1..400).contains(&status) {
        return Err(Error::Provider {
            provider: "browserless",
            status: 0,
            body: match v["errors"][0]["message"].as_str() {
                Some(msg) => msg.to_string(),
                None if status > 0 => format!("page answered {status}"),
                None => "page failed to load".into(),
            },
        });
    }
    v["data"]["markdown"]["markdown"]
        .as_str()
        .filter(|t| !t.is_empty())
        .map(String::from)
        .ok_or(Error::BadResponse("browserless"))
}

// Billing lives on api.browserless.io, not the browser region host.
fn api(base: &str) -> String {
    base.replace("production-sfo.browserless.io", "api.browserless.io")
}

/// `GET /v1/account/usage` is free; its `used` lags the billed ledger, `remaining` doesn't.
pub async fn balance(base: &str, key: &str, client: &reqwest::Client) -> Result<LiveBalance> {
    let resp = client
        .get(format!("{}/v1/account/usage", api(base)))
        .query(&[("token", key)])
        .send()
        .await
        .map_err(reqwest::Error::without_url)?;
    parse_balance(
        &check("browserless", resp)
            .await?
            .json()
            .await
            .map_err(reqwest::Error::without_url)?,
    )
}

fn parse_balance(v: &Value) -> Result<LiveBalance> {
    let remaining = v["units"]["remaining"]
        .as_i64()
        .ok_or(Error::BadResponse("browserless"))?;
    Ok(LiveBalance {
        remaining,
        total: v["units"]["included"].as_i64().unwrap_or(remaining),
        usd: None,
        reset_at: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_waits_for_idle_and_settles() {
        let read = body(Capability::Read, "https://example.com");
        let q = read["query"].as_str().unwrap();
        assert!(q.contains("waitUntil: domContentLoaded") && !q.contains("waitForTimeout"));
        assert_eq!(read["variables"]["url"], "https://example.com");
        let q = body(Capability::Browser, "https://example.com")["query"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(q.contains("waitUntil: networkIdle") && q.contains("waitForTimeout(time: 1500)"));
    }

    #[test]
    fn failed_navigation_is_an_error() {
        let ok = json!({"data": {"goto": {"status": 200}, "markdown": {"markdown": "# Example"}}});
        assert_eq!(page(&ok).unwrap(), "# Example");
        let dead = json!({"data": {"goto": {"status": null}, "markdown": {"markdown": "This site can’t be reached"}}});
        assert!(matches!(
            page(&dead),
            Err(Error::Provider { status: 0, .. })
        ));
        let walled = json!({"data": {"goto": {"status": 401}, "markdown": {"markdown": "Requires authentication"}}});
        assert!(matches!(
            page(&walled),
            Err(Error::Provider { status: 0, body, .. }) if body == "page answered 401"
        ));
        let timeout = json!({"errors": [{"message": "Timed out"}], "data": null});
        assert!(matches!(
            page(&timeout),
            Err(Error::Provider { body, .. }) if body == "Timed out"
        ));
    }

    #[test]
    fn parses_units() {
        let b = parse_balance(&json!({"units": {"included": 1000, "used": 9, "remaining": 983}}))
            .unwrap();
        assert_eq!((b.remaining, b.total), (983, 1000));
        assert!(parse_balance(&json!({"error": "Invalid API token"})).is_err());
        assert_eq!(
            api("https://production-sfo.browserless.io"),
            "https://api.browserless.io"
        );
    }

    #[tokio::test]
    async fn bills_the_reported_units() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/stealth/bql"))
            .and(query_param("token", "k"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("X-Browserless-Units", "2")
                    .set_body_json(json!({"data": {"goto": {"status": 200}, "markdown": {"markdown": "# Slow"}}})),
            )
            .mount(&server)
            .await;
        let input = Input {
            url: Some("https://example.com".into()),
            ..Default::default()
        };
        let out = call(
            &server.uri(),
            "k",
            &reqwest::Client::new(),
            Capability::Browser,
            &input,
        )
        .await
        .unwrap();
        assert_eq!((out.text.as_str(), out.cost), ("# Slow", 2));
    }

    #[tokio::test]
    async fn token_stays_out_of_transport_errors() {
        use wiremock::{matchers::any, Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
            .mount(&server)
            .await;
        let input = Input {
            url: Some("https://example.com".into()),
            ..Default::default()
        };
        let client = reqwest::Client::new();
        for base in ["http://127.0.0.1:9", &server.uri()] {
            let err = call(base, "secret-token", &client, Capability::Read, &input)
                .await
                .err()
                .unwrap();
            assert!(!err.to_string().contains("secret-token"));
            let err = balance(base, "secret-token", &client).await.err().unwrap();
            assert!(!err.to_string().contains("secret-token"));
        }
    }
}
