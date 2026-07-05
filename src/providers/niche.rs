//! Cross-provider research niche knobs (topic/recency/domains). Each provider maps the generic
//! `Input` fields to its native filter where it can; where it can't, the query is rewritten with
//! Google-style operators so the niche still applies (best-effort, `native`=false in the reply).

use chrono::{Duration, Utc};

use super::Input;

/// `recency` → an ISO `YYYY-MM-DD` start date (exa `startPublishedDate`). Accepts the shorthands or
/// passes an ISO date straight through.
pub fn recency_date(recency: &str) -> Option<String> {
    let days = match recency {
        "day" => 1,
        "week" => 7,
        "month" => 30,
        "year" => 365,
        iso if iso.len() == 10 && iso.as_bytes().get(4) == Some(&b'-') => {
            return Some(iso.to_string())
        }
        _ => return None,
    };
    Some(
        (Utc::now() - Duration::days(days))
            .format("%Y-%m-%d")
            .to_string(),
    )
}

/// `recency` → Google `tbs` shorthand (serper / firecrawl: `qdr:d|w|m|y`).
pub fn recency_tbs(recency: &str) -> Option<&'static str> {
    match recency {
        "day" => Some("qdr:d"),
        "week" => Some("qdr:w"),
        "month" => Some("qdr:m"),
        "year" => Some("qdr:y"),
        _ => None,
    }
}

/// `recency` → a rolling day count (Tavily `days`).
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

/// Bake `domains` + a topic hint + `recency` into the query as Google-style operators / prose, for
/// backends with no native filter (serper's `site:`, and web sessions which take everything as text).
pub fn rewrite_query(input: &Input) -> String {
    let mut q = input.query.clone().unwrap_or_default();
    if let Some(t) = input.topic.as_deref() {
        match t {
            "academic" => q.push_str(" (peer-reviewed OR research paper)"),
            "news" => q.push_str(" latest news"),
            _ => {}
        }
    }
    if let Some(domains) = &input.domains {
        for d in domains {
            match d.strip_prefix('-') {
                Some(x) => q.push_str(&format!(" -site:{x}")),
                None => q.push_str(&format!(" site:{d}")),
            }
        }
    }
    if let Some(r) = input.recency.as_deref().and_then(recency_date) {
        q.push_str(&format!(" after:{r}"));
    }
    q
}

/// Does this request carry any niche knob? (Cheap gate so the common case skips all of the above.)
pub fn any(input: &Input) -> bool {
    input.topic.is_some() || input.recency.is_some() || input.domains.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recency_shorthands_and_iso() {
        assert_eq!(recency_tbs("week"), Some("qdr:w"));
        assert_eq!(recency_days("month"), Some(30));
        assert_eq!(recency_date("2024-03-01").as_deref(), Some("2024-03-01"));
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
            domains: Some(vec!["nature.com".into(), "-reddit.com".into()]),
            ..Default::default()
        };
        let q = rewrite_query(&input);
        assert!(q.contains("site:nature.com") && q.contains("-site:reddit.com"));
    }
}
