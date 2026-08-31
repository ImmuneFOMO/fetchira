use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine;
use futures_util::StreamExt;
use regex::Regex;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use wreq_util::Emulation;

use crate::error::{Error, Result};

// grok's anti-bot `x-statsig-id`, reversed from its web bundle. A 70-byte token:
//   base64( xor_nonce( [nonce] + seed(48) + e_le(4) + sha256("METHOD!path!e" + SALT + l)[..16] + [3] ) )
// `seed` is server-issued (a `<meta name^="gr">` tag, fresh per page load) and `l` is a render
// fingerprint computed from `seed` + a per-build `curves` table embedded in the page. We scrape both
// once with the authed client, then mint tokens offline. `l`'s five seed-byte indices live in the
// generator JS chunk and rotate per build, so on a 403 we drop the cache and re-scrape.

// Lifted verbatim from grok's generator bundle: the SHA-256 message salt, the token's trailing
// version byte, and the epoch its timestamp counts from (1682924400 = 2023-05-01T07:00:00Z).
const SALT: &str = "obfiowerehiring";
const VERSION: u8 = 3;
const EPOCH: u64 = 1_682_924_400;

// The five seed-byte positions the generator reads to derive `l`; `curves[group][segment]` picks a
// curve, the three `ct` bytes set the animation time. Scraped per build (see `scrape_indices`).
#[derive(Clone)]
struct Indices {
    group: usize,
    segment: usize,
    ct: [usize; 3],
}

// `curves[group][segment]` = `[color×6, deg, bezier×4]`, the per-build table embedded in the page.
type Curves = Vec<Vec<[f64; 11]>>;

/// A build's minting state: a server-issued `seed` plus its computed render fingerprint `l`. One
/// value mints many tokens (fresh nonce + timestamp each) until grok rotates the build or seed.
#[derive(Clone)]
pub struct Statsig {
    seed: Vec<u8>,
    l: String,
}

impl Statsig {
    /// `x-statsig-id` for one request. `path` is the pathname only (no query, no host).
    pub fn token(&self, method: &str, path: &str) -> String {
        let e = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .saturating_sub(EPOCH) as u32;
        assemble(&self.seed, &self.l, method, path, e, nonce())
    }
}

// Seed/curves/l are build-global (not tied to an account's cookies), so one cached value serves
// every grok account. Cleared on a 403 to force a re-scrape against a rotated build/seed.
static STATE: LazyLock<Mutex<Option<Statsig>>> = LazyLock::new(|| Mutex::new(None));

/// The current build's statsig state, scraped (and cached) on first use via the authed client. The
/// lock is held across the scrape so a burst of concurrent calls triggers exactly one (the rest
/// reuse its result) rather than each running the chunk scan.
pub async fn current(base: &str, client: &wreq::Client) -> Result<Statsig> {
    let mut state = STATE.lock().await;
    if let Some(s) = state.as_ref() {
        return Ok(s.clone());
    }
    let s = scrape(base, client).await?;
    *state = Some(s.clone());
    Ok(s)
}

/// Drop the cached state so the next call re-scrapes — used after a 403 (rotated build or seed).
pub async fn invalidate() {
    *STATE.lock().await = None;
}

async fn scrape(base: &str, client: &wreq::Client) -> Result<Statsig> {
    let bad = || Error::BadResponse("grok_web");
    let html = client.get(base).send().await?.text().await?;
    if extract_seed(&html).is_some() && parse_curves(&html).is_some() {
        return scrape_html(client, &html).await;
    }
    // A logged-in grok.com session 307s to accounts.x.ai/mfa; the public homepage still has the seed.
    let anon = wreq::Client::builder()
        .emulation(Emulation::Chrome137)
        .build()
        .map_err(|_| bad())?;
    let html = anon.get(base).send().await?.text().await?;
    scrape_html(&anon, &html).await
}

async fn scrape_html(client: &wreq::Client, html: &str) -> Result<Statsig> {
    let bad = || Error::BadResponse("grok_web");
    let seed = STANDARD
        .decode(extract_seed(html).ok_or_else(bad)?)
        .map_err(|_| bad())?;
    let curves = parse_curves(html).ok_or_else(bad)?;
    if seed.len() != 48 || curves.len() != 4 || curves.iter().any(|r| r.len() != 16) {
        return Err(bad());
    }
    let idx = scrape_indices(html, client).await.ok_or_else(bad)?;
    let l = compute_l(&seed, &curves, &idx);
    Ok(Statsig { seed, l })
}

