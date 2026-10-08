use serde_json::Value;

use super::{check, fmt_hits, niche, s, Capability, Hit, Input, LiveBalance, Outcome};
use crate::error::{Error, Result};

const ABSTRACT_WORDS: usize = 40;

pub async fn call(
    base: &str,
    key: &str,
    client: &reqwest::Client,
    cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    if cap != Capability::Search {
        return Err(Error::Unsupported("openalex"));
    }
    if input.domains.is_some() {
        return Err(Error::Provider {
            provider: "openalex",
            status: 400,
            body: "openalex can't filter by domain; drop `domains` or pick another provider".into(),
        });
    }
    let mut req = client.get(format!("{base}/works")).query(&params(input)?);
    if !key.is_empty() {
        req = req.bearer_auth(key);
    }
    let resp = crate::httptrace::send_traced(req).await?;
    let v: Value = check("openalex", resp).await?.json().await?;
    let hits = v["results"]
        .as_array()
        .map(|a| a.iter().map(hit).collect::<Vec<_>>())
        .unwrap_or_default();
    Ok(Outcome::new(fmt_hits(&hits), 1))
}

// Full work records run to tens of KB each; `select` keeps only what a hit shows.
fn params(input: &Input) -> Result<Vec<(&'static str, String)>> {
    let mut q = vec![
        ("search", input.need_query()?.to_string()),
        ("per_page", input.results().to_string()),
        (
            "select",
            "id,doi,display_name,publication_year,primary_location,authorships,cited_by_count,abstract_inverted_index".into(),
        ),
    ];
    if let Some(d) = input.recency.as_deref().and_then(niche::recency_date) {
        q.push(("filter", format!("from_publication_date:{d}")));
    }
    Ok(q)
}

fn hit(o: &Value) -> Hit {
    let authors: Vec<&str> = o["authorships"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| a["author"]["display_name"].as_str())
        .collect();
    let mut meta = Vec::new();
    if !authors.is_empty() {
        let et_al = if authors.len() > 3 { " et al." } else { "" };
        meta.push(format!(
            "{}{et_al}",
            authors[..authors.len().min(3)].join(", ")
        ));
    }
    if let Some(y) = o["publication_year"].as_i64() {
        meta.push(y.to_string());
    }
    if let Some(venue) = o["primary_location"]["source"]["display_name"].as_str() {
        meta.push(venue.to_string());
    }
    meta.push(format!(
        "cited by {}",
        o["cited_by_count"].as_i64().unwrap_or(0)
    ));
    let mut snippet = meta.join(" · ");
    let abs = abstract_start(&o["abstract_inverted_index"]);
    if !abs.is_empty() {
        snippet.push_str(" — ");
        snippet.push_str(&abs);
    }
    Hit {
        title: s(o, "display_name"),
        url: [
            &o["doi"],
            &o["primary_location"]["landing_page_url"],
            &o["id"],
        ]
        .iter()
        .find_map(|f| f.as_str())
        .unwrap_or_default()
        .to_string(),
        snippet,
    }
}

// Abstracts ship as an inverted index (word → positions).
fn abstract_start(idx: &Value) -> String {
    let mut words: Vec<(u64, &str)> = idx
        .as_object()
        .into_iter()
        .flatten()
        .flat_map(|(w, ps)| {
            ps.as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p.as_u64())
                .map(move |p| (p, w.as_str()))
        })
        .collect();
    words.sort_unstable();
    let mut text = words
        .iter()
        .take(ABSTRACT_WORDS)
        .map(|(_, w)| *w)
        .collect::<Vec<_>>()
        .join(" ");
    if words.len() > ABSTRACT_WORDS {
        text.push('…');
    }
    text
}

/// `GET /rate-limit` (key only) → today's dollar budget, in searches.
pub async fn balance(base: &str, key: &str, client: &reqwest::Client) -> Result<LiveBalance> {
    if key.is_empty() {
        return Err(Error::MissingArg("api_key"));
    }
    let resp = client
        .get(format!("{base}/rate-limit"))
        .query(&[("api_key", key)])
        .send()
        .await?;
    parse_balance(&check("openalex", resp).await?.json().await?)
}

fn parse_balance(v: &Value) -> Result<LiveBalance> {
    let r = &v["rate_limit"];
    let remaining = r["daily_remaining_usd"]
        .as_f64()
        .ok_or(Error::BadResponse("openalex"))?;
    let cost = r["endpoint_costs_usd"]["search"]
        .as_f64()
        .filter(|c| *c > 0.0)
        .unwrap_or(0.001);
    Ok(LiveBalance {
        remaining: (remaining / cost).round() as i64,
        total: (r["daily_budget_usd"].as_f64().unwrap_or(remaining) / cost).round() as i64,
        usd: None,
        reset_at: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn domains_are_refused_not_dropped() {
        let input = Input {
            query: Some("crispr".into()),
            domains: Some(vec!["nature.com".into()]),
            ..Default::default()
        };
        let err = call(
            "http://127.0.0.1:9",
            "",
            &reqwest::Client::new(),
            Capability::Search,
            &input,
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(err, Error::Provider { status: 400, .. }));
    }

    #[test]
    fn builds_params() {
        let input = Input {
            query: Some("crispr".into()),
            recency: Some("2024-03-01".into()),
            ..Default::default()
        };
        let q = params(&input).unwrap();
        assert!(q.contains(&("search", "crispr".into())));
        assert!(q.contains(&("per_page", "5".into())));
        assert!(q.contains(&("filter", "from_publication_date:2024-03-01".into())));
        let plain = params(&Input {
            query: Some("crispr".into()),
            ..Default::default()
        })
        .unwrap();
        assert!(plain.iter().all(|(k, _)| *k != "filter"));
    }

    #[test]
    fn parses_work_into_hit() {
        let h = hit(&json!({
            "id": "https://openalex.org/W1",
            "doi": "https://doi.org/10.1/x",
            "display_name": "Scaling Laws",
            "publication_year": 2020,
            "cited_by_count": 42,
            "primary_location": { "source": { "display_name": "arXiv" } },
            "authorships": [
                { "author": { "display_name": "A" } },
                { "author": { "display_name": "B" } },
                { "author": { "display_name": "C" } },
                { "author": { "display_name": "D" } }
            ],
            "abstract_inverted_index": { "laws": [1], "Scaling": [0], "hold": [2] }
        }));
        assert_eq!(h.title, "Scaling Laws");
        assert_eq!(h.url, "https://doi.org/10.1/x");
        assert_eq!(
            h.snippet,
            "A, B, C et al. · 2020 · arXiv · cited by 42 — Scaling laws hold"
        );

        let bare = hit(&json!({ "id": "https://openalex.org/W2", "display_name": "T" }));
        assert_eq!(bare.url, "https://openalex.org/W2");
        assert_eq!(bare.snippet, "cited by 0");
    }

    #[test]
    fn parses_rate_limit_budget() {
        let b = parse_balance(&json!({ "rate_limit": {
            "daily_budget_usd": 1.0,
            "daily_remaining_usd": 0.982,
            "endpoint_costs_usd": { "search": 0.001 }
        }}))
        .unwrap();
        assert_eq!(b.remaining, 982);
        assert_eq!(b.total, 1000);
        assert!(parse_balance(&json!({ "error": "x" })).is_err());
    }
}
