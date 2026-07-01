use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::config::{resolve_secret, Config, Reset};
use crate::error::{Error, Result};
use crate::providers::{
    self, Capability, Input, LiveLimits, LiveQuota, OutImage, Provider, ProviderKind,
};
use crate::proxy;
use crate::usage::{period_key, DebugLog, RouteLog, Store};
use crate::web;

/// An account's transport: an API client (reqwest) or a cookie-authed impersonating client (wreq).
/// Web carries the raw cookies too, for providers that must drive a real browser (chatgpt_web).
pub enum Conn {
    Api(reqwest::Client),
    Web(wreq::Client, Vec<web::Cookie>),
}

pub struct Bucket {
    pub provider: Provider,
    pub conn: Conn,
    pub key: String,
    pub label: String,
    pub quota: i64,
    pub reset: Reset,
    pub dr_quota: i64,
    pub dr_reset: Reset,
    pub proxy: Option<String>,
}

/// A successful call's answer plus an optional resume token (`provider:opaque`) for web sessions.
#[derive(Debug)]
pub struct Reply {
    pub text: String,
    pub session: Option<String>,
    pub image: Option<OutImage>,
}

#[derive(Serialize)]
pub struct UsageView {
    pub provider: &'static str,
    pub label: String,
    pub period: String,
    pub quota: i64,
    pub used: i64,
    pub remaining: i64,
    pub exhausted: bool,
    pub proxy: String,
    /// Rolling-window length when the figure comes live from the provider (grok); else `None`.
    pub window_secs: Option<i64>,
    /// Live per-tier tool/model allowances (chatgpt_web), attached to a provider's main view.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits: Option<LiveLimits>,
}

pub struct Router {
    buckets: Vec<Bucket>,
    store: Store,
    // Short-lived cache of provider-reported quota, keyed by "{label}|{deep}", so polling the
    // dashboard or `usage` doesn't hit grok's rate-limit endpoint on every call.
    live: Mutex<HashMap<String, (Instant, LiveQuota)>>,
    // Same idea for the richer per-tier limits (chatgpt_web), keyed by label. `None` caches a miss.
    live_limits: Mutex<HashMap<String, (Instant, Option<LiveLimits>)>>,
    // `Some(retention_hours)` records every attempt to the debug log; `None` is disabled.
    debug: Option<i64>,
}

impl Router {
    pub fn from_parts(buckets: Vec<Bucket>, store: Store) -> Self {
        Self {
            buckets,
            store,
            live: Mutex::new(HashMap::new()),
            live_limits: Mutex::new(HashMap::new()),
            debug: None,
        }
    }

