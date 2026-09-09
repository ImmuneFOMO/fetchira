use base64::Engine;
use serde_json::{json, Value};

use super::{check, Capability, Input, LiveBalance, OutImage, Outcome};
use crate::error::{Error, Result};

// Browser Tools provide single-page content and artifacts. Multi-step browser control is
// outside this provider. Proxy bandwidth is billed separately from the per-tool charge.
pub async fn call(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    _cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    match input.mode.as_deref() {
        Some("screenshot") => artifact(base, key, client, input, "screenshot", "image/png").await,
        Some("pdf") => artifact(base, key, client, input, "pdf", "application/pdf").await,
        Some(other) => Err(Error::Provider {
            provider: "steel",
            status: 400,
            body: format!("unknown mode '{other}'; valid modes: screenshot, pdf"),
        }),
        None => scrape(base, key, client, input).await,
    }
}

async fn scrape(base: &str, key: &str, client: &reqwest::Client, input: &Input) -> Result<Outcome> {
    let resp = crate::httptrace::send_traced(
        client
            .post(format!("{base}/v1/scrape"))
            .header("steel-api-key", key)
            .json(&scrape_body(input.need_url()?)),
    )
    .await?;
    let v: Value = check("steel", resp).await?.json().await?;
    let text = v["content"]["markdown"]
        .as_str()
        .filter(|t| !t.is_empty())
        .ok_or(Error::BadResponse("steel"))?
        .to_string();
    Ok(Outcome::new(text, 1))
}

// Let JavaScript-rendered content settle before capture.
fn scrape_body(url: &str) -> Value {
    json!({ "url": url, "format": ["markdown"], "useProxy": true, "delay": 1500 })
}

async fn artifact(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    input: &Input,
    format: &str,
    mime: &str,
) -> Result<Outcome> {
    let resp = crate::httptrace::send_traced(
        client
            .post(format!("{base}/v1/{format}"))
            .header("steel-api-key", key)
            .json(&json!({ "url": input.need_url()?, "useProxy": true, "delay": 1500 })),
    )
    .await?;
    let resp = check("steel", resp).await?;
    // Current Steel returns a hosted artifact URL; older/self-hosted versions return bytes.
    let resp = if resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"))
    {
        let v: Value = resp.json().await?;
        let url = v["url"]
            .as_str()
            .ok_or(Error::BadResponse("steel: missing artifact URL"))?;
        check("steel", client.get(url).send().await?).await?
    } else {
        resp
    };
    let bytes = resp.bytes().await?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let mut out = Outcome::new(String::new(), 1);
    out.image = Some(OutImage {
        mime: mime.to_string(),
        b64,
    });
    Ok(out)
}

/// Live credit balance via `POST /v1/usage-details` with the api-key (the gateway routes it as
/// POST, not GET — a GET returns "no route"). The response carries a Stripe `creditBalanceSummary`;
/// sum the available grants (cents), then convert to reads (see `parse_balance`).
pub async fn balance(base: &str, key: &str, client: &reqwest::Client) -> Result<LiveBalance> {
    let resp = client
        .post(format!("{base}/v1/usage-details"))
        .header("steel-api-key", key)
        .header("content-type", "application/json")
        .send()
        .await?;
    let v: Value = check("steel", resp).await?.json().await?;
    parse_balance(&v)
}

// Sum the available credit across Stripe credit-balance grants (`value` is in cents), then estimate
// reads. A read isn't a flat fee: the $0.005 browser-tool charge plus residential-proxy bandwidth
// (~1 MB of a page at ~$10/GB ≈ $0.0098) — we send `useProxy:true` so pages actually render — puts a
// typical proxied read near $0.015. So cents ÷ 1.5 (= cents·2/3) reads; it's an estimate (page-weight
// dependent), shown with "≈".
// ponytail: total = remaining (bar full while funded); a stored high-water-mark would give a
// draining bar, add it if the flat gauge proves confusing.
fn parse_balance(v: &Value) -> Result<LiveBalance> {
    let balances = v["creditBalanceSummary"]["balances"]
        .as_array()
        .ok_or(Error::BadResponse("steel: missing credit balance"))?;
    let cents = balances
        .iter()
        .try_fold(0_i64, |sum, b| {
            b["available_balance"]["monetary"]["value"]
                .as_i64()
                .and_then(|value| sum.checked_add(value))
        })
        .ok_or(Error::BadResponse("steel: invalid credit balance"))?;
    let reads = cents.saturating_mul(2) / 3;
    Ok(LiveBalance {
        remaining: reads,
        total: reads,
        usd: Some(cents as f64 / 100.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credit_summary_to_reads() {
        let v = json!({"creditBalanceSummary": {"object": "billing.credit_balance_summary", "balances": [
            {"available_balance": {"monetary": {"currency": "usd", "value": 1000}, "type": "monetary"}}
        ]}});
        assert_eq!(parse_balance(&v).unwrap().remaining, 666); // $10.00 / ~$0.015 proxied read
        assert!(parse_balance(&json!({})).is_err());
        assert!(parse_balance(&json!({"creditBalanceSummary":{"balances":[{}]}})).is_err());
    }

    #[test]
    fn scrape_body_uses_proxy() {
        let body = scrape_body("https://example.com");
        assert_eq!(body["useProxy"], json!(true));
        assert_eq!(body["delay"], json!(1500));
        assert_eq!(body["url"], json!("https://example.com"));
    }

    #[tokio::test]
    async fn downloads_hosted_artifacts_and_keeps_legacy_bytes() {
        use wiremock::{
            matchers::{method, path},
            Mock, MockServer, ResponseTemplate,
        };
        let server = MockServer::start().await;
        let client = reqwest::Client::new();
        let input = Input {
            url: Some("https://example.org".into()),
            ..Input::default()
        };
        for (format, mime, bytes) in [
            ("screenshot", "image/png", b"PNG artifact".as_slice()),
            ("pdf", "application/pdf", b"%PDF-1.4 artifact".as_slice()),
        ] {
            for hosted_url in [true, false] {
                server.reset().await;
                let response = if hosted_url {
                    Mock::given(method("GET"))
                        .and(path("/artifact"))
                        .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
                        .expect(1)
                        .mount(&server)
                        .await;
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"url":format!("{}/artifact", server.uri())}))
                } else {
                    ResponseTemplate::new(200)
                        .insert_header("content-type", mime)
                        .set_body_bytes(bytes)
                };
                Mock::given(method("POST"))
                    .and(path(format!("/v1/{format}")))
                    .respond_with(response)
                    .expect(1)
                    .mount(&server)
                    .await;
                let out = artifact(&server.uri(), "test-key", &client, &input, format, mime)
                    .await
                    .unwrap();
                let img = out.image.unwrap();
                assert_eq!(img.mime, mime);
                assert_eq!(
                    base64::engine::general_purpose::STANDARD
                        .decode(img.b64)
                        .unwrap(),
                    bytes
                );
                for request in server.received_requests().await.unwrap() {
                    if request.method == "GET" {
                        assert!(!request.headers.contains_key("steel-api-key"));
                    }
                }
            }
        }
        server.reset().await;
        Mock::given(method("POST"))
            .and(path("/v1/pdf"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"url":format!("{}/missing",server.uri())})),
            )
            .mount(&server)
            .await;
        assert!(artifact(
            &server.uri(),
            "test-key",
            &client,
            &input,
            "pdf",
            "application/pdf"
        )
        .await
        .is_err());
    }
}
