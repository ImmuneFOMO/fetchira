/* Accounts: management table. API keys never shown — masked chip only. */
const { Card, Badge, Button, QuotaMeter, StatusDot, Input } = window.FetchiraDesignSystem_6526df;

function statusBadge(s) {
  if (s === 'exhausted') return <Badge tone="out" dot>exhausted</Badge>;
  if (s === 'needs-login') return <Badge tone="off" dot>needs login</Badge>;
  return <Badge tone="ok" dot>healthy</Badge>;
}

// "anton.bavirov@gmail.com" -> "an****@gm****om"
function maskEmail(e) {
  const at = String(e).indexOf('@');
  if (at < 1) return e;
  const local = e.slice(0, at), domain = e.slice(at + 1);
  return `${local.slice(0, 2)}****@${domain.slice(0, 2)}****${domain.slice(-2)}`;
}

// Account email chip: masked by default, click to reveal (loopback-only data, but not shouted).
function EmailChip({ email }) {
  const [show, setShow] = React.useState(false);
  return (
    <span onClick={(e) => { e.stopPropagation(); setShow((s) => !s); }} title="click to reveal"
      style={{ cursor: 'pointer', color: 'var(--text-lo)' }}>{show ? email : maskEmail(email)}</span>
  );
}

function Th({ children, style }) {
  return <th style={{ textAlign: 'left', fontFamily: 'var(--font-mono)', fontSize: 10, fontWeight: 600, letterSpacing: '0.1em', textTransform: 'uppercase', color: 'var(--text-faint)', padding: '0 14px 10px', ...style }}>{children}</th>;
}

// Attach a session captured elsewhere (any browser) to an existing web account — the headless path.
function PasteSessionModal({ label, onClose }) {
  const [val, setVal] = React.useState('');
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState(null);
  const save = async () => {
    if (busy || !val.trim()) return;
    setError(null); setBusy(true);
    try { await window.apiPost('/api/account/session', { label, session: val }); onClose(); if (window.fxRefresh) window.fxRefresh(); }
    catch (e) { setError(String(e.message || e)); setBusy(false); }
  };
  return (
    <div onClick={onClose} style={{ position: 'fixed', inset: 0, zIndex: 50, display: 'flex', alignItems: 'center', justifyContent: 'center', background: 'rgba(4,5,8,0.66)', backdropFilter: 'blur(3px)', padding: 20 }}>
      <div onClick={(e) => e.stopPropagation()} style={{ width: 460, maxWidth: '100%', background: 'var(--surface-raised, #0e1016)', border: '1px solid var(--border-hairline)', borderRadius: 'var(--r-lg)', overflow: 'hidden' }}>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '16px 20px', borderBottom: '1px solid var(--border-hairline)' }}>
          <span style={{ fontFamily: 'var(--font-display)', fontSize: 16, fontWeight: 600, color: 'var(--text-hi)' }}>Paste session · <span style={{ color: 'var(--lime-500)' }}>{label}</span></span>
          <button onClick={onClose} style={{ background: 'transparent', border: 'none', color: 'var(--text-lo)', cursor: 'pointer', fontSize: 18, padding: 4 }}>✕</button>
        </div>
        <div style={{ padding: 20, display: 'flex', flexDirection: 'column', gap: 10 }}>
          <textarea value={val} onChange={(e) => setVal(e.target.value)} spellCheck={false} autoFocus
            placeholder={'[{"name":"sso","value":"…","domain":".grok.com"}]'}
            style={{ width: '100%', minHeight: 120, resize: 'vertical', boxSizing: 'border-box', padding: '8px 10px', fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-hi)', background: 'var(--surface-sunken, rgba(255,255,255,0.03))', border: '1px solid var(--border-hairline)', borderRadius: 'var(--r-sm)' }} />
          <span style={{ fontFamily: 'var(--font-ui)', fontSize: 12, color: 'var(--text-lo)' }}>Cookie array or {'{"cookies":[…]}'} exported from a logged-in browser.</span>
          {error && <div style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--red-500)', background: 'var(--red-dim)', border: '1px solid rgba(242,85,90,0.3)', borderRadius: 'var(--r-sm)', padding: '8px 10px' }}>{error}</div>}
        </div>
        <div style={{ display: 'flex', gap: 8, padding: '14px 20px', borderTop: '1px solid var(--border-hairline)', justifyContent: 'flex-end' }}>
          <Button variant="ghost" onClick={onClose}>Cancel</Button>
          <Button variant="primary" onClick={save} disabled={busy || !val.trim()}>{busy ? 'Saving…' : 'Save session'}</Button>
        </div>
      </div>
    </div>
  );
}

