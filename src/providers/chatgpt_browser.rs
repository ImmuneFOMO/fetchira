use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::time::sleep;
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
// A normal-Chrome UA: the default `--headless` UA contains "HeadlessChrome", an instant Cloudflare tell.
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36";

pub async fn run(cookies: &[Cookie], cap: Capability, input: &Input) -> Result<Outcome> {
    let query = input.need_query()?.to_string();
    let search = matches!(cap, Capability::Search) && chatgpt_web::web_search_on(input);
    let browser = detect_browser()
        .ok_or_else(|| Error::Config("no Chrome/Chromium for chatgpt_web browser mode".into()))?;
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

    let out = drive(
        port,
        cookies,
        cap,
        input.model.as_deref(),
        search,
        input.session.as_deref(),
        input.file.as_deref(),
        &query,
    )
    .await;
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
    file: Option<&Path>,
    query: &str,
) -> Result<Outcome> {
    // Deep research has a plan step: kickoff drafts a plan (parked), then `dr|plan|<cid>` + "start"
    // approves it. A non-`dr|` session is a chat conversation to continue (dr polls go to HTTP).
    let dr_plan_cid = session.and_then(|s| s.strip_prefix("dr|plan|"));
    let approve =
        matches!(cap, Capability::DeepResearch) && dr_plan_cid.is_some() && is_start_word(query);
    let resume = session.filter(|s| !s.starts_with("dr|"));

    let ws_url = wait_for_page(port).await?;
    let (mut ws, _) = connect_async(ws_url.as_str()).await?;
    cmd(&mut ws, "Network.enable", json!({})).await?;
    cmd(&mut ws, "Page.enable", json!({})).await?;
    cmd(
        &mut ws,
        "Network.setCookies",
        json!({ "cookies": cdp_cookies(cookies) }),
    )
    .await?;
    let url = match (approve.then_some(dr_plan_cid).flatten(), resume) {
        (Some(cid), _) => format!("https://chatgpt.com/c/{cid}"),
        (None, Some(c)) => format!("https://chatgpt.com/c/{c}"),
        (None, None) => "https://chatgpt.com/".to_string(),
    };
    cmd(&mut ws, "Page.navigate", json!({ "url": url })).await?;

    // Wait for the composer to render (logged-in app); Cloudflare's interstitial clears first.
    let ok = wait_until(&mut ws, "!!document.querySelector('#prompt-textarea')", 45).await?;
    if std::env::var("CGPT_DEBUG").is_ok() {
        let _ = screenshot(&mut ws, Path::new("/tmp/cgpt-debug.png")).await;
        let url = eval(&mut ws, "location.href").await?;
        let title = eval(&mut ws, "document.title").await?;
        let cc = eval(&mut ws, "document.cookie.length").await?;
        eprintln!("CGPT_DEBUG composer={ok} url={url} title={title} cookie_len={cc}");
    }
    if !ok {
        return Err(login_err());
    }
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
        enable_tool(&mut ws, "create image").await?;
    } else if matches!(cap, Capability::DeepResearch) {
        enable_tool(&mut ws, "deep research").await?;
    } else if search {
        enable_tool(&mut ws, "web search").await?;
    }

    if let Some(path) = file {
        attach_file(&mut ws, path).await?;
    }

    if send_prompt(&mut ws, query).await? != "sent" {
        return Err(Error::BadResponse("chatgpt_web: composer drive failed"));
    }

    let cid = match resume {
        Some(c) => c.to_string(),
        None => wait_for_cid(&mut ws, 60).await?,
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
        let src = wait_for_image(&mut ws, IMAGE_WAIT).await?;
        let mut out = Outcome::new(String::new(), 1);
        out.image = Some(fetch_image(&mut ws, &src).await?);
        return Ok(out);
    }

    // Chat / web search: the turn finishes in seconds — wait, then read the clean message back.
    let deadline = Instant::now() + Duration::from_secs(CHAT_WAIT);
    loop {
        let conv = get_conversation(&mut ws, &cid).await?;
        if let Ok(mut out) = chatgpt_web::extract_answer_after(&conv, baseline) {
            out.session = Some(cid.clone());
            return Ok(out);
        }
        if Instant::now() >= deadline {
            return Err(Error::Timeout("chatgpt_web: no answer"));
        }
        sleep(Duration::from_secs(2)).await;
    }
}

