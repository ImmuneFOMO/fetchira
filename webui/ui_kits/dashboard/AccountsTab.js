(function () {
function ownKeys(e, r) { var t = Object.keys(e); if (Object.getOwnPropertySymbols) { var o = Object.getOwnPropertySymbols(e); r && (o = o.filter(function (r) { return Object.getOwnPropertyDescriptor(e, r).enumerable; })), t.push.apply(t, o); } return t; }
function _objectSpread(e) { for (var r = 1; r < arguments.length; r++) { var t = null != arguments[r] ? arguments[r] : {}; r % 2 ? ownKeys(Object(t), !0).forEach(function (r) { _defineProperty(e, r, t[r]); }) : Object.getOwnPropertyDescriptors ? Object.defineProperties(e, Object.getOwnPropertyDescriptors(t)) : ownKeys(Object(t)).forEach(function (r) { Object.defineProperty(e, r, Object.getOwnPropertyDescriptor(t, r)); }); } return e; }
function _defineProperty(e, r, t) { return (r = _toPropertyKey(r)) in e ? Object.defineProperty(e, r, { value: t, enumerable: !0, configurable: !0, writable: !0 }) : e[r] = t, e; }
function _toPropertyKey(t) { var i = _toPrimitive(t, "string"); return "symbol" == typeof i ? i : i + ""; }
function _toPrimitive(t, r) { if ("object" != typeof t || !t) return t; var e = t[Symbol.toPrimitive]; if (void 0 !== e) { var i = e.call(t, r || "default"); if ("object" != typeof i) return i; throw new TypeError("@@toPrimitive must return a primitive value."); } return ("string" === r ? String : Number)(t); }
/* Accounts: management table. API keys never shown — masked chip only. */
const {
  Card,
  Badge,
  Button,
  QuotaMeter,
  StatusDot,
  Input
} = window.FetchiraDesignSystem_6526df;
function statusBadge(s) {
  if (s === 'exhausted') return /*#__PURE__*/React.createElement(Badge, {
    tone: "out",
    dot: true
  }, "exhausted");
  if (s === 'needs-login') return /*#__PURE__*/React.createElement(Badge, {
    tone: "off",
    dot: true
  }, "needs login");
  return /*#__PURE__*/React.createElement(Badge, {
    tone: "ok",
    dot: true
  }, "healthy");
}

// Subscription badge for a connected account: paid plans pop (cyan), free stays quiet (neutral).
// Only web providers that report a tier have one; it streams in with the live limits.
function planBadge(tier) {
  if (!tier) return null;
  return /*#__PURE__*/React.createElement(Badge, {
    tone: tier === 'free' ? 'neutral' : 'cyan',
    variant: "outline"
  }, tier);
}

// "user@example.com" -> "us****@ex****om"
function maskEmail(e) {
  const at = String(e).indexOf('@');
  if (at < 1) return e;
  const local = e.slice(0, at),
    domain = e.slice(at + 1);
  return `${local.slice(0, 2)}****@${domain.slice(0, 2)}****${domain.slice(-2)}`;
}

// Account email chip: masked by default, click to reveal (loopback-only data, but not shouted).
function EmailChip({
  email
}) {
  const [show, setShow] = React.useState(false);
  return /*#__PURE__*/React.createElement("button", {
    type: "button",
    onClick: e => {
      e.stopPropagation();
      setShow(s => !s);
    },
    title: "click to reveal",
    style: {
      border: 0,
      background: 'transparent',
      cursor: 'pointer',
      color: 'var(--text-lo)',
      padding: 0
    }
  }, show ? email : maskEmail(email));
}
function Th({
  children,
  style
}) {
  return /*#__PURE__*/React.createElement("th", {
    style: _objectSpread({
      textAlign: 'left',
      fontFamily: 'var(--font-mono)',
      fontSize: 10,
      fontWeight: 600,
      letterSpacing: '0.1em',
      textTransform: 'uppercase',
      color: 'var(--text-faint)',
      padding: '0 14px 10px'
    }, style)
  }, children);
}

// Attach a session captured elsewhere (any browser) to an existing web account — the headless path.
function PasteSessionModal({
  label,
  onClose
}) {
  const [val, setVal] = React.useState('');
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState(null);
  const save = async () => {
    if (busy || !val.trim()) return;
    setError(null);
    setBusy(true);
    try {
      await window.apiPost('/api/account/session', {
        label,
        session: val
      });
      onClose();
      if (window.fxRefresh) window.fxRefresh();
    } catch (e) {
      setError(String(e.message || e));
      setBusy(false);
    }
  };
  return /*#__PURE__*/React.createElement("div", {
    onClick: onClose,
    style: {
      position: 'fixed',
      inset: 0,
      zIndex: 50,
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'center',
      background: 'rgba(4,5,8,0.66)',
      backdropFilter: 'blur(3px)',
      padding: 20
    }
  }, /*#__PURE__*/React.createElement("div", {
    onClick: e => e.stopPropagation(),
    style: {
      width: 460,
      maxWidth: '100%',
      background: 'var(--surface-raised, #0e1016)',
      border: '1px solid var(--border-hairline)',
      borderRadius: 'var(--r-lg)',
      overflow: 'hidden'
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'space-between',
      padding: '16px 20px',
      borderBottom: '1px solid var(--border-hairline)'
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-display)',
      fontSize: 16,
      fontWeight: 600,
      color: 'var(--text-hi)'
    }
  }, "Paste session \xB7 ", /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--lime-500)'
    }
  }, label)), /*#__PURE__*/React.createElement("button", {
    "aria-label": "Close",
    onClick: onClose,
    style: {
      background: 'transparent',
      border: 'none',
      color: 'var(--text-lo)',
      cursor: 'pointer',
      fontSize: 18,
      padding: 4
    }
  }, "\u2715")), /*#__PURE__*/React.createElement("div", {
    style: {
      padding: 20,
      display: 'flex',
      flexDirection: 'column',
      gap: 10
    }
  }, /*#__PURE__*/React.createElement("textarea", {
    value: val,
    onChange: e => setVal(e.target.value),
    spellCheck: false,
    autoFocus: true,
    placeholder: '[{"name":"sso","value":"…","domain":".grok.com"}]',
    style: {
      width: '100%',
      minHeight: 120,
      resize: 'vertical',
      boxSizing: 'border-box',
      padding: '8px 10px',
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: 'var(--text-hi)',
      background: 'var(--surface-sunken, rgba(255,255,255,0.03))',
      border: '1px solid var(--border-hairline)',
      borderRadius: 'var(--r-sm)'
    }
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-ui)',
      fontSize: 12,
      color: 'var(--text-lo)'
    }
  }, "Cookie array or ", '{"cookies":[…]}', " exported from a logged-in browser."), error && /*#__PURE__*/React.createElement("div", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: 'var(--red-500)',
      background: 'var(--red-dim)',
      border: '1px solid rgba(242,85,90,0.3)',
      borderRadius: 'var(--r-sm)',
      padding: '8px 10px'
    }
  }, error)), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      gap: 8,
      padding: '14px 20px',
      borderTop: '1px solid var(--border-hairline)',
      justifyContent: 'flex-end'
    }
  }, /*#__PURE__*/React.createElement(Button, {
    variant: "ghost",
    onClick: onClose
  }, "Cancel"), /*#__PURE__*/React.createElement(Button, {
    variant: "primary",
    onClick: save,
    disabled: busy || !val.trim()
  }, busy ? 'Saving…' : 'Save session'))));
}

