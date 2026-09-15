use std::path::PathBuf;
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolResult, Content, Implementation, ResourceContents, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{tool, tool_handler, tool_router, ErrorData, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::invoke::{
    route, save_image, session_footer, validate_input, CONTINUE, IMAGE_PENDING, IMAGE_READY,
};
use crate::providers::{Capability, Input, ProviderKind};
use crate::router::Router;

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// The search query (or a follow-up message when `session` is set). Optional only when polling a
    /// running deep-research `session` for its report.
    #[serde(default)]
    pub query: Option<String>,
    /// Force a specific provider instead of the priority order.
    pub provider: Option<ProviderKind>,
    /// Maximum number of results (default 5, capped at 20).
    pub max_results: Option<u32>,
    /// Resume token from a previous web-provider result; continues that conversation
    /// on the same provider. Web providers only.
    pub session: Option<String>,
    /// Web-provider model and/or thinking level. Call usage with that provider for available
    /// names and levels; omit to use its default. Ignored by the API providers.
    pub model: Option<String>,
    /// Provider-specific mode. grok: "auto"/"fast"/"expert"/"heavy" (search defaults to fast,
    /// deep_research to heavy then expert).
    pub mode: Option<String>,
    /// Research niche: "web" (default), "news", or "academic" — steers to a fitting backend.
    pub topic: Option<String>,
    /// Recency filter: "day"/"week"/"month"/"year" or an ISO date (e.g. "2024-01-01").
    pub recency: Option<String>,
    /// Restrict to these domains; prefix one with "-" to exclude it (e.g. ["nature.com","-reddit.com"]).
    pub domains: Option<Vec<String>>,
    /// Absolute paths of local files/images to attach and ask about. Local stdio only;
    /// hosted HTTP rejects file inputs because it cannot access the caller's filesystem.
    /// Web providers only;
    /// defaults to grok_web when no `provider` is forced.
    pub file: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ResearchArgs {
    #[serde(flatten)]
    pub base: SearchArgs,
    /// Depth: "standard" (default) or "deep" — deep is slower and may spend a paid balance
    /// (exa deep-reasoning, parallel core processor, grok heavy).
    pub depth: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ReadArgs {
    /// The URL to read and return as clean markdown.
    pub url: String,
    /// Force a specific provider instead of the priority order.
    pub provider: Option<ProviderKind>,
    /// Provider-specific escape hatch: firecrawl "crawl"/"extract", tavily "extract",
    /// serper "scrape", steel "screenshot"/"pdf". Call usage(provider=…) for the exact set.
    pub mode: Option<String>,
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
    /// What to draw, or how to edit the attached image.
    pub prompt: String,
    /// Force a specific provider (gemini_web / grok_web generate in-process over HTTP; chatgpt_web
    /// drives the browser). Otherwise the priority order applies, with failover.
    pub provider: Option<ProviderKind>,
    /// Local destination for stdio or the local hosted bridge. With a path, return the saved
    /// path without inline bytes; otherwise save under the local Fetchira home and include bytes.
    /// Direct hosted HTTP returns inline artifacts and never writes caller-supplied server paths.
    pub path: Option<String>,
    /// Resume a ChatGPT image conversation to edit its previous image.
    pub session: Option<String>,
    /// Absolute paths of images/files to attach (local stdio only, not hosted HTTP).
    /// Use this to edit an existing/generated image.
    pub file: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct UsageArgs {
    /// A provider to get the full capability sheet for (niches, modes, example calls + live limits).
    /// Omit for the compact all-provider quota snapshot.
    pub provider: Option<ProviderKind>,
}

pub struct Fetchira {
    router: Arc<tokio::sync::RwLock<Arc<Router>>>,
    tool_router: ToolRouter<Self>,
}

impl Fetchira {
    pub fn new(router: Arc<Router>) -> Self {
        Self::new_shared(Arc::new(tokio::sync::RwLock::new(router)))
    }

    pub fn new_shared(router: Arc<tokio::sync::RwLock<Arc<Router>>>) -> Self {
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
        write_file: bool,
    ) -> Result<CallToolResult, ErrorData> {
        let router = self.router.read().await.clone();
        Ok(match router.call(cap, &input, forced).await {
            Ok(reply) => {
                if let Some(img) = reply.image {
                    return Ok(image_result_with_write(
                        img,
                        None,
                        reply.session,
                        write_file,
                    ));
                }
                let mut text = reply.text;
                session_footer(&mut text, reply.session.as_deref(), CONTINUE);
                CallToolResult::success(vec![Content::text(text)])
            }
            Err(e) => CallToolResult::error(vec![Content::text(e.to_string())]),
        })
    }

    async fn run_http(
        &self,
        context: RequestContext<RoleServer>,
        cap: Capability,
        input: Input,
        forced: Option<ProviderKind>,
    ) -> Result<CallToolResult, ErrorData> {
        let cancellation = context.ct.clone();
        let is_http = context.extensions.get::<http::request::Parts>().is_some();
        validate_file_input(&input.file, is_http)?;
        let Some(request_id) = context
            .extensions
            .get::<http::request::Parts>()
            .and_then(|p| p.headers.get("x-fetchira-request-id"))
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
        else {
            return cancellable(cancellation, self.run(cap, input, forced, !is_http)).await?;
        };
        cancellable(
            cancellation,
            crate::usage::HOSTED_REQUEST_ID.scope(request_id, self.run(cap, input, forced, false)),
        )
        .await?
    }
}

pub(crate) async fn cancellable<T>(
    cancellation: tokio_util::sync::CancellationToken,
    future: impl std::future::Future<Output = T>,
) -> Result<T, ErrorData> {
    tokio::select! {
        result = future => Ok(result),
        _ = cancellation.cancelled() => Err(ErrorData::internal_error("request cancelled", None)),
    }
}

fn validate_file_input(files: &[PathBuf], is_http: bool) -> Result<(), ErrorData> {
    if is_http && !files.is_empty() {
        return Err(ErrorData::invalid_params(
            "File attachments require local stdio mode; hosted HTTP does not upload local files or accept server filesystem paths.",
            None,
        ));
    }
    Ok(())
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
        description = "Web search across quota-aware providers. API providers (serper, tavily, exa, parallel) return ranked title/url/snippet results; web providers (gemini_web, grok_web, chatgpt_web) return a synthesized answer with sources and a `session` token for follow-up turns. For chatgpt_web this is a full chat turn with web search on by default — `mode=\"chat\"` answers from the model alone. In local stdio mode, attach files with `file` to ask about them. Niche knobs: `topic`, `recency`, `domains`. Provider-specific extras (scholar/patents/places, structured extract…) → call usage(provider=…) for exact params & example calls."
    )]
    pub async fn search(
        &self,
        Parameters(args): Parameters<SearchArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let (forced, session) = route(args.provider, args.session.clone())
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        let input = search_input(args, session);
        validate_input(Capability::Search, &input, forced, input.query.is_some())
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        // An attachment needs a web session that can upload; grok_web is the default carrier.
        let forced = forced.or_else(|| (!input.file.is_empty()).then_some(ProviderKind::GrokWeb));
        self.run_http(context, Capability::Search, input, forced)
            .await
    }

    #[tool(
        description = "Read a URL and return its main content as clean markdown; auto-escalates to a headless browser when the plain read comes back empty. Provider-specific extras via `mode` (crawl, structured extract, screenshot…) → call usage(provider=…) for exact params & example calls."
    )]
    pub async fn read(
        &self,
        Parameters(args): Parameters<ReadArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let input = Input {
            url: Some(args.url),
            mode: args.mode,
            ..Default::default()
        };
        validate_input(Capability::Read, &input, args.provider, true)
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        self.run_http(context, Capability::Read, input, args.provider)
            .await
    }

    #[tool(
        description = "Deep research over multiple sources (parallel, exa, tavily, gemini_web, grok_web, chatgpt_web); may take minutes. gemini_web and chatgpt_web first return a research PLAN plus a `session`: pass that `session` with query \"start\" to run it, or with a revised request to replace the plan. gemini returns the finished report in the same call; chatgpt runs ~5-30 min after \"start\" — call again with the returned `session` to fetch the report (`model` is ignored there). Knobs: `topic`, `recency`, `domains`, `depth` (\"deep\" is slower/pricier). Provider-specific extras → call usage(provider=…)."
    )]
    pub async fn deep_research(
        &self,
        Parameters(args): Parameters<ResearchArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let (forced, session) = route(args.base.provider, args.base.session.clone())
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        let mut input = search_input(args.base, session);
        input.depth = args.depth;
        validate_input(
            Capability::DeepResearch,
            &input,
            forced,
            input.query.is_some(),
        )
        .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        self.run_http(context, Capability::DeepResearch, input, forced)
            .await
    }

    #[tool(
        description = "Load a URL in a headless browser (Steel) and return the page as markdown."
    )]
    pub async fn browser(
        &self,
        Parameters(args): Parameters<BrowserArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let input = Input {
            url: Some(args.url),
            ..Default::default()
        };
        validate_input(Capability::Browser, &input, None, true)
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        self.run_http(context, Capability::Browser, input, None)
            .await
    }

    #[tool(
        description = "Show remaining free quota per account and provider for the current period, plus each account's assigned proxy. For logged-in web sessions (grok/gemini/chatgpt) also includes live per-tier limits and the selectable model/mode catalog with thinking levels — a locked mode (e.g. grok Expert/Heavy on a lapsed sub) reads 0/0. Pass `provider` for that backend's full capability sheet — its niches, escape-hatch `mode` values, and ready-to-copy example calls."
    )]
    pub async fn usage(
        &self,
        Parameters(args): Parameters<UsageArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let cancellation = context.ct.clone();
        let router = self.router.read().await.clone();
        let future = router.usage_snapshot();
        let snapshot = if let Some(request_id) = context
            .extensions
            .get::<http::request::Parts>()
            .and_then(|p| p.headers.get("x-fetchira-request-id"))
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
        {
            cancellable(
                cancellation,
                crate::usage::HOSTED_REQUEST_ID.scope(request_id, future),
            )
            .await?
        } else {
            cancellable(cancellation, future).await?
        };
        Ok(match snapshot {
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
        description = "Generate or edit an image via a logged-in web session. For a new image pass `prompt`; in local stdio mode, edit an existing image by passing its absolute path in `file` and describing the changes. To continue editing a ChatGPT-generated image in the same chat, pass the returned `session` back. Local stdio and the local hosted bridge save the result and return its path; use `path` to choose the local destination. Direct hosted HTTP returns inline artifacts without writing server paths."
    )]
    pub async fn create_image(
        &self,
        Parameters(args): Parameters<ImageArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let cancellation = context.ct.clone();
        let (forced, session) = route(args.provider, args.session)
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        let input = Input {
            query: Some(args.prompt),
            session,
            file: args
                .file
                .unwrap_or_default()
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            ..Default::default()
        };
        validate_input(Capability::Image, &input, forced, true)
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        validate_file_input(
            &input.file,
            context.extensions.get::<http::request::Parts>().is_some(),
        )?;
        let router = self.router.read().await.clone();
        let future = router.call(Capability::Image, &input, forced);
        let result = if let Some(request_id) = context
            .extensions
            .get::<http::request::Parts>()
            .and_then(|p| p.headers.get("x-fetchira-request-id"))
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
        {
            cancellable(
                cancellation,
                crate::usage::HOSTED_REQUEST_ID.scope(request_id, future),
            )
            .await?
        } else {
            cancellable(cancellation, future).await?
        };
        Ok(match result {
            Ok(reply) => match reply.image {
                Some(img) => image_result_with_write(
                    img,
                    args.path,
                    reply.session,
                    context.extensions.get::<http::request::Parts>().is_none(),
                ),
                None => image_pending_result(reply.text, reply.session),
            },
            Err(e) => CallToolResult::error(vec![Content::text(e.to_string())]),
        })
    }
}