fn cdp_cookies(cookies: &[Cookie]) -> Vec<Value> {
    cookies
        .iter()
        .map(|c| {
            let mut v = json!({
                "name": c.name, "value": c.value, "domain": c.domain,
                "path": c.path, "secure": c.secure, "httpOnly": c.http_only,
            });
            if c.expires > 0.0 {
                v["expires"] = json!(c.expires);
            }
            v
        })
        .collect()
}

/// Attach a local file to the composer over CDP (`DOM.setFileInputFiles` on the hidden file input),
/// then wait for the upload to settle — ChatGPT rejects a send while an attachment is still uploading.
async fn attach_file(ws: &mut Ws, path: &Path) -> Result<()> {
    let abs = path.to_string_lossy().to_string();
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
        json!({"files": [abs], "nodeId": node}),
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
    let q = serde_json::to_string(query).unwrap_or_else(|_| "\"\"".into());
    let js = format!(
        r#"(async()=>{{
            const ed=document.querySelector('#prompt-textarea');
            if(!ed) return 'no-editor';
            ed.focus();
            document.execCommand('insertText',false,{q});
            await new Promise(r=>setTimeout(r,500));
            const btn=document.querySelector('[data-testid="send-button"]')
                ||[...document.querySelectorAll('button')].find(b=>/send/i.test(b.getAttribute('aria-label')||''));
            if(!btn||btn.disabled) return 'no-send';
            btn.click();
            return 'sent';
        }})()"#
    );
    Ok(eval(ws, &js).await?.as_str().unwrap_or("err").to_string())
}

/// Open the composer "+" menu and click the tool whose label matches (e.g. "deep research"). React
/// portals ignore synthetic `.click()`, so we dispatch real CDP mouse events at element centers.
async fn enable_tool(ws: &mut Ws, tool: &str) -> Result<()> {
    let plus = r#"[...document.querySelectorAll('button')].find(b=>/add files|add photos|attach/i.test(b.getAttribute('aria-label')||''))"#;
    if let Some((x, y)) = center_of(ws, plus).await? {
        click_at(ws, x, y).await?;
        sleep(Duration::from_millis(800)).await;
    }
    let item = format!(
        r#"[...document.querySelectorAll('div,button,a')].find(e=>e.children.length<=2&&(e.textContent||'').trim().toLowerCase().startsWith({t}))"#,
        t = serde_json::to_string(tool).unwrap_or_default()
    );
    if let Some((x, y)) = center_of(ws, &item).await? {
        click_at(ws, x, y).await?;
        sleep(Duration::from_millis(500)).await;
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
    let js = format!(
        r#"(async()=>{{
            const t=(await fetch('/api/auth/session').then(r=>r.json())).accessToken;
            return await fetch('/backend-api/conversation/{cid}?include_visually_hidden_messages=true',
                {{headers:{{Authorization:'Bearer '+t}}}}).then(r=>r.text());
        }})()"#
    );
    let text = eval(ws, &js).await?;
    let s = text.as_str().unwrap_or("");
    serde_json::from_str(s).map_err(|_| Error::BadResponse("chatgpt_web"))
}

async fn wait_for_cid(ws: &mut Ws, secs: u64) -> Result<String> {
    for _ in 0..secs {
        if let Some(p) = eval(ws, "location.pathname").await?.as_str() {
            if let Some(c) = p.strip_prefix("/c/") {
                if c.len() >= 32 {
                    return Ok(c.to_string());
                }
            }
        }
        sleep(Duration::from_secs(1)).await;
    }
    Err(Error::Timeout("chatgpt_web: no conversation id"))
}

/// Poll for the finished generated image and return its `src`. The thumbnails carry `alt=""`; the
/// rendered result's alt is `Generated image: <description>`, so match on that prefix.
async fn wait_for_image(ws: &mut Ws, secs: u64) -> Result<String> {
    let js = r#"(()=>{const i=[...document.querySelectorAll('img[alt^="Generated image"]')].pop();
        return (i&&i.complete&&i.naturalWidth>200)?i.src:null;})()"#;
    for _ in 0..secs {
        if let Some(s) = eval(ws, js).await?.as_str() {
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
    for _ in 0..60 {
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
