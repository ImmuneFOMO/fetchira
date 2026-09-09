use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use super::{chatgpt_web, uuid4, Capability, Input, OutImage, Outcome};
use crate::error::{Error, Result};
use crate::web::{detect_browser, Cookie};

// chatgpt.com generation is gated by an anti-bot defense pure HTTP can't pass (the real browser works,
// a byte-identical replay 403s "unusual activity"). So we drive a real Chrome over CDP: inject the
// captured session cookies, type the prompt into the composer (the page's own send is what passes the
// gate), then read the answer back via an in-page GET (reads are not gated) and reuse chatgpt_web's
// conversation parsers. Deep research / web search are enabled by clicking the composer's tools menu.
//
// Headless vs headful is a launch flag, not a driver split: locally we run `--headless=new`;
// the hosted container sets FETCHIRA_BROWSER_HEADFUL=1 and provides an X display (Xvfb) so the
// browser matches the local, proven environment — headless new-mode renders differently enough
// that image-heavy ChatGPT pages wedged its renderer on the hosted server.

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const CHAT_WAIT: u64 = 120;
// Image generation runs longer than a chat turn (often 30-60s).
// Kickoff holds the request briefly so fast generations return inline; slower renders hand back
// an `img|poll|` session instead of failing the call, and the follow-up resumes the wait.
const IMAGE_KICKOFF_WAIT: u64 = 90;
const DRIVE_WAIT: Duration = Duration::from_secs(240);
const CDP_COMMAND_WAIT: Duration = Duration::from_secs(20);
// A normal-Chrome UA: the default `--headless` UA contains "HeadlessChrome", an instant Cloudflare tell.
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36";

fn headless_arg() -> Option<&'static str> {
    (std::env::var("FETCHIRA_BROWSER_HEADFUL").as_deref() != Ok("1")).then_some("--headless=new")
}

fn normalize_conversation_cid(raw: &str) -> Result<String> {
    let cid = raw.strip_prefix("WEB:").unwrap_or(raw);
    let bytes = cid.as_bytes();
    if bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, byte)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
    {
        Ok(cid.to_ascii_lowercase())
    } else {
        Err(Error::Provider {
            provider: "chatgpt_web",
            status: 400,
            body: "invalid conversation session".into(),
        })
    }
}

/// The `img|poll|<cid>|<query>` session token carries the exact submitted prompt so the poll can
/// match the right user turn in a conversation that already holds image generations. Queries are
/// free text, so everything outside a safe set is percent-escaped; a bare `|` would split the
/// token early.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `img|poll|<cid>|<query>` — `cid` is empty when ChatGPT accepted the prompt but the
/// conversation list has not listed it yet. The follow-up then scans recent threads
/// for that exact submitted query instead of opening `/c/`.
fn parse_img_poll(session: Option<&str>) -> Result<Option<(String, String)>> {
    let Some(rest) = session.and_then(|s| s.strip_prefix("img|poll|")) else {
        return Ok(None);
    };
    let (raw_cid, raw_query) = rest.split_once('|').unwrap_or((rest, ""));
    let cid = if raw_cid.is_empty() {
        String::new()
    } else {
        normalize_conversation_cid(raw_cid)?
    };
    Ok(Some((cid, percent_decode(raw_query))))
}

