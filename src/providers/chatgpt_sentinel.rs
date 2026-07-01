use std::sync::LazyLock;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use regex::Regex;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::uuid4;
use crate::error::{Error, Result};

// OpenAI Sentinel proof-of-work — the mandatory gate on every chatgpt.com backend-api turn (a raw POST
// without it 403s "unusual activity"). Reversed from the web bundle (`/cdn/assets/*`, 2026-06-30): the
// proof is NOT crypto. The "hash" is a 32-bit FNV-1a + fmix32 (`LDt`), and an answer is
//   base64(JSON(config)) + "~S"
// brute-forced over `config[3]` (the iteration counter) until `fnv1a(seed + base64)[..difficulty.len]`
// clears a hex-prefix `difficulty`. Tokens: requirements/preflight = "gAAAAAC"+answer (self-seeded,
// difficulty "0"); enforcement = "gAAAAAB"+answer (against a server-issued proofofwork seed/difficulty).
//
// `token()` runs the prepare->finalize split and returns the `openai-sentinel-chat-requirements-token`
// header value. `turnstile.required` is advisory (finalize still returns a usable token). All calls go
// out with the full client header set carried in `Ctx`, including the per-turn conduit token.

const PREPARE: &str = "/backend-api/sentinel/chat-requirements/prepare";
const FINALIZE: &str = "/backend-api/sentinel/chat-requirements/finalize";
const MAX_ITERS: u32 = 500_000;
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36";
const BUILD_NUMBER: &str = "7904904";

/// The client identity headers every chatgpt.com backend-api call carries. A request missing them (or
/// the per-turn `conduit` from `/f/conversation/prepare`) is rejected as "unusual activity".
pub struct Ctx {
    pub bearer: String,
    pub account_id: String,
    pub device_id: String,
    pub session_id: String,
    pub build: String,
    pub conduit: String,
}

impl Ctx {
    pub fn apply(&self, req: wreq::RequestBuilder, path: &str) -> wreq::RequestBuilder {
        req.header("authorization", format!("Bearer {}", self.bearer))
            .header("chatgpt-account-id", &self.account_id)
            .header("content-type", "application/json")
            .header("oai-client-build-number", BUILD_NUMBER)
            .header("oai-client-version", &self.build)
            .header("oai-device-id", &self.device_id)
            .header("oai-language", "en-US")
            .header("oai-session-id", &self.session_id)
            .header("x-conduit-token", &self.conduit)
            .header("x-oai-turn-trace-id", uuid4())
            .header("x-openai-target-path", path)
            .header("x-openai-target-route", path)
    }
}

