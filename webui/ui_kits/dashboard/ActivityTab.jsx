/* Activity: one stream for everything the router did — every attempt (success or failure) with
   a response preview, expandable to the full request/response/error and raw HTTP. Filterable by
   capability / errors, live-tailing, with usage sparklines and provider health alongside.
   (Absorbed the old Debug tab — same data, one place.) */
const { Card, Badge, StatusDot, RouteLogLine } = window.FetchiraDesignSystem_6526df;

const CAP_COLOR = {
  search: 'var(--lime-500)',
  read: 'var(--cyan-500)',
  deep_research: '#C792EA',
  browser: 'var(--amber-500)',
  create_image: 'var(--amber-500)',
};

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

function pretty(s) {
  try { return JSON.stringify(JSON.parse(s), null, 2); } catch (e) { return s; }
}

// Headers/body may arrive as a JSON object (header map) or a plain string; render both readably.
function fmtTrace(v) {
  if (v == null) return '—';
  if (typeof v === 'string') return v || '—';
  return JSON.stringify(v, null, 2);
}

function RawHttp({ trace, pre, cap }) {
  const [open, setOpen] = React.useState(false);
  if (!Array.isArray(trace) || !trace.length) return null;
  const sub = { ...cap, marginTop: 6 };
  return (
    <div>
      <div onClick={() => setOpen(o => !o)} style={{ ...cap, marginBottom: open ? 6 : 0, cursor: 'pointer' }}>{open ? '▾' : '▸'} raw HTTP</div>
      {open && trace.map((rt, i) => (
        <div key={i} style={{ display: 'flex', flexDirection: 'column', gap: 4, marginBottom: 10 }}>
          <div style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-hi)', wordBreak: 'break-all' }}>{rt.method} {rt.url} → {rt.status}</div>
          <div style={sub}>req headers</div>
          <pre style={pre}>{fmtTrace(rt.reqHeaders)}</pre>
          <div style={sub}>req body</div>
          <pre style={pre}>{fmtTrace(rt.reqBody)}</pre>
          <div style={sub}>resp headers</div>
          <pre style={pre}>{fmtTrace(rt.respHeaders)}</pre>
          <div style={sub}>resp body</div>
          <pre style={{ ...pre, maxHeight: 320, overflowY: 'auto' }}>{fmtTrace(rt.respBody)}</pre>
        </div>
      ))}
    </div>
  );
}

function AttemptDetail({ ok, full }) {
  if (!full) return <div style={{ padding: '6px 10px', fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>loading…</div>;
  const body = full.response != null ? full.response : full.error;
  const label = full.response != null ? 'response' : 'error';
  const pre = { fontFamily: 'var(--font-mono)', fontSize: 12, whiteSpace: 'pre-wrap', wordBreak: 'break-word', margin: 0, padding: 10, background: 'var(--surface-well)', borderRadius: 'var(--r-xs)', color: 'var(--text-mid)' };
  const cap = { fontFamily: 'var(--font-mono)', fontSize: 10, letterSpacing: '0.1em', textTransform: 'uppercase', color: 'var(--text-faint)', marginBottom: 4 };
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8, padding: '2px 10px 10px' }}>
      <div>
        <div style={cap}>request</div>
        <pre style={pre}>{pretty(full.request)}</pre>
      </div>
      <div>
        <div style={cap}>{label}</div>
        <pre style={{ ...pre, maxHeight: 320, overflowY: 'auto', borderLeft: ok ? 'none' : '2px solid var(--red-500)' }}>{body != null ? body : '—'}</pre>
      </div>
      <RawHttp trace={full.httpTrace} pre={pre} cap={cap} />
    </div>
  );
}

