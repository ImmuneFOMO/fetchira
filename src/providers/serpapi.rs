use serde_json::Value;

use super::{check, fmt_hits, niche, s, Capability, Hit, Input, LiveBalance, Outcome};
use crate::error::{Error, Result};

pub async fn call(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    if cap != Capability::Search {
        return Err(Error::Unsupported("serpapi"));
    }
    let (params, arr) = params(input)?;
    // The key rides in the query string, so keep it out of the stored trace and error text.
    let resp = client
        .get(format!("{base}/search.json"))
        .query(&params)
        .query(&[("api_key", key)])
        .send()
        .await
        .map_err(reqwest::Error::without_url)?;
    let v: Value = check("serpapi", resp)
        .await
        .map_err(out_of_searches)?
        .json()
        .await
        .map_err(reqwest::Error::without_url)?;
    Ok(Outcome::new(fmt_hits(&hits(&v, arr, input)), 1))
}

// Returns the query params and the result-array key. Google takes no domain filter, so domains
// fold into `q`; several include sites must be OR-joined or Google ANDs them into nothing.
fn params(input: &Input) -> Result<(Vec<(&'static str, String)>, &'static str)> {
    let mut q = input.need_query()?.to_string();
    if let Some(domains) = &input.domains {
        let (inc, exc) = niche::split_domains(domains);
        if !inc.is_empty() {
            let sites: Vec<String> = inc.iter().map(|d| format!("site:{d}")).collect();
            q.push_str(&format!(" {}", sites.join(" OR ")));
        }
        for d in exc {
            q.push_str(&format!(" -site:{d}"));
        }
    }
    let recency = input.recency.as_deref();
    if input.topic.as_deref() == Some("academic") {
        let mut p = vec![
            ("engine", "google_scholar".into()),
            ("q", q),
            ("num", input.results().to_string()),
        ];
        // Scholar only filters by year.
        if let Some(d) = recency.and_then(niche::recency_date) {
            p.push(("as_ylo", d[..4].to_string()));
        }
        return Ok((p, "organic_results"));
    }
    let mut p = vec![("engine", "google".into()), ("q", q)];
    if let Some(tbs) = recency.and_then(tbs) {
        p.push(("tbs", tbs));
    }
    // google_news has no snippets and ignores `tbs`; the news tab of plain google has both.
    if input.topic.as_deref() == Some("news") {
        p.push(("tbm", "nws".into()));
        return Ok((p, "news_results"));
    }
    Ok((p, "organic_results"))
}

fn tbs(recency: &str) -> Option<String> {
    niche::recency_tbs(recency).map(String::from).or_else(|| {
        chrono::NaiveDate::parse_from_str(recency, "%Y-%m-%d")
            .ok()
            .map(|d| d.format("cdr:1,cd_min:%-m/%-d/%Y").to_string())
    })
}

// google ignores `num` and `-site:` is only a hint, so trim and filter; citations have no link.
fn hits(v: &Value, arr: &str, input: &Input) -> Vec<Hit> {
    let exc = input
        .domains
        .as_deref()
        .map(|d| niche::split_domains(d).1)
        .unwrap_or_default();
    v[arr]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|o| {
            let Some(url) = o["link"].as_str().and_then(|l| reqwest::Url::parse(l).ok()) else {
                return false;
            };
            let host = url.host_str().unwrap_or_default();
            !exc.iter()
                .any(|d| host == *d || host.ends_with(&format!(".{d}")))
        })
        .take(input.results() as usize)
        .map(|o| Hit {
            title: s(o, "title"),
            url: s(o, "link"),
            snippet: s(o, "snippet"),
        })
        .collect()
}

// 429 is both the hourly cap and the spent plan; only the latter is quota.
fn out_of_searches(e: Error) -> Error {
    match e {
        Error::RateLimit { message, .. } if message.contains("run out of searches") => {
            Error::QuotaExceeded(message)
        }
        e => e,
    }
}

/// `GET /account.json` is free and counts plan plus extra credits in `total_searches_left`.
pub async fn balance(base: &str, key: &str, client: &reqwest::Client) -> Result<LiveBalance> {
    let resp = client
        .get(format!("{base}/account.json"))
        .query(&[("api_key", key)])
        .send()
        .await
        .map_err(reqwest::Error::without_url)?;
    parse_balance(
        &check("serpapi", resp)
            .await?
            .json()
            .await
            .map_err(reqwest::Error::without_url)?,
    )
}

