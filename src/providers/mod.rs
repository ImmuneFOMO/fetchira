use serde::{Deserialize, Serialize};

use crate::config::Reset;
use crate::error::{Error, Result};

mod exa;
mod firecrawl;
mod gemini_web;
mod grok_statsig;
mod grok_web;
mod jina;
mod parallel;
mod perplexity_web;
mod serper;
mod steel;
mod tavily;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Tavily,
    Exa,
    Serper,
    Jina,
    Firecrawl,
    Parallel,
    Steel,
    GeminiWeb,
    GrokWeb,
    PerplexityWeb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Search,
    Read,
    DeepResearch,
    Browser,
}

impl Capability {
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Search => "search",
            Capability::Read => "read",
            Capability::DeepResearch => "deep_research",
            Capability::Browser => "browser",
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Input {
    pub query: Option<String>,
    pub url: Option<String>,
    pub max_results: Option<u32>,
    /// Resume token (opaque to the router) to continue a web-session conversation.
    pub session: Option<String>,
    /// Provider-specific model selector (e.g. a Gemini model id or Perplexity preference).
    pub model: Option<String>,
    /// Provider-specific mode (e.g. grok "expert", perplexity "deep research").
    pub mode: Option<String>,
    /// Local file paths to upload and attach to the message (grok_web only).
    pub attachments: Option<Vec<String>>,
}

impl Input {
    pub fn need_query(&self) -> Result<&str> {
        self.query.as_deref().ok_or(Error::MissingArg("query"))
    }
    pub fn need_url(&self) -> Result<&str> {
        self.url.as_deref().ok_or(Error::MissingArg("url"))
    }
    pub fn results(&self) -> u32 {
        self.max_results.unwrap_or(5).clamp(1, 20)
    }
}

#[derive(Default)]
pub struct Outcome {
    pub text: String,
    pub cost: i64,
    /// A token the caller can pass back as `Input.session` to continue this conversation.
    pub session: Option<String>,
}

impl Outcome {
    pub fn new(text: String, cost: i64) -> Self {
        Self {
            text,
            cost,
            session: None,
        }
    }
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Tavily => "tavily",
            ProviderKind::Exa => "exa",
            ProviderKind::Serper => "serper",
            ProviderKind::Jina => "jina",
            ProviderKind::Firecrawl => "firecrawl",
            ProviderKind::Parallel => "parallel",
            ProviderKind::Steel => "steel",
            ProviderKind::GeminiWeb => "gemini_web",
            ProviderKind::GrokWeb => "grok_web",
            ProviderKind::PerplexityWeb => "perplexity_web",
        }
    }

    pub fn base(self) -> &'static str {
        match self {
            ProviderKind::Tavily => "https://api.tavily.com",
            ProviderKind::Exa => "https://api.exa.ai",
            ProviderKind::Serper => "https://google.serper.dev",
            ProviderKind::Jina => "https://r.jina.ai",
            ProviderKind::Firecrawl => "https://api.firecrawl.dev",
            ProviderKind::Parallel => "https://api.parallel.ai",
            ProviderKind::Steel => "https://api.steel.dev",
            ProviderKind::GeminiWeb => "https://gemini.google.com",
            ProviderKind::GrokWeb => "https://grok.com",
            ProviderKind::PerplexityWeb => "https://www.perplexity.ai",
        }
    }

    pub fn supports(self, cap: Capability) -> bool {
        use Capability::*;
        use ProviderKind::*;
        matches!(
            (self, cap),
            (Tavily, Search | Read | DeepResearch)
                | (Exa, Search | Read | DeepResearch)
                | (Serper, Search)
                | (Jina, Read)
                | (Firecrawl, Search | Read)
                | (Parallel, Search | DeepResearch)
                | (Steel, Browser)
                | (GeminiWeb | GrokWeb | PerplexityWeb, Search | DeepResearch)
        )
    }

    pub fn is_web(self) -> bool {
        matches!(
            self,
            ProviderKind::GeminiWeb | ProviderKind::GrokWeb | ProviderKind::PerplexityWeb
        )
    }

    pub fn default_quota(self) -> i64 {
        match self {
            ProviderKind::Tavily => 1000,
            ProviderKind::Exa => 1000,
            ProviderKind::Serper => 2500,
            ProviderKind::Jina => 1_000_000, // 10M tokens; tracked loosely as requests
            ProviderKind::Firecrawl => 1000,
            ProviderKind::Parallel => 16000,
            ProviderKind::Steel => 360_000, // 100 browser-hours in seconds
            // Web sessions: server enforces the real (windowed) limits; these are nominal
            // ceilings so a 429 marks the account exhausted and the router fails over.
            ProviderKind::GeminiWeb => 1000,
            ProviderKind::GrokWeb => 100,
            ProviderKind::PerplexityWeb => 300,
        }
    }

    pub fn default_reset(self) -> Reset {
        match self {
            ProviderKind::Serper => Reset::Once,
            _ => Reset::Monthly,
        }
    }

    /// Deep-research budget (web providers only) — small + daily, since the real per-tier limit
    /// is tight and time-windowed. These are conservative free-tier guesses; override per account
    /// (`dr_quota`/`dr_reset`) to match your subscription. Non-web providers use their normal quota.
    pub fn dr_quota(self) -> i64 {
        match self {
            ProviderKind::GeminiWeb => 10,
            ProviderKind::PerplexityWeb => 5,
            ProviderKind::GrokWeb => 3,
            _ => self.default_quota(),
        }
    }

    pub fn dr_reset(self) -> Reset {
        if self.is_web() {
            Reset::Daily
        } else {
            self.default_reset()
        }
    }

    pub fn all() -> &'static [ProviderKind] {
        use ProviderKind::*;
        &[
            Serper,
            Tavily,
            Exa,
            Jina,
            Firecrawl,
            Parallel,
            Steel,
            PerplexityWeb,
            GeminiWeb,
            GrokWeb,
        ]
    }

    /// One-line description for the setup wizard / `providers` listing.
    pub fn blurb(self) -> &'static str {
        match self {
            ProviderKind::Serper => "Google search results (SERP)",
            ProviderKind::Tavily => "web search + answers tuned for LLMs",
            ProviderKind::Exa => "neural / semantic web search",
            ProviderKind::Jina => "read any URL as clean markdown",
            ProviderKind::Firecrawl => "scrape / crawl pages to markdown",
            ProviderKind::Parallel => "async deep-research API",
            ProviderKind::Steel => "headless-browser page scrape",
            ProviderKind::PerplexityWeb => "your logged-in Perplexity (search + deep research)",
            ProviderKind::GeminiWeb => "your logged-in Gemini (chat + deep research)",
            ProviderKind::GrokWeb => "your logged-in Grok (search + deepsearch)",
        }
    }

    /// Where to get an API key (key-based providers only).
    pub fn signup(self) -> &'static str {
        match self {
            ProviderKind::Serper => "https://serper.dev",
            ProviderKind::Tavily => "https://app.tavily.com",
            ProviderKind::Exa => "https://dashboard.exa.ai/api-keys",
            ProviderKind::Jina => "https://jina.ai/reader (free; key raises limits)",
            ProviderKind::Firecrawl => "https://firecrawl.dev",
            ProviderKind::Parallel => "https://parallel.ai",
            ProviderKind::Steel => "https://steel.dev",
            _ => "",
        }
    }
}

