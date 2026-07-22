use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
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
    #[schemars(with = "String")]
    pub query: Option<String>,
    /// Force a specific provider instead of the priority order.
    #[serde(default)]
    #[schemars(with = "ProviderKind")]
    pub provider: Option<ProviderKind>,
    /// Maximum number of results (default 5, capped at 20).
    #[serde(default)]
    #[schemars(with = "u32")]
    pub max_results: Option<u32>,
    /// Resume token from a previous web-provider result; continues that conversation
    /// on the same provider. Web providers only.
    #[serde(default)]
    #[schemars(with = "String")]
    pub session: Option<String>,
    /// Web-provider model and/or thinking level (gemini "3.1 pro"/"flash", grok "grok-4",
    /// chatgpt "gpt-5.4 high"/"o3"/"high" — levels vary per model; an unknown name returns
    /// the actual options). Ignored by the API providers.
    #[serde(default)]
    #[schemars(with = "String")]
    pub model: Option<String>,
    /// Provider-specific mode. grok: "auto"/"fast"/"expert"/"heavy" (search defaults to fast,
    /// deep_research to heavy then expert).
    #[serde(default)]
    #[schemars(with = "String")]
    pub mode: Option<String>,
    /// Research niche: "web" (default), "news", or "academic" — steers to a fitting backend.
    #[serde(default)]
    #[schemars(with = "String")]
    pub topic: Option<String>,
    /// Recency filter: "day"/"week"/"month"/"year" or an ISO date (e.g. "2024-01-01").
    #[serde(default)]
    #[schemars(with = "String")]
    pub recency: Option<String>,
    /// Restrict to these domains; prefix one with "-" to exclude it (e.g. ["nature.com","-reddit.com"]).
    #[serde(default)]
    #[schemars(with = "Vec<String>")]
    pub domains: Option<Vec<String>>,
    /// Absolute paths of local files/images to attach and ask about. Web providers only;
    /// defaults to grok_web when no `provider` is forced.
    #[serde(default)]
    #[schemars(with = "Vec<String>")]
    pub file: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ResearchArgs {
    #[serde(flatten)]
    pub base: SearchArgs,
    /// Depth: "standard" (default) or "deep" — deep is slower and may spend a paid balance
    /// (exa deep-reasoning, parallel pro tier, grok heavy).
    #[serde(default)]
    #[schemars(with = "String")]
    pub depth: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ReadArgs {
    /// The URL to read and return as clean markdown.
    pub url: String,
    /// Force a specific provider instead of the priority order.
    #[serde(default)]
    #[schemars(with = "ProviderKind")]
    pub provider: Option<ProviderKind>,
    /// Provider-specific escape hatch: firecrawl "crawl"/"extract"/"screenshot", tavily "extract",
    /// serper "scrape", steel "screenshot"/"pdf". Call usage(provider=…) for the exact set.
    #[serde(default)]
    #[schemars(with = "String")]
    pub mode: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct BrowserArgs {
    /// The URL to load in a headless browser.
    pub url: String,
    /// Reserved for multi-step automation; ignored in v1.
    #[serde(default)]
    #[schemars(with = "Vec<String>")]
    pub actions: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ImageArgs {
    /// What to draw.
    pub prompt: String,
    /// Force a specific provider (gemini_web / grok_web generate in-process over HTTP; chatgpt_web
    /// drives the browser). Otherwise the priority order applies, with failover.
    #[serde(default)]
    #[schemars(with = "ProviderKind")]
    pub provider: Option<ProviderKind>,
    /// Absolute path to save the image to. When set, only the path is returned (no inline
    /// bytes); otherwise it is saved under the fetchira home dir and also returned inline.
    #[serde(default)]
    #[schemars(with = "String")]
    pub path: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct UsageArgs {
    /// A provider to get the full capability sheet for (niches, modes, example calls + live limits).
    /// Omit for the compact all-provider quota snapshot.
    #[serde(default)]
    #[schemars(with = "ProviderKind")]
    pub provider: Option<ProviderKind>,
}

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
                    return Ok(image_result(img, None));
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
        file: args
            .file
            .unwrap_or_default()
            .into_iter()
            .map(std::path::PathBuf::from)
            .collect(),
        ..Default::default()
    }
}

#[tool_router]
impl Fetchira {
    #[tool(
        description = "Web search across quota-aware providers. API providers (serper, tavily, exa, parallel) return ranked title/url/snippet results; web providers (gemini_web, grok_web, chatgpt_web) return a synthesized answer with sources and a `session` token for follow-up turns. For chatgpt_web this is a full chat turn with web search on by default — `mode=\"chat\"` answers from the model alone. Attach local files with `file` to ask about them. Niche knobs: `topic`, `recency`, `domains`. Provider-specific extras (scholar/patents/places, structured extract…) → call usage(provider=…) for exact params & example calls."
    )]
    pub async fn search(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (forced, session) = route(args.provider, args.session.clone());
        let input = search_input(args, session);
        // An attachment needs a web session that can upload; grok_web is the default carrier.
        let forced = forced.or_else(|| (!input.file.is_empty()).then_some(ProviderKind::GrokWeb));
        self.run(Capability::Search, input, forced).await
    }

    #[tool(
        description = "Read a URL and return its main content as clean markdown; auto-escalates to a headless browser when the plain read comes back empty. Provider-specific extras via `mode` (crawl, structured extract, screenshot…) → call usage(provider=…) for exact params & example calls."
    )]
    pub async fn read(
        &self,
        Parameters(args): Parameters<ReadArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let input = Input {
            url: Some(args.url),
            mode: args.mode,
            ..Default::default()
        };
        self.run(Capability::Read, input, args.provider).await
    }

    #[tool(
        description = "Deep research over multiple sources (parallel, exa, tavily, gemini_web, grok_web, chatgpt_web); may take minutes. gemini_web and chatgpt_web first return a research PLAN plus a `session`: pass that `session` with query \"start\" to run it, or with a revised request to replace the plan. gemini returns the finished report in the same call; chatgpt runs ~5-30 min after \"start\" — call again with the returned `session` to fetch the report (`model` is ignored there). Knobs: `topic`, `recency`, `domains`, `depth` (\"deep\" is slower/pricier). Provider-specific extras → call usage(provider=…)."
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
        description = "Show remaining free quota per account and provider for the current period, plus each account's assigned proxy. For logged-in web sessions (grok/gemini/chatgpt) also includes live per-tier limits and the selectable model/mode catalog with thinking levels — a locked mode (e.g. grok Expert/Heavy on a lapsed sub) reads 0/0. Pass `provider` for that backend's full capability sheet — its niches, escape-hatch `mode` values, and ready-to-copy example calls."
    )]
    pub async fn usage(
        &self,
        Parameters(args): Parameters<UsageArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match self.router.usage_snapshot().await {
            Ok(views) => {
                let text = match args.provider {
                    Some(p) => crate::router::provider_sheet(p, &views),
                    None => crate::router::compact_usage(&views),
                };
                CallToolResult::success(vec![Content::text(text)])
            }
            Err(e) => CallToolResult::error(vec![Content::text(e.to_string())]),
        })
    }

    #[tool(
        description = "Generate an image from a text prompt via a logged-in web session (gemini_web / grok_web / chatgpt_web). Saves the image to disk and returns its path — pass `path` to pick where (then no inline bytes). Force one with `provider`; otherwise the router chooses and fails over."
    )]
    pub async fn create_image(
        &self,
        Parameters(args): Parameters<ImageArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let input = Input {
            query: Some(args.prompt),
            ..Default::default()
        };
        Ok(
            match self
                .router
                .call(Capability::Image, &input, args.provider)
                .await
            {
                Ok(reply) => match reply.image {
                    Some(img) => image_result(img, args.path),
                    None => CallToolResult::success(vec![Content::text(reply.text)]),
                },
                Err(e) => CallToolResult::error(vec![Content::text(e.to_string())]),
            },
        )
    }
}

