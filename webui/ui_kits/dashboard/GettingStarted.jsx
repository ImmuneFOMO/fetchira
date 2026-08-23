/* Dismissible getting-started checklist for the local dashboard; hosted uses the key bridge below. */
const { Card, Button, Badge, StatusDot } = window.FetchiraDesignSystem_6526df;

function apiGet(path) {
  return (window.apiGet ? window.apiGet(path) : fetch(path, { headers: { 'x-fetchira-token': window.FX_TOKEN } }).then((r) => (r.ok ? r.json() : null)))
    .catch(() => null);
}

// Detect coding tools, preselect the not-yet-registered ones, register on click.
// Shared by the onboarding step and the checklist modal.
function InstallTargets({ onDone }) {
  const [targets, setTargets] = React.useState(null);
  const [picked, setPicked] = React.useState({});
  const [busy, setBusy] = React.useState(false);
  const [results, setResults] = React.useState(null);

  const [failed, setFailed] = React.useState(false);
  React.useEffect(() => {
    apiGet('/api/install/targets').then((d) => {
      if (!d) { setFailed(true); return; }
      const ts = d.targets || [];
      setTargets(ts);
      const pre = {};
      ts.forEach((t) => { if (t.present && !t.installed) pre[t.name] = true; });
      setPicked(pre);
    });
  }, []);

  const install = async () => {
    const names = Object.keys(picked).filter((n) => picked[n]);
    if (!names.length || busy) return;
    setBusy(true);
    try { setResults((await window.apiPost('/api/install', { targets: names })).results); }
    catch (e) { setResults([{ name: 'error', ok: false, msg: String(e.message || e) }]); }
    setBusy(false);
    if (onDone) onDone();
  };

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
      {failed ? (
        <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--red-500)' }}>couldn't reach the server — reload this tab</span>
      ) : !targets ? (
        <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-faint)' }}>detecting tools…</span>
      ) : results ? (
        <React.Fragment>
          {results.map((r) => (
            <div key={r.name} style={{ display: 'flex', alignItems: 'baseline', gap: 8, fontFamily: 'var(--font-mono)', fontSize: 12 }}>
              <span style={{ color: r.ok ? 'var(--green-500)' : 'var(--red-500)' }}>{r.ok ? '✓' : '✗'}</span>
              <span style={{ color: 'var(--text-hi)', width: 120, flexShrink: 0 }}>{r.name}</span>
              <span style={{ color: 'var(--text-faint)', wordBreak: 'break-all' }}>{r.msg}</span>
            </div>
          ))}
          <span style={{ fontFamily: 'var(--font-ui)', fontSize: 12, color: 'var(--text-lo)' }}>
            Restart the tool (or reload its MCP servers) to pick up fetchira.
          </span>
        </React.Fragment>
      ) : (
        <React.Fragment>
          {targets.map((t) => (
            <label key={t.name} style={{ display: 'flex', alignItems: 'center', gap: 10, cursor: 'pointer', fontFamily: 'var(--font-mono)', fontSize: 13, color: 'var(--text-hi)' }}>
              <input type="checkbox" checked={!!picked[t.name]}
                onChange={(e) => setPicked((p) => ({ ...p, [t.name]: e.target.checked }))} />
              {t.name}
              {t.installed ? <Badge tone="ok" variant="soft">registered ✓</Badge>
                : t.present ? <Badge tone="accent" variant="outline">detected</Badge> : null}
            </label>
          ))}
          <Button variant="primary" onClick={install}
            disabled={busy || !Object.keys(picked).some((n) => picked[n])}
            style={{ alignSelf: 'flex-start' }}>
            {busy ? 'Registering…' : 'Register'}
          </Button>
        </React.Fragment>
      )}
    </div>
  );
}
window.InstallTargets = InstallTargets;

function InstallPanel({ onClose }) {
  return (
    <div onClick={onClose} style={{ position: 'fixed', inset: 0, zIndex: 50, display: 'flex', alignItems: 'center', justifyContent: 'center', background: 'rgba(4,5,8,0.66)', backdropFilter: 'blur(3px)', padding: 20 }}>
      <div onClick={(e) => e.stopPropagation()} style={{ width: 440, maxWidth: '100%' }}>
        <Card raised pad={0} style={{ borderRadius: 'var(--r-lg)' }}>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '16px 20px', borderBottom: '1px solid var(--border-hairline)' }}>
            <span style={{ fontFamily: 'var(--font-display)', fontSize: 17, fontWeight: 600, color: 'var(--text-hi)' }}>Register in your coding tools</span>
            <button onClick={onClose} style={{ background: 'transparent', border: 'none', color: 'var(--text-lo)', cursor: 'pointer', fontSize: 18, lineHeight: 1, padding: 4 }}>✕</button>
          </div>
          <div style={{ padding: 20 }}>
            <InstallTargets />
          </div>
          <div style={{ display: 'flex', gap: 8, padding: '14px 20px', borderTop: '1px solid var(--border-hairline)', justifyContent: 'flex-end' }}>
            <Button variant="ghost" onClick={onClose}>Close</Button>
          </div>
        </Card>
      </div>
    </div>
  );
}

