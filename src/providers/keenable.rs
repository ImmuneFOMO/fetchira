use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{check, fmt_hits, iso_ms, niche, s, Capability, Hit, Input, LiveBalance, Outcome};
use crate::error::{Error, Result};

pub async fn call(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    match cap {
        Capability::Search => search(base, key, client, input).await,
        Capability::Read => fetch(base, key, client, input).await,
        _ => Err(Error::Unsupported("keenable")),
    }
}

// Keyless calls go to the `/public` twins, which require an app name.
fn request(
    client: &reqwest::Client,
    method: reqwest::Method,
    base: &str,
    endpoint: &str,
    key: &str,
) -> reqwest::RequestBuilder {
    if key.is_empty() {
        client
            .request(method, format!("{base}/v1/{endpoint}/public"))
            .header("X-Keenable-Title", "fetchira")
    } else {
        client
            .request(method, format!("{base}/v1/{endpoint}"))
            .header("X-API-Key", key)
    }
}

async fn search(base: &str, key: &str, client: &reqwest::Client, input: &Input) -> Result<Outcome> {
    let resp = crate::httptrace::send_traced(
        request(client, reqwest::Method::POST, base, "search", key).json(&body(input)?),
    )
    .await?;
    let resp = check("keenable", resp).await?;
    let live = hourly(resp.headers());
    let v: Value = resp.json().await?;
    let hits = v["results"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|o| Hit {
                    title: s(o, "title"),
                    url: s(o, "url"),
                    snippet: s(o, "snippet"),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut out = Outcome::new(fmt_hits(&hits), 1);
    out.live = live;
    Ok(out)
}

// Only keyless replies carry these: a fixed hour per IP, counted apart for search and fetch.
fn hourly(h: &reqwest::header::HeaderMap) -> Option<LiveBalance> {
    let get = |name: &str| h.get(name)?.to_str().ok();
    Some(LiveBalance {
        remaining: get("x-ratelimit-remaining")?.parse().ok()?,
        total: get("x-ratelimit-limit")?.parse().ok()?,
        usd: None,
        // A reading must expire, or a spent one would bench the account for good.
        reset_at: Some(
            get("x-ratelimit-reset")
                .and_then(iso_ms)
                .unwrap_or_else(|| {
                    (chrono::Utc::now() + chrono::TimeDelta::hours(1)).timestamp_millis()
                }),
        ),
    })
}

/// The one include-only domain keenable filters natively; any other domain mix goes into the query.
pub(super) fn site(input: &Input) -> Option<&str> {
    match input.domains.as_deref() {
        Some([d]) if !d.starts_with('-') => Some(d),
        _ => None,
    }
}

// Snippets default to whole-page excerpts, so cap them to SERP size.
fn body(input: &Input) -> Result<Value> {
    let mut q = input.need_query()?.to_string() + niche::topic_hint(input);
    let site = site(input);
    if site.is_none() {
        q.push_str(&niche::sites(input));
    }
    let mut b = json!({ "query": q, "max_results": input.results(), "snippet_max_length": 500 });
    if let Some(d) = site {
        b["site"] = json!(d);
    }
    if let Some(after) = input.recency.as_deref().and_then(niche::recency_date) {
        b["published_after"] = json!(after);
    }
    if let Some(mode) = input.mode.as_deref() {
        if !matches!(mode, "pro" | "realtime") {
            return Err(Error::Provider {
                provider: "keenable",
                status: 400,
                body: format!("unknown mode '{mode}'; valid modes: pro, realtime"),
            });
        }
        b["mode"] = json!(mode);
    }
    Ok(b)
}

// Without `live=true` keenable only serves its indexed copy and rejects unindexed URLs.
async fn fetch(base: &str, key: &str, client: &reqwest::Client, input: &Input) -> Result<Outcome> {
    let resp = crate::httptrace::send_traced(
        request(client, reqwest::Method::GET, base, "fetch", key)
            .query(&[("url", input.need_url()?), ("live", "true")]),
    )
    .await?;
    let resp = check("keenable", resp).await?;
    let live = hourly(resp.headers());
    let v: Value = resp.json().await?;
    let text = s(&v, "content");
    if text.is_empty() {
        return Err(Error::BadResponse("keenable"));
    }
    let mut out = Outcome::new(text, 1);
    out.live = live;
    Ok(out)
}

pub(super) const AUTH_BASE: &str = "https://caqmfcgrdovyjdhfnwzw.supabase.co";
pub(crate) const STORAGE_KEY: &str = "sb-caqmfcgrdovyjdhfnwzw-auth-token";
// Supabase's public anon key; the console bundle hands it to every visitor.
const ANON_KEY: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6ImNhcW1mY2dyZG92eWpkaGZud3p3Iiwicm9sZSI6ImFub24iLCJpYXQiOjE3NjAwMTc4NTYsImV4cCI6MjA3NTU5Mzg1Nn0.gmf-2JuDTOVL7MAEgMYczSbT1P-PI3X1WOG1SXfhkNg";

/// Console-session balance, sent with the Supabase access token the console keeps in localStorage.
pub async fn balance(
    base: &str,
    auth: &str,
    client: &wreq::Client,
    storage: &BTreeMap<String, String>,
) -> Result<(LiveBalance, Vec<(String, String)>)> {
    let mut sess: Value = storage
        .get(STORAGE_KEY)
        .and_then(|raw| serde_json::from_str(raw).ok())
        .ok_or(Error::BadResponse("keenable"))?;
    let mut updates = Vec::new();
    if sess["expires_at"].as_i64().unwrap_or(0) <= chrono::Utc::now().timestamp() + 60 {
        // The rotation is saved only with a successful read; GoTrue still accepts the previous
        // refresh token, so a lost rotation or a concurrent refresh recovers on the next read.
        sess = read_json(
            client
                .post(format!("{auth}/auth/v1/token?grant_type=refresh_token"))
                .header("apikey", ANON_KEY)
                .header("content-type", "application/json")
                .body(json!({ "refresh_token": sess["refresh_token"] }).to_string())
                .send()
                .await?,
        )
        .await
        .map_err(|e| match e {
            // GoTrue refuses a revoked or reused refresh token with 400: the login is gone.
            Error::Provider {
                status: 400, body, ..
            } => Error::Provider {
                provider: "keenable",
                status: 401,
                body,
            },
            e => e,
        })?;
        updates.push((STORAGE_KEY.to_string(), sess.to_string()));
    }
    let v = read_json(
        client
            .get(format!("{base}/bff/organization/balance"))
            .bearer_auth(sess["access_token"].as_str().unwrap_or_default())
            .send()
            .await?,
    )
    .await?;
    Ok((parse_balance(&v, chrono::Utc::now())?, updates))
}

async fn read_json(resp: wreq::Response) -> Result<Value> {
    if resp.status().as_u16() != 200 {
        return Err(Error::Provider {
            provider: "keenable",
            status: resp.status().as_u16(),
            body: resp.text().await.unwrap_or_default(),
        });
    }
    serde_json::from_str(&resp.text().await.unwrap_or_default())
        .map_err(|_| Error::BadResponse("keenable"))
}

// The allowance renews on each monthly anniversary of the activation date.
fn parse_balance(v: &Value, now: chrono::DateTime<chrono::Utc>) -> Result<LiveBalance> {
    let n = |k: &str| v[k].as_f64().ok_or(Error::BadResponse("keenable"));
    let total = n("free_credits")? + n("paid_credits")?;
    let reset_at = v["activation_date"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .and_then(|start| {
            (1..)
                .filter_map(|m| start.checked_add_months(chrono::Months::new(m)))
                .find(|d| *d > now)
        })
        .map(|d| d.timestamp_millis());
    Ok(LiveBalance {
        remaining: (total - n("charged_spendings")?).max(0.0) as i64,
        total: total as i64,
        usd: None,
        reset_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_request() {
        let b = body(&Input {
            query: Some("crispr".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(b["query"], "crispr");
        assert_eq!(b["max_results"], 5);
        assert!(b.get("site").is_none() && b.get("published_after").is_none());
    }

    #[test]
    fn mode_is_checked() {
        let input = Input {
            query: Some("crispr".into()),
            mode: Some("realtime".into()),
            ..Default::default()
        };
        assert_eq!(body(&input).unwrap()["mode"], "realtime");
        assert!(body(&Input {
            mode: Some("fast".into()),
            ..input
        })
        .is_err());
    }

    #[test]
    fn single_site_is_native_other_domains_fold_into_query() {
        let one = Input {
            query: Some("crispr".into()),
            domains: Some(vec!["nature.com".into()]),
            recency: Some("2024-03-01".into()),
            ..Default::default()
        };
        let b = body(&one).unwrap();
        assert_eq!(b["site"], "nature.com");
        assert_eq!(b["query"], "crispr");
        assert_eq!(b["published_after"], "2024-03-01");

        let mixed = Input {
            query: Some("crispr".into()),
            domains: Some(vec!["nature.com".into(), "-reddit.com".into()]),
            ..Default::default()
        };
        assert_eq!(site(&mixed), None);
        let b = body(&mixed).unwrap();
        assert!(b.get("site").is_none());
        assert_eq!(b["query"], "crispr site:nature.com -site:reddit.com");

        let two = Input {
            domains: Some(vec!["nature.com".into(), "cell.com".into()]),
            ..mixed
        };
        assert_eq!(
            body(&two).unwrap()["query"],
            "crispr site:nature.com OR site:cell.com"
        );
    }

    #[tokio::test]
    async fn hits_use_the_snippet() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [
                {"title": "t", "url": "https://a.dev", "description": "summary", "snippet": "excerpt"}
            ]})))
            .mount(&server)
            .await;
        let input = Input {
            query: Some("q".into()),
            ..Default::default()
        };
        let out = call(
            &server.uri(),
            "k",
            &reqwest::Client::new(),
            Capability::Search,
            &input,
        )
        .await
        .unwrap();
        assert!(out.text.contains("excerpt") && !out.text.contains("summary"));
    }

    #[tokio::test]
    async fn keyless_uses_public_endpoint() {
        use wiremock::matchers::{header, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/fetch/public"))
            .and(header("X-Keenable-Title", "fetchira"))
            .and(query_param("live", "true"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-ratelimit-limit", "1000")
                    .insert_header("x-ratelimit-remaining", "999")
                    .insert_header("x-ratelimit-reset", "2026-10-08T17:18:43.005Z")
                    .set_body_json(json!({ "content": "# Example" })),
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
        let live = out.live.unwrap();
        assert_eq!(
            (live.remaining, live.total, live.reset_at),
            (999, 1000, Some(1_791_479_923_005))
        );
    }

    fn stored(expires_at: i64) -> BTreeMap<String, String> {
        BTreeMap::from([(
            STORAGE_KEY.to_string(),
            json!({"access_token": "old", "refresh_token": "r1", "expires_at": expires_at})
                .to_string(),
        )])
    }

    fn org(activation: &str) -> Value {
        json!({
            "org_id": "o", "activation_date": activation, "period_days": 1,
            "free_credits": 100000, "paid_credits": 0, "search_spendings": 9,
            "fetch_spendings": 1, "charged_spendings": 10
        })
    }

    fn activated_yesterday() -> (String, i64) {
        let act = (chrono::Utc::now() - chrono::TimeDelta::days(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let renews = act.parse::<chrono::DateTime<chrono::Utc>>().unwrap() + chrono::Months::new(1);
        (act, renews.timestamp_millis())
    }

    #[test]
    fn month_end_activation_renews_on_each_month_end() {
        let v = org("2026-01-31T10:00:00Z");
        let at = |now: &str| parse_balance(&v, now.parse().unwrap()).unwrap().reset_at;
        assert_eq!(at("2026-02-15T00:00:00Z"), iso_ms("2026-02-28T10:00:00Z"));
        assert_eq!(at("2026-03-01T00:00:00Z"), iso_ms("2026-03-31T10:00:00Z"));
    }

    #[tokio::test]
    async fn console_balance_renews_monthly_from_activation() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let (act, renews) = activated_yesterday();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/bff/organization/balance"))
            .and(header("authorization", "Bearer old"))
            .respond_with(ResponseTemplate::new(200).set_body_json(org(&act)))
            .mount(&server)
            .await;
        let fresh = stored(chrono::Utc::now().timestamp() + 3600);
        let (b, updates) = balance(&server.uri(), &server.uri(), &wreq::Client::new(), &fresh)
            .await
            .unwrap();
        assert_eq!(
            (b.remaining, b.total, b.usd, b.reset_at),
            (99_990, 100_000, None, Some(renews))
        );
        assert!(updates.is_empty());
    }

    #[tokio::test]
    async fn expired_console_token_is_refreshed_and_saved() {
        use wiremock::matchers::{body_json, header, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let rotated = json!({
            "access_token": "new", "token_type": "bearer", "expires_in": 3600,
            "expires_at": 9_999_999_999i64, "refresh_token": "r2", "user": {"id": "u"}
        });
        let (act, renews) = activated_yesterday();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/token"))
            .and(query_param("grant_type", "refresh_token"))
            .and(header("apikey", ANON_KEY))
            .and(body_json(json!({"refresh_token": "r1"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(rotated.clone()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/bff/organization/balance"))
            .and(header("authorization", "Bearer new"))
            .respond_with(ResponseTemplate::new(200).set_body_json(org(&act)))
            .mount(&server)
            .await;
        let (b, updates) = balance(
            &server.uri(),
            &server.uri(),
            &wreq::Client::new(),
            &stored(0),
        )
        .await
        .unwrap();
        assert_eq!((b.remaining, b.reset_at), (99_990, Some(renews)));
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].0, STORAGE_KEY);
        assert_eq!(
            serde_json::from_str::<Value>(&updates[0].1).unwrap(),
            rotated
        );
    }

    #[tokio::test]
    async fn refused_refresh_is_an_expired_login() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/token"))
            .respond_with(
                ResponseTemplate::new(400).set_body_json(
                    json!({"code": 400, "error_code": "refresh_token_already_used"}),
                ),
            )
            .mount(&server)
            .await;
        let out = balance(
            &server.uri(),
            &server.uri(),
            &wreq::Client::new(),
            &stored(0),
        )
        .await;
        assert!(matches!(out, Err(Error::Provider { status: 401, .. })));
    }

    #[tokio::test]
    async fn failed_read_after_refresh_drops_the_rotation() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/v1/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"access_token": "new", "refresh_token": "r2", "expires_at": 9_999_999_999i64}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/bff/organization/balance"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let out = balance(
            &server.uri(),
            &server.uri(),
            &wreq::Client::new(),
            &stored(0),
        )
        .await;
        assert!(matches!(out, Err(Error::Provider { status: 500, .. })));
    }

    #[test]
    fn reading_without_reset_expires_within_the_hour() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("x-ratelimit-limit", "1000".parse().unwrap());
        h.insert("x-ratelimit-remaining", "0".parse().unwrap());
        h.insert("x-ratelimit-reset", "1791479923".parse().unwrap());
        let now = chrono::Utc::now().timestamp_millis();
        let reset = hourly(&h).unwrap().reset_at.unwrap();
        assert!(reset > now && reset <= now + 3_600_000 + 1000);
    }
}