// Rename an account (label is its identity — the backend migrates the session/quota/proxy rows).
function RenameModal({ label, onClose }) {
  const [val, setVal] = React.useState(label);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState(null);
  const save = async () => {
    const next = val.trim();
    if (busy || !next) return;
    if (next === label) { onClose(); return; }
    setError(null); setBusy(true);
    try { await window.apiPost('/api/account/rename', { label, new_label: next }); onClose(); if (window.fxRefresh) window.fxRefresh(); }
    catch (e) { setError(String(e.message || e)); setBusy(false); }
  };
  return (
    <div onClick={onClose} style={{ position: 'fixed', inset: 0, zIndex: 50, display: 'flex', alignItems: 'center', justifyContent: 'center', background: 'rgba(4,5,8,0.66)', backdropFilter: 'blur(3px)', padding: 20 }}>
      <div onClick={(e) => e.stopPropagation()} style={{ width: 420, maxWidth: '100%', background: 'var(--surface-raised, #0e1016)', border: '1px solid var(--border-hairline)', borderRadius: 'var(--r-lg)', overflow: 'hidden' }}>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '16px 20px', borderBottom: '1px solid var(--border-hairline)' }}>
          <span style={{ fontFamily: 'var(--font-display)', fontSize: 16, fontWeight: 600, color: 'var(--text-hi)' }}>Rename · <span style={{ color: 'var(--lime-500)' }}>{label}</span></span>
          <button onClick={onClose} style={{ background: 'transparent', border: 'none', color: 'var(--text-lo)', cursor: 'pointer', fontSize: 18, padding: 4 }}>✕</button>
        </div>
        <div style={{ padding: 20, display: 'flex', flexDirection: 'column', gap: 10 }}>
          <Input label="New label" value={val} mono onChange={(e) => setVal(e.target.value)} />
          {error && <div style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--red-500)', background: 'var(--red-dim)', border: '1px solid rgba(242,85,90,0.3)', borderRadius: 'var(--r-sm)', padding: '8px 10px' }}>{error}</div>}
        </div>
        <div style={{ display: 'flex', gap: 8, padding: '14px 20px', borderTop: '1px solid var(--border-hairline)', justifyContent: 'flex-end' }}>
          <Button variant="ghost" onClick={onClose}>Cancel</Button>
          <Button variant="primary" onClick={save} disabled={busy || !val.trim()}>{busy ? 'Saving…' : 'Rename'}</Button>
        </div>
      </div>
    </div>
  );
}