function GettingStarted() {
  return window.fxHosted ? <HostedGettingStarted /> : <LocalGettingStarted />;
}

function LocalGettingStarted() {
  const [hidden, setHidden] = React.useState(() => localStorage.getItem('fx-gs-dismissed') === '1');
  const [installOpen, setInstallOpen] = React.useState(false);
  const [modalProv, setModalProv] = React.useState(null);
  const [registered, setRegistered] = React.useState(null); // null until the targets probe lands

  React.useEffect(() => {
    if (hidden) return;
    apiGet('/api/install/targets').then((d) => {
      if (d) setRegistered(d.targets.some((t) => t.installed));
    });
  }, [installOpen]); // re-check after the install panel closes

  if (hidden) return null;
  const accounts = window.FX.accounts || [];
  const items = [
    {
      label: 'Connect a provider',
      hint: 'search + read quota for the router',
      done: accounts.length > 0,
      action: () => setModalProv('tavily'),
    },
    {
      label: 'Add a web session',
      hint: 'gemini / grok / chatgpt login — unlocks deep research + images',
      done: accounts.some((a) => a.web && a.loggedIn),
      action: () => setModalProv('gemini_web'),
    },
    {
      label: 'Register fetchira in your coding tools',
      hint: 'one click into Claude Code, Codex, Cursor, …',
      done: !!registered,
      action: () => setInstallOpen(true),
    },
  ];
  const doneCount = items.filter((i) => i.done).length;
  const dismiss = () => { localStorage.setItem('fx-gs-dismissed', '1'); setHidden(true); };
  // All steps done -> self-dismiss for good (wait for the targets probe so we don't misjudge step 3).
  if (registered !== null && doneCount === items.length && !installOpen && !modalProv) {
    localStorage.setItem('fx-gs-dismissed', '1');
    return null;
  }

  return (
    <Card pad={0} style={{ overflow: 'hidden' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '12px 14px', borderBottom: '1px solid var(--border-faint)' }}>
        <StatusDot tone="accent" size={7} />
        <span style={{ fontFamily: 'var(--font-display)', fontSize: 14, fontWeight: 600, color: 'var(--text-hi)' }}>Getting started</span>
        <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>{doneCount}/{items.length}</span>
        <span style={{ flex: 1 }} />
        <button aria-label="Dismiss getting started" onClick={dismiss} title="dismiss" style={{ background: 'transparent', border: 'none', color: 'var(--text-faint)', cursor: 'pointer', fontSize: 14, lineHeight: 1, padding: 2 }}>✕</button>
      </div>
      <div style={{ display: 'flex', flexDirection: 'column' }}>
        {items.map((it, i) => (
          <button key={it.label} onClick={it.action}
            style={{ display: 'flex', alignItems: 'center', gap: 10, padding: '10px 14px', background: 'transparent', border: 'none', borderTop: i ? '1px solid var(--border-faint)' : 'none', cursor: 'pointer', textAlign: 'left', width: '100%' }}>
            <span style={{ width: 16, height: 16, flexShrink: 0, borderRadius: '50%', display: 'inline-flex', alignItems: 'center', justifyContent: 'center', fontSize: 10, color: it.done ? 'var(--green-500)' : 'transparent', border: it.done ? '1px solid rgba(70,209,122,0.5)' : '1px solid var(--border-strong)', background: it.done ? 'var(--green-dim)' : 'transparent' }}>✓</span>
            <span style={{ minWidth: 0 }}>
              <span style={{ display: 'block', fontFamily: 'var(--font-mono)', fontSize: 12.5, color: it.done ? 'var(--text-faint)' : 'var(--text-hi)', textDecoration: it.done ? 'line-through' : 'none' }}>{it.label}</span>
              <span style={{ display: 'block', fontFamily: 'var(--font-ui)', fontSize: 11.5, color: 'var(--text-faint)' }}>{it.hint}</span>
            </span>
            <span style={{ marginLeft: 'auto', fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-faint)' }}>→</span>
          </button>
        ))}
      </div>
      {installOpen && <InstallPanel onClose={() => setInstallOpen(false)} />}
      {modalProv && (
        <window.AddAccountModal initialProvider={modalProv}
          onClose={() => { setModalProv(null); if (window.fxRefresh) window.fxRefresh(); }} />
      )}
    </Card>
  );
}

function HostedGettingStarted() {
  const [hidden, setHidden] = React.useState(() => localStorage.getItem('fx-hosted-gs-dismissed') === '1');
  const [visibility, setVisibility] = React.useState('checking');
  const [key, setKey] = React.useState(null);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState('');
  const [copied, setCopied] = React.useState('');

  React.useEffect(() => {
    let cancelled = false;
    window.hostedAdminJSON('/admin/keys')
      .then((data) => {
        if (cancelled) return;
        const hasActiveKey = (data.keys || []).some((candidate) => {
          if (candidate.revoked) return false;
          return (candidate.scopes || []).includes('accounts:manage') && (!candidate.expiresAt || new Date(candidate.expiresAt).getTime() > Date.now());
        });
        setVisibility(hasActiveKey ? 'hidden' : 'show');
      })
      .catch(() => { if (!cancelled) setVisibility('show'); });
    return () => { cancelled = true; };
  }, []);

  if (hidden || visibility !== 'show') return null;

  const endpoint = `${location.origin}/mcp`;
  const createKey = async () => {
    setBusy(true); setError('');
    try {
      const id = `local-${Math.random().toString(36).slice(2, 8)}`;
      const result = await window.hostedAdminJSON('/admin/keys', { method: 'POST', body: { id, name: 'Local Fetchira', scopes: ['mcp', 'usage:read', 'accounts:manage'], rpm: 60, daily_limit: 0, monthly_limit: 0, concurrency_limit: 4, expires_at: null } });
      setKey(result.key);
    } catch (e) { setError(e.message || 'Unable to create API key'); }
    finally { setBusy(false); }
  };
  const command = `fetchira remote set ${endpoint} --key '${key || 'your-key'}'`;
  const copy = async (value, name) => { try { await navigator.clipboard.writeText(value); setCopied(name); setTimeout(() => setCopied(''), 1600); } catch (_) {} };
  const dismiss = () => { localStorage.setItem('fx-hosted-gs-dismissed', '1'); setHidden(true); };
  return <Card pad={0} style={{ overflow: 'hidden' }}>
    <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '12px 14px', borderBottom: '1px solid var(--border-faint)' }}>
      <StatusDot tone="accent" size={7} /><span style={{ fontFamily: 'var(--font-display)', fontSize: 14, fontWeight: 600, color: 'var(--text-hi)' }}>Connect a local Fetchira</span><span style={{ flex: 1 }} />
      <button aria-label="Dismiss getting started" onClick={dismiss} title="dismiss" style={{ background: 'transparent', border: 0, color: 'var(--text-faint)', cursor: 'pointer', fontSize: 14 }}>✕</button>
    </div>
    <div style={{ display: 'grid', gap: 14, padding: 14 }}>
      <div style={{ color: 'var(--text-lo)', fontSize: 12, lineHeight: 1.5 }}>Install Fetchira locally, generate a scoped key, then run the command below. Your local MCP tools will route through this hosted server.</div>
      <div style={{ display: 'grid', gap: 7 }}><div style={{ color: 'var(--text-faint)', font: '600 10px var(--font-mono)', letterSpacing: '.1em' }}>HOSTED ENDPOINT</div><div className="secret-row"><code>{endpoint}</code><Button variant="secondary" size="sm" onClick={() => copy(endpoint, 'endpoint')}>{copied === 'endpoint' ? 'Copied' : 'Copy'}</Button></div></div>
      <div style={{ display: 'grid', gap: 7 }}><div style={{ color: 'var(--text-faint)', font: '600 10px var(--font-mono)', letterSpacing: '.1em' }}>RUN THIS LOCALLY</div><div className="secret-row"><code>{command}</code><Button variant="secondary" size="sm" onClick={() => copy(command, 'command')}>{copied === 'command' ? 'Copied' : 'Copy'}</Button></div><div style={{ color: 'var(--text-faint)', font: '11px var(--font-mono)' }}>Run it in your terminal, then use <code>fetchira remote check</code> to verify version, schema and access.</div></div>
      {!key && <Button variant="primary" onClick={createKey} disabled={busy}>{busy ? 'Generating…' : 'Generate API key'}</Button>}
      {error && <div role="alert" style={{ color: 'var(--red-500)', font: '12px var(--font-mono)' }}>{error}</div>}
    </div>
  </Card>;
}

window.GettingStarted = GettingStarted;
