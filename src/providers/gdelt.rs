use serde_json::Value;

use super::{check, fmt_hits, niche, s, Capability, Hit, Input, Outcome};
use crate::error::{Error, Result};

pub async fn call(
    base: &str,
    _key: &str,
    client: &reqwest::Client,
    cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    if cap != Capability::Search {
        return Err(Error::Unsupported("gdelt"));
    }
    let resp = crate::httptrace::send_traced(
        client
            .get(format!("{base}/api/v2/doc/doc"))
            .query(&params(input)?),
    )
    .await?;
    let body = check("gdelt", resp).await?.text().await?;
    Ok(Outcome::new(fmt_hits(&parse(&body)?), 1))
}

fn params(input: &Input) -> Result<Vec<(&'static str, String)>> {
    let mut q = input.need_query()?.to_string();
    if let Some(domains) = &input.domains {
        let (inc, exc) = niche::split_domains(domains);
        match inc[..] {
            [] => {}
            [d] => q.push_str(&format!(" domain:{d}")),
            _ => {
                let any: Vec<String> = inc.iter().map(|d| format!("domain:{d}")).collect();
                q.push_str(&format!(" ({})", any.join(" OR ")));
            }
        }
        for d in exc {
            q.push_str(&format!(" -domain:{d}"));
        }
    }
    let mut p = vec![
        ("query", q),
        ("mode", "artlist".into()),
        ("format", "json".into()),
        ("maxrecords", input.results().to_string()),
        ("sort", "datedesc".into()),
    ];
    match input.recency.as_deref() {
        Some("day") => p.push(("timespan", "1d".into())),
        Some("week") => p.push(("timespan", "1w".into())),
        Some("month") => p.push(("timespan", "1m".into())),
        // The index only spans the last 3 months, which is already the default window.
        Some("year") | None => {}
        Some(iso) => {
            if let Some(d) = niche::recency_date(iso) {
                p.push(("startdatetime", format!("{}000000", d.replace('-', ""))));
            }
        }
    }
    Ok(p)
}

// Query errors (too-short words, bad syntax) come back as a 200 with a plain-text message.
fn parse(body: &str) -> Result<Vec<Hit>> {
    let v: Value = serde_json::from_str(body).map_err(|_| Error::Provider {
        provider: "gdelt",
        status: 400,
        body: body.trim().to_string(),
    })?;
    Ok(v["articles"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|o| Hit {
                    title: s(o, "title"),
                    url: s(o, "url"),
                    snippet: [s(o, "domain"), s(o, "seendate"), s(o, "language")]
                        .into_iter()
                        .filter(|f| !f.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · "),
                })
                .collect()
        })
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(p: &'a [(&str, String)], k: &str) -> Option<&'a str> {
        p.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn builds_niche_params() {
        let p = params(&Input {
            query: Some("ecb rates".into()),
            recency: Some("week".into()),
            domains: Some(vec![
                "bbc.co.uk".into(),
                "ft.com".into(),
                "-reddit.com".into(),
            ]),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            get(&p, "query"),
            Some("ecb rates (domain:bbc.co.uk OR domain:ft.com) -domain:reddit.com")
        );
        assert_eq!(get(&p, "timespan"), Some("1w"));
        assert_eq!(get(&p, "maxrecords"), Some("5"));

        let iso = params(&Input {
            query: Some("ecb".into()),
            recency: Some("2026-09-01".into()),
            domains: Some(vec!["bbc.co.uk".into()]),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(get(&iso, "query"), Some("ecb domain:bbc.co.uk"));
        assert_eq!(get(&iso, "startdatetime"), Some("20260901000000"));
        assert_eq!(get(&iso, "timespan"), None);
    }

    #[test]
    fn parses_articles_and_text_errors() {
        let hits = parse(
            r#"{"articles": [{"url": "https://a.b/x", "title": "T", "seendate": "20261002T104500Z", "domain": "a.b", "language": "English"}]}"#,
        )
        .unwrap();
        assert_eq!(hits[0].url, "https://a.b/x");
        assert_eq!(hits[0].snippet, "a.b · 20261002T104500Z · English");
        assert!(parse("{}").unwrap().is_empty());
        assert!(matches!(
            parse("Your search contained a keyword that was too short.\n"),
            Err(Error::Provider { body, .. }) if body == "Your search contained a keyword that was too short."
        ));
    }
}
