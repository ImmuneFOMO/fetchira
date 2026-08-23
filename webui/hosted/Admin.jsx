const { Card: HostedCard, Button: HostedButton, Badge: HostedBadge, StatusDot: HostedStatusDot } = window.FetchiraDesignSystem_6526df;

const HOSTED_SCOPE_INFO = [
  { id: 'mcp', label: 'mcp', description: 'Use Fetchira tools through the hosted endpoint.' },
  { id: 'usage:read', label: 'usage:read', description: 'View this key’s usage, limits and request history.' },
  { id: 'accounts:manage', label: 'accounts:manage', description: 'Add, replace or remove provider sessions.' },
  { id: 'server:update', label: 'server:update', description: 'Start a hosted server update and manage releases.' },
];

const hostedAdminCsrf = () => {
  const prefix = 'fetchira_csrf=';
  return document.cookie.split(';').map((x) => x.trim()).find((x) => x.startsWith(prefix))?.slice(prefix.length) || '';
};

async function hostedAdminJSON(path, options = {}) {
  const headers = { accept: 'application/json', ...(options.headers || {}) };
  if (options.body && typeof options.body !== 'string') {
    headers['content-type'] = 'application/json';
    options.body = JSON.stringify(options.body);
  }
  if (options.method && options.method !== 'GET') headers['x-csrf-token'] = hostedAdminCsrf();
  const response = await fetch(path, { credentials: 'same-origin', ...options, headers });
  let body = null;
  try { body = await response.json(); } catch (_) {}
  if (!response.ok) throw new Error((body && body.error) || body || `request failed (${response.status})`);
  return body;
}

function HostedAdminModal({ title, children, onClose }) {
  return <div role="presentation" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }} style={{ position: 'fixed', inset: 0, zIndex: 80, display: 'grid', placeItems: 'center', padding: 20, background: 'rgba(4,5,8,.72)', backdropFilter: 'blur(4px)' }}>
    <section role="dialog" aria-modal="true" aria-labelledby="hosted-modal-title" style={{ width: 'min(620px,100%)', maxHeight: 'calc(100vh - 40px)', overflow: 'auto', background: 'var(--surface-1)', border: '1px solid var(--border-strong)', borderRadius: 'var(--r-lg)', boxShadow: 'var(--elev-overlay)', padding: 24 }}>
      <header style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', gap: 16, marginBottom: 22, paddingBottom: 14, borderBottom: '1px solid var(--border-faint)' }}><h2 id="hosted-modal-title">{title}</h2><HostedButton variant="ghost" size="sm" onClick={onClose} aria-label="Close">×</HostedButton></header>
      {children}
    </section>
  </div>;
}

function HostedField({ label, children }) {
  return <label style={{ display: 'grid', gap: 7, color: 'var(--text-mid)', font: '600 11px var(--font-mono)' }}>{label}{children}</label>;
}

