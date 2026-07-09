/* Overview: provider grid grouped by capability + pinned live route log. */
const { RouteLogLine, Card, StatusDot, Badge, QuotaMeter, Button } = window.FetchiraDesignSystem_6526df;

function GroupHeader({ label, count }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 12 }}>
      <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, fontWeight: 600, letterSpacing: '0.12em', textTransform: 'uppercase', color: 'var(--text-lo)' }}>{label}</span>
      <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>{count}</span>
      <span style={{ flex: 1, height: 1, background: 'var(--border-faint)' }} />
    </div>
  );
}

// Format the reset instant in the VIEWER's timezone (the browser's): "resets fri 3:59am" for the
// next few days, "resets Jul 30, 12:05am" further out. Null for rolling windows (grok).
function fmtReset(iso) {
  if (!iso) return null;
  const d = new Date(iso);
  if (isNaN(d.getTime())) return null;
  const time = d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' }).replace(' ', '').toLowerCase();
  const days = (d.getTime() - Date.now()) / 86400000;
  if (days < 6) return 'resets ' + d.toLocaleDateString([], { weekday: 'short' }).toLowerCase() + ' ' + time;
  return 'resets ' + d.toLocaleDateString([], { month: 'short', day: 'numeric' }) + ', ' + time;
}


// Accounts closest to empty — remaining + reset cadence + observed burn rate.
function BurnRadar() {
  const burn = window.FX.burn || [];
  if (!burn.length) return null;
  return (
    <Card pad={0} style={{ overflow: 'hidden' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '12px 14px', borderBottom: '1px solid var(--border-faint)' }}>
        <StatusDot tone="low" size={7} />
        <span style={{ fontFamily: 'var(--font-display)', fontSize: 14, fontWeight: 600, color: 'var(--text-hi)' }}>Burn radar</span>
        <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>closest to empty</span>
      </div>
      <div style={{ display: 'flex', flexDirection: 'column' }}>
        {burn.map((b, i) => {
          const rem = b.remaining || 0;
          const tone = rem <= 0 ? 'out' : rem < 5 ? 'low' : 'ok';
          return (
            <div key={b.label + i} style={{ display: 'flex', alignItems: 'center', gap: 10, padding: '9px 14px', borderTop: i ? '1px solid var(--border-faint)' : 'none' }}>
              <StatusDot tone={tone} size={6} />
              <div style={{ minWidth: 0, flex: 1 }}>
                <div style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-hi)', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>{b.label}</div>
                <div style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>{b.provider}</div>
              </div>
              <div style={{ display: 'flex', alignItems: 'baseline', gap: 8, fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>
                {b.resetWindow && <Badge tone="neutral" variant="outline" uppercase>{b.resetWindow}</Badge>}
                {b.ratePerHour > 0 && <span>~{b.ratePerHour}/hr</span>}
                <span><b style={{ color: tone === 'out' ? 'var(--red-500)' : 'var(--text-hi)' }}>{rem.toLocaleString()}</b> left</span>
              </div>
            </div>
          );
        })}
      </div>
    </Card>
  );
}

// Routing priority: the failover chain the router walks per capability. Chips are numbered
// slots joined by "→" (1 is the default, the rest are failover); drag a chip to reorder —
// one save on drop. Providers without an account are dimmed (the router skips them live).
// Saved to fetchira.toml; running MCP servers pick it up on their next start.
function RoutingPriority() {
  const rows = window.FX.priority || [];
  const [open, setOpen] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [err, setErr] = React.useState(null);
  // Local order override per capability while dragging / awaiting the save.
  const [draft, setDraft] = React.useState({});
  const drag = React.useRef(null); // { cap, name, moved }
  if (!rows.length) return null;

  const configured = new Set((window.FX.accounts || []).map((a) => a.provider));
  const orderOf = (row) => draft[row.capability] || row.order;

  const post = async (capability, order) => {
    setBusy(true); setErr(null);
    try {
      await window.apiPost('/api/priority', { capability, order });
      if (window.fxRefresh) await window.fxRefresh();
    } catch (e) { setErr(e.message); }
    finally {
      setBusy(false);
      setDraft((d) => { const n = { ...d }; delete n[capability]; return n; });
    }
  };

  // Pointer-based drag: capture on the chip, live-preview the reorder as the pointer crosses
  // sibling chips, save once on release. A press without movement across a sibling is a no-op.
  const onDown = (row, name) => (e) => {
    if (busy) return;
    drag.current = { cap: row.capability, name, moved: false };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onMove = (row) => (e) => {
    const d = drag.current;
    if (!d || d.cap !== row.capability) return;
    const el = document.elementFromPoint(e.clientX, e.clientY);
    const chip = el && el.closest && el.closest(`[data-prio-cap="${row.capability}"][data-prio-name]`);
    if (!chip) return;
    const over = chip.getAttribute('data-prio-name');
    if (over === d.name) return;
    const order = orderOf(row).slice();
    const from = order.indexOf(d.name);
    const to = order.indexOf(over);
    if (from < 0 || to < 0) return;
    order.splice(from, 1);
    order.splice(to, 0, d.name);
    d.moved = true;
    setDraft((prev) => ({ ...prev, [row.capability]: order }));
  };
  const onUp = (row) => () => {
    const d = drag.current;
    drag.current = null;
    if (d && d.cap === row.capability && d.moved) post(row.capability, orderOf(row));
  };

  const dragged = (row, p) => drag.current && drag.current.cap === row.capability && drag.current.name === p;

  return (
    <section>
      <button onClick={() => setOpen((o) => !o)}
        style={{ display: 'flex', alignItems: 'center', gap: 10, width: '100%', background: 'transparent', border: 'none', cursor: 'pointer', padding: 0, marginBottom: open ? 12 : 0 }}>
        <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, fontWeight: 600, letterSpacing: '0.12em', textTransform: 'uppercase', color: 'var(--text-lo)' }}>Routing priority</span>
        <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>{rows.some((r) => r.custom) ? 'custom' : 'default'}</span>
        <span style={{ flex: 1, height: 1, background: 'var(--border-faint)' }} />
        <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>{open ? '− hide' : '+ show'}</span>
      </button>
      {open && (
        <Card pad={14} style={{ display: 'flex', flexDirection: 'column', gap: 13 }}>
          {err && <div style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--red-500)' }}>{err}</div>}
          {rows.map((row) => (
            <div key={row.capability} style={{ display: 'flex', alignItems: 'center', gap: 10, flexWrap: 'wrap' }}>
              <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-mid)', width: 110, flexShrink: 0 }}>
                {row.capability}{row.custom && <span title="custom order — reset restores the default" style={{ color: 'var(--lime-500)' }}> *</span>}
              </span>
              <div style={{ display: 'flex', alignItems: 'center', gap: 5, flexWrap: 'wrap', rowGap: 7 }}>
                {orderOf(row).map((p, i) => {
                  const noAcct = !configured.has(p);
                  const lift = dragged(row, p);
                  return (
                    <React.Fragment key={p}>
                      {i > 0 && <span style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: 'var(--text-faint)', userSelect: 'none' }}>→</span>}
                      <span data-prio-cap={row.capability} data-prio-name={p}
                        onPointerDown={onDown(row, p)} onPointerMove={onMove(row)} onPointerUp={onUp(row)}
                        title={(noAcct ? 'no account yet — skipped when routing · ' : '') + 'drag to reorder'}
                        style={{
                          display: 'inline-flex', alignItems: 'center', gap: 6, fontFamily: 'var(--font-mono)', fontSize: 11,
                          padding: '3px 9px 3px 7px', borderRadius: 4, cursor: busy ? 'default' : 'grab',
                          touchAction: 'none', userSelect: 'none',
                          color: noAcct ? 'var(--text-faint)' : 'var(--text-hi)',
                          border: i === 0 ? '1px solid color-mix(in srgb, var(--lime-500) 45%, transparent)' : `1px ${noAcct ? 'dashed' : 'solid'} var(--border-faint)`,
                          background: lift ? 'color-mix(in srgb, var(--lime-500) 10%, transparent)' : i === 0 ? 'color-mix(in srgb, var(--lime-500) 5%, transparent)' : 'transparent',
                          boxShadow: lift ? '0 2px 8px rgba(0,0,0,0.45)' : 'none',
                        }}>
                        <span style={{ fontSize: 10, color: i === 0 ? 'var(--lime-500)' : 'var(--text-faint)' }}>{i + 1}</span>
                        {p}
                      </span>
                    </React.Fragment>
                  );
                })}
                {/* steel is read's built-in last resort (browser escalation) even when not in the order */}
                {row.capability === 'read' && !orderOf(row).includes('steel') && (
                  <span title="when every read provider fails or returns empty, the router retries via the steel headless browser automatically"
                    style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)', userSelect: 'none' }}>
                    → steel · auto
                  </span>
                )}
                {row.available.map((p) => (
                  <button key={p} onClick={() => post(row.capability, [...orderOf(row), p])} disabled={busy} title="add to this capability's routing order"
                    style={{ background: 'transparent', border: '1px dashed var(--border-faint)', borderRadius: 4, padding: '3px 9px', cursor: 'pointer', fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>
                    + {p}
                  </button>
                ))}
                {row.custom && (
                  <button onClick={() => post(row.capability, [])} disabled={busy}
                    style={{ background: 'transparent', border: 'none', cursor: 'pointer', padding: 0, marginLeft: 3, fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)', textDecoration: 'underline' }}>
                    reset
                  </button>
                )}
              </div>
            </div>
          ))}
          <span style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: 'var(--text-faint)' }}>
            drag to reorder — 1 is tried first, the rest are failover · dimmed = no account yet · agents can force one with provider=… · running MCP servers pick changes up on restart
          </span>
        </Card>
      )}
    </section>
  );
}

