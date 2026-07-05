/* Activity: filterable full route log + usage sparkline charts + health list. */
const { Card, Badge, Button, StatusDot, RouteLogLine } = window.FetchiraDesignSystem_6526df;

function Sparkline({ data, color }) {
  const w = 132, h = 38, max = Math.max(...data, 1);
  const step = w / (data.length - 1);
  const pts = data.map((v, i) => [i * step, h - (v / max) * (h - 6) - 2]);
  const line = pts.map((p, i) => `${i === 0 ? 'M' : 'L'}${p[0].toFixed(1)} ${p[1].toFixed(1)}`).join(' ');
  const area = `${line} L${w} ${h} L0 ${h} Z`;
  return (
    <svg width={w} height={h} style={{ display: 'block' }}>
      <defs>
        <linearGradient id={`g-${color.replace(/[^a-z]/gi, '')}`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor={color} stopOpacity="0.30" />
          <stop offset="100%" stopColor={color} stopOpacity="0" />
        </linearGradient>
      </defs>
      <path d={area} fill={`url(#g-${color.replace(/[^a-z]/gi, '')})`} />
      <path d={line} fill="none" stroke={color} strokeWidth="1.5" strokeLinejoin="round" />
    </svg>
  );
}

function FilterChip({ label, active, onClick }) {
  return (
    <button onClick={onClick} style={{
      fontFamily: 'var(--font-mono)', fontSize: 12, padding: '4px 10px', borderRadius: 'var(--r-pill)',
      cursor: 'pointer', border: `1px solid ${active ? 'var(--border-accent)' : 'var(--border-hairline)'}`,
      background: active ? 'var(--lime-dim)' : 'transparent', color: active ? 'var(--lime-500)' : 'var(--text-lo)',
    }}>{label}</button>
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

function HealthRow({ h }) {
  const tone = h.state === 'exhausted' ? 'out' : h.state === 'needs-login' ? 'off' : 'ok';
  return (
    <div style={{ display: 'flex', alignItems: 'flex-start', gap: 12, padding: '11px 0', borderTop: '1px solid var(--border-faint)' }}>
      <StatusDot tone={tone} size={8} style={{ marginTop: 3 }} />
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}>
          <span style={{ fontFamily: 'var(--font-mono)', fontSize: 13, color: 'var(--text-hi)' }}>{h.provider}</span>
          <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>{h.lastSuccess}</span>
        </div>
        {h.lastError && <div style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: tone === 'out' ? 'var(--red-500)' : 'var(--text-lo)', marginTop: 3, lineHeight: 1.4 }}>{h.lastError}</div>}
      </div>
    </div>
  );
}

function ActivityTab() {
  const [filter, setFilter] = React.useState('all');
  const caps = ['all', 'search', 'read', 'deep_research', 'browser', 'failures'];
  const all = window.FX.log;
  const lines = all.filter((l) => filter === 'all' ? true : filter === 'failures' ? !!l.failover : l.capability === filter);
  const usage = (window.FX.usage && window.FX.usage.length) ? window.FX.usage : [];

  return (
    <div style={{ display: 'grid', gridTemplateColumns: 'minmax(0,1fr) 360px', gap: 20, alignItems: 'start' }}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 20 }}>
        <Card pad={0} style={{ overflow: 'hidden' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '12px 14px', borderBottom: '1px solid var(--border-faint)', flexWrap: 'wrap' }}>
            <span style={{ fontFamily: 'var(--font-display)', fontSize: 14, fontWeight: 600, color: 'var(--text-hi)', marginRight: 4 }}>Route log</span>
            {caps.map((c) => <FilterChip key={c} label={c.replace('_', ' ')} active={filter === c} onClick={() => setFilter(c)} />)}
          </div>
          <div style={{ padding: 8, display: 'flex', flexDirection: 'column', gap: 1 }}>
            {lines.length ? lines.map((l, i) => <LogRow key={i} {...l} />) : <div style={{ padding: 24, textAlign: 'center', fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-faint)' }}>No matching calls</div>}
          </div>
        </Card>

        <div>
          <div style={{ fontFamily: 'var(--font-mono)', fontSize: 11, fontWeight: 600, letterSpacing: '0.12em', textTransform: 'uppercase', color: 'var(--text-lo)', marginBottom: 12 }}>Usage · calls per day</div>
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(220px,1fr))', gap: 14 }}>
            {usage.length ? usage.map((u) => (
              <Card key={u.provider} pad={14}>
                <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'baseline', marginBottom: 8 }}>
                  <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-hi)' }}>{u.provider}</span>
                  <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>{u.series.reduce((a, b) => a + b, 0)} total</span>
                </div>
                <Sparkline data={u.series} color={u.color} />
              </Card>
            )) : <div style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-faint)', padding: '8px 2px' }}>No calls recorded yet — usage fills in as the router serves requests.</div>}
          </div>
        </div>
      </div>

      <Card pad={16} style={{ position: 'sticky', top: 84 }}>
        <div style={{ fontFamily: 'var(--font-display)', fontSize: 14, fontWeight: 600, color: 'var(--text-hi)', marginBottom: 4 }}>Provider health</div>
        <div style={{ fontFamily: 'var(--font-ui)', fontSize: 12, color: 'var(--text-lo)', marginBottom: 8 }}>Last success · last failover error</div>
        {window.FX.health.map((h) => <HealthRow key={h.provider} h={h} />)}
      </Card>
    </div>
  );
}

window.ActivityTab = ActivityTab;