// Rename an account (label is its identity — the backend migrates the session/quota/proxy rows).
function RenameModal({
  label,
  onClose
}) {
  const [val, setVal] = React.useState(label);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState(null);
  const save = async () => {
    const next = val.trim();
    if (busy || !next) return;
    if (next === label) {
      onClose();
      return;
    }
    setError(null);
    setBusy(true);
    try {
      await window.apiPost('/api/account/rename', {
        label,
        new_label: next
      });
      onClose();
      if (window.fxRefresh) window.fxRefresh();
    } catch (e) {
      setError(String(e.message || e));
      setBusy(false);
    }
  };
  return /*#__PURE__*/React.createElement("div", {
    onClick: onClose,
    style: {
      position: 'fixed',
      inset: 0,
      zIndex: 50,
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'center',
      background: 'rgba(4,5,8,0.66)',
      backdropFilter: 'blur(3px)',
      padding: 20
    }
  }, /*#__PURE__*/React.createElement("div", {
    onClick: e => e.stopPropagation(),
    style: {
      width: 420,
      maxWidth: '100%',
      background: 'var(--surface-raised, #0e1016)',
      border: '1px solid var(--border-hairline)',
      borderRadius: 'var(--r-lg)',
      overflow: 'hidden'
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'space-between',
      padding: '16px 20px',
      borderBottom: '1px solid var(--border-hairline)'
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-display)',
      fontSize: 16,
      fontWeight: 600,
      color: 'var(--text-hi)'
    }
  }, "Rename \xB7 ", /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--lime-500)'
    }
  }, label)), /*#__PURE__*/React.createElement("button", {
    "aria-label": "Close",
    onClick: onClose,
    style: {
      background: 'transparent',
      border: 'none',
      color: 'var(--text-lo)',
      cursor: 'pointer',
      fontSize: 18,
      padding: 4
    }
  }, "\u2715")), /*#__PURE__*/React.createElement("div", {
    style: {
      padding: 20,
      display: 'flex',
      flexDirection: 'column',
      gap: 10
    }
  }, /*#__PURE__*/React.createElement(Input, {
    label: "New label",
    value: val,
    mono: true,
    onChange: e => setVal(e.target.value)
  }), error && /*#__PURE__*/React.createElement("div", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: 'var(--red-500)',
      background: 'var(--red-dim)',
      border: '1px solid rgba(242,85,90,0.3)',
      borderRadius: 'var(--r-sm)',
      padding: '8px 10px'
    }
  }, error)), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      gap: 8,
      padding: '14px 20px',
      borderTop: '1px solid var(--border-hairline)',
      justifyContent: 'flex-end'
    }
  }, /*#__PURE__*/React.createElement(Button, {
    variant: "ghost",
    onClick: onClose
  }, "Cancel"), /*#__PURE__*/React.createElement(Button, {
    variant: "primary",
    onClick: save,
    disabled: busy || !val.trim()
  }, busy ? 'Saving…' : 'Rename'))));
}