// Change an account's proxy: one-click Direct / Pool, or a specific URL under Custom. A pinned URL
// is shown masked (creds never reach the browser), so switching to a new custom proxy means typing
// it in full. The raw string is sent as-is; the server normalises + validates it.
function ProxyModal({ label, current, onClose }) {
  const initMode = current === 'pool' ? 'pool' : current === 'direct' ? 'direct' : 'custom';
  const [mode, setMode] = React.useState(initMode);
  const [url, setUrl] = React.useState('');
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState(null);
  const proxy = mode === 'direct' ? '' : mode === 'pool' ? 'pool' : url.trim();
  const save = async () => {
    if (busy) return;
    if (mode === 'custom' && !proxy) { setError('Enter a proxy URL'); return; }
    setError(null); setBusy(true);
    try { await window.apiPost('/api/account/proxy', { label, proxy }); onClose(); if (window.fxRefresh) window.fxRefresh(); }
    catch (e) { setError(String(e.message || e)); setBusy(false); }
  };
  const seg = (val, text) => (
    <button onClick={() => setMode(val)} style={{ flex: 1, padding: '7px 10px', fontFamily: 'var(--font-mono)', fontSize: 12, fontWeight: 600, cursor: 'pointer', color: mode === val ? 'var(--text-hi)' : 'var(--text-lo)', background: mode === val ? 'var(--surface-2)' : 'transparent', border: '1px solid ' + (mode === val ? 'var(--lime-500)' : 'var(--border-hairline)'), borderRadius: 'var(--r-sm)' }}>{text}</button>
  );
  return (
    <div onClick={onClose} style={{ position: 'fixed', inset: 0, zIndex: 50, display: 'flex', alignItems: 'center', justifyContent: 'center', background: 'rgba(4,5,8,0.66)', backdropFilter: 'blur(3px)', padding: 20 }}>
      <div onClick={(e) => e.stopPropagation()} style={{ width: 440, maxWidth: '100%', background: 'var(--surface-raised, #0e1016)', border: '1px solid var(--border-hairline)', borderRadius: 'var(--r-lg)', overflow: 'hidden' }}>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '16px 20px', borderBottom: '1px solid var(--border-hairline)' }}>
          <span style={{ fontFamily: 'var(--font-display)', fontSize: 16, fontWeight: 600, color: 'var(--text-hi)' }}>Proxy · <span style={{ color: 'var(--lime-500)' }}>{label}</span></span>
          <button onClick={onClose} style={{ background: 'transparent', border: 'none', color: 'var(--text-lo)', cursor: 'pointer', fontSize: 18, padding: 4 }}>✕</button>
        </div>
        <div style={{ padding: 20, display: 'flex', flexDirection: 'column', gap: 12 }}>
          <div style={{ display: 'flex', gap: 6 }}>{seg('direct', 'Direct')}{seg('pool', 'Pool')}{seg('custom', 'Custom')}</div>
          {mode === 'custom' && <Input label="Proxy URL" value={url} mono autoFocus
            placeholder="http://user:pass@host:port"
            onChange={(e) => setUrl(e.target.value)}
            hint={initMode === 'custom' ? `Currently ${current} — type a new URL to change it.` : null} />}
          {mode === 'pool' && <span style={{ fontFamily: 'var(--font-ui)', fontSize: 12, color: 'var(--text-lo)' }}>A sticky proxy is assigned from your pool on the next call.</span>}
          {mode === 'direct' && <span style={{ fontFamily: 'var(--font-ui)', fontSize: 12, color: 'var(--text-lo)' }}>Connect directly — no proxy.</span>}
          {error && <div style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--red-500)', background: 'var(--red-dim)', border: '1px solid rgba(242,85,90,0.3)', borderRadius: 'var(--r-sm)', padding: '8px 10px' }}>{error}</div>}
        </div>
        <div style={{ display: 'flex', gap: 8, padding: '14px 20px', borderTop: '1px solid var(--border-hairline)', justifyContent: 'flex-end' }}>
          <Button variant="ghost" onClick={onClose}>Cancel</Button>
          <Button variant="primary" onClick={save} disabled={busy || (mode === 'custom' && !proxy)}>{busy ? 'Saving…' : 'Save proxy'}</Button>
        </div>
      </div>
    </div>
  );
}

