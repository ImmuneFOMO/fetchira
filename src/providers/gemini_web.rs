use base64::Engine;
use serde_json::{json, Value};

use super::{uuid4, Capability, Input, LiveLimits, ModelInfo, OutImage, Outcome};
use crate::error::{Error, Result};

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
        return Err(Error::Unsupported("gemini_web"));
    }
    let q = input.need_query()?;

    // Deep research is a 3-step flow over this one endpoint, threaded via the session token:
    //   1. deep_research(query)        -> DR-flagged turn returns a PLAN + a `dr|<ids>` session
    //   2. deep_research(session, ...) -> "start" confirms+runs the research; other text edits the plan
    let raw_session = input.session.as_deref();
    let is_dr_run = raw_session.is_some_and(|s| s.starts_with("dr|"));
    let resume_meta = raw_session.map(|s| s.strip_prefix("dr|").unwrap_or(s));
    let want_dr_plan = matches!(cap, Capability::DeepResearch) && raw_session.is_none();
    let query = if is_dr_run
        && matches!(
            q.trim().to_ascii_lowercase().as_str(),
            "start" | "go" | "run" | ""
        ) {
        "Start research"
    } else {
        q
    };

    // Preflight www.google.com to seed the NID/consent cookies Google expects alongside the
    // session; without it /app can render logged-out.
    let _ = client.get("https://www.google.com").send().await;

    // Scrape the session token from the /app bootstrap. The full captured cookie set must be sent —
    // trimming it to __Secure-1PSID(TS) makes /app 302 to /sorry (Google's anti-abuse interstitial),
    // which reads as logged-out. If it does render logged-out (a stale/absent rotating __Secure-1PSIDTS),
    // mint a fresh one via RotateCookies and retry ONCE. Doing the rotate unconditionally clobbers a
    // just-captured valid 1PSIDTS and logs the session out.
    let mut page = client
        .get(format!("{base}/app"))
        .send()
        .await?
        .text()
        .await
        .unwrap_or_default();
    if !logged_in(&page) {
        let _ = client
            .post("https://accounts.google.com/RotateCookies")
            .header("content-type", "application/json")
            .header("origin", "https://accounts.google.com")
            .body("[000,\"-0000000000000000000\"]")
            .send()
            .await;
        page = client
            .get(format!("{base}/app"))
            .send()
            .await?
            .text()
            .await
            .unwrap_or_default();
    }
    if !logged_in(&page) {
        return Err(Error::Provider {
            provider: "gemini_web",
            status: 0,
            body: "no session token; run `fetchira login gemini_web`".into(),
        });
    }
    let at = scrape(&page, "SNlM0e").unwrap_or_default();
    let bl = scrape(&page, "cfb2h").unwrap_or_default();
    let fsid = scrape(&page, "FdrFJe").unwrap_or_default();
    let hl = scrape(&page, "TuX5cc").unwrap_or_else(|| "en".into());

    // File turns need the `at` (SNlM0e) token: without it push.clients6 issues an unsigned upload id
    // and the file turn fails with Bard 1100. Chat/deep-research work fine without it, but for an
    // attachment a token-less session is unusable — fail with a retryable error so the router fails
    // over to a sibling gemini session that still carries the token.
    if !input.file.is_empty() && at.is_empty() {
        return Err(Error::Provider {
            provider: "gemini_web",
            status: 0,
            body: "gemini session can't attach files; run `fetchira login gemini_web`".into(),
        });
    }

    // Gemini has no image flag — it decides from prompt intent, so make the ask explicit.
    let img_prompt;
    let query: &str = if matches!(cap, Capability::Image) {
        img_prompt = format!("Generate an image of {query}");
        &img_prompt
    } else {
        query
    };

    let uuid = uuid4().to_uppercase();
    let mut inner_val = build_inner(query, &hl, &uuid, resume_meta);
    if want_dr_plan {
        if let Value::Array(a) = &mut inner_val {
            let blob: String = std::iter::repeat_with(|| uuid4().replace('-', ""))
                .take(82)
                .collect();
            a[3] = json!(format!("!{}", &blob[..2600.min(blob.len())]));
            a[4] = json!(uuid4().replace('-', ""));
            a[49] = json!(1);
            a[54] = json!([[[[[1]]]]]);
            a[55] = json!([[1]]);
        }
    }
    // Optional attachments: upload each (content-push single multipart, like HanaokaYuzu's working
    // browserless client) and reference them in message_content[3] = [[[id], name], ...].
    if !input.file.is_empty() {
        let mut refs = Vec::with_capacity(input.file.len());
        for p in &input.file {
            let (name, _mime, bytes) = super::read_attachment(p)?;
            let id = upload(base, client, &bytes).await?;
            refs.push(json!([[id], name]));
        }
        if let Value::Array(a) = &mut inner_val {
            if let Some(Value::Array(mc)) = a.get_mut(0) {
                mc[3] = json!(refs);
            }
        }
    }
    let inner = serde_json::to_string(&inner_val)?;
    let freq = serde_json::to_string(&json!([Value::Null, inner]))?;
    let url = format!(
        "{base}/_/BardChatUi/data/assistant.lamda.BardFrontendService/StreamGenerate\
         ?bl={bl}&f.sid={fsid}&hl={hl}&_reqid={}&rt=c",
        reqid()
    );

    let mut req = client
        .post(url)
        .header(
            "content-type",
            "application/x-www-form-urlencoded;charset=utf-8",
        )
        .header("origin", "https://gemini.google.com")
        .header("referer", "https://gemini.google.com/")
        .header("x-same-domain", "1")
        .header("x-goog-ext-525005358-jspb", format!("[\"{uuid}\",1]"));
    if let Some(id) = model_id(input.model.as_deref()) {
        req = req.header(
            "x-goog-ext-525001261-jspb",
            format!("[1,null,null,null,\"{id}\",null,null,0,[4]]"),
        );
    }
    let resp = req
        .body(form_encode(&[("at", &at), ("f.req", &freq)]))
        .send()
        .await?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    match status {
        400 | 401 => {
            return Err(Error::Provider {
                provider: "gemini_web",
                status,
                body: "session may be expired; run `fetchira login gemini_web`".into(),
            })
        }
        429 => return Err(Error::RateLimit("gemini_web: rate limited".into())),
        _ => {}
    }
    if matches!(cap, Capability::Image) {
        image_out(base, client, &text).await
    } else if want_dr_plan {
        parse_plan(&text)
    } else {
        parse(&text)
    }
}

