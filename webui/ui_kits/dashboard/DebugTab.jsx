/* Debug: live firehose of every provider request/response/error, full bodies on expand. */
const { Card, StatusDot } = window.FetchiraDesignSystem_6526df;

const CAP_COLOR = {
  search: 'var(--lime-500)',
  read: 'var(--cyan-500)',
  deep_research: '#C792EA',
  browser: 'var(--amber-500)',
};

function pretty(s) {
  try { return JSON.stringify(JSON.parse(s), null, 2); } catch (e) { return s; }
}

function DebugDetail({ row, full }) {
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
        <pre style={{ ...pre, maxHeight: 320, overflowY: 'auto', borderLeft: row.ok ? 'none' : '2px solid var(--red-500)' }}>{body != null ? body : '—'}</pre>
      </div>
    </div>
  );
}

function DebugRow({ row, open, full, onToggle }) {
  const capColor = CAP_COLOR[row.capability] || 'var(--text-mid)';
  return (
    <div style={{ borderRadius: 'var(--r-xs)', background: row.fresh ? 'var(--surface-2)' : 'transparent', animation: row.fresh ? 'fx-log-in var(--dur-mid) var(--ease-out)' : 'none' }}>
      <div onClick={onToggle} style={{ cursor: 'pointer', padding: '5px 10px' }}>
        <div style={{ display: 'grid', gridTemplateColumns: 'auto 92px 1fr auto', alignItems: 'center', gap: 10, fontFamily: 'var(--font-mono)', fontSize: 12, lineHeight: 1.2 }}>
          <span style={{ color: 'var(--text-faint)' }}>{row.time}</span>
          <span style={{ color: capColor, fontWeight: 500 }}>{row.capability}</span>
          <span style={{ color: 'var(--text-hi)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{row.provider}{row.account != null ? `-${row.account}` : ''}</span>
          <span style={{ display: 'inline-flex', alignItems: 'center', gap: 8, justifySelf: 'end' }}>
            <span style={{ color: row.ok ? 'var(--text-faint)' : 'var(--red-500)' }}>{row.status}</span>
            <span style={{ color: row.latencyMs > 800 ? 'var(--amber-500)' : 'var(--text-lo)', minWidth: 48, textAlign: 'right' }}>{row.latencyMs}ms</span>
          </span>
        </div>
        {row.preview && <div style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-lo)', marginTop: 3, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{row.preview}</div>}
      </div>
      {open && <DebugDetail row={row} full={full} />}
    </div>
  );
}

function DebugTab() {
  const [rows, setRows] = React.useState([]);
  const [paused, setPaused] = React.useState(false);
  const [openId, setOpenId] = React.useState(null);
  const [details, setDetails] = React.useState({});
  const lastId = React.useRef(0);
  const scrollRef = React.useRef(null);

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
      if (!alive || !d) return;
      lastId.current = d.maxId;
      setRows(d.rows.map((r) => ({ ...r, fresh: false })));
    })();
    return () => { alive = false; };
  }, []);

  React.useEffect(() => {
    if (paused) return;
    const id = setInterval(async () => {
      const d = await fetchRows(lastId.current);
      if (!d || !d.rows.length) return;
      lastId.current = d.maxId;
      setRows((prev) => [...prev, ...d.rows.map((r) => ({ ...r, fresh: true }))].slice(-300));
    }, 1500);
    return () => clearInterval(id);
  }, [paused]);

  React.useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [rows]);

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

  return (
    <div style={{ height: 'calc(100vh - 104px)' }}>
      <Card inset pad={0} style={{ display: 'flex', flexDirection: 'column', height: '100%', minHeight: 0, overflow: 'hidden' }}>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '12px 14px', borderBottom: '1px solid var(--border-faint)' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
            <StatusDot tone={paused ? 'off' : 'accent'} pulse={!paused} size={7} />
            <span style={{ fontFamily: 'var(--font-display)', fontSize: 14, fontWeight: 600, color: 'var(--text-hi)' }}>Debug log</span>
            <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>full request / response capture</span>
          </div>
          <button onClick={() => setPaused(p => !p)} style={{ background: 'transparent', border: '1px solid var(--border-hairline)', color: 'var(--text-lo)', fontFamily: 'var(--font-mono)', fontSize: 11, padding: '3px 8px', borderRadius: 'var(--r-xs)', cursor: 'pointer' }}>{paused ? '▶ resume' : '❚❚ pause'}</button>
        </div>
        <div ref={scrollRef} style={{ flex: 1, overflowY: 'auto', padding: 6, display: 'flex', flexDirection: 'column', gap: 1 }}>
          {rows.length
            ? rows.map((row) => <DebugRow key={row.id} row={row} open={openId === row.id} full={details[row.id]} onToggle={() => toggle(row)} />)
            : <div style={{ margin: 'auto', textAlign: 'center', fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-faint)', padding: 24 }}>waiting for debug activity…</div>}
        </div>
      </Card>
    </div>
  );
}

window.DebugTab = DebugTab;
