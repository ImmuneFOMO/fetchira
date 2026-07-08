use std::net::IpAddr;
use std::time::Duration;

use crate::config::ProxyPool;
use crate::error::{Error, Result};

/// Resolve the proxy pool to a list of `http://user:pass@ip:port` URLs.
/// Explicit `proxies` win; otherwise download the Webshare list.
pub async fn resolve_pool(pool: &ProxyPool, client: &reqwest::Client) -> Result<Vec<String>> {
    if !pool.proxies.is_empty() {
        return Ok(pool.proxies.iter().map(|p| normalize(p)).collect());
    }
    let Some(url) = &pool.webshare_url else {
        return Ok(Vec::new());
    };
    let body = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(body.lines().filter_map(parse_line).collect())
}

/// Webshare default line: `ip:port:username:password` (password may contain `:`).
fn parse_line(line: &str) -> Option<String> {
    let url = normalize(line);
    url.contains('@').then_some(url)
}

/// `ip:port:user:pass` / `user:pass:ip:port` → proxy URL; URLs and `host:port` pass through.
pub fn normalize(p: &str) -> String {
    let p = p.trim();
    if p.contains("://") || p.contains('@') {
        return p.to_string();
    }
    let [ip, port, user, pass] = p.splitn(4, ':').collect::<Vec<_>>()[..] else {
        return p.to_string();
    };
    // reversed order needs a literal ip:port tail — hostname:port is ambiguous with ':' in passwords
    if ip.parse::<IpAddr>().is_err() {
        if let [rport, rip, cred] = p.rsplitn(3, ':').collect::<Vec<_>>()[..] {
            if rip.parse::<IpAddr>().is_ok() && rport.parse::<u16>().is_ok() {
                if let Some((u, pw)) = cred.split_once(':') {
                    return format!("http://{u}:{pw}@{rip}:{rport}");
                }
            }
        }
    }
    if port.parse::<u16>().is_ok() {
        return format!("http://{user}:{pass}@{ip}:{port}");
    }
    p.to_string()
}

/// Reject a specific proxy URL the router couldn't use, so a typo is caught when it's set instead
/// of silently failing every call through that account. `reqwest::Proxy::all` is too lenient on its
/// own (it swallows a bad scheme), so check the shape first: known scheme + `host:port`.
pub fn validate(proxy: &str) -> Result<()> {
    let bad = |msg: &str| {
        Error::Config(format!(
            "{msg} — expected [scheme://][user:pass@]host:port or ip:port:user:pass"
        ))
    };
    let proxy = &normalize(proxy);
    let (scheme, rest) = proxy.split_once("://").unwrap_or(("http", proxy));
    if !matches!(scheme, "http" | "https" | "socks5" | "socks5h" | "socks4") {
        return Err(bad(&format!("unsupported proxy scheme '{scheme}'")));
    }
    let host_port = rest.rsplit_once('@').map(|(_, h)| h).unwrap_or(rest);
    let (host, port) = host_port
        .rsplit_once(':')
        .ok_or_else(|| bad("missing port"))?;
    if host.is_empty() || port.parse::<u16>().is_err() {
        return Err(bad("bad host or port"));
    }
    let (url, _) = split_auth(proxy);
    reqwest::Proxy::all(&url)?;
    Ok(())
}

/// One reqwest client per account: proxy is a client-level setting. Userinfo in the
/// URL is split out into explicit basic auth, since proxies are HTTP-CONNECT (`http://`).
pub fn build_client(proxy: Option<&str>) -> Result<reqwest::Client> {
    let mut b = reqwest::Client::builder().timeout(Duration::from_secs(60));
    if let Some(p) = proxy {
        let (url, auth) = split_auth(p);
        let mut px = reqwest::Proxy::all(&url)?;
        if let Some((u, pw)) = auth {
            px = px.basic_auth(&u, &pw);
        }
        b = b.proxy(px);
    }
    Ok(b.build()?)
}

pub(crate) fn split_auth(p: &str) -> (String, Option<(String, String)>) {
    let p = normalize(p);
    let (scheme, rest) = p.split_once("://").unwrap_or(("http", &p));
    if let Some((cred, host)) = rest.rsplit_once('@') {
        if let Some((u, pw)) = cred.split_once(':') {
            return (
                format!("{scheme}://{host}"),
                Some((u.to_string(), pw.to_string())),
            );
        }
    }
    (p, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_and_rejects() {
        for ok in [
            "http://1.2.3.4:8080",
            "http://u:p@1.2.3.4:8080",
            "1.2.3.4:3128",
            "socks5://host.example:1080",
            "216.173.98.240:6242:evwhooua:ev23pxr8vuj6",
        ] {
            assert!(validate(ok).is_ok(), "should accept {ok}");
        }
        for bad in ["ht!tp://%%%bad", "1.2.3.4", "http://host:notaport", ""] {
            assert!(validate(bad).is_err(), "should reject {bad}");
        }
    }

    #[test]
    fn normalize_webshare_line() {
        assert_eq!(
            normalize("1.2.3.4:6242:user:pa:ss"),
            "http://user:pa:ss@1.2.3.4:6242"
        );
        for reversed in ["user:pa:ss:1.2.3.4:6242", "user:1234:1.2.3.4:6242"] {
            let url = normalize(reversed);
            assert!(url.ends_with("@1.2.3.4:6242"), "{reversed} -> {url}");
            assert!(url.starts_with("http://user:"), "{reversed} -> {url}");
        }
        for unchanged in [
            "http://u:p@h:80",
            "u:p@h:80",
            "host.example:8080",
            "[::1]:8080",
        ] {
            assert_eq!(normalize(unchanged), unchanged);
        }
    }
}
