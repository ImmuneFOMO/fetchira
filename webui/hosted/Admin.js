(function () {
function ownKeys(e, r) { var t = Object.keys(e); if (Object.getOwnPropertySymbols) { var o = Object.getOwnPropertySymbols(e); r && (o = o.filter(function (r) { return Object.getOwnPropertyDescriptor(e, r).enumerable; })), t.push.apply(t, o); } return t; }
function _objectSpread(e) { for (var r = 1; r < arguments.length; r++) { var t = null != arguments[r] ? arguments[r] : {}; r % 2 ? ownKeys(Object(t), !0).forEach(function (r) { _defineProperty(e, r, t[r]); }) : Object.getOwnPropertyDescriptors ? Object.defineProperties(e, Object.getOwnPropertyDescriptors(t)) : ownKeys(Object(t)).forEach(function (r) { Object.defineProperty(e, r, Object.getOwnPropertyDescriptor(t, r)); }); } return e; }
function _defineProperty(e, r, t) { return (r = _toPropertyKey(r)) in e ? Object.defineProperty(e, r, { value: t, enumerable: !0, configurable: !0, writable: !0 }) : e[r] = t, e; }
function _toPropertyKey(t) { var i = _toPrimitive(t, "string"); return "symbol" == typeof i ? i : i + ""; }
function _toPrimitive(t, r) { if ("object" != typeof t || !t) return t; var e = t[Symbol.toPrimitive]; if (void 0 !== e) { var i = e.call(t, r || "default"); if ("object" != typeof i) return i; throw new TypeError("@@toPrimitive must return a primitive value."); } return ("string" === r ? String : Number)(t); }
const {
  Card: HostedCard,
  Button: HostedButton,
  Badge: HostedBadge,
  StatusDot: HostedStatusDot
} = window.FetchiraDesignSystem_6526df;
const HOSTED_SCOPE_INFO = [{
  id: 'mcp',
  label: 'mcp',
  description: 'Use Fetchira tools through the hosted endpoint.'
}, {
  id: 'usage:read',
  label: 'usage:read',
  description: 'View this key’s usage, limits and request history.'
}, {
  id: 'accounts:manage',
  label: 'accounts:manage',
  description: 'Add, replace or remove provider sessions.'
}, {
  id: 'server:update',
  label: 'server:update',
  description: 'Start a hosted server update and manage releases.'
}];
const hostedAdminCsrf = () => {
  var _document$cookie$spli;
  const prefix = 'fetchira_csrf=';
  return ((_document$cookie$spli = document.cookie.split(';').map(x => x.trim()).find(x => x.startsWith(prefix))) === null || _document$cookie$spli === void 0 ? void 0 : _document$cookie$spli.slice(prefix.length)) || '';
};
async function hostedAdminJSON(path, options = {}) {
  const headers = _objectSpread({
    accept: 'application/json'
  }, options.headers || {});
  if (options.body && typeof options.body !== 'string') {
    headers['content-type'] = 'application/json';
    options.body = JSON.stringify(options.body);
  }
  if (options.method && options.method !== 'GET') headers['x-csrf-token'] = hostedAdminCsrf();
  const response = await fetch(path, _objectSpread(_objectSpread({
    credentials: 'same-origin'
  }, options), {}, {
    headers
  }));
  let body = null;
  try {
    body = await response.json();
  } catch (_) {}
  if (!response.ok) throw new Error(body && body.error || body || `request failed (${response.status})`);
  return body;
}
function HostedAdminModal({
  title,
  children,
  onClose
}) {
  return /*#__PURE__*/React.createElement("div", {
    role: "presentation",
    onMouseDown: e => {
      if (e.target === e.currentTarget) onClose();
    },
    style: {
      position: 'fixed',
      inset: 0,
      zIndex: 80,
      display: 'grid',
      placeItems: 'center',
      padding: 20,
      background: 'rgba(4,5,8,.72)',
      backdropFilter: 'blur(4px)'
    }
  }, /*#__PURE__*/React.createElement("section", {
    role: "dialog",
    "aria-modal": "true",
    "aria-labelledby": "hosted-modal-title",
    style: {
      width: 'min(620px,100%)',
      maxHeight: 'calc(100vh - 40px)',
      overflow: 'auto',
      background: 'var(--surface-1)',
      border: '1px solid var(--border-strong)',
      borderRadius: 'var(--r-lg)',
      boxShadow: 'var(--elev-overlay)',
      padding: 24
    }
  }, /*#__PURE__*/React.createElement("header", {
    style: {
      display: 'flex',
      justifyContent: 'space-between',
      alignItems: 'center',
      gap: 16,
      marginBottom: 22,
      paddingBottom: 14,
      borderBottom: '1px solid var(--border-faint)'
    }
  }, /*#__PURE__*/React.createElement("h2", {
    id: "hosted-modal-title"
  }, title), /*#__PURE__*/React.createElement(HostedButton, {
    variant: "ghost",
    size: "sm",
    onClick: onClose,
    "aria-label": "Close"
  }, "\xD7")), children));
}
function HostedField({
  label,
  children
}) {
  return /*#__PURE__*/React.createElement("label", {
    style: {
      display: 'grid',
      gap: 7,
      color: 'var(--text-mid)',
      font: '600 11px var(--font-mono)'
    }
  }, label, children);
}
function HostedAdminFull({
  tab
}) {
  var _data$update, _data$update2;
  const [data, setData] = React.useState({
    keys: [],
    providers: [],
    requests: [],
    audit: [],
    update: null
  });
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState('');
  const [modal, setModal] = React.useState(null);
  const [secret, setSecret] = React.useState('');
  const [challenge, setChallenge] = React.useState(null);
  const [form, setForm] = React.useState({
    id: '',
    name: '',
    provider: 'chatgpt_web',
    label: '',
    session: '',
    scopes: ['mcp', 'usage:read'],
    rpm: 60,
    dailyLimit: 0,
    monthlyLimit: 0,
    concurrencyLimit: 4
  });
  const refresh = React.useCallback(async () => {
    setBusy(true);
    setError('');
    try {
      const [keys, providers, requests, audit, update] = await Promise.all([hostedAdminJSON('/admin/keys'), hostedAdminJSON('/admin/providers'), hostedAdminJSON('/admin/usage?limit=500'), hostedAdminJSON('/admin/audit?limit=500'), hostedAdminJSON('/admin/update')]);
      setData({
        keys: keys.keys || [],
        providers: providers.providers || [],
        requests: requests.rows || [],
        audit: audit.rows || [],
        update: update.job || update
      });
    } catch (e) {
      setError(e.message || 'Unable to load hosted administration');
    } finally {
      setBusy(false);
    }
  }, []);
  React.useEffect(() => {
    refresh();
  }, [refresh, tab]);
  const updateForm = (key, value) => setForm(f => _objectSpread(_objectSpread({}, f), {}, {
    [key]: value
  }));
  const createKey = async e => {
    e.preventDefault();
    setBusy(true);
    setError('');
    try {
      const result = await hostedAdminJSON('/admin/keys', {
        method: 'POST',
        body: {
          id: form.id.trim(),
          name: form.name.trim(),
          scopes: form.scopes,
          rpm: Number(form.rpm),
          daily_limit: Number(form.dailyLimit),
          monthly_limit: Number(form.monthlyLimit),
          concurrency_limit: Number(form.concurrencyLimit),
          expires_at: null
        }
      });
      setSecret(result.key);
      setModal('secret');
      await refresh();
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  };
  const revoke = async id => {
    if (!window.confirm(`Revoke API key ${id}? Existing clients stop immediately.`)) return;
    setBusy(true);
    setError('');
    try {
      await hostedAdminJSON(`/admin/keys/${encodeURIComponent(id)}/revoke`, {
        method: 'POST'
      });
      await refresh();
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  };
  const addProvider = async e => {
    e.preventDefault();
    setBusy(true);
    setError('');
    try {
      if (form.session.trim()) {
        await hostedAdminJSON('/admin/providers/session', {
          method: 'POST',
          body: {
            provider: form.provider,
            label: form.label.trim(),
            session: form.session.trim()
          }
        });
        setModal(null);
        await refresh();
        return;
      }
      const result = await hostedAdminJSON('/admin/login-challenges', {
        method: 'POST',
        body: {
          provider: form.provider,
          label: form.label.trim()
        }
      });
      setChallenge(result);
      setModal('challenge');
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  };
  const title = {
    keys: 'API keys',
    providers: 'Providers',
    requests: 'Requests',
    audit: 'Audit',
    update: 'Update'
  }[tab] || tab;
  const input = (key, type = 'text') => /*#__PURE__*/React.createElement("input", {
    type: type,
    value: form[key],
    onChange: e => updateForm(key, e.target.value),
    required: ['id', 'name', 'label'].includes(key)
  });
  const copy = value => {
    var _navigator$clipboard;
    return (_navigator$clipboard = navigator.clipboard) === null || _navigator$clipboard === void 0 ? void 0 : _navigator$clipboard.writeText(value);
  };
  return /*#__PURE__*/React.createElement("main", {
    style: {
      maxWidth: 'var(--container-max)',
      margin: '0 auto',
      padding: 20
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'end',
      justifyContent: 'space-between',
      gap: 16,
      marginBottom: 24
    }
  }, /*#__PURE__*/React.createElement("div", null, /*#__PURE__*/React.createElement("div", {
    style: {
      font: '11px var(--font-mono)',
      letterSpacing: '.12em',
      textTransform: 'uppercase',
      color: 'var(--text-lo)'
    }
  }, "Hosted administration"), /*#__PURE__*/React.createElement("h1", {
    style: {
      marginTop: 5
    }
  }, title)), /*#__PURE__*/React.createElement(HostedButton, {
    variant: "secondary",
    size: "sm",
    onClick: refresh,
    disabled: busy
  }, busy ? 'Loading…' : 'Refresh')), error && /*#__PURE__*/React.createElement("div", {
    role: "alert",
    style: {
      marginBottom: 14,
      padding: 10,
      border: '1px solid rgba(242,85,90,.35)',
      borderRadius: 'var(--r-sm)',
      color: 'var(--red-500)',
      font: '12px var(--font-mono)'
    }
  }, error), tab === 'keys' && /*#__PURE__*/React.createElement(HostedCard, {
    pad: 0,
    style: {
      overflow: 'hidden'
    }
  }, /*#__PURE__*/React.createElement("header", {
    style: {
      display: 'flex',
      justifyContent: 'space-between',
      alignItems: 'center',
      padding: 14,
      borderBottom: '1px solid var(--border-faint)'
    }
  }, /*#__PURE__*/React.createElement("div", null, /*#__PURE__*/React.createElement("h2", null, "Friend access"), /*#__PURE__*/React.createElement("p", {
    className: "muted",
    style: {
      margin: '4px 0 0'
    }
  }, "Scoped keys are shown once and stored hashed.")), /*#__PURE__*/React.createElement(HostedButton, {
    variant: "primary",
    size: "sm",
    onClick: () => {
      setError('');
      setModal('key');
    }
  }, "Create key")), /*#__PURE__*/React.createElement("div", {
    style: {
      overflow: 'auto'
    }
  }, /*#__PURE__*/React.createElement("table", null, /*#__PURE__*/React.createElement("thead", null, /*#__PURE__*/React.createElement("tr", null, /*#__PURE__*/React.createElement("th", null, "Name"), /*#__PURE__*/React.createElement("th", null, "Scopes"), /*#__PURE__*/React.createElement("th", null, "Limits"), /*#__PURE__*/React.createElement("th", null, "Status"), /*#__PURE__*/React.createElement("th", null))), /*#__PURE__*/React.createElement("tbody", null, data.keys.length ? data.keys.map(k => /*#__PURE__*/React.createElement("tr", {
    key: k.id
  }, /*#__PURE__*/React.createElement("td", null, /*#__PURE__*/React.createElement("strong", null, k.name), /*#__PURE__*/React.createElement("br", null), /*#__PURE__*/React.createElement("code", null, k.id)), /*#__PURE__*/React.createElement("td", null, (k.scopes || []).map(s => /*#__PURE__*/React.createElement(HostedBadge, {
    key: s,
    tone: "neutral",
    variant: "outline"
  }, s))), /*#__PURE__*/React.createElement("td", {
    className: "mono"
  }, k.rpm, " rpm \xB7 ", k.dailyLimit || '∞', " day", /*#__PURE__*/React.createElement("br", null), k.concurrencyLimit, " concurrent"), /*#__PURE__*/React.createElement("td", null, /*#__PURE__*/React.createElement(HostedBadge, {
    tone: k.revoked ? 'out' : 'ok'
  }, k.revoked ? 'revoked' : 'active')), /*#__PURE__*/React.createElement("td", null, !k.revoked && /*#__PURE__*/React.createElement(HostedButton, {
    variant: "danger",
    size: "sm",
    onClick: () => revoke(k.id),
    disabled: busy
  }, "Revoke")))) : /*#__PURE__*/React.createElement("tr", null, /*#__PURE__*/React.createElement("td", {
    colSpan: "5",
    className: "empty"
  }, "No API keys yet")))))), tab === 'providers' && /*#__PURE__*/React.createElement(HostedCard, {
    pad: 0,
    style: {
      overflow: 'hidden'
    }
  }, /*#__PURE__*/React.createElement("header", {
    style: {
      display: 'flex',
      justifyContent: 'space-between',
      alignItems: 'center',
      padding: 14,
      borderBottom: '1px solid var(--border-faint)'
    }
  }, /*#__PURE__*/React.createElement("div", null, /*#__PURE__*/React.createElement("h2", null, "Provider sessions"), /*#__PURE__*/React.createElement("p", {
    className: "muted",
    style: {
      margin: '4px 0 0'
    }
  }, "Credentials stay encrypted on the server.")), /*#__PURE__*/React.createElement(HostedButton, {
    variant: "primary",
    size: "sm",
    onClick: () => {
      setError('');
      setModal('provider');
    }
  }, "Add provider")), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'grid',
      gridTemplateColumns: 'repeat(auto-fit,minmax(280px,1fr))',
      gap: 14,
      padding: 16
    }
  }, data.providers.length ? data.providers.map(p => /*#__PURE__*/React.createElement(HostedCard, {
    key: p.label,
    accent: p.ready ? 'ok' : 'off',
    interactive: true
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      justifyContent: 'space-between',
      gap: 10
    }
  }, /*#__PURE__*/React.createElement("code", {
    style: {
      color: 'var(--cyan-500)'
    }
  }, p.provider), /*#__PURE__*/React.createElement(HostedBadge, {
    tone: p.ready ? 'ok' : 'off'
  }, /*#__PURE__*/React.createElement(HostedStatusDot, {
    tone: p.ready ? 'ok' : 'off',
    size: 6
  }), " ", p.ready ? 'ready' : 'needs login')), /*#__PURE__*/React.createElement("h2", {
    style: {
      marginTop: 14
    }
  }, p.label), /*#__PURE__*/React.createElement("p", {
    className: "muted"
  }, p.identity || 'No identity exposed'), /*#__PURE__*/React.createElement(HostedButton, {
    variant: "ghost",
    size: "sm",
    onClick: () => {
      setForm(f => _objectSpread(_objectSpread({}, f), {}, {
        provider: p.provider,
        label: p.label,
        session: ''
      }));
      setModal('provider');
    }
  }, p.ready ? 'Replace session' : 'Connect'))) : /*#__PURE__*/React.createElement("div", {
    className: "empty"
  }, "No provider accounts"))), tab === 'requests' && /*#__PURE__*/React.createElement(HostedCard, {
    pad: 0,
    style: {
      overflow: 'auto'
    }
  }, /*#__PURE__*/React.createElement("table", null, /*#__PURE__*/React.createElement("thead", null, /*#__PURE__*/React.createElement("tr", null, /*#__PURE__*/React.createElement("th", null, "Time"), /*#__PURE__*/React.createElement("th", null, "Key"), /*#__PURE__*/React.createElement("th", null, "Capability"), /*#__PURE__*/React.createElement("th", null, "Status"), /*#__PURE__*/React.createElement("th", null, "Latency"))), /*#__PURE__*/React.createElement("tbody", null, data.requests.length ? data.requests.map(r => /*#__PURE__*/React.createElement("tr", {
    key: r.id
  }, /*#__PURE__*/React.createElement("td", {
    className: "mono"
  }, r.createdAt || '—'), /*#__PURE__*/React.createElement("td", {
    className: "mono"
  }, r.apiKeyId || '—'), /*#__PURE__*/React.createElement("td", null, r.capability || 'mcp'), /*#__PURE__*/React.createElement("td", null, /*#__PURE__*/React.createElement(HostedBadge, {
    tone: Number(r.status) >= 400 ? 'out' : 'ok'
  }, r.status)), /*#__PURE__*/React.createElement("td", {
    className: "mono"
  }, r.latencyMs || 0, "ms"))) : /*#__PURE__*/React.createElement("tr", null, /*#__PURE__*/React.createElement("td", {
    colSpan: "5",
    className: "empty"
  }, "No requests yet"))))), tab === 'audit' && /*#__PURE__*/React.createElement(HostedCard, {
    pad: 0,
    style: {
      overflow: 'auto'
    }
  }, /*#__PURE__*/React.createElement("table", null, /*#__PURE__*/React.createElement("thead", null, /*#__PURE__*/React.createElement("tr", null, /*#__PURE__*/React.createElement("th", null, "Time"), /*#__PURE__*/React.createElement("th", null, "Actor"), /*#__PURE__*/React.createElement("th", null, "Action"), /*#__PURE__*/React.createElement("th", null, "Target"))), /*#__PURE__*/React.createElement("tbody", null, data.audit.length ? data.audit.map(r => /*#__PURE__*/React.createElement("tr", {
    key: r.id
  }, /*#__PURE__*/React.createElement("td", {
    className: "mono"
  }, r.createdAt || '—'), /*#__PURE__*/React.createElement("td", null, r.actor || '—'), /*#__PURE__*/React.createElement("td", null, r.action || '—'), /*#__PURE__*/React.createElement("td", null, r.target || '—'))) : /*#__PURE__*/React.createElement("tr", null, /*#__PURE__*/React.createElement("td", {
    colSpan: "4",
    className: "empty"
  }, "No administrative events"))))), tab === 'update' && /*#__PURE__*/React.createElement(HostedCard, {
    accent: "accent"
  }, /*#__PURE__*/React.createElement("h2", null, ((_data$update = data.update) === null || _data$update === void 0 ? void 0 : _data$update.status) || 'Server current'), /*#__PURE__*/React.createElement("p", {
    className: "muted"
  }, ((_data$update2 = data.update) === null || _data$update2 === void 0 ? void 0 : _data$update2.message) || 'No update job running.'), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      gap: 10,
      flexWrap: 'wrap'
    }
  }, /*#__PURE__*/React.createElement(HostedButton, {
    variant: "primary",
    disabled: busy,
    onClick: async () => {
      if (!window.confirm('Drain requests and update this server now?')) return;
      setBusy(true);
      try {
        await hostedAdminJSON('/admin/update', {
          method: 'POST',
          body: {
            mode: 'now'
          }
        });
        await refresh();
      } catch (e) {
        setError(e.message);
      } finally {
        setBusy(false);
      }
    }
  }, "Update server"), /*#__PURE__*/React.createElement(HostedButton, {
    variant: "secondary",
    disabled: busy,
    onClick: async () => {
      setBusy(true);
      try {
        await hostedAdminJSON('/admin/update', {
          method: 'POST',
          body: {
            mode: 'idle'
          }
        });
        await refresh();
      } catch (e) {
        setError(e.message);
      } finally {
        setBusy(false);
      }
    }
  }, "Update when idle"))), modal === 'key' && /*#__PURE__*/React.createElement(HostedAdminModal, {
    title: "Create API key",
    onClose: () => setModal(null)
  }, /*#__PURE__*/React.createElement("form", {
    onSubmit: createKey,
    style: {
      display: 'grid',
      gap: 16
    }
  }, /*#__PURE__*/React.createElement("div", {
    className: "form-grid"
  }, /*#__PURE__*/React.createElement(HostedField, {
    label: "KEY ID"
  }, input('id')), /*#__PURE__*/React.createElement(HostedField, {
    label: "DISPLAY NAME"
  }, input('name')), /*#__PURE__*/React.createElement(HostedField, {
    label: "REQUESTS / MINUTE"
  }, input('rpm', 'number')), /*#__PURE__*/React.createElement(HostedField, {
    label: "DAILY LIMIT"
  }, input('dailyLimit', 'number')), /*#__PURE__*/React.createElement(HostedField, {
    label: "MONTHLY LIMIT"
  }, input('monthlyLimit', 'number')), /*#__PURE__*/React.createElement(HostedField, {
    label: "CONCURRENCY"
  }, input('concurrencyLimit', 'number'))), /*#__PURE__*/React.createElement("fieldset", {
    className: "scope-options"
  }, /*#__PURE__*/React.createElement("legend", null, "PERMISSIONS"), HOSTED_SCOPE_INFO.map(scope => /*#__PURE__*/React.createElement("label", {
    className: "scope-option",
    key: scope.id
  }, /*#__PURE__*/React.createElement("input", {
    type: "checkbox",
    checked: form.scopes.includes(scope.id),
    onChange: e => updateForm('scopes', e.target.checked ? [...form.scopes, scope.id] : form.scopes.filter(x => x !== scope.id))
  }), /*#__PURE__*/React.createElement("span", null, /*#__PURE__*/React.createElement("code", null, scope.label), /*#__PURE__*/React.createElement("small", null, scope.description))))), /*#__PURE__*/React.createElement("footer", {
    style: {
      display: 'flex',
      justifyContent: 'flex-end',
      gap: 10
    }
  }, /*#__PURE__*/React.createElement(HostedButton, {
    variant: "secondary",
    type: "button",
    onClick: () => setModal(null)
  }, "Cancel"), /*#__PURE__*/React.createElement(HostedButton, {
    variant: "primary",
    type: "submit",
    disabled: busy
  }, busy ? 'Creating…' : 'Create key')))), modal === 'provider' && /*#__PURE__*/React.createElement(HostedAdminModal, {
    title: "Add provider",
    onClose: () => setModal(null)
  }, /*#__PURE__*/React.createElement("form", {
    onSubmit: addProvider,
    style: {
      display: 'grid',
      gap: 16
    }
  }, /*#__PURE__*/React.createElement("div", {
    className: "form-grid"
  }, /*#__PURE__*/React.createElement(HostedField, {
    label: "PROVIDER"
  }, /*#__PURE__*/React.createElement("select", {
    value: form.provider,
    onChange: e => updateForm('provider', e.target.value)
  }, (window.FX.catalog || []).map(p => /*#__PURE__*/React.createElement("option", {
    key: p.id,
    value: p.id
  }, p.label || p.id)))), /*#__PURE__*/React.createElement(HostedField, {
    label: "LABEL"
  }, input('label'))), /*#__PURE__*/React.createElement(HostedField, {
    label: "SESSION JSON (OPTIONAL)"
  }, /*#__PURE__*/React.createElement("textarea", {
    rows: "6",
    value: form.session,
    onChange: e => updateForm('session', e.target.value),
    placeholder: "Paste exported cookies/session JSON, or leave blank for browser login."
  })), /*#__PURE__*/React.createElement("footer", {
    style: {
      display: 'flex',
      justifyContent: 'flex-end',
      gap: 10
    }
  }, /*#__PURE__*/React.createElement(HostedButton, {
    variant: "secondary",
    type: "button",
    onClick: () => setModal(null)
  }, "Cancel"), /*#__PURE__*/React.createElement(HostedButton, {
    variant: "primary",
    type: "submit",
    disabled: busy
  }, busy ? 'Saving…' : form.session.trim() ? 'Save session' : 'Start browser login')))), modal === 'secret' && /*#__PURE__*/React.createElement(HostedAdminModal, {
    title: "Copy this key now",
    onClose: () => setModal(null)
  }, /*#__PURE__*/React.createElement("p", {
    className: "muted"
  }, "The plaintext is never shown again."), /*#__PURE__*/React.createElement("div", {
    className: "secret-row"
  }, /*#__PURE__*/React.createElement("code", null, secret), /*#__PURE__*/React.createElement(HostedButton, {
    variant: "secondary",
    size: "sm",
    onClick: () => copy(secret)
  }, "Copy")), /*#__PURE__*/React.createElement("footer", {
    style: {
      display: 'flex',
      justifyContent: 'flex-end',
      marginTop: 18
    }
  }, /*#__PURE__*/React.createElement(HostedButton, {
    variant: "primary",
    onClick: () => setModal(null)
  }, "Done"))), modal === 'challenge' && /*#__PURE__*/React.createElement(HostedAdminModal, {
    title: "Finish browser login",
    onClose: () => setModal(null)
  }, /*#__PURE__*/React.createElement("p", {
    className: "muted"
  }, "Run this on the computer where the provider browser session is available:"), /*#__PURE__*/React.createElement("div", {
    className: "secret-row"
  }, /*#__PURE__*/React.createElement("code", null, "fetchira remote login ", challenge === null || challenge === void 0 ? void 0 : challenge.challenge), /*#__PURE__*/React.createElement(HostedButton, {
    variant: "secondary",
    size: "sm",
    onClick: () => copy(`fetchira remote login ${challenge === null || challenge === void 0 ? void 0 : challenge.challenge}`)
  }, "Copy")), /*#__PURE__*/React.createElement("p", {
    className: "muted",
    style: {
      marginTop: 14
    }
  }, "The challenge expires at ", (challenge === null || challenge === void 0 ? void 0 : challenge.expiresAt) || (challenge === null || challenge === void 0 ? void 0 : challenge.expires_at) || '—', " and is single-use."), /*#__PURE__*/React.createElement(HostedButton, {
    variant: "primary",
    onClick: () => {
      setModal(null);
      refresh();
    }
  }, "Close")));
}
window.HostedAdminFull = HostedAdminFull;
})();
