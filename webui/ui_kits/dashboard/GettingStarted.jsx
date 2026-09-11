/* Dismissible getting-started checklist for the local dashboard; hosted uses the key bridge below. */
const { Card, Button, Badge, StatusDot } = window.FetchiraDesignSystem_6526df;

function apiGet(path) {
  return (window.apiGet ? window.apiGet(path) : fetch(path, { headers: { 'x-fetchira-token': window.FX_TOKEN } }).then((r) => (r.ok ? r.json() : null)))
    .catch(() => null);
}

// The local dashboard configures this computer; hosted admin onboarding manages the server.
function ConnectionSetup({ onReady }) {
  const [mode, setMode] = React.useState('local');
  const [endpoint, setEndpoint] = React.useState('');
  const [wasHosted, setWasHosted] = React.useState(false);
  const [clearConfirmed, setClearConfirmed] = React.useState(false);
  const [apiKey, setApiKey] = React.useState('');
  const [loading, setLoading] = React.useState(true);
  const [loadFailed, setLoadFailed] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState('');
  React.useEffect(() => {
    let cancelled = false;
    apiGet('/api/setup').then((data) => {
      if (cancelled) return;
      if (data) { setMode(data.mode); setEndpoint(data.endpoint || ''); setWasHosted(data.mode === 'hosted'); }
      else { setLoadFailed(true); setError('Unable to load connection settings. Reload the page to retry.'); }
      setLoading(false);
    });
    return () => { cancelled = true; };
  }, []);
  const save = async (event) => {
    event.preventDefault();
    if (loading || loadFailed || busy || (mode === 'local' && wasHosted && !clearConfirmed)) return;
    setBusy(true); setError('');
    try {
      const result = await window.apiPost('/api/setup', { mode, ...(mode === 'hosted' ? { endpoint: endpoint.trim(), api_key: apiKey } : { clear_remote: clearConfirmed }) });
      setApiKey('');
      onReady(result.setup.mode);
    } catch (e) { setError(String(e.message || e)); }
    finally { setBusy(false); }
  };
  return <form onSubmit={save} style={{ display: 'grid', gap: 14 }}>
    <fieldset disabled={loading || loadFailed || busy} style={{ border: 0, padding: 0, margin: 0, display: 'grid', gap: 12 }}>
      <legend style={{ color: 'var(--text-hi)', fontSize: 15, fontWeight: 600, marginBottom: 12 }}>Where should Fetchira run?</legend>
      {[
        { value: 'local', label: 'On this computer', hint: 'Use your own API keys and browser sessions on this computer.' },
        { value: 'hosted', label: 'Connect to a server', hint: 'Use an existing hosted Fetchira. You need its URL and API key.' },
      ].map((choice) => <label key={choice.value} style={{ display: 'flex', alignItems: 'flex-start', gap: 8, cursor: 'pointer' }}>
        <input type="radio" name="fetchira-connection" checked={mode === choice.value} onChange={() => setMode(choice.value)} />
        <span><span style={{ display: 'block', color: 'var(--text-hi)', fontSize: 13 }}>{choice.label}</span><span style={{ display: 'block', color: 'var(--text-mid)', fontSize: 12, lineHeight: 1.5 }}>{choice.hint}</span></span>
      </label>)}
      {mode === 'local' && wasHosted && <label style={{ display: 'flex', alignItems: 'flex-start', gap: 8, color: 'var(--text-mid)', fontSize: 12, lineHeight: 1.5 }}>
        <input type="checkbox" checked={clearConfirmed} onChange={(e) => setClearConfirmed(e.target.checked)} />
        Clear the saved server URL and API key. I have kept the key if I need to reconnect.
      </label>}
      {mode === 'hosted' && <div style={{ display: 'grid', gap: 10 }}>
        <label style={{ display: 'grid', gap: 5, color: 'var(--text-mid)', fontSize: 12 }}>Server URL
          <input type="url" required value={endpoint} onChange={(e) => setEndpoint(e.target.value)} placeholder="https://fetchira.example.com/mcp" style={{ width: '100%', boxSizing: 'border-box', padding: '9px 10px', borderRadius: 'var(--r-sm)', border: '1px solid var(--border-hairline)', background: 'var(--surface-sunken)', color: 'var(--text-hi)' }} />
        </label>
        <label style={{ display: 'grid', gap: 5, color: 'var(--text-mid)', fontSize: 12 }}>API key
          <input type="password" autoComplete="off" value={apiKey} onChange={(e) => setApiKey(e.target.value)} placeholder="Server API key" style={{ width: '100%', boxSizing: 'border-box', padding: '9px 10px', borderRadius: 'var(--r-sm)', border: '1px solid var(--border-hairline)', background: 'var(--surface-sunken)', color: 'var(--text-hi)' }} />
        </label>
        <span style={{ color: 'var(--text-mid)', fontSize: 12 }}>Leave the key blank to keep it only when the URL is unchanged. Access and compatibility are checked before saving; local accounts are preserved.</span>
      </div>}
    </fieldset>
    {error && <div role="alert" style={{ color: 'var(--red-500)', fontSize: 12 }}>{error}</div>}
    <Button type="submit" variant="primary" disabled={loading || loadFailed || busy || (mode === 'local' && wasHosted && !clearConfirmed)} style={{ justifySelf: 'start' }}>{loading ? 'Loading…' : busy ? 'Checking…' : 'Continue'}</Button>
  </form>;
}
window.ConnectionSetup = ConnectionSetup;

