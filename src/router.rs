use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
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
    // Same idea for the richer per-tier limits (chatgpt_web), keyed by label. Browser misses are
    // cached; HTTP-only misses are not, so a later browser-enabled snapshot can retry.
    live_limits: Mutex<HashMap<String, (Instant, Option<LiveLimits>)>>,
    // Live API-key balances (serper/tavily/firecrawl/steel), keyed by label. `None` caches a
    // miss so a provider without a usable balance endpoint isn't re-polled every snapshot.
    balance: Mutex<HashMap<String, (Instant, Option<LiveBalance>)>>,
    // `Some(retention_hours)` records every attempt to the debug log; `None` is disabled.
    debug: Option<i64>,
    // ponytail: serialize hosted Chromium work; overlapping browser processes reset CDP on small VPSes.
    browser_gate: Arc<tokio::sync::Mutex<()>>,
    // Legacy 429 marks are reconciled once per process at a time so concurrent callers do not
    // stampede the provider's balance endpoint during an upgrade.
    legacy_recovery_gate: tokio::sync::Mutex<()>,
}

const LIVE_LIMITS_CACHE: Duration = Duration::from_secs(30);
const DEFAULT_RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(60);

struct ExhaustionRecovery {
    /// A one-shot probe lease is released only after the provider accepts the request.
    probe_until: Option<i64>,
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
                        "no web session for {}; authenticate this provider account",
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
            browser_gate: Arc::new(tokio::sync::Mutex::new(())),
            legacy_recovery_gate: tokio::sync::Mutex::new(()),
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

        // A forced provider is tried directly (it may serve the cap without being in the auto-route
        // order, e.g. read via tavily/serper/steel); auto-choice walks the preference order.
        let order = match forced {
            Some(f) if !f.supports(cap) => return Err(Error::Unsupported(f.as_str())),
            Some(f) => vec![f],
            None => order_for(cap, input.topic.as_deref(), self.priority.for_cap(cap)),
        };
        for &kind in &order {
            let mut cands: Vec<(usize, i64, String, i64, String, Option<i64>)> = Vec::new();
            for (i, b) in self.buckets.iter().enumerate() {
                if b.provider.kind != kind {
                    continue;
                }
                if account_label.is_some_and(|label| b.label != label) {
                    continue;
                }
                let (blabel, bquota, breset) = budget(b, cap);
                let period = period_key(breset);
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
                let mut rem = self.store.remaining(&blabel, bquota, &period).await?;
                let mut probe_until = None;
                if rem == 0 {
                    if let Some(recovery) =
                        self.recover_exhaustion(b, cap, &blabel, &period).await?
                    {
                        probe_until = recovery.probe_until;
                        rem = self.store.remaining(&blabel, bquota, &period).await?;
                    } else if self.store.cooldown_remaining(&blabel).await?.is_none() {
                        // A sibling may have cleared the mark from a positive live balance while
                        // this caller waited on recovery. Re-read only when it left no probe lease;
                        // an active lease still owns the sole no-balance request.
                        rem = self.store.remaining(&blabel, bquota, &period).await?;
                    }
                }
                // For tool-gated capabilities, trust the provider's live allowance over the soft
                // counter: skip a bucket the provider says is exhausted (proactive failover). Only
                // when auto-choosing — a *forced* provider is attempted regardless (limits may be
                // stale), so an explicit request can still go through or surface a real rate-limit.
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
                    cands.push((i, rem, blabel, bquota, period, probe_until));
                }
            }
            cands.sort_by_key(|a| std::cmp::Reverse(a.1));