/// Preference order per capability: try providers left-to-right, skipping exhausted.
pub fn order(cap: Capability) -> &'static [ProviderKind] {
    use ProviderKind::*;
    match cap {
        Capability::Search => &[
            Serper,
            Tavily,
            Exa,
            Parallel,
            PerplexityWeb,
            GeminiWeb,
            GrokWeb,
        ],
        Capability::Read => &[Jina, Firecrawl],
        Capability::DeepResearch => &[Parallel, Exa, Tavily, PerplexityWeb, GeminiWeb, GrokWeb],
        Capability::Browser => &[Steel],
    }
}

/// Live remaining budget reported by the provider itself (only grok exposes one today).
#[derive(Clone, Copy)]
pub struct LiveQuota {
    pub remaining: i64,
    pub total: i64,
    pub window_secs: i64,
}

/// A provider endpoint: its kind plus a base URL (overridable so tests can point at a mock).
pub struct Provider {
    pub kind: ProviderKind,
    pub base: String,
}

impl Provider {
    pub fn new(kind: ProviderKind) -> Self {
        Self {
            kind,
            base: kind.base().to_string(),
        }
    }

    pub fn with_base(kind: ProviderKind, base: impl Into<String>) -> Self {
        Self {
            kind,
            base: base.into(),
        }
    }