// Change an account's proxy: one-click Direct / Pool, or a specific URL under Custom. A pinned URL
// is shown masked (creds never reach the browser), so switching to a new custom proxy means typing
// it in full. The raw string is sent as-is; the server normalises + validates it.
function ProxyModal({
  label,
  current,
  onClose
}) {
  const initMode = current === 'pool' ? 'pool' : current === 'direct' ? 'direct' : 'custom';
  const [mode, setMode] = React.useState(initMode);
  const [url, setUrl] = React.useState('');
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState(null);
  const proxy = mode === 'direct' ? '' : mode === 'pool' ? 'pool' : url.trim();
  const save = async () => {
    if (busy) return;
    if (mode === 'custom' && !proxy) {
      setError('Enter a proxy URL');
      return;
    }
    setError(null);
    setBusy(true);
    try {
      await window.apiPost('/api/account/proxy', {
        label,
        proxy
      });
      onClose();
      if (window.fxRefresh) window.fxRefresh();
    } catch (e) {
      setError(String(e.message || e));
      setBusy(false);
    }
  };
  const seg = (val, text) => /*#__PURE__*/React.createElement("button", {
    onClick: () => setMode(val),
    style: {
      flex: 1,
      padding: '7px 10px',
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      fontWeight: 600,
      cursor: 'pointer',
      color: mode === val ? 'var(--text-hi)' : 'var(--text-lo)',
      background: mode === val ? 'var(--surface-2)' : 'transparent',
      border: '1px solid ' + (mode === val ? 'var(--lime-500)' : 'var(--border-hairline)'),
      borderRadius: 'var(--r-sm)'
    }
  }, text);
  return /*#__PURE__*/React.createElement("div", {
    onClick: onClose,
    style: {
      position: 'fixed',
      inset: 0,
      zIndex: 50,
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'center',
      background: 'rgba(4,5,8,0.66)',
      backdropFilter: 'blur(3px)',
      padding: 20
    }
  }, /*#__PURE__*/React.createElement("div", {
    onClick: e => e.stopPropagation(),
    style: {
      width: 440,
      maxWidth: '100%',
      background: 'var(--surface-raised, #0e1016)',
      border: '1px solid var(--border-hairline)',
      borderRadius: 'var(--r-lg)',
      overflow: 'hidden'
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'space-between',
      padding: '16px 20px',
      borderBottom: '1px solid var(--border-hairline)'
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-display)',
      fontSize: 16,
      fontWeight: 600,
      color: 'var(--text-hi)'
    }
  }, "Proxy \xB7 ", /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--lime-500)'
    }
  }, label)), /*#__PURE__*/React.createElement("button", {
    "aria-label": "Close",
    onClick: onClose,
    style: {
      background: 'transparent',
      border: 'none',
      color: 'var(--text-lo)',
      cursor: 'pointer',
      fontSize: 18,
      padding: 4
    }
  }, "\u2715")), /*#__PURE__*/React.createElement("div", {
    style: {
      padding: 20,
      display: 'flex',
      flexDirection: 'column',
      gap: 12
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      gap: 6
    }
  }, seg('direct', 'Direct'), seg('pool', 'Pool'), seg('custom', 'Custom')), mode === 'custom' && /*#__PURE__*/React.createElement(Input, {
    label: "Proxy URL",
    value: url,
    mono: true,
    autoFocus: true,
    placeholder: "ip:port:user:pass or http://user:password@host:port or user:password@host:port",
    onChange: e => setUrl(e.target.value),
    hint: initMode === 'custom' ? `Currently ${current} — type a new URL to change it.` : null
  }), mode === 'pool' && /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-ui)',
      fontSize: 12,
      color: 'var(--text-lo)'
    }
  }, "A sticky proxy is assigned from your pool on the next call."), mode === 'direct' && /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-ui)',
      fontSize: 12,
      color: 'var(--text-lo)'
    }
  }, "Connect directly \u2014 no proxy."), error && /*#__PURE__*/React.createElement("div", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: 'var(--red-500)',
      background: 'var(--red-dim)',
      border: '1px solid rgba(242,85,90,0.3)',
      borderRadius: 'var(--r-sm)',
      padding: '8px 10px'
    }
  }, error)), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      gap: 8,
      padding: '14px 20px',
      borderTop: '1px solid var(--border-hairline)',
      justifyContent: 'flex-end'
    }
  }, /*#__PURE__*/React.createElement(Button, {
    variant: "ghost",
    onClick: onClose
  }, "Cancel"), /*#__PURE__*/React.createElement(Button, {
    variant: "primary",
    onClick: save,
    disabled: busy || mode === 'custom' && !proxy
  }, busy ? 'Saving…' : 'Save proxy'))));
}

