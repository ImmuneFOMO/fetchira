use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use regex::Regex;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::chatgpt_sentinel::Ctx;
use super::{
    chatgpt_sentinel, uuid4, with_sources, Capability, FeatureLimit, Input, LiveLimits, ModelInfo,
    Outcome,
};
use crate::error::{Error, Result};

// chatgpt.com web session, reusing the same login/cookie/impersonation path as the other web
// providers. `search` = a chat turn with the selected model (web search on unless mode opts out);
// `deep_research` = the connector deep-research flow, kicked off then polled across calls (it runs
// minutes, past the client timeout). Submitting a turn is a 3-step choreography reversed from the web
// bundle: POST /f/conversation/prepare (a conversation-shaped body) -> a per-turn `conduit_token`,
// then the Sentinel proof (`chatgpt_sentinel`), then POST /f/conversation carrying both. All calls
// ride the full client header set (`Ctx`); a turn missing the conduit or headers 403s "unusual activity".

const DEFAULT_MODEL: &str = "gpt-5-5";
const DR_HINT: &str = "connector:connector_openai_deep_research";

// Inline poll budgets: block briefly on a fresh kickoff (then hand back a poll session), longer when
// the caller is already polling. A poll is a cheap GET, so neither risks the 300s per-request cap.
const KICKOFF_WAIT: u64 = 90;
const POLL_WAIT: u64 = 240;

pub async fn call(
    base: &str,
    client: &wreq::Client,
    cap: Capability,
    input: &Input,
    acct: &str,
) -> Result<Outcome> {
    match cap {
        Capability::Search => chat(base, client, input, acct).await,
        Capability::DeepResearch => deep_research(base, client, input, acct).await,
        _ => Err(Error::Unsupported("chatgpt_web")),
    }
}

async fn chat(base: &str, client: &wreq::Client, input: &Input, acct: &str) -> Result<Outcome> {
    let query = input.need_query()?;
    let model = model_slug(input.model.as_deref());
    let hints: &[&str] = if web_search_on(input) {
        &["search"]
    } else {
        &[]
    };
    let resume = parse_session(input.session.as_deref());
    let parent = resume.as_ref().map(|(_, p)| p.as_str());
    let convo = resume.as_ref().map(|(c, _)| c.as_str());

    let (mut ctx, updates) = build_ctx(base, client, acct).await?;
    ctx.conduit = conduit_prepare(base, client, &ctx, &model, hints, parent, convo).await?;
    let token = chatgpt_sentinel::token(base, client, &ctx).await?;

    let msg = message_node(query, hints, false);
    let body = conv_body(json!([msg]), &model, hints, parent, convo);
    let cid = run_turn(base, client, &ctx, &token, &body, true).await?;
    let conv = get_conversation(base, client, &ctx, &cid).await?;
    let mut out = extract_answer(&conv)?;
    out.session = Some(session_token(&conv, &cid));
    out.cookie_updates = updates;
    Ok(out)
}

async fn deep_research(
    base: &str,
    client: &wreq::Client,
    input: &Input,
    acct: &str,
) -> Result<Outcome> {
    // Poll an in-flight run (cheap, no PoW/conduit, charges nothing).
    if let Some(cid) = input
        .session
        .as_deref()
        .and_then(|s| s.strip_prefix("dr|poll|"))
    {
        let (ctx, updates) = build_ctx(base, client, acct).await?;
        let mut out = wait_for_dr(base, client, &ctx, cid, POLL_WAIT, 0).await?;
        out.cookie_updates = updates;
        return Ok(out);
    }

    let query = input.need_query()?;
    let model = model_slug(input.model.as_deref());
    let hints: &[&str] = &[DR_HINT];

    let (mut ctx, updates) = build_ctx(base, client, acct).await?;
    ctx.conduit = conduit_prepare(base, client, &ctx, &model, hints, None, None).await?;
    let token = chatgpt_sentinel::token(base, client, &ctx).await?;

    let msg = message_node(query, hints, true);
    let body = conv_body(json!([msg]), &model, hints, None, None);
    let cid = run_turn(base, client, &ctx, &token, &body, false).await?;

    if is_background(input) {
        let mut out = Outcome::new(
            "Deep research started. Call deep_research again with this session to fetch the report \
             when ready (~5-30 min)."
                .into(),
            1,
        );
        out.session = Some(format!("dr|poll|{cid}"));
        out.cookie_updates = updates;
        return Ok(out);
    }
    let mut out = wait_for_dr(base, client, &ctx, &cid, KICKOFF_WAIT, 1).await?;
    out.cookie_updates = updates;
    Ok(out)
}