const SKILL_VARIANTS = [
  { value: 'both', label: 'MCP with CLI fallback', hint: 'Register MCP and install a skill that uses CLI when MCP is unavailable.' },
  { value: 'mcp', label: 'MCP', hint: 'Register MCP and install the skill for its tools.' },
  { value: 'cli', label: 'CLI only', hint: 'Install the shell skill. No MCP registration is needed.' },
  { value: 'skip', label: 'Later', hint: 'Keep your current agent integrations unchanged.' },
];

// Select one integration and its agent destinations.
// Shared by the onboarding step and the checklist modal.
function InstallTargets({ onDone }) {
  const [targets, setTargets] = React.useState(null);
  const [picked, setPicked] = React.useState({});
  const [skill, setSkill] = React.useState('both');
  const [installedSkill, setInstalledSkill] = React.useState(null);
  const [removeMcp, setRemoveMcp] = React.useState(false);
  const [outdatedSkills, setOutdatedSkills] = React.useState([]);
  const [busy, setBusy] = React.useState(false);
  const [results, setResults] = React.useState(null);
  const [lastAction, setLastAction] = React.useState('install');

  const [failed, setFailed] = React.useState(false);
  const loadTargets = () => {
    setFailed(false);
    apiGet('/api/install/targets').then((d) => {
      if (!d) { setFailed(true); return; }
      const ts = d.agents || [];
      setTargets(ts);
      setOutdatedSkills(d.outdatedSkills || []);
      const installed = Array.isArray(d.skills) ? d.skills : [d.skill];
      const current = ['both', 'mcp', 'cli', 'skip'].find((value) => installed.includes(value));
      if (current) {
        setInstalledSkill(current);
        if (new Set(installed.filter((value) => ['both', 'mcp', 'cli'].includes(value))).size === 1) setSkill(current);
      }
      const pre = {};
      const hasInstalled = ts.some((t) => t.installed);
      ts.forEach((t) => { if (hasInstalled ? t.installed : t.present) pre[t.name] = true; });
      setPicked(pre);
    });
  };
  React.useEffect(loadTargets, []);

  const install = async () => {
    const names = (targets || []).filter((t) => picked[t.name] && (skill !== 'cli' || t.skillSupported)).map((t) => t.name);
    if (busy || (skill !== 'skip' && !names.length)) return;
    setBusy(true);
    setResults(null);
    setLastAction('install');
    try {
      const data = await window.apiPost('/api/install', { targets: names, skill, remove_mcp: skill === 'cli' && removeMcp });
      setResults(data.results || []);
      if (onDone && (data.results || []).every((r) => r.ok)) onDone();
    } catch (e) {
      setResults([{ name: 'error', ok: false, msg: String(e.message || e) }]);
    } finally { setBusy(false); }
  };

  const refresh = async () => {
    if (busy) return;
    setBusy(true);
    setLastAction('refresh');
    try {
      const data = await window.apiPost('/api/install/refresh', {});
      setResults(data.results || []);
    } catch (e) { setResults([{ name: 'error', ok: false, msg: String(e.message || e) }]); }
    finally { setBusy(false); }
  };
  const existingMcp = (targets || []).filter((t) => picked[t.name] && t.skillSupported && t.mcpInstalled);
  const retry = () => { if (!busy) { if (lastAction === 'refresh') refresh(); else install(); } };
  const failedResult = results && results.some((r) => !r.ok);

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
      {failed ? (
        <React.Fragment>
          <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--red-500)' }}>couldn't reach the server</span>
          <Button variant="ghost" onClick={loadTargets} style={{ alignSelf: 'flex-start' }}>Try again</Button>
        </React.Fragment>
      ) : !targets ? (
        <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-faint)' }}>detecting tools…</span>
      ) : results ? (
        <React.Fragment>
          {results.map((r) => (
            <div key={r.name} style={{ display: 'flex', flexWrap: 'wrap', alignItems: 'baseline', gap: 8, fontFamily: 'var(--font-mono)', fontSize: 12 }}>
              <span style={{ color: r.ok ? 'var(--green-500)' : 'var(--red-500)' }}>{r.ok ? '✓' : '✗'}</span>
              <span style={{ color: 'var(--text-hi)', width: 120, flexShrink: 0 }}>{r.name}</span>
              <span style={{ color: 'var(--text-mid)', overflowWrap: 'anywhere', minWidth: 0 }}>{r.msg}</span>
            </div>
          ))}
          {failedResult ? (
            <div style={{ display: 'flex', gap: 8 }}>
              <Button variant="ghost" onClick={retry} disabled={busy}>Try again</Button>
              <Button variant="ghost" onClick={() => setResults(null)} disabled={busy}>Change selection</Button>
            </div>
          ) : (
            <span style={{ fontFamily: 'var(--font-ui)', fontSize: 12, color: 'var(--text-lo)' }}>
              {lastAction === 'refresh' ? 'Skills and launchers refreshed; your integration choices are unchanged. Restart your agents.' : skill === 'skip' ? 'Your integration settings are unchanged.' : 'Restart the agent to load the selected integrations.'}
            </span>
          )}
        </React.Fragment>
      ) : (
        <React.Fragment>
          {outdatedSkills.length > 0 && <div style={{ display: 'grid', gap: 8, color: 'var(--text-mid)', fontSize: 12 }}>
            <span>Updated skills are available for {outdatedSkills.map((item) => item.name).join(', ')}. Keep your current integration and back up previous files.</span>
            <Button variant="ghost" onClick={refresh} disabled={busy} style={{ justifySelf: 'start' }}>Refresh existing skills</Button>
          </div>}
          <fieldset disabled={busy} style={{ border: 0, padding: 0, margin: 0, display: 'flex', flexDirection: 'column', gap: 7 }}>
            <legend style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-mid)', marginBottom: 2 }}>How should your agents use Fetchira?</legend>
            {SKILL_VARIANTS.map((variant) => (
              <label key={variant.value} style={{ display: 'flex', alignItems: 'flex-start', gap: 8, cursor: 'pointer', fontFamily: 'var(--font-ui)', fontSize: 12, color: 'var(--text-hi)' }}>
                <input type="radio" name="fetchira-skill" value={variant.value} checked={skill === variant.value}
                  onChange={() => { setSkill(variant.value); setRemoveMcp(false); }} />
                <span>
                  <span style={{ display: 'block' }}>{variant.label}{installedSkill === variant.value ? ' · installed' : ''}</span>
                  <span style={{ display: 'block', color: 'var(--text-mid)', fontSize: 12 }}>{variant.hint}</span>
                </span>
              </label>
            ))}
          </fieldset>
          {skill !== 'skip' && <fieldset disabled={busy} style={{ border: 0, padding: 0, margin: 0, display: 'grid', gap: 10 }}>
            <legend style={{ color: 'var(--text-mid)', fontSize: 12, marginBottom: 8 }}>Choose agents</legend>
          {targets.filter((t) => skill !== 'cli' || t.skillSupported).map((t) => (
            <label key={t.name} style={{ display: 'flex', alignItems: 'center', gap: 10, cursor: 'pointer', fontFamily: 'var(--font-mono)', fontSize: 13, color: 'var(--text-hi)' }}>
              <input type="checkbox" checked={!!picked[t.name]}
                onChange={(e) => { setPicked((p) => ({ ...p, [t.name]: e.target.checked })); setRemoveMcp(false); }} />
              {t.name}
              {t.mcpInstalled ? <Badge tone="accent" variant="outline">MCP installed</Badge> : null}
              {t.present ? <Badge tone="accent" variant="outline">detected</Badge> : null}
              {skill !== 'cli' && !t.skillSupported && <span style={{ fontSize: 11, color: 'var(--text-mid)' }}>MCP registration only</span>}
            </label>
          ))}
          <span style={{ color: 'var(--text-mid)', fontSize: 12 }}>Codex and Gemini may share a skill directory. Cursor also reads other agents’ skill folders, so a skill can be available to several agents.</span>
          </fieldset>}
          {skill === 'cli' && existingMcp.length > 0 && <label style={{ display: 'flex', alignItems: 'flex-start', gap: 8, color: 'var(--text-mid)', fontSize: 12, lineHeight: 1.5 }}>
            <input type="checkbox" checked={removeMcp} disabled={busy} onChange={(event) => setRemoveMcp(event.target.checked)} />
            <span>Remove Fetchira MCP from {existingMcp.map((target) => target.name).join(', ')} after the CLI skill installs. Back up the configs and keep other tools. If unchecked, existing MCP tools remain available.</span>
          </label>}
          <Button variant="primary" onClick={install}
            disabled={busy || (skill !== 'skip' && !targets.some((t) => picked[t.name] && (skill !== 'cli' || t.skillSupported)))}
            style={{ alignSelf: 'flex-start' }}>
            {busy ? 'Installing…' : skill === 'skip' ? 'Keep current settings' : 'Install'}
          </Button>
        </React.Fragment>
      )}
    </div>
  );
}
window.InstallTargets = InstallTargets;