function AttemptRow({ row, open, full, onToggle }) {
  const capColor = CAP_COLOR[row.capability] || 'var(--text-mid)';
  return (
    <div style={{ borderRadius: 'var(--r-xs)', background: open ? 'var(--surface-2)' : row.fresh ? 'var(--surface-2)' : 'transparent', animation: row.fresh ? 'fx-log-in var(--dur-mid) var(--ease-out)' : 'none' }}>
      <div onClick={onToggle} style={{ cursor: 'pointer', padding: '5px 10px' }}>
        <div style={{ display: 'grid', gridTemplateColumns: '10px auto 100px 1fr auto', alignItems: 'center', gap: 10, fontFamily: 'var(--font-mono)', fontSize: 12, lineHeight: 1.2 }}>
          <span style={{ fontSize: 10, color: 'var(--text-faint)' }}>{open ? '▾' : '▸'}</span>
          <span style={{ color: 'var(--text-faint)' }}>{row.time}</span>
          <span style={{ color: capColor, fontWeight: 500 }}>{row.capability === 'create_image' ? 'image' : row.capability.replace('_', ' ')}</span>
          <span style={{ color: 'var(--text-hi)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{row.provider}{row.account != null ? `-${row.account}` : ''}</span>
          <span style={{ display: 'inline-flex', alignItems: 'center', gap: 8, justifySelf: 'end' }}>
            <span style={{ color: row.ok ? 'var(--text-faint)' : 'var(--red-500)' }}>{row.status}</span>
            <span style={{ color: row.latencyMs > 800 ? 'var(--amber-500)' : 'var(--text-lo)', minWidth: 48, textAlign: 'right' }}>{row.latencyMs}ms</span>
          </span>
        </div>
        {row.preview && <div style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: row.ok ? 'var(--text-lo)' : 'var(--red-500)', marginTop: 3, paddingLeft: 20, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{row.preview}</div>}
      </div>
      {open && <AttemptDetail ok={row.ok} full={full} />}
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
  const [rows, setRows] = React.useState([]);
  const [loaded, setLoaded] = React.useState(false);
  const [paused, setPaused] = React.useState(false);
  const [openId, setOpenId] = React.useState(null);
  const [details, setDetails] = React.useState({});
  const lastId = React.useRef(0);

  const fetchRows = async (after) => {
    try {
      const r = await fetch(`/api/debug?after=${after}&limit=200`, { headers: { 'x-fetchira-token': window.FX_TOKEN } });
      if (r.ok) return await r.json();
    } catch (e) { /* offline / opened as a static file */ }
  };

  React.useEffect(() => {
    let alive = true;
    (async () => {
      const d = await fetchRows(0);
      if (!alive) return;
      if (d) {
        lastId.current = d.maxId;
        setRows(d.rows.slice().reverse().map((r) => ({ ...r, fresh: false }))); // newest first
      }
      setLoaded(true);
    })();
    return () => { alive = false; };
  }, []);

  React.useEffect(() => {
    if (paused) return;
    const id = setInterval(async () => {
      const d = await fetchRows(lastId.current);
      if (!d || !d.rows.length) return;
      lastId.current = d.maxId;
      setRows((prev) => [...d.rows.slice().reverse().map((r) => ({ ...r, fresh: true })), ...prev].slice(0, 300));
    }, 1500);
    return () => clearInterval(id);
  }, [paused]);

  const toggle = async (row) => {
    const closing = openId === row.id;
    setOpenId(closing ? null : row.id);
    if (closing || details[row.id]) return;
    try {
      const r = await fetch(`/api/debug/${row.id}`, { headers: { 'x-fetchira-token': window.FX_TOKEN } });
      if (!r.ok) return;
      const full = await r.json();
      setDetails((prev) => ({ ...prev, [row.id]: full }));
    } catch (e) { /* offline */ }
  };

  const caps = ['all', 'search', 'read', 'deep_research', 'browser', 'image', 'errors'];
  const shown = rows.filter((r) =>
    filter === 'all' ? true
      : filter === 'errors' ? !r.ok
      : filter === 'image' ? r.capability === 'create_image'
      : r.capability === filter);
  // Full capture disabled (debug_log.enabled = false) or already expired: fall back to the
  // route-log snapshot so the tab still tells the routing story.
  const fallback = loaded && !rows.length && (window.FX.log || []).length > 0;
  const usage = (window.FX.usage && window.FX.usage.length) ? window.FX.usage : [];

  return (
    <div style={{ display: 'grid', gridTemplateColumns: 'minmax(0,1fr) 360px', gap: 20, alignItems: 'start' }}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 20 }}>
        <Card pad={0} style={{ overflow: 'hidden' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '12px 14px', borderBottom: '1px solid var(--border-faint)', flexWrap: 'wrap' }}>
            <StatusDot tone={paused ? 'off' : 'accent'} pulse={!paused} size={7} />
            <span style={{ fontFamily: 'var(--font-display)', fontSize: 14, fontWeight: 600, color: 'var(--text-hi)', marginRight: 4 }}>Activity</span>
            {caps.map((c) => <FilterChip key={c} label={c.replace('_', ' ')} active={filter === c} onClick={() => setFilter(c)} />)}
            <span style={{ flex: 1 }} />
            <button onClick={() => setPaused(p => !p)} style={{ background: 'transparent', border: '1px solid var(--border-hairline)', color: 'var(--text-lo)', fontFamily: 'var(--font-mono)', fontSize: 11, padding: '3px 8px', borderRadius: 'var(--r-xs)', cursor: 'pointer' }}>{paused ? '▶ resume' : '❚❚ pause'}</button>
          </div>
          <div style={{ padding: 8, display: 'flex', flexDirection: 'column', gap: 1, maxHeight: 'calc(100vh - 230px)', overflowY: 'auto' }}>
            {fallback ? (
              <React.Fragment>
                <div style={{ padding: '4px 10px 10px', fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>
                  full capture is off or expired (debug_log in fetchira.toml) — showing the routed-call log
                </div>
                {window.FX.log.map((l, i) => <RouteLogLine key={i} {...l} />)}
              </React.Fragment>
            ) : shown.length
              ? shown.map((row) => <AttemptRow key={row.id} row={row} open={openId === row.id} full={details[row.id]} onToggle={() => toggle(row)} />)
              : <div style={{ padding: 24, textAlign: 'center', fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-faint)' }}>{loaded ? 'No matching calls yet' : 'loading…'}</div>}
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