function HostedAdminFull({ tab }) {
  const [data, setData] = React.useState({ keys: [], providers: [], requests: [], audit: [], update: null });
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState('');
  const [modal, setModal] = React.useState(null);
  const [secret, setSecret] = React.useState('');
  const [challenge, setChallenge] = React.useState(null);
  const [form, setForm] = React.useState({ id: '', name: '', provider: 'chatgpt_web', label: '', session: '', scopes: ['mcp', 'usage:read'], rpm: 60, dailyLimit: 0, monthlyLimit: 0, concurrencyLimit: 4 });

  const refresh = React.useCallback(async () => {
    setBusy(true); setError('');
    try {
      const [keys, providers, requests, audit, update] = await Promise.all([
        hostedAdminJSON('/admin/keys'), hostedAdminJSON('/admin/providers'), hostedAdminJSON('/admin/usage?limit=500'), hostedAdminJSON('/admin/audit?limit=500'), hostedAdminJSON('/admin/update')
      ]);
      setData({ keys: keys.keys || [], providers: providers.providers || [], requests: requests.rows || [], audit: audit.rows || [], update: update.job || update });
    } catch (e) { setError(e.message || 'Unable to load hosted administration'); }
    finally { setBusy(false); }
  }, []);
  React.useEffect(() => { refresh(); }, [refresh, tab]);

  const updateForm = (key, value) => setForm((f) => ({ ...f, [key]: value }));
  const createKey = async (e) => {
    e.preventDefault(); setBusy(true); setError('');
    try {
      const result = await hostedAdminJSON('/admin/keys', { method: 'POST', body: { id: form.id.trim(), name: form.name.trim(), scopes: form.scopes, rpm: Number(form.rpm), daily_limit: Number(form.dailyLimit), monthly_limit: Number(form.monthlyLimit), concurrency_limit: Number(form.concurrencyLimit), expires_at: null } });
      setSecret(result.key); setModal('secret'); await refresh();
    } catch (e) { setError(e.message); }
    finally { setBusy(false); }
  };
  const revoke = async (id) => {
    if (!window.confirm(`Revoke API key ${id}? Existing clients stop immediately.`)) return;
    setBusy(true); setError('');
    try { await hostedAdminJSON(`/admin/keys/${encodeURIComponent(id)}/revoke`, { method: 'POST' }); await refresh(); }
    catch (e) { setError(e.message); }
    finally { setBusy(false); }
  };
  const addProvider = async (e) => {
    e.preventDefault(); setBusy(true); setError('');
    try {
      if (form.session.trim()) {
        await hostedAdminJSON('/admin/providers/session', { method: 'POST', body: { provider: form.provider, label: form.label.trim(), session: form.session.trim() } });
        setModal(null); await refresh(); return;
      }
      const result = await hostedAdminJSON('/admin/login-challenges', { method: 'POST', body: { provider: form.provider, label: form.label.trim() } });
      setChallenge(result); setModal('challenge');
    } catch (e) { setError(e.message); }
    finally { setBusy(false); }
  };
  const title = { keys: 'API keys', providers: 'Providers', requests: 'Requests', audit: 'Audit', update: 'Update' }[tab] || tab;
  const input = (key, type = 'text') => <input type={type} value={form[key]} onChange={(e) => updateForm(key, e.target.value)} required={['id', 'name', 'label'].includes(key)} />;
  const copy = (value) => navigator.clipboard?.writeText(value);

  return <main style={{ maxWidth: 'var(--container-max)', margin: '0 auto', padding: 20 }}>
    <div style={{ display: 'flex', alignItems: 'end', justifyContent: 'space-between', gap: 16, marginBottom: 24 }}><div><div style={{ font: '11px var(--font-mono)', letterSpacing: '.12em', textTransform: 'uppercase', color: 'var(--text-lo)' }}>Hosted administration</div><h1 style={{ marginTop: 5 }}>{title}</h1></div><HostedButton variant="secondary" size="sm" onClick={refresh} disabled={busy}>{busy ? 'Loading…' : 'Refresh'}</HostedButton></div>
    {error && <div role="alert" style={{ marginBottom: 14, padding: 10, border: '1px solid rgba(242,85,90,.35)', borderRadius: 'var(--r-sm)', color: 'var(--red-500)', font: '12px var(--font-mono)' }}>{error}</div>}
    {tab === 'keys' && <HostedCard pad={0} style={{ overflow: 'hidden' }}><header style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', padding: 14, borderBottom: '1px solid var(--border-faint)' }}><div><h2>Friend access</h2><p className="muted" style={{ margin: '4px 0 0' }}>Scoped keys are shown once and stored hashed.</p></div><HostedButton variant="primary" size="sm" onClick={() => { setError(''); setModal('key'); }}>Create key</HostedButton></header><div style={{ overflow: 'auto' }}><table><thead><tr><th>Name</th><th>Scopes</th><th>Limits</th><th>Status</th><th /></tr></thead><tbody>{data.keys.length ? data.keys.map((k) => <tr key={k.id}><td><strong>{k.name}</strong><br /><code>{k.id}</code></td><td>{(k.scopes || []).map((s) => <HostedBadge key={s} tone="neutral" variant="outline">{s}</HostedBadge>)}</td><td className="mono">{k.rpm} rpm · {k.dailyLimit || '∞'} day<br />{k.concurrencyLimit} concurrent</td><td><HostedBadge tone={k.revoked ? 'out' : 'ok'}>{k.revoked ? 'revoked' : 'active'}</HostedBadge></td><td>{!k.revoked && <HostedButton variant="danger" size="sm" onClick={() => revoke(k.id)} disabled={busy}>Revoke</HostedButton>}</td></tr>) : <tr><td colSpan="5" className="empty">No API keys yet</td></tr>}</tbody></table></div></HostedCard>}
    {tab === 'providers' && <HostedCard pad={0} style={{ overflow: 'hidden' }}><header style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', padding: 14, borderBottom: '1px solid var(--border-faint)' }}><div><h2>Provider sessions</h2><p className="muted" style={{ margin: '4px 0 0' }}>Credentials stay encrypted on the server.</p></div><HostedButton variant="primary" size="sm" onClick={() => { setError(''); setModal('provider'); }}>Add provider</HostedButton></header><div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit,minmax(280px,1fr))', gap: 14, padding: 16 }}>{data.providers.length ? data.providers.map((p) => <HostedCard key={p.label} accent={p.ready ? 'ok' : 'off'} interactive><div style={{ display: 'flex', justifyContent: 'space-between', gap: 10 }}><code style={{ color: 'var(--cyan-500)' }}>{p.provider}</code><HostedBadge tone={p.ready ? 'ok' : 'off'}><HostedStatusDot tone={p.ready ? 'ok' : 'off'} size={6} /> {p.ready ? 'ready' : 'needs login'}</HostedBadge></div><h2 style={{ marginTop: 14 }}>{p.label}</h2><p className="muted">{p.identity || 'No identity exposed'}</p><HostedButton variant="ghost" size="sm" onClick={() => { setForm((f) => ({ ...f, provider: p.provider, label: p.label, session: '' })); setModal('provider'); }}>{p.ready ? 'Replace session' : 'Connect'}</HostedButton></HostedCard>) : <div className="empty">No provider accounts</div>}</div></HostedCard>}
    {tab === 'requests' && <HostedCard pad={0} style={{ overflow: 'auto' }}><table><thead><tr><th>Time</th><th>Key</th><th>Capability</th><th>Status</th><th>Latency</th></tr></thead><tbody>{data.requests.length ? data.requests.map((r) => <tr key={r.id}><td className="mono">{r.createdAt || '—'}</td><td className="mono">{r.apiKeyId || '—'}</td><td>{r.capability || 'mcp'}</td><td><HostedBadge tone={Number(r.status) >= 400 ? 'out' : 'ok'}>{r.status}</HostedBadge></td><td className="mono">{r.latencyMs || 0}ms</td></tr>) : <tr><td colSpan="5" className="empty">No requests yet</td></tr>}</tbody></table></HostedCard>}
    {tab === 'audit' && <HostedCard pad={0} style={{ overflow: 'auto' }}><table><thead><tr><th>Time</th><th>Actor</th><th>Action</th><th>Target</th></tr></thead><tbody>{data.audit.length ? data.audit.map((r) => <tr key={r.id}><td className="mono">{r.createdAt || '—'}</td><td>{r.actor || '—'}</td><td>{r.action || '—'}</td><td>{r.target || '—'}</td></tr>) : <tr><td colSpan="4" className="empty">No administrative events</td></tr>}</tbody></table></HostedCard>}
    {tab === 'update' && <HostedCard accent="accent"><h2>{data.update?.status || 'Server current'}</h2><p className="muted">{data.update?.message || 'No update job running.'}</p><div style={{ display: 'flex', gap: 10, flexWrap: 'wrap' }}><HostedButton variant="primary" disabled={busy} onClick={async () => { if (!window.confirm('Drain requests and update this server now?')) return; setBusy(true); try { await hostedAdminJSON('/admin/update', { method: 'POST', body: { mode: 'now' } }); await refresh(); } catch (e) { setError(e.message); } finally { setBusy(false); } }}>Update server</HostedButton><HostedButton variant="secondary" disabled={busy} onClick={async () => { setBusy(true); try { await hostedAdminJSON('/admin/update', { method: 'POST', body: { mode: 'idle' } }); await refresh(); } catch (e) { setError(e.message); } finally { setBusy(false); } }}>Update when idle</HostedButton></div></HostedCard>}

    {modal === 'key' && <HostedAdminModal title="Create API key" onClose={() => setModal(null)}><form onSubmit={createKey} style={{ display: 'grid', gap: 16 }}><div className="form-grid"><HostedField label="KEY ID">{input('id')}</HostedField><HostedField label="DISPLAY NAME">{input('name')}</HostedField><HostedField label="REQUESTS / MINUTE">{input('rpm', 'number')}</HostedField><HostedField label="DAILY LIMIT">{input('dailyLimit', 'number')}</HostedField><HostedField label="MONTHLY LIMIT">{input('monthlyLimit', 'number')}</HostedField><HostedField label="CONCURRENCY">{input('concurrencyLimit', 'number')}</HostedField></div><fieldset className="scope-options"><legend>PERMISSIONS</legend>{HOSTED_SCOPE_INFO.map((scope) => <label className="scope-option" key={scope.id}><input type="checkbox" checked={form.scopes.includes(scope.id)} onChange={(e) => updateForm('scopes', e.target.checked ? [...form.scopes, scope.id] : form.scopes.filter((x) => x !== scope.id))} /><span><code>{scope.label}</code><small>{scope.description}</small></span></label>)}</fieldset><footer style={{ display: 'flex', justifyContent: 'flex-end', gap: 10 }}><HostedButton variant="secondary" type="button" onClick={() => setModal(null)}>Cancel</HostedButton><HostedButton variant="primary" type="submit" disabled={busy}>{busy ? 'Creating…' : 'Create key'}</HostedButton></footer></form></HostedAdminModal>}
    {modal === 'provider' && <HostedAdminModal title="Add provider" onClose={() => setModal(null)}><form onSubmit={addProvider} style={{ display: 'grid', gap: 16 }}><div className="form-grid"><HostedField label="PROVIDER"><select value={form.provider} onChange={(e) => updateForm('provider', e.target.value)}>{(window.FX.catalog || []).map((p) => <option key={p.id} value={p.id}>{p.label || p.id}</option>)}</select></HostedField><HostedField label="LABEL">{input('label')}</HostedField></div><HostedField label="SESSION JSON (OPTIONAL)" ><textarea rows="6" value={form.session} onChange={(e) => updateForm('session', e.target.value)} placeholder='Paste exported cookies/session JSON, or leave blank for browser login.' /></HostedField><footer style={{ display: 'flex', justifyContent: 'flex-end', gap: 10 }}><HostedButton variant="secondary" type="button" onClick={() => setModal(null)}>Cancel</HostedButton><HostedButton variant="primary" type="submit" disabled={busy}>{busy ? 'Saving…' : form.session.trim() ? 'Save session' : 'Start browser login'}</HostedButton></footer></form></HostedAdminModal>}
    {modal === 'secret' && <HostedAdminModal title="Copy this key now" onClose={() => setModal(null)}><p className="muted">The plaintext is never shown again.</p><div className="secret-row"><code>{secret}</code><HostedButton variant="secondary" size="sm" onClick={() => copy(secret)}>Copy</HostedButton></div><footer style={{ display: 'flex', justifyContent: 'flex-end', marginTop: 18 }}><HostedButton variant="primary" onClick={() => setModal(null)}>Done</HostedButton></footer></HostedAdminModal>}
    {modal === 'challenge' && <HostedAdminModal title="Finish browser login" onClose={() => setModal(null)}><p className="muted">Run this on the computer where the provider browser session is available:</p><div className="secret-row"><code>fetchira remote login {challenge?.challenge}</code><HostedButton variant="secondary" size="sm" onClick={() => copy(`fetchira remote login ${challenge?.challenge}`)}>Copy</HostedButton></div><p className="muted" style={{ marginTop: 14 }}>The challenge expires at {challenge?.expiresAt || challenge?.expires_at || '—'} and is single-use.</p><HostedButton variant="primary" onClick={() => { setModal(null); refresh(); }}>Close</HostedButton></HostedAdminModal>}
  </main>;
}

window.HostedAdminFull = HostedAdminFull;