fn image_pending_result(mut text: String, session: Option<String>) -> CallToolResult {
    session_footer(&mut text, session.as_deref(), IMAGE_PENDING);
    CallToolResult::success(vec![Content::text(text)])
}

/// Local stdio callers get a file because agents cannot consume inline MCP bytes. Hosted HTTP
/// callers skip this write; the local remote bridge materializes the returned image instead.
#[cfg(test)]
fn image_result(
    img: crate::providers::OutImage,
    path: Option<String>,
    session: Option<String>,
) -> CallToolResult {
    image_result_with_write(img, path, session, true)
}

fn image_result_with_write(
    img: crate::providers::OutImage,
    path: Option<String>,
    session: Option<String>,
    write_file: bool,
) -> CallToolResult {
    if !write_file {
        let mut content = vec![inline_artifact(&img)];
        if let Some(s) = session {
            content.push(Content::text(format!(
                "⟦session: {s} — pass as `session` to create_image to edit this image in the same chat⟧"
            )));
        }
        return CallToolResult::success(content);
    }
    let explicit = path.is_some();
    let mut note = match save_image(&img, path.as_deref().map(std::path::Path::new)) {
        Ok(note) => note,
        Err(e) => return CallToolResult::error(vec![Content::text(format!("{e:#}"))]),
    };
    session_footer(&mut note, session.as_deref(), IMAGE_READY);
    let note = Content::text(note);
    if explicit {
        CallToolResult::success(vec![note])
    } else {
        CallToolResult::success(vec![inline_artifact(&img), note])
    }
}

