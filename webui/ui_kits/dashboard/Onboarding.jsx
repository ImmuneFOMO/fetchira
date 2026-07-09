/* First-run welcome: connect one provider fast (validate on paste, show live quota),
   try a real routed search, then hand over to the dashboard. Everything is skippable. */
const { Card, Button, Input, Badge, StatusDot } = window.FetchiraDesignSystem_6526df;

// All key providers, best-first: tavily covers search+read+deep-research on a renewable
// monthly tier, serper has the biggest instant grant, firecrawl leads the read chain.
const OB_ORDER = ['tavily', 'serper', 'firecrawl', 'exa', 'parallel', 'steel'];
const OB_KEY_HINTS = { tavily: 'tvly-…', serper: 'paste your serper.dev key', exa: 'paste your exa key', firecrawl: 'fc-…', parallel: 'paste your parallel key', steel: 'ste-…' };

function obCatalog() {
  return (window.FX && window.FX.catalog) || [];
}

function SectionLabel({ children }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 10, margin: '26px 0 12px' }}>
      <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, fontWeight: 600, letterSpacing: '0.12em', textTransform: 'uppercase', color: 'var(--text-lo)' }}>{children}</span>
      <span style={{ flex: 1, height: 1, background: 'var(--border-faint)' }} />
    </div>
  );
}

// One key-paste provider: signup deep link, paste field, validate with a real call on connect.
function KeyProviderCard({ p, onOpenModal }) {
  const [key, setKey] = React.useState('');
  const [phase, setPhase] = React.useState('form'); // form | adding | testing | done | bad
  const [label, setLabel] = React.useState('');
  const [ms, setMs] = React.useState(null);
  const [err, setErr] = React.useState(null);

  const connect = async () => {
    if (key.trim().length < 8 || phase === 'adding' || phase === 'testing') return;
    setErr(null); setPhase('adding');
    let added;
    try {
      added = await window.apiPost('/api/account/add', { provider: p.id, label: '', key: key.trim() });
    } catch (e) { setErr(String(e.message || e)); setPhase('form'); return; }
    setLabel(added.label);
    if (window.fxRefresh) window.fxRefresh();
    setPhase('testing');
    try {
      const t = await window.apiPost('/api/account/test', { label: added.label });
      if (t.ok) { setMs(t.latencyMs); setPhase('done'); }
      else { setErr(t.error || 'test call failed'); setPhase('bad'); }
    } catch (e) { setErr(String(e.message || e)); setPhase('bad'); }
    if (window.fxRefresh) window.fxRefresh();
  };

  const removeRetry = async () => {
    try { await window.apiPost('/api/account/remove', { label }); } catch (e) {}
    setPhase('form'); setErr(null); setKey('');
    if (window.fxRefresh) window.fxRefresh();
  };

  const acct = (window.FX.accounts || []).find((a) => a.label === label);

  if (phase === 'done') {
    return (
      <Card accent="ok" pad={16} style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <StatusDot tone="ok" size={7} />
          <span style={{ fontFamily: 'var(--font-mono)', fontSize: 14, fontWeight: 600, color: 'var(--text-hi)' }}>{p.id}</span>
          <Badge tone="ok" variant="soft">connected ✓</Badge>
        </div>
        <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-mid)' }}>
          <span style={{ color: 'var(--lime-500)' }}>{label}</span> answered a live call in {ms}ms
          {acct && <> · <b style={{ color: 'var(--text-hi)' }}>{Math.max(0, (acct.quota || 0) - (acct.used || 0)).toLocaleString()}</b> requests in the tank</>}
        </span>
      </Card>
    );
  }

  return (
    <Card pad={16} style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
      <div style={{ display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', gap: 8, flexWrap: 'wrap' }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, minWidth: 0 }}>
          <span style={{ fontFamily: 'var(--font-mono)', fontSize: 14, fontWeight: 600, color: 'var(--text-hi)' }}>{p.id}</span>
          {p.free && <Badge tone="neutral" variant="outline">{p.free}</Badge>}
        </div>
        {p.signup && (
          <a href={p.signup} target="_blank" rel="noreferrer" style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--lime-500)', textDecoration: 'none', whiteSpace: 'nowrap' }}>
            get a free key ↗
          </a>
        )}
      </div>
      <span style={{ fontFamily: 'var(--font-ui)', fontSize: 12, color: 'var(--text-lo)', flex: 1 }}>{p.blurb}</span>
      <div style={{ display: 'flex', gap: 8 }}>
        <div style={{ flex: 1 }}>
          <Input placeholder={OB_KEY_HINTS[p.id] || 'paste API key'} value={key} mono type="password"
            onChange={(e) => setKey(e.target.value)}
            onKeyDown={(e) => { if (e.key === 'Enter') connect(); }} />
        </div>
        <Button variant="primary" onClick={connect} disabled={key.trim().length < 8 || phase === 'adding' || phase === 'testing'}>
          {phase === 'adding' ? 'Adding…' : phase === 'testing' ? 'Testing…' : 'Connect'}
        </Button>
      </div>
      {phase === 'bad' && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          <div style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--red-500)', background: 'var(--red-dim)', border: '1px solid rgba(242,85,90,0.3)', borderRadius: 'var(--r-sm)', padding: '8px 10px' }}>
            key saved as {label}, but the test call failed: {err}
          </div>
          <div style={{ display: 'flex', gap: 8 }}>
            <Button variant="ghost" onClick={removeRetry}>Remove & retry</Button>
            <Button variant="ghost" onClick={() => onOpenModal(p.id)}>Open full form</Button>
          </div>
        </div>
      )}
      {err && phase === 'form' && (
        <div style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--red-500)', background: 'var(--red-dim)', border: '1px solid rgba(242,85,90,0.3)', borderRadius: 'var(--r-sm)', padding: '8px 10px' }}>{err}</div>
      )}
    </Card>
  );
}