// Human twin of the agent's usage(provider) drill-in: per-provider niches + call modes.
function CapabilityMatrix() {
  const caps = window.FX.capabilities || [];
  const [open, setOpen] = React.useState(false);
  if (!caps.length) return null;
  return (
    <section>
      <button onClick={() => setOpen((o) => !o)}
        style={{ display: 'flex', alignItems: 'center', gap: 10, width: '100%', background: 'transparent', border: 'none', cursor: 'pointer', padding: 0, marginBottom: open ? 12 : 0 }}>
        <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, fontWeight: 600, letterSpacing: '0.12em', textTransform: 'uppercase', color: 'var(--text-lo)' }}>Capability matrix</span>
        <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>{caps.length} {caps.length === 1 ? 'provider' : 'providers'}</span>
        <span style={{ flex: 1, height: 1, background: 'var(--border-faint)' }} />
        <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>{open ? '− hide' : '+ show'}</span>
      </button>
      {open && (
        <div style={{ columnWidth: 290, columnGap: 14 }}>
          {caps.map((c) => (
            <div key={c.provider} style={{ breakInside: 'avoid', marginBottom: 14 }}>
            <Card pad={14} style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
              <span style={{ fontFamily: 'var(--font-mono)', fontSize: 14, fontWeight: 600, color: 'var(--text-hi)', letterSpacing: '-0.01em' }}>{c.provider}</span>
              {(c.niches || []).length > 0 && (
                <div style={{ display: 'flex', flexWrap: 'wrap', gap: 4 }}>
                  {c.niches.map((n) => (
                    <span key={n} style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-lo)', border: '1px solid var(--border-faint)', borderRadius: 4, padding: '1px 5px' }}>{n}</span>
                  ))}
                </div>
              )}
              <div style={{ display: 'flex', flexDirection: 'column', gap: 5 }}>
                {(c.modes || []).map(([mode, desc]) => (
                  <div key={mode} style={{ display: 'flex', alignItems: 'baseline', gap: 6 }}>
                    <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-mid)', flexShrink: 0 }}>{mode}</span>
                    <span style={{ fontFamily: 'var(--font-ui)', fontSize: 12, color: 'var(--text-lo)' }}>{desc}</span>
                  </div>
                ))}
              </div>
              <span style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: 'var(--text-faint)' }}>call usage(provider={c.provider}) for exact calls</span>
            </Card>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

// One limit = its own cube bar (each mode/model/feature has its own quota + reset cadence).
// Fuel-gauge fill: the bar shows what's LEFT (full when fresh), so we feed the meter `remaining`
// as its fill and force the colour from the real remaining (green → amber → red as it drains).
function LimitRow({ label, used, quota, window, resetAt, locked, off, approx, usd }) {
  const q = quota || 0;
  const remaining = Math.max(0, q - (used || 0));
  // Top-up $ providers have no ceiling to drain, so the bar is always full ("tank fuelled") — it's
  // there for visual parity with the credit providers; the real figure is the $ balance.
  if (usd != null) {
    return (
      <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
        <div style={{ display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', gap: 8 }}>
          <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-mid)' }}>{label === 'quota' ? 'balance' : label}</span>
          <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>
            <b style={{ color: 'var(--text-hi)' }}>${usd.toFixed(2)}</b> · ≈ {remaining.toLocaleString()} left
          </span>
        </div>
        <QuotaMeter used={1} quota={1} variant="segments" segments={18} showValues={false} state={off ? 'off' : remaining <= 0 ? 'out' : 'ok'} />
      </div>
    );
  }
  const remFrac = q > 0 ? remaining / q : 0;
  const st = off || locked ? 'off' : remaining <= 0 ? 'out' : remFrac < 0.15 ? 'low' : 'ok';
  const meta = locked ? 'locked' : [window, fmtReset(resetAt)].filter(Boolean).join(' · ');
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
      <div style={{ display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', gap: 8 }}>
        <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: locked ? 'var(--text-faint)' : 'var(--text-mid)' }}>{label}</span>
        <span style={{ display: 'flex', gap: 8, alignItems: 'baseline', fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>
          <span>{locked ? '0/0' : <>{approx ? '≈ ' : ''}<b style={{ color: 'var(--text-hi)' }}>{remaining.toLocaleString()}</b> / {q.toLocaleString()}</>}</span>
          {meta && <span>{meta}</span>}
        </span>
      </div>
      <QuotaMeter used={remaining} quota={q} variant="segments" segments={18} showValues={false} state={st} />
    </div>
  );
}

// A capability limit that reports only a remaining count (create image, file upload) — no ceiling,
// so it's an info row, not a bar.
function FeatureRow({ label, remaining, resetAt }) {
  const reset = fmtReset(resetAt);
  return (
    <div style={{ display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', gap: 8 }}>
      <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-mid)' }}>{label}</span>
      <span style={{ display: 'flex', gap: 8, alignItems: 'baseline', fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>
        <span><b style={{ color: 'var(--text-hi)' }}>{(remaining || 0).toLocaleString()}</b> left</span>
        {reset && <span>{reset}</span>}
      </span>
    </div>
  );
}

function FxProviderCard(p) {
  const [showSpent, setShowSpent] = React.useState(false);
  const needsLogin = p.webSession && !p.loggedIn;
  const health = needsLogin ? 'off' : 'ok';
  const limits = p.limits || [];
  const features = p.features || [];
  const catalog = p.catalog || [];
  // Hide locked / used-up limits behind a toggle so the card leads with what you can actually use.
  const isSpent = (l) => l.locked || (l.quota || 0) - (l.used || 0) <= 0;
  const active = limits.filter((l) => !isSpent(l));
  const spent = limits.filter(isSpent);
  return (
    <Card accent={health} interactive pad={14} style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
      <div style={{ display: 'flex', alignItems: 'flex-start', justifyContent: 'space-between', gap: 8 }}>
        <div style={{ minWidth: 0 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 7 }}>
            <StatusDot tone={health} size={7} />
            <span style={{ fontFamily: 'var(--font-mono)', fontSize: 14, fontWeight: 600, color: 'var(--text-hi)', letterSpacing: '-0.01em' }}>{p.name}</span>
          </div>
          <div style={{ fontFamily: 'var(--font-ui)', fontSize: 12, color: 'var(--text-lo)', marginTop: 3, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>{p.desc}</div>
        </div>
        {p.webSession && <Badge tone={p.loggedIn ? 'cyan' : 'low'} variant="soft">{p.loggedIn ? 'logged in ✓' : 'log in ⚠'}</Badge>}
      </div>

      <div style={{ display: 'flex', flexDirection: 'column', gap: 9 }}>
        {p.pending ? (
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '6px 0', fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-faint)' }}>
            <span style={{ width: 12, height: 12, border: '2px solid var(--border-hairline)', borderTopColor: 'var(--lime-500)', borderRadius: '50%', display: 'inline-block', animation: 'fx-spin 0.8s linear infinite' }} />
            loading limits…
          </div>
        ) : (
          <React.Fragment>
            {active.map((l) => <LimitRow key={l.label} {...l} off={needsLogin} />)}
            {showSpent && spent.map((l) => <LimitRow key={l.label} {...l} off={needsLogin} />)}
            {features.map((f) => <FeatureRow key={f.label} {...f} />)}
          </React.Fragment>
        )}
      </div>

      {!p.pending && spent.length > 0 && (
        <button onClick={() => setShowSpent((s) => !s)}
          style={{ alignSelf: 'flex-start', background: 'transparent', border: 'none', cursor: 'pointer', padding: 0, fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>
          {showSpent ? '− hide locked / used up' : `+ ${spent.length} more · locked / used up`}
        </button>
      )}

      {!p.pending && catalog.length > 0 && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 5 }}>
          <span style={{ fontFamily: 'var(--font-mono)', fontSize: 10, letterSpacing: '0.1em', textTransform: 'uppercase', color: 'var(--text-faint)' }}>chat models · pick per search</span>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 4 }}>
            {catalog.map((m) => (
              <span key={m.name} style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-lo)', border: '1px solid var(--border-faint)', borderRadius: 4, padding: '1px 5px' }}>
                {m.name}{m.levels && m.levels.length ? <span style={{ color: 'var(--text-faint)' }}> ·{m.levels.join('/')}</span> : null}
              </span>
            ))}
          </div>
        </div>
      )}

      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8, paddingTop: 10, borderTop: '1px solid var(--border-faint)' }}>
        {needsLogin ? <Badge tone="off" dot>needs login</Badge> : <Badge tone="ok" dot>healthy</Badge>}
        <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-lo)' }}>{p.accounts} {p.accounts === 1 ? 'acct' : 'accts'}</span>
      </div>
    </Card>
  );
}

