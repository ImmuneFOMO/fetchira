//! Local one-shot commands and the small pieces shared with MCP.
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use base64::Engine;

use crate::providers::{Capability, Input, OutImage, ProviderKind};
use crate::router::{Reply, Router};

pub(crate) const CONTINUE: &str = "pass as \"session\" to continue this conversation";
pub(crate) const IMAGE_PENDING: &str =
    "pass as `session` to create_image to fetch or edit this image";
pub(crate) const IMAGE_READY: &str =
    "pass as `session` to create_image to edit this image in the same chat";

pub(crate) fn session_footer(text: &mut String, session: Option<&str>, hint: &str) {
    if let Some(s) = session {
        text.push_str(&format!("\n\n⟦session: {s} — {hint}⟧"));
    }
}

/// Strip only the provider prefix; Router owns account-affinity decoding.
pub(crate) fn route(
    provider: Option<ProviderKind>,
    session: Option<String>,
) -> anyhow::Result<(Option<ProviderKind>, Option<String>)> {
    let Some(session) = session else {
        return Ok((provider, None));
    };
    let (prefix, rest) = session
        .split_once(':')
        .filter(|(_, rest)| !rest.trim().is_empty())
        .context("invalid session: pass the exact token returned by Fetchira")?;
    Ok((Some(parse_provider(prefix)?), Some(rest.to_string())))
}

/// Validate the generic search knobs before either CLI or MCP builds the router.
/// Providers map these values differently, so silently accepting a typo would produce
/// plausible-looking results without the requested filter.
pub(crate) fn validate_search_input(input: &Input) -> anyhow::Result<()> {
    if let Some(topic) = input.topic.as_deref() {
        if !matches!(topic, "web" | "news" | "academic") {
            bail!("topic must be web, news, or academic");
        }
    }
    if let Some(recency) = input.recency.as_deref() {
        let valid = matches!(recency, "day" | "week" | "month" | "year")
            || chrono::NaiveDate::parse_from_str(recency, "%Y-%m-%d").is_ok();
        if !valid {
            bail!("recency must be day, week, month, year, or an ISO date (YYYY-MM-DD)");
        }
    }
    if let Some(depth) = input.depth.as_deref() {
        if !matches!(depth, "standard" | "deep") {
            bail!("depth must be standard or deep");
        }
    }
    Ok(())
}

/// Validate the shared MCP/CLI input contract before a request reaches a provider.
pub(crate) fn validate_input(
    cap: Capability,
    input: &Input,
    provider: Option<ProviderKind>,
    text_supplied: bool,
) -> anyhow::Result<()> {
    match cap {
        Capability::Read | Capability::Browser => {
            if input
                .url
                .as_deref()
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .is_none()
            {
                bail!("expected exactly one URL");
            }
        }
        Capability::Search | Capability::DeepResearch | Capability::Image => {
            let empty = input
                .query
                .as_deref()
                .map(str::trim)
                .is_none_or(str::is_empty);
            if empty
                && !allows_empty_query_parts(provider, cap, input.session.as_deref(), text_supplied)
            {
                bail!(
                    "missing {}",
                    if cap == Capability::Image {
                        "prompt"
                    } else {
                        "query"
                    }
                );
            }
        }
    }
    if matches!(cap, Capability::Search | Capability::DeepResearch) {
        validate_search_input(input)?;
    }
    Ok(())
}

fn parse_provider(s: &str) -> anyhow::Result<ProviderKind> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .with_context(|| format!("unknown provider '{s}' (try `fetchira providers`)"))
}

fn absolute(path: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
    let path = path.as_ref();
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    })
}

// MIME comes from a provider; never use it as a path component (including on Windows).
fn image_extension(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "image/avif" => "avif",
        "application/pdf" => "pdf",
        _ => "png",
    }
}

/// Materialize bytes once for CLI and local MCP; hosted MCP keeps inline bytes.
pub(crate) fn save_image(img: &OutImage, path: Option<&Path>) -> anyhow::Result<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&img.b64)
        .context("bad image")?;
    let dest = match path {
        Some(path) => absolute(path)?,
        None => {
            let ext = image_extension(&img.mime);
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            absolute(
                crate::cli::home()
                    .join("images")
                    .join(format!("img-{ts}.{ext}")),
            )?
        }
    };
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {} failed", dir.display()))?;
    }
    std::fs::write(&dest, &bytes).with_context(|| format!("write {} failed", dest.display()))?;
    Ok(format!(
        "saved: {} ({}, {} bytes)",
        dest.display(),
        img.mime,
        bytes.len()
    ))
}