function InstallPanel({ onClose }) {
  const [connectionOpen, setConnectionOpen] = React.useState(false);
  return (
    <div onClick={onClose} style={{ position: 'fixed', inset: 0, zIndex: 50, display: 'flex', alignItems: 'center', justifyContent: 'center', background: 'rgba(4,5,8,0.66)', backdropFilter: 'blur(3px)', padding: 20 }}>
      <div onClick={(e) => e.stopPropagation()} style={{ width: 440, maxWidth: '100%', maxHeight: 'calc(100dvh - 40px)', overflowY: 'auto' }}>
        <Card raised pad={0} style={{ borderRadius: 'var(--r-lg)' }}>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '16px 20px', borderBottom: '1px solid var(--border-hairline)' }}>
            <span style={{ fontFamily: 'var(--font-display)', fontSize: 17, fontWeight: 600, color: 'var(--text-hi)' }}>Connect your coding agents</span>
            <button aria-label="Close integration settings" onClick={onClose} style={{ background: 'transparent', border: 'none', color: 'var(--text-lo)', cursor: 'pointer', fontSize: 18, lineHeight: 1, padding: 4 }}>✕</button>
          </div>
          <div style={{ padding: 20, display: 'grid', gap: 16 }}>
            {connectionOpen ? <ConnectionSetup onReady={() => setConnectionOpen(false)} /> : <>
              <Button variant="ghost" onClick={() => setConnectionOpen(true)} style={{ justifySelf: 'start' }}>Local or hosted connection</Button>
              <InstallTargets />
            </>}
          </div>
          <div style={{ display: 'flex', gap: 8, padding: '14px 20px', borderTop: '1px solid var(--border-hairline)', justifyContent: 'flex-end' }}>
            <Button variant="ghost" onClick={onClose}>Close</Button>
          </div>
        </Card>
      </div>
    </div>
  );
}

