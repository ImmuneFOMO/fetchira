use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::Connection;
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use wreq::cookie::Jar;
use wreq::Url;
use wreq_util::Emulation;

use crate::error::{Error, Result};
use crate::providers::ProviderKind;
use crate::proxy::split_auth;

/// A captured browser cookie. Field names match Chrome DevTools (camelCase) so the same
/// shape round-trips through CDP capture and the stored session JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    #[serde(default = "slash")]
    pub path: String,
    #[serde(default)]
    pub expires: f64,
    #[serde(default)]
    pub http_only: bool,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub session: bool,
}

fn slash() -> String {
    "/".into()
}

/// A captured web session: cookies plus any extra default headers to send with them. (grok's
/// `x-statsig-id` is not a cookie — it's generated per request in `providers::grok_web`.)
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Session {
    pub cookies: Vec<Cookie>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

/// Parse a stored session, accepting both the new `{cookies, headers}` object and the
/// original bare cookie array.
pub fn parse_session(raw: &str) -> Session {
    serde_json::from_str::<Session>(raw)
        .or_else(|_| {
            serde_json::from_str::<Vec<Cookie>>(raw).map(|cookies| Session {
                cookies,
                headers: BTreeMap::new(),
            })
        })
        .unwrap_or_default()
}

/// Name=value of every `Set-Cookie` in a response. NextAuth re-issues the session token on each
/// authed call; capturing + re-saving it keeps a cookie session from expiring.
pub fn set_cookie_updates(headers: &wreq::header::HeaderMap) -> Vec<(String, String)> {
    headers
        .get_all(wreq::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|c| {
            let (name, val) = c.split(';').next()?.split_once('=')?;
            Some((name.trim().to_string(), val.trim().to_string()))
        })
        .collect()
}

/// Build a Chrome-impersonating client with the captured cookies + headers (and optional
/// sticky proxy) baked in. The long timeout covers deep-research turns that run for minutes.
pub fn build_client(
    cookies: &[Cookie],
    headers: &BTreeMap<String, String>,
    proxy: Option<&str>,
) -> Result<wreq::Client> {
    let jar = Jar::default();
    for c in cookies {
        // Respect cookie-prefix rules or cookie_store silently drops these: `__Secure-`
        // requires Secure; `__Host-` requires Secure + Path=/ + no Domain.
        let host_only = c.name.starts_with("__Host-");
        let mut s = format!("{}={}; Path={}", c.name, c.value, c.path);
        if !host_only {
            s.push_str(&format!("; Domain={}", c.domain));
        }
        if c.secure || host_only || c.name.starts_with("__Secure-") {
            s.push_str("; Secure");
        }
        let site = format!("https://{}/", c.domain.trim_start_matches('.'));
        if let Ok(url) = site.parse::<Url>() {
            jar.add_cookie_str(&s, &url);
        }
    }
    let mut b = wreq::Client::builder()
        .emulation(Emulation::Chrome137)
        .cookie_provider(Arc::new(jar))
        .timeout(Duration::from_secs(300));
    if !headers.is_empty() {
        let mut hmap = wreq::header::HeaderMap::new();
        for (k, v) in headers {
            if let (Ok(name), Ok(val)) = (
                wreq::header::HeaderName::from_bytes(k.as_bytes()),
                wreq::header::HeaderValue::from_str(v),
            ) {
                hmap.insert(name, val);
            }
        }
        b = b.default_headers(hmap);
    }
    if let Some(p) = proxy {
        let (url, auth) = split_auth(p);
        let mut px = wreq::Proxy::all(&url)?;
        if let Some((u, pw)) = auth {
            px = px.basic_auth(&u, &pw);
        }
        b = b.proxy(px);
    }
    Ok(b.build()?)
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum BrowserKind {
    Chromium,
    Firefox,
}

impl BrowserKind {
    fn tag(self) -> &'static str {
        match self {
            BrowserKind::Chromium => "chrome",
            BrowserKind::Firefox => "firefox",
        }
    }
}

pub(crate) struct Browser {
    pub kind: BrowserKind,
    pub bin: PathBuf,
}

// Candidates are either absolute paths (checked as-is, macOS) or bare names (resolved on $PATH,
// Linux). Chrome is preferred because its CDP capture is proven; Firefox is the fallback.
const CHROMIUM_BINS: &[&str] = &[
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "brave-browser",
    "microsoft-edge",
    "/snap/bin/chromium",
];

const FIREFOX_BINS: &[&str] = &[
    "/Applications/Firefox.app/Contents/MacOS/firefox",
    "firefox",
    "firefox-esr",
    "/snap/bin/firefox",
];

fn find_bin(cands: &[&str]) -> Option<PathBuf> {
    cands.iter().find_map(|c| {
        if c.contains('/') {
            Some(PathBuf::from(c)).filter(|p| p.exists())
        } else {
            let path = std::env::var_os("PATH")?;
            std::env::split_paths(&path)
                .map(|d| d.join(c))
                .find(|p| p.is_file())
        }
    })
}

/// The preferred usable browser (Chrome, else Firefox). `FETCHIRA_BROWSER=chrome|firefox` pins one.
pub(crate) fn detect_browser() -> Option<Browser> {
    browser_candidates().into_iter().next()
}

// (url, cookie-domain, auth-cookie, optional page-eval that must be true for a *real* login —
// Google keeps its auth cookie while signed out, so the cookie alone isn't proof).
type LoginTarget = (
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
);

fn login_target(kind: ProviderKind) -> Result<LoginTarget> {
    Ok(match kind {
        // Google keeps `__Secure-1PSID` even when signed out (remembered account), so the cookie
        // alone isn't proof — gate on the page's `SNlM0e` token, which is empty until truly signed in
        // (and is exactly what the provider needs).
        ProviderKind::GeminiWeb => (
            "https://gemini.google.com/",
            "google.com",
            "__Secure-1PSID",
            Some(
                "(()=>{try{return!!(window.WIZ_global_data&&WIZ_global_data.SNlM0e&&\
                 WIZ_global_data.SNlM0e.length>0)}catch(e){return false}})()",
            ),
        ),
        ProviderKind::GrokWeb => ("https://grok.com/", "grok.com", "sso", None),
        ProviderKind::ChatgptWeb => (
            // NextAuth splits the large session token into `.0`/`.1` chunks — there is no
            // un-suffixed cookie, so wait for the first chunk as the "logged-in" signal.
            "https://chatgpt.com/",
            "chatgpt.com",
            "__Secure-next-auth.session-token.0",
            None,
        ),
        // Api-key providers whose live $ balance is only readable through the dashboard's cookie
        // session. NextAuth (`__Secure-next-auth.session-token`, present only when signed in — the
        // auth-wait also accepts the `.0` chunk when NextAuth splits it).
        ProviderKind::Parallel => (
            "https://platform.parallel.ai/home",
            "parallel.ai",
            "st", // platform session token (JWT); present only when signed in
            None,
        ),
        ProviderKind::Exa => (
            "https://dashboard.exa.ai/billing",
            "exa.ai",
            "next-auth.session-token", // NextAuth session JWT on .exa.ai; present only when signed in
            None,
        ),
        other => return Err(Error::Unsupported(other.as_str())),
    })
}

/// One browser profile per account label, so multiple accounts of the same provider can each be
/// logged into a different account (e.g. gemini-1 and gemini-2 as two different Google users).
fn profile_dir(home: &Path, tag: &str, label: &str) -> PathBuf {
    home.join(format!("{tag}-{label}"))
}

/// Browsers to try for login, in preference order: Chrome (default) then Firefox (fallback).
/// `FETCHIRA_BROWSER=chrome|firefox` pins one.
fn browser_candidates() -> Vec<Browser> {
    let chromium = || {
        find_bin(CHROMIUM_BINS).map(|bin| Browser {
            kind: BrowserKind::Chromium,
            bin,
        })
    };
    let firefox = || {
        find_bin(FIREFOX_BINS).map(|bin| Browser {
            kind: BrowserKind::Firefox,
            bin,
        })
    };
    match std::env::var("FETCHIRA_BROWSER").ok().as_deref() {
        Some("firefox" | "ff") => firefox().into_iter().collect(),
        Some("chrome" | "chromium") => chromium().into_iter().collect(),
        _ => [chromium(), firefox()].into_iter().flatten().collect(),
    }
}

/// Launch a real browser on this account's dedicated profile, let the user log in, and capture
/// the resulting cookie session once auth completes. Chrome is driven over CDP; Firefox (which
/// dropped CDP) is read straight from its plaintext `cookies.sqlite`. Chrome is the default; if its
/// capture fails (e.g. the DevTools socket resets on some builds) we fall back to Firefox.
pub async fn login(home: &Path, kind: ProviderKind, label: &str) -> Result<Session> {
    let candidates = browser_candidates();
    if candidates.is_empty() {
        return Err(Error::Config(
            "no Chrome/Chromium or Firefox found — install one, or attach a session manually \
             with `fetchira session <label>`"
                .into(),
        ));
    }
    let (url, domain, auth, check) = login_target(kind)?;
    let mut last_err = None;
    for browser in candidates {
        // Always start from an empty profile so `login` means "sign in and we capture this account",
        // not "silently re-grab whoever was left signed in" — the user picks the account each time.
        let profile = profile_dir(home, browser.kind.tag(), label);
        let _ = std::fs::remove_dir_all(&profile);
        let fut = async {
            match browser.kind {
                BrowserKind::Chromium => {
                    capture_chromium(&browser.bin, &profile, url, domain, auth, check).await
                }
                BrowserKind::Firefox => {
                    capture_firefox(&browser.bin, &profile, url, domain, auth).await
                }
            }
        };
        match timeout(Duration::from_secs(300), fut).await {
            Ok(Ok(session)) => return Ok(session),
            // A capture error (Chrome's DevTools socket resetting, a dead profile) — try the next
            // browser. A timeout means the user simply didn't finish, so don't switch on them.
            Ok(Err(e)) => {
                tracing::warn!(browser = ?browser.kind, error = %e, "browser login failed; trying next");
                last_err = Some(e);
            }
            Err(_) => return Err(Error::Timeout("login")),
        }
    }
    Err(last_err.unwrap_or(Error::Timeout("login")))
}

async fn capture_chromium(
    bin: &Path,
    profile: &Path,
    url: &str,
    domain: &str,
    auth: &str,
    login_check: Option<&str>,
) -> Result<Session> {
    // A free ephemeral port — 9222 collides with any other Chrome already exposing a debug port
    // (the user's main browser, an automation instance), which resets the CDP connection.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(9222);
    let mut child = tokio::process::Command::new(bin)
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--remote-debugging-port={port}"))
        .arg("--remote-allow-origins=*")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-logging")
        .arg("--log-level=3")
        // Without this Chrome binds Google's session to the device TPM key (DBSC), so the exported
        // cookies can't be replayed over HTTP — /app comes back logged-out and RotateCookies 401s.
        .arg("--disable-features=DeviceBoundSessionCredentials,StandardDeviceBoundSessionCredentials")
        .arg(format!("--app={url}"))
        // Chrome (and the GoogleUpdater it spawns) is noisy on stderr — keep it off the terminal.
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;

    let ws_url = wait_for_page(port).await?;
    let (ws, _) = tokio_tungstenite::connect_async(ws_url.as_str()).await?;
    let mut src = Cdp { ws, id: 1 };
    send_cmd(&mut src.ws, 1, "Network.enable", Value::Null).await?;
    // The auth cookie alone can be present while signed out (Google), so wait for a real login.
    if let Some(check) = login_check {
        wait_logged_in(&mut src, check).await?;
    }
    // Capture first (on the stable signed-in page), then swap the page for our confirmation so the
    // user isn't left staring at the provider UI while the window closes.
    let session = capture(&mut src, domain, auth).await;
    show_done(&mut src).await;
    // Capture done → close the window ourselves so the user isn't left with a stray browser (and
    // tempted to close it mid-capture). Browser.close quits cleanly; kill is the backstop.
    let _ = timeout(
        Duration::from_secs(3),
        send_cmd(&mut src.ws, src.id + 1, "Browser.close", Value::Null),
    )
    .await;
    let _ = child.kill().await;
    session
}

/// Replace the current page with a fetchira "login done" confirmation (best-effort). Overwriting the
/// document leaves cookies untouched, so the capture that follows still sees the session.
async fn show_done(src: &mut Cdp) {
    let js = r#"document.open();document.write('<meta name=viewport content="width=device-width,initial-scale=1"><body style="margin:0;height:100vh;display:flex;align-items:center;justify-content:center;background:#0b0b0d;color:#e8e8ea;font-family:system-ui,-apple-system,sans-serif"><div style="text-align:center"><div style="font-size:64px;line-height:1;color:#3ddc84">&#10003;</div><h2 style="margin:16px 0 6px;font-weight:600">fetchira &mdash; login captured</h2><p style="margin:0;opacity:.6">You can close this window.</p></div></body>');document.close();"#;
    src.id += 1;
    let _ = send_cmd(
        &mut src.ws,
        src.id,
        "Runtime.evaluate",
        json!({ "expression": js }),
    )
    .await;
}

/// Poll the page until `check` (a JS expression) evaluates true — a real signed-in state, since
/// some providers keep an auth cookie while signed out. The caller's outer timeout bounds the wait.
async fn wait_logged_in(src: &mut Cdp, check: &str) -> Result<()> {
    src.id += 1;
    let _ = send_cmd(&mut src.ws, src.id, "Runtime.enable", Value::Null).await;
    loop {
        src.id += 1;
        // Tolerate transient eval errors: the page reloads several times during sign-in and the JS
        // context is briefly gone. The caller's timeout bounds the overall wait.
        if let Ok(res) = send_cmd(
            &mut src.ws,
            src.id,
            "Runtime.evaluate",
            json!({ "expression": check, "returnByValue": true }),
        )
        .await
        {
            if res["result"]["value"].as_bool() == Some(true) {
                return Ok(());
            }
        }
        sleep(Duration::from_secs(1)).await;
    }
}

async fn capture_firefox(
    bin: &Path,
    profile: &Path,
    url: &str,
    domain: &str,
    auth: &str,
) -> Result<Session> {
    std::fs::create_dir_all(profile).ok();
    let mut child = tokio::process::Command::new(bin)
        .arg("--no-remote")
        .arg("--new-instance")
        .arg("--profile")
        .arg(profile)
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;

    let mut src = MozDb {
        path: profile.join("cookies.sqlite"),
    };
    let session = capture(&mut src, domain, auth).await;
    let _ = child.kill().await;
    session
}

/// A backend that returns the cookies currently scoped to `domain`. CDP polls Chrome; the
/// SQLite reader tails Firefox's profile.
trait CookieSource {
    async fn fetch(&mut self, domain: &str) -> Result<Vec<Cookie>>;
}

/// Two-phase capture shared by both backends: wait for the provider's auth cookie, then hold the
/// fullest set until it stops growing (companions like Google's `__Secure-1PSIDTS` land a beat
/// after the auth cookie). Caller wraps this in a timeout.
async fn capture<S: CookieSource>(src: &mut S, domain: &str, auth: &str) -> Result<Session> {
    // NextAuth splits large session tokens into `<name>.0`/`.1`, so accept the first chunk too.
    let chunk = format!("{auth}.0");
    let mut best = loop {
        let scoped = src.fetch(domain).await?;
        if scoped
            .iter()
            .any(|c| is_auth(c, auth) || is_auth(c, &chunk))
        {
            break scoped;
        }
        sleep(Duration::from_secs(1)).await;
    };
    let mut stable = 0;
    for _ in 0..8 {
        sleep(Duration::from_secs(1)).await;
        let scoped = src.fetch(domain).await?;
        if scoped.len() > best.len() {
            best = scoped;
            stable = 0;
        } else {
            stable += 1;
            if stable >= 2 {
                break;
            }
        }
    }
    Ok(Session {
        cookies: best,
        headers: BTreeMap::new(),
    })
}

struct Cdp {
    ws: Ws,
    id: u64,
}

impl CookieSource for Cdp {
    async fn fetch(&mut self, domain: &str) -> Result<Vec<Cookie>> {
        self.id += 1;
        let res = send_cmd(&mut self.ws, self.id, "Network.getAllCookies", Value::Null).await?;
        let all: Vec<Cookie> = serde_json::from_value(res["cookies"].clone()).unwrap_or_default();
        Ok(all
            .into_iter()
            .filter(|c| dom_match(&c.domain, domain))
            .collect())
    }
}

struct MozDb {
    path: PathBuf,
}

/// Firefox stores cookie values in plaintext (unlike Chrome), so we read `cookies.sqlite`
/// directly. Read-only honours the live profile's WAL; transient open/lock errors and a
/// not-yet-created file just mean "no cookies yet", so the caller keeps polling.
impl CookieSource for MozDb {
    async fn fetch(&mut self, domain: &str) -> Result<Vec<Cookie>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let opts = SqliteConnectOptions::new()
            .filename(&self.path)
            .read_only(true)
            .busy_timeout(Duration::from_secs(2));
        let mut conn = match sqlx::SqliteConnection::connect_with(&opts).await {
            Ok(c) => c,
            Err(_) => return Ok(Vec::new()),
        };
        let rows = sqlx::query_as::<_, MozCookie>(
            "SELECT COALESCE(name,'') AS name, COALESCE(value,'') AS value, \
             COALESCE(host,'') AS host, COALESCE(path,'/') AS path, \
             COALESCE(expiry,0) AS expiry, COALESCE(isSecure,0) AS is_secure, \
             COALESCE(isHttpOnly,0) AS is_http_only FROM moz_cookies",
        )
        .fetch_all(&mut conn)
        .await
        .unwrap_or_default();
        conn.close().await.ok();
        Ok(rows
            .into_iter()
            .map(Cookie::from)
            .filter(|c| dom_match(&c.domain, domain))
            .collect())
    }
}

