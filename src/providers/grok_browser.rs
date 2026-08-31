use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use super::grok_web;
use super::{uuid4, LiveLimits};
use crate::error::{Error, Result};
use crate::web::{detect_browser, Cookie};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36";

fn headless_arg() -> Option<&'static str> {
    (std::env::var("FETCHIRA_BROWSER_HEADFUL").as_deref() != Ok("1")).then_some("--headless=new")
}

/// grok.com REST plan/limits through a real Chromium so WKE can run. HTTP-only polls 403
/// `second-factor-needed` until `.x.ai` `sso` exists; the page origin after a GET of
/// `/rest/subscriptions` is still grok.com, so same-origin POSTs for rate-limits work too.
pub async fn limits(cookies: &[Cookie]) -> Result<LiveLimits> {
    let browser = detect_browser()
        .ok_or_else(|| Error::Config("no Chrome/Chromium for grok_web browser mode".into()))?;
    let profile = std::env::temp_dir().join(format!("fetchira-grok-limits-{}", &uuid4()[..8]));
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
        .arg("--window-size=1280,800")
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
        let jar = cdp_cookies(cookies);
        let set = cmd(&mut ws, "Network.setCookies", json!({ "cookies": jar })).await?;
        if set.get("success").is_some_and(|v| v == false) {
            return Err(Error::BadResponse(
                "grok_web: browser rejected session cookies",
            ));
        }
        cmd(
            &mut ws,
            "Page.navigate",
            json!({"url": "https://accounts.x.ai/mfa/verify?redirect=grok-com&return_to=%2F"}),
        )
        .await?;
        sleep(Duration::from_secs(4)).await;
        cmd(
            &mut ws,
            "Page.navigate",
            json!({"url": "https://grok.com/rest/subscriptions"}),
        )
        .await?;
        sleep(Duration::from_secs(2)).await;
        let js = r#"(async()=>{
            const grok='https://grok.com';
            const grab=async(url,opt)=>{
                try{
                    const r=await fetch(url,{credentials:'include',signal:AbortSignal.timeout(15000),...opt});
                    return {status:r.status,text:await r.text()};
                }catch(e){return {status:0,text:String(e)};}
            };
            const sub=await grab(grok+'/rest/subscriptions');
            const models=['grok-4-auto','grok-4','grok-4-heavy'];
            const rl={};
            for (const m of models){
                rl[m]=await grab(grok+'/rest/rate-limits',{
                    method:'POST',
                    headers:{'content-type':'application/json'},
                    body:JSON.stringify({requestKind:'DEFAULT',modelName:m})
                });
            }
            return JSON.stringify({href:location.href,sub,rl});
        })()"#;
        let mut last = Value::Null;
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
            last = v.clone();
            if v["sub"]["status"].as_u64() == Some(200) {
                return parse_browser_limits(&v);
            }
            sleep(Duration::from_secs(2)).await;
        }
        eprintln!(
            "browser miss href={} sub_status={} sub_body={}",
            last["href"],
            last["sub"]["status"],
            last["sub"]["text"].as_str().unwrap_or("").chars().take(180).collect::<String>()
        );
        Err(Error::BadResponse(match last["sub"]["status"].as_u64() {
            Some(403) => "grok_web: browser still 403 second-factor",
            _ => "grok_web: browser limits rejected session",
        }))
    })
    .await
    .map_err(|_| Error::Timeout("grok_web: limits browser"));
    let _ = child.kill().await;
    let _ = child.wait().await;
    let _ = std::fs::remove_dir_all(&profile);
    result?
}

fn parse_browser_limits(v: &Value) -> Result<LiveLimits> {
    let sub_text = v["sub"]["text"].as_str().unwrap_or("");
    let sub_v: Value = serde_json::from_str(sub_text).unwrap_or(Value::Null);
    let sub = grok_web::parse_subscriptions(&sub_v);
    let q = |model: &str| {
        let text = v["rl"][model]["text"].as_str().unwrap_or("");
        let st = v["rl"][model]["status"].as_u64().unwrap_or(0);
        if st != 200 {
            return None;
        }
        grok_web::parse_rate_limit(&serde_json::from_str(text).unwrap_or(Value::Null)).ok()
    };
    grok_web::limits_from(
        Some(sub),
        None,
        q("grok-4-auto"),
        q("grok-4"),
        q("grok-4-heavy"),
    )
}

fn cdp_cookies(cookies: &[Cookie]) -> Vec<Value> {
    let mut out = Vec::new();
    for c in cookies {
        let mut v = json!({
            "name": c.name, "value": c.value,
            "path": c.path, "secure": c.secure, "httpOnly": c.http_only,
        });
        if !c.name.starts_with("__Host-") {
            v["domain"] = json!(c.domain);
        } else {
            v["url"] = json!("https://grok.com/");
        }
        if c.expires > 0.0 {
            v["expires"] = json!(c.expires);
        }
        out.push(v.clone());
        if (c.name == "sso" || c.name == "sso-rw") && c.domain.contains("grok.com") {
            let mut x = v;
            x["domain"] = json!(".x.ai");
            out.push(x);
        }
    }
    out
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
    let id = CDP_ID.fetch_add(1, Ordering::Relaxed);
    let mut req = json!({ "id": id, "method": method });
    if !params.is_null() {
        req["params"] = params;
    }
    ws.send(Message::Text(req.to_string().into())).await?;
    while let Some(frame) = ws.next().await {
        if let Message::Text(txt) = frame? {
            let msg: Value = serde_json::from_str(txt.as_str())?;
            if msg["id"].as_u64() == Some(id) {
                if msg.get("error").is_some() {
                    return Err(Error::BadResponse("grok_web"));
                }
                return Ok(msg["result"].clone());
            }
        }
    }
    Err(Error::BadResponse("grok_web"))
}

static CDP_ID: AtomicU64 = AtomicU64::new(1);

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
    Err(Error::Timeout("grok_web: chrome devtools"))
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(9222)
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    #[ignore]
    async fn live_browser_limits() {
        let path = std::env::var("GROK_SESSION").expect("GROK_SESSION");
        let raw = std::fs::read_to_string(path).expect("session");
        let sess = crate::web::parse_session(&raw);
        match super::limits(&sess.cookies).await {
            Ok(ll) => eprintln!(
                "browser limits tier={:?} models={}",
                ll.tier,
                ll.models
                    .iter()
                    .map(|m| format!("{}:{:?}/{:?}", m.name, m.remaining, m.total))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Err(e) => eprintln!("browser limits err {e}"),
        }
    }
}