fn inline_artifact(img: &crate::providers::OutImage) -> Content {
    if img.mime == "application/pdf" {
        Content::resource(
            ResourceContents::blob(&img.b64, "fetchira:///artifact.pdf").with_mime_type(&img.mime),
        )
    } else {
        Content::image(&img.b64, &img.mime)
    }
}

fn update_error() -> ErrorData {
    ErrorData::internal_error(
        crate::instances::UPDATING,
        Some(serde_json::json!({"reason":"fetchira_updating","retryable":true})),
    )
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Fetchira {
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let home = crate::cli::home();
        let http = context.extensions.get::<http::request::Parts>();
        let hosted_cancel = context
            .extensions
            .get::<tokio_util::sync::CancellationToken>()
            .cloned()
            .or_else(|| {
                http.and_then(|parts| {
                    parts
                        .extensions
                        .get::<tokio_util::sync::CancellationToken>()
                })
                .cloned()
            });
        let _admission = if http.is_none() {
            Some(crate::instances::admit_request(&home).map_err(|_| update_error())?)
        } else {
            None
        };
        let call = self
            .tool_router
            .call(rmcp::handler::server::tool::ToolCallContext::new(
                self, request, context,
            ));
        let cancellation = async {
            match hosted_cancel {
                Some(token) => token.cancelled().await,
                None if _admission.is_some() => crate::instances::forced(&home).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! { biased;
            _ = cancellation => Err(update_error()),
            result = call => result,
        }
    }

    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("fetchira", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Quota-aware router over free web-search/scrape providers. \
                 Tools: search, read, deep_research, browser, create_image, usage. Pass `provider` \
                 to restrict routing to a specific backend (quota and retry limits still apply); otherwise \
                 providers are tried in the user's priority order with automatic failover.",
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::OutImage;
    use base64::Engine;
    use rmcp::model::RawContent;

    #[test]
    fn hosted_file_inputs_cannot_read_server_files() {
        let files = [PathBuf::from("/run/secrets/master_key")];
        assert!(validate_file_input(&files, true).is_err());
        assert!(validate_file_input(&files, false).is_ok());
        assert!(validate_file_input(&[], true).is_ok());
    }

    #[test]
    fn image_result_writes_explicit_path() {
        let dest = std::env::temp_dir().join("fetchira-test-img.png");
        let _ = std::fs::remove_file(&dest);
        let img = OutImage {
            mime: "image/png".into(),
            b64: base64::engine::general_purpose::STANDARD.encode(b"fakepng"),
        };
        let out = image_result(img, Some(dest.to_string_lossy().into_owned()), None);
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
        assert_eq!(image_result(img, None, None).is_error, Some(true));
    }

    #[test]
    fn pending_image_keeps_session() {
        let out = image_pending_result(
            "Image is still generating.".into(),
            Some("chatgpt_web:abc:img|poll||query".into()),
        );
        assert_ne!(out.is_error, Some(true));
        let RawContent::Text(text) = &out.content[0].raw else {
            panic!("expected text");
        };
        assert!(text.text.contains("still generating"));
        assert!(text
            .text
            .contains("⟦session: chatgpt_web:abc:img|poll||query"));
    }

    #[test]
    fn hosted_pdf_is_an_inline_blob_and_keeps_session() {
        let out = image_result_with_write(
            OutImage {
                mime: "application/pdf".into(),
                b64: base64::engine::general_purpose::STANDARD.encode(b"%PDF-test"),
            },
            None,
            Some("steel:test".into()),
            false,
        );
        assert!(matches!(out.content[0].raw, RawContent::Resource(_)));
        let RawContent::Text(text) = &out.content[1].raw else {
            panic!("expected session text");
        };
        assert!(text.text.contains("⟦session: steel:test"));
    }
}
