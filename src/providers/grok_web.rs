use base64::Engine;
use serde_json::{json, Value};

use super::{
    grok_statsig, uuid4, with_sources, Capability, FeatureLimit, Input, LiveLimits, LiveQuota,
    ModelInfo, OutImage, Outcome,
};
use crate::error::{Error, Result};

/// grok's degraded-mode `x-statsig-id`: base64 of a thrown `TypeError`. Chat submit, rate-limits,
/// and subscriptions reject it (they need a real signed token — see `grok_statsig`); `/rest/auth/get-user`
/// still takes it, so the identity poll skips the scrape. A *static* value gets fingerprinted, so
/// randomize each call.
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

    // Optional attachments: upload each, reference by id. grok caps at 4 per turn.
    let mut file_ids = Vec::new();
    for p in &input.file {
        file_ids.push(upload(base, client, &p.to_string_lossy()).await?);
    }

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
                body: format!(
                    "session expired; {}",
                    crate::usage::provider_login_hint("grok_web")
                ),
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
    let body = json!({ "requestKind": "DEFAULT", "modelName": model }).to_string();
    let (status, text) = grok_post(base, client, "/rest/rate-limits", body).await?;
    if status != 200 {
        tracing::warn!(provider = "grok_web", %status, body = %text.chars().take(240).collect::<String>(), "rate-limits request failed");
        return Err(Error::BadResponse("grok_web"));
    }
    let v: Value = serde_json::from_str(&text).map_err(|_| Error::BadResponse("grok_web"))?;
    parse_rate_limit(&v)
}

async fn grok_get(base: &str, client: &wreq::Client, path: &str) -> Result<(u16, String)> {
    let url = format!("{base}{path}");
    let token = grok_statsig::current(base, client)
        .await
        .ok()
        .map(|s| s.token("GET", path));
    let build = |tok: Option<&str>| {
        let mut req = client
            .get(&url)
            .header("origin", base)
            .header("x-xai-request-id", uuid4());
        if let Some(t) = tok {
            req = req.header("x-statsig-id", t);
        }
        req
    };
    let mut resp = build(token.as_deref()).send().await?;
    if resp.status().as_u16() == 403 {
        grok_statsig::invalidate().await;
        resp = build(None).send().await?;
    }
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    Ok((status, text))
}

async fn grok_post(
    base: &str,
    client: &wreq::Client,
    path: &str,
    body: String,
) -> Result<(u16, String)> {
    let url = format!("{base}{path}");
    let token = grok_statsig::current(base, client)
        .await
        .ok()
        .map(|s| s.token("POST", path));
    let build = |tok: Option<&str>| {
        let mut req = client
            .post(&url)
            .header("content-type", "application/json")
            .header("origin", base)
            .header("x-xai-request-id", uuid4())
            .body(body.clone());
        if let Some(t) = tok {
            req = req.header("x-statsig-id", t);
        }
        req
    };
    let mut resp = build(token.as_deref()).send().await?;
    if resp.status().as_u16() == 403 {
        grok_statsig::invalidate().await;
        resp = build(None).send().await?;
    }
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    Ok((status, text))
}

pub(crate) fn parse_rate_limit(v: &Value) -> Result<LiveQuota> {
    Ok(LiveQuota {
        remaining: v
            .get("remainingQueries")
            .and_then(Value::as_i64)
            .ok_or(Error::BadResponse("grok_web"))?,
        total: v
            .get("totalQueries")
            .and_then(Value::as_i64)
            .ok_or(Error::BadResponse("grok_web"))?,
        window_secs: v
            .get("windowSizeSeconds")
            .and_then(Value::as_i64)
            .unwrap_or(0),
    })
}