fn parse_balance(v: &Value) -> Result<LiveBalance> {
    let left = v["total_searches_left"]
        .as_i64()
        .ok_or(Error::BadResponse("serpapi"))?;
    Ok(LiveBalance {
        remaining: left,
        total: v["searches_per_month"]
            .as_i64()
            .unwrap_or_default()
            .max(left),
        usd: None,
        reset_at: None,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn get<'a>(p: &'a [(&str, String)], k: &str) -> Option<&'a str> {
        p.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn web_folds_domains_and_maps_recency() {
        let (p, arr) = params(&Input {
            query: Some("tokio".into()),
            recency: Some("2026-01-05".into()),
            domains: Some(vec![
                "docs.rs".into(),
                "tokio.rs".into(),
                "-reddit.com".into(),
            ]),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(arr, "organic_results");
        assert_eq!(get(&p, "engine"), Some("google"));
        assert_eq!(
            get(&p, "q"),
            Some("tokio site:docs.rs OR site:tokio.rs -site:reddit.com")
        );
        assert_eq!(get(&p, "tbs"), Some("cdr:1,cd_min:1/5/2026"));
        assert_eq!(get(&p, "num"), None);
    }

    #[test]
    fn topics_pick_engine() {
        let (p, arr) = params(&Input {
            query: Some("ecb".into()),
            topic: Some("news".into()),
            recency: Some("week".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(arr, "news_results");
        assert_eq!(get(&p, "tbm"), Some("nws"));
        assert_eq!(get(&p, "tbs"), Some("qdr:w"));

        let (p, arr) = params(&Input {
            query: Some("crispr".into()),
            topic: Some("academic".into()),
            recency: Some("2024-03-01".into()),
            max_results: Some(3),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(arr, "organic_results");
        assert_eq!(get(&p, "engine"), Some("google_scholar"));
        assert_eq!(get(&p, "num"), Some("3"));
        assert_eq!(get(&p, "as_ylo"), Some("2024"));
        assert_eq!(get(&p, "tbs"), None);
    }

    #[test]
    fn hits_drop_excluded_and_linkless_rows() {
        let input = Input {
            max_results: Some(2),
            domains: Some(vec!["-reddit.com".into()]),
            ..Default::default()
        };
        let h = hits(
            &json!({"organic_results": [
                {"title": "r", "link": "https://www.reddit.com/r/rust", "snippet": "x"},
                {"title": "[CITATION] c", "snippet": "x"},
                {"title": "a", "link": "https://a.dev/1", "snippet": "sa"},
                {"title": "b", "link": "https://b.dev/2", "snippet": "sb"},
                {"title": "c", "link": "https://c.dev/3", "snippet": "sc"}
            ]}),
            "organic_results",
            &input,
        );
        let urls: Vec<_> = h.iter().map(|h| h.url.as_str()).collect();
        assert_eq!(urls, ["https://a.dev/1", "https://b.dev/2"]);
        assert_eq!(h[0].snippet, "sa");
    }

    #[test]
    fn only_spent_plan_is_quota() {
        let spent = r#"serpapi: {"error":"Your account has run out of searches."}"#;
        assert!(matches!(
            out_of_searches(Error::rate_limit(spent)),
            Error::QuotaExceeded(_)
        ));
        assert!(matches!(
            out_of_searches(Error::rate_limit("serpapi: hourly limit")),
            Error::RateLimit { .. }
        ));
    }

    #[test]
    fn parses_account() {
        let b = parse_balance(
            &json!({"searches_per_month": 250, "plan_searches_left": 246, "total_searches_left": 246}),
        )
        .unwrap();
        assert_eq!((b.remaining, b.total), (246, 250));
        assert!(parse_balance(&json!({"plan_id": "free"})).is_err());
    }

    #[tokio::test]
    async fn key_stays_out_of_transport_errors() {
        use wiremock::{matchers::any, Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
            .mount(&server)
            .await;
        let input = Input {
            query: Some("q".into()),
            ..Default::default()
        };
        let client = reqwest::Client::new();
        for base in ["http://127.0.0.1:9", &server.uri()] {
            let err = call(base, "secret-key", &client, Capability::Search, &input)
                .await
                .err()
                .unwrap();
            assert!(!err.to_string().contains("secret-key"));
            let err = balance(base, "secret-key", &client).await.err().unwrap();
            assert!(!err.to_string().contains("secret-key"));
        }
    }
}
