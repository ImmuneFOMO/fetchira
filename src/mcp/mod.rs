use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ErrorData};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::providers::{Capability, Input, ProviderKind};
use crate::router::Router;

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// The search query (or a follow-up message when `session` is set). Optional only when polling a
    /// running deep-research `session` for its report.
    #[serde(default)]
    pub query: Option<String>,
    /// Force a specific provider instead of letting the router choose.
    pub provider: Option<ProviderKind>,
    /// Maximum number of results (default 5, capped at 20).
    pub max_results: Option<u32>,
    /// Resume token from a previous web-provider result; continues that conversation with
    /// its history (implies the same provider). Web providers only.
    pub session: Option<String>,
    /// Provider-specific model selector (e.g. gemini "3.1 pro"/"flash", perplexity "gpt-5",
    /// grok "grok-4"). For chatgpt_web this is two axes: a model ("gpt-5.5"/"gpt-5.4"/"o3"), a
    /// thinking level ("instant"/"medium"/"high"), or both ("gpt-5.4 high"). Levels are read live
    /// and vary per model (o3 only offers medium); an unknown name returns the actual options.
    /// Ignored by the API providers.
    pub model: Option<String>,
    /// Provider-specific mode. grok: "auto"/"fast"/"expert"/"heavy" (search defaults to fast,
    /// deep_research to heavy then expert); perplexity: "reasoning"/"deep research".
    pub mode: Option<String>,
    /// Research niche: "web" (default), "news", or "academic" — steers to a fitting backend
    /// (academic → scholar/exa papers, news → news sources).
    pub topic: Option<String>,
    /// Recency filter: "day"/"week"/"month"/"year" or an ISO date (e.g. "2024-01-01").
    pub recency: Option<String>,
    /// Restrict to these domains; prefix one with "-" to exclude it (e.g. ["nature.com","-reddit.com"]).
    pub domains: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ResearchArgs {
    #[serde(flatten)]
    pub base: SearchArgs,
    /// Depth: "standard" (default) or "deep" — deep is slower and may spend a paid balance
    /// (exa deep-reasoning, parallel pro tier, grok heavy).
    pub depth: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ReadArgs {
    /// The URL to read and return as clean markdown.
    pub url: String,
    /// Force a specific provider instead of letting the router choose.
    pub provider: Option<ProviderKind>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct BrowserArgs {
    /// The URL to load in a headless browser.
    pub url: String,
    /// Reserved for multi-step automation; ignored in v1.
    pub actions: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ImageArgs {
    /// What to draw.
    pub prompt: String,
    /// Force a specific provider (gemini_web / grok_web generate in-process over HTTP; chatgpt_web
    /// drives the browser). Otherwise the router picks the least-exhausted one and fails over.
    pub provider: Option<ProviderKind>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct UploadArgs {
    /// Absolute path to the local file or image to attach.
    pub path: String,
    /// Your prompt/question about the attached file.
    pub prompt: String,
    /// Which logged-in web session to use: grok_web, chatgpt_web, or gemini_web (defaults to grok_web).
    pub provider: Option<ProviderKind>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct NoArgs {}

pub struct Fetchira {
    router: Arc<Router>,
    tool_router: ToolRouter<Self>,
}

impl Fetchira {
    pub fn new(router: Arc<Router>) -> Self {
        Self {
            router,
            tool_router: Self::tool_router(),
        }
    }

    async fn run(
        &self,
        cap: Capability,
        input: Input,
        forced: Option<ProviderKind>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match self.router.call(cap, &input, forced).await {
            Ok(reply) => {
                if let Some(img) = reply.image {
                    return Ok(CallToolResult::success(vec![Content::image(
                        img.b64, img.mime,
                    )]));
                }
                let mut text = reply.text;
                if let Some(s) = reply.session {
                    text.push_str(&format!(
                        "\n\n⟦session: {s} — pass as \"session\" to continue this conversation⟧"
                    ));
                }
                CallToolResult::success(vec![Content::text(text)])
            }
            Err(e) => CallToolResult::error(vec![Content::text(e.to_string())]),
        })
    }
}

/// Resolve which provider to force and the opaque resume token. A `session` token is
/// `provider:opaque`; it pins the provider and carries the opaque part to the provider.
fn route(
    provider: Option<ProviderKind>,
    session: Option<String>,
) -> (Option<ProviderKind>, Option<String>) {
    if let Some(s) = &session {
        if let Some((p, rest)) = s.split_once(':') {
            if let Ok(kind) =
                serde_json::from_value::<ProviderKind>(serde_json::Value::String(p.to_string()))
            {
                return (Some(kind), Some(rest.to_string()));
            }
        }
    }
    (provider, None)
}

/// Build the shared `Input` from a search/research request (the niche knobs ride along for both).
fn search_input(args: SearchArgs, session: Option<String>) -> Input {
    Input {
        query: args.query,
        max_results: args.max_results,
        session,
        model: args.model,
        mode: args.mode,
        topic: args.topic,
        recency: args.recency,
        domains: args.domains,
        ..Default::default()
    }
}

#[tool_router]
impl Fetchira {
    #[tool(
        description = "Web search across quota-aware providers. API providers (serper, tavily, exa, parallel) return ranked title/url/snippet results; cookie-auth web providers (perplexity_web, gemini_web, grok_web, chatgpt_web) return a synthesized answer with sources and a `session` token for follow-ups. Force one with `provider`, pick a `model`/`mode`, or pass a `session` to continue a chat. For chatgpt_web this is a chat turn; `model` picks the composer's model and/or its thinking level (e.g. \"gpt-5.4 high\", \"o3\", or just \"high\" — levels are per-model, and an unknown name returns the options) with web search on by default — pass `mode=\"chat\"` to answer from the model alone without browsing. Niche knobs: `topic` (web/news/academic), `recency`, `domains`."
    )]
    pub async fn search(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (forced, session) = route(args.provider, args.session.clone());
        let input = search_input(args, session);
        self.run(Capability::Search, input, forced).await
    }

    #[tool(
        description = "Read a URL and return its main content as clean markdown (jina, firecrawl)."
    )]
    pub async fn read(
        &self,
        Parameters(args): Parameters<ReadArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let input = Input {
            url: Some(args.url),
            ..Default::default()
        };
        self.run(Capability::Read, input, args.provider).await
    }

    #[tool(
        description = "Deep research over multiple sources (parallel, exa, tavily, perplexity_web, gemini_web, grok_web, chatgpt_web). exa/parallel and the web sessions do true multi-round research; tavily returns a single synthesized answer. May take minutes. gemini_web and chatgpt_web both return a research PLAN plus a `session` first: pass that `session` with query \"start\" to run it, or with a revised research request to replace the plan. gemini returns the finished report in the same call; chatgpt then runs for ~5-30 min, so after \"start\" it hands back a `session` you call again to fetch the report (chatgpt uses its own research model — `model` is ignored). Pass a `session` to continue any web research thread. Niche knobs: `topic` (web/news/academic), `recency`, `domains`, `depth` (standard/deep — deep is slower/pricier)."
    )]
    pub async fn deep_research(
        &self,
        Parameters(args): Parameters<ResearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (forced, session) = route(args.base.provider, args.base.session.clone());
        let mut input = search_input(args.base, session);
        input.depth = args.depth;
        self.run(Capability::DeepResearch, input, forced).await
    }

    #[tool(
        description = "Load a URL in a headless browser (Steel) and return the page as markdown."
    )]
    pub async fn browser(
        &self,
        Parameters(args): Parameters<BrowserArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let input = Input {
            url: Some(args.url),
            ..Default::default()
        };
        self.run(Capability::Browser, input, None).await
    }

    #[tool(
        description = "Show remaining free quota per account and provider for the current period, plus each account's assigned proxy. For logged-in web sessions (grok/gemini/chatgpt) also includes live per-tier limits and the selectable model/mode catalog with thinking levels — a locked mode (e.g. grok Expert/Heavy on a lapsed sub) reads 0/0."
    )]
    pub async fn usage(
        &self,
        Parameters(_): Parameters<NoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match self.router.usage_snapshot().await {
            Ok(views) => {
                let text = crate::router::compact_usage(&views);
                CallToolResult::success(vec![Content::text(text)])
            }
            Err(e) => CallToolResult::error(vec![Content::text(e.to_string())]),
        })
    }

    #[tool(
        description = "Generate an image from a text prompt via a logged-in web session (gemini_web / grok_web / chatgpt_web). Returns the image itself as bytes (base64), not a link. Force one with `provider`; otherwise the router chooses and fails over."
    )]
    pub async fn create_image(
        &self,
        Parameters(args): Parameters<ImageArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let input = Input {
            query: Some(args.prompt),
            ..Default::default()
        };
        self.run(Capability::Image, input, args.provider).await
    }

    #[tool(
        description = "Attach a local file or image to a chat turn and ask about it, via a logged-in web session (grok_web, chatgpt_web, or gemini_web). `path` is an absolute local file path, `prompt` your question; returns the model's answer. Force one with `provider` (defaults to grok_web)."
    )]
    pub async fn upload(
        &self,
        Parameters(args): Parameters<UploadArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let provider = args.provider.unwrap_or(ProviderKind::GrokWeb);
        if !matches!(
            provider,
            ProviderKind::GeminiWeb | ProviderKind::GrokWeb | ProviderKind::ChatgptWeb
        ) {
            return Ok(CallToolResult::error(vec![Content::text(
                "upload supports gemini_web, grok_web, or chatgpt_web only".to_string(),
            )]));
        }
        let input = Input {
            query: Some(args.prompt),
            file: Some(std::path::PathBuf::from(args.path)),
            ..Default::default()
        };
        self.run(Capability::Search, input, Some(provider)).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Fetchira {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("fetchira", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Quota-aware router over free web-search/scrape providers. \
                 Tools: search, read, deep_research, browser, create_image, usage. Pass `provider` to \
                 force a specific backend; otherwise the least-exhausted one is chosen.",
            )
    }
}
