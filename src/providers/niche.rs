//! Research niche knobs (topic/recency/domains), mapped to native filters or folded into the query.

use chrono::{Duration, Utc};

use super::Input;

/// `recency` → a zero-padded `YYYY-MM-DD` start date, from a shorthand or an ISO date.
pub fn recency_date(recency: &str) -> Option<String> {
    let days = match recency {
        "day" => 1,
        "week" => 7,
        "month" => 30,
        "year" => 365,
        iso => {
            return chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d")
                .ok()
                .map(|d| d.format("%Y-%m-%d").to_string())
        }
    };
    Some(
        (Utc::now() - Duration::days(days))
            .format("%Y-%m-%d")
            .to_string(),
    )
}

/// `recency` → Google `tbs`: `qdr:d|w|m|y`, or a `cdr` range from an ISO date.
pub fn recency_tbs(recency: &str) -> Option<String> {
    match recency {
        "day" => Some("qdr:d".into()),
        "week" => Some("qdr:w".into()),
        "month" => Some("qdr:m".into()),
        "year" => Some("qdr:y".into()),
        _ => chrono::NaiveDate::parse_from_str(recency, "%Y-%m-%d")
            .ok()
            .map(|d| d.format("cdr:1,cd_min:%-m/%-d/%Y").to_string()),
    }
}

/// `recency` → a rolling day count.
pub fn recency_days(recency: &str) -> Option<u32> {
    match recency {
        "day" => Some(1),
        "week" => Some(7),
        "month" => Some(30),
        "year" => Some(365),
        _ => None,
    }
}

/// Split `domains` into (include, exclude); a `-` prefix marks an exclusion.
pub fn split_domains(domains: &[String]) -> (Vec<&str>, Vec<&str>) {
    let mut inc = Vec::new();
    let mut exc = Vec::new();
    for d in domains {
        match d.strip_prefix('-') {
            Some(x) => exc.push(x),
            None => inc.push(d.as_str()),
        }
    }
    (inc, exc)
}

/// `domains` as a `site:` query suffix; includes are OR-joined or Google ANDs them into nothing.
pub fn sites(input: &Input) -> String {
    let Some(domains) = &input.domains else {
        return String::new();
    };
    let (inc, exc) = split_domains(domains);
    let mut q = String::new();
    if !inc.is_empty() {
        let inc: Vec<String> = inc.iter().map(|d| format!("site:{d}")).collect();
        q.push_str(&format!(" {}", inc.join(" OR ")));
    }
    for d in exc {
        q.push_str(&format!(" -site:{d}"));
    }
    q
}

/// Bake a topic hint + `recency` into the query, for backends with no native filter for them.
pub fn rewrite_query(input: &Input) -> String {
    let mut q = input.query.clone().unwrap_or_default();
    q.push_str(topic_hint(input));
    if let Some(r) = input.recency.as_deref().and_then(recency_date) {
        q.push_str(&format!(" after:{r}"));
    }
    q
}

/// Query suffix standing in for a topic vertical the backend lacks.
pub fn topic_hint(input: &Input) -> &'static str {
    match input.topic.as_deref() {
        Some("academic") => " (peer-reviewed OR research paper)",
        Some("news") => " latest news",
        _ => "",
    }
}

/// Does this request carry any niche knob?
pub fn any(input: &Input) -> bool {
    input.topic.is_some() || input.recency.is_some() || input.domains.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recency_shorthands_and_iso() {
        assert_eq!(recency_tbs("week").as_deref(), Some("qdr:w"));
        assert_eq!(
            recency_tbs("2026-01-05").as_deref(),
            Some("cdr:1,cd_min:1/5/2026")
        );
        assert_eq!(recency_days("month"), Some(30));
        assert_eq!(recency_date("2024-03-01").as_deref(), Some("2024-03-01"));
        assert_eq!(recency_date("2025-1-5").as_deref(), Some("2025-01-05"));
        assert!(recency_date("week").is_some());
        assert_eq!(recency_tbs("bogus"), None);
    }

    #[test]
    fn domain_split_and_rewrite() {
        let doms = ["nature.com".to_string(), "-reddit.com".to_string()];
        let (inc, exc) = split_domains(&doms);
        assert_eq!(inc, ["nature.com"]);
        assert_eq!(exc, ["reddit.com"]);
        let input = Input {
            query: Some("crispr".into()),
            topic: Some("news".into()),
            domains: Some(vec!["nature.com".into(), "-reddit.com".into()]),
            ..Default::default()
        };
        assert_eq!(rewrite_query(&input), "crispr latest news");
    }
}
