use base64::Engine;
use serde_json::{json, Value};

use super::{grok_statsig, uuid4, with_sources, Capability, Input, LiveQuota, Outcome};
use crate::error::{Error, Result};

/// grok's degraded-mode `x-statsig-id`: base64 of a thrown `TypeError`, which xAI accepts on the
/// rate-limit poll when a real client's Statsig SDK fails to init. The chat-submit endpoint rejects
/// it (it needs a real signed token — see `grok_statsig`), but `/rest/rate-limits` still takes it,
/// so the quota poll skips the scrape. A *static* value gets fingerprinted, so randomize each call.
fn statsig_id() -> String {
    let props = [
        "childNodes",
        "children",
        "firstChild",
        "parentNode",
        "nextSibling",
        "classList",
    ];
    let kinds = ["null", "undefined"];
    let msg = format!(
        "x1:TypeError: Cannot read properties of {} (reading '{}')",
        kinds[pick(kinds.len())],
        props[pick(props.len())],
    );
    base64::engine::general_purpose::STANDARD.encode(msg)
}

fn pick(n: usize) -> usize {
    use std::sync::atomic::{AtomicU64, Ordering};
    static C: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    ((t ^ C.fetch_add(0x9E37_79B9, Ordering::Relaxed)) as usize) % n
}

pub async fn call(
    base: &str,
    client: &wreq::Client,
    cap: Capability,
    input: &Input,
) -> Result<Outcome> {
    if !matches!(cap, Capability::Search | Capability::DeepResearch) {
        return Err(Error::Unsupported("grok_web"));
    }
    let query = input.need_query()?;
    let mode = select(cap, input);

    // Resume an existing conversation, or start a new one.
    let url = match input.session.as_deref() {
        Some(conv) => format!("{base}/rest/app-chat/conversations/{conv}/responses"),
        None => format!("{base}/rest/app-chat/conversations/new"),
    };
    let path = url.strip_prefix(base).unwrap_or(&url);

    // grok references every attachment kind (images included) via `fileAttachments`.
    let mut attachments = Vec::new();
    if let Some(paths) = &input.attachments {
        for p in paths {
            attachments.push(upload(base, client, p).await?);
        }
    }

    let body = json!({
        "temporary": false,
        "message": query,
        "fileAttachments": attachments,
        "imageAttachments": [],
        "disableSearch": false,
        "enableImageGeneration": false,
        "returnImageBytes": false,
        "returnRawGrokInXaiRequest": false,
        "enableImageStreaming": false,
        "imageGenerationCount": 0,
        "forceConcise": false,
        "enableSideBySide": true,
        "sendFinalMetadata": true,
        "disableTextFollowUps": false,
        "responseMetadata": {},
        "disableMemory": true,
        "forceSideBySide": false,
        "isAsyncChat": false,
        "disableSelfHarmShortCircuit": false,
        "collectionIds": [],
        "disabledConnectorIds": [],
        "modeId": mode,
    })
    .to_string();

    let mut resp = send(base, client, &url, path, &body).await?;
    // A 403 is grok's app anti-bot — usually a rotated build/seed. Drop the cached statsig and retry.
    if resp.status().as_u16() == 403 {
        grok_statsig::invalidate().await;
        resp = send(base, client, &url, path, &body).await?;
    }
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    match status {
        401 => {
            return Err(Error::Provider {
                provider: "grok_web",
                status,
                body: "session expired; run `fetchira login grok_web`".into(),
            })
        }
        403 => {
            // Still rejected after a fresh statsig scrape — IP reputation/rate, or a build whose
            // generator we couldn't follow. Re-login won't help; the router fails over.
            return Err(Error::Provider {
                provider: "grok_web",
                status,
                body: "grok anti-bot rejected this request; failing over".into(),
            });
        }
        429 => return Err(Error::RateLimit("grok_web: rate limited".into())),
        _ => {}
    }
    parse(&text).map_err(|e| match e {
        // A non-auth status whose body isn't the expected stream: surface a snippet so the debug
        // log shows what grok actually sent (anti-bot HTML, a changed shape, an empty body…).
        Error::BadResponse(_) => Error::Provider {
            provider: "grok_web",
            status,
            body: format!("unexpected response shape: {}", snippet(&text)),
        },
        e => e,
    })
}

/// One POST attempt with a freshly minted `x-statsig-id` for the current build.
async fn send(
    base: &str,
    client: &wreq::Client,
    url: &str,
    path: &str,
    body: &str,
) -> Result<wreq::Response> {
    let statsig = grok_statsig::current(base, client).await?;
    Ok(client
        .post(url)
        .header("content-type", "application/json")
        .header(
            "baggage",
            "sentry-public_key=b311e0f2690c81f25e2c4cf6d4f7ce1c",
        )
        .header("origin", base)
        .header("x-statsig-id", statsig.token("POST", path))
        .header("x-xai-request-id", uuid4())
        .body(body.to_string())
        .send()
        .await?)
}