/// Aborted callers can leave a headless profile behind after the parent process is killed.
fn cleanup_stale_profiles() {
    let now = SystemTime::now();
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("fetchira-cgpt-"))
        {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > Duration::from_secs(3600));
        if stale {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

pub async fn run(cookies: &[Cookie], cap: Capability, input: &Input) -> Result<Outcome> {
    let query = input.query.as_deref().unwrap_or_default().to_string();
    let search = matches!(cap, Capability::Search) && chatgpt_web::web_search_on(input);
    let browser = detect_browser()
        .ok_or_else(|| Error::Config("no Chrome/Chromium for chatgpt_web browser mode".into()))?;
    cleanup_stale_profiles();
    clear_cdp_image_responses();
    let profile = std::env::temp_dir().join(format!("fetchira-cgpt-{}", &uuid4()[..8]));
    let port = free_port();
    let mut child = tokio::process::Command::new(&browser.bin)
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--remote-debugging-port={port}"))
        .arg("--remote-allow-origins=*")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-logging")
        .arg("--log-level=3")
        .arg("--disable-dev-shm-usage")
        .args(headless_arg())
        .arg("--disable-blink-features=AutomationControlled")
        .arg(format!("--user-agent={UA}"))
        .arg("--window-size=1280,1000")
        // The Debian sandbox cannot initialize inside the unprivileged hosted container.
        // The container itself is the isolation boundary; keep the normal local-browser
        // launch sandboxed.
        .args(
            (std::env::var("FETCHIRA_CONTAINER").as_deref() == Ok("1")
                || Path::new("/.dockerenv").exists())
            .then_some("--no-sandbox"),
        )
        .arg("about:blank")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;

    // Surface an immediate browser exit instead of waiting for the generic CDP timeout.
    tokio::time::sleep(Duration::from_millis(150)).await;
    if let Some(status) = child.try_wait()? {
        return Err(Error::Config(format!(
            "chatgpt browser exited before CDP started ({status})"
        )));
    }

    let out = match timeout(
        DRIVE_WAIT,
        drive(
            port,
            cookies,
            cap,
            input.model.as_deref(),
            search,
            input.session.as_deref(),
            &input.file,
            &query,
        ),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(Error::Timeout("chatgpt_web: browser drive")),
    };
    let _ = child.kill().await;
    let _ = std::fs::remove_dir_all(&profile);
    out
}

/// Read the same authenticated account/model endpoints as the local UI, through the browser
/// session. ChatGPT blocks these reads from the impersonating HTTP client on some VPS networks.
pub async fn limits(cookies: &[Cookie]) -> Result<super::LiveLimits> {
    let browser = detect_browser()
        .ok_or_else(|| Error::Config("no Chrome/Chromium for chatgpt_web browser mode".into()))?;
    cleanup_stale_profiles();
    let profile = std::env::temp_dir().join(format!("fetchira-cgpt-limits-{}", &uuid4()[..8]));
    let port = free_port();
    let mut child = tokio::process::Command::new(&browser.bin)
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--remote-debugging-port={port}"))
        .arg("--remote-allow-origins=*")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-logging")
        .arg("--log-level=3")
        .arg("--disable-dev-shm-usage")
        .args(headless_arg())
        .arg("--disable-blink-features=AutomationControlled")
        .arg(format!("--user-agent={UA}"))
        .arg("--window-size=1280,1000")
        .args(
            (std::env::var("FETCHIRA_CONTAINER").as_deref() == Ok("1")
                || Path::new("/.dockerenv").exists())
            .then_some("--no-sandbox"),
        )
        .arg("about:blank")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let result = timeout(Duration::from_secs(45), async {
        let ws_url = wait_for_page(port).await?;
        let (mut ws, _) = connect_async(ws_url.as_str()).await?;
        cmd(&mut ws, "Network.enable", json!({})).await?;
        cmd(&mut ws, "Page.enable", json!({})).await?;
        let _ = cmd(&mut ws, "Runtime.enable", json!({})).await;
        cmd(&mut ws, "Page.navigate", json!({"url":"https://chatgpt.com/"})).await?;
        sleep(Duration::from_millis(300)).await;
        let result = cmd(&mut ws, "Network.setCookies", json!({"cookies": cdp_cookies(cookies)})).await?;
        if result.get("success").is_some_and(|v| v == false) {
            return Err(Error::BadResponse("chatgpt_web: browser rejected session cookies"));
        }
        cmd(&mut ws, "Page.navigate", json!({"url":"https://chatgpt.com/"})).await?;
        sleep(Duration::from_secs(3)).await;
        // The backend endpoints return a Guest view unless the browser also supplies the
        // short-lived access token from `/api/auth/session`. The UI does this same exchange.
        // Keep the token browser-side; it never leaves the page or appears in logs.
        let js = r#"(async()=>{const s=async(r)=>({status:r.status,text:await r.text()});const sig=()=>AbortSignal.timeout(15000);let auth={},sess=null;try{const a=await fetch('/api/auth/session',{credentials:'include',signal:sig()});sess=await a.json();if(sess.accessToken){auth={'Authorization':'Bearer '+sess.accessToken};try{let b=sess.accessToken.split('.')[1].replace(/-/g,'+').replace(/_/g,'/');while(b.length%4)b+='=';const p=JSON.parse(atob(b));const id=p['https://api.openai.com/auth']&&p['https://api.openai.com/auth'].chatgpt_account_id;if(id)auth['ChatGPT-Account-ID']=id;}catch(_){}}}catch(_){}const get=(url,options={})=>fetch(url,{...options,credentials:'include',headers:{...auth,...(options.headers||{})},signal:sig()}).then(s).catch(e=>({status:0,text:String(e)}));const [i,m,t]=await Promise.all([get('/backend-api/conversation/init',{method:'POST',headers:{'content-type':'application/json'},body:'{}'}),get('/backend-api/models?history_and_training_disabled=false'),get('/backend-api/accounts/check/v4-2023-04-27')]);return JSON.stringify({init:i,models:m,tier:t,session:sess});})()"#;
        // Cookie injection reloads chatgpt.com; the inspected context can vanish for a beat.
        for _ in 0..4 {
            let Ok(raw) = eval(&mut ws, js).await else {
                sleep(Duration::from_secs(2)).await;
                continue;
            };
            let Some(body) = raw.as_str() else {
                sleep(Duration::from_secs(2)).await;
                continue;
            };
            let Ok(v) = serde_json::from_str::<Value>(body) else {
                sleep(Duration::from_secs(2)).await;
                continue;
            };
            if !v["init"]["status"].as_u64().is_some_and(|s| (200..300).contains(&s)) {
                sleep(Duration::from_secs(2)).await;
                continue;
            }
            let payload = json!({
                "init": v["init"]["text"],
                "models": v["models"]["text"],
                "tier": v["tier"]["text"],
                "session": v["session"],
            });
            return chatgpt_web::parse_limits_json(&payload);
        }
        Err(Error::BadResponse("chatgpt_web: limits endpoint rejected session"))
    }).await.map_err(|_| Error::Timeout("chatgpt_web: limits browser"));
    let _ = child.kill().await;
    let _ = child.wait().await;
    let _ = std::fs::remove_dir_all(&profile);
    result?
}

#[allow(clippy::too_many_arguments)]
async fn drive(
    port: u16,
    cookies: &[Cookie],
    cap: Capability,
    model: Option<&str>,
    search: bool,
    session: Option<&str>,
    files: &[PathBuf],
    query: &str,
) -> Result<Outcome> {
    // Deep research has a plan step: kickoff drafts a plan (parked), then `dr|plan|<cid>` + "start"
    // approves it. A non-`dr|`/non-`img|` session is a chat conversation to continue
    // (`dr|poll` and `img|poll` stay here; the HTTP chatgpt_web path cannot reuse this cookie jar).
    let dr_plan_cid = session
        .and_then(|s| s.strip_prefix("dr|plan|"))
        .map(normalize_conversation_cid)
        .transpose()?;
    let dr_poll_cid = session
        .and_then(|s| s.strip_prefix("dr|poll|"))
        .map(normalize_conversation_cid)
        .transpose()?;
    // Image poll carries the exact submitted prompt (`img|poll|<cid>|<percent-escaped query>`):
    // matching by `create_time` order alone can bind the response to the wrong turn in a chat
    // that already holds image generations (a same-conversation edit shares the cid).
    let img_poll = parse_img_poll(session)?;
    let approve =
        matches!(cap, Capability::DeepResearch) && dr_plan_cid.is_some() && is_start_word(query);
    let resume = session
        .filter(|s| !s.starts_with("dr|") && !s.starts_with("img|"))
        .map(|s| s.strip_prefix("chatgpt_web:").unwrap_or(s))
        .map(|s| s.split('|').next().unwrap_or(s))
        .map(normalize_conversation_cid)
        .transpose()?;

    let ws_url = wait_for_page(port).await?;
    let (mut ws, _) = connect_async(ws_url.as_str()).await?;
    cmd(&mut ws, "Network.enable", json!({})).await?;
    cmd(&mut ws, "Page.enable", json!({})).await?;
    cmd(&mut ws, "DOM.enable", json!({})).await?;
    let url = match (
        approve.then_some(dr_plan_cid.as_deref()).flatten(),
        dr_poll_cid.as_deref(),
        img_poll
            .as_ref()
            .map(|(cid, _)| cid.as_str())
            .filter(|cid| !cid.is_empty()),
        resume.as_deref(),
    ) {
        (Some(cid), _, _, _) | (None, Some(cid), _, _) | (None, None, Some(cid), _) => {
            format!("https://chatgpt.com/c/{cid}")
        }
        (None, None, None, Some(c)) => format!("https://chatgpt.com/c/{c}"),
        (None, None, None, None) => "https://chatgpt.com/".to_string(),
    };
    // Establish the ChatGPT origin before injecting cookies. In particular, __Host-* cookies
    // are host-only and are rejected by Chromium when set against the initial about:blank page.
    // Navigating to a conversation before cookies are installed redirects to login; reloading
    // that redirected page would lose the requested conversation and break follow-ups.
    cmd(
        &mut ws,
        "Page.navigate",
        json!({ "url": "https://chatgpt.com/" }),
    )
    .await?;
    sleep(Duration::from_millis(300)).await;
    let cookie_result = cmd(
        &mut ws,
        "Network.setCookies",
        json!({ "cookies": cdp_cookies(cookies) }),
    )
    .await?;
    if cookie_result.get("success").is_some_and(|v| v == false) {
        return Err(Error::BadResponse(
            "chatgpt_web: browser rejected session cookies",
        ));
    }
    cmd(&mut ws, "Page.navigate", json!({ "url": url })).await?;
    if resume.is_some()
        || approve
        || dr_poll_cid.is_some()
        || img_poll.as_ref().is_some_and(|(cid, _)| !cid.is_empty())
    {
        let cid = url.rsplit('/').next().unwrap_or_default();
        let escaped = serde_json::to_string(cid).unwrap_or_else(|_| "\"\"".into());
        wait_until(
            &mut ws,
            &format!("location.href.includes('/c/') && location.href.includes({escaped})"),
            45,
        )
        .await?;
    }

    // Wait for the composer to render; Cloudflare's interstitial clears first. The composer alone is
    // NOT proof of a live session: the logged-out homepage renders the same #prompt-textarea, so the
    // old gate let an unauthenticated page through — it then "sent" into the login flow
    // (send_result=sent) and hung in wait_for_cid forever, since no conversation is ever created.
    // Confirm the injected cookies actually authenticate before driving the composer.
    // ChatGPT has shipped both a textarea and a contenteditable composer. Prefer the visible
    // editor; the textarea can remain as a hidden compatibility node on current builds.
    // `/api/auth/session` is a cookie-backed fetch and can hang independently of CDP.
    // Keep this check bounded so an expired/broken browser session returns a useful login error.
    let logged_in = is_logged_in(&mut ws).await?;
    if !logged_in {
        return Err(login_err());
    }
    // Google OAuth sessions can show ChatGPT's remembered-account modal even though the
    // `/api/auth/session` check is already authenticated. Select the remembered account so the
    // composer is usable instead of silently waiting on the modal.
    dismiss_account_picker(&mut ws).await?;
    // The auth endpoint can be ahead of the SPA shell (especially with Google OAuth cookies).
    // A visible login button means the composer is a logged-out shell; reload once so tool
    // requests do not get silently accepted and then hang forever waiting for a response.
    if !app_shell_authenticated(&mut ws).await? {
        cmd(
            &mut ws,
            "Page.navigate",
            json!({ "url": "https://chatgpt.com/" }),
        )
        .await?;
        wait_until(&mut ws, "document.readyState === 'complete'", 15).await?;
        // The next send performs the bounded composer lookup.
        dismiss_account_picker(&mut ws).await?;
    }
    if !app_shell_authenticated(&mut ws).await? {
        return Err(login_err());
    }
    // `dr|poll` belongs to whichever account ran the kickoff. The HTTP path in chatgpt_web.rs
    // mints a fresh bearer on every poll and chokes on the rolling-NextAuth rotation that the
    // browser session tolerates; stay in the browser for the whole deep-research lifecycle.
    if let Some(cid) = dr_poll_cid.as_deref() {
        let conv = get_conversation(&mut ws, cid).await?;
        if let Some((report, sources)) = chatgpt_web::dr_report(&conv) {
            return Ok(Outcome::new(
                format!("{report}\n\nSources:\n{}", sources.join("\n")),
                0,
            ));
        }
        let mut out = Outcome::new(
            "Deep research still running. Call deep_research again with this session to fetch the report when ready.".into(),
            0,
        );
        out.session = Some(format!("dr|poll|{cid}"));
        return Ok(out);
    }
    // Image generation resumed after the kickoff handed back its poll session: only fetch the
    // result, never re-submit the prompt. The image source list lives in the already-connected
    // main page; no second page is opened (headful Chrome's page socket can lack the Runtime
    // domain on a freshly attached about:blank target, which wedged the hosted image polls).
    if let Some((cid, poll_query)) = img_poll {
        return fetch_polled_image(&mut ws, cid, &poll_query, 0, IMAGE_KICKOFF_WAIT).await;
    }
    if resume.is_some() {
        // The first target navigation can still be redirected while ChatGPT establishes the
        // authenticated app shell. Retry only after a redirect: reloading an already-hydrating
        // image conversation can wedge Chromium's renderer on small hosted servers.
        let cid = url
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .trim_start_matches("WEB:");
        let expected = serde_json::to_string(cid).unwrap_or_else(|_| "\"\"".into());
        let on_target = eval(
            &mut ws,
            &format!("location.href.includes('/c/') && location.href.includes({expected})"),
        )
        .await?
        .as_bool()
        .unwrap_or(false);
        if !on_target {
            cmd(&mut ws, "Page.navigate", json!({ "url": url })).await?;
            let web_url = format!("https://chatgpt.com/c/WEB:{cid}");
            if !wait_until(
                &mut ws,
                &format!("location.href.includes('/c/') && location.href.includes({expected})"),
                45,
            )
            .await?
            {
                cmd(&mut ws, "Page.navigate", json!({ "url": web_url })).await?;
                if !wait_until(
                    &mut ws,
                    &format!("location.href.includes('/c/') && location.href.includes({expected})"),
                    45,
                )
                .await?
                {
                    return Err(Error::BadResponse(
                        "chatgpt_web: conversation navigation redirected",
                    ));
                }
            }
        }
    }
    install_cid_probe(&mut ws).await?;
    sleep(Duration::from_millis(800)).await;

    // Approve a parked deep-research plan: click its Start button, then hand back a poll session.
    if let Some(cid) = approve.then_some(dr_plan_cid.as_deref()).flatten() {
        return start_dr_plan(&mut ws, cid).await;
    }

    // Continuing a conversation: note the newest existing answer and image sources so we wait
    // for the *new* reply/image instead of returning an older generated asset.
    let (baseline, baseline_image_sources) = match resume.as_deref() {
        Some(c) => {
            let conv = get_conversation(&mut ws, c).await?;
            (
                chatgpt_web::last_assistant_time(&conv),
                conversation_image_sources(&conv),
            )
        }
        None => (f64::NEG_INFINITY, Vec::new()),
    };

    // Snapshot every CID source used after submission. Using only sidebar links here lets an old
    // probe/performance CID appear "new" later and match a repeated prompt.
    let known_cids = if resume.is_none() {
        if matches!(cap, Capability::Image) {
            recent_conversation_cids(&mut ws, &[]).await?
        } else {
            candidate_cids(&mut ws, &[]).await?
        }
    } else {
        Vec::new()
    };

    // Image generation uses its own model regardless of the picker, so only chat/research honor it.
    if !matches!(cap, Capability::Image) {
        if let Some(m) = model {
            select_model(&mut ws, m).await?;
        }
    }
    if matches!(cap, Capability::Image) {
        if resume.is_some() {
            edit_existing_image(&mut ws).await?;
            tracing::debug!("chatgpt image edit target enabled");
        }
    } else if matches!(cap, Capability::DeepResearch) {
        enable_tool(&mut ws, "deep research").await?;
    } else if search {
        enable_tool(&mut ws, "web search").await?;
    }

    // The Edit image modal already carries the selected generated asset and its transformation
    // metadata. Re-uploading the same file targets the background composer and stalls submission.
    if !(files.is_empty() || matches!(cap, Capability::Image) && resume.is_some()) {
        attach_file(&mut ws, files).await?;
    }
    if matches!(cap, Capability::Image) {
        clear_cdp_image_responses();
    }
    // Keep the pre-send assistant count so a follow-up cannot be satisfied by the previous
    // turn's rendered node while the new response is still streaming.
    let assistant_count = if matches!(cap, Capability::Image) {
        0
    } else {
        eval(
            &mut ws,
            "document.querySelectorAll('[data-message-author-role=\"assistant\"]').length",
        )
        .await
        .ok()
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
    };
    let image_sources = baseline_image_sources;
    let fresh_image_prompt = (matches!(cap, Capability::Image) && resume.is_none())
        .then(|| format!("Generate an image from this request:\n{query}"));
    let submitted_query = fresh_image_prompt.as_deref().unwrap_or(query);
    let sent = send_prompt(&mut ws, submitted_query, matches!(cap, Capability::Image)).await?;
    tracing::debug!(sent = %sent, "chatgpt browser prompt submitted");
    if sent != "sent" {
        return Err(Error::BadResponse("chatgpt_web: composer drive failed"));
    }
    // Image submission can navigate immediately and then wedge its renderer. Its fresh verifier
    // below is the acceptance signal; do not put a diagnostic Runtime call back on the hot path.
    if !matches!(cap, Capability::Image) {
        if let Some(message) = browser_rate_limit(&mut ws).await? {
            return Err(Error::rate_limit(message));
        }
    }

    // A fresh ChatGPT turn can render before the SPA exposes its conversation id. Image generation
    // can keep the shell at `/`, so its CID and asset are resolved together below.
    let cid = match resume.as_deref() {
        Some(c) => c.to_string(),
        None if matches!(cap, Capability::DeepResearch) => {
            wait_for_cid(&mut ws, 60, &known_cids).await?
        }
        None => String::new(),
    };

    // Deep research: ChatGPT drafts a plan in an embedded widget and waits (it only auto-starts in a
    // focused tab). Read the plan and hand it back parked so the agent can approve or revise it —
    // the render, and the whole run, happen over the HTTP poll the router resumes on `dr|poll|`.
    if matches!(cap, Capability::DeepResearch) {
        if let Some(plan) = await_dr_plan(&mut ws).await? {
            let mut out = Outcome::new(
                format!(
                    "ChatGPT drafted a deep-research plan:\n\n{}\n\nReply with this session + query \
                     \"start\" to run it, or send a revised research request to replace the plan.",
                    plan.text
                ),
                1,
            );
            out.session = Some(format!("dr|plan|{cid}"));
            return Ok(out);
        }
        // No plan surfaced (a narrow query can start researching directly) — poll for the report.
        let mut out = Outcome::new(
            "Deep research started. Call deep_research again with this session to fetch the report \
             when ready (~5-30 min)."
                .into(),
            1,
        );
        out.session = Some(format!("dr|poll|{cid}"));
        return Ok(out);
    }

    // Create image: poll for the generated image on the same page WS after submission. The
    // inline flow already rides `ws`, and using a single socket dodges the fresh-target Runtime
    // gap headful Chrome hits when a second about:blank page is created mid-stream.
    if matches!(cap, Capability::Image) {
        tracing::debug!("chatgpt waiting for generated image");
        let (image_cid, src) = wait_for_image(
            &mut ws,
            IMAGE_KICKOFF_WAIT,
            (!cid.is_empty()).then_some(cid.as_str()),
            &known_cids,
            submitted_query,
            &image_sources,
        )
        .await?;
        tracing::debug!("chatgpt generated image found");
        if src.is_empty() {
            // Submitted and accepted, but the render outlived this request: park the fetch on an
            // `img|poll|` session instead of erroring so the router never re-submits the prompt
            // (failover to a second account duplicated generations before this contract).
            return fetch_polled_image(&mut ws, image_cid, submitted_query, 1, 0).await;
        }
        let mut out = Outcome::new(String::new(), 1);
        out.image = Some(fetch_image(&mut ws, &src).await?);
        out.session = Some(image_cid);
        return Ok(out);
    }

    // Chat / web search: the turn finishes in seconds — wait, then read the clean message back.
    // The response can be read from the rendered assistant node even when the conversation GET is
    // briefly unavailable while ChatGPT is still persisting the turn.
    // Current ChatGPT renders each turn as a `[data-message-author-role]` node. Keep the
    // count baseline so a follow-up cannot return an older assistant node.
    let rendered = format!(
        r#"(()=>{{const a=[...document.querySelectorAll('[data-message-author-role="assistant"]')].slice({assistant_count}).pop();return a&&a.innerText?a.innerText:null;}})()"#
    );
    let deadline = Instant::now() + Duration::from_secs(CHAT_WAIT);
    let mut last_rendered = String::new();
    let mut rendered_stable = 0_u8;
    let mut polls = 0_u8;
    loop {
        if polls == 0 {
            if let Some(message) = browser_rate_limit(&mut ws).await? {
                return Err(Error::rate_limit(message));
            }
        }
        polls = (polls + 1) % 5;
        // The rendered UI can finish before the persistence endpoint does. Read it first; this
        // also avoids waiting behind a stalled service-worker fetch.
        // ChatGPT can keep a visible stop control mounted while no assistant node exists yet.
        // Once a fresh assistant node has text, it is the authoritative completion signal.
        let streaming = eval(
            &mut ws,
            r#"(()=>[...document.querySelectorAll('[data-testid*=stop],button[aria-label*=Stop]')].some(e=>{const r=e.getBoundingClientRect();return r.width>0&&r.height>0&&!e.disabled&&getComputedStyle(e).visibility!=='hidden'&&getComputedStyle(e).display!=='none'}))()"#,
        )
        .await
        .ok()
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
        // The renderer can ignore Runtime.evaluate briefly while the submitted turn starts.
        // A missed poll is not a failed turn; the bounded outer deadline remains authoritative.
        if let Some(text) = eval(&mut ws, &rendered)
            .await
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
        {
            let text = text.trim();
            if !text.is_empty() {
                if text == last_rendered {
                    rendered_stable = rendered_stable.saturating_add(1);
                } else {
                    last_rendered.clear();
                    last_rendered.push_str(text);
                    rendered_stable = 0;
                }
            }
            // New ChatGPT streams can hand off to an SSE/WebSocket while leaving a stale
            // Stop button mounted. A fresh assistant node with stable text is authoritative;
            // requiring two identical polls avoids returning a partial token stream.
            if !text.is_empty() && (rendered_stable >= 2 || (!streaming && assistant_count > 0)) {
                let mut out = Outcome::new(text.to_string(), 1);
                let session = if cid.is_empty() {
                    wait_for_cid(&mut ws, 5, &known_cids).await.ok()
                } else {
                    Some(answer_session(&mut ws, &cid).await)
                };
                if let Some(session) = session {
                    out.session = Some(session);
                }
                return Ok(out);
            }
        }
        let page_cid = if cid.is_empty() {
            eval(
                &mut ws,
                r#"location.href.match(/\/c\/(?:WEB:)?([0-9a-f-]{36})/i)?.[1] || null"#,
            )
            .await
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
        } else {
            Some(cid.clone())
        };
        if let Some(page_cid) = page_cid {
            if let Ok(conv) = get_conversation(&mut ws, &page_cid).await {
                if let Ok(mut out) = chatgpt_web::extract_answer_after(&conv, baseline) {
                    out.session = Some(chatgpt_web::session_token(&conv, &page_cid));
                    return Ok(out);
                }
            }
        }
        if Instant::now() >= deadline {
            let diag = eval(
                &mut ws,
                r#"JSON.stringify({url:location.href,title:document.title,body:(document.body?.innerText||'').slice(-1200),assistantCount:document.querySelectorAll('[data-message-author-role="assistant"]').length,composerCount:document.querySelectorAll('#prompt-textarea,[contenteditable="true"][role="textbox"]').length})"#,
            )
            .await
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "{}".into());
            tracing::warn!(diagnostics = %diag, "chatgpt browser response did not render before timeout");
            return Err(Error::Timeout("chatgpt_web: no answer"));
        }
        sleep(Duration::from_secs(2)).await;
    }
}