/// Extract a generated image and download its bytes. Gemini has no image flag — the prompt intent
/// triggers it; the URL lives at `candidate[12][7][0][k][0][3][3]` (googleusercontent, auth-scoped),
/// so we fetch it with the same session and return the bytes. Empty `[12][7]` = a text-only reply.
async fn image_out(base: &str, client: &wreq::Client, body: &str) -> Result<Outcome> {
    let url = match scan_image(body)? {
        Some(u) => u,
        None => {
            // Gemini answers a spent image quota with a plain-text "limit reached" turn.
            if body.contains("Image Generation Limit") {
                return Err(Error::RateLimit(
                    "gemini_web: image generation limit reached — resets daily".into(),
                ));
            }
            // Gemini image generation is region-gated (unavailable in parts of the EU); it replies in
            // text saying so rather than rendering an image.
            if body.contains("your location") || body.contains("can't create") {
                return Err(Error::Provider {
                    provider: "gemini_web",
                    status: 0,
                    body:
                        "image generation isn't available for this account/region on gemini_web; \
                           use grok_web or chatgpt_web"
                            .into(),
                });
            }
            return Err(Error::BadResponse(
                "gemini_web: no generated image (text-only reply?)",
            ));
        }
    };
    let resp = client
        .get(&url)
        .header("referer", format!("{base}/"))
        .header("origin", base)
        .send()
        .await?;
    let mime = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/png")
        .to_string();
    let bytes = resp
        .bytes()
        .await
        .map_err(|_| Error::BadResponse("gemini_web"))?;
    let mut out = Outcome::new(String::new(), 1);
    out.image = Some(OutImage {
        mime,
        b64: base64::engine::general_purpose::STANDARD.encode(&bytes),
    });
    Ok(out)
}