// Browser-session providers (gemini/grok/chatgpt): sets expectations, then the shared modal
// runs the guided login (browser picker, session-paste fallback).
function WebProviderCard({ p, onOpenModal }) {
  const connected = (window.FX.accounts || []).some((a) => a.provider === p.id && a.loggedIn);
  return (
    <Card accent={connected ? 'ok' : undefined} pad={16} style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
        {connected && <StatusDot tone="ok" size={7} />}
        <span style={{ fontFamily: 'var(--font-mono)', fontSize: 14, fontWeight: 600, color: 'var(--text-hi)' }}>{p.id}</span>
        <Badge tone="cyan" variant="outline">browser login</Badge>
        {connected && <Badge tone="ok" variant="soft">connected ✓</Badge>}
      </div>
      <span style={{ fontFamily: 'var(--font-ui)', fontSize: 12, color: 'var(--text-lo)', flex: 1 }}>{p.blurb}</span>
      <Button variant="secondary" onClick={() => onOpenModal(p.id)} style={{ alignSelf: 'flex-start' }}>
        {connected ? 'Add another account' : 'Connect — sign in via browser'}
      </Button>
      <span style={{ fontFamily: 'var(--font-ui)', fontSize: 11, color: 'var(--text-faint)' }}>
        Opens a browser window for you to sign in (~30s). Cookies stay on this machine — no password is stored.
      </span>
    </Card>
  );
}

// The aha: one real routed search, showing which provider the router picked.
function TrySearch() {
  const [q, setQ] = React.useState('');
  const [busy, setBusy] = React.useState(false);
  const [res, setRes] = React.useState(null);

  const run = async () => {
    if (!q.trim() || busy) return;
    setBusy(true); setRes(null);
    try { setRes(await window.apiPost('/api/try', { q: q.trim() })); }
    catch (e) { setRes({ ok: false, error: String(e.message || e) }); }
    setBusy(false);
    if (window.fxRefresh) window.fxRefresh();
  };

  return (
    <Card raised pad={16} style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
        <StatusDot tone="accent" pulse size={7} />
        <span style={{ fontFamily: 'var(--font-display)', fontSize: 15, fontWeight: 600, color: 'var(--text-hi)' }}>Try it — run a real search</span>
      </div>
      <div style={{ display: 'flex', gap: 8 }}>
        <div style={{ flex: 1 }}>
          <Input placeholder="e.g. latest rust 1.80 release notes" value={q} mono
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => { if (e.key === 'Enter') run(); }} />
        </div>
        <Button variant="primary" onClick={run} disabled={busy || !q.trim()}>{busy ? 'Routing…' : 'Search'}</Button>
      </div>
      {res && res.ok && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-mid)' }}>
            <Badge tone="ok" variant="soft">200</Badge>
            routed via <b style={{ color: 'var(--lime-500)' }}>{res.provider || 'router'}</b>
            {res.label && <span style={{ color: 'var(--text-faint)' }}>({res.label})</span>}
            · {res.latencyMs}ms
          </div>
          <pre style={{ margin: 0, maxHeight: 220, overflowY: 'auto', whiteSpace: 'pre-wrap', wordBreak: 'break-word', fontFamily: 'var(--font-mono)', fontSize: 11.5, lineHeight: 1.5, color: 'var(--text-mid)', background: 'var(--surface-sunken, rgba(255,255,255,0.03))', border: '1px solid var(--border-hairline)', borderRadius: 'var(--r-sm)', padding: '10px 12px' }}>{res.text}</pre>
        </div>
      )}
      {res && !res.ok && (
        <div style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--red-500)', background: 'var(--red-dim)', border: '1px solid rgba(242,85,90,0.3)', borderRadius: 'var(--r-sm)', padding: '8px 10px' }}>{res.error}</div>
      )}
    </Card>
  );
}