#[derive(sqlx::FromRow)]
struct MozCookie {
    name: String,
    value: String,
    host: String,
    path: String,
    expiry: i64,
    is_secure: i64,
    is_http_only: i64,
}

impl From<MozCookie> for Cookie {
    fn from(m: MozCookie) -> Self {
        Cookie {
            name: m.name,
            value: m.value,
            domain: m.host,
            path: m.path,
            expires: m.expiry as f64,
            http_only: m.is_http_only != 0,
            secure: m.is_secure != 0,
            session: m.expiry == 0,
        }
    }
}

fn is_auth(c: &Cookie, auth: &str) -> bool {
    c.name == auth && !c.value.is_empty() && (c.session || c.expires <= 0.0 || c.expires > now())
}

/// Poll the DevTools HTTP endpoint for a page target and return its WebSocket URL.
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

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Send one CDP command and read frames until the matching id; events (no id) are skipped.
async fn send_cmd(ws: &mut Ws, id: u64, method: &str, params: Value) -> Result<Value> {
    let mut req = json!({ "id": id, "method": method });
    if !params.is_null() {
        req["params"] = params;
    }
    ws.send(Message::Text(req.to_string().into())).await?;
    while let Some(frame) = ws.next().await {
        if let Message::Text(txt) = frame? {
            let msg: Value = serde_json::from_str(txt.as_str())?;
            if msg["id"].as_u64() == Some(id) {
                return Ok(msg["result"].clone());
            }
        }
    }
    Err(Error::BadResponse("cdp connection closed"))
}