// Everything beyond Test/Login lives here so the action column stays one width for every row.
function RowMenu({ r, onLogin, onError }) {
  const [open, setOpen] = React.useState(false);
  const [confirmRm, setConfirmRm] = React.useState(false);
  const [paste, setPaste] = React.useState(false);
  const [edit, setEdit] = React.useState(false);
  const [proxyEdit, setProxyEdit] = React.useState(false);

  const close = () => { setOpen(false); setConfirmRm(false); };
  const doRemove = async () => {
    if (!confirmRm) { setConfirmRm(true); return; }
    close();
    try { await window.apiPost('/api/account/remove', { label: r.label }); if (window.fxRefresh) window.fxRefresh(); }
    catch (e) { onError(String(e.message || e)); }
  };
  const item = (label, onClick, danger) => (
    <button onClick={onClick} style={{ display: 'block', width: '100%', textAlign: 'left', background: 'transparent', border: 'none', cursor: 'pointer', padding: '7px 12px', fontFamily: 'var(--font-mono)', fontSize: 12, color: danger ? 'var(--red-500)' : 'var(--text-mid)', whiteSpace: 'nowrap' }}
      onMouseEnter={(e) => e.currentTarget.style.background = 'var(--surface-2)'}
      onMouseLeave={(e) => e.currentTarget.style.background = 'transparent'}>{label}</button>
  );

  return (
    <span style={{ position: 'relative', display: 'inline-block' }}>
      <Button size="sm" variant="ghost" onClick={() => setOpen((o) => !o)} title="more actions">⋯</Button>
      {open && (
        <React.Fragment>
          <div onClick={close} style={{ position: 'fixed', inset: 0, zIndex: 40 }} />
          <div style={{ position: 'absolute', right: 0, top: '100%', marginTop: 4, zIndex: 41, minWidth: 170, padding: '4px 0', background: 'var(--surface-raised, #0e1016)', border: '1px solid var(--border-hairline)', borderRadius: 'var(--r-sm)', boxShadow: '0 8px 24px rgba(0,0,0,0.5)' }}>
            {item('Rename…', () => { close(); setEdit(true); })}
            {item('Proxy…', () => { close(); setProxyEdit(true); })}
            {r.web && item('Paste session…', () => { close(); setPaste(true); })}
            {r.web && item('Log in via Chrome', () => { close(); onLogin('chrome'); })}
            {r.web && item('Log in via Firefox', () => { close(); onLogin('firefox'); })}
            {item(confirmRm ? 'Really remove?' : 'Remove', doRemove, true)}
          </div>
        </React.Fragment>
      )}
      {paste && <PasteSessionModal label={r.label} onClose={() => setPaste(false)} />}
      {edit && <RenameModal label={r.label} onClose={() => setEdit(false)} />}
      {proxyEdit && <ProxyModal label={r.label} current={r.proxy} onClose={() => setProxyEdit(false)} />}
    </span>
  );
}