/// Upload a local file to grok and return its `fileMetadataId` for the chat body's `fileAttachments`.
/// Same JSON+base64 shape and statsig/403 handling as the chat submit.
async fn upload(base: &str, client: &wreq::Client, path: &str) -> Result<String> {
    let bytes = tokio::fs::read(path).await?;
    let name = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("upload");
    let body = json!({
        "fileName": name,
        "fileMimeType": mime_of(path),
        "content": base64::engine::general_purpose::STANDARD.encode(&bytes),
    })
    .to_string();
    let url = format!("{base}/rest/app-chat/upload-file");
    let path = "/rest/app-chat/upload-file";

    let mut resp = send(base, client, &url, path, &body).await?;
    if resp.status().as_u16() == 403 {
        grok_statsig::invalidate().await;
        resp = send(base, client, &url, path, &body).await?;
    }
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if status != 200 {
        return Err(Error::Provider {
            provider: "grok_web",
            status,
            body: format!("upload failed: {}", snippet(&text)),
        });
    }
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| {
            v.get("fileMetadataId")
                .and_then(|x| x.as_str())
                .map(str::to_owned)
        })
        .ok_or(Error::BadResponse("grok_web"))
}

/// grok requires a mime type on every upload; map by extension, default to octet-stream.
fn mime_of(path: &str) -> &'static str {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("pdf") => "application/pdf",
        Some("json") => "application/json",
        Some("csv") => "text/csv",
        Some("html" | "htm") => "text/html",
        // Source/config files: grok parses text/plain; octet-stream would be treated as opaque.
        Some(
            "md" | "markdown" | "txt" | "py" | "rs" | "js" | "ts" | "go" | "c" | "h" | "cpp" | "sh"
            | "toml" | "yaml" | "yml" | "xml",
        ) => "text/plain",
        _ => "application/octet-stream",
    }
}

/// Live remaining budget grok.com's own web UI polls, for a given model. grok keys the quota by
/// MODEL (grok-4 ~40/2h, grok-4-heavy ~20/2h, grok-3 ~140/2h), not by request kind, so DEFAULT is
/// enough. The window is rolling (`windowSizeSeconds`).
pub async fn rate_limit(base: &str, client: &wreq::Client, model: &str) -> Result<LiveQuota> {
    let body = json!({ "requestKind": "DEFAULT", "modelName": model });
    let resp = client
        .post(format!("{base}/rest/rate-limits"))
        .header("content-type", "application/json")
        .header(
            "baggage",
            "sentry-public_key=b311e0f2690c81f25e2c4cf6d4f7ce1c",
        )
        .header("x-statsig-id", statsig_id())
        .header("x-xai-request-id", uuid4())
        .body(body.to_string())
        .send()
        .await?;
    if resp.status().as_u16() != 200 {
        return Err(Error::BadResponse("grok_web"));
    }
    let v: Value = serde_json::from_str(&resp.text().await.unwrap_or_default())
        .map_err(|_| Error::BadResponse("grok_web"))?;
    let n = |k: &str| v.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
    Ok(LiveQuota {
        remaining: n("remainingQueries"),
        total: n("totalQueries"),
        window_secs: n("windowSizeSeconds"),
    })
}

/// grok's web-UI `modeId` for a call: search -> Fast, deep_research -> Expert. Heavy isn't accepted
/// for every account, so it's only sent when explicitly requested via `mode`. grok's anti-bot now
/// rejects the old `modelName`/`deepsearchPreset`/`isReasoning` body fields, so one `modeId` carries
/// the whole selection.
fn select(cap: Capability, input: &Input) -> &'static str {
    let mode = match cap {
        Capability::DeepResearch => "expert",
        _ => "fast",
    };
    match input.mode.as_deref() {
        Some(m) => match m.to_ascii_lowercase().as_str() {
            "auto" => "auto",
            "fast" => "fast",
            "expert" | "deepsearch" | "deep search" | "deep research" | "deep_research"
            | "research" => "expert",
            "heavy" => "heavy",
            _ => mode,
        },
        None => mode,
    }
}

