use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::config::{resolve_secret, Config, Priority, Reset};
use crate::error::{Error, Result};
use crate::httptrace;
use crate::providers::{
    self, order_for, Capability, Input, LiveBalance, LiveLimits, LiveQuota, ModelInfo, OutImage,
    Provider, ProviderKind,
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
    /// Cookie client for reading the dashboard balance of a `balance_session` provider (exa/parallel),
    /// separate from the api-key `conn` that does the actual calls. `None` until `fetchira login`.
    pub balance_conn: Option<wreq::Client>,
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
    /// Real dollar balance for top-up $ providers (exa/parallel/steel); None for credit providers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usd: Option<f64>,
    /// Cached snapshot only: the live figure isn't fetched yet, so the dashboard shows a loader
    /// instead of the soft placeholder. Never set in a fetching snapshot.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
}

pub struct Router {
    buckets: Vec<Bucket>,
    store: Store,
    // User's per-capability provider order (fetchira priority); empty = built-in order.
    priority: Priority,
    // Short-lived cache of provider-reported quota, keyed by "{label}|{deep}", so polling the
    // dashboard or `usage` doesn't hit grok's rate-limit endpoint on every call.
    live: Mutex<HashMap<String, (Instant, LiveQuota)>>,
    // Same idea for the richer per-tier limits (chatgpt_web), keyed by label. `None` caches a miss.
    live_limits: Mutex<HashMap<String, (Instant, Option<LiveLimits>)>>,
    // Live API-key balances (serper/tavily/firecrawl/steel), keyed by label. `None` caches a
    // miss so a provider without a usable balance endpoint isn't re-polled every snapshot.
    balance: Mutex<HashMap<String, (Instant, Option<LiveBalance>)>>,
    // `Some(retention_hours)` records every attempt to the debug log; `None` is disabled.
    debug: Option<i64>,
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
        }
    }

    /// Replace the user priority after construction (`build` reads it from config; this is for
    /// callers assembling a router from parts, e.g. tests).
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
        // 300s like the web client — above any legit call (exa research is one long POST), so
        // only a truly stalled endpoint trips it.
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
            // exa/parallel keep their api-key for calls but read the live $ balance through a
            // captured dashboard cookie session (best-effort; absent until `fetchira login`).
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
                quota: acc.quota.unwrap_or_else(|| acc.provider.default_quota()),
                reset: acc.reset.unwrap_or_else(|| acc.provider.default_reset()),
                dr_quota: acc.dr_quota.unwrap_or_else(|| acc.provider.dr_quota()),
                dr_reset: acc.dr_reset.unwrap_or_else(|| acc.provider.dr_reset()),
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

        // A forced provider is tried directly (it may serve the cap without being in the auto-route
        // order, e.g. read via tavily/serper/steel); auto-choice walks the preference order.
        let order = match forced {
            Some(f) if !f.supports(cap) => return Err(Error::Unsupported(f.as_str())),
            Some(f) => vec![f],
            None => order_for(cap, input.topic.as_deref(), self.priority.for_cap(cap)),
        };
        for &kind in &order {
            let mut cands: Vec<(usize, i64, String, i64, String)> = Vec::new();
            for (i, b) in self.buckets.iter().enumerate() {
                if b.provider.kind != kind {
                    continue;
                }
                let (blabel, bquota, breset) = budget(b, cap);
                let period = period_key(breset);
                let mut rem = self.store.remaining(&blabel, bquota, &period).await?;
                // For tool-gated capabilities, trust the provider's live allowance over the soft
                // counter: skip a bucket the provider says is exhausted (proactive failover). Only
                // when auto-choosing — a *forced* provider is attempted regardless (limits may be
                // stale), so an explicit request can still go through or surface a real rate-limit.
                if rem > 0 && forced.is_none() {
                    if let Some(feature) = live_feature(cap) {
                        if let Some(live) = self
                            .live_limits_for(b, true)
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
                        // An empty read isn't a real answer — refund and fall through so failover
                        // (and the browser escalation below) get a shot instead of returning blank.
                        if cap == Capability::Read && o.text.trim().is_empty() {
                            let _ = self.store.refund(&blabel, &period, 1).await;
                            prev_fail = Some((acct.to_string(), 0));
                            last_err = Some(Error::BadResponse(kind.as_str()));
                            continue;
                        }
                        // The reservation already charged 1; settle the rest for costlier calls.
                        if o.cost != 1 {
                            let _ = self
                                .store
                                .record(kind.as_str(), &blabel, &period, o.cost - 1)
                                .await;
                        }
                        // Best-effort route log (never fail the call over telemetry).
                        let niche = match providers::niche_native(kind, input) {
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
                        // Deep-research honesty: exa/parallel `deep` bills a live $ balance.
                        let mut text = o.text;
                        if cap == Capability::DeepResearch
                            && input.depth.as_deref() == Some("deep")
                            && matches!(kind, ProviderKind::Exa | ProviderKind::Parallel)
                        {
                            text = format!(
                                "⟦deep research via {} — spends live $ balance⟧\n{text}",
                                kind.as_str()
                            );
                        }
                        let session = o.session.map(|s| format!("{}:{}", kind.as_str(), s));
                        return Ok(Reply {
                            text,
                            session,
                            image: o.image,
                        });
                    }
                    Err(e) => {
                        // The reserved unit never became real usage — give it back.
                        let _ = self.store.refund(&blabel, &period, 1).await;
                        match e {
                            Error::RateLimit(msg) => {
                                let _ = self
                                    .store
                                    .mark_exhausted(kind.as_str(), &blabel, &period)
                                    .await;
                                prev_fail = Some((acct.to_string(), 429));
                                let hint = self.reset_hint(b, cap).await;
                                last_err = Some(Error::RateLimit(enrich_limit(msg, hint)));
                            }
                            Error::QuotaExceeded(msg) => {
                                let _ = self
                                    .store
                                    .mark_exhausted(kind.as_str(), &blabel, &period)
                                    .await;
                                prev_fail = Some((acct.to_string(), 402));
                                let hint = self.reset_hint(b, cap).await;
                                last_err = Some(Error::QuotaExceeded(enrich_limit(msg, hint)));
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

        // Read escalation: if every read backend failed or returned nothing, the page likely needs a
        // real browser — try Steel once before giving up.
        if cap == Capability::Read && forced.is_none() && last_err.is_some() {
            if let Ok(reply) = Box::pin(self.call(Capability::Browser, input, None)).await {
                return Ok(reply);
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

    /// One-shot snapshot that fetches missing live figures inline (CLI `list`/`usage`, MCP usage).
    pub async fn usage_snapshot(&self) -> Result<Vec<UsageView>> {
        self.snapshot(true).await
    }

    /// Cached-only snapshot: never blocks on a provider, so the dashboard paints instantly and the
    /// background `warm` loop fills each account's limits/balance in as its fetch lands.
    pub async fn usage_snapshot_cached(&self) -> Result<Vec<UsageView>> {
        self.snapshot(false).await
    }

    async fn snapshot(&self, fetch: bool) -> Result<Vec<UsageView>> {
        // Fan the buckets out concurrently: a fetching snapshot waits for the slowest single
        // provider (not the sum), a cached one returns immediately.
        let per = self.buckets.iter().map(|b| self.bucket_views(b, fetch));
        let mut out = Vec::with_capacity(self.buckets.len());
        for r in futures_util::future::join_all(per).await {
            out.extend(r?);
        }
        Ok(out)
    }

    /// Refresh every live cache (called on a timer by the dashboard) so cached snapshots stay fresh
    /// without any request blocking on a cold provider fan-out.
    pub async fn warm(&self) {
        let _ = self.snapshot(true).await;
    }

    async fn bucket_views(&self, b: &Bucket, fetch: bool) -> Result<Vec<UsageView>> {
        let proxy = b.proxy.clone().unwrap_or_else(|| "direct".to_string());
        let ll = self.live_limits_for(b, fetch).await;
        let mut mv = self
            .view(b.provider.kind.as_str(), &b.label, b.quota, b.reset, &proxy)
            .await?;
        self.patch_live(b, false, &mut mv, fetch).await;
        self.patch_live_balance(b, &mut mv, fetch).await;
        mv.limits = ll.clone();
        // Cached snapshot: flag accounts whose live figure isn't cached yet so the UI shows a loader
        // instead of a soft placeholder. Web → limits cache; API-key → balance cache.
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
            self.patch_live(b, true, &mut dv, fetch).await;
            // Prefer the provider's live deep-research allowance over the soft counter. A live
            // `total` (grok, tier-aware) overrides the ceiling too, so a locked tier reads 0/0
            // instead of the nominal daily budget.
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
            out.push(dv);
        }
        Ok(out)
    }

    /// Replace the soft local counter with the provider's live figure when it reports one (grok),
    /// caching it briefly. Best-effort: a failed fetch leaves the soft view untouched.
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

    /// Overwrite an API-key bucket's soft counter with the provider's live balance when it reports
    /// one (serper/tavily/firecrawl, and steel on paid tiers). Cached 20s; a miss is cached too,
    /// so providers without a usable endpoint (exa/parallel/steel-free) keep the corrected constant.
    async fn patch_live_balance(&self, b: &Bucket, v: &mut UsageView, fetch: bool) {
        let Conn::Api(c) = &b.conn else { return };
        let cached = self.balance.lock().ok().and_then(|m| {
            m.get(&b.label)
                .map(|(t, bal)| (t.elapsed() < Duration::from_secs(20), *bal))
        });
        let bal = match cached {
            Some((true, bal)) => bal,
            Some((false, bal)) if !fetch => bal,
            None if !fetch => return,
            _ => {
                // exa/parallel read their $ balance through the dashboard cookie session; the rest
                // (serper/tavily/firecrawl/steel) through the api-key. The dashboard re-issues a
                // rolling NextAuth token on every fetch, so re-save it to keep the session alive.
                let fresh = bounded(async {
                    match &b.balance_conn {
                        Some(wc) => match b.provider.live_balance_web(wc).await {
                            Some((bal, updates)) => {
                                self.refresh_session(b, &updates).await;
                                Some(bal)
                            }
                            None => None,
                        },
                        None => b.provider.live_balance(&b.key, c).await,
                    }
                })
                .await;
                if let Ok(mut m) = self.balance.lock() {
                    m.insert(b.label.clone(), (Instant::now(), fresh));
                }
                fresh
            }
        };
        let Some(bal) = bal else { return };
        // Top-ups can push the balance past the original grant, so the gauge ceiling follows it.
        v.quota = bal.total.max(bal.remaining);
        v.remaining = bal.remaining.max(0);
        v.used = (v.quota - v.remaining).max(0);
        v.exhausted = v.remaining <= 0;
        v.usd = bal.usd;
    }

    /// Re-save a `balance_session` account's stored cookies with the rolling token the dashboard
    /// just re-issued (any `Set-Cookie` whose value changed), so polling the balance keeps the
    /// NextAuth session from expiring.
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

    /// Live per-tier limits for a web bucket, cached 20s (a miss is cached too, so a provider
    /// without them isn't re-polled on every snapshot).
    async fn live_limits_for(&self, b: &Bucket, fetch: bool) -> Option<LiveLimits> {
        let Conn::Web(c, _) = &b.conn else {
            return None;
        };
        let cached = self.live_limits.lock().ok().and_then(|m| {
            m.get(&b.label)
                .map(|(t, v)| (t.elapsed() < Duration::from_secs(20), v.clone()))
        });
        match cached {
            Some((true, v)) => return v,            // fresh
            Some((false, v)) if !fetch => return v, // stale, but the caller won't wait on a fetch
            None if !fetch => return None,          // nothing cached and we mustn't block
            _ => {}
        }
        let fresh = bounded(b.provider.live_limits(c)).await;
        if let Ok(mut m) = self.live_limits.lock() {
            m.insert(b.label.clone(), (Instant::now(), fresh.clone()));
        }
        fresh
    }

    /// When a gated call is rate-limited, a "resets …" note from the provider's live limit for that
    /// feature (an absolute reset for chatgpt, a rolling window for grok). `None` if unavailable.
    async fn reset_hint(&self, b: &Bucket, cap: Capability) -> Option<String> {
        let feature = live_feature(cap)?;
        let ll = self.live_limits_for(b, true).await?;
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
        })
    }
}

/// Bound on a live limit/balance read: one stalled endpoint must never hang the usage fan-out.
async fn bounded<T>(fut: impl std::future::Future<Output = Option<T>>) -> Option<T> {
    tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .ok()
        .flatten()
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

/// A compact, LLM-friendly rendering of the usage snapshot for the MCP `usage` tool: web sessions
/// with their live model/mode catalog + limits (exactly what to pass to search/deep_research), then
/// one terse API-key line. Far fewer tokens than the full JSON, and doubles as a selection cheatsheet
/// — the dashboard / `fetchira list` keep the detailed view.
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
        if !v.provider.ends_with("_web") {
            api.push(format!("{} {}", v.provider, v.remaining));
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
            None => web.push_str("    models: (none — check `fetchira login`)\n"),
        }
        if let Some(ll) = v.limits.as_ref().filter(|l| !l.features.is_empty()) {
            let feats: Vec<String> = ll
                .features
                .iter()
                .map(|f| match f.total {
                    Some(t) => format!("{} {}/{}", f.feature, f.remaining, t),
                    None => format!("{} {}", f.feature, f.remaining),
                })
                .collect();
            web.push_str(&format!("    limits: {}\n", feats.join(" · ")));
        } else if let Some(d) = dr.get(v.label.as_str()) {
            web.push_str(&format!(
                "    deep_research (soft): {}/{}\n",
                d.remaining, d.quota
            ));
        }
    }
    format!(
        "web sessions — live models + limits (pass a model/mode back to search/deep_research):\n{web}\napi keys: {}\n\nextras: {}\n\ntip: an unknown model/mode makes the tool return the live options; the router also auto-fails-over on its own.",
        api.join(" · "),
        extras_index()
    )
}

/// One-line teaser of the escape-hatch modes, pointing at `usage(provider=…)` for the full sheet.
/// Non-web providers only (web modes ride along in the model catalog above).
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

/// The full capability sheet for one provider: its live limit/balance line(s) from the snapshot,
/// merged with the static `extras` table (niches, escape-hatch modes, example calls). This is what
/// `usage(provider=…)` returns — everything an agent needs to drive that backend by hand.
pub fn provider_sheet(kind: ProviderKind, views: &[UsageView]) -> String {
    use std::fmt::Write;
    let ex = providers::extras(kind);
    let mut s = format!("{} — {}\n", kind.as_str(), kind.blurb());
    for v in views
        .iter()
        .filter(|v| v.provider == kind.as_str() && !v.label.ends_with("#dr"))
    {
        let _ = writeln!(s, "  {} — {}/{} left", v.label, v.remaining, v.quota);
        if let Some(ll) = &v.limits {
            if let Some(t) = &ll.tier {
                let _ = writeln!(s, "    tier: {t}");
            }
            if !ll.models.is_empty() {
                let parts: Vec<String> = ll.models.iter().map(fmt_model).collect();
                let _ = writeln!(s, "    models: {}", parts.join(" · "));
            }
            if !ll.features.is_empty() {
                let feats: Vec<String> = ll
                    .features
                    .iter()
                    .map(|f| match f.total {
                        Some(t) => format!("{} {}/{}", f.feature, f.remaining, t),
                        None => format!("{} {}", f.feature, f.remaining),
                    })
                    .collect();
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

/// Append the reset window + a pick-another hint to a rate/quota message (the "try something else,
/// here's when it's back" behaviour agents get on a live limit).
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
        // rolling-window live count
        assert_eq!(
            fmt_model(&mi("Fast", &[], Some(7), Some(7), Some(86400), false)),
            "Fast 7/7·24h"
        );
        // locked mode -> LOCKED, no numbers
        assert_eq!(
            fmt_model(&mi("Heavy", &[], Some(0), Some(0), None, true)),
            "Heavy LOCKED"
        );
        // catalog-only with thinking levels (chatgpt)
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
        // no live count (gemini)
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
        // No matching views: still renders the static sheet (modes + examples).
        let sheet = provider_sheet(ProviderKind::Serper, &[]);
        assert!(sheet.contains("serper"));
        assert!(sheet.contains("patents"));
        assert!(sheet.contains("examples:"));

        // A matching view contributes a live "left" line.
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
        };
        let sheet = provider_sheet(ProviderKind::Serper, &[v]);
        assert!(sheet.contains("2400/2500 left"));
    }

    #[test]
    fn extras_index_points_at_usage() {
        let idx = extras_index();
        assert!(idx.contains("serper("));
        assert!(idx.contains("usage(provider"));
    }
}