// The five seed indices live in the generator chunk, which is lazy-loaded under a per-build hash
// with no stable name. The homepage's chunks have no naming signal, so we fetch them until one
// turns out to be the consumer that imports the generator (module id `1645e3`, stable across
// builds), follow it to the generator chunk, and read the indices there.
async fn scrape_indices(html: &str, client: &wreq::Client) -> Option<Indices> {
    let urls = chunk_urls(html);
    let cdn = urls.first()?.split_once("static/chunks/")?.0.to_string();
    let mut bodies = futures_util::stream::iter(urls.into_iter().map(|url| {
        let client = client.clone();
        async move { client.get(&url).send().await.ok()?.text().await.ok() }
    }))
    .buffer_unordered(16);
    while let Some(body) = bodies.next().await {
        let Some(chunk) = body.as_deref().and_then(generator_chunk) else {
            continue;
        };
        let src = client
            .get(format!("{cdn}{chunk}"))
            .send()
            .await
            .ok()?
            .text()
            .await
            .ok()?;
        return parse_indices(&src);
    }
    None
}

fn chunk_urls(html: &str) -> Vec<String> {
    let re = match Regex::new(r#"https://[^"]+?/static/chunks/[^"]+?\.js"#) {
        Ok(re) => re,
        Err(_) => return Vec::new(),
    };
    let mut seen = std::collections::HashSet::new();
    re.find_iter(html)
        .map(|m| m.as_str().to_string())
        .filter(|u| seen.insert(u.clone()))
        .collect()
}

fn nonce() -> u8 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static C: AtomicU64 = AtomicU64::new(0);
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    (t ^ C.fetch_add(0x9E37_79B9, Ordering::Relaxed)) as u8
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

// Port of V8's `DoubleToRadixCString` for radix 16 (`Number.prototype.toString(16)`). Inputs are
// pre-rounded to 2 decimals, so the shortest-round-trip output matches the browser byte-for-byte.
fn js_tostring16(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }
    let neg = value.is_sign_negative();
    let value = value.abs();
    let mut integer = value.floor();
    let mut fraction = value - integer;
    let mut delta = (0.5 * (value.next_up() - value)).max(0.0_f64.next_up());
    let mut frac: Vec<u8> = Vec::new();
    if fraction >= delta {
        loop {
            fraction *= 16.0;
            delta *= 16.0;
            let digit = fraction as u8;
            frac.push(digit);
            fraction -= digit as f64;
            if (fraction > 0.5 || (fraction == 0.5 && (digit & 1) == 1)) && fraction + delta > 1.0 {
                loop {
                    match frac.pop() {
                        None => {
                            integer += 1.0;
                            break;
                        }
                        Some(d) => {
                            if d + 1 < 16 {
                                frac.push(d + 1);
                                break;
                            }
                        }
                    }
                }
                break;
            }
            if fraction < delta {
                break;
            }
        }
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    out.push_str(&format!("{:x}", integer as u64));
    if !frac.is_empty() {
        out.push('.');
        out.extend(frac.iter().map(|&d| HEX[d as usize] as char));
    }
    out
}

// WebKit/Chrome UnitBezier solver (endpoints 0,0 and 1,1): 8-iter Newton, bisection fallback.
fn bezier(x1: f64, y1: f64, x2: f64, y2: f64, x: f64) -> f64 {
    let cx = 3.0 * x1;
    let bx = 3.0 * (x2 - x1) - cx;
    let ax = 1.0 - cx - bx;
    let cy = 3.0 * y1;
    let by = 3.0 * (y2 - y1) - cy;
    let ay = 1.0 - cy - by;
    let sx = |t: f64| ((ax * t + bx) * t + cx) * t;
    let sdx = |t: f64| (3.0 * ax * t + 2.0 * bx) * t + cx;
    let solve = |x: f64| -> f64 {
        let mut t = x;
        for _ in 0..8 {
            let e = sx(t) - x;
            if e.abs() < 1e-6 {
                return t;
            }
            let d = sdx(t);
            if d.abs() < 1e-6 {
                break;
            }
            t -= e / d;
        }
        let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
        let mut t = x;
        if t < lo {
            return lo;
        }
        if t > hi {
            return hi;
        }
        while lo < hi {
            let xv = sx(t);
            if (xv - x).abs() < 1e-6 {
                return t;
            }
            if x > xv {
                lo = t;
            } else {
                hi = t;
            }
            t = (hi - lo) / 2.0 + lo;
        }
        t
    };
    let t = solve(x);
    ((ay * t + by) * t + cy) * t
}

