use serde_json::Value;

use super::{check, Capability, Input, LiveBalance, Outcome};
use crate::error::{Error, Result};

const TOKENS_PER_READ: i64 = 4000;

pub async fn call(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    _cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    // A bare `#` would end our own URL, losing the target's hash route.
    let mut req = client
        .get(format!("{base}/{}", input.need_url()?.replace('#', "%23")))
        .header("Accept", "application/json");
    if !key.is_empty() {
        req = req.bearer_auth(key);
    }
    let req = match input.mode.as_deref() {
        None => req,
        Some("screenshot") => req.header("X-Return-Format", "screenshot"),
        // `all` keeps every link in page order; `true` dedupes them into a text-keyed map.
        Some("links") => req.header("X-With-Links-Summary", "all"),
        Some(other) => {
            return Err(Error::Provider {
                provider: "jina",
                status: 400,
                body: format!("unknown mode '{other}'; valid modes: screenshot, links"),
            })
        }
    };
    let resp = crate::httptrace::send_traced(req).await?;
    let v: Value = check("jina", resp).await?.json().await?;
    Ok(Outcome::new(parse(&v["data"])?, 1))
}

fn parse(d: &Value) -> Result<String> {
    let mut text = [&d["content"], &d["screenshotUrl"]]
        .iter()
        .find_map(|f| f.as_str())
        .filter(|t| !t.is_empty())
        .ok_or(Error::BadResponse("jina"))?
        .to_string();
    if let Some(links) = d["links"].as_array() {
        text.push_str("\n\nLinks:");
        for l in links {
            text.push_str(&format!(
                "\n- [{}]({})",
                l[0].as_str().unwrap_or_default(),
                l[1].as_str().unwrap_or_default()
            ));
        }
    }
    Ok(text)
}

/// Token wallet in reads.
pub async fn balance(key: &str, client: &reqwest::Client) -> Result<LiveBalance> {
    if key.is_empty() {
        return Err(Error::MissingArg("api_key"));
    }
    // The key rides in the query string, so transport errors must not carry the url.
    let resp = client
        .get("https://embeddings-dashboard-api.jina.ai/api/v1/api_key/user")
        .query(&[("api_key", key)])
        .send()
        .await
        .map_err(reqwest::Error::without_url)?;
    parse_balance(
        &check("jina", resp)
            .await?
            .json()
            .await
            .map_err(reqwest::Error::without_url)?,
    )
}

// Jina lets the balance go negative rather than hard-blocking, so floor at 0 reads.
fn parse_balance(v: &Value) -> Result<LiveBalance> {
    let tokens = v["wallet"]["total_balance"]
        .as_i64()
        .ok_or(Error::BadResponse("jina"))?
        .max(0);
    Ok(LiveBalance {
        remaining: tokens / TOKENS_PER_READ,
        total: 10_000_000 / TOKENS_PER_READ,
        usd: None,
        reset_at: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_content_screenshot_and_links() {
        assert_eq!(
            parse(&json!({"content": "# Example"})).unwrap(),
            "# Example"
        );
        assert_eq!(
            parse(&json!({"screenshotUrl": "https://s/1.png"})).unwrap(),
            "https://s/1.png"
        );
        let linked = parse(&json!({
            "content": "body",
            "links": [["Learn more", "https://iana.org"], ["", "https://a.b"]]
        }))
        .unwrap();
        assert_eq!(
            linked,
            "body\n\nLinks:\n- [Learn more](https://iana.org)\n- [](https://a.b)"
        );
        assert!(parse(&json!({"content": ""})).is_err());
    }

    #[test]
    fn parses_wallet_and_floors_negative() {
        let full = parse_balance(&json!({"wallet": {"total_balance": 10_000_000}})).unwrap();
        assert_eq!(full.remaining, 2500);
        assert_eq!(full.total, 2500);
        let overdrawn = parse_balance(&json!({"wallet": {"total_balance": -467579}})).unwrap();
        assert_eq!(overdrawn.remaining, 0);
        assert!(parse_balance(&json!({"detail": "Invalid API key."})).is_err());
    }

    #[tokio::test]
    async fn keyless_sends_no_auth() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, Request, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/https://example.com"))
            .and(header("Accept", "application/json"))
            .and(|r: &Request| !r.headers.contains_key("authorization"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "data": { "content": "# Example" } })),
            )
            .mount(&server)
            .await;
        let input = Input {
            url: Some("https://example.com".into()),
            ..Default::default()
        };
        let out = call(
            &server.uri(),
            "",
            &reqwest::Client::new(),
            Capability::Read,
            &input,
        )
        .await
        .unwrap();
        assert_eq!(out.text, "# Example");
    }

    #[tokio::test]
    async fn keeps_the_hash_route() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/https://docs.example.com/%23/guide/install"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "data": { "content": "# Install" } })),
            )
            .mount(&server)
            .await;
        let input = Input {
            url: Some("https://docs.example.com/#/guide/install".into()),
            ..Default::default()
        };
        let out = call(
            &server.uri(),
            "",
            &reqwest::Client::new(),
            Capability::Read,
            &input,
        )
        .await
        .unwrap();
        assert_eq!(out.text, "# Install");
    }
}