/// Live per-tier catalog + limits. grok's rate-limit endpoint returns a model's NOMINAL ceiling
/// regardless of entitlement (a lapsed sub still reports grok-4-heavy 20/2h), so a locked mode can't
/// be read from it alone — combine it with `/rest/subscriptions` (status/tier) to gate the paid
/// modes. Free/inactive → only Fast is real; Expert/Heavy/deep_research read 0/0. Fast/Auto share
/// grok-4-auto, Expert = grok-4 (reasoning), Heavy = grok-4-heavy (a Heavy-capable tier only).
pub(crate) async fn limits(base: &str, client: &wreq::Client) -> Result<LiveLimits> {
    let sub = subscription(base, client).await;
    let user_tier = if sub.is_none() {
        plan_from_user(base, client).await.and_then(|(_, t)| t)
    } else {
        None
    };
    let fast_q = rate_limit(base, client, "grok-4-auto").await.ok();
    let expert_q = rate_limit(base, client, "grok-4").await.ok();
    let heavy_q = rate_limit(base, client, "grok-4-heavy").await.ok();
    limits_from(sub, user_tier, fast_q, expert_q, heavy_q)
}

/// Build the dashboard catalog from live `/rest/subscriptions` + `/rest/rate-limits` only.
/// A failed poll is `None`/`Err`, not free/0/0 — those numbers only appear when the endpoint said so.
/// `user_tier` is a get-user label used only when subscriptions missed; it never locks modes.
pub(crate) fn limits_from(
    sub: Option<(bool, Option<String>)>,
    user_tier: Option<String>,
    fast_q: Option<LiveQuota>,
    expert_q: Option<LiveQuota>,
    heavy_q: Option<LiveQuota>,
) -> Result<LiveLimits> {
    let (known, active, tier_raw) = match sub {
        Some((active, tier_raw)) => (true, active, tier_raw),
        None => (false, false, user_tier),
    };
    if !known && tier_raw.is_none() && fast_q.is_none() && expert_q.is_none() && heavy_q.is_none() {
        return Err(Error::BadResponse("grok_web"));
    }
    let heavy_ok = known
        && active
        && tier_raw
            .as_deref()
            .is_some_and(|t| t.to_ascii_uppercase().contains("HEAVY"));
    let expert_ok = known && active;

    let mk = |id: &str, name: &str, q: Option<LiveQuota>, available: bool| -> Option<ModelInfo> {
        if available {
            let q = q?;
            Some(ModelInfo {
                id: id.to_string(),
                name: name.to_string(),
                levels: Vec::new(),
                remaining: Some(q.remaining),
                total: Some(q.total),
                window_secs: (q.window_secs > 0).then_some(q.window_secs),
                reset_after: None,
                locked: false,
            })
        } else if known {
            Some(ModelInfo {
                id: id.to_string(),
                name: name.to_string(),
                levels: Vec::new(),
                remaining: Some(0),
                total: Some(0),
                window_secs: None,
                reset_after: None,
                locked: true,
            })
        } else {
            None
        }
    };
    let mut models = Vec::new();
    models.extend(mk("fast", "Fast", fast_q, true));
    models.extend(mk("auto", "Auto", fast_q, true));
    models.extend(mk("expert", "Expert", expert_q, expert_ok));
    models.extend(mk("heavy", "Heavy", heavy_q, heavy_ok));

    let features = if expert_ok {
        expert_q
            .map(|q| FeatureLimit {
                feature: "deep_research".into(),
                remaining: q.remaining,
                total: Some(q.total),
                window_secs: (q.window_secs > 0).then_some(q.window_secs),
                reset_after: None,
            })
            .into_iter()
            .collect()
    } else if known {
        vec![FeatureLimit {
            feature: "deep_research".into(),
            remaining: 0,
            total: Some(0),
            window_secs: None,
            reset_after: None,
        }]
    } else {
        Vec::new()
    };

    Ok(LiveLimits {
        tier: if known {
            friendly_tier(tier_raw, active)
        } else {
            tier_raw.and_then(|t| friendly_tier(Some(t), true))
        },
        features,
        models,
        ..Default::default()
    })
}