fn dom_match(cookie_domain: &str, want: &str) -> bool {
    let c = cookie_domain.trim_start_matches('.');
    c == want || c.ends_with(&format!(".{want}"))
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_bin_resolves_paths_and_path_names() {
        // Bare names resolve on $PATH ("sh" exists on any unix runner); the first hit wins.
        assert!(find_bin(&["fetchira-no-such-browser-xyz", "sh"]).is_some());
        // Absolute candidates are checked as-is; missing ones are skipped.
        assert!(find_bin(&["/no/such/path", "/bin/sh"]).is_some());
        assert!(find_bin(&["/no/such/path"]).is_none());
    }

    #[tokio::test]
    async fn firefox_reads_scoped_cookies() {
        let dir = std::env::temp_dir().join(format!("fetchira-moz-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("cookies.sqlite");

        let opts = SqliteConnectOptions::new()
            .filename(&db)
            .create_if_missing(true);
        let mut conn = sqlx::SqliteConnection::connect_with(&opts).await.unwrap();
        sqlx::query(
            "CREATE TABLE moz_cookies (id INTEGER PRIMARY KEY, name TEXT, value TEXT, host TEXT, \
             path TEXT, expiry INTEGER, isSecure INTEGER, isHttpOnly INTEGER, sameSite INTEGER)",
        )
        .execute(&mut conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO moz_cookies (name,value,host,path,expiry,isSecure,isHttpOnly,sameSite) \
             VALUES ('sso','tok','.grok.com','/',9999999999,1,1,0), \
                    ('junk','x','example.com','/',0,0,0,0)",
        )
        .execute(&mut conn)
        .await
        .unwrap();
        conn.close().await.unwrap();

        let cookies = MozDb { path: db }.fetch("grok.com").await.unwrap();
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(cookies.len(), 1);
        let c = &cookies[0];
        assert_eq!(
            (c.name.as_str(), c.value.as_str(), c.domain.as_str()),
            ("sso", "tok", ".grok.com")
        );
        assert!(c.secure && c.http_only);
        assert!(is_auth(c, "sso"));
    }
}