/// Find the first generated-image URL in the framed response. Live-confirmed path:
/// `candidate[12][7][0][k][0][3][3]` (candidate = `resp[4][i]`). `Err(RateLimit)` on code 1037.
fn scan_image(body: &str) -> Result<Option<String>> {
    let mut found: Option<String> = None;
    for frame in frames(body) {
        let outer: Value = match serde_json::from_str(&frame) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for item in outer.as_array().into_iter().flatten() {
            let it = match item.as_array() {
                Some(x) => x,
                None => continue,
            };
            if it.first().and_then(|x| x.as_str()) != Some("wrb.fr") {
                continue;
            }
            if usage_limited(it) {
                return Err(Error::RateLimit("gemini_web: usage limit exceeded".into()));
            }
            let resp: Value = match it.get(2).and_then(|x| x.as_str()) {
                Some(p) => match serde_json::from_str(p) {
                    Ok(v) => v,
                    Err(_) => continue,
                },
                None => continue,
            };
            for cand in resp.get(4).and_then(|x| x.as_array()).into_iter().flatten() {
                let imgs = cand
                    .get(12)
                    .and_then(|x| x.get(7))
                    .and_then(|x| x.get(0))
                    .and_then(|x| x.as_array());
                for g in imgs.into_iter().flatten() {
                    if let Some(u) = g
                        .get(0)
                        .and_then(|x| x.get(3))
                        .and_then(|x| x.get(3))
                        .and_then(|x| x.as_str())
                    {
                        found = Some(u.to_string());
                    }
                }
            }
        }
    }
    Ok(found)
}

/// Upload one attachment to Google's Scotty push service and return its opaque id (a
/// `/contrib_service/ttl_1d/…_AX…` path) for `message_content[3]`. The web client uses a resumable
/// upload to `push.clients6.google.com` (NOT content-push.googleapis.com, which returns a shorter,
/// un-suffixed id the file turn then rejects with Bard 1100): a `start` POST returns an upload URL,
/// then an `upload, finalize` POST of the bytes returns the signed id in its body.
async fn upload(base: &str, client: &wreq::Client, bytes: &[u8]) -> Result<String> {
    // Upload to push.clients6.google.com (a *.google.com host, so the session cookies apply — unlike
    // content-push.googleapis.com which is *.googleapis.com and gets no cookies → unsigned id).
    // Resumable: `start` returns an upload URL; `upload, finalize` returns the signed id in its body.
    let start = client
        .post("https://push.clients6.google.com/upload/")
        .header("push-id", "feeds/mcudyrk2a4khkz")
        .header("x-tenant-id", "bard-storage")
        .header("x-goog-upload-protocol", "resumable")
        .header("x-goog-upload-command", "start")
        .header(
            "x-goog-upload-header-content-length",
            bytes.len().to_string(),
        )
        .header(
            "content-type",
            "application/x-www-form-urlencoded;charset=utf-8",
        )
        .header("origin", base)
        .header("referer", format!("{base}/"))
        .body(Vec::new())
        .send()
        .await?;
    let upload_url = start
        .headers()
        .get("x-goog-upload-url")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .ok_or(Error::BadResponse("gemini_web"))?;
    let fin = client
        .post(&upload_url)
        .header("push-id", "feeds/mcudyrk2a4khkz")
        .header("x-tenant-id", "bard-storage")
        .header("x-goog-upload-command", "upload, finalize")
        .header("x-goog-upload-offset", "0")
        .header(
            "content-type",
            "application/x-www-form-urlencoded;charset=utf-8",
        )
        .header("origin", base)
        .header("referer", format!("{base}/"))
        .body(bytes.to_vec())
        .send()
        .await?;
    if fin.status().as_u16() != 200 {
        return Err(Error::Provider {
            provider: "gemini_web",
            status: fin.status().as_u16(),
            body: "file upload rejected".into(),
        });
    }
    let id = fin.text().await.unwrap_or_default().trim().to_string();
    if id.is_empty() {
        return Err(Error::BadResponse("gemini_web"));
    }
    Ok(id)
}

/// The inner f.req array (length 69): only a handful of indices carry data, the rest are null.
/// `resume` is a prior `cid,rid,rcid` token; setting inner[2] to it continues that conversation.
fn build_inner(prompt: &str, hl: &str, uuid: &str, resume: Option<&str>) -> Value {
    let mut a = vec![Value::Null; 69];
    a[0] = json!([prompt, 0, null, null, null, null, 0]);
    a[1] = json!([hl]);
    a[2] = match resume {
        Some(meta) => {
            let ids: Vec<&str> = meta.split(',').collect();
            json!([
                ids.first().copied().unwrap_or(""),
                ids.get(1).copied().unwrap_or(""),
                ids.get(2).copied().unwrap_or(""),
            ])
        }
        None => json!(["", "", "", null, null, null, null, null, null, ""]),
    };
    a[6] = json!([1]);
    a[7] = json!(1);
    a[10] = json!(1);
    a[11] = json!(0);
    a[17] = json!([[0]]);
    a[18] = json!(0);
    a[27] = json!(1);
    a[30] = json!([4]);
    a[41] = json!([1]);
    a[53] = json!(0);
    a[59] = json!(uuid);
    a[61] = json!([]);
    a[68] = json!(2);
    Value::Array(a)
}