    pub async fn build(cfg: Config, store: Store) -> Result<Self> {
        let debug = cfg
            .debug_log
            .enabled
            .then_some(cfg.debug_log.retention_hours);
        // Only fetch the proxy list if a "pool" account still lacks a (cached) assignment —
        // otherwise every server launch would re-download it and block the MCP handshake.
        let mut needs_pool = false;
        for acc in &cfg.accounts {
            if acc.proxy.as_deref() == Some("pool") && store.proxy_for(&acc.label).await?.is_none()
            {
                needs_pool = true;
                break;
            }
        }
        let pool = if needs_pool {
            // Bounded so a slow/hanging Webshare endpoint can't stall startup.
            let bootstrap = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(8))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new());
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                proxy::resolve_pool(&cfg.proxy_pool, &bootstrap),
            )
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default()
        } else {
            Vec::new()
        };
        let direct = reqwest::Client::new();
        let mut assigned = store.assignment_count().await? as usize;
        let mut buckets = Vec::with_capacity(cfg.accounts.len());

        for acc in cfg.accounts {
            let proxy_url = match acc.proxy.as_deref() {
                Some("pool") => sticky_pool(&store, &acc.label, &pool, &mut assigned).await?,
                Some(url) => Some(url.to_string()),
                None => None,
            };
            let (conn, key) = if acc.provider.is_web() {
                let Some(raw) = store.load_session(&acc.label).await? else {
                    tracing::warn!(
                        label = %acc.label,
                        "no web session; run `fetchira login {}`",
                        acc.provider.as_str()
                    );
                    continue;
                };
                let sess = web::parse_session(&raw);
                let client = web::build_client(&sess.cookies, &sess.headers, proxy_url.as_deref())?;
                (Conn::Web(client, sess.cookies), String::new())
            } else {
                let Some(key) = acc
                    .api_key
                    .as_deref()
                    .and_then(|s| resolve_secret(s).ok())
                    .filter(|k| !k.trim().is_empty())
                else {
                    tracing::warn!(
                        label = %acc.label,
                        "no usable API key; set it or run `fetchira add {}`",
                        acc.provider.as_str()
                    );
                    continue;
                };
                let client = match &proxy_url {
                    Some(p) => proxy::build_client(Some(p))?,
                    None => direct.clone(),
                };
                (Conn::Api(client), key)
            };
            buckets.push(Bucket {
                quota: acc.quota.unwrap_or_else(|| acc.provider.default_quota()),
                reset: acc.reset.unwrap_or_else(|| acc.provider.default_reset()),
                dr_quota: acc.dr_quota.unwrap_or_else(|| acc.provider.dr_quota()),
                dr_reset: acc.dr_reset.unwrap_or_else(|| acc.provider.dr_reset()),
                provider: Provider::new(acc.provider),
                conn,
                key,
                label: acc.label,
                proxy: proxy_url,
            });
        }
        Ok(Self {
            buckets,
            store,
            live: Mutex::new(HashMap::new()),
            live_limits: Mutex::new(HashMap::new()),
            debug,
        })
    }

    /// Pick the most-preferred provider with a non-exhausted account (most-remaining
    /// account wins within a provider's pool); fail over on rate/quota/transport errors.
    /// `forced` restricts candidates to one provider and errors rather than switching.
    pub async fn call(
        &self,
        cap: Capability,
        input: &Input,
        forced: Option<ProviderKind>,
    ) -> Result<Reply> {
        let mut last_err: Option<Error> = None;
        // The most recent failed attempt in this call, so a later success records the failover hop.
        let mut prev_fail: Option<(String, i64)> = None;

        for &kind in providers::order(cap) {
            if matches!(forced, Some(f) if f != kind) {
                continue;
            }
            let mut cands: Vec<(usize, i64, String, i64, String)> = Vec::new();
            for (i, b) in self.buckets.iter().enumerate() {
                if b.provider.kind != kind {
                    continue;
                }
                let (blabel, bquota, breset) = budget(b, cap);
                let period = period_key(breset);
                let mut rem = self.store.remaining(&blabel, bquota, &period).await?;
                // For tool-gated capabilities, trust the provider's live allowance over the soft
                // counter: skip a bucket the provider says is exhausted (proactive failover).
                if rem > 0 {
                    if let Some(feature) = live_feature(cap) {
                        if let Some(live) = self
                            .live_limits_for(b)
                            .await
                            .and_then(|l| l.remaining(feature))
                        {
                            rem = live;
                        }
                    }
                }
                if rem > 0 {
                    cands.push((i, rem, blabel, bquota, period));
                }
            }
            cands.sort_by(|a, c| c.1.cmp(&a.1));

            for (i, _rem, blabel, bquota, period) in cands {
                let b = &self.buckets[i];
                // Reserve the slot *before* the network call so concurrent tasks can't all clear the
                // same `remaining > 0` gate and stampede one account past its quota. Claim a nominal
                // 1 (the per-call minimum), settle the real cost on success, refund on any failure.
                // A `false` means a sibling took the last slot meanwhile — move to the next account.
                if !self
                    .store
                    .reserve(kind.as_str(), &blabel, bquota, &period, 1)
                    .await?
                {
                    continue;
                }
                let t0 = Instant::now();
                let res = match &b.conn {
                    Conn::Api(c) => b.provider.call(&b.key, c, cap, input).await,
                    // chatgpt.com gates generation behind an anti-bot defense pure HTTP can't pass, so
                    // it drives a real browser with the captured cookies. A deep-research poll is a
                    // plain GET (not gated), so resume it over HTTP instead of relaunching a browser.
                    Conn::Web(c, cookies) if b.provider.kind == ProviderKind::ChatgptWeb => {
                        if input
                            .session
                            .as_deref()
                            .is_some_and(|s| s.starts_with("dr|poll|"))
                        {
                            b.provider.call_web(c, cap, input).await
                        } else {
                            providers::chatgpt_browser::run(cookies, cap, input).await
                        }
                    }
                    Conn::Web(c, _) => b.provider.call_web(c, cap, input).await,
                };
                let latency = t0.elapsed().as_millis() as i64;
                let acct = strip_dr(&blabel);
                // Firehose: every attempt (success or failure, incl. a 403 body) lands here.
                if let Some(retention) = self.debug {
                    let err = res.as_ref().err().map(|e| e.to_string());
                    let req = describe_input(cap, input);
                    let _ = self
                        .store
                        .log_debug(
                            &DebugLog {
                                capability: cap.as_str(),
                                provider: kind.as_str(),
                                label: acct,
                                status: match &res {
                                    Ok(_) => 200,
                                    Err(e) => err_code(e),
                                },
                                latency_ms: latency,
                                request: &req,
                                response: res.as_ref().ok().map(|o| o.text.as_str()),
                                error: err.as_deref(),
                            },
                            retention,
                        )
                        .await;
                }
                match res {
                    Ok(o) => {
                        // The reservation already charged 1; settle the rest for costlier calls.
                        if o.cost != 1 {
                            let _ = self
                                .store
                                .record(kind.as_str(), &blabel, &period, o.cost - 1)
                                .await;
                        }
                        // Best-effort route log (never fail the call over telemetry).
                        let _ = self
                            .store
                            .log_route(&RouteLog {
                                capability: cap.as_str(),
                                provider: kind.as_str(),
                                label: acct,
                                status: 200,
                                latency_ms: latency,
                                fail_from: prev_fail.as_ref().map(|(l, _)| l.as_str()),
                                fail_code: prev_fail.as_ref().map(|(_, c)| *c),
                            })
                            .await;
                        let session = o.session.map(|s| format!("{}:{}", kind.as_str(), s));
                        return Ok(Reply {
                            text: o.text,
                            session,
                            image: o.image,
                        });
                    }
                    Err(e) => {
                        // The reserved unit never became real usage — give it back.
                        let _ = self.store.refund(&blabel, &period, 1).await;
                        match e {
                            Error::RateLimit(_) | Error::QuotaExceeded(_) => {
                                let _ = self
                                    .store
                                    .mark_exhausted(kind.as_str(), &blabel, &period)
                                    .await;
                                let code = if matches!(e, Error::RateLimit(_)) {
                                    429
                                } else {
                                    402
                                };
                                prev_fail = Some((acct.to_string(), code));
                                last_err = Some(e);
                            }
                            Error::Provider { .. }
                            | Error::Transport(_)
                            | Error::Timeout(_)
                            | Error::BadResponse(_) => {
                                prev_fail = Some((acct.to_string(), 0));
                                last_err = Some(e);
                            }
                            _ => return Err(e),
                        }
                    }
                }
            }
        }

        // A real error from an attempt beats the generic "no account" message — surface it,
        // forced or not, so the caller sees why (e.g. an expired web session or a 403).
        if let Some(e) = last_err {
            return Err(e);
        }
        if let Some(f) = forced {
            return Err(Error::ProviderForced(f.as_str().to_string()));
        }
        Err(Error::NoCandidate(cap.as_str()))
    }

    pub async fn usage_snapshot(&self) -> Result<Vec<UsageView>> {
        let mut out = Vec::with_capacity(self.buckets.len());
        for b in &self.buckets {
            let proxy = b.proxy.clone().unwrap_or_else(|| "direct".to_string());
            let ll = self.live_limits_for(b).await;
            let mut mv = self
                .view(b.provider.kind.as_str(), &b.label, b.quota, b.reset, &proxy)
                .await?;
            self.patch_live(b, false, &mut mv).await;
            mv.limits = ll.clone();
            out.push(mv);
            // Web providers track deep_research against a separate daily budget.
            if b.provider.kind.is_web() {
                let label = format!("{}#dr", b.label);
                let mut dv = self
                    .view(
                        b.provider.kind.as_str(),
                        &label,
                        b.dr_quota,
                        b.dr_reset,
                        &proxy,
                    )
                    .await?;
                self.patch_live(b, true, &mut dv).await;
                // Prefer the provider's live deep-research allowance over the soft counter.
                if let Some(rem) = ll.as_ref().and_then(|l| l.remaining("deep_research")) {
                    dv.remaining = rem;
                    dv.used = (dv.quota - rem).max(0);
                    dv.exhausted = rem == 0;
                }
                out.push(dv);
            }
        }
        Ok(out)
    }

    /// Replace the soft local counter with the provider's live figure when it reports one (grok),
    /// caching it briefly. Best-effort: a failed fetch leaves the soft view untouched.
    async fn patch_live(&self, b: &Bucket, deep: bool, v: &mut UsageView) {
        let Conn::Web(c, _) = &b.conn else { return };
        let key = format!("{}|{}", b.label, deep);
        let cached = self.live.lock().ok().and_then(|m| {
            m.get(&key)
                .filter(|(t, _)| t.elapsed() < Duration::from_secs(20))
                .map(|(_, lq)| *lq)
        });
        let lq = match cached {
            Some(lq) => lq,
            None => {
                let Some(lq) = b.provider.live_quota(c, deep).await else {
                    return;
                };
                if let Ok(mut m) = self.live.lock() {
                    m.insert(key, (Instant::now(), lq));
                }
                lq
            }
        };
        v.quota = lq.total;
        v.remaining = lq.remaining;
        v.used = (lq.total - lq.remaining).max(0);
        v.exhausted = lq.remaining == 0;
        v.window_secs = Some(lq.window_secs);
    }

    /// Live per-tier limits for a web bucket, cached 20s (a miss is cached too, so a provider
    /// without them isn't re-polled on every snapshot).
    async fn live_limits_for(&self, b: &Bucket) -> Option<LiveLimits> {
        let Conn::Web(c, _) = &b.conn else {
            return None;
        };
        if let Some(hit) = self.live_limits.lock().ok().and_then(|m| {
            m.get(&b.label)
                .filter(|(t, _)| t.elapsed() < Duration::from_secs(20))
                .map(|(_, v)| v.clone())
        }) {
            return hit;
        }
        let fresh = b.provider.live_limits(c).await;
        if let Ok(mut m) = self.live_limits.lock() {
            m.insert(b.label.clone(), (Instant::now(), fresh.clone()));
        }
        fresh
    }

    async fn view(
        &self,
        provider: &'static str,
        label: &str,
        quota: i64,
        reset: Reset,
        proxy: &str,
    ) -> Result<UsageView> {
        let period = period_key(reset);
        let u = self.store.usage_for(label, &period).await?;
        Ok(UsageView {
            provider,
            label: label.to_string(),
            period,
            quota,
            used: u.used,
            remaining: if u.exhausted {
                0
            } else {
                (quota - u.used).max(0)
            },
            exhausted: u.exhausted,
            proxy: proxy.to_string(),
            window_secs: None,
            limits: None,
        })
    }
}