/// Agents can't reach inline MCP bytes, so every image also lands on disk and the
/// result names the file. An explicit `path` means "file only" — skip the inline copy.
fn image_result(img: crate::providers::OutImage, path: Option<String>) -> CallToolResult {
    let bytes = match base64::engine::general_purpose::STANDARD.decode(&img.b64) {
        Ok(b) => b,
        Err(e) => return CallToolResult::error(vec![Content::text(format!("bad image: {e}"))]),
    };
    let explicit = path.is_some();
    let dest = path.map(PathBuf::from).unwrap_or_else(|| {
        let ext = match img.mime.as_str() {
            "image/jpeg" => "jpg",
            m => m.rsplit('/').next().unwrap_or("png"),
        };
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        crate::cli::home()
            .join("images")
            .join(format!("img-{ts}.{ext}"))
    });
    if let Some(dir) = dest.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&dest, &bytes) {
        return CallToolResult::error(vec![Content::text(format!(
            "write {} failed: {e}",
            dest.display()
        ))]);
    }
    let note = Content::text(format!(
        "saved: {} ({}, {} bytes)",
        dest.display(),
        img.mime,
        bytes.len()
    ));
    if explicit {
        CallToolResult::success(vec![note])
    } else {
        CallToolResult::success(vec![Content::image(img.b64, img.mime), note])
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Fetchira {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("fetchira", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Quota-aware router over free web-search/scrape providers. \
                 Tools: search, read, deep_research, browser, create_image, usage. Pass `provider` \
                 to force a specific backend (it is used even if its quota looks spent); otherwise \
                 providers are tried in the user's priority order with automatic failover.",
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::OutImage;

    #[test]
    fn image_result_writes_explicit_path() {
        let dest = std::env::temp_dir().join("fetchira-test-img.png");
        let _ = std::fs::remove_file(&dest);
        let img = OutImage {
            mime: "image/png".into(),
            b64: base64::engine::general_purpose::STANDARD.encode(b"fakepng"),
        };
        let out = image_result(img, Some(dest.to_string_lossy().into_owned()));
        assert_eq!(std::fs::read(&dest).unwrap(), b"fakepng");
        assert_eq!(out.content.len(), 1);
        let _ = std::fs::remove_file(&dest);
    }

    #[test]
    fn image_result_bad_b64_is_error() {
        let img = OutImage {
            mime: "image/png".into(),
            b64: "%%%".into(),
        };
        assert_eq!(image_result(img, None).is_error, Some(true));
    }

    #[test]
    fn tool_input_schemas_omit_nullable_optional_fields() {
        fn assert_schema_is_non_nullable(value: &serde_json::Value, path: &str) {
            if let Some(nullable) = value.get("nullable") {
                panic!("{path}.nullable must be absent, got {nullable}");
            }
            if value.get("type") == Some(&serde_json::Value::String("null".into())) {
                panic!("{path}.type must not be null");
            }
            if value
                .get("type")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|types| types.iter().any(|item| item == "null"))
            {
                panic!("{path}.type must not contain null");
            }

            match value {
                serde_json::Value::Array(items) => {
                    for (index, item) in items.iter().enumerate() {
                        assert_schema_is_non_nullable(item, &format!("{path}[{index}]"));
                    }
                }
                serde_json::Value::Object(fields) => {
                    for (key, item) in fields {
                        assert_schema_is_non_nullable(item, &format!("{path}.{key}"));
                    }
                }
                serde_json::Value::Null
                | serde_json::Value::Bool(_)
                | serde_json::Value::Number(_)
                | serde_json::Value::String(_) => {}
            }
        }

        let tools = Fetchira::tool_router().list_all();
        assert_eq!(tools.len(), 6);

        let expected_required = [
            ("search", &[][..]),
            ("deep_research", &[][..]),
            ("read", &["url"][..]),
            ("browser", &["url"][..]),
            ("create_image", &["prompt"][..]),
            ("usage", &[][..]),
        ];

        for tool in tools {
            let schema = serde_json::to_value(tool.input_schema).expect("schema must serialize");
            assert_schema_is_non_nullable(&schema, &tool.name);
            let required = schema
                .get("required")
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|item| item.as_str().expect("required names are strings"))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let expected = expected_required
                .iter()
                .find(|(name, _)| *name == tool.name)
                .map(|(_, fields)| fields.to_vec())
                .expect("unexpected tool");
            assert_eq!(required, expected, "required fields for {}", tool.name);
        }
    }
}