fn compute_l(seed: &[u8], curves: &Curves, idx: &Indices) -> String {
    let a = &curves[seed[idx.group] as usize % 4][seed[idx.segment] as usize % 16];
    let ct = (((seed[idx.ct[0]] % 16) as f64
        * (seed[idx.ct[1]] % 16) as f64
        * (seed[idx.ct[2]] % 16) as f64)
        / 10.0)
        .round()
        * 10.0;
    let p = ct / 4096.0;
    let ef = bezier(
        round2(a[7] / 255.0),
        round2(a[8] * 2.0 / 255.0 - 1.0),
        round2(a[9] / 255.0),
        round2(a[10] * 2.0 / 255.0 - 1.0),
        p,
    );
    let r = (a[0] + (a[3] - a[0]) * ef).round();
    let g = (a[1] + (a[4] - a[1]) * ef).round();
    let b = (a[2] + (a[5] - a[2]) * ef).round();
    let deg = (a[6] * 300.0 / 255.0 + 60.0).floor();
    let ang = deg * ef * std::f64::consts::PI / 180.0;
    let (cos, sin) = (ang.cos(), ang.sin());
    let nums = [r, g, b, cos, sin, -sin, cos, 0.0, 0.0];
    let mut out = String::new();
    for &x in &nums {
        out.push_str(&js_tostring16(round2(x)));
    }
    out.chars().filter(|c| *c != '.' && *c != '-').collect()
}

fn assemble(seed: &[u8], l: &str, method: &str, path: &str, e: u32, nonce: u8) -> String {
    let msg = format!("{method}!{path}!{e}{SALT}{l}");
    let h = Sha256::digest(msg.as_bytes());
    let mut plain = Vec::with_capacity(70);
    plain.push(nonce);
    plain.extend_from_slice(seed);
    plain.extend_from_slice(&e.to_le_bytes());
    plain.extend_from_slice(&h[..16]);
    plain.push(VERSION);
    for b in plain.iter_mut().skip(1) {
        *b ^= nonce;
    }
    STANDARD_NO_PAD.encode(&plain)
}

fn extract_seed(html: &str) -> Option<&str> {
    let after = html.split("name=\"grok-site").nth(1)?;
    let after = after.split("content=\"").nth(1)?;
    let end = after.find('"')?;
    Some(&after[..end])
}

fn parse_curves(html: &str) -> Option<Curves> {
    let key = html
        .find("curves\\\":[[")
        .or_else(|| html.find("curves\":[["))?;
    let start = html[key..].find("[[")? + key;
    let mut depth = 0i32;
    let mut end = start;
    for (i, &b) in html.as_bytes()[start..].iter().enumerate() {
        match b {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    end = start + i;
                    break;
                }
            }
            _ => (),
        }
    }
    let raw = html[start..=end].replace("\\\"", "\"");
    let v = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    let mut curves = Vec::with_capacity(v.as_array()?.len());
    for seg in v.as_array()? {
        let mut row = Vec::new();
        for o in seg.as_array()? {
            let color = o.get("color")?.as_array()?;
            let bez = o.get("bezier")?.as_array()?;
            row.push([
                color.first()?.as_f64()?,
                color.get(1)?.as_f64()?,
                color.get(2)?.as_f64()?,
                color.get(3)?.as_f64()?,
                color.get(4)?.as_f64()?,
                color.get(5)?.as_f64()?,
                o.get("deg")?.as_f64()?,
                bez.first()?.as_f64()?,
                bez.get(1)?.as_f64()?,
                bez.get(2)?.as_f64()?,
                bez.get(3)?.as_f64()?,
            ]);
        }
        curves.push(row);
    }
    Some(curves)
}