window.InstallPanel = InstallPanel;

function GettingStarted() {
  return window.fxHosted ? <HostedGettingStarted /> : <><UpgradeNotice /><LocalGettingStarted /></>;
}

// Upgrade guidance remains visible even when the old getting-started checklist was dismissed.
function UpgradeNotice() {
  const [data, setData] = React.useState(null);
  const [open, setOpen] = React.useState(false);
  const [dismissed, setDismissed] = React.useState(false);
  const [refreshResults, setRefreshResults] = React.useState(null);
  React.useEffect(() => {
    let cancelled = false;
    apiGet('/api/install/targets').then((result) => {
      if (!cancelled && result) {
        setData(result);
        setDismissed(localStorage.getItem('fx-agent-upgrade-' + result.version) === 'dismissed');
        if (((result.outdatedSkills || []).length || result.upgradePending) && !refreshResults) {
          window.apiPost('/api/install/refresh', {}).then((updated) => {
            if (!cancelled) setRefreshResults(updated.results || []);
          }).catch((error) => {
            if (!cancelled) setRefreshResults([{ name: 'Skills', ok: false, msg: String(error.message || error) }]);
          });
        }
      }
    });
    return () => { cancelled = true; };
  }, [open]);
  const stale = (data?.outdatedSkills || []).length > 0;
  const legacyMcp = (data?.agents || []).filter((target) => target.mcpInstalled && target.skillSupported && !target.skillInstalled);
  if (!data || dismissed || (!stale && !legacyMcp.length && !refreshResults && !data.upgradePending)) return null;
  return <>
    <Card pad={16} style={{ marginBottom: 16 }}>
      <div style={{ display: 'grid', gap: 10 }}>
        <span style={{ color: 'var(--text-hi)', fontWeight: 600 }}>Finish your Fetchira update</span>
        <span style={{ color: 'var(--text-mid)', fontSize: 13, lineHeight: 1.5 }}>
          {refreshResults ? (refreshResults.every((item) => item.ok) ? 'Your existing skills and launchers are up to date. Changed files were backed up; your integration choices are unchanged.' : 'Some skills need attention. Review the results in agent setup; your existing files are preserved.') : stale ? 'Refreshing your existing agent skills and backing up previous files…' : `Fetchira MCP is installed in ${legacyMcp.map((target) => target.name).join(', ')}. Add the current skill or switch to CLI-only.`} Switching to CLI-only is optional; MCP is removed only with your confirmation.
        </span>
        <div style={{ display: 'flex', gap: 8 }}>
          <Button variant="primary" onClick={() => setOpen(true)}>Review agent setup</Button>
          <Button variant="ghost" onClick={() => { localStorage.setItem('fx-agent-upgrade-' + data.version, 'dismissed'); setDismissed(true); }}>Later</Button>
        </div>
      </div>
    </Card>
    {open && <InstallPanel onClose={() => setOpen(false)} />}
  </>;
}