/// Parse newline-delimited JSON. Prefer the terminal `modelResponse.message`; otherwise
/// concatenate streamed string `token`s. Collect `webSearchResults` as sources and the
/// `conversationId` as the resume token.
fn parse(ndjson: &str) -> Result<Outcome> {
    let mut tokens = String::new();
    let mut final_msg: Option<String> = None;
    let mut sources: Vec<String> = Vec::new();
    let mut conv: Option<String> = None;

    for line in ndjson.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("error").is_some() {
            return Err(Error::RateLimit("grok_web: stream error".into()));
        }
        if conv.is_none() {
            conv = find_str(&v, "conversationId");
        }
        let resp = match v.get("result").and_then(|r| r.get("response")) {
            Some(r) => r,
            None => continue,
        };
        if let Some(tok) = resp.get("token").and_then(|t| t.as_str()) {
            tokens.push_str(tok);
        }
        if let Some(m) = resp
            .get("modelResponse")
            .and_then(|m| m.get("message"))
            .and_then(|x| x.as_str())
        {
            final_msg = Some(m.to_string());
        }
        if let Some(results) = resp
            .get("webSearchResults")
            .and_then(|w| w.get("results"))
            .and_then(|r| r.as_array())
        {
            for r in results {
                if let Some(u) = r.get("url").and_then(|x| x.as_str()) {
                    if !sources.iter().any(|s| s == u) {
                        sources.push(u.to_string());
                    }
                }
            }
        }
    }

    let answer = final_msg.unwrap_or(tokens);
    if answer.trim().is_empty() {
        return Err(Error::BadResponse("grok_web"));
    }
    let mut out = Outcome::new(with_sources(strip_render(&answer), &sources), 1);
    out.session = conv;
    Ok(out)
}

/// Drop grok's inline `<grok:render …>…</grok:render>` citation-card markup.
fn strip_render(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("<grok:render") {
        out.push_str(&rest[..i]);
        match rest[i..].find("</grok:render>") {
            Some(j) => rest = &rest[i + j + "</grok:render>".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// First chunk of a body, single-lined and capped — enough to tell an HTML/anti-bot page from a
/// changed JSON shape when an unexpected response lands in the debug log.
fn snippet(s: &str) -> String {
    let t = s.trim();
    if t.is_empty() {
        return "<empty body>".to_string();
    }
    t.chars()
        .take(200)
        .collect::<String>()
        .replace(['\n', '\r'], " ")
}

/// Recursively find the first string value under `key` anywhere in the JSON.
fn find_str(v: &Value, key: &str) -> Option<String> {
    match v {
        Value::Object(o) => o
            .get(key)
            .and_then(|x| x.as_str())
            .map(str::to_string)
            .or_else(|| o.values().find_map(|x| find_str(x, key))),
        Value::Array(a) => a.iter().find_map(|x| find_str(x, key)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ndjson_stream() {
        let lines = [
            r#"{"result":{"response":{"token":"Hello ","conversationId":"conv-99"}}}"#,
            r#"{"result":{"response":{"token":"world","webSearchResults":{"results":[{"title":"t","url":"https://x.ai","preview":"p"}]}}}}"#,
            r#"{"result":{"response":{"modelResponse":{"message":"Hello world, final."}}}}"#,
        ]
        .join("\n");
        let out = parse(&lines).unwrap();
        assert!(out.text.starts_with("Hello world, final."));
        assert!(out.text.contains("x.ai"));
        assert_eq!(out.session.as_deref(), Some("conv-99"));
    }

    #[test]
    fn stream_error_is_rate_limit() {
        let line = r#"{"error":{"code":429,"message":"rate"}}"#;
        assert!(matches!(parse(line), Err(Error::RateLimit(_))));
    }

    #[test]
    fn mime_by_extension() {
        assert_eq!(mime_of("/tmp/a.PNG"), "image/png");
        assert_eq!(mime_of("photo.jpeg"), "image/jpeg");
        assert_eq!(mime_of("notes.md"), "text/plain");
        assert_eq!(mime_of("main.rs"), "text/plain");
        assert_eq!(mime_of("app.js"), "text/plain");
        assert_eq!(mime_of("blob"), "application/octet-stream");
    }

    #[test]
    fn strips_render_cards() {
        let s = "Rust 1.96<grok:render card_id=\"x\"><argument>0</argument></grok:render> is out.";
        assert_eq!(strip_render(s), "Rust 1.96 is out.");
    }

    #[test]
    fn snippet_caps_and_flags_empty() {
        assert_eq!(snippet("   \n  "), "<empty body>");
        assert_eq!(snippet("line1\nline2"), "line1 line2");
        assert_eq!(snippet(&"x".repeat(500)).len(), 200);
    }

    #[test]
    fn statsig_is_base64_x1() {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(statsig_id())
            .unwrap();
        assert!(String::from_utf8(raw).unwrap().starts_with("x1:TypeError"));
    }
}