fn generator_chunk(src: &str) -> Option<String> {
    let marker = src.find("t(1645e3)").or_else(|| src.find("t(1645000)"))?;
    let arr = src[..marker].rfind("Promise.all([")? + "Promise.all([".len();
    let end = src[arr..].find(']')? + arr;
    let chunk = Regex::new(r#"static/chunks/[^\\"]+?\.js"#)
        .ok()?
        .find(&src[arr..end])?;
    Some(chunk.as_str().to_string())
}

fn parse_indices(src: &str) -> Option<Indices> {
    let mods = Regex::new(r"\(\w\[(\d+)\],\s*16\)").ok()?;
    let mut it = mods.captures_iter(src).map(|c| c[1].parse::<usize>());
    let segment = it.next()?.ok()?;
    let ct = [it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?];
    let group = Regex::new(r"\[\w\[(\d+)\]\s*%\s*4\]").ok()?.captures(src)?[1]
        .parse::<usize>()
        .ok()?;
    Some(Indices { group, segment, ct })
}

#[cfg(test)]
mod tests {
    use super::*;

    // A synthetic vector cross-checked against grok's JS generator (the same generator verified to
    // reproduce real server-accepted tokens): a 1×1 curve + a crafted seed whose indices select it,
    // pinning compute_l (bezier + color + rotate + toString16) and assemble (sha256 + xor + base64).
    #[test]
    fn compute_l_and_assemble() {
        let curves = vec![vec![[
            219.0, 4.0, 240.0, 196.0, 212.0, 140.0, 153.0, 210.0, 204.0, 247.0, 242.0,
        ]]];
        let idx = Indices {
            group: 0,
            segment: 1,
            ct: [2, 3, 4],
        };
        let mut seed = vec![0u8; 48];
        seed[2] = 3;
        seed[3] = 5;
        seed[4] = 7;
        for (i, b) in seed.iter_mut().enumerate().skip(5) {
            *b = i as u8;
        }
        let l = compute_l(&seed, &curves, &idx);
        assert_eq!(l, "db8ee10147ae147ae147b0147ae147ae147b100");
        assert_eq!(
            assemble(&seed, &l, "POST", "/rest/x", 42_000_000, 99),
            "Y2NjYGZkZmVka2ppaG9ubWxzcnFwd3Z1dHt6eXh/fn18Q0JBQEdGRURLSklIT05NTOO942FVpSTMZ29NGYcuCxeTOirDYA"
        );
    }

    #[test]
    fn tostring16_matches_v8() {
        // Number(Number(x).toFixed(2)).toString(16) per node v22.
        for (x, want) in [
            (0.0, "0"),
            (255.0, "ff"),
            (128.0, "80"),
            (1.0, "1"),
            (0.5, "0.8"),
            (-0.5, "-0.8"),
            (0.87, "0.deb851eb851eb8"),
            (0.07, "0.11eb851eb851ec"),
            (0.99, "0.fd70a3d70a3d7"),
            (0.01, "0.028f5c28f5c28f6"),
            (-0.01, "-0.028f5c28f5c28f6"),
            (0.33, "0.547ae147ae147c"),
        ] {
            assert_eq!(js_tostring16(round2(x)), want, "x={x}");
        }
    }

    #[test]
    fn parses_indices_both_builds() {
        let old = "f(n[24],16)*g(n[17],16)*h(n[8],16)*k(n[11],16);x=q[n[5]%4]";
        assert_eq!(
            parse_indices(old).map(|i| (i.group, i.segment, i.ct)),
            Some((5, 24, [17, 8, 11]))
        );
        let new = "f(n[47],16)*g(n[31],16)*h(n[8],16)*k(n[14],16);x=q[n[5]%4]";
        assert_eq!(
            parse_indices(new).map(|i| (i.group, i.segment, i.ct)),
            Some((5, 47, [31, 8, 14]))
        );
    }

    #[test]
    fn extracts_seed_and_chunks() {
        let html = r#"<meta name="grok-site verification" content="QUJD"/><script src="https://cdn.grok.com/_next/static/chunks/0ab.js"></script><script src="https://cdn.grok.com/_next/static/chunks/0ab.js"></script><script src="https://cdn.grok.com/_next/static/chunks/0cd.js">"#;
        assert_eq!(extract_seed(html), Some("QUJD"));
        assert_eq!(
            chunk_urls(html),
            [
                "https://cdn.grok.com/_next/static/chunks/0ab.js",
                "https://cdn.grok.com/_next/static/chunks/0cd.js"
            ]
        );
    }
}
