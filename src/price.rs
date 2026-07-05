//! Conservative "est. retail" price per operation — each provider's own published overage rate.
//! Used to tally what the pooled free tiers would have cost at list price (the savings odometer).
//! Deliberately low-balled so the number under-promises. Deep research is priced above plain search.
//! Unknown pairs return 0.0 (uncounted).

pub fn retail_usd(provider: &str, capability: &str) -> f64 {
    match (provider, capability) {
        ("serper", _) => 0.001,
        ("tavily", _) => 0.008,
        ("exa", "deep_research") => 0.05,
        ("exa", _) => 0.007,
        ("parallel", "deep_research") => 0.01,
        ("parallel", _) => 0.005,
        ("firecrawl", _) => 0.002,
        ("jina", _) => 0.02,
        ("steel", _) => 0.005,
        (p, "deep_research") if p.ends_with("_web") => 0.20,
        (p, _) if p.ends_with("_web") => 0.02,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_research_beats_search() {
        assert!(retail_usd("exa", "deep_research") > retail_usd("exa", "search"));
        assert!(retail_usd("grok_web", "deep_research") > retail_usd("grok_web", "search"));
    }

    #[test]
    fn web_sessions_priced_and_unknown_is_zero() {
        assert_eq!(retail_usd("chatgpt_web", "search"), 0.02);
        assert_eq!(retail_usd("serper", "search"), 0.001);
        assert_eq!(retail_usd("nope", "search"), 0.0);
    }
}