function RowActions({ r }) {
  const [busy, setBusy] = React.useState(false);
  const [test, setTest] = React.useState(null);
  const needsLogin = r.status === 'needs-login';

  const doTest = async () => {
    if (busy) return;
    setBusy(true); setTest(null);
    try { setTest(await window.apiPost('/api/account/test', { label: r.label })); }
    catch (e) { setTest({ ok: false, error: String(e.message || e) }); }
    setBusy(false);
  };
  const doLogin = async (browser) => {
    if (busy) return;
    setBusy(true); setTest(null);
    try { await window.apiPost('/api/account/login', { label: r.label, browser }); if (window.fxRefresh) window.fxRefresh(); }
    catch (e) { setTest({ ok: false, error: String(e.message || e) }); }
    setBusy(false);
  };

  return (
    <div onClick={(e) => e.stopPropagation()} style={{ display: 'flex', gap: 6, justifyContent: 'flex-end', alignItems: 'center' }}>
      {/* fixed-width slot so a result appearing never shifts the buttons */}
      <span title={test ? (test.error || '') : ''} style={{ width: 74, textAlign: 'right', fontFamily: 'var(--font-mono)', fontSize: 11, color: test ? (test.ok ? 'var(--green-500)' : 'var(--red-500)') : 'transparent', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
        {busy ? <span style={{ color: 'var(--text-faint)' }}>working…</span> : test ? (test.ok ? '✓ ' + test.latencyMs + 'ms' : '✕ failed') : '·'}
      </span>
      <Button size="sm" variant="ghost" onClick={doTest}>Test</Button>
      {r.web
        ? <Button size="sm" variant={needsLogin ? 'primary' : 'secondary'} onClick={() => doLogin('chrome')}>{needsLogin ? 'Login' : 'Re-login'}</Button>
        : <span style={{ width: 62 }} />}
      <RowMenu r={r} onLogin={doLogin} onError={(e) => setTest({ ok: false, error: e })} />
    </div>
  );
}

// The variable-length detail — tier, per-feature limits, the model catalog — lives in a drawer
// under the row, so every collapsed row keeps the same height.
function LimitChips({ limits }) {
  return (
    <div style={{ display: 'flex', flexWrap: 'wrap', gap: 4 }}>
      {limits.tier && <Badge tone="cyan" variant="outline">{limits.tier}</Badge>}
      {(limits.features || []).map((f) => (
        <span key={f.feature} title={f.resetAfter ? 'resets ' + f.resetAfter : ''}
          style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: 'var(--text-lo)', border: '1px solid var(--border-faint)', borderRadius: 4, padding: '1px 5px' }}>
          {f.feature} <b style={{ color: 'var(--text-hi)' }}>{f.total != null ? f.remaining + '/' + f.total : f.remaining}</b>
        </span>
      ))}
      {(limits.models || []).map((m) => (
        <span key={m.id} title={m.windowSecs ? 'rolling ' + Math.round(m.windowSecs / 3600) + 'h' : (m.resetAfter ? 'resets ' + m.resetAfter : (m.locked ? 'locked on this tier' : ''))}
          style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: m.locked ? 'var(--text-faint)' : 'var(--text-lo)', border: '1px solid var(--border-faint)', borderRadius: 4, padding: '1px 5px', opacity: m.locked ? 0.65 : 1 }}>
          {m.name}{m.levels && m.levels.length ? ' ·' + m.levels.join('/') : ''} <b style={{ color: m.locked ? 'var(--text-faint)' : 'var(--text-hi)' }}>{m.locked ? '0/0' : (m.total != null ? m.remaining + '/' + m.total : (m.remaining != null ? m.remaining : '—'))}</b>
        </span>
      ))}
    </div>
  );
}