function Onboarding({ onDone }) {
  const [modalProv, setModalProv] = React.useState(null);
  const catalog = obCatalog();
  const accounts = window.FX.accounts || [];
  const connected = accounts.length;

  const keys = OB_ORDER.map((id) => catalog.find((c) => c.id === id))
    .filter(Boolean)
    .concat(catalog.filter((c) => !c.web && !OB_ORDER.includes(c.id)));
  const webs = catalog.filter((c) => c.web);

  return (
    <div style={{ maxWidth: 860, margin: '5vh auto 60px', padding: '0 20px' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 14 }}>
        <img src="../../assets/logo-mark.svg" alt="" style={{ width: 34, height: 34 }} />
        <span style={{ fontFamily: 'var(--font-display)', fontSize: 24, fontWeight: 600, letterSpacing: '-0.03em', color: 'var(--text-hi)' }}>fetchira</span>
      </div>
      <div style={{ fontFamily: 'var(--font-display)', fontSize: 20, fontWeight: 600, color: 'var(--text-hi)', letterSpacing: '-0.02em', marginBottom: 6 }}>
        One search router for all your agents
      </div>
      <div style={{ fontFamily: 'var(--font-ui)', fontSize: 14, color: 'var(--text-mid)', lineHeight: 1.55, maxWidth: 620 }}>
        fetchira gives your AI tools web search, scraping and deep research — routed across
        free-tier providers with quota-aware failover. Connect <b style={{ color: 'var(--text-hi)' }}>one</b> provider
        to start; everything else can wait.
      </div>

      <SectionLabel>free api keys — no credit card, ~60 seconds</SectionLabel>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(320px, 1fr))', gap: 14 }}>
        {keys.map((p) => <KeyProviderCard key={p.id} p={p} onOpenModal={setModalProv} />)}
      </div>

      <SectionLabel>your ai subscriptions — unlock deep research + images</SectionLabel>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(250px, 1fr))', gap: 14 }}>
        {webs.map((p) => <WebProviderCard key={p.id} p={p} onOpenModal={setModalProv} />)}
      </div>

      {connected > 0 && (
        <div style={{ marginTop: 26, display: 'flex', flexDirection: 'column', gap: 14 }}>
          <TrySearch />
          <Card raised pad={16} style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
              <StatusDot tone="accent" size={7} />
              <span style={{ fontFamily: 'var(--font-display)', fontSize: 15, fontWeight: 600, color: 'var(--text-hi)' }}>Add fetchira to your coding agents</span>
            </div>
            <span style={{ fontFamily: 'var(--font-ui)', fontSize: 12, color: 'var(--text-lo)' }}>
              Register the MCP server so your agents route their web research through the providers you just connected.
            </span>
            <window.InstallTargets />
          </Card>
        </div>
      )}

      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginTop: 30, paddingTop: 16, borderTop: '1px solid var(--border-faint)' }}>
        <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: connected ? 'var(--lime-500)' : 'var(--text-faint)' }}>
          {connected ? `${connected} ${connected === 1 ? 'provider' : 'providers'} connected` : 'nothing connected yet'}
        </span>
        <div style={{ display: 'flex', gap: 8 }}>
          <Button variant="ghost" onClick={onDone}>Skip for now</Button>
          <Button variant="primary" onClick={onDone} disabled={!connected}>Go to dashboard →</Button>
        </div>
      </div>

      {modalProv && (
        <window.AddAccountModal initialProvider={modalProv}
          onClose={() => { setModalProv(null); if (window.fxRefresh) window.fxRefresh(); }} />
      )}
    </div>
  );
}

window.Onboarding = Onboarding;