// Tag on a route-log row when the request carried a niche filter: green "native" = the filter hit a
// real provider param; amber "rewrite" = served best-effort via query text (hint: add a provider that
// filters natively). Absent field → no tag.
function NicheBadge({ niche }) {
  if (niche !== 'native' && niche !== 'rewrite') return null;
  const native = niche === 'native';
  return (
    <Badge tone={native ? 'ok' : 'low'} variant="soft" uppercase
      title={native ? 'niche filter mapped to a native provider param' : 'niche served via query text — add a provider that filters natively'}
      style={{ height: 16, padding: '0 6px', fontSize: 10, flexShrink: 0 }}>{niche}</Badge>
  );
}

function LogRow(l) {
  if (!l.niche) return <RouteLogLine {...l} />;
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
      <RouteLogLine {...l} style={{ flex: 1, minWidth: 0 }} />
      <NicheBadge niche={l.niche} />
    </div>
  );
}

function LiveLog() {
  const [lines, setLines] = React.useState(() => window.FX.log.map((l, i) => ({ ...l, _id: i, fresh: false })));
  const idRef = React.useRef(window.FX.log.length);
  const [paused, setPaused] = React.useState(false);

  React.useEffect(() => {
    if (paused) return;
    const token = new URLSearchParams(location.search).get('token') || '';
    let es;
    try {
      es = new EventSource('/api/events?token=' + encodeURIComponent(token));
      es.onmessage = (e) => {
        let batch;
        try { batch = JSON.parse(e.data); } catch (err) { return; }
        if (!Array.isArray(batch) || !batch.length) return;
        // Newest on top: prepend the (reversed) batch and keep the freshest 40.
        setLines((prev) => [...batch.slice().reverse().map((l) => ({ ...l, _id: idRef.current++, fresh: true })), ...prev].slice(0, 40));
      };
    } catch (err) { /* no SSE when opened as a static file */ }
    return () => { if (es) es.close(); };
  }, [paused]);

  const scrollRef = React.useRef(null);
  React.useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = 0; // newest is at the top — keep it pinned there
  }, [lines]);

  return (
    <Card inset pad={0} style={{ display: 'flex', flexDirection: 'column', height: '100%', minHeight: 0, overflow: 'hidden' }}>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '12px 14px', borderBottom: '1px solid var(--border-faint)' }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <StatusDot tone="accent" pulse size={7} />
          <span style={{ fontFamily: 'var(--font-display)', fontSize: 14, fontWeight: 600, color: 'var(--text-hi)' }}>Live route log</span>
        </div>
        <button onClick={() => setPaused(p => !p)} style={{ background: 'transparent', border: '1px solid var(--border-hairline)', color: 'var(--text-lo)', fontFamily: 'var(--font-mono)', fontSize: 11, padding: '3px 8px', borderRadius: 'var(--r-xs)', cursor: 'pointer' }}>{paused ? '▶ resume' : '❚❚ pause'}</button>
      </div>
      <div ref={scrollRef} style={{ flex: 1, overflowY: 'auto', padding: 6, display: 'flex', flexDirection: 'column', gap: 1 }}>
        {lines.length
          ? lines.map((l) => <LogRow key={l._id} {...l} />)
          : <div style={{ margin: 'auto', textAlign: 'center', fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-faint)', padding: 24 }}>waiting for route activity…</div>}
      </div>
    </Card>
  );
}

