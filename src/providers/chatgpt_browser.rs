use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use base64::Engine;
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
// a byte-identical replay 403s "unusual activity"). So we drive a headless Chrome over CDP: inject the
// captured session cookies, type the prompt into the composer (the page's own send is what passes the
// gate), then read the answer back via an in-page GET (reads are not gated) and reuse chatgpt_web's
// conversation parsers. Deep research / web search are enabled by clicking the composer's tools menu.

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const CHAT_WAIT: u64 = 120;
// Image generation runs longer than a chat turn (often 30-60s).
const IMAGE_WAIT: u64 = 180;
const DRIVE_WAIT: Duration = Duration::from_secs(240);
// A normal-Chrome UA: the default `--headless` UA contains "HeadlessChrome", an instant Cloudflare tell.
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36";

/// Aborted callers can leave a headless profile behind after the parent process is killed.
/// Requests finish within a few minutes, so profiles older than an hour are never active.
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
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--disable-blink-features=AutomationControlled")
        .arg(format!("--user-agent={UA}"))
        .arg("--window-size=1280,1000")
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
    // approves it. A non-`dr|` session is a chat conversation to continue (dr polls go to HTTP).
    let dr_plan_cid = session.and_then(|s| s.strip_prefix("dr|plan|"));
    let dr_poll_cid = session
        .and_then(|s| s.strip_prefix("dr|poll|"))
        .map(|s| s.strip_prefix("WEB:").unwrap_or(s));
    let approve =
        matches!(cap, Capability::DeepResearch) && dr_plan_cid.is_some() && is_start_word(query);
    let resume = session
        .filter(|s| !s.starts_with("dr|"))
        .map(|s| s.strip_prefix("chatgpt_web:").unwrap_or(s))
        .map(|s| s.split('|').next().unwrap_or(s));

    let ws_url = wait_for_page(port).await?;
    let (mut ws, _) = connect_async(ws_url.as_str()).await?;
    cmd(&mut ws, "Network.enable", json!({})).await?;
    cmd(&mut ws, "Page.enable", json!({})).await?;
    let url = match (
        approve.then_some(dr_plan_cid).flatten(),
        dr_poll_cid,
        resume,
    ) {
        (Some(cid), _, _) => format!("https://chatgpt.com/c/{cid}"),
        (None, Some(cid), _) => format!("https://chatgpt.com/c/{cid}"),
        (None, None, Some(c)) => format!("https://chatgpt.com/c/{c}"),
        (None, None, None) => "https://chatgpt.com/".to_string(),
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
    cmd(
        &mut ws,
        "Network.setCookies",
        json!({ "cookies": cdp_cookies(cookies) }),
    )
    .await?;
    cmd(&mut ws, "Page.navigate", json!({ "url": url })).await?;
    if resume.is_some() || approve || dr_poll_cid.is_some() {
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
    let composer = wait_until(&mut ws, "!!(()=>{const q=[...document.querySelectorAll('#prompt-textarea,[contenteditable=\"true\"][role=\"textbox\"]')];return q.find(e=>{const r=e.getBoundingClientRect();return r.width>0&&r.height>0&&!e.disabled;})})()", 45).await?;
    let logged_in = composer && is_logged_in(&mut ws).await?;
    if std::env::var("CGPT_DEBUG").is_ok() {
        let _ = screenshot(&mut ws, Path::new("/tmp/cgpt-debug.png")).await;
        let url = eval(&mut ws, "location.href").await?;
        let title = eval(&mut ws, "document.title").await?;
        let cc = eval(&mut ws, "document.cookie.length").await?;
        eprintln!(
            "CGPT_DEBUG composer={composer} logged_in={logged_in} url={url} title={title} cookie_len={cc}"
        );
    }
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
        wait_until(&mut ws, "!!(()=>{const q=[...document.querySelectorAll('#prompt-textarea,[contenteditable=\"true\"][role=\"textbox\"]')];return q.find(e=>{const r=e.getBoundingClientRect();return r.width>0&&r.height>0&&!e.disabled;})})()", 45).await?;
        dismiss_account_picker(&mut ws).await?;
    }
    if !app_shell_authenticated(&mut ws).await? {
        return Err(login_err());
    }
    if let Some(cid) = dr_poll_cid {
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
    if resume.is_some() {
        // The first target navigation can still be redirected while ChatGPT establishes the
        // authenticated app shell. Retry from the now-authenticated shell before sending a
        // follow-up; otherwise it silently starts a fresh root conversation.
        cmd(&mut ws, "Page.navigate", json!({ "url": url })).await?;
        let cid = url
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .trim_start_matches("WEB:");
        let expected = serde_json::to_string(cid).unwrap_or_else(|_| "\"\"".into());
        if !wait_until(
            &mut ws,
            &format!("location.href.includes('/c/') && location.href.includes({expected})"),
            45,
        )
        .await?
        {
            let web_url = format!("https://chatgpt.com/c/WEB:{cid}");
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
        wait_until(
            &mut ws,
            "document.querySelector('[data-message-author-role]') !== null",
            15,
        )
        .await?;
    }
    install_cid_probe(&mut ws).await?;
    let _ = eval(
        &mut ws,
        "document.querySelector('#prompt-textarea')?.focus()",
    )
    .await;
    sleep(Duration::from_millis(800)).await;

    // Approve a parked deep-research plan: click its Start button, then hand back a poll session.
    if let Some(cid) = approve.then_some(dr_plan_cid).flatten() {
        return start_dr_plan(&mut ws, cid).await;
    }

    // Continuing a conversation: note the newest existing answer so we wait for the *new* reply.
    let baseline = match resume {
        Some(c) => chatgpt_web::last_assistant_time(&get_conversation(&mut ws, c).await?),
        None => f64::NEG_INFINITY,
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
        } else {
            enable_tool(&mut ws, "create image").await?;
        }
    } else if matches!(cap, Capability::DeepResearch) {
        enable_tool(&mut ws, "deep research").await?;
    } else if search {
        enable_tool(&mut ws, "web search").await?;
    }

    if !files.is_empty() {
        attach_file(&mut ws, files).await?;
    }

    let known_cids = conversation_ids(&mut ws).await?;
    // Keep the pre-send assistant count so a follow-up cannot be satisfied by the previous
    // turn's rendered node while the new response is still streaming.
    let assistant_count = eval(
        &mut ws,
        "document.querySelectorAll('[data-message-author-role=\"assistant\"]').length",
    )
    .await?
    .as_u64()
    .unwrap_or(0);
    let image_count = eval(
        &mut ws,
        "document.querySelectorAll('img[alt^=\\\"Generated image\\\"]').length",
    )
    .await?
    .as_u64()
    .unwrap_or(0);
    let image_sources = eval(
        &mut ws,
        "JSON.stringify([...document.querySelectorAll('img[alt^=\\\"Generated image\\\"]')].map(i=>i.src).filter(Boolean))",
    )
    .await?
    .as_str()
    .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
    .unwrap_or_default();
    if std::env::var("CGPT_DEBUG").is_ok() {
        let loc = eval(&mut ws, "location.href").await?;
        let nodes = eval(
            &mut ws,
            "document.querySelectorAll('[data-message-author-role=\\\"assistant\\\"]').length",
        )
        .await?;
        eprintln!(
            "CGPT_DEBUG before_send location={loc} assistant_count={assistant_count} nodes={nodes}"
        );
    }
    let sent = send_prompt(&mut ws, query).await?;
    if std::env::var("CGPT_DEBUG").is_ok() {
        eprintln!("CGPT_DEBUG send_result={sent}");
    }
    if sent != "sent" {
        return Err(Error::BadResponse("chatgpt_web: composer drive failed"));
    }

    // A fresh ChatGPT turn can render its answer before the SPA exposes the new conversation
    // id (the sidebar/navigation is eventually consistent). Chat/image flows still need the id,
    // but ordinary chat can safely read the rendered assistant node without blocking on it.
    let cid = match resume {
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

    // Create image: wait for the rendered result, then read its bytes from inside the page — the CDN
    // URL is session-gated, so an out-of-band GET 403s.
    if matches!(cap, Capability::Image) {
        let src = wait_for_image(&mut ws, IMAGE_WAIT, image_count, &image_sources).await?;
        let mut out = Outcome::new(String::new(), 1);
        out.image = Some(fetch_image(&mut ws, &src).await?);
        out.session = if cid.is_empty() {
            wait_for_cid(&mut ws, 5, &known_cids).await.ok()
        } else {
            Some(cid)
        };
        return Ok(out);
    }

    // Chat / web search: the turn finishes in seconds — wait, then read the clean message back.
    // The response can be read from the rendered assistant node even when the conversation GET is
    // briefly unavailable while ChatGPT is still persisting the turn.
    let rendered = format!(
        r#"(()=>{{const a=[...document.querySelectorAll('[data-message-author-role="assistant"]')].slice({assistant_count}).pop();return a&&a.innerText?a.innerText:null;}})()"#
    );
    let deadline = Instant::now() + Duration::from_secs(CHAT_WAIT);
    loop {
        // The rendered UI can finish before the persistence endpoint does. Read it first; this
        // also avoids waiting behind a stalled service-worker fetch.
        let streaming = eval(
            &mut ws,
            "!!document.querySelector('[data-testid*=stop],button[aria-label*=Stop]')",
        )
        .await?
        .as_bool()
        .unwrap_or(false);
        if !streaming {
            if let Some(text) = eval(&mut ws, &rendered).await?.as_str() {
                if text.trim().is_empty() {
                    sleep(Duration::from_secs(2)).await;
                    continue;
                }
                let mut out = Outcome::new(text.trim().to_string(), 1);
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
        if !cid.is_empty() {
            if let Ok(conv) = get_conversation(&mut ws, &cid).await {
                if let Ok(mut out) = chatgpt_web::extract_answer_after(&conv, baseline) {
                    out.session = Some(cid.clone());
                    return Ok(out);
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(Error::Timeout("chatgpt_web: no answer"));
        }
        sleep(Duration::from_secs(2)).await;
    }
}

async fn edit_existing_image(ws: &mut Ws) -> Result<()> {
    // Conversation pages virtualize old turns. Scroll the image into the viewport so its
    // toolbar is mounted; otherwise headless Chrome can have a valid conversation with no
    // rendered image/action controls at all.
    let _ = eval(ws, "window.scrollTo(0, document.body.scrollHeight)").await;
    let image = r#"[...document.querySelectorAll('img[alt^="Generated image"]')].find(i=>i.complete&&i.naturalWidth>200)"#;
    if let Some((x, y)) = center_of(ws, image).await? {
        hover_at(ws, x, y).await?;
        sleep(Duration::from_millis(700)).await;
    }
    let selector = r#"[...document.querySelectorAll('button,[role=button]')].find(e=>/edit image/i.test((e.getAttribute('aria-label')||e.innerText||'').trim()))"#;
    for _ in 0..60 {
        if let Some((x, y)) = center_of(ws, selector).await? {
            click_at(ws, x, y).await?;
            sleep(Duration::from_millis(500)).await;
            return Ok(());
        }
        if let Some((x, y)) = center_of(ws, image).await? {
            click_at(ws, x, y).await?;
            sleep(Duration::from_millis(500)).await;
        }
        sleep(Duration::from_millis(500)).await;
    }
    Err(Error::BadResponse(
        "chatgpt_web: image edit control not found",
    ))
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

async fn send_prompt(ws: &mut Ws, query: &str) -> Result<String> {
    let editor = r#"(()=>{const q=[...document.querySelectorAll('#prompt-textarea,[contenteditable="true"][role="textbox"]')];return q.find(e=>{const r=e.getBoundingClientRect();return r.width>0&&r.height>0&&!e.disabled;})})()"#;
    if !eval(ws, &format!("!!({editor})"))
        .await?
        .as_bool()
        .unwrap_or(false)
    {
        return Ok("no-editor".into());
    }
    eval(ws, &format!("({editor})?.focus()")).await?;
    cmd(ws, "Input.insertText", json!({"text": query})).await?;
    sleep(Duration::from_millis(500)).await;
    let selector = "[data-testid='send-button'],button[aria-label*='Send']";
    let send_expr = format!("document.querySelector(\"{selector}\")");
    if let Some((x, y)) = center_of(ws, &send_expr).await? {
        let ready = eval(ws, &format!("(()=>{{const b=document.querySelector(\"{selector}\");return !!b&&!b.disabled;}})()"))
            .await?.as_bool().unwrap_or(false);
        if !ready {
            return Ok("no-send".into());
        }
        click_at(ws, x, y).await?;
        Ok("sent".into())
    } else {
        if std::env::var("CGPT_DEBUG").is_ok() {
            let controls = eval(ws, "JSON.stringify([...document.querySelectorAll('button')].map(b=>({aria:b.getAttribute('aria-label'),test:b.getAttribute('data-testid'),disabled:b.disabled,text:(b.innerText||'').trim().slice(0,60)})).filter(x=>x.aria||x.test||x.text))").await?;
            eprintln!("CGPT_DEBUG composer_controls={controls}");
        }
        Ok("no-send".into())
    }
}

/// Open the composer "+" menu and click the tool whose label matches (e.g. "deep research"). React
/// portals ignore synthetic `.click()`, so we dispatch real CDP mouse events at element centers.
async fn enable_tool(ws: &mut Ws, tool: &str) -> Result<()> {
    let plus = r#"[...document.querySelectorAll('button')].find(b=>/add files|add photos|attach/i.test(b.getAttribute('aria-label')||''))"#;
    if std::env::var("CGPT_DEBUG").is_ok() {
        let buttons = eval(ws, "JSON.stringify([...document.querySelectorAll('button')].map(b=>({aria:b.getAttribute('aria-label'),test:b.getAttribute('data-testid'),text:(b.innerText||'').trim()})))").await?;
        eprintln!("CGPT_DEBUG tool_buttons={buttons}");
    }
    if let Some((x, y)) = center_of(ws, plus).await? {
        click_at(ws, x, y).await?;
        sleep(Duration::from_millis(500)).await;
    }
    let item = if tool == "web search" {
        r#"[...document.querySelectorAll('div,button,a,[role=menuitem]')].find(e=>{const t=(e.innerText||e.textContent||'').trim().toLowerCase();return (t==='search'||t.startsWith('web search'))&&e.getBoundingClientRect().width>0&&e.getBoundingClientRect().height>0})"#.to_string()
    } else {
        format!(
            r#"[...document.querySelectorAll('div,button,a,[role=menuitem]')].find(e=>{{const t=(e.innerText||e.textContent||'').trim().toLowerCase().replace('create an image','create image');return t.startsWith({t})&&e.getBoundingClientRect().width>0&&e.getBoundingClientRect().height>0}})"#,
            t = serde_json::to_string(tool).unwrap_or_default()
        )
    };
    if tool == "create image" {
        let shortcut = r#"[...document.querySelectorAll('button,[role=button],div')].find(e=>/^create an image$/i.test((e.innerText||'').trim()))"#;
        let mut point = None;
        for _ in 0..20 {
            point = center_of(ws, shortcut).await?;
            if point.is_some() {
                break;
            }
            sleep(Duration::from_millis(500)).await;
        }
        if let Some((x, y)) = point {
            click_at(ws, x, y).await?;
            let mode = r#"(()=>{const e=[...document.querySelectorAll('textarea,[contenteditable="true"]')].find(e=>{const r=e.getBoundingClientRect();return r.width>0&&r.height>0});return e?.getAttribute('placeholder')||''})()"#;
            if !wait_until(ws, &format!("({mode}).toLowerCase().includes('image')"), 10).await? {
                // Some builds expose the shortcut as a suggestion card but require a second
                // real click after the portal settles.
                if let Some((sx, sy)) = center_of(ws, shortcut).await? {
                    click_at(ws, sx, sy).await?;
                    wait_until(ws, &format!("({mode}).toLowerCase().includes('image')"), 10)
                        .await?;
                }
            }
            return Ok(());
        }
    }
    if let Some((x, y)) = center_of(ws, &item).await? {
        click_at(ws, x, y).await?;
        sleep(Duration::from_millis(500)).await;
    } else if std::env::var("CGPT_DEBUG").is_ok() {
        let menu = eval(ws, "JSON.stringify([...document.querySelectorAll('[role=menuitem],button,[role=menu]')].map(e=>({role:e.getAttribute('role'),aria:e.getAttribute('aria-label'),text:(e.innerText||e.textContent||'').trim(),html:e.outerHTML.slice(0,500)})).filter(x=>x.text||x.aria))").await?;
        eprintln!("CGPT_DEBUG tool_menu={menu}");
    }
    Ok(())
}

/// Center (viewport coords) of the element returned by `find`, or `None` if absent / zero-sized.
async fn center_of(ws: &mut Ws, find: &str) -> Result<Option<(f64, f64)>> {
    let js = format!(
        "(()=>{{const e={find};if(!e)return null;const r=e.getBoundingClientRect();\
         return (r.width>0&&r.height>0)?[r.left+r.width/2,r.top+r.height/2]:null;}})()"
    );
    Ok(eval(ws, &js)
        .await?
        .as_array()
        .filter(|a| a.len() == 2)
        .map(|a| (a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0))))
}

async fn click_at(ws: &mut Ws, x: f64, y: f64) -> Result<()> {
    let down = json!({"type":"mousePressed","x":x,"y":y,"button":"left","clickCount":1});
    let up = json!({"type":"mouseReleased","x":x,"y":y,"button":"left","clickCount":1});
    cmd(ws, "Input.dispatchMouseEvent", down).await?;
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
    let cid = cid.strip_prefix("WEB:").unwrap_or(cid);
    let js = format!(
        r#"(async()=>{{
            const u='/backend-api/conversation/{cid}?include_visually_hidden_messages=true';
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
    let js = r#"(async()=>{try{const r=await fetch('/api/auth/session',{credentials:'include'});const v=await r.json();return !!(v&&v.user);}catch(_){return false;}})()"#;
    Ok(eval(ws, js).await?.as_bool().unwrap_or(false))
}

async fn app_shell_authenticated(ws: &mut Ws) -> Result<bool> {
    let js = r#"(()=>{const visible=e=>{const r=e.getBoundingClientRect();return r.width>0&&r.height>0};return ![...document.querySelectorAll('[data-testid="login-button"],[data-testid="signup-button"]')].some(visible)})()"#;
    Ok(eval(ws, js).await?.as_bool().unwrap_or(false))
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

async fn conversation_ids(ws: &mut Ws) -> Result<Vec<String>> {
    let js = r#"(()=>[...document.querySelectorAll('a[href*="/c/"]')]
        .map(e=>e.getAttribute('href')||'')
        .map(u=>u.match(/\/c\/(?:WEB:)?([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})/i)?.[1])
        .filter(Boolean))()"#;
    Ok(eval(ws, js)
        .await?
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}

async fn wait_for_cid(ws: &mut Ws, secs: u64, known: &[String]) -> Result<String> {
    let known = serde_json::to_string(known).unwrap_or_else(|_| "[]".into());
    let js = r#"(()=>{
        const uuid=/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})/i;
        const path=/\/c\/(?:WEB:)?([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})/i;
        const known=KNOWN;
        const direct=[...(window.__fxConversationIds||[]),location.href,
            // The new thread is inserted into the sidebar as soon as the first turn is
            // accepted. This is the most reliable signal on builds that stream through a
            // service worker (which hides the response body from page-level fetch hooks).
            ...[...document.querySelectorAll('a[href*="/c/"]')].map(e=>e.getAttribute('href')||''),
            ...[...document.querySelectorAll('a[href]')].map(e=>e.getAttribute('href')||'').filter(u=>path.test(u)),
            ...[...document.querySelectorAll('[data-conversation-id]')].map(e=>e.getAttribute('data-conversation-id')||''),
            ...performance.getEntriesByType('resource').map(e=>e.name).filter(u=>/\/(?:f\/)?conversation(?:\/|\?|$)/i.test(u)),
            ...performance.getEntriesByType('resource').map(e=>e.name).filter(u=>/\/backend-api\/conversation\//i.test(u))];
        for(const u of direct){const m=u.match(path)||u.match(uuid);if(m&&!known.includes(m[1]))return m[1];}
        // Some ChatGPT builds keep the newly-created id in serialized React state while the
        // address bar remains on `/`. Restrict the fallback to conversation-labelled fields;
        // scanning arbitrary markup returns unrelated UUIDs (feature flags, telemetry, etc.).
        const html=document.documentElement?.innerHTML||'';
        const labelled=html.match(/(?:conversation[_-]?id|conversationId)[^0-9a-f]{0,24}([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})/i);
        if(labelled && !known.includes(labelled[1]))return labelled[1];
        return null;
    })()"#
        .replace("KNOWN", &known);
    for _ in 0..secs {
        if let Some(c) = eval(ws, &js).await?.as_str() {
            return Ok(c.to_string());
        }
        sleep(Duration::from_secs(1)).await;
    }
    Err(Error::Timeout("chatgpt_web: no conversation id"))
}

/// Poll for the finished generated image and return its `src`. The thumbnails carry `alt=""`; the
/// rendered result's alt is `Generated image: <description>`, so match on that prefix.
async fn wait_for_image(
    ws: &mut Ws,
    secs: u64,
    baseline_images: u64,
    baseline_sources: &[String],
) -> Result<String> {
    let baseline_sources = serde_json::to_string(baseline_sources).unwrap_or_else(|_| "[]".into());
    let js = format!(
        r#"(()=>{{const old={baseline_sources};const all=[...document.querySelectorAll('img[alt^="Generated image"]')].filter(i=>i.complete&&i.naturalWidth>200&&i.src);const i=all.slice({baseline_images}).pop()||all.find(i=>!old.includes(i.src));
        return (i&&i.complete&&i.naturalWidth>200)?i.src:null;}})()"#
    );
    for _ in 0..secs {
        if let Some(s) = eval(ws, &js).await?.as_str() {
            return Ok(s.to_string());
        }
        sleep(Duration::from_secs(1)).await;
    }
    Err(Error::Timeout("chatgpt_web: no image"))
}

/// Fetch the rendered image inside the page (its session cookies satisfy the gate) and split the
/// resulting `data:<mime>;base64,<data>` URL into mime + b64. `eval` already awaits the promise.
async fn fetch_image(ws: &mut Ws, src: &str) -> Result<OutImage> {
    let s = serde_json::to_string(src).unwrap_or_default();
    let js = format!(
        r#"(async()=>{{
            const b=await fetch({s}).then(r=>r.blob());
            return await new Promise(res=>{{const fr=new FileReader();fr.onload=()=>res(fr.result);fr.readAsDataURL(b);}});
        }})()"#
    );
    let data = eval(ws, &js).await?;
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
        if eval(ws, cond).await?.as_bool() == Some(true) {
            return Ok(true);
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

/// Like `cmd` but optionally targets an attached (OOPIF) session by id. Responses carry the same
/// globally-unique `id`, so matching by id works regardless of which session answered.
async fn cmd_on(ws: &mut Ws, sess: Option<&str>, method: &str, params: Value) -> Result<Value> {
    let id = CDP_ID.fetch_add(1, Ordering::Relaxed);
    let mut frame = json!({ "id": id, "method": method, "params": params });
    if let Some(s) = sess {
        frame["sessionId"] = json!(s);
    }
    ws.send(Message::Text(frame.to_string().into())).await?;
    let result = timeout(Duration::from_secs(15), async {
        while let Some(f) = ws.next().await {
            if let Message::Text(txt) = f? {
                let msg: Value = serde_json::from_str(txt.as_str())?;
                if msg["id"].as_u64() == Some(id) {
                    if msg.get("error").is_some() {
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
        Err(_) => Err(Error::Timeout("chatgpt_web: cdp command")),
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

/// Save a screenshot of the headless page (debugging the driver).
#[allow(dead_code)]
async fn screenshot(ws: &mut Ws, path: &Path) -> Result<()> {
    let r = cmd(ws, "Page.captureScreenshot", json!({ "format": "png" })).await?;
    if let Some(d) = r["data"].as_str() {
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(d) {
            let _ = std::fs::write(path, bytes);
        }
    }
    Ok(())
}

async fn wait_for_page(port: u16) -> Result<String> {
    let http = reqwest::Client::new();
    for _ in 0..20 {
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
        body: "not logged in; run `fetchira login chatgpt_web`".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::norm;

    #[test]
    fn norm_matches_picker_labels() {
        assert_eq!(norm("High"), "high");
        assert_eq!(norm("GPT-5.4"), "gpt54");
        assert_eq!(norm("gpt-5.5"), "gpt55");
        assert_eq!(norm("o3"), "o3");
        // user input variants land on the same key as the live label
        assert_eq!(norm("GPT 5.4"), norm("GPT-5.4"));
    }
}