async fn edit_existing_image(ws: &mut Ws) -> Result<()> {
    // Image turns are virtualized. A bounded hover/click through the AX tree keeps this
    // responsive on a VPS; the old 60 x 30s Runtime.evaluate loop could hang for 30 minutes.
    // Keep each DOM call short. If the renderer is busy, retry the browser-process AX query
    // instead of stacking more Runtime.evaluate requests behind it.
    for _ in 0..3 {
        // Conversation pages are virtualized; scrolling is needed before the image toolbar can
        // mount, but it must remain bounded while the page hydrates.
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            eval(ws, "window.scrollTo(0, document.body.scrollHeight)"),
        )
        .await;
        let image =
            match tokio::time::timeout(Duration::from_secs(2), generated_image_point(ws)).await {
                Ok(Ok(point)) => point,
                Ok(Err(Error::Timeout(_))) | Err(_) => None,
                Ok(Err(error)) => return Err(error),
            };
        tracing::debug!(found = image.is_some(), "chatgpt image edit target");
        if let Some((x, y)) = image {
            let _ = hover_at(ws, x, y).await;
            sleep(Duration::from_millis(350)).await;
        }
        // Prefer a real coordinate click scoped to the newest generated-image turn. Full-page AX
        // and text fallbacks can stall the renderer or select an older image in the conversation.
        let edit = match tokio::time::timeout(
            Duration::from_secs(2),
            center_of(
                ws,
                r#"(()=>{const turns=[...document.querySelectorAll('[data-message-author-role="assistant"]')]
                    .filter(t=>t.querySelector('img[alt^="Generated image"],button[aria-label^="Generated image"]'));
                    return turns.at(-1)?.querySelector('button[aria-label="Edit image"]')||null})()"#,
            ),
        )
        .await
        {
            Ok(Ok(point)) => point,
            Ok(Err(Error::Timeout(_))) | Err(_) => None,
            Ok(Err(error)) => return Err(error),
        };
        if let Some((x, y)) = edit {
            click_at(ws, x, y).await?;
            sleep(Duration::from_millis(700)).await;
            return Ok(());
        }
        // Some ChatGPT builds reveal the toolbar only after a real image click.
        if let Some((x, y)) = image {
            let _ = click_at(ws, x, y).await;
            sleep(Duration::from_millis(400)).await;
        }
        sleep(Duration::from_secs(1)).await;
    }
    // ChatGPT can virtualize every previous message while keeping the authenticated composer.
    // In that shell the edit button does not exist; a same-conversation instruction still carries
    // the generated image context and is preferable to failing before submission.
    tracing::debug!("chatgpt image edit modal unavailable; using conversation follow-up");
    Ok(())
}

/// Return the newest visible generated image, excluding user uploads in the same conversation.
async fn generated_image_point(ws: &mut Ws) -> Result<Option<(f64, f64)>> {
    center_of(
        ws,
        r#"(()=>{const turns=[...document.querySelectorAll('[data-message-author-role="assistant"]')]
            .filter(t=>t.querySelector('img[alt^="Generated image"],button[aria-label^="Generated image"]'));
            const nodes=[...(turns.at(-1)?.querySelectorAll('img[alt^="Generated image"],button[aria-label^="Generated image"]')||[])]
                .filter(e=>{const r=e.getBoundingClientRect();return r.width>0&&r.height>0});
            const image=nodes.at(-1);const target=image?.closest('button')||image;
            target?.scrollIntoView({block:'center'});return target})()"#,
    )
    .await
}

async fn answer_session(ws: &mut Ws, cid: &str) -> String {
    // The rendered turn can finish before ChatGPT persists its message node. Keep polling long
    // enough to obtain the real parent id; returning `client-created-root` makes the next turn
    // lose conversation context.
    for _ in 0..30 {
        if let Ok(conv) = get_conversation(ws, cid).await {
            let token = chatgpt_web::session_token(&conv, cid);
            if !token.ends_with("|client-created-root") {
                return token;
            }
        }
        sleep(Duration::from_secs(1)).await;
    }
    cid.to_string()
}

fn cdp_cookies(cookies: &[Cookie]) -> Vec<Value> {
    cookies
        .iter()
        .map(|c| {
            // `__Host-` cookies are host-only by definition: Chrome rejects a CDP cookie
            // carrying Domain (even when the captured cookie's host is chatgpt.com).
            let mut v = json!({
                "name": c.name, "value": c.value,
                "path": c.path, "secure": c.secure, "httpOnly": c.http_only,
            });
            if !c.name.starts_with("__Host-") {
                v["domain"] = json!(c.domain);
            } else {
                // CDP needs a URL when Domain is omitted; otherwise a host-only cookie is
                // rejected against the initial about:blank target and auth silently disappears.
                v["url"] = json!("https://chatgpt.com/");
            }
            if c.expires > 0.0 {
                v["expires"] = json!(c.expires);
            }
            v
        })
        .collect()
}

/// Attach local files to the composer over CDP (`DOM.setFileInputFiles` on the hidden file input),
/// then wait for the upload to settle — ChatGPT rejects a send while an attachment is still uploading.
async fn attach_file(ws: &mut Ws, files: &[PathBuf]) -> Result<()> {
    let abs: Vec<String> = files
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    cmd(ws, "DOM.enable", json!({})).await?;
    let doc = cmd(ws, "DOM.getDocument", json!({"depth": -1, "pierce": true})).await?;
    let root = doc
        .pointer("/root/nodeId")
        .and_then(|x| x.as_i64())
        .ok_or(Error::BadResponse("chatgpt_web: no document"))?;
    let q = cmd(
        ws,
        "DOM.querySelector",
        json!({"nodeId": root, "selector": "input[type=file]"}),
    )
    .await?;
    let node = q.get("nodeId").and_then(|x| x.as_i64()).unwrap_or(0);
    if node == 0 {
        return Err(Error::BadResponse("chatgpt_web: no file input in composer"));
    }
    cmd(
        ws,
        "DOM.setFileInputFiles",
        json!({"files": abs, "nodeId": node}),
    )
    .await?;
    // The send button stays disabled until the attachment finishes uploading; since the prompt text
    // is typed afterwards, an enabled send button is a clean "upload done" signal.
    let ready = r#"(()=>{const b=document.querySelector('[data-testid="send-button"]');return !!b&&!b.disabled;})()"#;
    if wait_until(ws, ready, 40).await? {
        Ok(())
    } else {
        Err(Error::Timeout("chatgpt_web: file upload did not complete"))
    }
}

