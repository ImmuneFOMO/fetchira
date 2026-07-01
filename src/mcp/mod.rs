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
    /// The search query (or a follow-up message when `session` is set).
    pub query: String,
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
    /// Force a specific provider (only chatgpt_web generates images today).
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

#[tool_router]
impl Fetchira {
    #[tool(
        description = "Web search across quota-aware providers. API providers (serper, tavily, exa, parallel) return ranked title/url/snippet results; cookie-auth web providers (perplexity_web, gemini_web, grok_web, chatgpt_web) return a synthesized answer with sources and a `session` token for follow-ups. Force one with `provider`, pick a `model`/`mode`, or pass a `session` to continue a chat. For chatgpt_web this is a chat turn; `model` picks the composer's model and/or its thinking level (e.g. \"gpt-5.4 high\", \"o3\", or just \"high\" — levels are per-model, and an unknown name returns the options) with web search on by default — pass `mode=\"chat\"` to answer from the model alone without browsing."
    )]
    pub async fn search(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (forced, session) = route(args.provider, args.session);
        let input = Input {
            query: Some(args.query),
            max_results: args.max_results,
            session,
            model: args.model,
            mode: args.mode,
            ..Default::default()
        };
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
        description = "Deep research: long-running multi-source synthesis with citations (parallel, exa, tavily, perplexity_web, gemini_web, grok_web, chatgpt_web). May take minutes. For gemini_web this returns a research PLAN plus a `session`; pass that `session` with query \"start\" to run it. For chatgpt_web it kicks off ChatGPT Deep Research (its own research model — `model` is ignored), waits briefly, then — if still running — returns a `session`; call deep_research again with that `session` to fetch the finished report (pass `mode=\"background\"` to return the session immediately without waiting). Pass a `session` to continue any web research thread."
    )]
    pub async fn deep_research(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (forced, session) = route(args.provider, args.session);
        let input = Input {
            query: Some(args.query),
            max_results: args.max_results,
            session,
            model: args.model,
            mode: args.mode,
            ..Default::default()
        };
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
        description = "Show remaining free quota per account and provider for the current period, plus each account's assigned proxy."
    )]
    pub async fn usage(
        &self,
        Parameters(_): Parameters<NoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match self.router.usage_snapshot().await {
            Ok(views) => {
                let text = serde_json::to_string_pretty(&views).unwrap_or_default();
                CallToolResult::success(vec![Content::text(text)])
            }
            Err(e) => CallToolResult::error(vec![Content::text(e.to_string())]),
        })
    }

    #[tool(
        description = "Generate an image from a text prompt via your logged-in ChatGPT (chatgpt_web). Returns the image itself as bytes (base64 PNG), not a link."
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