/// Poll the conversation until the deep-research widget reports `completed`, the deadline passes, or
/// it fails. On timeout, hand back a `dr|poll|<cid>` session so the caller resumes the poll.
async fn wait_for_dr(
    base: &str,
    client: &wreq::Client,
    ctx: &Ctx,
    cid: &str,
    max_wait: u64,
    cost: i64,
) -> Result<Outcome> {
    let deadline = Instant::now() + Duration::from_secs(max_wait);
    loop {
        let conv = get_conversation(base, client, ctx, cid).await?;
        if let Some((report, sources)) = dr_report(&conv) {
            return Ok(Outcome::new(with_sources(report, &sources), cost));
        }
        if Instant::now() >= deadline {
            let mut out = Outcome::new(
                "Deep research still running. Call deep_research again with this session to fetch \
                 the report when ready."
                    .into(),
                cost,
            );
            out.session = Some(format!("dr|poll|{cid}"));
            return Ok(out);
        }
        tokio::time::sleep(Duration::from_secs(8)).await;
    }
}

/// Register the upcoming turn and get its `conduit_token` (sent as `x-conduit-token` on the sentinel +
/// conversation calls). The body is conversation-shaped with no messages. Best-effort: on failure we
/// fall back to `no-token` (the conversation will then 403, surfaced to the caller).
async fn conduit_prepare(
    base: &str,
    client: &wreq::Client,
    ctx: &Ctx,
    model: &str,
    hints: &[&str],
    parent: Option<&str>,
    convo: Option<&str>,
) -> Result<String> {
    let path = "/backend-api/f/conversation/prepare";
    let body = conv_body(json!([]), model, hints, parent, convo);
    let resp = ctx
        .apply(client.post(format!("{base}{path}")), path)
        .body(body.to_string())
        .send()
        .await?;
    let status = resp.status().as_u16();
    let retry_after = crate::error::parse_retry_after(
        resp.headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok()),
    );
    let text = resp.text().await.unwrap_or_default();
    if status >= 400 {
        if status == 429 {
            return Err(Error::rate_limit_after(
                "chatgpt_web: conduit prepare rate limited",
                retry_after,
            ));
        }
        if status == 401 || status == 403 {
            invalidate_ctx().await;
        }
        return Err(Error::Provider {
            provider: "chatgpt_web",
            status,
            body: "conduit prepare rejected (body schema drift?)".into(),
        });
    }
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok(v.get("conduit_token")
        .and_then(|x| x.as_str())
        .unwrap_or("no-token")
        .to_string())
}

/// POST a turn to `/backend-api/f/conversation` and return its `conversation_id`. A chat turn is
/// read to completion (it finishes in seconds; we then re-fetch the clean message); a deep-research
/// turn is read only until the id appears, since its stream stays open for the whole run.
async fn run_turn(
    base: &str,
    client: &wreq::Client,
    ctx: &Ctx,
    token: &str,
    body: &Value,
    drain: bool,
) -> Result<String> {
    let path = "/backend-api/f/conversation";
    let req = ctx
        .apply(client.post(format!("{base}{path}")), path)
        .header("openai-sentinel-chat-requirements-token", token)
        .header("accept", "text/event-stream")
        .header("origin", base)
        .header("referer", format!("{base}/"));
    let mut resp = req.body(body.to_string()).send().await?;
    match resp.status().as_u16() {
        401 | 403 => {
            let body = resp.text().await.unwrap_or_default();
            // OpenAI rate-limits the completion endpoint with a 403 "unusual activity"; treat it as a
            // (temporary) rate limit so the router fails over instead of nuking the session.
            if body.contains("Unusual activity") || body.contains("try again later") {
                return Err(Error::rate_limit(
                    "chatgpt_web: unusual activity, try again later",
                ));
            }
            chatgpt_sentinel::invalidate().await;
            invalidate_ctx().await;
            return Err(session_err());
        }
        429 => {
            return Err(Error::rate_limit_after(
                "chatgpt_web: rate limited",
                crate::error::parse_retry_after(
                    resp.headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok()),
                ),
            ))
        }
        s if s >= 400 => {
            return Err(Error::Provider {
                provider: "chatgpt_web",
                status: s,
                body: resp.text().await.unwrap_or_default(),
            })
        }
        _ => {}
    }

    if drain {
        let text = resp.text().await.unwrap_or_default();
        return conversation_id(&text).ok_or(Error::BadResponse("chatgpt_web"));
    }
    let mut buf = String::new();
    while let Some(chunk) = resp.chunk().await? {
        buf.push_str(&String::from_utf8_lossy(chunk.as_ref()));
        if let Some(cid) = conversation_id(&buf) {
            return Ok(cid);
        }
        if buf.len() > 131_072 {
            break;
        }
    }
    conversation_id(&buf).ok_or(Error::BadResponse("chatgpt_web"))
}

async fn get_conversation(
    base: &str,
    client: &wreq::Client,
    ctx: &Ctx,
    cid: &str,
) -> Result<Value> {
    let path = format!("/backend-api/conversation/{cid}");
    let resp = ctx
        .apply(
            client.get(format!(
                "{base}{path}?include_visually_hidden_messages=true"
            )),
            &path,
        )
        .send()
        .await?;
    if !resp.status().is_success() {
        if resp.status().as_u16() == 429 {
            return Err(Error::rate_limit_after(
                "chatgpt_web: conversation fetch rate limited",
                crate::error::parse_retry_after(
                    resp.headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok()),
                ),
            ));
        }
        return Err(Error::Provider {
            provider: "chatgpt_web",
            status: resp.status().as_u16(),
            body: "conversation fetch failed".into(),
        });
    }
    Ok(serde_json::from_str(&resp.text().await?)?)
}