// Everything beyond Test/Login lives here so the action column stays one width for every row.
function RowMenu({
  r,
  onLogin,
  onError,
  disabled
}) {
  const [open, setOpen] = React.useState(false);
  const [pos, setPos] = React.useState({
    top: 0,
    right: 0
  });
  const [confirmRm, setConfirmRm] = React.useState(false);
  const [paste, setPaste] = React.useState(false);
  const [edit, setEdit] = React.useState(false);
  const [proxyEdit, setProxyEdit] = React.useState(false);
  const close = () => {
    setOpen(false);
    setConfirmRm(false);
  };
  const doRemove = async () => {
    if (!confirmRm) {
      setConfirmRm(true);
      return;
    }
    close();
    try {
      await window.apiPost('/api/account/remove', {
        label: r.label
      });
      if (window.fxRefresh) window.fxRefresh();
    } catch (e) {
      onError(String(e.message || e));
    }
  };
  const item = (label, onClick, danger) => /*#__PURE__*/React.createElement("button", {
    onClick: onClick,
    disabled: disabled,
    style: {
      display: 'block',
      width: '100%',
      textAlign: 'left',
      background: 'transparent',
      border: 'none',
      cursor: disabled ? 'not-allowed' : 'pointer',
      opacity: disabled ? 0.45 : 1,
      padding: '7px 12px',
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: danger ? 'var(--red-500)' : 'var(--text-mid)',
      whiteSpace: 'nowrap'
    },
    onMouseEnter: e => e.currentTarget.style.background = 'var(--surface-2)',
    onMouseLeave: e => e.currentTarget.style.background = 'transparent'
  }, label);
  return /*#__PURE__*/React.createElement("span", {
    style: {
      position: 'relative',
      display: 'inline-block'
    }
  }, /*#__PURE__*/React.createElement(Button, {
    size: "sm",
    variant: "ghost",
    "aria-label": `More actions for ${r.label}`,
    disabled: disabled,
    onClick: e => {
      const b = e.currentTarget.getBoundingClientRect();
      setPos({
        top: b.bottom + 4,
        right: window.innerWidth - b.right
      });
      setOpen(o => !o);
    },
    title: "more actions"
  }, "\u22EF"), open && /*#__PURE__*/React.createElement(React.Fragment, null, /*#__PURE__*/React.createElement("div", {
    onClick: close,
    style: {
      position: 'fixed',
      inset: 0,
      zIndex: 40
    }
  }), /*#__PURE__*/React.createElement("div", {
    style: {
      position: 'fixed',
      right: pos.right,
      top: pos.top,
      zIndex: 41,
      minWidth: 170,
      padding: '4px 0',
      background: 'var(--surface-raised, #0e1016)',
      border: '1px solid var(--border-hairline)',
      borderRadius: 'var(--r-sm)',
      boxShadow: '0 8px 24px rgba(0,0,0,0.5)'
    }
  }, item('Rename…', () => {
    close();
    setEdit(true);
  }), item('Proxy…', () => {
    close();
    setProxyEdit(true);
  }), r.web && item('Paste session…', () => {
    close();
    setPaste(true);
  }), r.web && item('Log in via Chrome', () => {
    close();
    onLogin('chrome');
  }), r.web && item('Log in via Firefox', () => {
    close();
    onLogin('firefox');
  }), item(confirmRm ? 'Really remove?' : 'Remove', doRemove, true))), paste && /*#__PURE__*/React.createElement(PasteSessionModal, {
    label: r.label,
    onClose: () => setPaste(false)
  }), edit && /*#__PURE__*/React.createElement(RenameModal, {
    label: r.label,
    onClose: () => setEdit(false)
  }), proxyEdit && /*#__PURE__*/React.createElement(ProxyModal, {
    label: r.label,
    current: r.proxy,
    onClose: () => setProxyEdit(false)
  }));
}
function RowActions({
  r
}) {
  const [busy, setBusy] = React.useState(null);
  const busyRef = React.useRef(false);
  const [test, setTest] = React.useState(null);
  const [hostedLogin, setHostedLogin] = React.useState(false);
  const needsLogin = r.status === 'needs-login';
  const spinner = /*#__PURE__*/React.createElement("span", {
    className: "fx-spin",
    "aria-hidden": "true",
    style: {
      width: 11,
      height: 11,
      flexShrink: 0,
      border: '1.5px solid currentColor',
      borderRightColor: 'transparent',
      borderRadius: '50%'
    }
  });
  const doTest = async () => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy('test');
    setTest(null);
    try {
      setTest(await window.apiPost('/api/account/test', {
        label: r.label
      }));
    } catch (e) {
      setTest({
        ok: false,
        error: String(e.message || e)
      });
    }
    busyRef.current = false;
    setBusy(null);
  };
  const doLogin = async browser => {
    if (window.fxHosted) {
      setHostedLogin(true);
      return;
    }
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy('login');
    setTest(null);
    try {
      await window.apiPost('/api/account/login', {
        label: r.label,
        browser
      });
      if (window.fxRefresh) window.fxRefresh();
    } catch (e) {
      setTest({
        ok: false,
        error: String(e.message || e)
      });
    }
    busyRef.current = false;
    setBusy(null);
  };
  return /*#__PURE__*/React.createElement("div", {
    onClick: e => e.stopPropagation(),
    "aria-busy": !!busy,
    style: {
      display: 'flex',
      gap: 6,
      justifyContent: 'flex-end',
      alignItems: 'center'
    }
  }, /*#__PURE__*/React.createElement("span", {
    title: busy === 'login' ? 'Complete sign-in in the browser window' : test ? test.error || '' : '',
    style: {
      width: 112,
      textAlign: 'right',
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: busy ? 'var(--lime-500)' : test ? test.ok ? 'var(--green-500)' : 'var(--red-500)' : 'transparent',
      overflow: 'hidden',
      textOverflow: 'ellipsis',
      whiteSpace: 'nowrap'
    }
  }, busy === 'login' ? 'finish in browser…' : busy === 'test' ? 'testing…' : test ? test.ok ? '✓ ' + test.latencyMs + 'ms' : '✕ failed' : '·'), /*#__PURE__*/React.createElement(Button, {
    size: "sm",
    variant: "ghost",
    onClick: doTest,
    disabled: !!busy
  }, busy === 'test' ? /*#__PURE__*/React.createElement(React.Fragment, null, spinner, " Testing") : 'Test'), r.web ? /*#__PURE__*/React.createElement(Button, {
    size: "sm",
    variant: needsLogin ? 'primary' : 'secondary',
    onClick: () => doLogin('chrome'),
    disabled: !!busy
  }, busy === 'login' ? /*#__PURE__*/React.createElement(React.Fragment, null, spinner, " Waiting\u2026") : needsLogin ? 'Login' : 'Re-login') : /*#__PURE__*/React.createElement("span", {
    style: {
      width: 62
    }
  }), /*#__PURE__*/React.createElement(RowMenu, {
    r: r,
    onLogin: doLogin,
    onError: e => setTest({
      ok: false,
      error: e
    }),
    disabled: !!busy
  }), hostedLogin && /*#__PURE__*/React.createElement(window.AddAccountModal, {
    initialProvider: r.provider,
    initialLabel: r.label,
    loginOnly: true,
    onClose: () => {
      setHostedLogin(false);
      if (window.fxRefresh) window.fxRefresh();
    }
  }));
}
function fmtResetIn(resetAfter, windowSecs) {
  if (resetAfter) {
    const d = new Date(resetAfter);
    if (isNaN(d.getTime())) return null;
    const s = Math.max(0, Math.round((d.getTime() - Date.now()) / 1000));
    if (s < 60) return 'reset in ' + s + 's';
    if (s < 3600) return 'reset in ' + Math.round(s / 60) + 'm';
    if (s < 86400) {
      const h = Math.floor(s / 3600);
      const m = Math.round(s % 3600 / 60);
      return 'reset in ' + (m ? h + 'h ' + m + 'm' : h + 'h');
    }
    return 'reset in ' + Math.round(s / 86400) + 'd';
  }
  if (!windowSecs) return null;
  if (windowSecs % 3600 === 0) return 'every ' + windowSecs / 3600 + 'h';
  if (windowSecs % 60 === 0) return 'every ' + windowSecs / 60 + 'm';
  return 'every ' + windowSecs + 's';
}

