/* Add-account modal + guided browser-login flow.
   Key providers → POST the key. Web providers → POST add (the server opens Chrome, you sign
   in, it captures the session). On success the dashboard refreshes. */
const { Card, Button, Input, Select, Badge, StatusDot } = window.FetchiraDesignSystem_6526df;

const PROVIDER_CATALOG = [
  { id: 'serper', kind: 'key', note: 'Web search API' },
  { id: 'tavily', kind: 'key', note: 'Search + extract API' },
  { id: 'exa', kind: 'key', note: 'Neural search API' },
  { id: 'parallel', kind: 'key', note: 'Search API' },
  { id: 'jina', kind: 'key', note: 'Reader · URL → markdown' },
  { id: 'firecrawl', kind: 'key', note: 'Crawl + scrape API' },
  { id: 'steel', kind: 'key', note: 'Headless browser sessions' },
  { id: 'perplexity_web', kind: 'web', note: 'Browser session · search + deep research' },
  { id: 'gemini_web', kind: 'web', note: 'Browser session · search + #dr' },
  { id: 'grok_web', kind: 'web', note: 'Browser session · search + #dr' },
];

function Overlay({ children, onClose }) {
  return (
    <div onClick={onClose} style={{
      position: 'fixed', inset: 0, zIndex: 50, display: 'flex', alignItems: 'center', justifyContent: 'center',
      background: 'rgba(4,5,8,0.66)', backdropFilter: 'blur(3px)', padding: 20,
      animation: 'fx-log-in var(--dur-mid) var(--ease-out)',
    }}>
      <div onClick={(e) => e.stopPropagation()} style={{ width: 460, maxWidth: '100%' }}>{children}</div>
    </div>
  );
}

function Field({ label, children }) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
      <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, letterSpacing: '0.04em', textTransform: 'uppercase', color: 'var(--text-lo)' }}>{label}</span>
      {children}
    </div>
  );
}