/// Per-turn request id: seeded low, advanced by 100000 each call (matches the web client).
fn reqid() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static R: AtomicU64 = AtomicU64::new(10000);
    R.fetch_add(100_000, Ordering::Relaxed)
}

/// Scrape a `"key":"value"` JS literal from the bootstrap HTML (tolerates whitespace after `:`).
fn scrape(html: &str, key: &str) -> Option<String> {
    let anchor = format!("\"{key}\":");
    let i = html.find(&anchor)? + anchor.len();
    let rest = html[i..].trim_start().strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// A signed-in bootstrap carries the WIZ boot tokens. `SNlM0e` alone is no longer reliable (Google
/// dropped it for logged-in sessions in Apr 2026), so any of these proves the session.
fn logged_in(page: &str) -> bool {
    ["SNlM0e", "cfb2h", "FdrFJe", "TuX5cc"]
        .iter()
        .any(|k| scrape(page, k).is_some())
}

fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Parse the framed StreamGenerate response. Frames are cumulative snapshots, so keep the
/// longest answer (at `resp[4][0][1][0]`) and the latest conversation ids (cid,rid at
/// `resp[1]`; rcid at `resp[4][0][0]`) for follow-ups.
fn parse(body: &str) -> Result<Outcome> {
    let mut best = String::new();
    let mut meta: Option<String> = None;
    for frame in frames(body) {
        let outer: Value = match serde_json::from_str(&frame) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for item in outer.as_array().into_iter().flatten() {
            let it = match item.as_array() {
                Some(x) => x,
                None => continue,
            };
            if it.first().and_then(|x| x.as_str()) != Some("wrb.fr") {
                continue;
            }
            if usage_limited(it) {
                return Err(Error::RateLimit("gemini_web: usage limit exceeded".into()));
            }
            let resp: Value = match it.get(2).and_then(|x| x.as_str()) {
                Some(p) => match serde_json::from_str(p) {
                    Ok(v) => v,
                    Err(_) => continue,
                },
                None => continue,
            };
            if let Some(t) = resp
                .get(4)
                .and_then(|c| c.get(0))
                .and_then(|c| c.get(1))
                .and_then(|c| c.get(0))
                .and_then(|t| t.as_str())
            {
                if t.len() > best.len() {
                    best = t.to_string();
                }
            }
            let cid = resp.get(1).and_then(|m| m.get(0)).and_then(|x| x.as_str());
            let rid = resp.get(1).and_then(|m| m.get(1)).and_then(|x| x.as_str());
            if let (Some(c), Some(r)) = (cid, rid) {
                let rcid = resp
                    .get(4)
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get(0))
                    .and_then(|x| x.as_str())
                    .unwrap_or_default();
                meta = Some(format!("{c},{r},{rcid}"));
            }
        }
    }
    if best.is_empty() {
        // Bard error 1100 on a file-attached turn: the session's upload id came back unsigned. This
        // tracks the `at` token — a session carrying SNlM0e signs the upload and works; one without
        // it (older/downgraded) gets 1100. The empty-`at` guard in `call` catches that case up front,
        // so reaching here means a token-carrying session still failed — surface it for failover.
        if body.contains("[1100]") {
            return Err(Error::Provider {
                provider: "gemini_web",
                status: 1100,
                body: "gemini rejected the attachment; run `fetchira login gemini_web`".into(),
            });
        }
        return Err(Error::BadResponse("gemini_web"));
    }
    let mut out = Outcome::new(best, 1);
    out.session = meta;
    Ok(out)
}

