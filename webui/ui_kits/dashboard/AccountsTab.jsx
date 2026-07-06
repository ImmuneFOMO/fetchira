/* Accounts: management table. API keys never shown — masked chip only. */
const { Card, Badge, Button, QuotaMeter, StatusDot, Input } = window.FetchiraDesignSystem_6526df;

function statusBadge(s) {
  if (s === 'exhausted') return <Badge tone="out" dot>exhausted</Badge>;
  if (s === 'needs-login') return <Badge tone="off" dot>needs login</Badge>;
  return <Badge tone="ok" dot>healthy</Badge>;
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

function RowActions({ r }) {
  const [busy, setBusy] = React.useState(false);
  const [test, setTest] = React.useState(null);
  const [confirmRm, setConfirmRm] = React.useState(false);
  const [paste, setPaste] = React.useState(false);
  const [edit, setEdit] = React.useState(false);
  const needsLogin = r.status === 'needs-login';

  const doTest = async () => {
    if (busy) return;
    setBusy(true); setTest(null);
    try { setTest(await window.apiPost('/api/account/test', { label: r.label })); }
    catch (e) { setTest({ ok: false, error: String(e.message || e) }); }
    setBusy(false);
  };
  const doLogin = async () => {
    if (busy) return;
    setBusy(true); setTest(null);
    try { await window.apiPost('/api/account/login', { label: r.label }); if (window.fxRefresh) window.fxRefresh(); }
    catch (e) { setTest({ ok: false, error: String(e.message || e) }); setBusy(false); }
  };
  const doRemove = async () => {
    if (busy) return;
    if (!confirmRm) { setConfirmRm(true); return; }
    setBusy(true);
    try { await window.apiPost('/api/account/remove', { label: r.label }); if (window.fxRefresh) window.fxRefresh(); }
    catch (e) { setTest({ ok: false, error: String(e.message || e) }); setBusy(false); setConfirmRm(false); }
  };

  return (
    <div style={{ display: 'flex', gap: 6, justifyContent: 'flex-end', alignItems: 'center' }}>
      {test && <span title={test.error || ''} style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: test.ok ? 'var(--green-500)' : 'var(--red-500)' }}>{test.ok ? ('✓ ' + test.latencyMs + 'ms') : '✕ failed'}</span>}
      <Button size="sm" variant="ghost" onClick={doTest}>Test</Button>
      {r.web && <Button size="sm" variant={needsLogin ? 'primary' : 'secondary'} onClick={doLogin}>{needsLogin ? 'Login' : 'Re-login'}</Button>}
      {r.web && <Button size="sm" variant="ghost" onClick={() => setPaste(true)}>Session</Button>}
      <Button size="sm" variant="ghost" onClick={() => setEdit(true)}>Edit</Button>
      <Button size="sm" variant="ghost" onClick={doRemove} style={{ color: confirmRm ? 'var(--red-500)' : 'var(--text-faint)' }}>{confirmRm ? 'Confirm?' : 'Remove'}</Button>
      {paste && <PasteSessionModal label={r.label} onClose={() => setPaste(false)} />}
      {edit && <RenameModal label={r.label} onClose={() => setEdit(false)} />}
    </div>
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
            {rows.map((r, i) => {
              const needsLogin = r.status === 'needs-login';
              return (
                <tr key={r.label} style={{ borderTop: '1px solid var(--border-faint)' }}
                  onMouseEnter={(e) => e.currentTarget.style.background = 'var(--surface-2)'}
                  onMouseLeave={(e) => e.currentTarget.style.background = 'transparent'}>
                  <td style={{ width: 8, padding: 0 }}>
                    <span style={{ display: 'block', width: 3, height: 34, marginLeft: 6, borderRadius: 2, background: r.status === 'exhausted' ? 'var(--red-500)' : needsLogin ? 'var(--grey-500)' : 'var(--green-500)' }} />
                  </td>
                  <td style={{ padding: '12px 14px' }}>
                    <div style={{ fontFamily: 'var(--font-mono)', fontSize: 13, color: 'var(--text-hi)', fontWeight: 600 }}>{r.label}</div>
                    <div style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-faint)' }}>{r.provider}</div>
                    {r.limits && (
                      <div style={{ marginTop: 5, display: 'flex', flexWrap: 'wrap', gap: 4, maxWidth: 280 }}>
                        {r.limits.tier && <Badge tone="cyan" variant="outline">{r.limits.tier}</Badge>}
                        {(r.limits.features || []).map((f) => (
                          <span key={f.feature} title={f.resetAfter ? 'resets ' + f.resetAfter : ''}
                            style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: 'var(--text-lo)', border: '1px solid var(--border-faint)', borderRadius: 4, padding: '1px 5px' }}>
                            {f.feature} <b style={{ color: 'var(--text-hi)' }}>{f.total != null ? f.remaining + '/' + f.total : f.remaining}</b>
                          </span>
                        ))}
                        {(r.limits.models || []).map((m) => (
                          <span key={m.id} title={m.windowSecs ? 'rolling ' + Math.round(m.windowSecs / 3600) + 'h' : (m.resetAfter ? 'resets ' + m.resetAfter : (m.locked ? 'locked on this tier' : ''))}
                            style={{ fontFamily: 'var(--font-mono)', fontSize: 10, color: m.locked ? 'var(--text-faint)' : 'var(--text-lo)', border: '1px solid var(--border-faint)', borderRadius: 4, padding: '1px 5px', opacity: m.locked ? 0.65 : 1 }}>
                            {m.name}{m.levels && m.levels.length ? ' ·' + m.levels.join('/') : ''} <b style={{ color: m.locked ? 'var(--text-faint)' : 'var(--text-hi)' }}>{m.locked ? '0/0' : (m.total != null ? m.remaining + '/' + m.total : (m.remaining != null ? m.remaining : '—'))}</b>
                          </span>
                        ))}
                      </div>
                    )}
                  </td>
                  <td style={{ padding: '12px 14px' }}>
                    <QuotaMeter used={r.used} quota={r.quota} variant="bar" size="sm" showValues={false} state={needsLogin ? 'off' : undefined} style={{ marginBottom: 4 }} />
                    <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--text-lo)' }}>{needsLogin ? '—' : (r.quota - r.used).toLocaleString()} <span style={{ color: 'var(--text-faint)' }}>/ {r.quota.toLocaleString()}</span></span>
                  </td>
                  <td style={{ padding: '12px 14px' }}><Badge tone="neutral" variant="outline" uppercase>{r.resetWindow}</Badge></td>
                  <td style={{ padding: '12px 14px', fontFamily: 'var(--font-mono)', fontSize: 12, color: r.proxy === 'direct' ? 'var(--text-faint)' : 'var(--text-mid)' }}>
                    {r.proxy === 'pool' ? <Badge tone="cyan" variant="outline">pool</Badge> : r.proxy}
                  </td>
                  <td style={{ padding: '12px 14px' }}>
                    {r.key ? <Badge tone="ok" variant="outline">•••• key set</Badge> : r.web ? <Badge tone={r.loggedIn ? 'cyan' : 'off'} variant="outline">{r.loggedIn ? 'session ✓' : 'no session'}</Badge> : <Badge tone="off" variant="outline">no key</Badge>}
                  </td>
                  <td style={{ padding: '12px 14px' }}>{statusBadge(r.status)}</td>
                  <td style={{ padding: '12px 14px' }}>
                    <RowActions r={r} />
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </Card>
  );
}

window.AccountsTab = AccountsTab;