/// Read the account's subscription state: `(is_active, raw_tier)`. Cookie session is enough on
/// grok.com (the browser sends no statsig). A real signed token is used when the scrape works;
/// the degraded TypeError token is rejected and would look like free. No ACTIVE row = free.
async fn subscription(base: &str, client: &wreq::Client) -> Option<(bool, Option<String>)> {
    let (status, body) = match grok_get(base, client, "/rest/subscriptions").await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(provider = "grok_web", error = %e, "subscriptions request failed");
            return None;
        }
    };
    if status != 200 {
        tracing::warn!(provider = "grok_web", %status, body = %body.chars().take(240).collect::<String>(), "subscriptions request failed");
        return None;
    }
    let v: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => {
            tracing::warn!(provider = "grok_web", %status, body = %body.chars().take(120).collect::<String>(), "subscriptions response was not JSON");
            return None;
        }
    };
    Some(parse_subscriptions(&v))
}

/// `/rest/auth/get-user` still works from datacenter IPs when `/rest/subscriptions` comes back empty.
/// Numeric `sessionTierId` only tells paid vs free — Pro vs Heavy comes from the store product id.
async fn plan_from_user(base: &str, client: &wreq::Client) -> Option<(bool, Option<String>)> {
    let resp = client
        .get(format!("{base}/rest/auth/get-user"))
        .header("origin", base)
        .header("x-statsig-id", statsig_id())
        .header("x-xai-request-id", uuid4())
        .send()
        .await
        .ok()?;
    if resp.status().as_u16() != 200 {
        return None;
    }
    let v: Value = serde_json::from_str(&resp.text().await.unwrap_or_default()).ok()?;
    let tier = plan_from_user_json(&v)?;
    Some((true, Some(tier)))
}

fn plan_from_user_json(v: &Value) -> Option<String> {
    if let Some(t) = v["xSubscriptionType"].as_str().filter(|s| !s.is_empty()) {
        return Some(if t.starts_with("SUBSCRIPTION_TIER_") {
            t.to_string()
        } else {
            format!("SUBSCRIPTION_TIER_{}", t.to_ascii_uppercase())
        });
    }
    // Numeric sessionTierId is paid-vs-free only — it cannot tell Pro from Heavy.
    let id = v["sessionTierId"].as_str()?.to_ascii_uppercase();
    if id.contains("HEAVY") {
        Some("SUBSCRIPTION_TIER_SUPER_GROK_HEAVY".into())
    } else if id.contains("PRO") {
        Some("SUBSCRIPTION_TIER_SUPER_GROK_PRO".into())
    } else if id.contains("LITE") {
        Some("SUBSCRIPTION_TIER_SUPER_GROK_LITE".into())
    } else {
        None
    }
}

pub(crate) fn parse_subscriptions(v: &Value) -> (bool, Option<String>) {
    let Some(arr) = v["subscriptions"].as_array() else {
        return (false, None);
    };
    let active = |s: &&Value| {
        matches!(
            s["status"]
                .as_str()
                .map(|x| x.to_ascii_uppercase())
                .as_deref(),
            Some("SUBSCRIPTION_STATUS_ACTIVE" | "ACTIVE")
        )
    };
    let free = |s: &&Value| {
        let Some(t) = s["tier"].as_str() else {
            return true;
        };
        let key = t
            .strip_prefix("SUBSCRIPTION_TIER_")
            .unwrap_or(t)
            .to_ascii_uppercase();
        matches!(key.as_str(), "GROK" | "FREE" | "NONE" | "")
    };
    let sub = arr
        .iter()
        .find(|s| active(s) && !free(s))
        .or_else(|| arr.iter().find(active))
        .or_else(|| arr.first());
    let Some(sub) = sub else {
        return (false, None);
    };
    (active(&sub), sub_tier(sub))
}

/// Play/App Store product id wins over `tier`: Google Play SuperGrok Heavy is `grok.ultra` while
/// `/rest/subscriptions` still reports `SUBSCRIPTION_TIER_SUPER_GROK_PRO`.
fn sub_tier(s: &Value) -> Option<String> {
    product_tier(s).or_else(|| s["tier"].as_str().map(str::to_string))
}

