use serde_json::{json, Value};

use super::{
    check, fmt_hits, niche, s, Capability, Hit, Input, LiveBalance, Outcome, ProviderKind,
};
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
    if let Some(code) = v["code"].as_u64().filter(|&c| c != 0) {
        return Err(no_balance(Error::Provider {
            provider: "octen",
            status: code as u16,
            body: v.to_string(),
        }));
    }
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
    let q = input.need_query()?;
    let mut b = json!({
        "count": input.results(),
        "highlight": { "enable": true, "max_tokens": 200 },
    });
    niche::domain_filter(&mut b, input, "include_domains", "exclude_domains");
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
        b["query"] = json!(q);
        // Subject clustering is on by default and bills an extra query.
        b["subjects"] = json!({ "enable": false });
        return Ok(("news-search", b, 3));
    }
    b["query"] = json!(q.to_string() + niche::topic_hint(input));
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

/// Dashboard-session balance; its CSRF check wants the `x_apt_csrf_token_v2` cookie echoed.
pub async fn balance(
    base: &str,
    client: &wreq::Client,
    cookies: &[crate::web::Cookie],
) -> Result<(LiveBalance, Vec<(String, String)>)> {
    let csrf = cookies
        .iter()
        .find(|c| c.name == "x_apt_csrf_token_v2")
        .ok_or(Error::BadResponse("octen"))?;
    let resp = client
        .post(format!(
            "{}/api/v1/credit/balance",
            base.replace("api.octen.ai", "octen.ai")
        ))
        .header("content-type", "application/json")
        .header("X-Apt-CSRF-Token", csrf.value.as_str())
        .body("{}")
        .send()
        .await?;
    if resp.status().as_u16() != 200 {
        return Err(Error::Provider {
            provider: "octen",
            status: resp.status().as_u16(),
            body: resp.text().await.unwrap_or_default(),
        });
    }
    let updates = crate::web::set_cookie_updates(resp.headers());
    let v: Value = serde_json::from_str(&resp.text().await.unwrap_or_default())
        .map_err(|_| Error::BadResponse("octen"))?;
    Ok((parse_balance(&v)?, updates))
}

fn parse_balance(v: &Value) -> Result<LiveBalance> {
    let usd: f64 = v["data"]["credit_balance"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or(Error::BadResponse("octen"))?;
    Ok(LiveBalance {
        remaining: (usd * 1000.0) as i64,
        total: ProviderKind::Octen.default_quota(),
        usd: Some(usd),
        reset_at: None,
    })
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

    #[tokio::test]
    async fn dashboard_balance_echoes_csrf_cookie() {
        use wiremock::matchers::{body_string, header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/credit/balance"))
            .and(header("X-Apt-CSRF-Token", "tok"))
            .and(body_string("{}"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"data": {"credit_balance": "4.97"}, "code": 0, "msg": ""}),
                ),
            )
            .mount(&server)
            .await;
        let csrf: crate::web::Cookie = serde_json::from_value(
            json!({"name": "x_apt_csrf_token_v2", "value": "tok", "domain": ".octen.ai"}),
        )
        .unwrap();
        let (b, _) = balance(&server.uri(), &wreq::Client::new(), &[csrf])
            .await
            .unwrap();
        assert_eq!((b.remaining, b.total, b.usd), (4970, 5000, Some(4.97)));
        assert!(balance(&server.uri(), &wreq::Client::new(), &[])
            .await
            .is_err());
    }

    #[tokio::test]
    async fn envelope_error_code_is_an_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"code": 403, "msg": "Insufficient balance in account", "request_id": "r"}),
            ))
            .mount(&server)
            .await;
        let input = Input {
            query: Some("tokio".into()),
            ..Default::default()
        };
        let out = call(
            &server.uri(),
            "k",
            &reqwest::Client::new(),
            Capability::Search,
            &input,
        )
        .await;
        assert!(matches!(out, Err(Error::QuotaExceeded(_))));
    }

    #[test]
    fn dust_is_not_a_search() {
        let b = parse_balance(&json!({"data": {"credit_balance": "0.0009"}})).unwrap();
        assert_eq!(b.remaining, 0);
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
