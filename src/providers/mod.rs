use serde::{Deserialize, Serialize};

use crate::config::Reset;
use crate::error::{Error, Result};

pub(crate) mod chatgpt_browser;
mod chatgpt_sentinel;
mod chatgpt_web;
mod exa;
mod firecrawl;
mod gemini_web;
mod grok_statsig;
mod grok_web;
mod jina;
pub(crate) mod niche;
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
    ChatgptWeb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Search,
    Read,
    DeepResearch,
    Browser,
    Image,
}

impl Capability {
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Search => "search",
            Capability::Read => "read",
            Capability::DeepResearch => "deep_research",
            Capability::Browser => "browser",
            Capability::Image => "create_image",
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
    /// A local file to attach to the chat turn (the `upload` tool). The provider uploads it, then
    /// references it in the send so the model can see it. gemini_web / grok_web only.
    pub file: Option<std::path::PathBuf>,
    /// Cross-provider research niche: `web` (default), `news`, or `academic`. The router steers to a
    /// backend that serves it and each provider maps it to its native vertical (else query-rewrite).
    pub topic: Option<String>,
    /// Recency filter: `day`/`week`/`month`/`year` or an ISO date; mapped per provider (else `after:`).
    pub recency: Option<String>,
    /// Restrict to these domains (a `-` prefix excludes); native filter where supported, else `site:`.
    pub domains: Option<Vec<String>>,
    /// Deep-research depth: `standard` (default) or `deep` (slower/pricier — exa deep-reasoning,
    /// parallel pro tier, grok heavy). Deep-research only.
    pub depth: Option<String>,
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
    /// A generated image returned as bytes instead of `text` (create_image only).
    pub image: Option<OutImage>,
}

/// A base64-encoded image plus its MIME type, carried out-of-band from `text`.
#[derive(Debug)]
pub struct OutImage {
    pub mime: String,
    pub b64: String,
}

impl Outcome {
    pub fn new(text: String, cost: i64) -> Self {
        Self {
            text,
            cost,
            session: None,
            image: None,
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
            ProviderKind::ChatgptWeb => "chatgpt_web",
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
            ProviderKind::ChatgptWeb => "https://chatgpt.com",
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
                | (
                    GeminiWeb | GrokWeb | PerplexityWeb | ChatgptWeb,
                    Search | DeepResearch
                )
                | (ChatgptWeb | GrokWeb | GeminiWeb, Image)
        )
    }

    pub fn is_web(self) -> bool {
        matches!(
            self,
            ProviderKind::GeminiWeb
                | ProviderKind::GrokWeb
                | ProviderKind::PerplexityWeb
                | ProviderKind::ChatgptWeb
        )
    }

    /// Api-key providers whose live balance is only readable through their dashboard's cookie session
    /// (no key-usable endpoint). `fetchira login <them>` captures that session; the api-key still does
    /// the actual calls. exa/parallel are real $ balances the api can't report.
    pub fn balance_session(self) -> bool {
        matches!(self, ProviderKind::Parallel | ProviderKind::Exa)
    }

    /// Fallback ceiling when the provider exposes no live balance (exa, steel-free, parallel) or the
    /// live fetch fails. Providers with a live endpoint (serper/tavily/firecrawl/jina) overwrite this
    /// from `live_balance` — the constant only seeds the soft counter.
    pub fn default_quota(self) -> i64 {
        match self {
            ProviderKind::Tavily => 1000,    // 1k credits/mo
            ProviderKind::Exa => 20_000,     // 20k requests/mo (forever-free)
            ProviderKind::Serper => 2500,    // 2.5k credits, one-time
            ProviderKind::Jina => 2500,      // 10M free tokens ÷ ~4k/read ≈ 2.5k reads, one-time
            ProviderKind::Firecrawl => 1000, // 1k credits/mo
            ProviderKind::Parallel => 16000, // 16k free requests (no reset advertised)
            ProviderKind::Steel => 6000,     // $30 one-time ÷ $0.005/scrape ≈ 6k scrapes
            // Web sessions: server enforces the real (windowed) limits; these are nominal
            // ceilings so a 429 marks the account exhausted and the router fails over.
            ProviderKind::GeminiWeb => 1000,
            ProviderKind::GrokWeb => 100,
            ProviderKind::PerplexityWeb => 300,
            ProviderKind::ChatgptWeb => 100,
        }
    }

    pub fn default_reset(self) -> Reset {
        match self {
            // One-time grants / top-up $ balances (no monthly reset): "lifetime" badge, not "monthly".
            ProviderKind::Serper
            | ProviderKind::Jina
            | ProviderKind::Parallel
            | ProviderKind::Exa
            | ProviderKind::Steel => Reset::Once,
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
            // ChatGPT Plus deep research is a monthly bucket (~25/mo); the server reports the live
            // remaining count in `conversation/init`, so this is just the failover ceiling.
            ProviderKind::ChatgptWeb => 25,
            _ => self.default_quota(),
        }
    }

