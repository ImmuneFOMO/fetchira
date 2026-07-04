/* Overview: provider grid grouped by capability + pinned live route log. */
const { RouteLogLine, Card, StatusDot, Badge, QuotaMeter } = window.FetchiraDesignSystem_6526df;

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

// One limit = its own cube bar (each mode/model/feature has its own quota + reset cadence).
// Fuel-gauge fill: the bar shows what's LEFT (full when fresh), so we feed the meter `remaining`
// as its fill and force the colour from the real remaining (green → amber → red as it drains).
function LimitRow({ label, used, quota, window, resetAt, locked, off }) {
  const q = quota || 0;
  const remaining = Math.max(0, q - (used || 0));
  const remFrac = q > 0 ? remaining / q : 0;
  const st = off || locked ? 'off' : remaining <= 0 ? 'out' : remFrac < 0.15 ? 'low' : 'ok';
  const meta = locked ? 'locked' : [window, fmtReset(resetAt)].filter(Boolean).join(' · ');
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
      <div style={{ display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', gap: 8 }}>
        <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: locked ? 'var(--text-faint)' : 'var(--text-mid)' }}>{label}</span>
        <span style={{ display: 'flex', gap: 8, alignItems: 'baseline', fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>
          <span>{locked ? '0/0' : <><b style={{ color: 'var(--text-hi)' }}>{remaining.toLocaleString()}</b> / {q.toLocaleString()}</>}</span>
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
        {active.map((l) => <LimitRow key={l.label} {...l} off={needsLogin} />)}
        {showSpent && spent.map((l) => <LimitRow key={l.label} {...l} off={needsLogin} />)}
        {features.map((f) => <FeatureRow key={f.label} {...f} />)}
      </div>

      {spent.length > 0 && (
        <button onClick={() => setShowSpent((s) => !s)}
          style={{ alignSelf: 'flex-start', background: 'transparent', border: 'none', cursor: 'pointer', padding: 0, fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>
          {showSpent ? '− hide locked / used up' : `+ ${spent.length} more · locked / used up`}
        </button>
      )}

      {catalog.length > 0 && (
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
        setLines((prev) => [...prev.slice(-40), ...batch.map((l) => ({ ...l, _id: idRef.current++, fresh: true }))]);
      };
    } catch (err) { /* no SSE when opened as a static file */ }
    return () => { if (es) es.close(); };
  }, [paused]);

  const scrollRef = React.useRef(null);
  React.useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
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
          ? lines.map((l) => <RouteLogLine key={l._id} {...l} />)
          : <div style={{ margin: 'auto', textAlign: 'center', fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-faint)', padding: 24 }}>waiting for route activity…</div>}
      </div>
    </Card>
  );
}

function OverviewTab() {
  const groups = window.FX.groups;
  return (
    <div style={{ display: 'grid', gridTemplateColumns: 'minmax(0, 1fr) 380px', gap: 20, alignItems: 'start', height: '100%' }}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 24 }}>
        {groups.map((g) => (
          <section key={g.id}>
            <GroupHeader label={g.label} count={`${g.providers.length} ${g.providers.length === 1 ? 'provider' : 'providers'}`} />
            <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(290px, 1fr))', gap: 14 }}>
              {g.providers.map((p) => <FxProviderCard {...p} key={p.name} />)}
            </div>
          </section>
        ))}
      </div>
      <div style={{ position: 'sticky', top: 84, height: 'calc(100vh - 104px)' }}>
        <LiveLog />
      </div>
    </div>
  );
}

window.OverviewTab = OverviewTab;