            for (i, _rem, blabel, bquota, period, probe_until) in cands {
                let b = &self.buckets[i];
                // Reserve the slot *before* the network call so concurrent tasks can't all clear the
                // same `remaining > 0` gate and stampede one account past its quota. Claim a nominal
                // 1 (the per-call minimum), settle the real cost on success, refund on any failure.
                // A `false` means a sibling took the last slot meanwhile — move to the next account.
                if !self
                    .store
                    .reserve(kind.as_str(), &blabel, bquota, &period, 1, probe_until)
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
                        // Image + deep-research polls stay in the browser: the HTTP chatgpt_web
                        // path mints a fresh bearer per call and chokes on the rolling NextAuth
                        // rotation that an already-authenticated Chromium tolerates.
                        let _browser_gate = self.browser_gate.lock().await;
                        providers::chatgpt_browser::run(cookies, cap, input).await
                    }
                    // ChatGPT drives a real browser for chat / deep-research / image. The pure-HTTP
                    // turn (chatgpt_web) is blocked by OpenAI's "unusual activity" anti-bot gate
                    // (429/403) that the cookie client can't pass even with a freshly-minted sentinel
                    // token, so the turn always goes through the browser. chatgpt_web is still used
                    // for cookie-only reads (limits / tier / identity), which aren't anti-bot gated.
                    Conn::Web(_, cookies) if b.provider.kind == ProviderKind::ChatgptWeb => {
                        let _browser_gate = self.browser_gate.lock().await;
                        providers::chatgpt_browser::run(cookies, cap, input).await
                    }
                    Conn::Web(c, _) => b.provider.call_web(c, cap, input, &b.label).await,
                };
                // Persist rotated session cookies a web turn captured on a bearer cache-miss, so the
                // next process starts from the freshest token (keeps the session alive across procs).
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
                        // The reserved unit never became real usage — give it back.
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
                                // A provider-side 402 is authoritative for now, but a top-up must
                                // eventually revive even a `Reset::Once` account. One process gets
                                // a fresh balance check/probe after this persisted interval.
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
                            Error::Provider { .. }
                            | Error::Transport(_)
                            | Error::Timeout(_)
                            | Error::BadResponse(_) => {
                                prev_fail = Some((acct.to_string(), 0));
                                last_err = Some(e);
                            }
                            _ => return Err(e),
                        }
                        // Image may already have submitted the prompt. Fail over only when
                        // this account never accepted the turn (login / composer).
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

    /// Recover legacy 429 marks and periodically recheck confirmed provider quota denials. The
    /// persisted probe claim prevents separate CLI/MCP/hosted processes from retrying together.
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

        // Web quota signals are feature-specific. Prefer a fresh live allowance when available;
        // otherwise allow the claimed request to probe once after the cooldown.
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

        // API providers with a balance endpoint recover only from a fresh positive balance. When
        // no balance can be read, clear the marker for this claimed one-shot provider probe.
        if let Some(balance) = self.fetch_live_balance(b).await {
            if let Ok(mut cache) = self.balance.lock() {
                cache.insert(b.label.clone(), (Instant::now(), Some(balance)));
            }
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
        // Legacy versions did not distinguish 429 from quota. A web account or an API provider
        // without readable balance gets one bounded request; a new denial recreates a typed mark.
        if b.provider.kind.is_web() {
            let cleared = self.store.clear_legacy_exhausted(label, period).await?;
            return Ok(cleared.then_some(ExhaustionRecovery {
                probe_until: Some(probe_until),
            }));
        }
        let Some(balance) = self.fetch_live_balance(b).await else {
            let cleared = self.store.clear_legacy_exhausted(label, period).await?;
            return Ok(cleared.then_some(ExhaustionRecovery {
                probe_until: Some(probe_until),
            }));
        };
        if let Ok(mut cache) = self.balance.lock() {
            cache.insert(b.label.clone(), (Instant::now(), Some(balance)));
        }
        if balance.remaining <= 0 && !balance.usd.is_some_and(|usd| usd > 0.0) {
            return Ok(None);
        }
        // Clear the marker and release the lease in one commit. A sibling checks the marker before
        // entering `legacy_recovery_gate`; separate writes would let it skip the gate while the
        // lease is still visible and incorrectly report a 60-second cooldown.
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

    /// Cached-only snapshot: never blocks on a provider, so the dashboard paints instantly and the
    /// background `warm` loop fills each account's limits/balance in as its fetch lands.
    pub async fn usage_snapshot_cached(&self) -> Result<Vec<UsageView>> {
        self.snapshot(false, false).await
    }

    async fn snapshot(&self, fetch: bool, browser_fallback: bool) -> Result<Vec<UsageView>> {
        // Fan the buckets out concurrently: a fetching snapshot waits for the slowest single
        // provider (not the sum), a cached one returns immediately.
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

    /// Refresh every live cache (called on a timer by the dashboard) so cached snapshots stay fresh
    /// without any request blocking on a cold provider fan-out. Includes ChatGPT's CDP fallback:
    /// HTTP-only misses are not cached, so a hosted VPS that 403s `conversation/init` would
    /// otherwise leave Accounts on `pending` forever.
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
        let Conn::Api(_) = &b.conn else { return };
        let cached = self.balance.lock().ok().and_then(|m| {
            m.get(&b.label)
                .map(|(t, bal)| (t.elapsed() < Duration::from_secs(20), *bal))
        });
        let bal = match cached {
            Some((true, bal)) => bal,
            Some((false, bal)) if !fetch => bal,
            None if !fetch => return,
            _ => {
                let fresh = self.fetch_live_balance(b).await;
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

    /// Fetch a balance without consulting the cache. Recovery uses this so a stale positive cache
    /// can never clear a durable-looking legacy exhaustion mark.
    async fn fetch_live_balance(&self, b: &Bucket) -> Option<LiveBalance> {
        let Conn::Api(c) = &b.conn else { return None };
        // exa/parallel read their $ balance through the dashboard cookie session; the rest through
        // the api-key. Dashboard reads may rotate cookies, so persist those updates.
        bounded(async {
            match &b.balance_conn {
                Some(wc) => match b.provider.live_balance_web(wc).await {
                    Some((balance, updates)) => {
                        self.refresh_session(b, &updates).await;
                        Some(balance)
                    }
                    None => None,
                },
                None => b.provider.live_balance(&b.key, c).await,
            }
        })
        .await
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

    /// Live per-tier limits for a web bucket, cached briefly. Browser misses are cached so an
    /// unsupported provider isn't re-polled, but HTTP-only misses stay uncached for fallback.
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
        let mut fresh =
            bounded_browser(
                b.provider
                    .live_limits(c, &b.label, cookies, browser_fallback),
            )
            .await;
        // Persist any rotated session cookies captured on a bearer cache-miss, so the next process
        // starts from the freshest token instead of rotating from a stale one.
        if let Some(ll) = &mut fresh {
            if !ll.cookie_updates.is_empty() {
                self.refresh_session(b, &ll.cookie_updates).await;
            }
            if let Some(id) = ll.identity.as_deref().filter(|s| !s.is_empty()) {
                let _ = self.store.set_identity(&b.label, id).await;
            }
            // Persist a successful live read for later sessions; never overlay stored plan/quotas
            // onto a miss — the dashboard must not show last-seen numbers as current.
            if let Some(tier) = ll.tier.as_deref().filter(|t| !t.is_empty()) {
                let _ = self.store.set_plan(&b.label, tier).await;
            }
            if ll.has_quotas() {
                if let Ok(raw) = serde_json::to_string(ll) {
                    let _ = self.store.set_limits(&b.label, &raw).await;
                }
            }
        }
        // An HTTP-only miss must not hide the browser fallback from a later full snapshot or
        // reset hint. Successful HTTP reads remain reusable; browser attempts may cache misses.
        if fresh.is_some() || browser_fallback {
            if let Ok(mut m) = self.live_limits.lock() {
                m.insert(b.label.clone(), (Instant::now(), fresh.clone()));
            }
        }
        fresh
    }

    /// When a gated call is rate-limited, a "resets …" note from the provider's live limit for that
    /// feature (an absolute reset for chatgpt, a rolling window for grok). `None` if unavailable.
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
        .unwrap_or(label)
}

/// HTTP-ish status for the debug log: the provider's real code when it has one, else 429/402 for
/// the rate/quota cases and 0 for transport/shape errors.
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

/// (db label, quota, reset) for a bucket+capability. Web deep research and image generation use
/// separate feature budgets so exhausting one does not disable ordinary chat/search.
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
            None => web.push_str("    models: (none — authenticate this provider account)\n"),
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

fn human_wait(wait: Duration) -> String {
    let seconds = wait
        .as_secs()
        .saturating_add(u64::from(wait.subsec_nanos() != 0));
    human_dur(seconds.min(i64::MAX as u64) as i64)
}

fn budget(b: &Bucket, cap: Capability) -> (String, i64, Reset) {
    if b.provider.kind.is_web() {
        match cap {
            Capability::DeepResearch => return (format!("{}#dr", b.label), b.dr_quota, b.dr_reset),
            // Image limits are feature-specific and normally daily. Never let one exhausted image
            // allowance disable ordinary search/chat on the same web account.
            Capability::Image => return (format!("{}#image", b.label), b.quota, Reset::Daily),
            _ => {}
        }
    }
    (b.label.clone(), b.quota, b.reset)
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
            proxy: None,
            balance_conn: None,
        };
        assert_eq!(
            budget(&bucket, Capability::Image),
            ("gemini-1#image".into(), 100, Reset::Daily)
        );
        assert_eq!(
            budget(&bucket, Capability::Search),
            ("gemini-1".into(), 100, Reset::Monthly)
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
            // Tavily has no web-limit implementation, making this an immediate HTTP-only miss
            // while exercising the same cache branch as ChatGPT's failed HTTP probe.
            provider: Provider::new(ProviderKind::Tavily),
            conn: Conn::Web(wreq::Client::new(), Vec::new()),
            key: String::new(),
            label: "limits-account".into(),
            quota: 100,
            reset: Reset::Monthly,
            dr_quota: 100,
            dr_reset: Reset::Monthly,
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