function LocalGettingStarted() {
  const [hidden, setHidden] = React.useState(() => localStorage.getItem('fx-gs-dismissed') === '1');
  const [installOpen, setInstallOpen] = React.useState(false);
  const [modalProv, setModalProv] = React.useState(null);
  const [hostedEndpoint, setHostedEndpoint] = React.useState(null);
  const [registered, setRegistered] = React.useState(null); // null until the targets probe lands

  React.useEffect(() => {
    if (hidden) return;
    let cancelled = false;
    Promise.all([apiGet('/api/install/targets'), apiGet('/api/setup')]).then(([d, setup]) => {
      if (cancelled) return;
      setHostedEndpoint(setup?.mode === 'hosted' ? setup.endpoint : null);
      if (d) {
        const skillInstalled = (Array.isArray(d.skills) ? d.skills : [d.skill])
          .some((skill) => ['both', 'mcp', 'cli'].includes(skill));
        setRegistered(skillInstalled || (d.targets || []).some((t) => t.installed));
      }
    });
    return () => { cancelled = true; };
  }, [installOpen, hidden]); // Re-check after connection or installation settings close.

  if (hidden) return null;
  const accounts = window.FX.accounts || [];
  const items = [
    ...(hostedEndpoint ? [{
      label: 'Connected to a hosted server',
      hint: hostedEndpoint,
      done: true,
      action: () => setInstallOpen(true),
    }] : [{
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
    }]),
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
            <span style={{ width: 16, height: 16, flexShrink: 0, borderRadius: '50%', display: 'inline-flex', alignItems: 'center', justifyContent: 'center', fontSize: 10, color: it.done ? 'var(--green-500)' : 'transparent', border: it.done ? '1px solid rgba(70,209,122,0.5)' : '1px solid var(--border-strong)', background: it.done ? 'var(--green-dim)' : 'transparent' }}>{it.done ? '✓' : ''}</span>
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
          return (candidate.scopes || []).includes('mcp') && (!candidate.expiresAt || new Date(candidate.expiresAt).getTime() > Date.now());
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
      const result = await window.hostedAdminJSON('/admin/keys', { method: 'POST', body: { id, name: 'Local Fetchira', scopes: ['mcp', 'usage:read'], rpm: 60, daily_limit: 0, monthly_limit: 0, concurrency_limit: 4, expires_at: null } });
      setKey(result.key);
    } catch (e) { setError(e.message || 'Unable to create API key'); }
    finally { setBusy(false); }
  };
  const command = 'fetchira setup';
  const copy = async (value, name) => { try { await navigator.clipboard.writeText(value); setCopied(name); setTimeout(() => setCopied(''), 1600); } catch (_) {} };
  const dismiss = () => { localStorage.setItem('fx-hosted-gs-dismissed', '1'); setHidden(true); };
  return <Card pad={0} style={{ overflow: 'hidden' }}>
    <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '12px 14px', borderBottom: '1px solid var(--border-faint)' }}>
      <StatusDot tone="accent" size={7} /><span style={{ fontFamily: 'var(--font-display)', fontSize: 14, fontWeight: 600, color: 'var(--text-hi)' }}>Connect a local Fetchira</span><span style={{ flex: 1 }} />
      <button aria-label="Dismiss getting started" onClick={dismiss} title="dismiss" style={{ background: 'transparent', border: 0, color: 'var(--text-faint)', cursor: 'pointer', fontSize: 14 }}>✕</button>
    </div>
    <div style={{ display: 'grid', gap: 14, padding: 14 }}>
      <div style={{ color: 'var(--text-lo)', fontSize: 12, lineHeight: 1.5 }}>Install Fetchira locally, generate a key, then run setup. Choose Connect to a server and enter this endpoint and key. Setup checks access before saving.</div>
      <div style={{ display: 'grid', gap: 7 }}><div style={{ color: 'var(--text-faint)', font: '600 10px var(--font-mono)', letterSpacing: '.1em' }}>HOSTED ENDPOINT</div><div className="secret-row"><code>{endpoint}</code><Button variant="secondary" size="sm" onClick={() => copy(endpoint, 'endpoint')}>{copied === 'endpoint' ? 'Copied' : 'Copy'}</Button></div></div>
      <div style={{ display: 'grid', gap: 7 }}><div style={{ color: 'var(--text-faint)', font: '600 10px var(--font-mono)', letterSpacing: '.1em' }}>RUN THIS LOCALLY</div><div className="secret-row"><code>{command}</code><Button variant="secondary" size="sm" onClick={() => copy(command, 'command')}>{copied === 'command' ? 'Copied' : 'Copy'}</Button></div><div style={{ color: 'var(--text-faint)', font: '11px var(--font-mono)' }}>Run it in your terminal, then use <code>fetchira remote check</code> to verify version, schema and access.</div></div>
      {key && <div className="secret-row"><code>{key}</code><Button variant="secondary" size="sm" onClick={() => copy(key, 'key')}>{copied === 'key' ? 'Copied' : 'Copy API key'}</Button></div>}
      {!key && <Button variant="primary" onClick={createKey} disabled={busy}>{busy ? 'Generating…' : 'Generate API key'}</Button>}
      {error && <div role="alert" style={{ color: 'var(--red-500)', font: '12px var(--font-mono)' }}>{error}</div>}
    </div>
  </Card>;
}

window.GettingStarted = GettingStarted;
