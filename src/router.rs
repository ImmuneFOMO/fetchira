use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::Serialize;

use crate::config::{resolve_secret, Config, Priority, Reset};
use crate::error::{Error, Result};
use crate::httptrace;
use crate::providers::{
    self, order_for, Capability, FeatureLimit, Input, LiveBalance, LiveLimits, LiveQuota,
    ModelInfo, OutImage, Provider, ProviderKind,
};
use crate::proxy;
use crate::usage::{period_key, DebugLog, LiveRow, RouteLog, Store};
use crate::web;

/// An account's transport; Web keeps the raw cookies for providers that drive a real browser.
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
    /// The quota came from config, so it caps live limits instead of yielding to them.
    pub pinned: bool,
    pub dr_pinned: bool,
    pub proxy: Option<String>,
    /// Dashboard cookie client for a `balance_session` provider's balance; `None` until login.
    pub balance_conn: Option<wreq::Client>,
}

/// A successful call's answer plus an optional resume token (`provider:<base64-label>:opaque`).
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
    /// Real dollar balance for top-up $ providers (exa/parallel/steel); None for credit providers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usd: Option<f64>,
    /// Cached snapshot only: the live figure isn't fetched yet, so the dashboard shows a loader.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
    /// Unix ms when the provider renews the live figure, when it says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<i64>,
    /// Unix ms the live figure was read; our own calls count it down since.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<i64>,
    /// Rate-limited only: no quota to show, so routing uses an effectively unlimited ceiling.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unmetered: bool,
}

pub struct Router {
    buckets: Vec<Bucket>,
    store: Store,
    // User's per-capability provider order (fetchira priority); empty = built-in order.
    priority: Priority,
    // Short-lived provider quota keyed by "{label}|{deep}", so polling doesn't hammer grok.
    live: Mutex<HashMap<String, (Instant, LiveQuota)>>,
    // Per-tier limits by label; HTTP-only misses stay out so a browser snapshot can retry.
    live_limits: Mutex<HashMap<String, (Instant, Option<LiveLimits>)>>,
    // Last balance fetch per label, hit or miss: snapshots re-poll at most every 20s.
    balance: Mutex<HashMap<String, Instant>>,
    // `Some(retention_hours)` records every attempt to the debug log; `None` is disabled.
    debug: Option<i64>,
    // Overlapping browser processes reset CDP on small VPSes.
    browser_gate: Arc<tokio::sync::Mutex<()>>,
    // One legacy 429 reconciliation at a time, so callers don't stampede the balance endpoint.
    legacy_recovery_gate: tokio::sync::Mutex<()>,
}

const LIVE_LIMITS_CACHE: Duration = Duration::from_secs(30);
const DEFAULT_RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(60);
const REJECTED_KEY_COOLDOWN: Duration = Duration::from_secs(300);
const LIVE_REFRESH: Duration = Duration::from_secs(60);
// Outlasts the concurrent bounded balance reads plus their writes; also caps a waiter's wait.
const LIVE_READ: Duration = Duration::from_secs(20);
pub const UNMETERED: i64 = 1_000_000_000;

struct ExhaustionRecovery {
    /// A one-shot probe lease is released only after the provider accepts the request.
    probe_until: Option<i64>,
}

/// Frees a `#live` lease whose re-read was dropped mid-flight, so waiters don't sit out its expiry.
struct Lease {
    store: Store,
    label: String,
    until: Option<i64>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let (Some(until), Ok(rt)) = (self.until, tokio::runtime::Handle::try_current()) else {
            return;
        };
        let (store, label) = (self.store.clone(), std::mem::take(&mut self.label));
        rt.spawn(async move {
            let _ = store.release_cooldown(&label, until).await;
        });
    }
}

impl Router {
    pub fn from_parts(buckets: Vec<Bucket>, store: Store) -> Self {
        Self {
            buckets,
            store,
            priority: Priority::default(),
            live: Mutex::new(HashMap::new()),
            live_limits: Mutex::new(HashMap::new()),
            balance: Mutex::new(HashMap::new()),
            debug: None,
            browser_gate: Arc::new(tokio::sync::Mutex::new(())),
            legacy_recovery_gate: tokio::sync::Mutex::new(()),
        }
    }