/// Mint a chat-requirements token (prepare -> finalize). `turnstile.required` is advisory — finalize
/// still returns a usable token without solving it (verified live 2026-06-30), so we don't bail on it.
pub async fn token(base: &str, client: &wreq::Client, ctx: &Ctx) -> Result<String> {
    let prep = post(
        base,
        client,
        ctx,
        PREPARE,
        &json!({ "p": answer("gAAAAAC", &seed(), "0", &ctx.build) }),
    )
    .await?;
    let prepare_token = prep
        .get("prepare_token")
        .and_then(Value::as_str)
        .ok_or(Error::BadResponse("chatgpt_web"))?;

    let mut body = json!({ "prepare_token": prepare_token });
    if flag(&prep, "/proofofwork/required") {
        let s = prep
            .pointer("/proofofwork/seed")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let d = prep
            .pointer("/proofofwork/difficulty")
            .and_then(Value::as_str)
            .unwrap_or("0");
        body["proofofwork"] = json!(answer("gAAAAAB", s, d, &ctx.build));
    }
    let fin = post(base, client, ctx, FINALIZE, &body).await?;
    fin.get("token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(Error::BadResponse("chatgpt_web"))
}

// Build id is page-global (not per-account), so one cached value serves every call; dropped on a 403 so
// the next call re-scrapes a rotated deploy. Mirrors grok_statsig's cache.
static BUILD: LazyLock<Mutex<Option<String>>> = LazyLock::new(|| Mutex::new(None));

pub async fn invalidate() {
    *BUILD.lock().await = None;
}

pub async fn build_id(base: &str, client: &wreq::Client) -> Result<String> {
    let mut cache = BUILD.lock().await;
    if let Some(b) = cache.as_ref() {
        return Ok(b.clone());
    }
    let html = client.get(base).send().await?.text().await?;
    let b = scrape_build(&html).ok_or(Error::BadResponse("chatgpt_web"))?;
    *cache = Some(b.clone());
    Ok(b)
}

fn scrape_build(html: &str) -> Option<String> {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"data-build="([^"]+)""#).unwrap());
    RE.captures(html).map(|c| c[1].to_string())
}

async fn post(
    base: &str,
    client: &wreq::Client,
    ctx: &Ctx,
    path: &str,
    body: &Value,
) -> Result<Value> {
    let resp = ctx
        .apply(client.post(format!("{base}{path}")), path)
        .body(body.to_string())
        .send()
        .await?;
    match resp.status().as_u16() {
        401 | 403 => Err(Error::Provider {
            provider: "chatgpt_web",
            status: 403,
            body: "session/cloudflare; run `fetchira login chatgpt_web`".into(),
        }),
        s if s >= 400 => Err(Error::Provider {
            provider: "chatgpt_web",
            status: s,
            body: "sentinel rejected".into(),
        }),
        _ => {
            let text = resp.text().await.unwrap_or_default();
            Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
        }
    }
}

/// `prefix + base64(JSON(config)) + "~S"`, brute-forcing `config[3]` until `fnv1a(seed+base64)` clears
/// the hex-prefix `difficulty`. The config is full of env/random values the server can't validate; only
/// `config[6]` (build id) is load-bearing.
fn answer(prefix: &str, seed: &str, difficulty: &str, build: &str) -> String {
    let n = difficulty.len().min(8);
    let mut cfg = config(build);
    for i in 0..MAX_ITERS {
        cfg[3] = json!(i);
        let b64 = STANDARD.encode(cfg_json(&cfg));
        if fnv1a(seed, &b64)[..n] <= difficulty[..n] {
            return format!("{prefix}{b64}~S");
        }
    }
    format!("{prefix}{}~S", STANDARD.encode(cfg_json(&config(build))))
}

fn cfg_json(cfg: &[Value]) -> String {
    serde_json::to_string(cfg).unwrap_or_default()
}

/// FNV-1a (32-bit) + fmix32 over `seed ++ b64`, as 8 hex chars (the bundle's `LDt`). Inputs are ASCII,
/// so byte iteration matches the JS `charCodeAt`.
fn fnv1a(seed: &str, b64: &str) -> String {
    let mut h: u32 = 2166136261;
    for byte in seed.bytes().chain(b64.bytes()) {
        h ^= byte as u32;
        h = h.wrapping_mul(16777619);
    }
    h ^= h >> 16;
    h = h.wrapping_mul(2246822507);
    h ^= h >> 13;
    h = h.wrapping_mul(3266489909);
    h ^= h >> 16;
    format!("{h:08x}")
}

fn config(build: &str) -> Vec<Value> {
    json!([
        4000,
        chrono::Utc::now()
            .format("%a %b %d %Y %H:%M:%S GMT+0000")
            .to_string(),
        4_294_705_152u64,
        0,
        UA,
        "",
        build,
        "en-US",
        "en-US,en",
        0,
        "language−function",
        "0",
        "0",
        1000,
        uuid4(),
        "",
        8,
        1_700_000_000_000u64,
        0,
        0,
        0,
        0,
        0,
        0,
        1
    ])
    .as_array()
    .cloned()
    .unwrap_or_default()
}

fn seed() -> String {
    format!("0.{}", uuid4().replace('-', ""))
}

fn flag(v: &Value, ptr: &str) -> bool {
    v.pointer(ptr).and_then(Value::as_bool).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pinned against the bundle's `LDt` (FNV-1a + fmix32 over the concatenated input).
    #[test]
    fn fnv1a_matches_bundle() {
        assert_eq!(fnv1a("", ""), "ab3e7c0b");
        assert_eq!(fnv1a("seed", "QQ=="), "d6f87bf8");
        assert_eq!(fnv1a("", "abc"), "1cc93dbc");
    }

    #[test]
    fn answer_shape() {
        let a = answer("gAAAAAC", &seed(), "0", "prod-x");
        assert!(a.starts_with("gAAAAAC"));
        assert!(a.ends_with("~S"));
    }

    #[test]
    fn scrapes_build() {
        let html = r#"<html data-build="prod-abc123" lang="en">"#;
        assert_eq!(scrape_build(html).as_deref(), Some("prod-abc123"));
    }
}