async fn send_prompt(ws: &mut Ws, query: &str, image: bool) -> Result<String> {
    if !focus_editor(ws).await? {
        return Ok("no-editor".into());
    }
    cmd(ws, "Input.insertText", json!({"text": query})).await?;
    sleep(Duration::from_millis(if image { 800 } else { 500 })).await;
    for (kind, key) in [("rawKeyDown", "Enter"), ("keyUp", "Enter")] {
        cmd(
            ws,
            "Input.dispatchKeyEvent",
            json!({
                "type": kind,
                "key": key,
                "code": "Enter",
                "windowsVirtualKeyCode": 13,
                "nativeVirtualKeyCode": 13,
            }),
        )
        .await?;
    }
    Ok("sent".into())
}

fn composer_backend_node(tree: &Value) -> Option<i64> {
    tree.get("nodes")?.as_array()?.iter().find_map(|node| {
        let role = node
            .get("role")
            .and_then(|v| v.get("value"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let name = node
            .get("name")
            .and_then(|v| v.get("value"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        (role.eq_ignore_ascii_case("textbox")
            && name.to_ascii_lowercase().contains("chat with chatgpt"))
        .then(|| node.get("backendDOMNodeId").and_then(Value::as_i64))
        .flatten()
    })
}

async fn focus_editor(ws: &mut Ws) -> Result<bool> {
    let selectors = [
        "[role='dialog'] [contenteditable='true'][role='textbox']",
        "#prompt-textarea",
        "textarea[placeholder*='Chat with ChatGPT']",
        "[contenteditable='true'][role='textbox']",
    ];
    for selector in selectors {
        let selector =
            serde_json::to_string(selector).unwrap_or_else(|_| "\"#prompt-textarea\"".into());
        let expr = format!("(()=>{{const e=[...document.querySelectorAll({selector})].find(e=>{{const r=e.getBoundingClientRect();return r.width>0&&r.height>0&&!e.disabled}});if(!e)return false;e.focus();return true;}})()");
        match tokio::time::timeout(Duration::from_secs(3), eval(ws, &expr)).await {
            Ok(Ok(value)) if value.as_bool() == Some(true) => return Ok(true),
            Ok(Err(Error::Timeout(_))) | Err(_) | Ok(Ok(_)) => {}
            Ok(Err(error)) => return Err(error),
        }
    }

    // Hosted Chromium can expose the composer in its accessibility tree while page-side DOM
    // queries miss the React/shadow-root node. Focus the same backend node Chrome exposes to AX.
    let tree = match tokio::time::timeout(
        Duration::from_secs(3),
        cmd(
            ws,
            "Accessibility.getFullAXTree",
            json!({"interestingOnly": false}),
        ),
    )
    .await
    {
        Ok(Ok(tree)) => tree,
        Ok(Err(Error::Timeout(_))) | Err(_) => return Ok(false),
        Ok(Err(error)) => return Err(error),
    };
    let Some(backend) = composer_backend_node(&tree) else {
        return Ok(false);
    };
    match tokio::time::timeout(
        Duration::from_secs(3),
        cmd(ws, "DOM.focus", json!({"backendNodeId": backend})),
    )
    .await
    {
        Ok(Ok(_)) => Ok(true),
        Ok(Err(Error::Timeout(_))) | Err(_) => Ok(false),
        Ok(Err(error)) => Err(error),
    }
}

/// Open the composer "+" menu and click the tool whose label matches (e.g. "deep research"). React
/// portals ignore synthetic `.click()`, so we dispatch real CDP mouse events at element centers.
async fn enable_tool(ws: &mut Ws, tool: &str) -> Result<()> {
    let names: &[&str] = match tool {
        "web search" => &["Web search", "Search the web"],
        "deep research" => &["Deep research"],
        _ => return Err(Error::BadResponse("chatgpt_web: requested tool not found")),
    };
    let mut menu_found = false;
    for _ in 0..3 {
        let Some((x, y)) =
            ax_exact_point_retry(ws, &["Add files and more", "Add photos and files"], 2).await?
        else {
            continue;
        };
        menu_found = true;
        click_at(ws, x, y).await?;
        sleep(Duration::from_millis(700)).await;
        if let Some((x, y)) = ax_exact_point_retry(ws, names, 2).await? {
            click_at(ws, x, y).await?;
            sleep(Duration::from_millis(900)).await;
            return Ok(());
        }
    }
    Err(Error::BadResponse(if menu_found {
        "chatgpt_web: requested tool not available"
    } else {
        "chatgpt_web: tool menu button not found"
    }))
}

fn ax_text<'a>(node: &'a Value, key: &str) -> &'a str {
    node.get(key)
        .and_then(|value| value.get("value"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn ax_focusable(node: &Value) -> bool {
    node.get("properties")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|property| {
            property.get("name").and_then(Value::as_str) == Some("focusable")
                && property.pointer("/value/value").and_then(Value::as_bool) == Some(true)
        })
}

fn ax_parent<'a>(nodes: &'a [Value], node: &Value) -> Option<&'a Value> {
    let id = node.get("parentId")?.as_str()?;
    nodes
        .iter()
        .find(|parent| parent.get("nodeId").and_then(Value::as_str) == Some(id))
}

fn ax_exact_backend(tree: &Value, names: &[&str]) -> Option<i64> {
    let nodes = tree.get("nodes")?.as_array()?;
    for node in nodes {
        if node.get("ignored").and_then(Value::as_bool) == Some(true)
            || !names
                .iter()
                .any(|name| ax_text(node, "name").eq_ignore_ascii_case(name))
        {
            continue;
        }
        let mut hit = node;
        let mut current = node;
        let mut history = false;
        for _ in 0..12 {
            let role = ax_text(current, "role");
            let name = ax_text(current, "name");
            if matches!(role, "navigation" | "list" | "listitem" | "link")
                || ["Chat history", "Sidebar"]
                    .iter()
                    .any(|blocked| name.eq_ignore_ascii_case(blocked))
            {
                history = true;
                break;
            }
            if ax_focusable(current) {
                hit = current;
                break;
            }
            let Some(parent) = ax_parent(nodes, current) else {
                break;
            };
            current = parent;
        }
        if !history {
            if let Some(backend) = hit
                .get("backendDOMNodeId")
                .and_then(Value::as_i64)
                .or_else(|| node.get("backendDOMNodeId").and_then(Value::as_i64))
            {
                return Some(backend);
            }
        }
    }
    None
}

async fn ax_exact_point(ws: &mut Ws, names: &[&str]) -> Result<Option<(f64, f64)>> {
    let tree = cmd(
        ws,
        "Accessibility.getFullAXTree",
        json!({"interestingOnly": false}),
    )
    .await?;
    let Some(backend) = ax_exact_backend(&tree, names) else {
        return Ok(None);
    };
    let model = cmd(ws, "DOM.getBoxModel", json!({"backendNodeId": backend})).await?;
    let Some(quad) = model.pointer("/model/content").and_then(Value::as_array) else {
        return Ok(None);
    };
    let numbers: Vec<f64> = quad.iter().filter_map(Value::as_f64).collect();
    if numbers.len() < 8 {
        return Ok(None);
    }
    let x = (numbers[0] + numbers[2] + numbers[4] + numbers[6]) / 4.0;
    let y = (numbers[1] + numbers[3] + numbers[5] + numbers[7]) / 4.0;
    Ok(Some((x, y)))
}

async fn ax_exact_point_retry(
    ws: &mut Ws,
    names: &[&str],
    attempts: usize,
) -> Result<Option<(f64, f64)>> {
    for _ in 0..attempts {
        match ax_exact_point(ws, names).await {
            Ok(Some(point)) => return Ok(Some(point)),
            Ok(None) | Err(Error::Timeout(_)) => sleep(Duration::from_millis(250)).await,
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}

/// Center (viewport coords) of the element returned by `find`, or `None` if absent / zero-sized.
async fn center_of(ws: &mut Ws, find: &str) -> Result<Option<(f64, f64)>> {
    let js = format!(
        "(()=>{{const e={find};if(!e)return null;const r=e.getBoundingClientRect();\
         return (r.width>0&&r.height>0)?[r.left+r.width/2,r.top+r.height/2]:null;}})()"
    );
    let value = match eval(ws, &js).await {
        Ok(value) => value,
        // ChatGPT replaces the document while hydrating. A stale DOM context is a miss, not a
        // request failure; the caller will retry against the fresh page.
        Err(Error::BadResponse("cdp error")) => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(value
        .as_array()
        .filter(|a| a.len() == 2)
        .map(|a| (a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0))))
}

async fn click_at(ws: &mut Ws, x: f64, y: f64) -> Result<()> {
    let down =
        json!({"type":"mousePressed","x":x,"y":y,"button":"left","buttons":1,"clickCount":1});
    let up = json!({"type":"mouseReleased","x":x,"y":y,"button":"left","buttons":0,"clickCount":1});
    cmd(ws, "Input.dispatchMouseEvent", down).await?;
    sleep(Duration::from_millis(80)).await;
    cmd(ws, "Input.dispatchMouseEvent", up).await?;
    Ok(())
}

// Submenus (the model-version list) open on hover, so a real pointer move is needed to reveal them.
async fn hover_at(ws: &mut Ws, x: f64, y: f64) -> Result<()> {
    cmd(
        ws,
        "Input.dispatchMouseEvent",
        json!({"type":"mouseMoved","x":x,"y":y}),
    )
    .await?;
    Ok(())
}

/// Match key for a model/level name: lowercase, alphanumerics only ("GPT-5.4" -> "gpt54").
fn norm(s: &str) -> String {
    s.to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

struct PickItem {
    label: String,
    x: f64,
    y: f64,
    radio: bool,
}

/// Read the open model picker: the intelligence radios plus the model-version submenu trigger
/// (and, once that submenu is open, the version radios). Nothing is hardcoded — the menu is the
/// source of truth, so renamed/added models work without a code change.
async fn scan_picker(ws: &mut Ws) -> Result<Vec<PickItem>> {
    let js = r#"(()=>{const out=[];const seen=new Set();
        for(const e of document.querySelectorAll('[role="menuitemradio"],[role="menuitem"][aria-haspopup="menu"]')){
            const r=e.getBoundingClientRect();if(r.width<=0||r.height<=0)continue;
            const t=e.querySelector('.truncate');const label=((t?t.textContent:e.textContent)||'').trim();if(!label)continue;
            const radio=e.getAttribute('role')==='menuitemradio';
            const key=label+radio;if(seen.has(key))continue;seen.add(key);
            out.push({label,x:r.left+r.width/2,y:r.top+r.height/2,radio});
        }return out;})()"#;
    Ok(eval(ws, js)
        .await?
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|i| {
                    Some(PickItem {
                        label: i.get("label")?.as_str()?.to_string(),
                        x: i.get("x")?.as_f64()?,
                        y: i.get("y")?.as_f64()?,
                        radio: i.get("radio")?.as_bool()?,
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

async fn open_picker(ws: &mut Ws) -> Result<()> {
    let pill = r#"document.querySelector('button.__composer-pill[aria-haspopup="menu"]')"#;
    if let Some((x, y)) = center_of(ws, pill).await? {
        click_at(ws, x, y).await?;
        sleep(Duration::from_millis(700)).await;
    }
    Ok(())
}

/// Apply a model/level selection. The picker has two axes: a model (GPT-5.5/o3/...) and that
/// model's own thinking levels — which vary per model (GPT-5.5 has Instant/Medium/High, o3 only
/// Medium). `want` may name a model, a level, or both ("gpt-5.4 high"). The model is selected first
/// because it determines which levels exist; names are read live, so an unknown one errors with the
/// options actually offered.
async fn select_model(ws: &mut Ws, want: &str) -> Result<()> {
    let tokens: Vec<String> = want
        .split([' ', ',', '/'])
        .map(norm)
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return Ok(());
    }

    open_picker(ws).await?;
    let main = scan_picker(ws).await?;
    let cur_model = main
        .iter()
        .find(|i| !i.radio)
        .map(|i| i.label.clone())
        .unwrap_or_default();
    let level_labels: Vec<String> = main
        .iter()
        .filter(|i| i.radio)
        .map(|i| i.label.clone())
        .collect();

    // The model list lives in the submenu; open it and take the radios that aren't already levels.
    let models: Vec<PickItem> = match main.iter().find(|i| !i.radio) {
        Some(sub) => {
            hover_at(ws, sub.x, sub.y).await?;
            sleep(Duration::from_millis(500)).await;
            scan_picker(ws)
                .await?
                .into_iter()
                .filter(|i| i.radio && !level_labels.iter().any(|l| norm(l) == norm(&i.label)))
                .collect()
        }
        None => Vec::new(),
    };

    let model_tok = tokens
        .iter()
        .find(|t| models.iter().any(|m| norm(&m.label) == **t))
        .cloned();
    let level_tok = tokens
        .iter()
        .find(|t| Some(t.as_str()) != model_tok.as_deref())
        .cloned();

    // Select the model first — it closes the picker and changes which levels are offered.
    let mut model_name = cur_model.clone();
    if let Some(mt) = &model_tok {
        if let Some(m) = models.iter().find(|m| norm(&m.label) == *mt) {
            model_name = m.label.clone();
            click_at(ws, m.x, m.y).await?;
            sleep(Duration::from_millis(500)).await;
        }
        if level_tok.is_some() {
            open_picker(ws).await?;
        }
    }

    let Some(lt) = &level_tok else {
        return Ok(()); // model-only (or nothing) selection is done
    };

    // Select the level for the now-current model.
    let cur = scan_picker(ws).await?;
    let lvls: Vec<&PickItem> = cur
        .iter()
        .filter(|i| i.radio && !models.iter().any(|m| norm(&m.label) == norm(&i.label)))
        .collect();
    if let Some(it) = lvls.iter().find(|i| norm(&i.label) == *lt) {
        click_at(ws, it.x, it.y).await?;
        sleep(Duration::from_millis(400)).await;
        return Ok(());
    }
    let offered: Vec<&str> = lvls.iter().map(|i| i.label.as_str()).collect();
    let models: Vec<&str> = models.iter().map(|m| m.label.as_str()).collect();
    if model_tok.is_some() {
        // A real level token that this model doesn't offer.
        Err(Error::Provider {
            provider: "chatgpt_web",
            status: 400,
            body: format!(
                "{model_name} has no level {lt:?}; it offers: {}",
                offered.join(", ")
            ),
        })
    } else {
        // The lone token matched neither a model nor the current model's levels.
        Err(Error::Provider {
            provider: "chatgpt_web",
            status: 400,
            body: format!(
                "{want:?} not recognized. models: [{}]; levels for {model_name}: [{}] \
                 (levels vary per model — pass e.g. \"gpt-5.4 high\" or \"o3\")",
                models.join(", "),
                offered.join(", ")
            ),
        })
    }
}

async fn get_conversation(ws: &mut Ws, cid: &str) -> Result<Value> {
    // ChatGPT now prefixes web conversation URLs with `WEB:` (/c/WEB:<uuid>); the backend-api
    // conversation endpoint wants the bare id and rejects the prefixed form ("Invalid conversation").
    let cid = normalize_conversation_cid(cid)?;
    let url = serde_json::to_string(&format!(
        "/backend-api/conversation/{cid}?include_visually_hidden_messages=true"
    ))?;
    let js = format!(
        r#"(async()=>{{
            const u={url};
            const signal=AbortSignal.timeout(8000);
            // The browser UI is cookie-authenticated. `/api/auth/session` may rotate or omit
            // accessToken on Google OAuth sessions, so prefer the same cookie request the UI uses.
            let r=await fetch(u,{{credentials:'include',signal}});
            if(!r.ok){{
                const v=await fetch('/api/auth/session',{{credentials:'include',signal}}).then(x=>x.json()).catch(()=>({{}}));
                if(v.accessToken) r=await fetch(u,{{credentials:'include',signal,headers:{{Authorization:'Bearer '+v.accessToken}}}});
            }}
            return await r.text();
        }})()"#
    );
    let text = eval(ws, &js).await?;
    let s = text.as_str().unwrap_or("");
    serde_json::from_str(s).map_err(|_| Error::BadResponse("chatgpt_web"))
}

async fn is_logged_in(ws: &mut Ws) -> Result<bool> {
    // A broken/stalled auth service must become a provider error, not hold the CDP socket
    // forever. Runtime.evaluate has its own transport timeout, but an explicit browser-side
    // abort gives ChatGPT's page a chance to unwind the promise cleanly.
    let js = r#"(async()=>{try{const r=await fetch('/api/auth/session',{credentials:'include',signal:AbortSignal.timeout(8000)});const v=await r.json();return !!(v&&v.user);}catch(_){return false;}})()"#;
    Ok(eval(ws, js).await?.as_bool().unwrap_or(false))
}

async fn app_shell_authenticated(ws: &mut Ws) -> Result<bool> {
    let js = r#"(()=>{const visible=e=>{const r=e.getBoundingClientRect();return r.width>0&&r.height>0};return ![...document.querySelectorAll('[data-testid="login-button"],[data-testid="signup-button"]')].some(visible)})()"#;
    Ok(eval(ws, js).await?.as_bool().unwrap_or(false))
}

async fn browser_rate_limit(ws: &mut Ws) -> Result<Option<String>> {
    let js = r#"(()=>{const t=(document.body?.innerText||'').toLowerCase();for(const p of ['making requests too quickly','temporarily limited access to your conversations','you\'ve made too many requests','too many requests','please wait a few minutes before trying again','unusual activity'])if(t.includes(p))return p;return null})()"#;
    match eval(ws, js).await {
        Ok(value) => Ok(value
            .as_str()
            .map(|phrase| format!("chatgpt_web: {phrase}; wait before retrying"))),
        // ChatGPT's renderer can be busy during image submission. A diagnostic probe must not
        // abort the request that is already in flight.
        Err(Error::Timeout(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn dismiss_account_picker(ws: &mut Ws) -> Result<()> {
    let selector = r#"(()=>{const root=[...document.querySelectorAll('[role=dialog],body')].find(e=>/Welcome back|Choose an account/i.test(e.innerText||''));if(!root)return null;const e=[...root.querySelectorAll('*')].find(e=>/^[^\n@]+@[^\n@]+$/.test((e.innerText||'').trim()));if(!e)return null;const b=e.closest('button,a,[role=button]')||e.parentElement;const r=b?.getBoundingClientRect();return r&&r.width&&r.height?[r.left+r.width/2,r.top+r.height/2]:null;})()"#;
    if let Some((x, y)) = center_of(ws, selector).await? {
        click_at(ws, x, y).await?;
        sleep(Duration::from_millis(800)).await;
    }
    Ok(())
}

/// Keep the conversation id from the streaming create request. New ChatGPT builds can leave the
/// address bar at `/` while the response is still open, so URL/DOM polling alone races the stream.
async fn install_cid_probe(ws: &mut Ws) -> Result<()> {
    let js = r#"(()=>{
        if(window.__fxCidProbe)return true;
        window.__fxCidProbe=true; window.__fxConversationIds=[];
        const path=/\/c\/(?:WEB:)?([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})/i;
        const uuid=/(?:conversation[_-]?id|conversationId)[^0-9a-f]{0,24}([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})/i;
        const save=s=>{const m=String(s||'').match(path)||String(s||'').match(uuid);if(m&&!window.__fxConversationIds.includes(m[1]))window.__fxConversationIds.push(m[1]);};
        const orig=window.fetch.bind(window);
        window.fetch=async(...a)=>{const r=await orig(...a);try{const u=typeof a[0]==='string'?a[0]:(a[0]&&a[0].url)||'';if(/\/conversation(?:\/|\?|$)/i.test(u)){save(u);const rd=r.clone().body?.getReader(),dec=new TextDecoder();(async()=>{if(!rd)return;for(;;){const x=await rd.read();if(x.done)break;save(dec.decode(x.value,{stream:true}));}})().catch(()=>{});}}catch(_){}return r;};
        const XO=XMLHttpRequest.prototype.open,XS=XMLHttpRequest.prototype.send;
        XMLHttpRequest.prototype.open=function(m,u,...r){this.__fxUrl=String(u);return XO.call(this,m,u,...r);};
        XMLHttpRequest.prototype.send=function(...a){this.addEventListener('readystatechange',()=>{if(this.readyState===4&&/\/conversation(?:\/|\?|$)/i.test(this.__fxUrl))save(this.__fxUrl+this.responseText);});return XS.apply(this,a);};
        save(location.href);
        return true;
    })()"#;
    eval(ws, js).await?;
    Ok(())
}

fn without_known_cids(candidates: Vec<String>, known: &[String]) -> Vec<String> {
    candidates
        .into_iter()
        .filter(|cid| !known.iter().any(|old| old == cid))
        .collect()
}

fn conversation_list_cids(list: &Value) -> Vec<String> {
    list.get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .filter_map(|cid| normalize_conversation_cid(cid).ok())
        .collect()
}

/// Read the authoritative recent-conversation list without touching the submitted page's DOM.
async fn recent_conversation_cids(ws: &mut Ws, known: &[String]) -> Result<Vec<String>> {
    let js = r#"(async()=>{
        const u='/backend-api/conversations?offset=0&limit=28&order=updated&is_archived=false';
        const signal=AbortSignal.timeout(8000);
        let r=await fetch(u,{credentials:'include',cache:'no-store',signal});
        if(!r.ok){
            const v=await fetch('/api/auth/session',{credentials:'include',signal}).then(x=>x.json()).catch(()=>({}));
            if(v.accessToken)r=await fetch(u,{credentials:'include',cache:'no-store',signal,headers:{Authorization:'Bearer '+v.accessToken}});
        }
        return await r.text();
    })()"#;
    let text = eval(ws, js).await?;
    let list: Value = serde_json::from_str(text.as_str().unwrap_or(""))
        .map_err(|_| Error::BadResponse("chatgpt_web: conversation list"))?;
    Ok(without_known_cids(conversation_list_cids(&list), known))
}

async fn candidate_cids(ws: &mut Ws, known: &[String]) -> Result<Vec<String>> {
    let js = r#"(()=>{
        const uuid=/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})/i;
        const path=/\/c\/(?:WEB:)?([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})/i;
        const out=[];
        const direct=[location.href,
            // The new thread is inserted into the sidebar as soon as the first turn is
            // accepted. This is the most reliable signal on builds that stream through a
            // service worker (which hides the response body from page-level fetch hooks).
            ...[...document.querySelectorAll('a[href*="/c/"]')].map(e=>e.getAttribute('href')||''),
            ...[...document.querySelectorAll('a[href]')].map(e=>e.getAttribute('href')||'').filter(u=>path.test(u)),
            ...[...document.querySelectorAll('[data-conversation-id]')].map(e=>e.getAttribute('data-conversation-id')||''),
            ...(window.__fxConversationIds||[]),
            ...performance.getEntriesByType('resource').map(e=>e.name).filter(u=>/\/(?:f\/)?conversation(?:\/|\?|$)/i.test(u)),
            ...performance.getEntriesByType('resource').map(e=>e.name).filter(u=>/\/backend-api\/conversation\//i.test(u))];
        for(const u of direct){const m=String(u||'').match(path)||String(u||'').match(uuid);if(m&&!out.includes(m[1]))out.push(m[1]);}
        return out;
    })()"#;
    let candidates = eval(ws, js)
        .await?
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok(without_known_cids(candidates, known))
}

async fn wait_for_cid(ws: &mut Ws, secs: u64, known: &[String]) -> Result<String> {
    for _ in 0..secs {
        if let Some(c) = candidate_cids(ws, known).await?.into_iter().next() {
            return Ok(c);
        }
        sleep(Duration::from_secs(1)).await;
    }
    Err(Error::Timeout("chatgpt_web: no conversation id"))
}

/// Return only an image persisted in the conversation that contains this submitted prompt.
/// DOM and network images are useful byte caches, but cannot prove turn ownership.
async fn wait_for_image(
    ws: &mut Ws,
    secs: u64,
    fixed_cid: Option<&str>,
    known_cids: &[String],
    query: &str,
    baseline_sources: &[String],
) -> Result<(String, String)> {
    let fixed_cid = fixed_cid.map(str::to_owned);
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut query_cid = None;
    while Instant::now() < deadline {
        let mut candidates = match &fixed_cid {
            Some(cid) => vec![cid.clone()],
            None => match timeout(
                Duration::from_secs(8),
                recent_conversation_cids(ws, known_cids),
            )
            .await
            {
                Ok(Ok(cids)) => cids,
                _ => Vec::new(),
            },
        };
        if fixed_cid.is_none() {
            if let Ok(Ok(extra)) =
                timeout(Duration::from_secs(5), candidate_cids(ws, known_cids)).await
            {
                for cid in extra {
                    if !candidates.iter().any(|old| old == &cid) {
                        candidates.push(cid);
                    }
                }
            }
        }
        for cid in candidates {
            let conv = match timeout(Duration::from_secs(10), get_conversation(ws, &cid)).await {
                Ok(Ok(conv)) => conv,
                _ => continue,
            };
            if conversation_query_time(&conv, query).is_some() {
                query_cid = Some(cid.clone());
            }
            let Some(source) =
                conversation_image_source_after_query(&conv, query, baseline_sources)
            else {
                continue;
            };
            if let Ok(Ok(Some(source))) = timeout(
                Duration::from_secs(10),
                resolve_conversation_image_source(ws, &source, &cid),
            )
            .await
            {
                return Ok((cid, source));
            }
        }
        // Image generation is asynchronous; poll often enough to catch the estuary PNG
        // before the kickoff budget ends (15s gaps missed a live asset at t≈77s).
        sleep(Duration::from_secs(4)).await;
    }
    // ChatGPT can accept the prompt and still take longer than one hosted HTTP budget to
    // persist the conversation. Hand back a poll session (empty cid if the thread is not
    // listed yet) so the router never failovers and duplicates the generation.
    if let Some(cid) = query_cid.or(fixed_cid.filter(|cid| !cid.is_empty())) {
        return Ok((cid, String::new()));
    }
    tracing::warn!("chatgpt image conversation not yet listed; parking on poll session");
    Ok((String::new(), String::new()))
}

/// Fetch the result of an image generation that outlived its kickoff request. Matches the stored
/// submitted prompt, so a same-conversation edit cannot bind the response to an older turn.
async fn fetch_polled_image(
    ws: &mut Ws,
    cid: String,
    query: &str,
    cost: i64,
    secs: u64,
) -> Result<Outcome> {
    let (image_cid, src) = wait_for_image(
        ws,
        secs,
        (!cid.is_empty()).then_some(cid.as_str()),
        &[],
        query,
        &[],
    )
    .await?;
    if src.is_empty() {
        let mut out = Outcome::new(
            "Image is still generating. Call create_image again with this session to fetch it when ready."
                .into(),
            cost,
        );
        let park = if image_cid.is_empty() { cid } else { image_cid };
        out.session = Some(format!("img|poll|{park}|{}", percent_encode(query)));
        return Ok(out);
    }
    tracing::debug!("chatgpt generated image found");
    let mut out = Outcome::new(String::new(), cost.max(1));
    out.image = Some(fetch_image(ws, &src).await?);
    out.session = Some(image_cid);
    Ok(out)
}

fn trusted_estuary_source(source: &str) -> bool {
    fn relative(source: &str) -> bool {
        source
            .strip_prefix("/backend-api/estuary/content")
            .is_some_and(|suffix| {
                suffix.is_empty() || suffix.starts_with('/') || suffix.starts_with('?')
            })
    }
    relative(source)
        || source
            .strip_prefix("https://chatgpt.com")
            .is_some_and(relative)
}

fn sediment_file(source: &str) -> Option<&str> {
    let file = source.strip_prefix("sediment://")?;
    (file.len() > "file_".len()
        && file.starts_with("file_")
        && file
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
    .then_some(file)
}

fn conversation_image_sources_after(conv: &Value, after: f64) -> Vec<String> {
    fn walk(v: &Value, image_gen: bool, create_time: f64, out: &mut Vec<(f64, String)>) {
        match v {
            Value::String(s) if trusted_estuary_source(s) => {
                out.push((create_time, s.clone()));
            }
            Value::String(s) if image_gen && sediment_file(s).is_some() => {
                out.push((create_time, s.clone()));
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, image_gen, create_time, out)),
            Value::Object(o) => o
                .values()
                .for_each(|x| walk(x, image_gen, create_time, out)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    let messages: Vec<&Value> = conv
        .get("mapping")
        .and_then(Value::as_object)
        .map(|mapping| {
            mapping
                .values()
                .filter_map(|node| node.get("message"))
                .filter(|message| !message.is_null())
                .collect()
        })
        .unwrap_or_default();
    if messages.is_empty() && after == f64::NEG_INFINITY {
        // Keep parsing older conversation shapes that do not expose a mapping.
        walk(conv, false, 0.0, &mut found);
    } else {
        for message in messages {
            let create_time = message
                .get("create_time")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            if create_time < after
                || message.pointer("/author/role").and_then(Value::as_str) == Some("user")
            {
                continue;
            }
            let image_gen = message
                .pointer("/metadata/notification_channel_id")
                .and_then(Value::as_str)
                == Some("image_gen")
                || message
                    .pointer("/metadata/permissions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .any(|permission| {
                        permission
                            .get("notification_channel_id")
                            .and_then(Value::as_str)
                            == Some("image_gen")
                    });
            walk(message, image_gen, create_time, &mut found);
        }
    }
    found.sort_by(|(ta, sa), (tb, sb)| ta.total_cmp(tb).then_with(|| sa.cmp(sb)));
    let mut out = Vec::new();
    for (_, source) in found {
        if !out.iter().any(|old| old == &source) {
            out.push(source);
        }
    }
    out
}

fn conversation_image_sources(conv: &Value) -> Vec<String> {
    conversation_image_sources_after(conv, f64::NEG_INFINITY)
}

fn stored_query_matches(stored: &str, query: &str) -> bool {
    const IMAGE_WRAP: &str = "Generate an image from this request:\n";
    let stored = stored.trim();
    let query = query.trim();
    if stored.is_empty() || query.is_empty() {
        return false;
    }
    if stored == query {
        return true;
    }
    query
        .strip_prefix(IMAGE_WRAP)
        .is_some_and(|inner| inner.trim() == stored)
        || stored
            .strip_prefix(IMAGE_WRAP)
            .is_some_and(|inner| inner.trim() == query)
}

fn conversation_query_time(conv: &Value, query: &str) -> Option<f64> {
    conv.get("mapping")?
        .as_object()?
        .values()
        .filter_map(|node| node.get("message"))
        .filter(|message| {
            message.pointer("/author/role").and_then(Value::as_str) == Some("user")
                && message
                    .pointer("/content/parts")
                    .and_then(Value::as_array)
                    .is_some_and(|parts| {
                        stored_query_matches(
                            &parts
                                .iter()
                                .filter_map(|part| {
                                    part.as_str()
                                        .or_else(|| part.get("text").and_then(Value::as_str))
                                })
                                .collect::<String>(),
                            query,
                        )
                    })
        })
        .filter_map(|message| message.get("create_time").and_then(Value::as_f64))
        .max_by(f64::total_cmp)
}

fn conversation_image_source_after_query(
    conv: &Value,
    query: &str,
    baseline_sources: &[String],
) -> Option<String> {
    let query_time = conversation_query_time(conv, query)?;
    conversation_image_sources_after(conv, query_time)
        .into_iter()
        .rev()
        .find(|source| !baseline_sources.iter().any(|old| old == source))
}

fn sediment_download_url(source: &str, cid: &str) -> Option<String> {
    let file = sediment_file(source)?;
    let cid = normalize_conversation_cid(cid).ok()?;
    Some(format!(
        "/backend-api/files/download/{file}?conversation_id={cid}&inline=false&download_intent=false"
    ))
}

async fn resolve_conversation_image_source(
    ws: &mut Ws,
    source: &str,
    cid: &str,
) -> Result<Option<String>> {
    if trusted_estuary_source(source) {
        return Ok(Some(source.to_string()));
    }
    let Some(download_path) = sediment_download_url(source, cid) else {
        return Ok(None);
    };
    let path = serde_json::to_string(&download_path).unwrap_or_default();
    let js = format!(
        r#"(async()=>{{
            const u={path};
            const signal=AbortSignal.timeout(8000);
            let r=await fetch(u,{{credentials:'include',signal}});
            if(!r.ok){{
                const v=await fetch('/api/auth/session',{{credentials:'include',signal}}).then(x=>x.json()).catch(()=>({{}}));
                if(v.accessToken)r=await fetch(u,{{credentials:'include',signal,headers:{{Authorization:'Bearer '+v.accessToken}}}});
            }}
            if(!r.ok)return null;
            const v=await r.json().catch(()=>null);
            return v&&typeof v.download_url==='string'&&v.download_url?v.download_url:null;
        }})()"#
    );
    Ok(eval(ws, &js)
        .await?
        .as_str()
        .filter(|url| !url.is_empty())
        .map(str::to_owned))
}

/// Fetch the rendered image inside the page (its session cookies satisfy the gate) and split the
/// resulting `data:<mime>;base64,<data>` URL into mime + b64. `eval` already awaits the promise.
async fn fetch_image(ws: &mut Ws, src: &str) -> Result<OutImage> {
    tracing::debug!(
        source_kind = image_url_kind(src),
        "chatgpt fetching generated image bytes"
    );
    // Network.getResponseBody reads the exact authenticated response already received by
    // Chromium. This avoids a second fetch, which can hang or be rejected after the signed URL
    // has been consumed by the page.
    if let Some(item) = cdp_image_response(src) {
        if let Ok(body) = cmd(
            ws,
            "Network.getResponseBody",
            json!({"requestId": item.request_id}),
        )
        .await
        {
            if let (Some(data), Some(base64_encoded)) = (
                body.get("body").and_then(Value::as_str),
                body.get("base64Encoded").and_then(Value::as_bool),
            ) {
                if base64_encoded && !data.is_empty() {
                    return Ok(OutImage {
                        mime: if item.mime.is_empty() {
                            "image/png".into()
                        } else {
                            item.mime
                        },
                        b64: data.into(),
                    });
                }
            }
        }
    }
    let s = serde_json::to_string(src).unwrap_or_default();
    // ChatGPT's signed estuary URL can hang on a second fetch even though Chromium has already
    // decoded and displayed the image. Capture the rendered image element first; this stays
    // inside the authenticated browser and does not depend on the CDN request completing again.
    let clip = eval(
        ws,
        &format!(
            r#"(()=>{{const i=[...document.images].find(i=>(i.currentSrc||i.src)==={s});if(!i)return null;const r=i.getBoundingClientRect();return i.complete&&i.naturalWidth>0&&r.width>0&&r.height>0?{{x:r.left,y:r.top,width:r.width,height:r.height,scale:1}}:null;}})()"#
        ),
    )
    .await?;
    if !clip.is_null() {
        // Chromium can stop answering captureScreenshot while the image compositor is busy.
        // The authenticated in-page fetch below is an equivalent fallback; a screenshot timeout
        // must not discard a successfully rendered image.
        match cmd(
            ws,
            "Page.captureScreenshot",
            json!({"format":"png","fromSurface":true,"captureBeyondViewport":true,"clip":clip}),
        )
        .await
        {
            Ok(shot) => {
                if let Some(b64) = shot
                    .get("data")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    return Ok(OutImage {
                        mime: "image/png".into(),
                        b64: b64.into(),
                    });
                }
            }
            Err(error) => {
                tracing::warn!(%error, "chatgpt browser screenshot unavailable; using in-page image fetch")
            }
        }
    }
    // If the source came from conversation JSON rather than a mounted image element, load it
    // into a tiny detached Image and read it via canvas. This keeps the request cookie-authenticated
    // and avoids a second CDP screenshot of the busy compositor.
    let js = format!(
        r#"(async()=>{{
            try {{
                const b=await fetch({s},{{credentials:'include',signal:AbortSignal.timeout(12000)}})
                    .then(r=>{{if(!r.ok)throw Error('HTTP '+r.status);return r.blob();}});
                return await new Promise(res=>{{const fr=new FileReader();fr.onload=()=>res(fr.result);fr.onerror=()=>res(null);fr.readAsDataURL(b);}});
            }} catch (_) {{ return null; }}
        }})()"#
    );
    let data = match tokio::time::timeout(Duration::from_secs(15), eval(ws, &js)).await {
        Ok(Ok(value)) => value,
        Ok(Err(Error::Timeout(_))) | Err(_) => Value::Null,
        Ok(Err(error)) => return Err(error),
    };
    if data.as_str().is_none() {
        return Err(Error::BadResponse("chatgpt_web: image bytes unavailable"));
    }
    let (mime, b64) = data
        .as_str()
        .and_then(|u| u.strip_prefix("data:"))
        .and_then(|r| r.split_once(";base64,"))
        .ok_or(Error::BadResponse("chatgpt_web: image bytes"))?;
    Ok(OutImage {
        mime: mime.to_string(),
        b64: b64.to_string(),
    })
}