fn product_tier(s: &Value) -> Option<String> {
    let ids = [
        s.pointer("/google/productId").and_then(Value::as_str),
        s.pointer("/apple/productId").and_then(Value::as_str),
        s.pointer("/stripe/productId").and_then(Value::as_str),
        s.pointer("/web/productId").and_then(Value::as_str),
    ];
    for id in ids.into_iter().flatten() {
        let u = id.to_ascii_lowercase();
        if u.contains("ultra") || u.contains("heavy") {
            return Some("SUBSCRIPTION_TIER_SUPER_GROK_HEAVY".into());
        }
        if u.contains("lite") {
            return Some("SUBSCRIPTION_TIER_SUPER_GROK_LITE".into());
        }
        if u.contains("pro") || u.contains("plus") {
            return Some("SUBSCRIPTION_TIER_SUPER_GROK_PRO".into());
        }
    }
    None
}

/// The signed-in account's email, for the dashboard's masked display and duplicate-account
/// detection. `/rest/auth/get-user` takes the degraded statsig like the subscription poll. Prefer
/// `email`; fall back to `xUsername` (X-login accounts carry no email) then `googleEmail`.
pub(crate) async fn identity(base: &str, client: &wreq::Client) -> Result<Option<String>> {
    let resp = client
        .get(format!("{base}/rest/auth/get-user"))
        .header("x-statsig-id", statsig_id())
        .header("x-xai-request-id", uuid4())
        .send()
        .await?;
    if resp.status().as_u16() != 200 {
        return Err(Error::BadResponse("grok_web"));
    }
    let v: Value = serde_json::from_str(&resp.text().await.unwrap_or_default())
        .map_err(|_| Error::BadResponse("grok_web"))?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    Ok(s("email")
        .or_else(|| s("xUsername"))
        .or_else(|| s("googleEmail")))
}