// `/api/auth/session` rotates the chunked NextAuth session-token (`__Secure-next-auth.session-token.0/.1`)
// on every hit; the cookie jar can't keep both chunks in sync across rapid rotations, and after a few
// the reassembled token decodes to a stale (expired) accessToken — the session dies. The accessToken is
// good for ~34h, so we mint it once per account and reuse it until it nears expiry instead of re-fetching.
static CTX: LazyLock<Mutex<HashMap<String, (String, String)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const BEARER_MARGIN: i64 = 300;

pub(crate) async fn invalidate_ctx() {
    CTX.lock().await.clear();
}

/// Bearer + account id for an account, minted from `/api/auth/session` once and cached until near
/// expiry. Re-hitting that endpoint rotates the chunked NextAuth session cookie, and rapid
/// rotations desync the two chunks → a stale accessToken → the session dies; caching stops the
/// churn. On a cache miss we also hand back the response's `Set-Cookie` updates so the caller can
/// persist the freshly-rotated token (keeps the refresh chain alive across processes).
async fn ensure_bearer(
    base: &str,
    client: &wreq::Client,
    acct: &str,
) -> Result<(String, String, Vec<(String, String)>)> {
    let mut cache = CTX.lock().await;
    if let Some((bearer, account_id)) = cache.get(acct) {
        if jwt_exp_in(bearer) > BEARER_MARGIN {
            return Ok((bearer.clone(), account_id.clone(), Vec::new()));
        }
    }
    let resp = match client.get(format!("{base}/api/auth/session")).send().await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!(provider = "chatgpt_web", account = %acct, error = %e, "chatgpt auth session request failed");
            return Err(e.into());
        }
    };
    let updates = crate::web::set_cookie_updates(resp.headers());
    let text = resp.text().await.unwrap_or_default();
    let sess: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let Some(bearer) = sess.get("accessToken").and_then(|x| x.as_str()) else {
        tracing::warn!(provider = "chatgpt_web", account = %acct, body = %text.chars().take(240).collect::<String>(), "chatgpt auth session has no access token");
        return Err(Error::Provider {
            provider: "chatgpt_web",
            status: 401,
            body: format!(
                "no session token; {}",
                crate::usage::provider_login_hint("chatgpt_web")
            ),
        });
    };
    let bearer = bearer.to_string();
    // A dead refresh chain makes `/api/auth/session` keep handing back an already-expired
    // accessToken — caching it would loop forever, so bail loudly and let the caller re-login.
    if jwt_exp_in(&bearer) <= BEARER_MARGIN {
        return Err(session_err());
    }
    let account_id = account_from_jwt(&bearer).unwrap_or_default();
    cache.insert(acct.to_string(), (bearer.clone(), account_id.clone()));
    Ok((bearer, account_id, updates))
}

/// Client identity for a turn (cached bearer + account id + the stable device/session ids). Returns
/// the `Set-Cookie` updates captured on a bearer cache-miss, for the caller to persist.
async fn build_ctx(
    base: &str,
    client: &wreq::Client,
    acct: &str,
) -> Result<(Ctx, Vec<(String, String)>)> {
    let (bearer, account_id, updates) = ensure_bearer(base, client, acct).await?;
    let build = chatgpt_sentinel::build_id(base, client).await?;
    Ok((
        Ctx {
            bearer,
            account_id,
            device_id: device_id().to_string(),
            session_id: session_id().to_string(),
            build,
            conduit: "no-token".into(),
        },
        updates,
    ))
}