/// Extract a deep-research PLAN (title, ETA, steps) + conversation ids. The plan lives at
/// `resp[4][0][12][0]["56"]`: [0]=title, [1]=steps tree, [2]=eta.
fn parse_plan(body: &str) -> Result<Outcome> {
    let mut plan: Option<String> = None;
    let mut meta: Option<String> = None;
    for frame in frames(body) {
        let outer: Value = match serde_json::from_str(&frame) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for item in outer.as_array().into_iter().flatten() {
            let it = match item.as_array() {
                Some(x) => x,
                None => continue,
            };
            if it.first().and_then(|x| x.as_str()) != Some("wrb.fr") {
                continue;
            }
            if usage_limited(it) {
                return Err(Error::RateLimit("gemini_web: usage limit exceeded".into()));
            }
            let resp: Value = match it.get(2).and_then(|x| x.as_str()) {
                Some(p) => match serde_json::from_str(p) {
                    Ok(v) => v,
                    Err(_) => continue,
                },
                None => continue,
            };
            if let Some(p) = resp
                .get(4)
                .and_then(|c| c.get(0))
                .and_then(|c| c.get(12))
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("56"))
            {
                let title = p.get(0).and_then(|x| x.as_str()).unwrap_or("Research plan");
                let eta = p.get(2).and_then(|x| x.as_str()).unwrap_or_default();
                let mut steps = Vec::new();
                if let Some(s) = p.get(1) {
                    collect_strings(s, &mut steps);
                }
                plan = Some(format!("# {title}\n_{eta}_\n\n{}", steps.join("\n")));
            }
            let cid = resp.get(1).and_then(|m| m.get(0)).and_then(|x| x.as_str());
            let rid = resp.get(1).and_then(|m| m.get(1)).and_then(|x| x.as_str());
            if let (Some(c), Some(r)) = (cid, rid) {
                let rcid = resp
                    .get(4)
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get(0))
                    .and_then(|x| x.as_str())
                    .unwrap_or_default();
                meta = Some(format!("{c},{r},{rcid}"));
            }
        }
    }
    let plan = plan.ok_or(Error::BadResponse(
        "gemini_web: no research plan (deep research may be unavailable on this account)",
    ))?;
    let mut out = Outcome::new(
        format!("{plan}\n\nReply with this session + query \"start\" to run the research, or send an adjustment to refine the plan."),
        2,
    );
    out.session = meta.map(|m| format!("dr|{m}"));
    Ok(out)
}

/// Recursively gather human-readable strings (dropping googleusercontent artifacts).
fn collect_strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            if s.len() > 2 && !s.starts_with("http://googleusercontent.com") {
                out.push(s.clone());
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_strings(x, out)),
        Value::Object(o) => o.values().for_each(|x| collect_strings(x, out)),
        _ => {}
    }
}

/// Map a friendly model name to Gemini's opaque model id (or pass a raw id through).
/// ponytail: ids rotate with model launches; update this map or pass the raw id. Unknown
/// names fall back to the account default (no header).
fn model_id(m: Option<&str>) -> Option<String> {
    let m = m?;
    let k: String = m
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let id = match k.as_str() {
        "pro" | "3pro" | "31pro" | "gemini3pro" | "gemini31pro" => "e6fa609c3fa255c0",
        "flash" | "3flash" | "35flash" | "gemini3flash" | "gemini35flash" => "56fdd199312815e2",
        "flashlite" | "31flashlite" | "35flashlite" | "geminiflashlite" => "8c46e95b1a07cecc",
        _ if m.len() >= 12 && m.bytes().all(|b| b.is_ascii_hexdigit()) => {
            return Some(m.to_string())
        }
        _ => return None,
    };
    Some(id.to_string())
}

/// Live model catalog via the `otAQ7b` (LIST_MODELS) batchexecute RPC. Gemini's web app exposes NO
/// live usage/remaining count (verified by capture: only the reactive 1037 in `usage_limited`), so
/// this is a catalog only — the selectable models + their thinking levels, no per-model limit. Model
/// ids rotate, so fetching keeps the picker honest even as they change.
pub(crate) async fn limits(base: &str, client: &wreq::Client) -> Result<LiveLimits> {
    // Same session handshake as `call`: preflight for consent cookies, then scrape the bootstrap,
    // rotating the companion cookie only if it renders logged-out.
    let _ = client.get("https://www.google.com").send().await;
    let mut page = client
        .get(format!("{base}/app"))
        .send()
        .await?
        .text()
        .await
        .unwrap_or_default();
    if !logged_in(&page) {
        let _ = client
            .post("https://accounts.google.com/RotateCookies")
            .header("content-type", "application/json")
            .header("origin", "https://accounts.google.com")
            .body("[000,\"-0000000000000000000\"]")
            .send()
            .await;
        page = client
            .get(format!("{base}/app"))
            .send()
            .await?
            .text()
            .await
            .unwrap_or_default();
    }
    if !logged_in(&page) {
        return Err(Error::Provider {
            provider: "gemini_web",
            status: 0,
            body: "no session token; run `fetchira login gemini_web`".into(),
        });
    }
    let at = scrape(&page, "SNlM0e").unwrap_or_default();
    let bl = scrape(&page, "cfb2h").unwrap_or_default();
    let fsid = scrape(&page, "FdrFJe").unwrap_or_default();
    let hl = scrape(&page, "TuX5cc").unwrap_or_else(|| "en".into());

    let raw = rpc(base, client, &at, &bl, &fsid, &hl, "otAQ7b", "[]").await?;
    Ok(LiveLimits {
        tier: None,
        features: Vec::new(),
        models: parse_models(&raw),
    })
}