/// `SUBSCRIPTION_TIER_SUPER_GROK_PRO` -> `"SuperGrok Pro"`, suffixed `(inactive)` when lapsed.
/// No record = free. Legacy `GROK_PRO` is the pre-rebrand SuperGrok plan.
fn friendly_tier(raw: Option<String>, active: bool) -> Option<String> {
    let Some(t) = raw else {
        return Some("free".into());
    };
    let key = t
        .strip_prefix("SUBSCRIPTION_TIER_")
        .unwrap_or(&t)
        .to_ascii_uppercase();
    let name = match key.as_str() {
        "GROK" | "FREE" | "NONE" | "" => "free".to_string(),
        "GROK_PRO" | "SUPER_GROK" => "SuperGrok".into(),
        "SUPER_GROK_LITE" => "SuperGrok Lite".into(),
        "SUPER_GROK_PRO" => "SuperGrok Pro".into(),
        "SUPER_GROK_HEAVY" => "SuperGrok Heavy".into(),
        other => other.replace('_', " ").to_ascii_lowercase(),
    };
    Some(if active || name == "free" {
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
            Some("SuperGrok (inactive)")
        );
        assert_eq!(
            friendly_tier(Some("SUBSCRIPTION_TIER_GROK_PRO".into()), true).as_deref(),
            Some("SuperGrok")
        );
        assert_eq!(
            friendly_tier(Some("SUBSCRIPTION_TIER_SUPER_GROK_PRO".into()), true).as_deref(),
            Some("SuperGrok Pro")
        );
        assert_eq!(
            friendly_tier(Some("SUBSCRIPTION_TIER_SUPER_GROK_HEAVY".into()), true).as_deref(),
            Some("SuperGrok Heavy")
        );
        assert_eq!(friendly_tier(None, false).as_deref(), Some("free"));
    }

    #[test]
    fn parse_subscriptions_prefers_active_paid() {
        let v = json!({"subscriptions":[
            {"status":"SUBSCRIPTION_STATUS_CANCELED","tier":"SUBSCRIPTION_TIER_GROK"},
            {"status":"SUBSCRIPTION_STATUS_ACTIVE","tier":"SUBSCRIPTION_TIER_SUPER_GROK_PRO"}
        ]});
        let (active, tier) = parse_subscriptions(&v);
        assert!(active);
        assert_eq!(tier.as_deref(), Some("SUBSCRIPTION_TIER_SUPER_GROK_PRO"));
    }

    #[test]
    fn play_ultra_product_is_heavy() {
        let v = json!({"subscriptions":[{
            "status":"SUBSCRIPTION_STATUS_ACTIVE",
            "tier":"SUBSCRIPTION_TIER_SUPER_GROK_PRO",
            "google":{"productId":"grok.ultra","basePlanId":"p1m"}
        }]});
        let (active, tier) = parse_subscriptions(&v);
        assert!(active);
        assert_eq!(tier.as_deref(), Some("SUBSCRIPTION_TIER_SUPER_GROK_HEAVY"));
        assert_eq!(
            friendly_tier(tier, true).as_deref(),
            Some("SuperGrok Heavy")
        );
    }

    #[test]
    fn plan_from_user_session_tier() {
        assert_eq!(plan_from_user_json(&json!({"sessionTierId":"1"})), None);
        assert_eq!(plan_from_user_json(&json!({"sessionTierId":"2"})), None);
        assert_eq!(
            plan_from_user_json(&json!({"xSubscriptionType":"SUPER_GROK_HEAVY"})).as_deref(),
            Some("SUBSCRIPTION_TIER_SUPER_GROK_HEAVY")
        );
    }

    #[test]
    fn user_tier_without_subscriptions_does_not_lock_modes() {
        let ll = limits_from(
            None,
            Some("SUBSCRIPTION_TIER_SUPER_GROK_HEAVY".into()),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(ll.tier.as_deref(), Some("SuperGrok Heavy"));
        assert!(ll.models.is_empty());
        assert!(ll.features.is_empty());
    }

    #[test]
    fn limits_from_miss_is_error_not_free() {
        assert!(limits_from(None, None, None, None, None).is_err());
    }

    #[test]
    fn limits_from_rate_limits_without_plan_has_no_fake_tier() {
        let q = LiveQuota {
            remaining: 12,
            total: 20,
            window_secs: 7200,
        };
        let ll = limits_from(None, None, Some(q), None, None).unwrap();
        assert_eq!(ll.tier, None);
        assert_eq!(ll.models.len(), 2);
        assert_eq!(ll.models[0].name, "Fast");
        assert_eq!(ll.models[0].remaining, Some(12));
        assert_eq!(ll.models[0].total, Some(20));
        assert_eq!(ll.models[0].window_secs, Some(7200));
        assert!(ll.features.is_empty());
        assert!(!ll.models.iter().any(|m| m.id == "heavy"));
    }

    #[test]
    fn limits_from_live_heavy_keeps_endpoint_counts() {
        let fast = LiveQuota {
            remaining: 140,
            total: 150,
            window_secs: 7200,
        };
        let expert = LiveQuota {
            remaining: 130,
            total: 140,
            window_secs: 7200,
        };
        let heavy = LiveQuota {
            remaining: 17,
            total: 20,
            window_secs: 7200,
        };
        let ll = limits_from(
            Some((true, Some("SUBSCRIPTION_TIER_SUPER_GROK_HEAVY".into()))),
            None,
            Some(fast),
            Some(expert),
            Some(heavy),
        )
        .unwrap();
        assert_eq!(ll.tier.as_deref(), Some("SuperGrok Heavy"));
        let h = ll.models.iter().find(|m| m.id == "heavy").unwrap();
        assert_eq!(h.remaining, Some(17));
        assert_eq!(h.total, Some(20));
        assert!(!h.locked);
        assert_eq!(ll.feature("deep_research").unwrap().remaining, 130);
    }

    #[test]
    fn limits_from_known_free_locks_paid_modes() {
        let ll = limits_from(Some((false, None)), None, None, None, None).unwrap();
        assert_eq!(ll.tier.as_deref(), Some("free"));
        assert!(ll
            .models
            .iter()
            .all(|m| m.locked || m.id == "fast" || m.id == "auto"));
        let heavy = ll.models.iter().find(|m| m.id == "heavy").unwrap();
        assert!(heavy.locked);
        assert_eq!(heavy.remaining, Some(0));
        assert_eq!(heavy.total, Some(0));
    }

    #[test]
    fn parse_rate_limit_requires_live_fields() {
        assert!(parse_rate_limit(&json!({})).is_err());
        let q = parse_rate_limit(&json!({
            "remainingQueries": 3,
            "totalQueries": 20,
            "windowSizeSeconds": 7200
        }))
        .unwrap();
        assert_eq!(q.remaining, 3);
        assert_eq!(q.total, 20);
        assert_eq!(q.window_secs, 7200);
    }

    #[test]
    fn statsig_is_base64_x1() {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(statsig_id())
            .unwrap();
        assert!(String::from_utf8(raw).unwrap().starts_with("x1:TypeError"));
    }

    fn redact_grok_json(raw: &str) -> String {
        let Ok(mut v) = serde_json::from_str::<Value>(raw) else {
            return raw.chars().take(240).collect();
        };
        if let Some(arr) = v.get_mut("subscriptions").and_then(|x| x.as_array_mut()) {
            for sub in arr {
                if let Some(t) = sub.pointer_mut("/google/purchaseToken") {
                    *t = json!("<redacted>");
                }
                if let Some(id) = sub.get_mut("xaiUserId") {
                    *id = json!("<redacted>");
                }
            }
        }
        v.to_string()
    }

    async fn poll_live(label: &str, client: &wreq::Client) {
        let base = "https://grok.com";
        let (st, body) = grok_get(base, client, "/rest/subscriptions")
            .await
            .expect("subscriptions");
        eprintln!(
            "[{label}] GET /rest/subscriptions status={st} body={}",
            redact_grok_json(&body)
        );
        for model in ["grok-4-auto", "grok-4", "grok-4-heavy"] {
            let payload = json!({ "requestKind": "DEFAULT", "modelName": model }).to_string();
            let (st, body) = grok_post(base, client, "/rest/rate-limits", payload)
                .await
                .expect("rate-limits");
            eprintln!(
                "[{label}] POST /rest/rate-limits {model} status={st} body={}",
                redact_grok_json(&body)
            );
        }
        match limits(base, client).await {
            Ok(ll) => eprintln!(
                "[{label}] limits tier={:?} models={}",
                ll.tier,
                ll.models
                    .iter()
                    .map(|m| format!("{}:{:?}/{:?}", m.name, m.remaining, m.total))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Err(e) => eprintln!("[{label}] limits err {e}"),
        }
    }

    #[tokio::test]
    #[ignore]
    async fn live_subscriptions_and_rate_limits() {
        let path = std::env::var("GROK_SESSION").expect("GROK_SESSION json path");
        let raw = std::fs::read_to_string(&path).expect("session");
        let sess = crate::web::parse_session(&raw);
        let client = crate::web::build_client(&sess.cookies, &sess.headers, None).expect("client");
        poll_live("grok-1", &client).await;
        if let Ok(profile) = std::env::var("GROK_CHROME_PROFILE") {
            let all = crate::web::chromium_profile_cookies(profile.as_ref())
                .await
                .expect("profile cookies");
            let keep: Vec<_> = all
                .into_iter()
                .filter(|c| {
                    let d = c.domain.trim_start_matches('.');
                    d == "grok.com"
                        || d.ends_with(".grok.com")
                        || d == "x.ai"
                        || d.ends_with(".x.ai")
                })
                .collect();
            eprintln!(
                "profile cookie hosts {}",
                keep.iter()
                    .filter(|c| c.name == "sso")
                    .map(|c| c.domain.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let client =
                crate::web::build_client(&keep, &Default::default(), None).expect("profile client");
            poll_live("xai-sso", &client).await;
        }
    }
}