// One fixed-height row; the tier/limits/models detail expands in a drawer row underneath so
// nothing in the table jumps as live limits stream in per account.
function AccountRow({ r }) {
  const [open, setOpen] = React.useState(false);
  const needsLogin = r.status === 'needs-login';
  const spinner = (text) => (
    <span style={{ display: 'flex', alignItems: 'center', gap: 6, fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>
      <span style={{ width: 10, height: 10, border: '1.5px solid var(--border-hairline)', borderTopColor: 'var(--lime-500)', borderRadius: '50%', display: 'inline-block', animation: 'fx-spin 0.8s linear infinite' }} />
      {text}
    </span>
  );
  const hasDetail = !!r.limits || (r.web && r.loggedIn);
  const td = { padding: '11px 14px', whiteSpace: 'nowrap' };
  return (
    <React.Fragment>
      <tr style={{ borderTop: '1px solid var(--border-faint)', height: 58, cursor: hasDetail ? 'pointer' : 'default' }}
        onClick={hasDetail ? () => setOpen((o) => !o) : undefined}
        onMouseEnter={(e) => e.currentTarget.style.background = 'var(--surface-2)'}
        onMouseLeave={(e) => e.currentTarget.style.background = 'transparent'}>
        <td style={{ width: 8, padding: 0 }}>
          <span style={{ display: 'block', width: 3, height: 34, marginLeft: 6, borderRadius: 2, background: r.status === 'exhausted' ? 'var(--red-500)' : needsLogin ? 'var(--grey-500)' : 'var(--green-500)' }} />
        </td>
        <td style={td}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
            <span style={{ width: 10, flexShrink: 0, fontFamily: 'var(--font-mono)', fontSize: 10, color: 'var(--text-faint)' }}>{hasDetail ? (open ? '▾' : '▸') : ''}</span>
            <div style={{ minWidth: 0 }}>
              <div style={{ fontFamily: 'var(--font-mono)', fontSize: 13, color: 'var(--text-hi)', fontWeight: 600 }}>{r.label}</div>
              <div style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)', overflow: 'hidden', textOverflow: 'ellipsis' }}>
                {r.provider}
                {r.email ? <span> · <EmailChip email={r.email} /></span> : null}
                {r.limits && r.limits.tier ? <span style={{ color: 'var(--cyan-500)' }}> · {r.limits.tier}</span> : null}
              </div>
            </div>
          </div>
        </td>
        <td style={{ ...td, width: 220 }}>
          {r.pending ? spinner('loading…') : (
            <React.Fragment>
              <QuotaMeter used={r.used} quota={r.quota} variant="bar" size="sm" showValues={false} state={needsLogin ? 'off' : undefined} style={{ marginBottom: 4 }} />
              <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-lo)' }}>{needsLogin ? '—' : (r.quota - r.used).toLocaleString()} <span style={{ color: 'var(--text-faint)' }}>/ {r.quota.toLocaleString()}</span></span>
            </React.Fragment>
          )}
        </td>
        <td style={td}><Badge tone="neutral" variant="outline" uppercase>{r.resetWindow}</Badge></td>
        <td style={{ ...td, fontFamily: 'var(--font-mono)', fontSize: 12, color: r.proxy === 'direct' ? 'var(--text-faint)' : 'var(--text-mid)' }}>
          {r.proxy === 'pool' ? <Badge tone="cyan" variant="outline">pool</Badge> : r.proxy}
        </td>
        <td style={td}>
          {r.key ? <Badge tone="ok" variant="outline">•••• key set</Badge> : r.web ? <Badge tone={r.loggedIn ? 'cyan' : 'off'} variant="outline">{r.loggedIn ? 'session ✓' : 'no session'}</Badge> : <Badge tone="off" variant="outline">no key</Badge>}
        </td>
        <td style={td}>{statusBadge(r.status)}</td>
        <td style={td}><RowActions r={r} /></td>
      </tr>
      {open && (
        <tr style={{ background: 'var(--surface-inset)' }}>
          <td />
          <td colSpan={7} style={{ padding: '10px 14px 14px 30px' }}>
            {r.limits
              ? <LimitChips limits={r.limits} />
              : spinner('loading live limits…')}
          </td>
        </tr>
      )}
    </React.Fragment>
  );
}

function AccountsTab({ onAdd }) {
  const rows = window.FX.accounts;
  return (
    <Card pad={0} style={{ overflow: 'hidden' }}>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '14px 16px', borderBottom: '1px solid var(--border-hairline)' }}>
        <div style={{ display: 'flex', alignItems: 'baseline', gap: 10 }}>
          <span style={{ fontFamily: 'var(--font-display)', fontSize: 16, fontWeight: 600, color: 'var(--text-hi)' }}>Accounts</span>
          <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--text-lo)' }}>{rows.length} configured</span>
        </div>
        <Button variant="primary" size="sm" iconLeft={<span style={{ fontFamily: 'var(--font-mono)', fontWeight: 700 }}>+</span>} onClick={onAdd}>Add account</Button>
      </div>
      <div style={{ overflowX: 'auto' }}>
        <table style={{ width: '100%', borderCollapse: 'collapse', minWidth: 880 }}>
          <thead>
            <tr style={{ background: 'var(--surface-inset)' }}>
              <th style={{ padding: '10px 0' }} />
              <Th style={{ paddingTop: 10 }}>Account</Th>
              <Th style={{ paddingTop: 10, width: 220 }}>Quota</Th>
              <Th style={{ paddingTop: 10 }}>Reset</Th>
              <Th style={{ paddingTop: 10 }}>Proxy</Th>
              <Th style={{ paddingTop: 10 }}>Key</Th>
              <Th style={{ paddingTop: 10 }}>Status</Th>
              <Th style={{ paddingTop: 10, textAlign: 'right' }}>Actions</Th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => <AccountRow key={r.label} r={r} />)}
          </tbody>
        </table>
      </div>
    </Card>
  );
}

window.AccountsTab = AccountsTab;