pub fn command_help(command: &str) -> String {
    let search_flags = "[--provider P] [--max N] [--session S] [--model M] [--mode M] [--topic T] [--recency R] [--domain D]... [--file PATH]...";
    let args = match command {
        "search" => format!("QUERY... {search_flags}"),
        "deep_research" | "dr" => format!("QUERY... {search_flags} [--depth standard|deep]"),
        "read" => "URL [--provider P] [--mode M]".into(),
        "browser" => "URL".into(),
        "create_image" => {
            "PROMPT... [--provider P] [--path DEST] [--session S] [--file PATH]...".into()
        }
        "usage" => "[PROVIDER]".into(),
        _ => unreachable!("only CLI tool commands"),
    };
    format!("usage: fetchira {command} {args}\nFlags may mix with words; -- ends flags. ChatGPT research/image poll sessions may omit text.")
}

#[derive(Debug)]
struct Request {
    capability: Option<Capability>, // None is the usage snapshot, not a provider call.
    input: Input,
    provider: Option<ProviderKind>,
    /// Keep the prefixed form for a hosted MCP call; local routing consumes the prefix.
    remote_session: Option<String>,
    path: Option<PathBuf>,
}

fn parse(command: &str, args: impl Iterator<Item = String>) -> anyhow::Result<Request> {
    parse_args(command, args).with_context(|| command_help(command))
}

fn parse_args(command: &str, mut args: impl Iterator<Item = String>) -> anyhow::Result<Request> {
    let cap = Capability::parse(command);
    let search = matches!(cap, Some(Capability::Search | Capability::DeepResearch));
    let image = cap == Some(Capability::Image);
    let mut req = Request {
        capability: cap,
        input: Input::default(),
        provider: None,
        remote_session: None,
        path: None,
    };
    let mut words = Vec::new();
    let mut session = None;
    let mut flags = true;
    while let Some(arg) = args.next() {
        if flags && arg == "--" {
            flags = false;
            continue;
        }
        if !flags || !arg.starts_with('-') || arg == "-" {
            words.push(arg);
            continue;
        }
        let allowed = match arg.as_str() {
            "--provider" => search || image || cap == Some(Capability::Read),
            "--session" | "--file" => search || image,
            "--mode" => search || cap == Some(Capability::Read),
            "--max" | "--model" | "--topic" | "--recency" | "--domain" => search,
            "--depth" => cap == Some(Capability::DeepResearch),
            "--path" => image,
            _ => false,
        };
        if !allowed {
            bail!("unknown flag '{arg}' for {command}");
        }
        let value = crate::cli::flag_value(&mut args, &arg)?;
        match arg.as_str() {
            "--provider" => req.provider = Some(parse_provider(&value)?),
            "--session" => session = Some(value),
            "--file" => req.input.file.push(absolute(value)?),
            "--path" => req.path = Some(absolute(value)?),
            "--max" => {
                req.input.max_results = Some(
                    value
                        .parse::<u32>()
                        .context("--max must be an unsigned integer")?,
                )
            }
            "--model" => req.input.model = Some(value),
            "--mode" => req.input.mode = Some(value),
            "--topic" => req.input.topic = Some(value),
            "--recency" => req.input.recency = Some(value),
            "--domain" => req.input.domains.get_or_insert_with(Vec::new).push(value),
            "--depth" => {
                if !matches!(value.as_str(), "standard" | "deep") {
                    bail!("--depth must be standard or deep");
                }
                req.input.depth = Some(value);
            }
            _ => unreachable!(),
        }
    }
    req.remote_session = session.clone();
    (req.provider, req.input.session) = route(req.provider, session)?;
    match cap {
        None => {
            if words.len() > 1 {
                bail!("usage accepts at most one provider");
            }
            req.provider = words.first().map(|p| parse_provider(p)).transpose()?;
        }
        Some(Capability::Read | Capability::Browser) => {
            if words.len() != 1 || words[0].trim().is_empty() {
                bail!("expected exactly one URL");
            }
            req.input.url = words.pop();
        }
        Some(_) => {
            let query = words.join(" ");
            req.input.query = (!words.is_empty()).then_some(query);
        }
    }
    // Match MCP search attachment routing; research and images keep their own priority order.
    if cap == Some(Capability::Search) && !req.input.file.is_empty() {
        req.provider = req.provider.or(Some(ProviderKind::GrokWeb));
    }
    if let Some(cap) = cap {
        validate_input(cap, &req.input, req.provider, !words.is_empty())?;
    }
    Ok(req)
}