function AddAccountModal({ onClose }) {
  const [providerId, setProviderId] = React.useState('serper');
  const [label, setLabel] = React.useState('');
  const [apiKey, setApiKey] = React.useState('');
  const [proxy, setProxy] = React.useState('');
  const [touched, setTouched] = React.useState(false);
  const [phase, setPhase] = React.useState('form'); // form | logging-in | success
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState(null);
  const [addedLabel, setAddedLabel] = React.useState('');

  const provider = PROVIDER_CATALOG.find((p) => p.id === providerId);
  const isWeb = provider.kind === 'web';
  const keyMissing = !isWeb && apiKey.trim().length < 8;

  const submitKey = async () => {
    setTouched(true);
    if (keyMissing || !label.trim() || busy) return;
    setError(null); setBusy(true);
    try {
      const res = await window.apiPost('/api/account/add', { provider: providerId, label: label.trim(), key: apiKey, proxy: proxy.trim() });
      setAddedLabel((res && res.label) || label.trim());
      setPhase('success');
      if (window.fxRefresh) window.fxRefresh();
    } catch (e) { setError(String(e.message || e)); }
    setBusy(false);
  };

  const startLogin = async () => {
    if (busy) return;
    setError(null); setPhase('logging-in');
    try {
      const res = await window.apiPost('/api/account/add', { provider: providerId, label: label.trim(), proxy: proxy.trim() });
      setAddedLabel((res && res.label) || (label.trim() || provider.id));
      setPhase('success');
      if (window.fxRefresh) window.fxRefresh();
    } catch (e) { setError(String(e.message || e)); setPhase('form'); }
  };

  // ---- Success ----
  if (phase === 'success') {
    return (
      <Overlay onClose={onClose}>
        <Card raised pad={0} style={{ borderRadius: 'var(--r-lg)' }}>
          <div style={{ padding: '36px 28px', textAlign: 'center' }}>
            <div style={{ width: 52, height: 52, margin: '0 auto 16px', borderRadius: '50%', background: 'var(--green-dim)', border: '1px solid rgba(70,209,122,0.4)', display: 'flex', alignItems: 'center', justifyContent: 'center', color: 'var(--green-500)', fontSize: 24 }}>✓</div>
            <div style={{ fontFamily: 'var(--font-display)', fontSize: 19, fontWeight: 600, color: 'var(--text-hi)', marginBottom: 6 }}>Account added</div>
            <div style={{ fontFamily: 'var(--font-mono)', fontSize: 13, color: 'var(--text-mid)' }}>
              <span style={{ color: 'var(--lime-500)' }}>{addedLabel}</span> is live in the router rotation.
            </div>
          </div>
          <div style={{ display: 'flex', gap: 8, padding: '14px 20px', borderTop: '1px solid var(--border-hairline)', justifyContent: 'flex-end' }}>
            <Button variant="primary" onClick={onClose}>Done</Button>
          </div>
        </Card>
      </Overlay>
    );
  }

  // ---- Guided login (web providers — real Chrome capture happens server-side) ----
  if (phase === 'logging-in') {
    return (
      <Overlay onClose={() => {}}>
        <Card raised pad={0} style={{ borderRadius: 'var(--r-lg)' }}>
          <div style={{ padding: '28px' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 16 }}>
              <span style={{ width: 16, height: 16, borderRadius: '50%', border: '2px solid var(--lime-dim)', borderTopColor: 'var(--lime-500)', display: 'inline-block', animation: 'fx-spin 0.8s linear infinite' }} />
              <span style={{ fontFamily: 'var(--font-display)', fontSize: 16, fontWeight: 600, color: 'var(--text-hi)' }}>Guided login</span>
            </div>
            <div style={{ fontFamily: 'var(--font-mono)', fontSize: 13, color: 'var(--text-mid)', lineHeight: 1.5 }}>
              A Chrome window is opening for <span style={{ color: 'var(--lime-500)' }}>{provider.id}</span>. Sign in there — fetchira captures the session automatically and this closes when it's done.
            </div>
          </div>
        </Card>
      </Overlay>
    );
  }

  // ---- Form ----
  return (
    <Overlay onClose={onClose}>
      <Card raised pad={0} style={{ borderRadius: 'var(--r-lg)' }}>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '16px 20px', borderBottom: '1px solid var(--border-hairline)' }}>
          <span style={{ fontFamily: 'var(--font-display)', fontSize: 17, fontWeight: 600, color: 'var(--text-hi)', letterSpacing: '-0.01em' }}>Add account</span>
          <button onClick={onClose} style={{ background: 'transparent', border: 'none', color: 'var(--text-lo)', cursor: 'pointer', fontSize: 18, lineHeight: 1, padding: 4 }}>✕</button>
        </div>

        <div style={{ padding: 20, display: 'flex', flexDirection: 'column', gap: 16 }}>
          <Field label="Provider">
            <Select value={providerId} onChange={(e) => { setProviderId(e.target.value); setTouched(false); setError(null); }}>
              {PROVIDER_CATALOG.map((p) => <option key={p.id} value={p.id}>{p.id}</option>)}
            </Select>
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 2 }}>
              <Badge tone={isWeb ? 'cyan' : 'accent'} variant="outline">{isWeb ? 'browser login' : 'API key'}</Badge>
              <span style={{ fontFamily: 'var(--font-ui)', fontSize: 12, color: 'var(--text-lo)' }}>{provider.note}</span>
            </div>
          </Field>

          <Input label="Label" placeholder={`${provider.id}-1`} value={label} mono
            onChange={(e) => setLabel(e.target.value)}
            invalid={touched && !label.trim()} hint={touched && !label.trim() ? 'Give this account a label' : null} />

          {!isWeb ? (
            <Input label="API key" placeholder="paste secret key" value={apiKey} mono
              type="password" onChange={(e) => setApiKey(e.target.value)}
              invalid={touched && keyMissing}
              hint={touched && keyMissing ? 'Enter a valid API key' : 'Stored locally · never displayed again'} />
          ) : (
            <Field label="Authentication">
              <Button variant="secondary" onClick={startLogin} style={{ width: '100%', justifyContent: 'center' }}
                iconLeft={<span style={{ fontSize: 13 }}>◧</span>}>Log in with browser</Button>
              <span style={{ fontFamily: 'var(--font-ui)', fontSize: 12, color: 'var(--text-lo)' }}>Opens Chrome so you can sign in. The session is captured locally — no password is stored.</span>
            </Field>
          )}

          <Input label="Proxy · optional" placeholder="pool, direct, or http://host:port" value={proxy} mono
            onChange={(e) => setProxy(e.target.value)} hint="Leave blank for a direct connection" />

          {error && <div style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--red-500)', background: 'var(--red-dim)', border: '1px solid rgba(242,85,90,0.3)', borderRadius: 'var(--r-sm)', padding: '8px 10px' }}>{error}</div>}
        </div>

        <div style={{ display: 'flex', gap: 8, padding: '14px 20px', borderTop: '1px solid var(--border-hairline)', justifyContent: 'flex-end' }}>
          <Button variant="ghost" onClick={onClose}>Cancel</Button>
          {!isWeb && <Button variant="primary" onClick={submitKey}>{busy ? 'Adding…' : 'Add account'}</Button>}
        </div>
      </Card>
    </Overlay>
  );
}

window.AddAccountModal = AddAccountModal;