async fn wait_until(ws: &mut Ws, cond: &str, secs: u64) -> Result<bool> {
    for _ in 0..secs {
        match eval(ws, cond).await {
            Ok(value) if value.as_bool() == Some(true) => return Ok(true),
            Ok(_) => {}
            Err(Error::Timeout(_)) => {
                tracing::warn!("chatgpt browser wait condition timed out; retrying");
            }
            Err(e) => return Err(e),
        }
        sleep(Duration::from_secs(1)).await;
    }
    Ok(false)
}

async fn eval(ws: &mut Ws, expr: &str) -> Result<Value> {
    let r = cmd(
        ws,
        "Runtime.evaluate",
        json!({ "expression": expr, "awaitPromise": true, "returnByValue": true }),
    )
    .await?;
    Ok(r.pointer("/result/value").cloned().unwrap_or(Value::Null))
}

async fn cmd(ws: &mut Ws, method: &str, params: Value) -> Result<Value> {
    cmd_on(ws, None, method, params).await
}

static CDP_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
struct CdpImageResponse {
    request_id: String,
    url: String,
    mime: String,
    finished: bool,
}

fn image_url_kind(url: &str) -> &'static str {
    if url.contains("/estuary/content") {
        "estuary"
    } else if url.contains("/backend-api/files") {
        "file"
    } else if url.contains("/dalle") {
        "dalle"
    } else if url.starts_with("data:image/") {
        "data"
    } else {
        "other"
    }
}