fn jwt_payload(bearer: &str) -> Option<Value> {
    let payload = bearer.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn account_from_jwt(bearer: &str) -> Option<String> {
    jwt_payload(bearer)?
        .get("https://api.openai.com/auth")
        .and_then(|a| a.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn plan_from_jwt(bearer: &str) -> Option<String> {
    jwt_payload(bearer)?
        .get("https://api.openai.com/auth")
        .and_then(|a| a.get("chatgpt_plan_type"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn jwt_exp_in(bearer: &str) -> i64 {
    let Some(payload) = bearer.split('.').nth(1) else {
        return -1;
    };
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload) else {
        return -1;
    };
    let exp = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| v.get("exp").and_then(Value::as_i64))
        .unwrap_or(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    exp - now
}

fn email_from_jwt(bearer: &str) -> Option<String> {
    jwt_payload(bearer)?
        .get("https://api.openai.com/profile")
        .and_then(|p| p.get("email"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn identity_from_session(v: &Value) -> Option<String> {
    v.pointer("/user/email")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            v.get("accessToken")
                .and_then(Value::as_str)
                .and_then(email_from_jwt)
        })
}

fn message_node(query: &str, hints: &[&str], dr: bool) -> Value {
    let mut md = json!({"system_hints": hints, "selected_sources": []});
    if dr {
        md["deep_research_version"] = json!("standard");
    }
    json!({
        "id": uuid4(),
        "author": {"role": "user"},
        "create_time": now_f64(),
        "content": {"content_type": "text", "parts": [query]},
        "metadata": md,
    })
}

/// A full conversation-shaped body (the wire shape the server validates). `messages` is `[]` for the
/// conduit prepare and `[msg]` for the actual turn.
fn conv_body(
    messages: Value,
    model: &str,
    hints: &[&str],
    parent: Option<&str>,
    convo: Option<&str>,
) -> Value {
    let mut body = json!({
        "action": "next",
        "messages": messages,
        "parent_message_id": parent.unwrap_or("client-created-root"),
        "model": model,
        "client_prepare_state": "none",
        "timezone_offset_min": 0,
        "timezone": "UTC",
        "conversation_mode": {"kind": "primary_assistant"},
        "enable_message_followups": true,
        "system_hints": hints,
        "supports_buffering": true,
        "supported_encodings": ["v1"],
        "client_contextual_info": {
            "is_dark_mode": true,
            "time_since_loaded": 1000,
            "page_height": 1080,
            "page_width": 1920,
            "pixel_ratio": 2,
            "screen_height": 1080,
            "screen_width": 1920,
            "app_name": "chatgpt.com",
            "has_web_push_capabilities": false,
            "web_push_notification_permission": "default",
        },
        "paragen_cot_summary_display_override": "allow",
        "force_parallel_switch": "auto",
    });
    if let Some(cid) = convo {
        body["conversation_id"] = json!(cid);
    }
    body
}

/// Latest visible assistant answer in the conversation (chat / web-search turns).
pub(crate) fn extract_answer(conv: &Value) -> Result<Outcome> {
    extract_answer_after(conv, f64::NEG_INFINITY)
}

/// Like `extract_answer` but only considers answers created strictly after `after` — used to wait
/// for the *new* reply when continuing an existing conversation (which already holds older answers).
pub(crate) fn extract_answer_after(conv: &Value, after: f64) -> Result<Outcome> {
    let mut best: Option<(f64, &Value)> = None;
    for m in nodes(conv) {
        if role(m) != "assistant" || hidden(m) {
            continue;
        }
        // Current ChatGPT responses may omit `recipient` (or use a tool-specific recipient)
        // even though they are visible assistant messages. Keep the legacy `all` path while
        // accepting any non-empty visible assistant response.
        let t = create_time(m);
        if t <= after || text_parts(m).trim().is_empty() {
            continue;
        }
        if best.is_none_or(|(bt, _)| t >= bt) {
            best = Some((t, m));
        }
    }
    let m = best.ok_or(Error::BadResponse("chatgpt_web"))?.1;
    let mut sources = Vec::new();
    collect_refs(m.get("metadata"), &mut sources);
    Ok(Outcome::new(with_sources(text_parts(m), &sources), 1))
}

/// Newest visible answer's create_time (−∞ if none) — the baseline for `extract_answer_after`.
pub(crate) fn last_assistant_time(conv: &Value) -> f64 {
    nodes(conv)
        .iter()
        .filter(|m| role(m) == "assistant" && !hidden(m) && !text_parts(m).trim().is_empty())
        .map(|m| create_time(m))
        .fold(f64::NEG_INFINITY, f64::max)
}

/// The finished deep-research report lives in `metadata.chatgpt_sdk.widget_state` (a JSON string),
/// not a plain message node; `status == "completed"` is the done signal.
pub(crate) fn dr_report(conv: &Value) -> Option<(String, Vec<String>)> {
    for m in nodes(conv) {
        let ws = match m
            .pointer("/metadata/chatgpt_sdk/widget_state")
            .and_then(|x| x.as_str())
        {
            Some(s) => s,
            None => continue,
        };
        let w: Value = match serde_json::from_str(ws) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if w.get("status").and_then(|x| x.as_str()) != Some("completed") {
            continue;
        }
        let rm = w.get("report_message")?;
        let report = text_parts(rm);
        if report.trim().is_empty() {
            continue;
        }
        let mut sources = Vec::new();
        collect_refs(rm.get("metadata"), &mut sources);
        return Some((report, sources));
    }
    None
}

fn nodes(conv: &Value) -> Vec<&Value> {
    conv.get("mapping")
        .and_then(|m| m.as_object())
        .map(|m| m.values().filter_map(|n| n.get("message")).collect())
        .unwrap_or_default()
}

fn role(m: &Value) -> &str {
    m.pointer("/author/role")
        .and_then(|x| x.as_str())
        .unwrap_or("")
}

fn hidden(m: &Value) -> bool {
    m.pointer("/metadata/is_visually_hidden_from_conversation")
        .and_then(|x| x.as_bool())
        .unwrap_or(false)
}

fn create_time(m: &Value) -> f64 {
    m.get("create_time").and_then(|x| x.as_f64()).unwrap_or(0.0)
}

/// Join the message's text parts, stripping the inline citation markers (PUA ``..``).
fn text_parts(m: &Value) -> String {
    let parts = match m.pointer("/content/parts").and_then(|p| p.as_array()) {
        Some(p) => p,
        None => return String::new(),
    };
    let raw: String = parts.iter().filter_map(|p| p.as_str()).collect();
    let mut out = String::with_capacity(raw.len());
    let mut in_cite = false;
    for c in raw.chars() {
        match c {
            '\u{e200}' => in_cite = true,
            '\u{e201}' => in_cite = false,
            _ if in_cite => {}
            _ if ('\u{e202}'..='\u{e2ff}').contains(&c) => {}
            _ => out.push(c),
        }
    }
    out
}

/// Collect source URLs from `content_references` + `safe_urls` (shapes vary; any http string counts).
fn collect_refs(meta: Option<&Value>, out: &mut Vec<String>) {
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => {
                if s.starts_with("http") && !out.iter().any(|u| u == s) {
                    out.push(s.clone());
                }
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            Value::Object(o) => o.values().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let meta = match meta {
        Some(m) => m,
        None => return,
    };
    if let Some(cr) = meta.get("content_references") {
        walk(cr, out);
    }
    if let Some(su) = meta.get("safe_urls") {
        walk(su, out);
    }
}

/// `<conversation_id>|<latest assistant message id>` — passed back as `session` to continue the thread.
pub(crate) fn session_token(conv: &Value, cid: &str) -> String {
    let mut best: Option<(f64, &str)> = None;
    for m in nodes(conv) {
        if role(m) != "assistant" {
            continue;
        }
        if let Some(id) = m.get("id").and_then(|x| x.as_str()) {
            let t = create_time(m);
            if best.is_none_or(|(bt, _)| t >= bt) {
                best = Some((t, id));
            }
        }
    }
    let pmid = best.map(|(_, id)| id).unwrap_or("client-created-root");
    format!("{cid}|{pmid}")
}

fn parse_session(s: Option<&str>) -> Option<(String, String)> {
    let s = s?;
    if s.starts_with("dr|") {
        return None;
    }
    let (cid, pmid) = s.split_once('|')?;
    Some((cid.to_string(), pmid.to_string()))
}

/// Live per-tier limits: the tool allowances from `conversation/init` plus the subscription plan.
/// All three calls are plain authenticated reads (not behind the generation anti-bot gate), so the
/// lightweight cookie client handles them — no browser needed.
pub(crate) async fn limits(base: &str, client: &wreq::Client, acct: &str) -> Result<LiveLimits> {
    // Shares the bearer cache with the turn path, so polling limits doesn't re-hit
    // `/api/auth/session` (which rotates the session cookie); the captured `Set-Cookie` updates are
    // handed back for the router to persist.
    let (bearer, account_id, updates) = ensure_bearer(base, client, acct).await?;
    let auth = format!("Bearer {bearer}");

    let init: Value = {
        let resp = client
            .post(format!("{base}/backend-api/conversation/init"))
            .header("authorization", &auth)
            .header("content-type", "application/json")
            .body("{}")
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            tracing::warn!(provider = "chatgpt_web", endpoint = "conversation/init", %status, body = %body.chars().take(240).collect::<String>(), "live limits request failed");
        }
        serde_json::from_str(&body).unwrap_or(Value::Null)
    };

    let tier = account_tier(base, client, &auth, &account_id)
        .await
        .and_then(|p| friendly_plan(Some(p)))
        .or_else(|| plan_from_jwt(&bearer).and_then(|p| friendly_plan(Some(p))));
    let models = model_catalog(base, client, &auth).await;
    let mut payload = json!({"init": init, "models": Value::Null, "tier": tier});
    payload["models"] = serde_json::to_value(&models).unwrap_or(Value::Null);
    let mut out = parse_limits_json(&payload)?;
    out.models = models;
    out.cookie_updates = updates;
    if out.identity.is_none() {
        out.identity = email_from_jwt(&bearer);
    }
    if out.tier.is_none() {
        out.tier = plan_from_jwt(&bearer).and_then(|p| friendly_plan(Some(p)));
    }
    Ok(out)
}

pub(crate) fn parse_limits_json(payload: &Value) -> Result<LiveLimits> {
    let parsed = |key: &str| match &payload[key] {
        Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
        v => v.clone(),
    };
    let init = parsed("init");
    let model_json = parsed("models");
    let tier_json = parsed("tier");
    let feat = |v: &Value, name_key: &str| {
        Some(FeatureLimit::simple(
            v[name_key].as_str().filter(|s| !s.is_empty())?,
            v["remaining"].as_i64()?,
            v["reset_after"].as_str().map(str::to_string),
        ))
    };
    let mut features: Vec<FeatureLimit> = Vec::new();
    if let Some(arr) = init["limits_progress"].as_array() {
        features.extend(arr.iter().filter_map(|v| feat(v, "feature_name")));
    }
    // Per-model message caps (usually empty on Plus until you near a cap — the "X messages left").
    if let Some(arr) = init["model_limits"].as_array() {
        features.extend(arr.iter().filter_map(|v| {
            let slug = v["model_slug"].as_str()?;
            Some(FeatureLimit::simple(
                format!("model:{slug}"),
                v["remaining"].as_i64()?,
                v["reset_after"].as_str().map(str::to_string),
            ))
        }));
    }

    let session = parsed("session");
    Ok(LiveLimits {
        tier: tier_json.as_str().map(str::to_string).or_else(|| {
            account_tier_value(&tier_json)
                .or_else(|| account_tier_value(&init))
                .or_else(|| {
                    session
                        .get("accessToken")
                        .and_then(Value::as_str)
                        .and_then(plan_from_jwt)
                })
                .and_then(|v| friendly_plan(Some(v)))
        }),
        features,
        models: model_catalog_value(&model_json),
        cookie_updates: Vec::new(),
        identity: identity_from_session(&session),
    })
}

fn model_catalog_value(v: &Value) -> Vec<ModelInfo> {
    let Some(cats) = v["categories"].as_array() else {
        return Vec::new();
    };
    let mut order = Vec::new();
    let mut levels = std::collections::HashMap::<String, Vec<&'static str>>::new();
    for c in cats {
        let Some((id, ls)) = c["category"].as_str().and_then(composer_model) else {
            continue;
        };
        let entry = levels.entry(id.clone()).or_insert_with(|| {
            order.push(id.clone());
            Vec::new()
        });
        for l in ls {
            if !entry.contains(&l) {
                entry.push(l);
            }
        }
    }
    order
        .into_iter()
        .map(|id| {
            let ls = levels.remove(&id).unwrap_or_default();
            ModelInfo {
                name: id.to_uppercase(),
                id,
                levels: ls.into_iter().map(String::from).collect(),
                remaining: None,
                total: None,
                window_secs: None,
                reset_after: None,
                locked: false,
            }
        })
        .collect()
}

fn account_tier_value(v: &Value) -> Option<String> {
    v["accounts"].as_object()?.values().find_map(|a| {
        a.pointer("/account/plan_type")
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

/// Best-effort account email (masked in the dashboard + duplicate-account detection): `user.email`
/// from the session, falling back to the access-token JWT's `.../profile` email claim.
pub(crate) async fn identity(base: &str, client: &wreq::Client) -> Result<Option<String>> {
    let sess = client
        .get(format!("{base}/api/auth/session"))
        .send()
        .await?
        .text()
        .await
        .unwrap_or_default();
    let sess: Value = serde_json::from_str(&sess)?;
    if let Some(email) = sess.pointer("/user/email").and_then(|x| x.as_str()) {
        return Ok(Some(email.to_string()));
    }
    Ok(sess
        .get("accessToken")
        .and_then(|x| x.as_str())
        .and_then(email_from_jwt))
}

/// The composer's selectable chat models, from `GET /backend-api/models`'s `categories` (the picker
/// structure), collapsed to the DOM vocab an agent passes to `search`: a base model
/// (gpt-5.5/gpt-5.4/gpt-5.3/o3) + its intelligence levels (instant/medium/high). We deliberately
/// use `categories`, NOT the flat `models[]` — that list is a different (HTTP-send) taxonomy with
/// non-composer lanes (deep-research/agent/mini) and `standard/extended` labels the browser
/// `select_model` doesn't accept. Best-effort; empty on any failure.
async fn model_catalog(base: &str, client: &wreq::Client, auth: &str) -> Vec<ModelInfo> {
    let (status, text) = async {
        let resp = client
            .get(format!(
                "{base}/backend-api/models?history_and_training_disabled=false"
            ))
            .header("authorization", auth)
            .send()
            .await
            .ok()?;
        let status = resp.status();
        let text = resp.text().await.ok()?;
        Some((status, text))
    }
    .await
    .unwrap_or_else(|| (wreq::StatusCode::INTERNAL_SERVER_ERROR, String::new()));
    if !status.is_success() {
        tracing::warn!(provider = "chatgpt_web", endpoint = "models", %status, body = %text.chars().take(240).collect::<String>(), "live model catalog request failed");
    }
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let Some(cats) = v["categories"].as_array() else {
        return Vec::new();
    };
    let mut order: Vec<String> = Vec::new();
    let mut levels_by_id: std::collections::HashMap<String, Vec<&'static str>> =
        std::collections::HashMap::new();
    for c in cats {
        let Some((id, levels)) = c["category"].as_str().and_then(composer_model) else {
            continue;
        };
        let acc = levels_by_id.entry(id.clone()).or_insert_with(|| {
            order.push(id.clone());
            Vec::new()
        });
        for l in levels {
            if !acc.contains(&l) {
                acc.push(l);
            }
        }
    }
    order
        .into_iter()
        .map(|id| {
            let mut levels: Vec<String> = levels_by_id
                .remove(&id)
                .unwrap_or_default()
                .into_iter()
                .map(String::from)
                .collect();
            levels.sort_by_key(|l| match l.as_str() {
                "instant" => 0,
                "medium" => 1,
                "high" => 2,
                _ => 3,
            });
            let name = if id.starts_with("gpt") {
                id.to_uppercase()
            } else {
                id.clone()
            };
            ModelInfo {
                id,
                name,
                levels,
                remaining: None,
                total: None,
                window_secs: None,
                reset_after: None,
                locked: false,
            }
        })
        .collect()
}

/// Map a `/backend-api/models` category to (composer model id, its intelligence levels). `_instant`
/// → the instant level; `_reasoning` → medium+high (the DOM splits reasoning into two). `gpt_5_auto`
/// and unknowns are skipped (Auto isn't a distinct DOM model).
fn composer_model(cat: &str) -> Option<(String, Vec<&'static str>)> {
    if cat == "o3" {
        return Some(("o3".to_string(), Vec::new()));
    }
    let (base, instant) = match (cat.strip_suffix("_instant"), cat.strip_suffix("_reasoning")) {
        (Some(b), _) => (b, true),
        (_, Some(b)) => (b, false),
        _ => return None,
    };
    let n = base.strip_prefix("gpt_5_")?;
    let levels = if instant {
        vec!["instant"]
    } else {
        vec!["medium", "high"]
    };
    Some((format!("gpt-5.{n}"), levels))
}

async fn account_tier(
    base: &str,
    client: &wreq::Client,
    auth: &str,
    account_id: &str,
) -> Option<String> {
    let mut req = client
        .get(format!("{base}/backend-api/accounts/check/v4-2023-04-27"))
        .header("authorization", auth);
    if !account_id.is_empty() {
        req = req.header("chatgpt-account-id", account_id);
    }
    let resp = req.send().await.ok()?;
    let status = resp.status();
    let body = resp.text().await.ok()?;
    if !status.is_success() {
        tracing::warn!(provider = "chatgpt_web", endpoint = "accounts/check", %status, body = %body.chars().take(240).collect::<String>(), "live account tier request failed");
    }
    let v: Value = serde_json::from_str(&body).ok()?;
    v["accounts"].as_object()?.values().find_map(|a| {
        a.pointer("/account/plan_type")
            .and_then(|x| x.as_str())
            .map(str::to_string)
    })
}

/// OpenAI's `plan_type` / `subscription_plan` (`plus`, `chatgptplusplan`, …) → a short badge.
/// Missing plan is `None` (don't invent free); an explicit `free` plan still badges as `free`.
fn friendly_plan(raw: Option<String>) -> Option<String> {
    let raw = raw?;
    let lower = raw.to_ascii_lowercase();
    let stripped = lower.strip_prefix("chatgpt").unwrap_or(&lower);
    let key = stripped.strip_suffix("plan").unwrap_or(stripped);
    Some(match key {
        "" | "free" => "free".to_string(),
        "plus" => "Plus".into(),
        "pro" => "Pro".into(),
        "team" => "Team".into(),
        "enterprise" => "Enterprise".into(),
        "max" => "Max".into(),
        "max5" => "Max 5×".into(),
        "max20" => "Max 20×".into(),
        _ => capitalize(key),
    })
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Web search is on by default for the search capability; `mode` of chat/off/none turns it off.
pub(crate) fn web_search_on(input: &Input) -> bool {
    !matches!(
        input
            .mode
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("chat" | "off" | "none" | "nosearch")
    )
}

fn is_background(input: &Input) -> bool {
    matches!(
        input
            .mode
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("background" | "async")
    )
}

/// Friendly model name -> chatgpt.com web `model` slug (raw passthrough otherwise; slugs drift).
fn model_slug(m: Option<&str>) -> String {
    let m = match m {
        Some(m) => m,
        None => return DEFAULT_MODEL.into(),
    };
    let k: String = m
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    match k.as_str() {
        "gpt55" | "gpt5" | "auto" | "5" => "gpt-5-5",
        "instant" | "fast" | "gpt55instant" => "gpt-5-5-instant",
        "thinking" | "reasoning" | "gpt55thinking" => "gpt-5-5-thinking",
        _ => return m.to_string(),
    }
    .to_string()
}

fn now_f64() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// A stable per-process device id (the web client sends a persistent `oai-device-id`).
fn device_id() -> &'static str {
    static ID: LazyLock<String> = LazyLock::new(uuid4);
    &ID
}

/// A stable per-process `oai-session-id` (one browser session = one id for its lifetime).
fn session_id() -> &'static str {
    static ID: LazyLock<String> = LazyLock::new(uuid4);
    &ID
}

fn conversation_id(s: &str) -> Option<String> {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#""conversation_id"\s*:\s*"([0-9a-fA-F-]{36})""#).unwrap());
    RE.captures(s).map(|c| c[1].to_string())
}

fn session_err() -> Error {
    Error::Provider {
        provider: "chatgpt_web",
        status: 403,
        body: format!(
            "session/cloudflare; {}",
            crate::usage::provider_login_hint("chatgpt_web")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_conversation_id() {
        let sse = r#"event: delta
data: {"v":{"conversation_id":"6a43081a-099c-83eb-b23b-092573129b5e","message":{}}}"#;
        assert_eq!(
            conversation_id(sse).as_deref(),
            Some("6a43081a-099c-83eb-b23b-092573129b5e")
        );
    }

    #[test]
    fn extracts_latest_visible_answer() {
        let conv = json!({"mapping": {
            "a": {"message": {"id": "a", "author": {"role": "user"}, "recipient": "all",
                "create_time": 1.0, "content": {"content_type": "text", "parts": ["hi"]}}},
            "b": {"message": {"id": "b", "author": {"role": "assistant"}, "recipient": "all",
                "create_time": 2.0, "content": {"content_type": "text", "parts": ["older"]}}},
            "c": {"message": {"id": "c", "author": {"role": "assistant"}, "recipient": "all",
                "create_time": 3.0, "content": {"content_type": "text", "parts": ["The \u{e200}cite\u{e201}answer."]},
                "metadata": {"safe_urls": ["https://example.com"]}}},
        }});
        let out = extract_answer(&conv).unwrap();
        assert!(out.text.contains("The answer."));
        assert!(!out.text.contains('\u{e200}'));
        assert!(out.text.contains("example.com"));
        assert_eq!(session_token(&conv, "X"), "X|c");
    }

    #[test]
    fn extracts_assistant_without_recipient() {
        let conv = json!({"mapping": {
            "x": {"message": {"author": {"role": "assistant"}, "create_time": 4.0,
                "content": {"content_type": "text", "parts": ["current"]}}}
        }});
        let out = extract_answer(&conv).unwrap();
        assert_eq!(out.text, "current");
    }

    #[test]
    fn dr_report_from_widget_state() {
        let ws = json!({
            "status": "completed",
            "report_message": {"content": {"content_type": "text", "parts": ["# Report\nbody"]},
                "metadata": {"content_references": [{"url": "https://src.dev"}]}}
        })
        .to_string();
        let conv = json!({"mapping": {
            "x": {"message": {"author": {"role": "assistant"},
                "metadata": {"chatgpt_sdk": {"widget_state": ws}}}}
        }});
        let (report, sources) = dr_report(&conv).unwrap();
        assert!(report.contains("# Report"));
        assert_eq!(sources, ["https://src.dev"]);
    }

    #[test]
    fn dr_report_none_while_running() {
        let ws = json!({"status": "running", "report_message": null}).to_string();
        let conv = json!({"mapping": {
            "x": {"message": {"metadata": {"chatgpt_sdk": {"widget_state": ws}}}}
        }});
        assert!(dr_report(&conv).is_none());
    }

    #[test]
    fn friendly_plan_badges() {
        assert_eq!(friendly_plan(None), None);
        assert_eq!(
            friendly_plan(Some("chatgptfreeplan".into())).as_deref(),
            Some("free")
        );
        assert_eq!(
            friendly_plan(Some("chatgptplusplan".into())).as_deref(),
            Some("Plus")
        );
        assert_eq!(
            friendly_plan(Some("chatgptproplan".into())).as_deref(),
            Some("Pro")
        );
        assert_eq!(
            friendly_plan(Some("chatgptmaxplan".into())).as_deref(),
            Some("Max")
        );
        assert_eq!(
            friendly_plan(Some("chatgptmax5plan".into())).as_deref(),
            Some("Max 5×")
        );
        assert_eq!(
            friendly_plan(Some("chatgptmax20plan".into())).as_deref(),
            Some("Max 20×")
        );
        assert_eq!(
            friendly_plan(Some("chatgptteamplan".into())).as_deref(),
            Some("Team")
        );
        // `account.plan_type` hands back bare lower-case tiers (the real current plan).
        assert_eq!(friendly_plan(Some("free".into())).as_deref(), Some("free"));
        assert_eq!(friendly_plan(Some("plus".into())).as_deref(), Some("Plus"));
        assert_eq!(friendly_plan(Some("pro".into())).as_deref(), Some("Pro"));
        assert_eq!(friendly_plan(Some("max".into())).as_deref(), Some("Max"));
        assert_eq!(friendly_plan(Some("team".into())).as_deref(), Some("Team"));
    }

    #[test]
    fn parse_limits_reads_plan_and_email_from_browser_payload() {
        let payload = json!({
            "init": json!({"limits_progress":[{"feature_name":"image_gen","remaining":2}]}).to_string(),
            "models": "{}",
            "tier": json!({"accounts":{"default":{"account":{"plan_type":"plus"}}}}).to_string(),
            "session": {"user":{"email":"a@t.org"}},
        });
        let ll = parse_limits_json(&payload).unwrap();
        assert_eq!(ll.tier.as_deref(), Some("Plus"));
        assert_eq!(ll.identity.as_deref(), Some("a@t.org"));
        assert_eq!(ll.features[0].feature, "image_gen");
    }

    #[test]
    fn parse_limits_reads_plan_from_session_jwt_when_check_fails() {
        let claims = json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acc",
                "chatgpt_plan_type": "plus"
            },
            "https://api.openai.com/profile": {"email": "a@t.org"}
        });
        let jwt = format!(
            "eyJhbGciOiJub25lIn0.{}.sig",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let payload = json!({
            "init": json!({"limits_progress":[{"feature_name":"image_gen","remaining":2}]}).to_string(),
            "models": "{}",
            "tier": "<html>denied</html>",
            "session": {"accessToken": jwt, "user":{"email":"a@t.org"}},
        });
        let ll = parse_limits_json(&payload).unwrap();
        assert_eq!(ll.tier.as_deref(), Some("Plus"));
        assert_eq!(ll.identity.as_deref(), Some("a@t.org"));
    }
}