    pub fn dr_reset(self) -> Reset {
        match self {
            ProviderKind::ChatgptWeb => Reset::Monthly,
            _ if self.is_web() => Reset::Daily,
            _ => self.default_reset(),
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
            ChatgptWeb,
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
            ProviderKind::ChatgptWeb => {
                "your logged-in ChatGPT (chat + web search + deep research)"
            }
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
            ChatgptWeb,
        ],
        Capability::Read => &[Jina, Firecrawl],
        Capability::DeepResearch => &[
            Parallel,
            Exa,
            Tavily,
            PerplexityWeb,
            GeminiWeb,
            GrokWeb,
            ChatgptWeb,
        ],
        Capability::Browser => &[Steel],
        // grok generates in-process and is region-agnostic; gemini image-gen is EU-gated (falls back
        // to text) so it sits behind grok; chatgpt is the browser fallback.
        Capability::Image => &[GrokWeb, GeminiWeb, ChatgptWeb],
    }
}

/// `order(cap)` re-ranked for a research niche: academic floats Exa (papers) then Serper (scholar)
/// to the front, news floats Serper then Tavily. Only reorders providers already serving the cap;
/// `None`/`web` is the plain order. Quota-aware failover still runs within the returned list.
pub fn order_for(cap: Capability, topic: Option<&str>) -> Vec<ProviderKind> {
    use ProviderKind::*;
    let mut list = order(cap).to_vec();
    let front: &[ProviderKind] = match topic {
        Some("academic") => &[Exa, Serper],
        Some("news") => &[Serper, Tavily],
        _ => return list,
    };
    for &p in front.iter().rev() {
        if let Some(i) = list.iter().position(|&k| k == p) {
            list.remove(i);
            list.insert(0, p);
        }
    }
    list
}

/// Live remaining budget reported by the provider itself (only grok exposes one today).
#[derive(Clone, Copy)]
pub struct LiveQuota {
    pub remaining: i64,
    pub total: i64,
    pub window_secs: i64,
}

/// Live remaining balance for an API-key provider, already normalized to the unit fetchira displays
/// (searches/reads/credits — each provider converts from its native $/tokens/credits). Authoritative:
/// it reflects usage outside fetchira plus any top-up or paid plan. `total` is the grant for the
/// fuel gauge (0 when unknown).
#[derive(Clone, Copy)]
pub struct LiveBalance {
    pub remaining: i64,
    pub total: i64,
}

/// One tool/feature's live allowance for the account's tier (e.g. deep_research, image_gen).
#[derive(Clone, Serialize)]
pub struct FeatureLimit {
    pub feature: String,
    pub remaining: i64,
    /// Ceiling for this window when the provider reports one (grok); `None` keeps the soft quota.
    /// `Some(0)` means the feature is locked on this tier — display as 0/0.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    /// Rolling-window length in seconds (grok's per-model windows); `None` for fixed resets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_secs: Option<i64>,
    /// ISO-8601 instant the allowance resets (absolute, not a rolling window).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_after: Option<String>,
}

impl FeatureLimit {
    /// A feature with only a remaining count (chatgpt's shape): no ceiling/window/reset.
    pub fn simple(feature: impl Into<String>, remaining: i64, reset_after: Option<String>) -> Self {
        Self {
            feature: feature.into(),
            remaining,
            total: None,
            window_secs: None,
            reset_after,
        }
    }
}

/// One selectable model or mode in a provider's live catalog, with its per-entry allowance.
/// `total == Some(0)` / `locked` marks an entry the current tier can't select (shown as 0/0).
#[derive(Clone, Serialize)]
pub struct ModelInfo {
    /// The selector an agent passes back (a grok mode, a chatgpt slug, a gemini id).
    pub id: String,
    /// Display name ("Expert", "GPT-5.5", "3.1 Pro").
    pub name: String,
    /// Thinking levels for this model ("instant"/"medium"/"high", "standard"/"extended"); empty if none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub levels: Vec<String>,
    /// Live remaining; `None` when the provider exposes no count (gemini).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<i64>,
    /// Ceiling; `Some(0)` = locked on this tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    /// Rolling-window length in seconds (grok), else `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_secs: Option<i64>,
    /// ISO-8601 reset instant (chatgpt), else `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_after: Option<String>,
    /// Not selectable on the current tier (entitlement-gated). Display as 0/0.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
}

/// The provider's live per-tier limits: a subscription label, per-feature remaining counts, and the
/// selectable model/mode catalog. Generic across web providers (chatgpt/grok fill limits, gemini
/// fills the catalog only — it exposes no live count).
#[derive(Clone, Serialize, Default)]
pub struct LiveLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    pub features: Vec<FeatureLimit>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelInfo>,
}

impl LiveLimits {
    pub fn remaining(&self, feature: &str) -> Option<i64> {
        self.feature(feature).map(|f| f.remaining)
    }

