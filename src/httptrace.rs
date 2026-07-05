use std::cell::RefCell;

use serde_json::{json, Value};

tokio::task_local! {
    static TRACE: RefCell<Vec<Value>>;
}

// Request bodies are usually a small JSON payload; response bodies (a scrape, a search page) run
// larger, so they get a roomier cap before truncation.
const REQ_BODY_CAP: usize = 4 * 1024;
const RESP_BODY_CAP: usize = 8 * 1024;

/// Run `fut` with an active trace buffer, returning its output plus every round-trip that
/// `send_traced` recorded inside it. Calls made outside a `capture` scope are simply not traced.
pub async fn capture<F: std::future::Future>(fut: F) -> (F::Output, Vec<Value>) {
    TRACE
        .scope(RefCell::new(Vec::new()), async move {
            let out = fut.await;
            (out, TRACE.with(|t| t.take()))
        })
        .await
}

/// Drop-in for `builder.send()` that records the raw request + response (redacting secret headers)
/// into the current `capture` scope, then rebuilds the `Response` so downstream `.json()`/`.text()`
/// still consume the body.
pub async fn send_traced(builder: reqwest::RequestBuilder) -> reqwest::Result<reqwest::Response> {
    let (client, req) = builder.build_split();
    let req = req?;

    let method = req.method().to_string();
    let url = req.url().to_string();
    let req_headers = header_json(req.headers(), true);
    let req_body = req
        .body()
        .and_then(|b| b.as_bytes())
        .map(|b| clip(&String::from_utf8_lossy(b), REQ_BODY_CAP));

    let resp = client.execute(req).await?;
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.bytes().await?;

    TRACE
        .try_with(|t| {
            t.borrow_mut().push(json!({
                "method": method,
                "url": url,
                "reqHeaders": req_headers,
                "reqBody": req_body,
                "status": status.as_u16(),
                "respHeaders": header_json(&headers, false),
                "respBody": clip(&String::from_utf8_lossy(&bytes), RESP_BODY_CAP),
            }));
        })
        .ok();

    let mut rebuilt = http::Response::new(reqwest::Body::from(bytes));
    *rebuilt.status_mut() = status;
    *rebuilt.headers_mut() = headers;
    Ok(reqwest::Response::from(rebuilt))
}

fn header_json(headers: &reqwest::header::HeaderMap, redact: bool) -> Value {
    let mut map = serde_json::Map::new();
    for (name, val) in headers {
        let v = if redact && is_secret(name.as_str()) {
            "‹redacted›".to_string()
        } else {
            val.to_str().unwrap_or("<binary>").to_string()
        };
        map.insert(name.as_str().to_string(), Value::String(v));
    }
    Value::Object(map)
}

fn is_secret(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "authorization"
        || n.contains("key")
        || n.contains("token")
        || n.contains("secret")
        || n.contains("cookie")
}

/// Truncate on a char boundary so a large body can't bloat a trace, marking what was dropped.
fn clip(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    let mut end = cap;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[+{} bytes truncated]", &s[..end], s.len() - end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_headers_are_redacted() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("X-API-KEY", "sk-abc".parse().unwrap());
        h.insert("Authorization", "Bearer t".parse().unwrap());
        h.insert("Content-Type", "application/json".parse().unwrap());
        let j = header_json(&h, true);
        assert_eq!(j["x-api-key"], "‹redacted›");
        assert_eq!(j["authorization"], "‹redacted›");
        assert_eq!(j["content-type"], "application/json");
        // Response side never redacts.
        assert_eq!(header_json(&h, false)["x-api-key"], "sk-abc");
    }

    #[test]
    fn clip_marks_truncation_on_boundary() {
        let big = "𝓍".repeat(REQ_BODY_CAP); // 4 bytes each
        let out = clip(&big, REQ_BODY_CAP);
        assert!(out.contains("truncated"));
        assert!(out.starts_with('𝓍'));
        assert_eq!(clip("short", REQ_BODY_CAP), "short");
    }

    #[tokio::test]
    async fn capture_collects_only_inside_scope() {
        // Outside a scope: try_with is a no-op, nothing panics.
        TRACE.try_with(|t| t.borrow_mut().push(json!({}))).ok();

        let (out, traces) = capture(async {
            TRACE
                .try_with(|t| t.borrow_mut().push(json!({"n": 1})))
                .ok();
            42
        })
        .await;
        assert_eq!(out, 42);
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0]["n"], 1);
    }
}