// A provider with no account yet: dimmed placeholder that connects it right here
// instead of sending the user hunting through the Accounts tab.
function GhostProviderCard({ c, onConnect }) {
  return (
    <Card pad={14} style={{ display: 'flex', flexDirection: 'column', gap: 10, opacity: 0.65, borderStyle: 'dashed' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}>
        <span style={{ fontFamily: 'var(--font-mono)', fontSize: 14, fontWeight: 600, color: 'var(--text-mid)' }}>{c.id}</span>
        {c.free && <Badge tone="neutral" variant="outline">{c.free}</Badge>}
      </div>
      <span style={{ fontFamily: 'var(--font-ui)', fontSize: 12, color: 'var(--text-lo)', flex: 1 }}>{c.blurb}</span>
      <Button variant="secondary" onClick={onConnect} style={{ alignSelf: 'flex-start' }}>
        {c.web ? 'Connect — sign in via browser' : '+ Connect'}
      </Button>
    </Card>
  );
}

const OVERVIEW_GROUPS = [
  ['search', 'Search'],
  ['read', 'Read / scrape'],
  ['browser', 'Browser'],
  ['web', 'Web sessions'],
];

function OverviewTab() {
  const [modalProv, setModalProv] = React.useState(null);
  const live = Object.fromEntries((window.FX.groups || []).map((g) => [g.id, g.providers]));
  const configured = new Set((window.FX.accounts || []).map((a) => a.provider));
  const catalog = window.FX.catalog || [];
  const groups = OVERVIEW_GROUPS.map(([id, label]) => ({
    id,
    label,
    providers: live[id] || [],
    ghosts: catalog.filter((c) => c.group === id && !configured.has(c.id)),
  })).filter((g) => g.providers.length || g.ghosts.length);
  return (
    <div style={{ display: 'grid', gridTemplateColumns: 'minmax(0, 1fr) 380px', gap: 20, alignItems: 'start', height: '100%' }}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 24 }}>
        <window.GettingStarted />
        <BurnRadar />
        {groups.map((g) => (
          <section key={g.id}>
            <GroupHeader label={g.label} count={`${g.providers.length}/${g.providers.length + g.ghosts.length} connected`} />
            <div style={{ columnWidth: 290, columnGap: 14 }}>
              {g.providers.map((p) => (
                <div key={p.name} style={{ breakInside: 'avoid', marginBottom: 14 }}>
                  <FxProviderCard {...p} />
                </div>
              ))}
              {g.ghosts.map((c) => (
                <div key={c.id} style={{ breakInside: 'avoid', marginBottom: 14 }}>
                  <GhostProviderCard c={c} onConnect={() => setModalProv(c.id)} />
                </div>
              ))}
            </div>
          </section>
        ))}
        <RoutingPriority />
        <CapabilityMatrix />
      </div>
      <div style={{ position: 'sticky', top: 84, height: 'calc(100vh - 104px)' }}>
        <LiveLog />
      </div>
      {modalProv && (
        <window.AddAccountModal initialProvider={modalProv}
          onClose={() => { setModalProv(null); if (window.fxRefresh) window.fxRefresh(); }} />
      )}
    </div>
  );
}

window.OverviewTab = OverviewTab;