    pub async fn call(
        &self,
        key: &str,
        client: &reqwest::Client,
        cap: Capability,
        input: &Input,
    ) -> Result<Outcome> {
        let b = &self.base;
        match self.kind {
            ProviderKind::Tavily => tavily::call(b, key, client, cap, input).await,
            ProviderKind::Exa => exa::call(b, key, client, cap, input).await,
            ProviderKind::Serper => serper::call(b, key, client, cap, input).await,
            ProviderKind::Jina => jina::call(b, key, client, cap, input).await,
            ProviderKind::Firecrawl => firecrawl::call(b, key, client, cap, input).await,
            ProviderKind::Parallel => parallel::call(b, key, client, cap, input).await,
            ProviderKind::Steel => steel::call(b, key, client, cap, input).await,
            ProviderKind::GeminiWeb | ProviderKind::GrokWeb | ProviderKind::PerplexityWeb => {
                Err(Error::Unsupported(self.kind.as_str()))
            }
        }
    }

    /// Web-session providers: cookies are already baked into the impersonating `client`.
    pub async fn call_web(
        &self,
        client: &wreq::Client,
        cap: Capability,
        input: &Input,
    ) -> Result<Outcome> {
        let b = &self.base;
        match self.kind {
            ProviderKind::GeminiWeb => gemini_web::call(b, client, cap, input).await,
            ProviderKind::GrokWeb => grok_web::call(b, client, cap, input).await,
            ProviderKind::PerplexityWeb => perplexity_web::call(b, client, cap, input).await,
            _ => Err(Error::Unsupported(self.kind.as_str())),
        }
    }

    /// Live remaining budget pulled straight from the provider, when it has one. `deep` asks for
    /// the deep-research budget. grok keys quota by model: search/Fast = grok-4; deep_research =
    /// grok-4-heavy (Heavy) when the account has it, else grok-4 (Expert). Best-effort: `None` if
    /// unsupported or the request fails.
    pub async fn live_quota(&self, client: &wreq::Client, deep: bool) -> Option<LiveQuota> {
        if self.kind != ProviderKind::GrokWeb {
            return None;
        }
        if deep {
            match grok_web::rate_limit(&self.base, client, "grok-4-heavy").await {
                Ok(lq) if lq.total > 0 => return Some(lq),
                _ => {}
            }
        }
        grok_web::rate_limit(&self.base, client, "grok-4")
            .await
            .ok()
    }
}

struct Hit {
    title: String,
    url: String,
    snippet: String,
}

/// Extract a string field, defaulting to empty when absent or non-string.
fn s(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Render a value as text: strings verbatim, everything else pretty-printed JSON.
/// Used for async-research payloads whose exact shape varies by provider.
fn value_to_text(v: &serde_json::Value) -> String {
    match v.as_str() {
        Some(t) => t.to_string(),
        None => serde_json::to_string_pretty(v).unwrap_or_default(),
    }
}

fn fmt_hits(hits: &[Hit]) -> String {
    hits.iter()
        .enumerate()
        .map(|(i, h)| format!("{}. {}\n   {}\n   {}", i + 1, h.title, h.url, h.snippet))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Append a numbered source list to a synthesized answer (web providers don't return SERP rows).
fn with_sources(answer: String, sources: &[String]) -> String {
    if sources.is_empty() {
        return answer;
    }
    let list = sources
        .iter()
        .enumerate()
        .map(|(i, u)| format!("{}. {}", i + 1, u))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{answer}\n\nSources:\n{list}")
}

/// A uuid4-shaped correlation id (lowercase). No dependency: derived from time + a counter,
/// which is sufficient for the client-generated request ids these web APIs expect.
fn uuid4() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let c = CTR.fetch_add(1, Ordering::Relaxed);
    let a = t ^ c.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let b = t.rotate_left(32) ^ c.wrapping_add(0xD1B5_4A32_D192_ED03);
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (a >> 32) as u32,
        (a >> 16) as u16,
        (a as u16) & 0x0fff,
        ((b >> 48) as u16 & 0x3fff) | 0x8000,
        b & 0xffff_ffff_ffff
    )
}

/// Map a non-2xx response to the right error: 429/402 trigger failover + exhaustion.
async fn check(provider: &'static str, resp: reqwest::Response) -> Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let code = status.as_u16();
    let body = resp.text().await.unwrap_or_default();
    Err(match code {
        429 => Error::RateLimit(format!("{provider}: {body}")),
        402 => Error::QuotaExceeded(format!("{provider}: {body}")),
        _ => Error::Provider {
            provider,
            status: code,
            body,
        },
    })
}