fn allows_empty_query_parts(
    provider: Option<ProviderKind>,
    capability: Capability,
    session: Option<&str>,
    text_supplied: bool,
) -> bool {
    let Some(session) = session else {
        return false;
    };
    let opaque = crate::router::decode_session_affinity(session).map(|(_, opaque)| opaque);
    let token = opaque.as_deref().unwrap_or(session);
    match (provider, capability) {
        (Some(ProviderKind::ChatgptWeb), Capability::DeepResearch) => token.starts_with("dr|poll|"),
        (Some(ProviderKind::ChatgptWeb), Capability::Image) => token.starts_with("img|poll|"),
        // Gemini treats an explicitly empty follow-up as "start" for its research plan.
        (Some(ProviderKind::GeminiWeb), Capability::DeepResearch) => {
            text_supplied && token.starts_with("dr|")
        }
        _ => false,
    }
}

fn reply_text(reply: Reply, cap: Capability, path: Option<&Path>) -> anyhow::Result<String> {
    let (mut text, hint) = match reply.image {
        Some(img) => (save_image(&img, path)?, IMAGE_READY),
        None => (
            reply.text,
            if cap == Capability::Image {
                IMAGE_PENDING
            } else {
                CONTINUE
            },
        ),
    };
    session_footer(&mut text, reply.session.as_deref(), hint);
    Ok(text)
}

fn remote_arguments(req: &Request) -> rmcp::model::JsonObject {
    let mut args = serde_json::Map::new();
    let insert = |args: &mut serde_json::Map<String, serde_json::Value>, key: &str, value| {
        args.insert(key.to_string(), value);
    };
    if let Some(provider) = req.provider {
        insert(&mut args, "provider", serde_json::json!(provider));
    }
    if let Some(session) = req.remote_session.as_deref() {
        insert(&mut args, "session", serde_json::json!(session));
    }
    match req.capability {
        Some(Capability::Search | Capability::DeepResearch) => {
            if let Some(query) = req.input.query.as_deref() {
                insert(&mut args, "query", serde_json::json!(query));
            }
            if let Some(max_results) = req.input.max_results {
                insert(&mut args, "max_results", serde_json::json!(max_results));
            }
            for (key, value) in [
                ("model", req.input.model.as_deref()),
                ("mode", req.input.mode.as_deref()),
                ("topic", req.input.topic.as_deref()),
                ("recency", req.input.recency.as_deref()),
            ] {
                if let Some(value) = value {
                    insert(&mut args, key, serde_json::json!(value));
                }
            }
            if let Some(domains) = req.input.domains.as_ref() {
                insert(&mut args, "domains", serde_json::json!(domains));
            }
            if req.capability == Some(Capability::DeepResearch) {
                if let Some(depth) = req.input.depth.as_deref() {
                    insert(&mut args, "depth", serde_json::json!(depth));
                }
            }
        }
        Some(Capability::Read | Capability::Browser) => {
            insert(
                &mut args,
                "url",
                serde_json::json!(req.input.url.as_deref().unwrap_or_default()),
            );
            if req.capability == Some(Capability::Read) {
                if let Some(mode) = req.input.mode.as_deref() {
                    insert(&mut args, "mode", serde_json::json!(mode));
                }
            }
        }
        Some(Capability::Image) => {
            // MCP's image schema requires prompt even for a provider poll. An empty prompt
            // preserves the poll token while matching the provider's empty follow-up behavior.
            insert(
                &mut args,
                "prompt",
                serde_json::json!(req.input.query.as_deref().unwrap_or_default()),
            );
        }
        None => {}
    }
    args
}