    pub fn feature(&self, feature: &str) -> Option<&FeatureLimit> {
        self.features.iter().find(|f| f.feature == feature)
    }
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
            ProviderKind::GeminiWeb
            | ProviderKind::GrokWeb
            | ProviderKind::PerplexityWeb
            | ProviderKind::ChatgptWeb => Err(Error::Unsupported(self.kind.as_str())),
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
            ProviderKind::ChatgptWeb => chatgpt_web::call(b, client, cap, input).await,
            _ => Err(Error::Unsupported(self.kind.as_str())),
        }
    }

    /// Live remaining budget pulled straight from the provider, when it has one. `deep` asks for
    /// the deep-research budget. grok keys quota by model: search/Fast = grok-4; deep_research =
    /// grok-4-heavy (Heavy) when the account has it, else grok-4 (Expert). Best-effort: `None` if
    /// unsupported or the request fails.
    pub async fn live_quota(&self, client: &wreq::Client, deep: bool) -> Option<LiveQuota> {
        // grok's main (Fast) counter only. Deep-research now comes through `live_limits` (its
        // `deep_research` feature), which is tier-aware — a lapsed sub reads 0/0 there, whereas the
        // rate-limit endpoint returns a model's nominal ceiling regardless of entitlement.
        if self.kind != ProviderKind::GrokWeb || deep {
            return None;
        }
        // Search routes to Fast (grok-4-auto), so the main counter tracks that model — matching the
        // Fast row in the catalog. (Paid: 50/2h; free: 7/24h.)
        grok_web::rate_limit(&self.base, client, "grok-4-auto")
            .await
            .ok()
    }

    /// Live per-tier tool/model limits + the selectable model catalog, when the provider exposes
    /// them. Best-effort: `None` if unsupported or the fetch fails.
    pub async fn live_limits(&self, client: &wreq::Client) -> Option<LiveLimits> {
        match self.kind {
            ProviderKind::ChatgptWeb => chatgpt_web::limits(&self.base, client).await.ok(),
            ProviderKind::GrokWeb => grok_web::limits(&self.base, client).await.ok(),
            ProviderKind::GeminiWeb => gemini_web::limits(&self.base, client).await.ok(),
            _ => None,
        }
    }

    /// Live remaining balance for an API-key provider, in fetchira's display unit. Best-effort:
    /// `None` when the provider has no key-usable balance endpoint (exa, parallel), the account is a
    /// free tier the endpoint doesn't cover (steel), or the fetch fails.
    pub async fn live_balance(&self, key: &str, client: &reqwest::Client) -> Option<LiveBalance> {
        match self.kind {
            ProviderKind::Serper => serper::balance(&self.base, key, client).await.ok(),
            ProviderKind::Tavily => tavily::balance(&self.base, key, client).await.ok(),
            ProviderKind::Firecrawl => firecrawl::balance(&self.base, key, client).await.ok(),
            ProviderKind::Jina => jina::balance(key, client).await.ok(),
            ProviderKind::Steel => steel::balance(&self.base, key, client).await.ok(),
            _ => None,
        }
    }

    /// Live balance for a `balance_session` provider (exa/parallel), read through its dashboard's
    /// cookie session (the `client` carries the captured cookies). Best-effort.
    pub async fn live_balance_web(&self, client: &wreq::Client) -> Option<LiveBalance> {
        match self.kind {
            ProviderKind::Parallel => parallel::balance(client).await.ok(),
            ProviderKind::Exa => exa::balance(client).await.ok(),
            _ => None,
        }
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

/// Read a local attachment for the upload tool: `(filename, mime, bytes)`. Guessing the MIME from
/// the extension is enough for these web endpoints.
pub(crate) fn read_attachment(path: &std::path::Path) -> Result<(String, String, Vec<u8>)> {
    let bytes = std::fs::read(path).map_err(|e| Error::Provider {
        provider: "upload",
        status: 0,
        body: format!("cannot read {}: {e}", path.display()),
    })?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    let mime = mime_of(&name);
    Ok((name, mime, bytes))
}

fn mime_of(name: &str) -> String {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" => "text/plain",
        "json" => "application/json",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        _ => "application/octet-stream",
    }
    .to_string()
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

#[cfg(test)]
mod tests {
    use super::*;
    use ProviderKind::*;

    #[test]
    fn order_for_floats_niche_providers() {
        // academic -> Exa then Serper to the front, rest of the order preserved
        let acad = order_for(Capability::Search, Some("academic"));
        assert_eq!(acad[0], Exa);
        assert_eq!(acad[1], Serper);
        assert_eq!(acad.len(), order(Capability::Search).len());
        // news -> Serper then Tavily
        let news = order_for(Capability::Search, Some("news"));
        assert_eq!(&news[..2], &[Serper, Tavily]);
        // web / none leaves the base order untouched
        assert_eq!(
            order_for(Capability::Search, None),
            order(Capability::Search)
        );
        assert_eq!(
            order_for(Capability::Search, Some("web")),
            order(Capability::Search)
        );
        // only reorders providers already present for the cap (Serper isn't a DeepResearch backend)
        let dr = order_for(Capability::DeepResearch, Some("academic"));
        assert_eq!(dr[0], Exa);
        assert!(!dr.contains(&Serper));
    }
}