static CDP_IMAGE_RESPONSES: OnceLock<Mutex<Vec<CdpImageResponse>>> = OnceLock::new();

fn cdp_image_responses() -> &'static Mutex<Vec<CdpImageResponse>> {
    CDP_IMAGE_RESPONSES.get_or_init(|| Mutex::new(Vec::new()))
}

fn clear_cdp_image_responses() {
    if let Ok(mut responses) = cdp_image_responses().lock() {
        responses.clear();
    }
}

fn record_cdp_event(msg: &Value) {
    match msg.get("method").and_then(Value::as_str) {
        Some("Network.responseReceived") => {
            let Some(response) = msg.pointer("/params/response") else {
                return;
            };
            let Some(url) = response.get("url").and_then(Value::as_str) else {
                return;
            };
            if url.contains("/backend-api/conversation") || url.contains("/backend-api/files") {
                tracing::debug!(
                    endpoint = image_url_kind(url),
                    "chatgpt browser backend response"
                );
            }
            let mime = response
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !(mime.starts_with("image/")
                || url.contains("/estuary/content")
                || url.contains("/dalle")
                || (url.contains("/backend-api/files") && mime != "application/json"))
            {
                return;
            }
            let Some(request_id) = msg.pointer("/params/requestId").and_then(Value::as_str) else {
                return;
            };
            if let Ok(mut responses) = cdp_image_responses().lock() {
                if !responses.iter().any(|item| item.request_id == request_id) {
                    tracing::debug!(
                        kind = image_url_kind(url),
                        mime,
                        "chatgpt browser image network response"
                    );
                    responses.push(CdpImageResponse {
                        request_id: request_id.to_string(),
                        url: url.to_string(),
                        mime: mime.to_string(),
                        finished: false,
                    });
                }
            }
        }
        Some("Network.loadingFinished") => {
            let Some(request_id) = msg.pointer("/params/requestId").and_then(Value::as_str) else {
                return;
            };
            if let Ok(mut responses) = cdp_image_responses().lock() {
                if let Some(item) = responses
                    .iter_mut()
                    .find(|item| item.request_id == request_id)
                {
                    item.finished = true;
                }
            }
        }
        _ => {}
    }
}