fn remote_result_text(
    result: rmcp::model::CallToolResult,
    path: Option<&Path>,
) -> anyhow::Result<String> {
    if result.is_error == Some(true) {
        let message = result
            .content
            .iter()
            .filter_map(|content| content.raw.as_text().map(|text| text.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        if message.is_empty() {
            bail!("remote tool failed");
        }
        bail!("{message}");
    }
    let mut text = Vec::new();
    let mut saved = None;
    for content in result.content {
        match content.raw {
            rmcp::model::RawContent::Text(content) => text.push(content.text),
            rmcp::model::RawContent::Image(content) => {
                if saved.is_some() {
                    bail!("hosted tool returned multiple artifacts");
                }
                saved = Some(save_image(
                    &OutImage {
                        mime: content.mime_type,
                        b64: content.data,
                    },
                    path,
                )?);
            }
            rmcp::model::RawContent::Resource(content) => match content.resource {
                rmcp::model::ResourceContents::TextResourceContents { text: body, .. } => {
                    text.push(body)
                }
                rmcp::model::ResourceContents::BlobResourceContents {
                    blob, mime_type, ..
                } => {
                    if saved.is_some() {
                        bail!("hosted tool returned multiple artifacts");
                    }
                    saved = Some(save_image(
                        &OutImage {
                            mime: mime_type.unwrap_or_else(|| "application/octet-stream".into()),
                            b64: blob,
                        },
                        path,
                    )?);
                }
            },
            rmcp::model::RawContent::Audio(_) => {
                bail!("hosted tool returned an unsupported audio artifact");
            }
            rmcp::model::RawContent::ResourceLink(link) => {
                text.push(format!("resource: {}", link.uri));
            }
        }
    }
    if let Some(saved) = saved {
        if !text.is_empty() {
            text.insert(0, saved);
            return Ok(text.join("\n"));
        }
        return Ok(saved);
    }
    Ok(text.join("\n"))
}

async fn run_remote(cfg: &crate::config::Config, req: &Request) -> anyhow::Result<String> {
    if !req.input.file.is_empty() {
        bail!("File attachments require local stdio mode; hosted HTTP does not upload local files or accept server filesystem paths.");
    }
    let Some(capability) = req.capability else {
        let result = crate::remote::call_tool(cfg, "usage", remote_arguments(req)).await?;
        return remote_result_text(result, None);
    };
    let result = crate::remote::call_tool(cfg, capability.as_str(), remote_arguments(req)).await?;
    remote_result_text(result, req.path.as_deref())
}

pub async fn run(
    home: &Path,
    command: &str,
    args: impl Iterator<Item = String>,
) -> anyhow::Result<()> {
    let args: Vec<_> = args.collect();
    if matches!(args.as_slice(), [arg] if arg == "--help" || arg == "-h") {
        println!("{}", command_help(command));
        return Ok(());
    }
    let req = parse(command, args.into_iter())?;
    let cfg = crate::cli::load_or_empty(home)?;
    if cfg.remote.endpoint.is_some() {
        let text = run_remote(&cfg, &req).await?;
        println!("{text}");
        return Ok(());
    }
    let store = crate::usage::Store::open(&crate::config::resolve_db(home, &cfg.db_path)).await?;
    let router = Router::build(cfg, store).await?;
    let text = match req.capability {
        Some(cap) => reply_text(
            router.call(cap, &req.input, req.provider).await?,
            cap,
            req.path.as_deref(),
        )?,
        None => {
            let views = router.usage_snapshot().await?;
            match req.provider {
                Some(p) => crate::router::provider_sheet(p, &views),
                None => crate::router::compact_usage(&views),
            }
        }
    };
    println!("{text}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(cmd: &str, args: &[&str]) -> anyhow::Result<Request> {
        parse(cmd, args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn mixed_flags_paths_and_session_affinity() {
        let req = request(
            "search",
            &[
                "what",
                "--provider",
                "serper",
                "--file",
                "one.txt",
                "changed",
                "--domain",
                "nature.com",
                "--domain",
                "-reddit.com",
                "--file",
                "two.png",
                "--max",
                "99",
                "--session",
                "chatgpt_web:YWNjdA:opaque:keep",
            ],
        )
        .unwrap();
        assert_eq!(req.input.query.as_deref(), Some("what changed"));
        assert_eq!(req.provider, Some(ProviderKind::ChatgptWeb));
        assert_eq!(req.input.session.as_deref(), Some("YWNjdA:opaque:keep"));
        assert_eq!(req.input.results(), 20);
        assert_eq!(req.input.domains.unwrap(), ["nature.com", "-reddit.com"]);
        assert_eq!(
            req.input.file,
            [absolute("one.txt").unwrap(), absolute("two.png").unwrap()]
        );
        let req = request("search", &["--", "--provider", "words"]).unwrap();
        assert_eq!(req.input.query.as_deref(), Some("--provider words"));
        assert_eq!(
            request("search", &["question", "--file", "x"])
                .unwrap()
                .provider,
            Some(ProviderKind::GrokWeb)
        );
    }

    #[test]
    fn command_validation_and_polling() {
        for (cmd, args) in [
            ("search", vec![]),
            ("search", vec![""]),
            ("create_image", vec!["   "]),
            ("dr", vec![" "]),
            ("read", vec![]),
            ("browser", vec!["a", "b"]),
            ("create_image", vec![]),
            ("search", vec!["x", "--max", "no"]),
            ("search", vec!["x", "--provider", "unknown"]),
            ("read", vec!["a", "--file", "x"]),
            ("search", vec!["x", "--model", "--max", "5"]),
            ("search", vec!["x", "--bogus"]),
            ("search", vec!["x", "--session", "broken-token"]),
            ("search", vec!["x", "--session", "chatgpt_web:"]),
            ("search", vec!["x", "--session", "unknown:token"]),
            ("dr", vec!["x", "--depth", "shallow"]),
            ("search", vec!["x", "--topic", "blogs"]),
            ("search", vec!["x", "--recency", "yesterday"]),
            ("search", vec!["x", "--recency", "2026-02-30"]),
            ("usage", vec!["serper", "tavily"]),
            ("search", vec!["--session", "chatgpt_web:YWNjdA:dr|poll|id"]),
            ("dr", vec!["--session", "gemini_web:YWNjdA:dr|poll|id"]),
        ] {
            let error = request(cmd, &args).unwrap_err();
            assert!(
                error.to_string().starts_with("usage: fetchira"),
                "{cmd}: {error}"
            );
        }
        for (cmd, token) in [
            ("dr", "chatgpt_web:YWNjdA:dr|poll|id"),
            ("create_image", "chatgpt_web:YWNjdA:img|poll|id||prompt"),
            ("deep_research", "chatgpt_web:dr|poll|id"),
        ] {
            let req = request(cmd, &["--session", token]).unwrap();
            assert!(req.input.query.is_none());
        }
        let req = request("dr", &["", "--session", "gemini_web:YWNjdA:dr|id"]).unwrap();
        assert_eq!(req.input.query.as_deref(), Some(""));
        assert_eq!(request("usage", &[]).unwrap().provider, None);
        assert_eq!(
            request("usage", &["serper"]).unwrap().provider,
            Some(ProviderKind::Serper)
        );
    }

    #[test]
    fn pending_image_has_session_without_saved_file() {
        let text = reply_text(
            Reply {
                text: "still generating".into(),
                session: Some("chatgpt_web:token".into()),
                image: None,
            },
            Capability::Image,
            None,
        )
        .unwrap();
        assert!(text.starts_with("still generating"));
        assert!(text.contains("⟦session: chatgpt_web:token"));
        assert!(!text.contains("saved:"));
    }

    #[test]
    fn hosted_arguments_keep_mcp_names_and_prefixed_session() {
        let req = request(
            "deep_research",
            &[
                "battery",
                "recycling",
                "--provider",
                "tavily",
                "--max",
                "7",
                "--session",
                "tavily:opaque-token",
                "--model",
                "research",
                "--mode",
                "expert",
                "--topic",
                "news",
                "--recency",
                "week",
                "--domain",
                "nature.com",
                "--depth",
                "deep",
            ],
        )
        .unwrap();
        let args = remote_arguments(&req);
        assert_eq!(args["query"], "battery recycling");
        assert_eq!(args["provider"], "tavily");
        assert_eq!(args["session"], "tavily:opaque-token");
        assert_eq!(args["max_results"], 7);
        assert_eq!(args["model"], "research");
        assert_eq!(args["mode"], "expert");
        assert_eq!(args["topic"], "news");
        assert_eq!(args["recency"], "week");
        assert_eq!(args["domains"], serde_json::json!(["nature.com"]));
        assert_eq!(args["depth"], "deep");
        assert!(!args.contains_key("file"));
    }

    #[test]
    fn hosted_image_poll_uses_empty_schema_prompt() {
        let req = request(
            "create_image",
            &["--session", "chatgpt_web:img|poll|cid|prompt"],
        )
        .unwrap();
        let args = remote_arguments(&req);
        assert_eq!(args["prompt"], "");
        assert_eq!(args["session"], "chatgpt_web:img|poll|cid|prompt");
    }

    #[test]
    fn hosted_arguments_cover_read_browser_and_usage() {
        let read = request(
            "read",
            &[
                "https://example.test",
                "--provider",
                "steel",
                "--mode",
                "pdf",
            ],
        )
        .unwrap();
        let args = remote_arguments(&read);
        assert_eq!(args["url"], "https://example.test");
        assert_eq!(args["provider"], "steel");
        assert_eq!(args["mode"], "pdf");

        let browser = request("browser", &["https://example.test"]).unwrap();
        let args = remote_arguments(&browser);
        assert_eq!(args["url"], "https://example.test");
        assert_eq!(args.len(), 1);

        let usage = request("usage", &["serper"]).unwrap();
        let args = remote_arguments(&usage);
        assert_eq!(args["provider"], "serper");
        assert_eq!(args.len(), 1);
    }

    #[tokio::test]
    async fn hosted_calls_reject_local_file_attachments() {
        let req = request("search", &["question", "--file", "./notes.txt"]).unwrap();
        let error = run_remote(&crate::config::Config::default(), &req)
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("File attachments require local stdio mode"));
    }

    #[test]
    fn hosted_artifact_is_saved_and_text_is_preserved() {
        let path = std::env::temp_dir().join(format!(
            "fetchira-cli-hosted-image-{}.png",
            std::process::id()
        ));
        let result = rmcp::model::CallToolResult::success(vec![
            rmcp::model::Content::image(
                base64::engine::general_purpose::STANDARD.encode(b"hosted image"),
                "image/png",
            ),
            rmcp::model::Content::text("⟦session: chatgpt_web:token — continue⟧"),
        ]);
        let text = remote_result_text(result, Some(&path)).unwrap();
        assert!(text.starts_with("saved: "));
        assert!(text.contains("⟦session: chatgpt_web:token"));
        assert_eq!(std::fs::read(&path).unwrap(), b"hosted image");
        let req = request(
            "create_image",
            &["prompt", "--path", path.to_str().unwrap()],
        )
        .unwrap();
        assert!(!remote_arguments(&req).contains_key("path"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn hosted_pdf_resource_is_saved_as_pdf() {
        let path = std::env::temp_dir().join(format!(
            "fetchira-cli-hosted-pdf-{}.pdf",
            std::process::id()
        ));
        let result = rmcp::model::CallToolResult::success(vec![rmcp::model::Content::resource(
            rmcp::model::ResourceContents::blob(
                base64::engine::general_purpose::STANDARD.encode(b"%PDF-hosted"),
                "fetchira:///artifact.pdf",
            )
            .with_mime_type("application/pdf"),
        )]);
        let text = remote_result_text(result, Some(&path)).unwrap();
        assert!(text.starts_with("saved: "));
        assert_eq!(std::fs::read(&path).unwrap(), b"%PDF-hosted");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn hosted_tool_error_becomes_cli_error() {
        let result =
            rmcp::model::CallToolResult::error(vec![rmcp::model::Content::text("provider failed")]);
        assert_eq!(
            remote_result_text(result, None).unwrap_err().to_string(),
            "provider failed"
        );
    }

    #[test]
    fn images_are_files_even_for_read_and_write_errors_propagate() {
        assert_eq!(image_extension(r"image/x\..\..\fetchira.toml"), "png");
        assert_eq!(image_extension("image/jpeg"), "jpg");
        assert_eq!(image_extension("application/pdf"), "pdf");
        let dest =
            std::env::temp_dir().join(format!("fetchira-cli-image-{}.png", std::process::id()));
        let img = || OutImage {
            mime: "image/png".into(),
            b64: base64::engine::general_purpose::STANDARD.encode(b"image bytes"),
        };
        let text = reply_text(
            Reply {
                text: String::new(),
                session: Some("provider:session".into()),
                image: Some(img()),
            },
            Capability::Read,
            Some(&dest),
        )
        .unwrap();
        assert!(text.starts_with(&format!("saved: {}", dest.display())));
        assert!(text.contains("⟦session: provider:session"));
        assert!(!text.contains(&img().b64));
        assert_eq!(std::fs::read(&dest).unwrap(), b"image bytes");
        assert!(save_image(&img(), Some(&dest.join("cannot-write.png"))).is_err());
        std::fs::remove_file(dest).unwrap();
    }
}
