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

use super::{chatgpt_web, uuid4, Capability, Input, Outcome};
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

    // A non-`dr|` session is a chat conversation id to continue (the router sends dr polls to HTTP).
    let resume = input.session.as_deref().filter(|s| !s.starts_with("dr|"));
    let out = drive(
        port,
        cookies,
        cap,
        input.model.as_deref(),
        search,
        resume,
        &query,
    )
    .await;
    let _ = child.kill().await;
    let _ = std::fs::remove_dir_all(&profile);
    out
}

async fn drive(
    port: u16,
    cookies: &[Cookie],
    cap: Capability,
    model: Option<&str>,
    search: bool,
    resume: Option<&str>,
    query: &str,
) -> Result<Outcome> {
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
    let url = match resume {
        Some(c) => format!("https://chatgpt.com/c/{c}"),
        None => "https://chatgpt.com/".to_string(),
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

    if send_prompt(&mut ws, query).await? != "sent" {
        return Err(Error::BadResponse("chatgpt_web: composer drive failed"));
    }

    let cid = match resume {
        Some(c) => c.to_string(),
        None => wait_for_cid(&mut ws, 60).await?,
    };

    // Deep research runs for minutes; the browser only kicks it off. Polling is a plain GET (not
    // gated), so the router resumes the `dr|poll|<cid>` session through the HTTP path.
    if matches!(cap, Capability::DeepResearch) {
        let mut out = Outcome::new(
            "Deep research started. Call deep_research again with this session to fetch the report \
             when ready (~5-30 min)."
                .into(),
            1,
        );
        out.session = Some(format!("dr|poll|{cid}"));
        return Ok(out);
    }

    // Create image: wait for the rendered result and return its (session-scoped chatgpt.com) URL.
    if matches!(cap, Capability::Image) {
        let url = wait_for_image(&mut ws, IMAGE_WAIT).await?;
        return Ok(Outcome::new(format!("![generated image]({url})"), 1));
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
            const label=(e.textContent||'').trim();if(!label)continue;
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
/// rendered result is the `alt="Generated image"` element once it has decoded.
async fn wait_for_image(ws: &mut Ws, secs: u64) -> Result<String> {
    let js = r#"(()=>{const i=document.querySelector('img[alt="Generated image"]');
        return (i&&i.complete&&i.naturalWidth>200)?i.src:null;})()"#;
    for _ in 0..secs {
        if let Some(s) = eval(ws, js).await?.as_str() {
            return Ok(s.to_string());
        }
        sleep(Duration::from_secs(1)).await;
    }
    Err(Error::Timeout("chatgpt_web: no image"))
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
    static ID: AtomicU64 = AtomicU64::new(1);
    let id = ID.fetch_add(1, Ordering::Relaxed);
    ws.send(Message::Text(
        json!({ "id": id, "method": method, "params": params })
            .to_string()
            .into(),
    ))
    .await?;
    while let Some(frame) = ws.next().await {
        if let Message::Text(txt) = frame? {
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