/// Display label for the route log: drop the `#dr` budget suffix back to the account label.
fn strip_dr(label: &str) -> &str {
    label.strip_suffix("#dr").unwrap_or(label)
}

/// HTTP-ish status for the debug log: the provider's real code when it has one, else 429/402 for
/// the rate/quota cases and 0 for transport/shape errors.
fn err_code(e: &Error) -> i64 {
    match e {
        Error::Provider { status, .. } => *status as i64,
        Error::RateLimit(_) => 429,
        Error::QuotaExceeded(_) => 402,
        _ => 0,
    }
}

/// The request side of a debug entry: the meaningful inputs, as compact JSON.
fn describe_input(cap: Capability, input: &Input) -> String {
    serde_json::json!({
        "capability": cap.as_str(),
        "query": input.query,
        "url": input.url,
        "model": input.model,
        "mode": input.mode,
        "session": input.session,
        "max_results": input.max_results,
    })
    .to_string()
}

/// (db label, quota, reset) for a bucket+capability. Web deep_research uses a separate
/// `<label>#dr` daily budget so a research doesn't deplete the chat counter.
/// The provider feature whose live allowance gates a capability (for proactive failover). `None`
/// means use only the soft counter (chat/search caps are effectively unlimited on paid tiers).
fn live_feature(cap: Capability) -> Option<&'static str> {
    match cap {
        Capability::DeepResearch => Some("deep_research"),
        Capability::Image => Some("image_gen"),
        _ => None,
    }
}

fn budget(b: &Bucket, cap: Capability) -> (String, i64, Reset) {
    if b.provider.kind.is_web() && cap == Capability::DeepResearch {
        (format!("{}#dr", b.label), b.dr_quota, b.dr_reset)
    } else {
        (b.label.clone(), b.quota, b.reset)
    }
}

async fn sticky_pool(
    store: &Store,
    label: &str,
    pool: &[String],
    assigned: &mut usize,
) -> Result<Option<String>> {
    if let Some(p) = store.proxy_for(label).await? {
        return Ok(Some(p));
    }
    if pool.is_empty() {
        return Ok(None);
    }
    let p = pool[*assigned % pool.len()].clone();
    *assigned += 1;
    store.assign_proxy(label, &p).await?;
    Ok(Some(p))
}