// The variable-length detail — tier, per-feature limits, the model catalog — lives in a drawer
// under the row, so every collapsed row keeps the same height.
function LimitChips({
  limits
}) {
  const chip = (key, label, value, reset, locked) => /*#__PURE__*/React.createElement("span", {
    key: key,
    title: reset || (locked ? 'locked on this tier' : ''),
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 10,
      color: locked ? 'var(--text-faint)' : 'var(--text-lo)',
      border: '1px solid var(--border-faint)',
      borderRadius: 4,
      padding: '1px 5px',
      opacity: locked ? 0.65 : 1
    }
  }, label, value != null ? /*#__PURE__*/React.createElement(React.Fragment, null, " ", /*#__PURE__*/React.createElement("b", {
    style: {
      color: locked ? 'var(--text-faint)' : 'var(--text-hi)'
    }
  }, value)) : null, reset && !locked ? /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--text-faint)'
    }
  }, " \xB7 ", reset) : null);
  const featVal = f => {
    if (f.remaining == null) return null;
    return f.total != null ? f.remaining + '/' + f.total : f.remaining;
  };
  const modelVal = m => {
    if (m.locked) return '0/0';
    if (m.total != null && m.remaining != null) return m.remaining + '/' + m.total;
    if (m.remaining != null) return m.remaining;
    return null;
  };
  return /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      flexWrap: 'wrap',
      gap: 4
    }
  }, limits.tier && /*#__PURE__*/React.createElement(Badge, {
    tone: "cyan",
    variant: "outline"
  }, limits.tier), (limits.features || []).filter(f => f.remaining != null).map(f => chip(f.feature, f.feature, featVal(f), fmtResetIn(f.resetAfter, f.windowSecs), false)), (limits.models || []).map(m => chip(m.id, m.name + (m.levels && m.levels.length ? ' ·' + m.levels.join('/') : ''), modelVal(m), fmtResetIn(m.resetAfter, m.windowSecs), m.locked)));
}