fn cdp_image_response(src: &str) -> Option<CdpImageResponse> {
    cdp_image_responses()
        .lock()
        .ok()?
        .iter()
        .rev()
        .find(|item| item.finished && item.url == src)
        .cloned()
}

/// Like `cmd` but optionally targets an attached (OOPIF) session by id. Responses carry the same
/// globally-unique `id`, so matching by id works regardless of which session answered.
async fn cmd_on(ws: &mut Ws, sess: Option<&str>, method: &str, params: Value) -> Result<Value> {
    let id = CDP_ID.fetch_add(1, Ordering::Relaxed);
    let mut frame = json!({ "id": id, "method": method, "params": params });
    if let Some(s) = sess {
        frame["sessionId"] = json!(s);
    }
    ws.send(Message::Text(frame.to_string().into())).await?;
    let result = timeout(CDP_COMMAND_WAIT, async {
        while let Some(f) = ws.next().await {
            if let Message::Text(txt) = f? {
                let msg: Value = serde_json::from_str(txt.as_str())?;
                record_cdp_event(&msg);
                if msg["id"].as_u64() == Some(id) {
                    if msg.get("error").is_some() {
                        tracing::warn!(method, response = %msg, "chatgpt browser CDP returned an error");
                        return Err(Error::BadResponse("cdp error"));
                    }
                    return Ok(msg["result"].clone());
                }
            }
        }
        Err(Error::BadResponse("cdp connection closed"))
    })
    .await;
    match result {
        Ok(result) => result,
        Err(_) => {
            let detail = params
                .get("expression")
                .and_then(Value::as_str)
                .map(|s| s.chars().take(180).collect::<String>());
            tracing::warn!(method, ?detail, "chatgpt browser CDP command timed out");
            Err(Error::Timeout("chatgpt_web: cdp command"))
        }
    }
}

/// Evaluate `expr` inside a specific execution context of an attached session.
async fn eval_ctx(ws: &mut Ws, sess: &str, ctx: i64, expr: &str) -> Result<Value> {
    let r = cmd_on(
        ws,
        Some(sess),
        "Runtime.evaluate",
        json!({ "expression": expr, "contextId": ctx, "returnByValue": true }),
    )
    .await?;
    Ok(r.pointer("/result/value").cloned().unwrap_or(Value::Null))
}

struct DrPlan {
    text: String,
    start_x: f64,
    start_y: f64,
}

/// The deep-research query words that approve the current plan (anything else is treated as a
/// revised research request).
fn is_start_word(q: &str) -> bool {
    matches!(
        q.trim().to_ascii_lowercase().as_str(),
        "start" | "go" | "run" | "approve" | "yes" | "ok" | ""
    )
}

/// Drop the widget's button/countdown footer, leaving just the plan title + steps.
fn clean_plan(txt: &str) -> String {
    for marker in ["\nEdit\nCancel", "\nEdit\n", "\nPlan starts in"] {
        if let Some(i) = txt.find(marker) {
            return txt[..i].trim().to_string();
        }
    }
    txt.trim().to_string()
}

/// Attach to the deep-research sandbox iframe (a separate cross-origin target) and return its CDP
/// session id, plus the iframe element's top-page offset (to turn in-frame coords into page coords).
async fn dr_sandbox(ws: &mut Ws) -> Result<Option<(String, f64, f64)>> {
    let tgts = cmd(ws, "Target.getTargets", json!({})).await?;
    let tid = tgts
        .pointer("/targetInfos")
        .and_then(|x| x.as_array())
        .into_iter()
        .flatten()
        .find(|ti| {
            ti.get("url")
                .and_then(|x| x.as_str())
                .is_some_and(|u| u.contains("connector_openai_deep_research"))
        })
        .and_then(|ti| {
            ti.get("targetId")
                .and_then(|x| x.as_str())
                .map(String::from)
        });
    let Some(tid) = tid else { return Ok(None) };
    let at = cmd(
        ws,
        "Target.attachToTarget",
        json!({"targetId": tid, "flatten": true}),
    )
    .await?;
    let Some(sid) = at
        .get("sessionId")
        .and_then(|x| x.as_str())
        .map(String::from)
    else {
        return Ok(None);
    };
    let _ = cmd_on(ws, Some(&sid), "Page.enable", json!({})).await;
    let off = eval(ws, r#"(()=>{const f=[...document.querySelectorAll('iframe')].find(f=>/connector_openai_deep_research/.test(f.src));if(!f)return null;const r=f.getBoundingClientRect();return [r.left,r.top];})()"#).await?;
    let (ox, oy) = off
        .as_array()
        .filter(|a| a.len() == 2)
        .map(|a| (a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)))
        .unwrap_or((0.0, 0.0));
    Ok(Some((sid, ox, oy)))
}

/// Find the plan card (rendered in a same-origin child frame of the sandbox) and read its text plus
/// the Start button's page coordinates.
async fn read_dr_plan(ws: &mut Ws, sid: &str, ox: f64, oy: f64) -> Result<Option<DrPlan>> {
    let tree = cmd_on(ws, Some(sid), "Page.getFrameTree", json!({})).await?;
    let mut frames = Vec::new();
    fn collect(f: &Value, out: &mut Vec<String>) {
        if let Some(id) = f.pointer("/frame/id").and_then(|x| x.as_str()) {
            out.push(id.to_string());
        }
        if let Some(ch) = f.get("childFrames").and_then(|x| x.as_array()) {
            ch.iter().for_each(|c| collect(c, out));
        }
    }
    collect(tree.get("frameTree").unwrap_or(&Value::Null), &mut frames);
    for fid in &frames {
        let iw = cmd_on(
            ws,
            Some(sid),
            "Page.createIsolatedWorld",
            json!({"frameId": fid, "worldName": "fx"}),
        )
        .await?;
        let ctx = match iw.pointer("/executionContextId").and_then(|x| x.as_i64()) {
            Some(c) => c,
            None => continue,
        };
        let v = eval_ctx(ws, sid, ctx, r#"JSON.stringify({txt:(document.body.innerText||''),btn:(()=>{const b=[...document.querySelectorAll('button,[role=button]')].find(b=>/^Start/.test((b.textContent||'').trim()));if(!b)return null;const r=b.getBoundingClientRect();return [r.left+r.width/2,r.top+r.height/2];})()})"#).await?;
        let parsed: Value = v
            .as_str()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null);
        if let Some(btn) = parsed.get("btn").and_then(|x| x.as_array()) {
            let bx = btn.first().and_then(|x| x.as_f64()).unwrap_or(0.0);
            let by = btn.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0);
            let txt = parsed.get("txt").and_then(|x| x.as_str()).unwrap_or("");
            return Ok(Some(DrPlan {
                text: clean_plan(txt),
                start_x: ox + bx,
                start_y: oy + by,
            }));
        }
    }
    Ok(None)
}