/// One batchexecute RPC. Envelope: `f.req=[[[rpcid, <payload-json-string>, null, "generic"]]]` + `at`.
#[allow(clippy::too_many_arguments)]
async fn rpc(
    base: &str,
    client: &wreq::Client,
    at: &str,
    bl: &str,
    fsid: &str,
    hl: &str,
    rpcid: &str,
    payload: &str,
) -> Result<String> {
    let freq = serde_json::to_string(&json!([[[rpcid, payload, Value::Null, "generic"]]]))?;
    let url = format!(
        "{base}/_/BardChatUi/data/batchexecute\
         ?rpcids={rpcid}&source-path=%2Fapp&bl={bl}&f.sid={fsid}&hl={hl}&_reqid={}&rt=c",
        reqid()
    );
    let resp = client
        .post(url)
        .header(
            "content-type",
            "application/x-www-form-urlencoded;charset=utf-8",
        )
        .header("origin", "https://gemini.google.com")
        .header("referer", "https://gemini.google.com/")
        .header("x-same-domain", "1")
        .body(form_encode(&[("f.req", &freq), ("at", at)]))
        .send()
        .await?;
    Ok(resp.text().await.unwrap_or_default())
}

/// Parse `otAQ7b`: models live at frame `inner[15]`, each `[id, name, desc, ...]`. Pro carries a
/// Standard/Extended thinking axis in the picker; Flash/Flash-Lite are single-level.
fn parse_models(body: &str) -> Vec<ModelInfo> {
    for frame in frames(body) {
        let outer: Value = match serde_json::from_str(&frame) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for item in outer.as_array().into_iter().flatten() {
            let it = match item.as_array() {
                Some(x) => x,
                None => continue,
            };
            if it.first().and_then(|x| x.as_str()) != Some("wrb.fr") {
                continue;
            }
            if it.get(1).and_then(|x| x.as_str()) != Some("otAQ7b") {
                continue;
            }
            let inner: Value = match it
                .get(2)
                .and_then(|x| x.as_str())
                .and_then(|p| serde_json::from_str(p).ok())
            {
                Some(v) => v,
                None => continue,
            };
            let Some(arr) = inner.get(15).and_then(|x| x.as_array()) else {
                continue;
            };
            return arr
                .iter()
                .filter_map(|m| {
                    let id = m.get(0).and_then(|x| x.as_str())?;
                    let name = m.get(1).and_then(|x| x.as_str()).unwrap_or(id);
                    let levels = if name.to_ascii_lowercase().contains("pro") {
                        vec!["standard".to_string(), "extended".to_string()]
                    } else {
                        Vec::new()
                    };
                    Some(ModelInfo {
                        id: id.to_string(),
                        name: name.to_string(),
                        levels,
                        remaining: None,
                        total: None,
                        window_secs: None,
                        reset_after: None,
                        locked: false,
                    })
                })
                .collect();
        }
    }
    Vec::new()
}

/// Gemini's only reliable "out of quota" signal: a fatal error code at `wrb.fr[5][2][0][1][0]`,
/// where 1037 = USAGE_LIMIT_EXCEEDED. The web app exposes no remaining-count, so this reactive
/// hit is all there is — map it to a rate-limit so the account is marked exhausted and the router
/// fails over. Index path mirrors HanaokaYuzu/Gemini-API's detection.
fn usage_limited(it: &[Value]) -> bool {
    it.get(5)
        .and_then(|x| x.get(2))
        .and_then(|x| x.get(0))
        .and_then(|x| x.get(1))
        .and_then(|x| x.get(0))
        .and_then(|x| x.as_i64())
        == Some(1037)
}

