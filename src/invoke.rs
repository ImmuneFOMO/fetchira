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
            if query.trim().is_empty() && !allows_empty_query(&req, !words.is_empty()) {
                bail!("missing {}", if image { "prompt" } else { "query" });
            }
            req.input.query = (!words.is_empty()).then_some(query);
        }
    }
    // Match MCP search attachment routing; research and images keep their own priority order.
    if cap == Some(Capability::Search) && !req.input.file.is_empty() {
        req.provider = req.provider.or(Some(ProviderKind::GrokWeb));
    }
    if search {
        validate_search_input(&req.input)?;
    }
    Ok(req)
}

fn allows_empty_query(req: &Request, text_supplied: bool) -> bool {
    let Some(session) = req.input.session.as_deref() else {
        return false;
    };
    let opaque = crate::router::decode_session_affinity(session).map(|(_, opaque)| opaque);
    let token = opaque.as_deref().unwrap_or(session);
    match (req.provider, req.capability) {
        (Some(ProviderKind::ChatgptWeb), Some(Capability::DeepResearch)) => {
            token.starts_with("dr|poll|")
        }
        (Some(ProviderKind::ChatgptWeb), Some(Capability::Image)) => token.starts_with("img|poll|"),
        // Gemini treats an explicitly empty follow-up as "start" for its research plan.
        (Some(ProviderKind::GeminiWeb), Some(Capability::DeepResearch)) => {
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
        bail!("CLI tools need local accounts; this configuration uses a hosted endpoint. Use MCP for hosted Fetchira, or a local FETCHIRA_HOME.");
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