/// Poll for the deep-research plan card to render (it appears a few seconds after kickoff).
async fn await_dr_plan(ws: &mut Ws) -> Result<Option<DrPlan>> {
    let mut sess: Option<(String, f64, f64)> = None;
    for _ in 0..16 {
        if sess.is_none() {
            sess = dr_sandbox(ws).await.unwrap_or(None);
        }
        if let Some((sid, ox, oy)) = &sess {
            if let Some(plan) = read_dr_plan(ws, sid, *ox, *oy).await? {
                return Ok(Some(plan));
            }
        }
        sleep(Duration::from_secs(3)).await;
    }
    Ok(None)
}

/// Approve a parked plan: click its Start button, then hand back the poll session. Best-effort — if
/// the plan card is gone (already started/expired), the poll still fetches the eventual report.
async fn start_dr_plan(ws: &mut Ws, cid: &str) -> Result<Outcome> {
    if let Some(plan) = await_dr_plan(ws).await? {
        click_at(ws, plan.start_x, plan.start_y).await?;
        sleep(Duration::from_secs(3)).await;
    }
    let mut out = Outcome::new(
        "Deep research is now running. Call deep_research again with this session to fetch the \
         report when ready (~5-30 min)."
            .into(),
        1,
    );
    out.session = Some(format!("dr|poll|{cid}"));
    Ok(out)
}

async fn wait_for_page(port: u16) -> Result<String> {
    let http = reqwest::Client::new();
    // Hosted Chromium can cold-start under CPU contention while loading the ChatGPT shell.
    // Ten seconds made a healthy browser look unavailable and caused an unnecessary failover.
    for _ in 0..120 {
        if let Ok(resp) = http
            .get(format!("http://127.0.0.1:{port}/json"))
            .send()
            .await
        {
            if let Ok(targets) = resp.json::<Vec<Value>>().await {
                if let Some(ws) = targets.iter().find_map(|t| {
                    (t["type"] == "page")
                        .then(|| t["webSocketDebuggerUrl"].as_str())
                        .flatten()
                }) {
                    return Ok(ws.to_string());
                }
            }
        }
        sleep(Duration::from_millis(500)).await;
    }
    Err(Error::Timeout("chrome devtools"))
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(9222)
}

fn login_err() -> Error {
    Error::Provider {
        provider: "chatgpt_web",
        status: 401,
        body: format!(
            "not logged in; {}",
            crate::usage::provider_login_hint("chatgpt_web")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ax_exact_backend, composer_backend_node, conversation_image_source_after_query,
        conversation_image_sources, conversation_list_cids, conversation_query_time, norm,
        normalize_conversation_cid, parse_img_poll, percent_decode, percent_encode,
        sediment_download_url, stored_query_matches, trusted_estuary_source, without_known_cids,
    };

    #[test]
    fn img_poll_query_round_trips_separators_and_unicode() {
        for query in [
            "A blue square|with text 100% sure",
            "multiline\nprompt: emoji 🖼️",
            "Generate an image from this request:\nplain",
        ] {
            assert_eq!(percent_decode(&percent_encode(query)), query);
        }
        assert!(percent_encode("a|b").find('|').is_none());
    }

    #[test]
    fn img_poll_allows_empty_cid_before_conversation_lists() {
        let query = "Generate an image from this request:\nHOSTED";
        let token = format!("img|poll||{}", percent_encode(query));
        let (cid, got) = parse_img_poll(Some(&token)).unwrap().unwrap();
        assert!(cid.is_empty());
        assert_eq!(got, query);
        let cid = "6a7ad1b3-9208-83eb-a59c-a79ff60054d4";
        let token = format!("img|poll|{cid}|{}", percent_encode(query));
        let parsed = parse_img_poll(Some(&token)).unwrap().unwrap();
        assert_eq!(parsed.0, cid);
        assert_eq!(parsed.1, query);
        assert!(parse_img_poll(Some("img|poll|not-a-uuid|q")).is_err());
        assert!(parse_img_poll(Some("dr|poll|abc")).unwrap().is_none());
    }

    #[test]
    fn ax_tool_picker_rejects_sidebar_history() {
        let tree = serde_json::json!({"nodes": [
            {"nodeId":"history","role":{"value":"link"},"name":{"value":"Create image"},"backendDOMNodeId":10},
            {"nodeId":"label","parentId":"row","role":{"value":"generic"},"name":{"value":"Create image"},"backendDOMNodeId":20},
            {"nodeId":"row","parentId":"group","role":{"value":"generic"},"name":{"value":""},"backendDOMNodeId":21,
             "properties":[{"name":"focusable","value":{"value":true}}]},
            {"nodeId":"group","role":{"value":"group"},"name":{"value":""}}
        ]});
        assert_eq!(ax_exact_backend(&tree, &["Create image"]), Some(21));
    }

    #[test]
    fn ax_tree_finds_chatgpt_composer() {
        let tree = serde_json::json!({
            "nodes": [{
                "role": {"value": "textbox"},
                "name": {"value": "chat with chatgpt"},
                "backendDOMNodeId": 42
            }]
        });
        assert_eq!(composer_backend_node(&tree), Some(42));
    }

    #[test]
    fn norm_matches_picker_labels() {
        assert_eq!(norm("High"), "high");
        assert_eq!(norm("GPT-5.4"), "gpt54");
        assert_eq!(norm("gpt-5.5"), "gpt55");
        assert_eq!(norm("o3"), "o3");
        // user input variants land on the same key as the live label
        assert_eq!(norm("GPT 5.4"), norm("GPT-5.4"));
    }

    #[test]
    fn prefixed_image_prompt_matches_unprefixed_user_turn() {
        assert!(stored_query_matches(
            "orange circle",
            "Generate an image from this request:\norange circle"
        ));
        let conv = serde_json::json!({"mapping": {
            "prompt": {"message": {
                "create_time": 2.0,
                "author": {"role": "user"},
                "content": {"parts": ["orange circle"]}
            }}
        }});
        assert!(conversation_query_time(
            &conv,
            "Generate an image from this request:\norange circle"
        )
        .is_some());
        assert!(!stored_query_matches("orange circle", "circle"));
    }

    #[test]
    fn image_source_must_follow_the_submitted_query() {
        let conv = serde_json::json!({"mapping": {
            "stale": {"message": {
                "create_time": 1.0,
                "author": {"role": "tool"},
                "metadata": {"notification_channel_id": "image_gen"},
                "content": {"parts": [{"asset_pointer": "sediment://file_stale"}]}
            }},
            "prompt": {"message": {
                "create_time": 2.0,
                "author": {"role": "user"},
                "content": {"parts": ["orange circle"]}
            }},
            "fresh": {"message": {
                "create_time": 3.0,
                "author": {"role": "tool"},
                "metadata": {"notification_channel_id": "image_gen"},
                "content": {"parts": [{"asset_pointer": "sediment://file_orange"}]}
            }}
        }});

        assert_eq!(
            conversation_image_source_after_query(&conv, "orange circle", &[]).as_deref(),
            Some("sediment://file_orange")
        );
        assert_eq!(
            conversation_image_source_after_query(&conv, "foreign prompt", &[]),
            None
        );
    }

    #[test]
    fn candidate_baseline_filters_every_preexisting_cid_source() {
        let known = vec!["sidebar-old".into(), "probe-old".into()];
        assert_eq!(
            without_known_cids(
                vec!["sidebar-old".into(), "probe-old".into(), "fresh".into()],
                &known,
            ),
            vec!["fresh"]
        );
    }

    #[test]
    fn conversation_list_provides_renderer_independent_candidates() {
        let fresh = "6a7ac323-f374-83eb-b93d-9bd32f32923d";
        let older = "6a7ab8aa-0000-4000-8000-000000000000";
        let list = serde_json::json!({"items": [
            {"id": format!("WEB:{fresh}")},
            {"id": older},
            {"id": "../../malicious"}
        ]});
        assert_eq!(conversation_list_cids(&list), vec![fresh, older]);
    }

    #[test]
    fn conversation_session_accepts_only_a_uuid_with_optional_web_prefix() {
        let cid = "6A7AC323-F374-83EB-B93D-9BD32F32923D";
        assert_eq!(
            normalize_conversation_cid(&format!("WEB:{cid}")).unwrap(),
            cid.to_ascii_lowercase()
        );
        for malicious in [
            "../../api/auth/session",
            "WEB:WEB:6a7ac323-f374-83eb-b93d-9bd32f32923d",
            "6a7ac323-f374-83eb-b93d-9bd32f32923d';alert(1)//",
            "//evil.test/6a7ac323-f374-83eb-b93d-9bd32f32923d",
        ] {
            assert!(normalize_conversation_cid(malicious).is_err());
        }
    }

    #[test]
    fn conversation_image_source_rejects_untrusted_urls() {
        assert!(trusted_estuary_source("/backend-api/estuary/content/file"));
        assert!(trusted_estuary_source(
            "https://chatgpt.com/backend-api/estuary/content/file"
        ));
        for foreign in [
            "https://evil.test/backend-api/estuary/content/file",
            "https://chatgpt.com.evil.test/backend-api/estuary/content/file",
            "//evil.test/backend-api/estuary/content/file",
            "data:text/plain,/backend-api/estuary/content/file",
        ] {
            assert!(!trusted_estuary_source(foreign));
        }

        let conv = serde_json::json!({"parts": [
            "https://evil.test/backend-api/estuary/content/file",
            "//evil.test/backend-api/estuary/content/file",
            "data:text/plain,/backend-api/estuary/content/file",
            "/backend-api/estuary/content/file",
            "https://chatgpt.com/backend-api/estuary/content/file"
        ]});
        assert_eq!(
            conversation_image_sources(&conv),
            vec![
                "/backend-api/estuary/content/file",
                "https://chatgpt.com/backend-api/estuary/content/file"
            ]
        );
    }

    #[test]
    fn sediment_image_source_resolves_to_file_download() {
        let cid = "6a7a71ac-0afc-83eb-ac78-cad9334210e4";
        let source = "sediment://file_00000000119481f4a8f8bdb6105f9dc7";
        let conv = serde_json::json!({
            "mapping": {
                "user-upload": {"message": {
                    "create_time": 1.0,
                    "author": {"role": "user"},
                    "metadata": {"notification_channel_id": "file_upload"},
                    "content": {"parts": [{"asset_pointer": "sediment://file_user"}]}
                }},
                "older-image": {"message": {
                    "create_time": 2.0,
                    "author": {"role": "tool"},
                    "metadata": {"notification_channel_id": "image_gen"},
                    "content": {"parts": [{"asset_pointer": "sediment://file_old"}]}
                }},
                "newer-image": {"message": {
                    "create_time": 3.0,
                    "author": {"role": "tool"},
                    "metadata": {"permissions": [{"notification_channel_id": "image_gen"}]},
                    "content": {"parts": [{"asset_pointer": source}]}
                }},
                "legacy": {"message": {
                    "create_time": 4.0,
                    "author": {"role": "assistant"},
                    "metadata": {},
                    "content": {"parts": ["/backend-api/estuary/content/legacy"]}
                }}
            }
        });

        assert_eq!(
            conversation_image_sources(&conv),
            vec![
                "sediment://file_old",
                source,
                "/backend-api/estuary/content/legacy"
            ]
        );
        assert_eq!(
            sediment_download_url(source, cid).as_deref(),
            Some("/backend-api/files/download/file_00000000119481f4a8f8bdb6105f9dc7?conversation_id=6a7a71ac-0afc-83eb-ac78-cad9334210e4&inline=false&download_intent=false")
        );
        assert_eq!(
            sediment_download_url("https://evil.test/image.png", cid),
            None
        );
        assert_eq!(
            sediment_download_url("sediment://file_safe?redirect=evil", cid),
            None
        );
    }
}