// Each payload is one compact-JSON array per physical line (Gemini escapes any newline
// inside strings), so split on lines and skip the `)]}'` prelude + bare length markers.
// ponytail: line-based instead of the documented UTF-16 length prefix, whose count desyncs
// the stream here; revisit only if a payload ever spans physical lines.
fn frames(s: &str) -> Vec<String> {
    s.lines()
        .map(str::trim)
        .filter(|l| l.starts_with("[["))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_framed_answer_with_multibyte() {
        let resp = json!([null, null, null, null, [[null, ["The answer 😀 inside"]]]]).to_string();
        let outer = json!([["wrb.fr", null, resp]]).to_string();
        let n = outer.encode_utf16().count();
        let body = format!(")]}}'\n{n}\n{outer}");
        let out = parse(&body).unwrap();
        assert!(out.text.contains("The answer"));
        assert!(out.text.contains('😀'));
    }

    #[test]
    fn scrape_reads_token() {
        let html = r#"...,"SNlM0e":"abc123","cfb2h":"build_42",..."#;
        assert_eq!(scrape(html, "SNlM0e").as_deref(), Some("abc123"));
        assert_eq!(scrape(html, "cfb2h").as_deref(), Some("build_42"));
    }

    // Build nested arrays so the result indexed by `path` equals `leaf`.
    fn nested_at(path: &[usize], leaf: Value) -> Value {
        let mut v = leaf;
        for &i in path.iter().rev() {
            let mut arr = vec![Value::Null; i + 1];
            arr[i] = v;
            v = Value::Array(arr);
        }
        v
    }

    #[test]
    fn scan_image_finds_url_at_confirmed_path() {
        let url = "https://lh3.googleusercontent.com/gen=s512-rj";
        // one image entry g: g[0][3] = [.., .., alt, url]
        let g = json!([[null, null, null, [null, null, "alt", url]]]);
        // live path: resp[4][0][12][7][0] = [g]  (candidate[12][7][0] = images array)
        let resp = nested_at(&[4, 0, 12, 7, 0], json!([g]));
        let inner = json!([["wrb.fr", null, resp.to_string()]]).to_string();
        let n = inner.encode_utf16().count();
        let body = format!(")]}}'\n{n}\n{inner}");
        assert_eq!(scan_image(&body).unwrap().as_deref(), Some(url));

        // usage-limit frame (1037) -> RateLimit
        let limited = json!([[
            "wrb.fr",
            null,
            null,
            null,
            null,
            [null, null, [[null, [1037]]]]
        ]]);
        let n2 = limited.to_string().encode_utf16().count();
        let body2 = format!(")]}}'\n{n2}\n{}", limited);
        assert!(matches!(scan_image(&body2), Err(Error::RateLimit(_))));
    }

    #[test]
    fn parses_otaq7b_model_catalog() {
        let mut inner = vec![Value::Null; 16];
        inner[15] = json!([
            ["e6fa609c3fa255c0", "Pro", "Advanced maths and code"],
            ["8c46e95b1a07cecc", "3.1 Flash-Lite", "Fastest answers"],
        ]);
        let frame = json!([[
            "wrb.fr",
            "otAQ7b",
            Value::Array(inner).to_string(),
            null,
            null,
            null,
            "generic"
        ]]);
        let body = format!(")]}}'\n\n0\n{}", frame);
        let models = parse_models(&body);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].name, "Pro");
        assert_eq!(models[0].levels, vec!["standard", "extended"]);
        assert_eq!(models[0].id, "e6fa609c3fa255c0");
        assert!(models[0].remaining.is_none());
        assert!(models[1].levels.is_empty());
    }

    #[test]
    fn usage_limit_maps_to_rate_limit() {
        // wrb.fr item carrying the fatal code at [5][2][0][1][0] = 1037 (USAGE_LIMIT_EXCEEDED).
        let item = json!([
            "wrb.fr",
            null,
            null,
            null,
            null,
            [null, null, [[null, [1037]]]],
            "generic"
        ]);
        let outer = json!([item]).to_string();
        let n = outer.encode_utf16().count();
        let body = format!(")]}}'\n{n}\n{outer}");
        assert!(matches!(parse(&body), Err(Error::RateLimit(_))));
    }
}
