use base64::Engine;
use serde_json::{json, Value};

use super::{
    grok_statsig, uuid4, with_sources, Capability, FeatureLimit, Input, LiveLimits, LiveQuota,
    ModelInfo, OutImage, Outcome,
};
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
    if !matches!(
        cap,
        Capability::Search | Capability::DeepResearch | Capability::Image
    ) {
        return Err(Error::Unsupported("grok_web"));
    }
    let query = input.need_query()?;
    let image = matches!(cap, Capability::Image);
    let mode = select(cap, input);

    // Resume an existing conversation, or start a new one.
    let url = match input.session.as_deref() {
        Some(conv) => format!("{base}/rest/app-chat/conversations/{conv}/responses"),
        None => format!("{base}/rest/app-chat/conversations/new"),
    };
    let path = url.strip_prefix(base).unwrap_or(&url);

    // Optional attachment: upload first, reference by id. grok caps at 4 per turn.
    let file_ids: Vec<String> = match &input.file {
        Some(p) => {
            let (name, mime, bytes) = super::read_attachment(p)?;
            vec![upload(base, client, &name, &mime, &bytes).await?]
        }
        None => Vec::new(),
    };

    let body = json!({
        "temporary": false,
        "message": query,
        "fileAttachments": file_ids,
        "imageAttachments": [],
        "disableSearch": false,
        "enableImageGeneration": image,
        "returnImageBytes": false,
        "returnRawGrokInXaiRequest": false,
        "enableImageStreaming": image,
        "imageGenerationCount": if image { 2 } else { 0 },
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
    if image {
        return image_out(base, client, &text).await.map_err(|e| match e {
            Error::BadResponse(_) => Error::Provider {
                provider: "grok_web",
                status,
                body: format!("no generated image in response: {}", snippet(&text)),
            },
            e => e,
        });
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

/// Extract the generated image(s) from the ndjson stream and download the first as bytes. grok
/// returns RELATIVE asset paths (`modelResponse.generatedImageUrls`, one level shallower on a
/// continuation) that must be fetched from assets.grok.com with the same session — they're
/// auth-gated and short-lived, so we hand back the bytes, not the link.
async fn image_out(base: &str, client: &wreq::Client, ndjson: &str) -> Result<Outcome> {
    // Older builds put generatedImageUrls straight in the send stream; take it if present.
    let rel = match first_image_rel(ndjson) {
        Some(r) => r,
        // Current build: the image renders async. The send returns only the conversation id; get the
        // response id(s) from /response-node, then poll /load-responses until generatedImageUrls lands.
        None => {
            let conv = find_str(&json_lines(ndjson), "conversationId")
                .ok_or(Error::BadResponse("grok_web"))?;
            let mut got = None;
            // The send returns only the conversation frame; the render is async. Fetch the response
            // ids, then poll load-responses until the image file attachment lands (~a few seconds).
            for _ in 0..12 {
                if let Ok(rids) = response_ids(base, client, &conv).await {
                    if let Some(r) = load_response_image(base, client, &conv, &rids).await? {
                        got = Some(r);
                        break;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            }
            got.ok_or(Error::BadResponse("grok_web"))?
        }
    };
    let url = if rel.starts_with("http") {
        rel
    } else {
        format!("https://assets.grok.com/{}", rel.trim_start_matches('/'))
    };
    let resp = client.get(&url).header("accept", "image/*").send().await?;
    let mime = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_string();
    let bytes = resp
        .bytes()
        .await
        .map_err(|_| Error::BadResponse("grok_web"))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let mut out = Outcome::new(String::new(), 1);
    out.image = Some(OutImage { mime, b64 });
    Ok(out)
}

/// The response ids for a conversation, from `/response-node` (`responseNodes[].responseId`) — the
/// send doesn't hand these back for an async image turn, so we fetch them separately.
async fn response_ids(base: &str, client: &wreq::Client, conv: &str) -> Result<Vec<String>> {
    let path = format!("/rest/app-chat/conversations/{conv}/response-node");
    let statsig = grok_statsig::current(base, client).await?;
    let resp = client
        .get(format!("{base}{path}"))
        .header("origin", base)
        .header("x-statsig-id", statsig.token("GET", &path))
        .header("x-xai-request-id", uuid4())
        .send()
        .await?;
    let v: Value = serde_json::from_str(&resp.text().await.unwrap_or_default())
        .map_err(|_| Error::BadResponse("grok_web"))?;
    let ids: Vec<String> = v
        .get("responseNodes")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|n| {
                    n.get("responseId")
                        .and_then(|x| x.as_str())
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default();
    if ids.is_empty() {
        return Err(Error::BadResponse("grok_web"));
    }
    Ok(ids)
}

/// Poll a conversation's finished responses for the generated image path. `responses[k].
/// generatedImageUrls` is empty until the async image render completes.
async fn load_response_image(
    base: &str,
    client: &wreq::Client,
    conv: &str,
    rids: &[String],
) -> Result<Option<String>> {
    let path = format!("/rest/app-chat/conversations/{conv}/load-responses");
    let statsig = grok_statsig::current(base, client).await?;
    let resp = client
        .post(format!("{base}{path}"))
        .header("content-type", "application/json")
        .header("origin", base)
        .header("x-statsig-id", statsig.token("POST", &path))
        .header("x-xai-request-id", uuid4())
        .body(json!({ "responseIds": rids }).to_string())
        .send()
        .await?;
    if resp.status().as_u16() != 200 {
        return Ok(None);
    }
    let v: Value = serde_json::from_str(&resp.text().await.unwrap_or_default())
        .map_err(|_| Error::BadResponse("grok_web"))?;
    Ok(generated_image_uri(&v))
}

/// The finished render arrives as an image file attachment — `fileAttachmentsMetadata[k].fileUri`
/// (a `users/.../generated/.../image.jpg` asset path) — while `generatedImageUrls` stays empty on
/// the current build. Empty until the async render lands, so the caller keeps polling.
fn generated_image_uri(v: &Value) -> Option<String> {
    v.get("responses")?
        .as_array()?
        .iter()
        .flat_map(|r| {
            r.get("fileAttachmentsMetadata")
                .and_then(|m| m.as_array())
                .into_iter()
                .flatten()
        })
        .find(|m| {
            m.get("fileMimeType")
                .and_then(|x| x.as_str())
                .is_some_and(|s| s.starts_with("image/"))
        })
        .and_then(|m| m.get("fileUri").and_then(|x| x.as_str()).map(String::from))
}

/// Concatenate the ndjson lines into one JSON array value so `find_str` can scan the whole stream.
fn json_lines(ndjson: &str) -> Value {
    Value::Array(
        ndjson
            .lines()
            .filter_map(|l| serde_json::from_str(l.trim()).ok())
            .collect(),
    )
}

/// First generated-image relative path from the ndjson stream. `generatedImageUrls` sits under
/// `result.response.modelResponse` (new conv) or `result.modelResponse` (continuation) — a recursive
/// key search handles both nestings.
fn first_image_rel(ndjson: &str) -> Option<String> {
    for line in ndjson.lines() {
        let v: Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(u) = find_val(&v, "generatedImageUrls")
            .and_then(|x| x.as_array())
            .and_then(|a| a.iter().find_map(|x| x.as_str()))
        {
            return Some(u.to_string());
        }
    }
    None
}

/// Recursively find the first value under `key` anywhere in the JSON.
fn find_val<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(o) => o
            .get(key)
            .or_else(|| o.values().find_map(|x| find_val(x, key))),
        Value::Array(a) => a.iter().find_map(|x| find_val(x, key)),
        _ => None,
    }
}

/// Upload one attachment (base64-in-JSON, not multipart) and return its `fileMetadataId` — the bare
/// string that goes into the send body's `fileAttachments`. Under `/rest/app-chat/`, so statsig-gated.
async fn upload(
    base: &str,
    client: &wreq::Client,
    name: &str,
    mime: &str,
    bytes: &[u8],
) -> Result<String> {
    let body = json!({
        "fileName": name,
        "fileMimeType": mime,
        "content": base64::engine::general_purpose::STANDARD.encode(bytes),
    })
    .to_string();
    let statsig = grok_statsig::current(base, client).await?;
    let resp = client
        .post(format!("{base}/rest/app-chat/upload-file"))
        .header("content-type", "application/json")
        .header(
            "baggage",
            "sentry-public_key=b311e0f2690c81f25e2c4cf6d4f7ce1c",
        )
        .header("origin", base)
        .header(
            "x-statsig-id",
            statsig.token("POST", "/rest/app-chat/upload-file"),
        )
        .header("x-xai-request-id", uuid4())
        .body(body)
        .send()
        .await?;
    if resp.status().as_u16() != 200 {
        return Err(Error::Provider {
            provider: "grok_web",
            status: resp.status().as_u16(),
            body: "file upload rejected".into(),
        });
    }
    serde_json::from_str::<Value>(&resp.text().await.unwrap_or_default())
        .ok()
        .and_then(|v| {
            v.get("fileMetadataId")
                .and_then(|x| x.as_str())
                .map(String::from)
        })
        .ok_or(Error::BadResponse("grok_web"))
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

/// Live per-tier catalog + limits. grok's rate-limit endpoint returns a model's NOMINAL ceiling
/// regardless of entitlement (a lapsed sub still reports grok-4-heavy 20/2h), so a locked mode can't
/// be read from it alone — combine it with `/rest/subscriptions` (status/tier) to gate the paid
/// modes. Free/inactive → only Fast is real; Expert/Heavy/deep_research read 0/0. Fast/Auto share
/// grok-4-auto, Expert = grok-4 (reasoning), Heavy = grok-4-heavy (a Heavy-capable tier only).
pub(crate) async fn limits(base: &str, client: &wreq::Client) -> Result<LiveLimits> {
    let (active, tier_raw) = subscription(base, client).await;
    let heavy_ok = active
        && tier_raw
            .as_deref()
            .is_some_and(|t| t.to_ascii_uppercase().contains("HEAVY"));

    let fast_q = rate_limit(base, client, "grok-4-auto").await.ok();
    let expert_q = if active {
        rate_limit(base, client, "grok-4").await.ok()
    } else {
        None
    };
    let heavy_q = if heavy_ok {
        rate_limit(base, client, "grok-4-heavy").await.ok()
    } else {
        None
    };

    let mk = |id: &str, name: &str, q: Option<LiveQuota>, available: bool| ModelInfo {
        id: id.to_string(),
        name: name.to_string(),
        levels: Vec::new(),
        remaining: if available {
            q.map(|x| x.remaining)
        } else {
            Some(0)
        },
        total: if available {
            q.map(|x| x.total)
        } else {
            Some(0)
        },
        window_secs: if available {
            q.map(|x| x.window_secs)
        } else {
            None
        },
        reset_after: None,
        locked: !available,
    };
    let models = vec![
        mk("fast", "Fast", fast_q, true),
        mk("auto", "Auto", fast_q, true),
        mk("expert", "Expert", expert_q, active),
        mk("heavy", "Heavy", heavy_q, heavy_ok),
    ];

    // Deep search runs as Expert (grok-4 reasoning); locked to 0/0 when the sub is inactive.
    let dr = if active {
        FeatureLimit {
            feature: "deep_research".into(),
            remaining: expert_q.map(|x| x.remaining).unwrap_or(0),
            total: expert_q.map(|x| x.total),
            window_secs: expert_q.map(|x| x.window_secs),
            reset_after: None,
        }
    } else {
        FeatureLimit {
            feature: "deep_research".into(),
            remaining: 0,
            total: Some(0),
            window_secs: None,
            reset_after: None,
        }
    };

    Ok(LiveLimits {
        tier: friendly_tier(tier_raw, active),
        features: vec![dr],
        models,
    })
}

/// Read the account's subscription state: `(is_active, raw_tier)`. `/rest/subscriptions` takes the
/// degraded statsig like the rate-limit poll. No record (or any non-`ACTIVE` status) = free tier.
async fn subscription(base: &str, client: &wreq::Client) -> (bool, Option<String>) {
    let resp = client
        .get(format!("{base}/rest/subscriptions"))
        .header("x-statsig-id", statsig_id())
        .header("x-xai-request-id", uuid4())
        .send()
        .await;
    let Ok(resp) = resp else { return (false, None) };
    if resp.status().as_u16() != 200 {
        return (false, None);
    }
    let v: Value = match serde_json::from_str(&resp.text().await.unwrap_or_default()) {
        Ok(v) => v,
        Err(_) => return (false, None),
    };
    let Some(sub) = v["subscriptions"].as_array().and_then(|a| a.first()) else {
        return (false, None);
    };
    let active = sub["status"].as_str() == Some("SUBSCRIPTION_STATUS_ACTIVE");
    (active, sub["tier"].as_str().map(str::to_string))
}

/// `SUBSCRIPTION_TIER_GROK_PRO` -> `"grok pro"`, suffixed `(inactive)` when lapsed. No record = free.
fn friendly_tier(raw: Option<String>, active: bool) -> Option<String> {
    let name = match raw {
        None => return Some("free".into()),
        Some(t) => t
            .strip_prefix("SUBSCRIPTION_TIER_")
            .unwrap_or(&t)
            .replace('_', " ")
            .to_ascii_lowercase(),
    };
    Some(if active {
        name
    } else {
        format!("{name} (inactive)")
    })
}

/// grok's web-UI `modeId` for a call: search -> Fast, deep_research -> Expert. Heavy isn't accepted
/// for every account, so it's only sent when explicitly requested via `mode`. grok's anti-bot now
/// rejects the old `modelName`/`deepsearchPreset`/`isReasoning` body fields, so one `modeId` carries
/// the whole selection. Absent an explicit `mode`, deep_research honours `depth`: `deep` starts on
/// Heavy (falls back to Expert on quota lock), everything else on Expert.
fn select(cap: Capability, input: &Input) -> &'static str {
    let mode = match cap {
        Capability::DeepResearch => match input.depth.as_deref() {
            Some("deep") => "heavy",
            _ => "expert",
        },
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
    fn first_image_rel_handles_both_nestings() {
        // new conversation wraps in result.response.modelResponse
        let new_conv = r#"{"result":{"response":{"modelResponse":{"generatedImageUrls":["users/x/gen/a.jpg","b.jpg"]}}}}"#;
        assert_eq!(
            first_image_rel(new_conv).as_deref(),
            Some("users/x/gen/a.jpg")
        );
        // continuation is one level shallower (result.modelResponse)
        let cont = r#"{"result":{"modelResponse":{"generatedImageUrls":["c.jpg"]}}}"#;
        assert_eq!(first_image_rel(cont).as_deref(), Some("c.jpg"));
        // a plain text turn has no image
        assert_eq!(
            first_image_rel(r#"{"result":{"response":{"token":"hi"}}}"#),
            None
        );
    }

    #[test]
    fn generated_image_uri_reads_file_attachment() {
        // Current build: generatedImageUrls empty, the render is a fileAttachmentsMetadata image.
        let v: Value = serde_json::from_str(
            r#"{"responses":[
                {"generatedImageUrls":[],"fileAttachmentsMetadata":[]},
                {"generatedImageUrls":[],"fileAttachmentsMetadata":[
                    {"fileMimeType":"image/jpeg","fileUri":"users/a/generated/e/image.jpg"}
                ]}
            ]}"#,
        )
        .unwrap();
        assert_eq!(
            generated_image_uri(&v).as_deref(),
            Some("users/a/generated/e/image.jpg")
        );
        // not rendered yet -> keep polling
        let empty: Value =
            serde_json::from_str(r#"{"responses":[{"fileAttachmentsMetadata":[]}]}"#).unwrap();
        assert_eq!(generated_image_uri(&empty), None);
    }

    #[test]
    fn select_maps_depth_and_mode() {
        let inp = |depth: Option<&str>, mode: Option<&str>| Input {
            depth: depth.map(str::to_string),
            mode: mode.map(str::to_string),
            ..Default::default()
        };
        // search is always Fast, regardless of depth
        assert_eq!(select(Capability::Search, &inp(Some("deep"), None)), "fast");
        // deep_research default -> Expert; depth=deep starts on Heavy
        assert_eq!(select(Capability::DeepResearch, &inp(None, None)), "expert");
        assert_eq!(
            select(Capability::DeepResearch, &inp(Some("standard"), None)),
            "expert"
        );
        assert_eq!(
            select(Capability::DeepResearch, &inp(Some("deep"), None)),
            "heavy"
        );
        // an explicit mode overrides depth
        assert_eq!(
            select(Capability::DeepResearch, &inp(Some("deep"), Some("expert"))),
            "expert"
        );
    }

    #[test]
    fn friendly_tier_marks_inactive() {
        assert_eq!(
            friendly_tier(Some("SUBSCRIPTION_TIER_GROK_PRO".into()), false).as_deref(),
            Some("grok pro (inactive)")
        );
        assert_eq!(
            friendly_tier(Some("SUBSCRIPTION_TIER_GROK_PRO".into()), true).as_deref(),
            Some("grok pro")
        );
        assert_eq!(friendly_tier(None, false).as_deref(), Some("free"));
    }

    #[test]
    fn statsig_is_base64_x1() {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(statsig_id())
            .unwrap();
        assert!(String::from_utf8(raw).unwrap().starts_with("x1:TypeError"));
    }
}