// One fixed-height row; the tier/limits/models detail expands in a drawer row underneath so
// nothing in the table jumps as live limits stream in per account.
function AccountRow({
  r
}) {
  const [open, setOpen] = React.useState(false);
  const needsLogin = r.status === 'needs-login';
  const spinner = text => /*#__PURE__*/React.createElement("span", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 6,
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: 'var(--text-faint)'
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      width: 10,
      height: 10,
      border: '1.5px solid var(--border-hairline)',
      borderTopColor: 'var(--lime-500)',
      borderRadius: '50%',
      display: 'inline-block',
      animation: 'fx-spin 0.8s linear infinite'
    }
  }), text);
  const hasDetail = !!r.limits || r.web && r.loggedIn;
  const td = {
    padding: '11px 14px',
    whiteSpace: 'nowrap'
  };
  return /*#__PURE__*/React.createElement(React.Fragment, null, /*#__PURE__*/React.createElement("tr", {
    style: {
      borderTop: '1px solid var(--border-faint)',
      height: 58,
      cursor: hasDetail ? 'pointer' : 'default'
    },
    onClick: hasDetail ? () => setOpen(o => !o) : undefined,
    onMouseEnter: e => e.currentTarget.style.background = 'var(--surface-2)',
    onMouseLeave: e => e.currentTarget.style.background = 'transparent'
  }, /*#__PURE__*/React.createElement("td", {
    style: {
      width: 8,
      padding: 0
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      display: 'block',
      width: 3,
      height: 34,
      marginLeft: 6,
      borderRadius: 2,
      background: r.status === 'exhausted' ? 'var(--red-500)' : needsLogin ? 'var(--grey-500)' : 'var(--green-500)'
    }
  })), /*#__PURE__*/React.createElement("td", {
    style: td
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 6
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      width: 10,
      flexShrink: 0,
      fontFamily: 'var(--font-mono)',
      fontSize: 10,
      color: 'var(--text-faint)'
    }
  }, hasDetail ? open ? '▾' : '▸' : ''), /*#__PURE__*/React.createElement("div", {
    style: {
      minWidth: 0
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 6,
      minWidth: 0
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 13,
      color: 'var(--text-hi)',
      fontWeight: 600
    }
  }, r.label), planBadge(r.limits && r.limits.tier)), /*#__PURE__*/React.createElement("div", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: 'var(--text-faint)',
      overflow: 'hidden',
      textOverflow: 'ellipsis'
    }
  }, r.provider, r.email ? /*#__PURE__*/React.createElement("span", null, " \xB7 ", /*#__PURE__*/React.createElement(EmailChip, {
    email: r.email
  })) : null)))), /*#__PURE__*/React.createElement("td", {
    style: _objectSpread(_objectSpread({}, td), {}, {
      width: 220
    })
  }, r.pending ? spinner('loading…') : r.web && !r.limits ? /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: 'var(--text-faint)'
    }
  }, "\u2014") : /*#__PURE__*/React.createElement(React.Fragment, null, /*#__PURE__*/React.createElement(QuotaMeter, {
    used: r.used,
    quota: r.quota,
    variant: "bar",
    size: "sm",
    showValues: false,
    state: needsLogin ? 'off' : undefined,
    style: {
      marginBottom: 4
    }
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: 'var(--text-lo)'
    }
  }, needsLogin ? '—' : (r.quota - r.used).toLocaleString(), " ", /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--text-faint)'
    }
  }, "/ ", r.quota.toLocaleString())))), /*#__PURE__*/React.createElement("td", {
    style: td
  }, /*#__PURE__*/React.createElement(Badge, {
    tone: "neutral",
    variant: "outline",
    uppercase: true
  }, r.resetWindow)), /*#__PURE__*/React.createElement("td", {
    style: _objectSpread(_objectSpread({}, td), {}, {
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: r.proxy === 'direct' ? 'var(--text-faint)' : 'var(--text-mid)'
    })
  }, r.proxy === 'pool' ? /*#__PURE__*/React.createElement(Badge, {
    tone: "cyan",
    variant: "outline"
  }, "pool") : r.proxy), /*#__PURE__*/React.createElement("td", {
    style: td
  }, r.key ? /*#__PURE__*/React.createElement(Badge, {
    tone: "ok",
    variant: "outline"
  }, "\u2022\u2022\u2022\u2022 key set") : r.web ? /*#__PURE__*/React.createElement(Badge, {
    tone: r.loggedIn ? 'cyan' : 'off',
    variant: "outline"
  }, r.loggedIn ? 'session ✓' : 'no session') : /*#__PURE__*/React.createElement(Badge, {
    tone: "off",
    variant: "outline"
  }, "no key")), /*#__PURE__*/React.createElement("td", {
    style: td
  }, statusBadge(r.status)), /*#__PURE__*/React.createElement("td", {
    style: td
  }, /*#__PURE__*/React.createElement(RowActions, {
    r: r
  }))), open && /*#__PURE__*/React.createElement("tr", {
    style: {
      background: 'var(--surface-inset)'
    }
  }, /*#__PURE__*/React.createElement("td", null), /*#__PURE__*/React.createElement("td", {
    colSpan: 7,
    style: {
      padding: '10px 14px 14px 30px'
    }
  }, r.pending ? spinner('loading live limits…') : r.limits ? /*#__PURE__*/React.createElement(LimitChips, {
    limits: r.limits
  }) : /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: 'var(--text-faint)'
    }
  }, "live limits unavailable \xB7 account is still connected"))));
}
function AccountsTab({
  onAdd
}) {
  const rows = window.FX.accounts;
  return /*#__PURE__*/React.createElement(Card, {
    pad: 0,
    style: {
      overflow: 'hidden'
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'space-between',
      padding: '14px 16px',
      borderBottom: '1px solid var(--border-hairline)'
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'baseline',
      gap: 10
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-display)',
      fontSize: 16,
      fontWeight: 600,
      color: 'var(--text-hi)'
    }
  }, "Accounts"), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: 'var(--text-lo)'
    }
  }, rows.length, " configured")), /*#__PURE__*/React.createElement(Button, {
    variant: "primary",
    size: "sm",
    iconLeft: /*#__PURE__*/React.createElement("span", {
      style: {
        fontFamily: 'var(--font-mono)',
        fontWeight: 700
      }
    }, "+"),
    onClick: onAdd
  }, "Add account")), /*#__PURE__*/React.createElement("div", {
    style: {
      overflowX: 'auto'
    }
  }, /*#__PURE__*/React.createElement("table", {
    style: {
      width: '100%',
      borderCollapse: 'collapse',
      minWidth: 880
    }
  }, /*#__PURE__*/React.createElement("thead", null, /*#__PURE__*/React.createElement("tr", {
    style: {
      background: 'var(--surface-inset)'
    }
  }, /*#__PURE__*/React.createElement("th", {
    style: {
      padding: '10px 0'
    }
  }), /*#__PURE__*/React.createElement(Th, {
    style: {
      paddingTop: 10
    }
  }, "Account"), /*#__PURE__*/React.createElement(Th, {
    style: {
      paddingTop: 10,
      width: 220
    }
  }, "Quota"), /*#__PURE__*/React.createElement(Th, {
    style: {
      paddingTop: 10
    }
  }, "Reset"), /*#__PURE__*/React.createElement(Th, {
    style: {
      paddingTop: 10
    }
  }, "Proxy"), /*#__PURE__*/React.createElement(Th, {
    style: {
      paddingTop: 10
    }
  }, "Key"), /*#__PURE__*/React.createElement(Th, {
    style: {
      paddingTop: 10
    }
  }, "Status"), /*#__PURE__*/React.createElement(Th, {
    style: {
      paddingTop: 10,
      textAlign: 'right'
    }
  }, "Actions"))), /*#__PURE__*/React.createElement("tbody", null, rows.map(r => /*#__PURE__*/React.createElement(AccountRow, {
    key: r.label,
    r: r
  }))))));
}
window.AccountsTab = AccountsTab;
})();