    /// Replace the user priority on a router assembled from parts.
    pub fn with_priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    pub async fn build(cfg: Config, store: Store) -> Result<Self> {
        let debug = cfg
            .debug_log
            .enabled
            .then_some(cfg.debug_log.retention_hours);
        let priority = cfg.priority.clone();
        // The proxy list download blocks the MCP handshake, so only when an assignment is missing.
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
        // Above any legit call (exa research is one long POST); only a stalled endpoint trips it.
        let direct = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
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
                        "no web session for {}; authenticate this provider account",
                        acc.provider.as_str()
                    );
                    continue;
                };
                let sess = web::parse_session(&raw);
                let client = web::build_client(&sess.cookies, &sess.headers, proxy_url.as_deref())?;
                (Conn::Web(client, sess.cookies), String::new())
            } else {
                let key = match acc.api_key.as_deref() {
                    Some(s) => resolve_secret(s).ok().filter(|k| !k.trim().is_empty()),
                    None => (!acc.provider.needs_key()).then(String::new),
                };
                let Some(key) = key else {
                    tracing::warn!(
                        label = %acc.label,
                        "no usable API key for {}; add a provider account",
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
            let balance_conn = match acc.provider.balance_session() {
                true => match store.load_session(&acc.label).await? {
                    Some(raw) => {
                        let sess = web::parse_session(&raw);
                        web::build_client(&sess.cookies, &sess.headers, proxy_url.as_deref()).ok()
                    }
                    None => None,
                },
                false => None,
            };
            buckets.push(Bucket {
                quota: acc
                    .quota
                    .unwrap_or_else(|| default_ceiling(acc.provider, !key.is_empty())),
                reset: acc.reset.unwrap_or_else(|| acc.provider.default_reset()),
                dr_quota: acc.dr_quota.unwrap_or_else(|| acc.provider.dr_quota()),
                dr_reset: acc.dr_reset.unwrap_or_else(|| acc.provider.dr_reset()),
                pinned: acc.quota.is_some(),
                dr_pinned: acc.dr_quota.is_some(),
                provider: Provider::new(acc.provider),
                conn,
                key,
                label: acc.label,
                proxy: proxy_url,
                balance_conn,
            });
        }
        Ok(Self {
            buckets,
            store,
            priority,
            live: Mutex::new(HashMap::new()),
            live_limits: Mutex::new(HashMap::new()),
            balance: Mutex::new(HashMap::new()),
            debug,
            browser_gate: Arc::new(tokio::sync::Mutex::new(())),
            legacy_recovery_gate: tokio::sync::Mutex::new(()),
        })
    }

    /// Route to the most-preferred provider with room and fail over; `forced` pins one provider.
    pub async fn call(
        &self,
        cap: Capability,
        input: &Input,
        forced: Option<ProviderKind>,
    ) -> Result<Reply> {
        if let (Some(kind), Some(session)) = (forced, input.session.as_deref()) {
            if let Some((label, opaque)) = decode_session_affinity(session) {
                if self
                    .buckets
                    .iter()
                    .any(|b| b.provider.kind == kind && b.label == label)
                {
                    let mut pinned = input.clone();
                    pinned.session = Some(opaque);
                    return self
                        .call_with_account(cap, &pinned, forced, Some(&label))
                        .await;
                }
                return Err(Error::ProviderForced(format!(
                    "{} account '{}' from session is unavailable",
                    kind.as_str(),
                    label
                )));
            }
        }
        self.call_with_account(cap, input, forced, None).await
    }

    /// Call one configured account without changing the normal provider routing API.
    pub async fn call_account(
        &self,
        cap: Capability,
        input: &Input,
        provider: ProviderKind,
        label: &str,
    ) -> Result<Reply> {
        self.call_with_account(cap, input, Some(provider), Some(label))
            .await
    }

    async fn call_with_account(
        &self,
        cap: Capability,
        input: &Input,
        forced: Option<ProviderKind>,
        account_label: Option<&str>,
    ) -> Result<Reply> {
        let mut last_err: Option<Error> = None;
        // The most recent failed attempt in this call, so a later success records the failover hop.
        let mut prev_fail: Option<(String, i64)> = None;

        // A forced provider may serve the cap outside the auto-route order (read via tavily).
        let order = match forced {
            Some(f) if !f.supports(cap) => return Err(Error::Unsupported(f.as_str())),
            Some(f) => vec![f],
            None => {
                let mut order = order_for(cap, input.topic.as_deref(), self.priority.for_cap(cap));
                // OpenAlex has no host filter and links through doi.org, so it can't honor domains.
                if input.domains.is_some() {
                    order.retain(|&k| k != ProviderKind::Openalex);
                }
                order
            }
        };
        for &kind in &order {
            let mut cands: Vec<(usize, i64, String, i64, String, Option<i64>)> = Vec::new();
            let mut spent = Vec::new();
            for (i, b) in self.buckets.iter().enumerate() {
                if b.provider.kind != kind {
                    continue;
                }
                if account_label.is_some_and(|label| b.label != label) {
                    continue;
                }
                let (blabel, bquota, pinned, breset) = budget(b, cap);
                let period = period_key(breset);
                if let Some(wait) = self
                    .store
                    .cooldown_remaining(&format!("{}#rejected", b.label))
                    .await?
                {
                    last_err = Some(Error::Provider {
                        provider: kind.as_str(),
                        status: 401,
                        body: format!(
                            "account '{}' has a rejected api key; benched for {}",
                            b.label,
                            human_wait(wait)
                        ),
                    });
                    continue;
                }
                if let Some(wait) = self.store.cooldown_remaining(&blabel).await? {
                    last_err = Some(Error::rate_limit_after(
                        format!(
                            "{}: account '{}' is cooling down; retry after {}",
                            kind.as_str(),
                            b.label,
                            human_wait(wait)
                        ),
                        Some(wait),
                    ));
                    continue;
                }
                let (mut quota, mut rem) = self.headroom(&blabel, bquota, pinned, &period).await?;
                let mut probe_until = None;
                if rem == 0 {
                    if let Some(recovery) =
                        self.recover_exhaustion(b, cap, &blabel, &period).await?
                    {
                        probe_until = recovery.probe_until;
                        (quota, rem) = self.headroom(&blabel, bquota, pinned, &period).await?;
                    } else if self.store.cooldown_remaining(&blabel).await?.is_none() {
                        // A sibling may have cleared the mark; a live probe lease owns the call.
                        (quota, rem) = self.headroom(&blabel, bquota, pinned, &period).await?;
                    }
                    if rem == 0 && probe_until.is_some() {
                        (quota, rem) = self
                            .provider_decides(&blabel, bquota, pinned, &period)
                            .await?;
                    } else if rem == 0 {
                        spent.push((i, blabel, bquota, pinned, period));
                        continue;
                    }
                }
                // Tool-gated caps follow the live allowance; a forced call tries anyway.
                if rem > 0 && forced.is_none() {
                    if let Some(feature) = live_feature(cap) {
                        if let Some(live) = self
                            .live_limits_for(b, true, true)
                            .await
                            .and_then(|l| l.remaining(feature))
                        {
                            rem = live;
                        }
                    }
                }
                if rem > 0 {
                    cands.push((i, rem, blabel, quota, period, probe_until));
                }
            }
            // A re-read can stall for seconds, so a spent balance waits until no sibling has room.
            if cands.is_empty() {
                for (i, blabel, bquota, pinned, period) in spent {
                    let Some(landed) = self
                        .refresh_live(&self.buckets[i], &blabel, bquota, pinned, &period)
                        .await?
                    else {
                        continue;
                    };
                    let (mut quota, mut rem) =
                        self.headroom(&blabel, bquota, pinned, &period).await?;
                    // A zero the provider reported holds; only our own count-down defers to it.
                    if rem == 0
                        && !landed
                        && self
                            .store
                            .load_live(&blabel)
                            .await?
                            .is_none_or(|s| s.remaining > 0)
                    {
                        (quota, rem) = self
                            .provider_decides(&blabel, bquota, pinned, &period)
                            .await?;
                    }
                    if rem > 0 {
                        cands.push((i, rem, blabel, quota, period, None));
                    }
                }
            }
            // Free rate-limited accounts go first; metered credits only cover their overflow.
            cands.sort_by_key(|a| std::cmp::Reverse((self.buckets[a.0].quota == UNMETERED, a.1)));

            for (i, _rem, blabel, quota, period, probe_until) in cands {
                let b = &self.buckets[i];
                // Reserve 1 before the call so concurrent callers can't overrun the quota.
                if !self
                    .store
                    .reserve(kind.as_str(), &blabel, quota, &period, 1, probe_until)
                    .await?
                {
                    continue;
                }
                let t0 = Instant::now();
                let mut http_trace = None;
                let res = match &b.conn {
                    Conn::Api(c) => {
                        let (res, traces) =
                            httptrace::capture(b.provider.call(&b.key, c, cap, input)).await;
                        if !traces.is_empty() {
                            http_trace = serde_json::to_string(&traces).ok();
                        }
                        res
                    }
                    Conn::Web(_, cookies)
                        if b.provider.kind == ProviderKind::ChatgptWeb
                            && input.session.as_deref().is_some_and(|s| {
                                let s = s.strip_prefix("chatgpt_web:").unwrap_or(s);
                                s.starts_with("dr|poll|") || s.starts_with("img|poll|")
                            }) =>
                    {
                        // The HTTP path's per-call bearer breaks on NextAuth rotation mid-poll.
                        let _browser_gate = self.browser_gate.lock().await;
                        providers::chatgpt_browser::run(cookies, cap, input).await
                    }
                    // OpenAI's anti-bot gate blocks the HTTP turn, so turns drive a real browser.
                    Conn::Web(_, cookies) if b.provider.kind == ProviderKind::ChatgptWeb => {
                        let _browser_gate = self.browser_gate.lock().await;
                        providers::chatgpt_browser::run(cookies, cap, input).await
                    }
                    Conn::Web(c, _) => b.provider.call_web(c, cap, input, &b.label).await,
                };
                // Keep rotated cookies so the next process starts from the freshest token.
                if let Ok(out) = &res {
                    if !out.cookie_updates.is_empty() {
                        self.refresh_session(b, &out.cookie_updates).await;
                    }
                }
                let latency = t0.elapsed().as_millis() as i64;
                let acct = strip_budget(&blabel);
                let hosted_request = crate::usage::HOSTED_REQUEST_ID
                    .try_with(|id| id.clone())
                    .ok();
                if let Some(request_id) = hosted_request.as_deref() {
                    let _ = self
                        .store
                        .log_attempt(
                            request_id,
                            kind.as_str(),
                            acct,
                            match &res {
                                Ok(_) => 200,
                                Err(e) => err_code(e),
                            },
                            latency,
                            res.as_ref().err().map(ToString::to_string).as_deref(),
                            res.is_ok(),
                        )
                        .await;
                }
                let mut debug_id = None;
                if let Some(retention) = self.debug {
                    let err = res.as_ref().err().map(|e| e.to_string());
                    let req = describe_input(cap, input);
                    debug_id = self
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
                                http_trace: http_trace.as_deref(),
                            },
                            retention,
                        )
                        .await
                        .ok();
                }
                match res {
                    Ok(o) => {
                        // An empty read isn't an answer: let failover or the browser try.
                        if cap == Capability::Read && o.text.trim().is_empty() && o.image.is_none()
                        {
                            let _ = self.store.refund(&blabel, &period, 1).await;
                            prev_fail = Some((acct.to_string(), 0));
                            last_err = Some(Error::BadResponse(kind.as_str()));
                            continue;
                        }
                        if let Some(until) = probe_until {
                            let _ = self.store.release_cooldown(&blabel, until).await;
                        }
                        // The reservation already charged 1; settle the rest for costlier calls.
                        if o.cost != 1 {
                            let _ = self
                                .store
                                .record(kind.as_str(), &blabel, &period, o.cost - 1)
                                .await;
                        }
                        if let Some(live) = o.live {
                            let _ = self.store.save_reply_live(&blabel, &period, &live).await;
                        }
                        let niche = match providers::niche_native(kind, cap, input) {
                            Some(true) => "native",
                            Some(false) => "rewrite",
                            None => "",
                        };
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
                                niche,
                                debug_id,
                            })
                            .await;
                        // Deep-research honesty: `deep` on these bills a live $ balance.
                        let mut text = o.text;
                        if cap == Capability::DeepResearch
                            && input.depth.as_deref() == Some("deep")
                            && matches!(
                                kind,
                                ProviderKind::Exa
                                    | ProviderKind::Parallel
                                    | ProviderKind::Linkup
                                    | ProviderKind::You
                            )
                        {
                            text = format!(
                                "⟦deep research via {} — spends live $ balance⟧\n{text}",
                                kind.as_str()
                            );
                        }
                        let session = o.session.map(|s| {
                            format!(
                                "{}:{}:{}",
                                kind.as_str(),
                                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&b.label),
                                s
                            )
                        });
                        return Ok(Reply {
                            text,
                            session,
                            image: o.image,
                        });
                    }
                    Err(e) => {
                        let _ = self.store.refund(&blabel, &period, 1).await;
                        match e {
                            Error::RateLimit {
                                message,
                                retry_after,
                            } => {
                                let delay = retry_after
                                    .unwrap_or(DEFAULT_RATE_LIMIT_COOLDOWN)
                                    .max(Duration::from_secs(1));
                                let _ = self.store.set_cooldown(&blabel, delay).await;
                                prev_fail = Some((acct.to_string(), 429));
                                last_err = Some(Error::rate_limit_after(
                                    enrich_limit(
                                        message,
                                        Some(format!("retry after {}", human_wait(delay))),
                                    ),
                                    Some(delay),
                                ));
                            }
                            Error::QuotaExceeded(msg) => {
                                let _ = self
                                    .store
                                    .mark_exhausted(kind.as_str(), &blabel, &period)
                                    .await;
                                let _ = self.store.drain_live(&blabel, &period).await;
                                // The recheck lets a top-up revive even a `Reset::Once` account.
                                let _ = self
                                    .store
                                    .set_cooldown(&blabel, DEFAULT_RATE_LIMIT_COOLDOWN)
                                    .await;
                                prev_fail = Some((acct.to_string(), 402));
                                let hint = self.reset_hint(b, cap).await.or_else(|| {
                                    Some("balance will be rechecked in ~1m".to_string())
                                });
                                last_err = Some(Error::QuotaExceeded(enrich_limit(msg, hint)));
                            }
                            Error::Provider { status, .. } => {
                                // A rejected key won't heal; `remove` clears the bench.
                                if status == 401 && !kind.is_web() {
                                    let _ = self
                                        .store
                                        .set_cooldown(&blabel, REJECTED_KEY_COOLDOWN)
                                        .await;
                                    let _ = self
                                        .store
                                        .set_cooldown(
                                            &format!("{}#rejected", b.label),
                                            REJECTED_KEY_COOLDOWN,
                                        )
                                        .await;
                                }
                                prev_fail = Some((acct.to_string(), status as i64));
                                last_err = Some(e);
                            }
                            Error::Transport(_) | Error::Timeout(_) | Error::BadResponse(_) => {
                                prev_fail = Some((acct.to_string(), 0));
                                last_err = Some(e);
                            }
                            _ => return Err(e),
                        }
                        // An image prompt may already be submitted; fail over only if not.
                        if matches!(cap, Capability::Image)
                            && last_err
                                .as_ref()
                                .is_some_and(|err| !image_error_can_failover(err))
                        {
                            if let Some(err) = last_err {
                                return Err(err);
                            }
                        }
                    }
                }
            }
        }

        // Every read failed or came back empty: the page likely needs a real browser.
        if cap == Capability::Read && forced.is_none() && last_err.is_some() {
            if let Ok(reply) = Box::pin(self.call(Capability::Browser, input, None)).await {
                return Ok(reply);
            }
        }

        // A real attempt error beats the generic "no account" one.
        if let Some(e) = last_err {
            return Err(e);
        }
        if let Some(f) = forced {
            return Err(Error::ProviderForced(f.as_str().to_string()));
        }
        Err(Error::NoCandidate(cap.as_str()))
    }

    /// Recheck legacy 429 marks and quota denials, one probe across processes.
    async fn recover_exhaustion(
        &self,
        b: &Bucket,
        cap: Capability,
        label: &str,
        period: &str,
    ) -> Result<Option<ExhaustionRecovery>> {
        if self.store.legacy_exhausted(label, period).await? {
            return self.recover_legacy_exhaustion(b, label, period).await;
        }
        if !self.store.quota_exhausted(label, period).await? {
            return Ok(None);
        }
        let Some(probe_until) = self
            .store
            .claim_cooldown(label, DEFAULT_RATE_LIMIT_COOLDOWN)
            .await?
        else {
            return Ok(None);
        };

        // Web accounts follow a live feature allowance, else probe once.
        if b.provider.kind.is_web() {
            if let Some(feature) = live_feature(cap) {
                match self
                    .live_limits_for(b, true, true)
                    .await
                    .and_then(|limits| limits.remaining(feature))
                {
                    Some(remaining) if remaining <= 0 => return Ok(None),
                    Some(_) => {
                        let cleared = self.store.clear_quota_exhausted(label, period).await?;
                        if cleared {
                            let _ = self.store.release_cooldown(label, probe_until).await;
                        }
                        return Ok(cleared.then_some(ExhaustionRecovery { probe_until: None }));
                    }
                    None => {}
                }
            }
            let cleared = self.store.clear_quota_exhausted(label, period).await?;
            return Ok(cleared.then_some(ExhaustionRecovery {
                probe_until: Some(probe_until),
            }));
        }

        // Only a fresh positive balance recovers; without one the claimed probe decides.
        if let Some(balance) = self.fetch_live_balance(b, label).await {
            if balance.remaining <= 0 && !balance.usd.is_some_and(|usd| usd > 0.0) {
                return Ok(None);
            }
            let cleared = self.store.clear_quota_exhausted(label, period).await?;
            if cleared {
                let _ = self.store.release_cooldown(label, probe_until).await;
            }
            return Ok(cleared.then_some(ExhaustionRecovery { probe_until: None }));
        }
        let cleared = self.store.clear_quota_exhausted(label, period).await?;
        if cleared {
            // The 402 drained the cached balance; the probe now decides instead.
            let _ = self.store.clear_live(label).await;
        }
        Ok(cleared.then_some(ExhaustionRecovery {
            probe_until: Some(probe_until),
        }))
    }

    async fn recover_legacy_exhaustion(
        &self,
        b: &Bucket,
        label: &str,
        period: &str,
    ) -> Result<Option<ExhaustionRecovery>> {
        let _gate = self.legacy_recovery_gate.lock().await;
        if !self.store.legacy_exhausted(label, period).await? {
            return Ok(None);
        }
        let Some(probe_until) = self
            .store
            .claim_cooldown(label, DEFAULT_RATE_LIMIT_COOLDOWN)
            .await?
        else {
            return Ok(None);
        };
        // Legacy marks mixed 429s with quota; without a readable balance, probe once.
        if b.provider.kind.is_web() {
            let cleared = self.store.clear_legacy_exhausted(label, period).await?;
            return Ok(cleared.then_some(ExhaustionRecovery {
                probe_until: Some(probe_until),
            }));
        }
        let Some(balance) = self.fetch_live_balance(b, label).await else {
            let cleared = self.store.clear_legacy_exhausted(label, period).await?;
            return Ok(cleared.then_some(ExhaustionRecovery {
                probe_until: Some(probe_until),
            }));
        };
        if balance.remaining <= 0 && !balance.usd.is_some_and(|usd| usd > 0.0) {
            return Ok(None);
        }
        // One commit: a sibling checks the marker before the gate and mustn't see the lease alone.
        let cleared = self
            .store
            .clear_legacy_exhausted_and_release_cooldown(label, period, probe_until)
            .await?;
        Ok(cleared.then_some(ExhaustionRecovery { probe_until: None }))
    }

    /// One-shot snapshot that fetches missing live figures inline (CLI `list`/`usage`, MCP usage).
    pub async fn usage_snapshot(&self) -> Result<Vec<UsageView>> {
        self.snapshot(true, true).await
    }

    /// Cached-only snapshot that never blocks on a provider; `warm` fills it in.
    pub async fn usage_snapshot_cached(&self) -> Result<Vec<UsageView>> {
        self.snapshot(false, false).await
    }

    async fn snapshot(&self, fetch: bool, browser_fallback: bool) -> Result<Vec<UsageView>> {
        let per = self
            .buckets
            .iter()
            .map(|b| self.bucket_views(b, fetch, browser_fallback));
        let mut out = Vec::with_capacity(self.buckets.len());
        for r in futures_util::future::join_all(per).await {
            out.extend(r?);
        }
        Ok(out)
    }

    /// Refresh every live cache; the browser fallback keeps a 403ing ChatGPT from staying pending.
    pub async fn warm(&self) {
        let _ = self.snapshot(true, true).await;
    }

    async fn bucket_views(
        &self,
        b: &Bucket,
        fetch: bool,
        browser_fallback: bool,
    ) -> Result<Vec<UsageView>> {
        let proxy = b.proxy.clone().unwrap_or_else(|| "direct".to_string());
        let ll = self.live_limits_for(b, fetch, browser_fallback).await;
        let mut mv = self
            .view(b.provider.kind.as_str(), &b.label, b.quota, b.reset, &proxy)
            .await?;
        mv.unmetered = !b.pinned && b.provider.kind.unmetered(!b.key.is_empty());
        self.patch_live(b, false, &mut mv, fetch).await;
        self.patch_live_balance(b, &mut mv, fetch).await;
        mv.limits = ll.clone();
        let mut split_open = false;
        // Keyless search and fetch spend apart, so each hourly allowance shows on its own.
        if b.provider.kind.split_window(!b.key.is_empty()).is_some() {
            let mut rv = self
                .view(
                    b.provider.kind.as_str(),
                    &format!("{}#read", b.label),
                    b.quota,
                    b.reset,
                    &proxy,
                )
                .await?;
            self.overlay_live(b, &mut rv, b.quota, b.pinned).await;
            split_open = rv.remaining > 0;
            let features: Vec<FeatureLimit> = [("search", &mv), ("read", &rv)]
                .into_iter()
                .filter(|(_, v)| v.fetched_at.is_some())
                .map(|(feature, v)| view_feature(feature, v))
                .collect();
            if !features.is_empty() {
                mv.limits = Some(LiveLimits {
                    features,
                    ..Default::default()
                });
            }
        }
        // Tinyfish search draws on its own pool, apart from the reads this view counts.
        if b.provider.kind == ProviderKind::Tinyfish {
            let (label, quota, _, reset) = budget(b, Capability::Search);
            let sv = self
                .view(b.provider.kind.as_str(), &label, quota, reset, &proxy)
                .await?;
            split_open = sv.remaining > 0;
            mv.limits
                .get_or_insert_with(Default::default)
                .features
                .push(view_feature("search", &sv));
        }
        // Flag accounts whose live figure isn't cached yet so the UI shows a loader.
        if !fetch {
            mv.pending = if b.provider.kind.is_web() {
                !self
                    .live_limits
                    .lock()
                    .map(|m| m.contains_key(&b.label))
                    .unwrap_or(true)
            } else {
                !self
                    .balance
                    .lock()
                    .map(|m| m.contains_key(&b.label))
                    .unwrap_or(true)
            };
        }
        let mut out = vec![mv];
        // Web providers and tinyfish track deep_research against a separate budget.
        if b.provider.kind.dr_budget() {
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
            self.overlay_live(b, &mut dv, b.dr_quota, b.dr_pinned).await;
            self.patch_live(b, true, &mut dv, fetch).await;
            // A live `total` overrides the ceiling too, so a locked tier reads 0/0.
            if let Some(f) = ll.as_ref().and_then(|l| l.feature("deep_research")) {
                dv.remaining = f.remaining.max(0);
                if let Some(t) = f.total {
                    dv.quota = t;
                }
                dv.used = (dv.quota - dv.remaining).max(0);
                dv.exhausted = dv.remaining == 0 || dv.quota == 0;
                if let Some(w) = f.window_secs {
                    dv.window_secs = Some(w);
                }
            }
            // No live feature carries an API provider's research budget, so the main view does.
            if !b.provider.kind.is_web() {
                out[0]
                    .limits
                    .get_or_insert_with(Default::default)
                    .features
                    .push(view_feature("deep_research", &dv));
            }
            out.push(dv);
        }
        // Exhausted means the router skips it: a spent figure it would still re-read is not.
        for v in &mut out {
            let (quota, pinned) = if v.label.ends_with("#dr") {
                (b.dr_quota, b.dr_pinned)
            } else {
                (b.quota, b.pinned)
            };
            v.exhausted = v.remaining <= 0
                && !(split_open && v.label == b.label)
                && !self
                    .unconfirmed(b, &v.label, quota, pinned, &v.period)
                    .await?;
            if v.exhausted {
                v.used = v.used.max(v.quota);
            }
        }
        Ok(out)
    }

    /// Overlay a web provider's live quota (grok), cached briefly; a miss keeps the soft view.
    async fn patch_live(&self, b: &Bucket, deep: bool, v: &mut UsageView, fetch: bool) {
        let Conn::Web(c, _) = &b.conn else { return };
        let key = format!("{}|{}", b.label, deep);
        let cached = self.live.lock().ok().and_then(|m| {
            m.get(&key)
                .map(|(t, lq)| (t.elapsed() < Duration::from_secs(20), *lq))
        });
        let lq = match cached {
            Some((true, lq)) => lq,
            Some((false, lq)) if !fetch => lq,
            None if !fetch => return,
            _ => {
                let Some(lq) = bounded(b.provider.live_quota(c, deep)).await else {
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

    /// Overlay an API-key bucket's live balance, re-read at most every 20s when fetching.
    async fn patch_live_balance(&self, b: &Bucket, v: &mut UsageView, fetch: bool) {
        let Conn::Api(_) = &b.conn else { return };
        let recent = self.balance.lock().ok().is_some_and(|m| {
            m.get(&b.label)
                .is_some_and(|t| t.elapsed() < Duration::from_secs(20))
        });
        if fetch && !recent {
            self.fetch_live_balance(b, &b.label).await;
        }
        self.overlay_live(b, v, b.quota, b.pinned).await;
    }

    /// Overlay the cached live reading on a view, counted down by our calls since it was read.
    async fn overlay_live(&self, b: &Bucket, v: &mut UsageView, quota: i64, pinned: bool) {
        let Conn::Api(_) = &b.conn else { return };
        let Ok(Some(snap)) = self.store.load_live(&v.label).await else {
            return;
        };
        let eff = effective_quota(quota, pinned, Some(&snap), &v.period);
        // Top-ups can push the balance past the original grant, so the gauge ceiling follows it.
        let ceiling = snap.total.max(snap.remaining);
        v.quota = if pinned { ceiling.min(quota) } else { ceiling };
        // The router honors an exhausted mark over any reading until recovery re-checks it.
        v.remaining = if v.exhausted {
            0
        } else {
            (eff - v.used).min(v.quota).max(0)
        };
        v.used = (v.quota - v.remaining).max(0);
        v.exhausted = v.remaining <= 0;
        v.usd = snap.usd;
        v.reset_at = snap.reset_at;
        v.fetched_at = Some(snap.fetched_at);
        v.unmetered = false;
        if let Some(w) = b.provider.kind.split_window(!b.key.is_empty()) {
            v.window_secs = Some(w);
        }
    }

    /// The routing ceiling for a budget label and what is left of it.
    async fn headroom(
        &self,
        label: &str,
        quota: i64,
        pinned: bool,
        period: &str,
    ) -> Result<(i64, i64)> {
        let quota = effective_quota(
            quota,
            pinned,
            self.store.load_live(label).await?.as_ref(),
            period,
        );
        Ok((quota, self.store.remaining(label, quota, period).await?))
    }

    /// The provider decides a spent ceiling nothing can confirm; a pinned cap still holds.
    async fn provider_decides(
        &self,
        label: &str,
        quota: i64,
        pinned: bool,
        period: &str,
    ) -> Result<(i64, i64)> {
        Ok(if pinned {
            (quota, self.store.remaining(label, quota, period).await?)
        } else {
            (i64::MAX, 1)
        })
    }

    /// Re-read a spent budget's stale balance, once a minute across processes; `Some(landed)`.
    async fn refresh_live(
        &self,
        b: &Bucket,
        label: &str,
        quota: i64,
        pinned: bool,
        period: &str,
    ) -> Result<Option<bool>> {
        if !self.unconfirmed(b, label, quota, pinned, period).await? {
            return Ok(None);
        }
        // A read that failed within the minute isn't retried yet.
        let unread = format!("{label}#unread");
        if self.store.cooldown_remaining(&unread).await?.is_some() {
            return Ok(Some(false));
        }
        let lock = format!("{label}#live");
        let Some(until) = self.store.claim_cooldown(&lock, LIVE_READ).await? else {
            return self.await_read(label, &lock, &unread).await;
        };
        let mut lease = Lease {
            store: self.store.clone(),
            label: lock,
            until: Some(until),
        };
        self.fetch_live_balance(b, label).await;
        // Only a reading routing can use counts; one already past its reset is no reading.
        let landed = self
            .store
            .load_live(label)
            .await?
            .is_some_and(|s| fresh(&s));
        if !landed {
            self.store.set_cooldown(&unread, LIVE_REFRESH).await?;
        }
        self.store.release_cooldown(&lease.label, until).await?;
        lease.until = None;
        Ok(Some(landed))
    }

    /// Take the outcome of a sibling's in-flight re-read (any process), waiting at most `LIVE_READ`.
    async fn await_read(&self, label: &str, lock: &str, unread: &str) -> Result<Option<bool>> {
        let deadline = Instant::now() + LIVE_READ;
        loop {
            // Checked first: a released lock means the reading, if any, is already saved.
            let done = self.store.cooldown_remaining(lock).await?.is_none();
            if self
                .store
                .load_live(label)
                .await?
                .is_some_and(|s| fresh(&s))
            {
                return Ok(Some(true));
            }
            if done
                || Instant::now() >= deadline
                || self.store.cooldown_remaining(unread).await?.is_some()
            {
                return Ok(Some(false));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Whether a spent `label` rests on a stale or missing balance reading the router would re-read.
    async fn unconfirmed(
        &self,
        b: &Bucket,
        label: &str,
        quota: i64,
        pinned: bool,
        period: &str,
    ) -> Result<bool> {
        let own = label == b.label || label.strip_suffix("#dr") == Some(b.label.as_str());
        if !own || !reads_balance(b) {
            return Ok(false);
        }
        // No reading can lift a pinned cap the counter already reached.
        let u = self.store.usage_for(label, period).await?;
        if u.exhausted || (pinned && u.used >= quota) {
            return Ok(false);
        }
        Ok(self
            .store
            .load_live(label)
            .await?
            .is_none_or(|s| !fresh(&s)))
    }

    /// Fetch `label`'s balance past the cache, so a stale reading can't clear a mark.
    async fn fetch_live_balance(&self, b: &Bucket, label: &str) -> Option<LiveBalance> {
        let Conn::Api(c) = &b.conn else { return None };
        let research = match label.strip_prefix(b.label.as_str()) {
            Some("") => false,
            Some("#dr") => true,
            _ => return None,
        };
        // Dashboard reads may rotate session cookies, so persist the updates.
        let main = bounded(async {
            match &b.balance_conn {
                // Stored cookies carry the rotations `refresh_session` saved, like the client's jar.
                Some(wc) => match b
                    .provider
                    .live_balance_web(wc, &self.session_cookies(&b.label).await)
                    .await
                {
                    Some((balance, updates)) => {
                        self.refresh_session(b, &updates).await;
                        Some(balance)
                    }
                    None => None,
                },
                None => b.provider.live_balance(&b.key, c).await,
            }
        });
        let (bal, dr) = tokio::join!(main, bounded(b.provider.live_dr_balance(&b.key, c)));
        if let Ok(mut m) = self.balance.lock() {
            m.insert(b.label.clone(), Instant::now());
        }
        match &bal {
            Some(bal) => {
                let _ = self
                    .store
                    .save_live(&b.label, &period_key(b.reset), bal)
                    .await;
            }
            // Routing keeps counting the reading down, but its dollar figure is no longer current.
            None if reads_balance(b) => {
                let _ = self.store.forget_live_usd(&b.label).await;
            }
            None => {}
        }
        if let Some(dr) = &dr {
            let _ = self
                .store
                .save_live(&format!("{}#dr", b.label), &period_key(b.dr_reset), dr)
                .await;
        }
        if research {
            dr
        } else {
            bal
        }
    }

    async fn session_cookies(&self, label: &str) -> Vec<web::Cookie> {
        match self.store.load_session(label).await {
            Ok(Some(raw)) => web::parse_session(&raw).cookies,
            _ => Vec::new(),
        }
    }

    /// Re-save rotated session cookies so polling keeps the NextAuth session alive.
    async fn refresh_session(&self, b: &Bucket, updates: &[(String, String)]) {
        if updates.is_empty() {
            return;
        }
        let Ok(Some(raw)) = self.store.load_session(&b.label).await else {
            return;
        };
        let mut sess = web::parse_session(&raw);
        let mut changed = false;
        for (name, val) in updates {
            for c in sess.cookies.iter_mut().filter(|c| &c.name == name) {
                if &c.value != val {
                    c.value = val.clone();
                    changed = true;
                }
            }
        }
        if !changed {
            return;
        }
        if let Ok(json) = serde_json::to_string(&sess) {
            let _ = self
                .store
                .save_session(&b.label, b.provider.kind.as_str(), &json)
                .await;
        }
    }

    /// Live per-tier limits for a web bucket, cached briefly.
    async fn live_limits_for(
        &self,
        b: &Bucket,
        fetch: bool,
        browser_fallback: bool,
    ) -> Option<LiveLimits> {
        let Conn::Web(c, cookies) = &b.conn else {
            return None;
        };
        let cached = self.live_limits.lock().ok().and_then(|m| {
            m.get(&b.label)
                .map(|(t, v)| (t.elapsed() < LIVE_LIMITS_CACHE, v.clone()))
        });
        match cached {
            Some((true, v)) => return v,            // fresh
            Some((false, v)) if !fetch => return v, // stale, but the caller won't wait on a fetch
            None if !fetch => return None,          // nothing cached and we mustn't block
            _ => {}
        }
        let _browser_gate = if browser_fallback {
            Some(self.browser_gate.lock().await)
        } else {
            None
        };
        // Another request can fill the cache while this one waits for Chromium.
        if let Some((t, value)) = self
            .live_limits
            .lock()
            .ok()
            .and_then(|m| m.get(&b.label).cloned())
        {
            if t.elapsed() < LIVE_LIMITS_CACHE {
                return value;
            }
        }
        let mut fetched =
            bounded_browser(
                b.provider
                    .live_limits(c, &b.label, cookies, browser_fallback),
            )
            .await;
        // Keep rotated cookies so the next process starts from the freshest token.
        if let Some(ll) = &mut fetched {
            if !ll.cookie_updates.is_empty() {
                self.refresh_session(b, &ll.cookie_updates).await;
            }
            if let Some(id) = ll.identity.as_deref().filter(|s| !s.is_empty()) {
                let _ = self.store.set_identity(&b.label, id).await;
            }
            // Stored for later sessions; a miss never shows them as current.
            if let Some(tier) = ll.tier.as_deref().filter(|t| !t.is_empty()) {
                let _ = self.store.set_plan(&b.label, tier).await;
            }
            if ll.has_quotas() {
                if let Ok(raw) = serde_json::to_string(ll) {
                    let _ = self.store.set_limits(&b.label, &raw).await;
                }
            }
        }
        // An HTTP-only miss stays uncached so a later browser attempt still runs.
        if fetched.is_some() || browser_fallback {
            if let Ok(mut m) = self.live_limits.lock() {
                m.insert(b.label.clone(), (Instant::now(), fetched.clone()));
            }
        }
        fetched
    }

    /// A "resets …" note from the provider's live limit for a rate-limited feature.
    async fn reset_hint(&self, b: &Bucket, cap: Capability) -> Option<String> {
        let feature = live_feature(cap)?;
        let ll = self.live_limits_for(b, true, true).await?;
        let f = ll.feature(feature)?;
        if let Some(iso) = f.reset_after.as_deref() {
            return Some(format!("resets at {iso}"));
        }
        f.window_secs
            .map(|w| format!("resets within ~{}", human_dur(w)))
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
            usd: None,
            pending: false,
            reset_at: None,
            fetched_at: None,
            unmetered: false,
        })
    }
}

fn view_feature(feature: &str, v: &UsageView) -> FeatureLimit {
    FeatureLimit {
        feature: feature.into(),
        remaining: v.remaining,
        total: Some(v.quota),
        window_secs: v.window_secs,
        reset_after: v
            .reset_at
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|t| t.to_rfc3339()),
    }
}

/// Bound on a live limit/balance read: one stalled endpoint must never hang the usage fan-out.
async fn bounded<T>(fut: impl std::future::Future<Output = Option<T>>) -> Option<T> {
    tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .ok()
        .flatten()
}

/// Browser-backed ChatGPT limits need extra cold-start time on a small VPS.
async fn bounded_browser<T>(fut: impl std::future::Future<Output = Option<T>>) -> Option<T> {
    tokio::time::timeout(Duration::from_secs(30), fut)
        .await
        .ok()
        .flatten()
}

/// Display label for route/debug logs: drop an internal capability-budget suffix.
fn strip_budget(label: &str) -> &str {
    label
        .strip_suffix("#dr")
        .or_else(|| label.strip_suffix("#image"))
        .or_else(|| label.strip_suffix("#search"))
        .or_else(|| label.strip_suffix("#read"))
        .unwrap_or(label)
}

/// Status for the debug log: the provider's code, 429/402 for rate/quota, else 0.
fn err_code(e: &Error) -> i64 {
    match e {
        Error::Provider { status, .. } => *status as i64,
        Error::RateLimit { .. } => 429,
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

/// The feature whose live allowance gates a capability; `None` uses only the soft counter.
fn live_feature(cap: Capability) -> Option<&'static str> {
    match cap {
        Capability::DeepResearch => Some("deep_research"),
        Capability::Image => Some("image_gen"),
        _ => None,
    }
}

/// Compact usage for the MCP `usage` tool: web sessions with live models + limits, then api keys.
pub fn compact_usage(views: &[UsageView]) -> String {
    let dr: HashMap<&str, &UsageView> = views
        .iter()
        .filter_map(|v| v.label.strip_suffix("#dr").map(|b| (b, v)))
        .collect();
    let mut web = String::new();
    let mut api: Vec<String> = Vec::new();
    for v in views {
        if v.label.ends_with("#dr") {
            continue;
        }
        if v.unmetered || !v.provider.ends_with("_web") {
            let mut line = if v.unmetered {
                format!("{} ∞ (rate-limited)", v.provider)
            } else {
                format!("{} {}", v.provider, v.remaining)
            };
            if let Some(ll) = v.limits.as_ref().filter(|l| !l.features.is_empty()) {
                let feats: Vec<String> = ll.features.iter().map(fmt_feature).collect();
                line.push_str(&format!(" ({})", feats.join(", ")));
            }
            api.push(line);
            continue;
        }
        let tier = v
            .limits
            .as_ref()
            .and_then(|l| l.tier.as_deref())
            .unwrap_or("—");
        web.push_str(&format!("  {}/{} · {tier}\n", v.provider, v.label));
        match v.limits.as_ref().filter(|l| !l.models.is_empty()) {
            Some(ll) => {
                let parts: Vec<String> = ll.models.iter().map(fmt_model).collect();
                web.push_str(&format!("    models: {}\n", parts.join(" · ")));
            }
            None => web.push_str("    models: (none — authenticate this provider account)\n"),
        }
        if let Some(ll) = v.limits.as_ref().filter(|l| !l.features.is_empty()) {
            let feats: Vec<String> = ll.features.iter().map(fmt_feature).collect();
            web.push_str(&format!("    limits: {}\n", feats.join(" · ")));
        } else if let Some(d) = dr.get(v.label.as_str()) {
            web.push_str(&format!(
                "    deep_research (soft): {}/{}\n",
                d.remaining, d.quota
            ));
        }
    }
    let web = match web.is_empty() {
        true => String::new(),
        false => format!("web sessions — live models + limits (pass a model/mode back to search/deep_research):\n{web}\n"),
    };
    format!(
        "{web}api keys: {}\n\nextras: {}\n\ntip: an unknown model/mode makes the tool return the live options; the router also auto-fails-over on its own.",
        api.join(" · "),
        extras_index()
    )
}

/// One-line teaser of the non-web escape-hatch modes, pointing at `usage(provider=…)`.
fn extras_index() -> String {
    let mut idx: Vec<String> = ProviderKind::all()
        .iter()
        .filter(|k| !k.is_web())
        .filter_map(|&k| {
            let modes = providers::extras(k).modes;
            if modes.is_empty() {
                return None;
            }
            let names: Vec<&str> = modes.iter().take(2).map(|(n, _)| *n).collect();
            Some(format!("{}({})", k.as_str(), names.join("·")))
        })
        .collect();
    idx.push("→ usage(provider=…) for params & examples".to_string());
    idx.join(" · ")
}

/// The sheet `usage(provider=…)` returns: live limit lines plus the static extras.
pub fn provider_sheet(kind: ProviderKind, views: &[UsageView]) -> String {
    use std::fmt::Write;
    let ex = providers::extras(kind);
    let mut s = format!("{} — {}\n", kind.as_str(), kind.blurb());
    for v in views
        .iter()
        .filter(|v| v.provider == kind.as_str() && !v.label.ends_with("#dr"))
    {
        if v.unmetered {
            let _ = writeln!(s, "  {} — ∞ (rate-limited)", v.label);
        } else {
            let reset = v
                .reset_at
                .and_then(chrono::DateTime::from_timestamp_millis)
                .map(|t| format!(" · resets {}", t.format("%Y-%m-%d %H:%M UTC")))
                .unwrap_or_default();
            let _ = writeln!(s, "  {} — {}/{} left{reset}", v.label, v.remaining, v.quota);
        }
        if let Some(ll) = &v.limits {
            if let Some(t) = &ll.tier {
                let _ = writeln!(s, "    tier: {t}");
            }
            if !ll.models.is_empty() {
                let parts: Vec<String> = ll.models.iter().map(fmt_model).collect();
                let _ = writeln!(s, "    models: {}", parts.join(" · "));
            }
            if !ll.features.is_empty() {
                let feats: Vec<String> = ll.features.iter().map(fmt_feature).collect();
                let _ = writeln!(s, "    limits: {}", feats.join(" · "));
            }
        }
    }
    if !ex.niches.is_empty() {
        s.push_str("niches:\n");
        for n in ex.niches {
            let _ = writeln!(s, "  {n}");
        }
    }
    if !ex.modes.is_empty() {
        s.push_str("modes (pass as `mode`):\n");
        for (m, desc) in ex.modes {
            if desc.is_empty() {
                let _ = writeln!(s, "  {m}");
            } else {
                let _ = writeln!(s, "  {m} — {desc}");
            }
        }
    }
    s.push_str("examples:\n");
    for e in ex.examples {
        let _ = writeln!(s, "  {e}");
    }
    s
}

fn fmt_feature(f: &FeatureLimit) -> String {
    match f.total {
        Some(t) => format!("{} {}/{}", f.feature, f.remaining, t),
        None => format!("{} {}", f.feature, f.remaining),
    }
}

/// One model/mode as a compact token: `name[levels] value·window`, `LOCKED` when the tier can't use it.
fn fmt_model(m: &ModelInfo) -> String {
    let levels = if m.levels.is_empty() {
        String::new()
    } else {
        format!("[{}]", m.levels.join("/"))
    };
    let val = if m.locked {
        " LOCKED".to_string()
    } else {
        match (m.remaining, m.total) {
            (Some(r), Some(t)) => format!(" {r}/{t}"),
            (Some(r), None) => format!(" {r}"),
            _ => String::new(),
        }
    };
    let win = m
        .window_secs
        .map(|w| format!("·{}h", w / 3600))
        .unwrap_or_default();
    format!("{}{levels}{val}{win}", m.name)
}

/// Append the reset hint and a pick-another nudge to a rate/quota message.
fn enrich_limit(msg: String, hint: Option<String>) -> String {
    match hint {
        Some(h) => format!("{msg} — {h}; try another provider or mode"),
        None => format!("{msg} — try another provider or mode"),
    }
}

/// Coarse human duration for a rolling window: "24h", "2h", "30m".
fn human_dur(secs: i64) -> String {
    if secs <= 0 {
        "now".to_string()
    } else if secs % 86400 == 0 {
        format!("{}d", secs / 86400)
    } else if secs >= 3600 {
        format!("{}h", secs / 3600)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

fn human_wait(wait: Duration) -> String {
    let seconds = wait
        .as_secs()
        .saturating_add(u64::from(wait.subsec_nanos() != 0));
    human_dur(seconds.min(i64::MAX as u64) as i64)
}

fn reads_balance(b: &Bucket) -> bool {
    b.balance_conn.is_some()
        || (matches!(b.conn, Conn::Api(_)) && b.provider.kind.has_live_balance(!b.key.is_empty()))
}

/// (db label, quota, pinned, reset) for a bucket+capability.
fn budget(b: &Bucket, cap: Capability) -> (String, i64, bool, Reset) {
    if b.provider.kind.dr_budget() {
        match cap {
            Capability::DeepResearch => {
                return (
                    format!("{}#dr", b.label),
                    b.dr_quota,
                    b.dr_pinned,
                    b.dr_reset,
                )
            }
            // A spent image allowance must not disable chat/search on the same web account.
            Capability::Image => {
                return (
                    format!("{}#image", b.label),
                    b.quota,
                    b.pinned,
                    Reset::Daily,
                )
            }
            _ => {}
        }
    }
    if cap == Capability::Search {
        if let Some(pool) = b.provider.kind.search_pool() {
            return (format!("{}#search", b.label), pool, false, b.reset);
        }
    }
    // Keyless search and fetch each get their own hourly allowance.
    if cap == Capability::Read && b.provider.kind.split_window(!b.key.is_empty()).is_some() {
        return (format!("{}#read", b.label), b.quota, b.pinned, b.reset);
    }
    (b.label.clone(), b.quota, b.pinned, b.reset)
}

/// A reading young enough that routing trusts it without a re-read.
fn fresh(s: &LiveRow) -> bool {
    chrono::Utc::now().timestamp_millis() - s.fetched_at < LIVE_REFRESH.as_millis() as i64
}

/// Routing ceiling for an account without a configured quota.
pub fn default_ceiling(kind: ProviderKind, keyed: bool) -> i64 {
    if kind.unmetered(keyed) {
        UNMETERED
    } else {
        kind.default_quota()
    }
}

/// Routing ceiling: the cached live limit (capped when pinned), else the configured estimate.
pub fn effective_quota(quota: i64, pinned: bool, live: Option<&LiveRow>, period: &str) -> i64 {
    let Some(snap) = live else { return quota };
    // A reading from an earlier local period still owes the calls made after it in that period.
    let base = if snap.period == period {
        snap.used_at_fetch
    } else {
        snap.used_at_fetch - snap.used_now
    };
    let eff = base + snap.remaining;
    if pinned {
        quota.min(eff)
    } else {
        eff
    }
}

fn image_error_can_failover(err: &Error) -> bool {
    matches!(
        err,
        Error::Provider { status: 401, .. }
            | Error::BadResponse("chatgpt_web: composer drive failed")
    )
}

pub(crate) fn decode_session_affinity(session: &str) -> Option<(String, String)> {
    let (encoded, opaque) = session.split_once(':')?;
    let label = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())?;
    Some((label, opaque.to_string()))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn mi(
        name: &str,
        levels: &[&str],
        r: Option<i64>,
        t: Option<i64>,
        w: Option<i64>,
        locked: bool,
    ) -> ModelInfo {
        ModelInfo {
            id: name.to_ascii_lowercase(),
            name: name.into(),
            levels: levels.iter().map(|s| s.to_string()).collect(),
            remaining: r,
            total: t,
            window_secs: w,
            reset_after: None,
            locked,
        }
    }

    #[test]
    fn fmt_model_renders_each_shape() {
        assert_eq!(
            fmt_model(&mi("Fast", &[], Some(7), Some(7), Some(86400), false)),
            "Fast 7/7·24h"
        );
        assert_eq!(
            fmt_model(&mi("Heavy", &[], Some(0), Some(0), None, true)),
            "Heavy LOCKED"
        );
        assert_eq!(
            fmt_model(&mi(
                "GPT-5.5",
                &["instant", "medium", "high"],
                None,
                None,
                None,
                false
            )),
            "GPT-5.5[instant/medium/high]"
        );
        assert_eq!(
            fmt_model(&mi(
                "Pro",
                &["standard", "extended"],
                None,
                None,
                None,
                false
            )),
            "Pro[standard/extended]"
        );
    }

    #[test]
    fn provider_sheet_renders_extras_and_live_line() {
        let sheet = provider_sheet(ProviderKind::Serper, &[]);
        assert!(sheet.contains("serper"));
        assert!(sheet.contains("patents"));
        assert!(sheet.contains("examples:"));

        let v = UsageView {
            provider: "serper",
            label: "serper".into(),
            period: "once".into(),
            quota: 2500,
            used: 100,
            remaining: 2400,
            exhausted: false,
            proxy: "direct".into(),
            window_secs: None,
            limits: None,
            usd: None,
            pending: false,
            reset_at: None,
            fetched_at: None,
            unmetered: false,
        };
        let sheet = provider_sheet(ProviderKind::Serper, std::slice::from_ref(&v));
        assert!(sheet.contains("2400/2500 left"));
        let v = UsageView {
            reset_at: Some(1_791_541_677_000),
            ..v
        };
        let sheet = provider_sheet(ProviderKind::Serper, &[v]);
        assert!(sheet.contains("2400/2500 left · resets 2026-10-09 10:27 UTC"));
    }

    #[test]
    fn effective_quota_follows_live_snapshot() {
        let snap = |period: &str, used_at_fetch, remaining| LiveRow {
            period: period.into(),
            used_at_fetch,
            used_now: used_at_fetch,
            remaining,
            total: 0,
            usd: None,
            reset_at: None,
            fetched_at: 0,
        };
        assert_eq!(effective_quota(100, false, None, "p"), 100);
        assert_eq!(
            effective_quota(100, false, Some(&snap("p", 90, 910)), "p"),
            1000
        );
        assert_eq!(
            effective_quota(100, false, Some(&snap("p", 90, 0)), "p"),
            90
        );
        assert_eq!(
            effective_quota(100, true, Some(&snap("p", 90, 910)), "p"),
            100
        );
        assert_eq!(
            effective_quota(100, false, Some(&snap("old", 90, 40)), "p"),
            40
        );
        let rolled = LiveRow {
            used_now: 94,
            ..snap("old", 90, 40)
        };
        assert_eq!(effective_quota(100, false, Some(&rolled), "p"), 36);
    }

    #[test]
    fn extras_index_points_at_usage() {
        let idx = extras_index();
        assert!(idx.contains("serper("));
        assert!(idx.contains("usage(provider"));
    }

    #[test]
    fn session_affinity_round_trips_arbitrary_labels() {
        let label = "friends: ChatGPT / main";
        let token = format!(
            "{}:conversation|message",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(label)
        );
        assert_eq!(
            decode_session_affinity(&token),
            Some((label.to_string(), "conversation|message".to_string()))
        );
        assert_eq!(decode_session_affinity("legacy-conversation|message"), None);
    }

    #[test]
    fn image_errors_fail_over_only_before_submit() {
        assert!(image_error_can_failover(&Error::Provider {
            provider: "chatgpt_web",
            status: 401,
            body: "not logged in".into(),
        }));
        assert!(image_error_can_failover(&Error::BadResponse(
            "chatgpt_web: composer drive failed"
        )));
        assert!(!image_error_can_failover(&Error::Timeout(
            "chatgpt_web: browser drive"
        )));
        assert!(!image_error_can_failover(&Error::rate_limit(
            "chatgpt_web: wait before retrying"
        )));
    }

    #[test]
    fn web_image_budget_is_separate_and_daily() {
        let bucket = Bucket {
            provider: Provider::new(ProviderKind::GeminiWeb),
            conn: Conn::Web(wreq::Client::new(), Vec::new()),
            key: String::new(),
            label: "gemini-1".into(),
            quota: 100,
            reset: Reset::Monthly,
            dr_quota: 10,
            dr_reset: Reset::Daily,
            pinned: false,
            dr_pinned: false,
            proxy: None,
            balance_conn: None,
        };
        assert_eq!(
            budget(&bucket, Capability::Image),
            ("gemini-1#image".into(), 100, false, Reset::Daily)
        );
        assert_eq!(
            budget(&bucket, Capability::Search),
            ("gemini-1".into(), 100, false, Reset::Monthly)
        );
    }

    #[tokio::test]
    async fn missing_session_account_returns_affinity_error() {
        let path = std::env::temp_dir().join(format!(
            "fetchira_router_affinity_{}_{}.db",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let store = Store::open(path.to_str().expect("temp path"))
            .await
            .expect("open store");
        let router = Router::from_parts(vec![], store);
        let label = "missing-account";
        let input = Input {
            session: Some(format!(
                "{}:opaque",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(label)
            )),
            ..Default::default()
        };
        let err = router
            .call(Capability::Search, &input, Some(ProviderKind::ChatgptWeb))
            .await
            .expect_err("missing affinity account must fail");
        assert!(matches!(
            err,
            Error::ProviderForced(message)
                if message.contains("chatgpt_web") && message.contains(label)
        ));
    }

    #[tokio::test]
    async fn http_only_limit_miss_stays_uncached_for_browser_fallback() {
        let path = std::env::temp_dir().join(format!(
            "fetchira_router_limits_{}_{}.db",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let store = Store::open(path.to_str().expect("temp path"))
            .await
            .expect("open store");
        let bucket = Bucket {
            provider: Provider::new(ProviderKind::Tavily),
            conn: Conn::Web(wreq::Client::new(), Vec::new()),
            key: String::new(),
            label: "limits-account".into(),
            quota: 100,
            reset: Reset::Monthly,
            dr_quota: 100,
            dr_reset: Reset::Monthly,
            pinned: false,
            dr_pinned: false,
            proxy: None,
            balance_conn: None,
        };
        let router = Router::from_parts(vec![bucket], store);
        assert!(router
            .live_limits_for(&router.buckets[0], true, false)
            .await
            .is_none());
        assert!(!router
            .live_limits
            .lock()
            .expect("limits lock")
            .contains_key("limits-account"));
        assert!(router
            .live_limits_for(&router.buckets[0], true, true)
            .await
            .is_none());
        assert!(router
            .live_limits
            .lock()
            .expect("limits lock")
            .contains_key("limits-account"));
    }
}
