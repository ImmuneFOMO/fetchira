(function () {
function ownKeys(e, r) { var t = Object.keys(e); if (Object.getOwnPropertySymbols) { var o = Object.getOwnPropertySymbols(e); r && (o = o.filter(function (r) { return Object.getOwnPropertyDescriptor(e, r).enumerable; })), t.push.apply(t, o); } return t; }
function _objectSpread(e) { for (var r = 1; r < arguments.length; r++) { var t = null != arguments[r] ? arguments[r] : {}; r % 2 ? ownKeys(Object(t), !0).forEach(function (r) { _defineProperty(e, r, t[r]); }) : Object.getOwnPropertyDescriptors ? Object.defineProperties(e, Object.getOwnPropertyDescriptors(t)) : ownKeys(Object(t)).forEach(function (r) { Object.defineProperty(e, r, Object.getOwnPropertyDescriptor(t, r)); }); } return e; }
function _defineProperty(e, r, t) { return (r = _toPropertyKey(r)) in e ? Object.defineProperty(e, r, { value: t, enumerable: !0, configurable: !0, writable: !0 }) : e[r] = t, e; }
function _toPropertyKey(t) { var i = _toPrimitive(t, "string"); return "symbol" == typeof i ? i : i + ""; }
function _toPrimitive(t, r) { if ("object" != typeof t || !t) return t; var e = t[Symbol.toPrimitive]; if (void 0 !== e) { var i = e.call(t, r || "default"); if ("object" != typeof i) return i; throw new TypeError("@@toPrimitive must return a primitive value."); } return ("string" === r ? String : Number)(t); }
/* Dismissible getting-started checklist for the local dashboard; hosted uses the key bridge below. */
const {
  Card,
  Button,
  Badge,
  StatusDot
} = window.FetchiraDesignSystem_6526df;
function apiGet(path) {
  return (window.apiGet ? window.apiGet(path) : fetch(path, {
    headers: {
      'x-fetchira-token': window.FX_TOKEN
    }
  }).then(r => r.ok ? r.json() : null)).catch(() => null);
}
const SKILL_VARIANTS = [{
  value: 'both',
  label: 'MCP + CLI',
  hint: 'Use MCP when it is available; fall back to the fetchira command.'
}, {
  value: 'mcp',
  label: 'MCP only',
  hint: 'Install the agent instructions for MCP tools only.'
}, {
  value: 'cli',
  label: 'CLI only',
  hint: 'Use the fetchira command from the shell.'
}, {
  value: 'skip',
  label: 'Skip skill',
  hint: 'Register selected MCP targets without installing an agent skill.'
}];

// Detect coding tools, preselect the not-yet-registered ones, register on click.
// Shared by the onboarding step and the checklist modal.
function InstallTargets({
  onDone
}) {
  const [targets, setTargets] = React.useState(null);
  const [picked, setPicked] = React.useState({});
  const [skill, setSkill] = React.useState('both');
  const [installedSkill, setInstalledSkill] = React.useState(null);
  const [busy, setBusy] = React.useState(false);
  const [results, setResults] = React.useState(null);
  const [failed, setFailed] = React.useState(false);
  const loadTargets = () => {
    setFailed(false);
    apiGet('/api/install/targets').then(d => {
      if (!d) {
        setFailed(true);
        return;
      }
      const ts = d.targets || [];
      setTargets(ts);
      const installed = Array.isArray(d.skills) ? d.skills : [d.skill];
      const current = ['both', 'mcp', 'cli', 'skip'].find(value => installed.includes(value));
      if (current) setInstalledSkill(current);
      const pre = {};
      ts.forEach(t => {
        if (t.present && !t.installed) pre[t.name] = true;
      });
      setPicked(pre);
    });
  };
  React.useEffect(loadTargets, []);
  const install = async () => {
    const names = Object.keys(picked).filter(n => picked[n]);
    if (busy || skill === 'skip' && !names.length) return;
    setBusy(true);
    setResults(null);
    try {
      const data = await window.apiPost('/api/install', {
        targets: names,
        skill
      });
      setResults(data.results || []);
      if (onDone && (data.results || []).every(r => r.ok)) onDone();
    } catch (e) {
      setResults([{
        name: 'error',
        ok: false,
        msg: String(e.message || e)
      }]);
    } finally {
      setBusy(false);
    }
  };
  const retry = () => {
    if (!busy) install();
  };
  const failedResult = results && results.some(r => !r.ok);
  return /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      flexDirection: 'column',
      gap: 10
    }
  }, failed ? /*#__PURE__*/React.createElement(React.Fragment, null, /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: 'var(--red-500)'
    }
  }, "couldn't reach the server"), /*#__PURE__*/React.createElement(Button, {
    variant: "ghost",
    onClick: loadTargets,
    style: {
      alignSelf: 'flex-start'
    }
  }, "Try again")) : !targets ? /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: 'var(--text-faint)'
    }
  }, "detecting tools\u2026") : results ? /*#__PURE__*/React.createElement(React.Fragment, null, results.map(r => /*#__PURE__*/React.createElement("div", {
    key: r.name,
    style: {
      display: 'flex',
      flexWrap: 'wrap',
      alignItems: 'baseline',
      gap: 8,
      fontFamily: 'var(--font-mono)',
      fontSize: 12
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      color: r.ok ? 'var(--green-500)' : 'var(--red-500)'
    }
  }, r.ok ? '✓' : '✗'), /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--text-hi)',
      width: 120,
      flexShrink: 0
    }
  }, r.name), /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--text-mid)',
      overflowWrap: 'anywhere',
      minWidth: 0
    }
  }, r.msg))), failedResult ? /*#__PURE__*/React.createElement(Button, {
    variant: "ghost",
    onClick: retry,
    style: {
      alignSelf: 'flex-start'
    }
  }, "Try again") : /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-ui)',
      fontSize: 12,
      color: 'var(--text-lo)'
    }
  }, "Restart the agent to load the selected integrations.")) : /*#__PURE__*/React.createElement(React.Fragment, null, /*#__PURE__*/React.createElement("fieldset", {
    style: {
      border: 0,
      padding: 0,
      margin: 0,
      display: 'flex',
      flexDirection: 'column',
      gap: 7
    }
  }, /*#__PURE__*/React.createElement("legend", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: 'var(--text-mid)',
      marginBottom: 2
    }
  }, "Agent skill"), SKILL_VARIANTS.map(variant => /*#__PURE__*/React.createElement("label", {
    key: variant.value,
    style: {
      display: 'flex',
      alignItems: 'flex-start',
      gap: 8,
      cursor: 'pointer',
      fontFamily: 'var(--font-ui)',
      fontSize: 12,
      color: 'var(--text-hi)'
    }
  }, /*#__PURE__*/React.createElement("input", {
    type: "radio",
    name: "fetchira-skill",
    value: variant.value,
    checked: skill === variant.value,
    onChange: () => setSkill(variant.value)
  }), /*#__PURE__*/React.createElement("span", null, /*#__PURE__*/React.createElement("span", {
    style: {
      display: 'block'
    }
  }, variant.label, installedSkill === variant.value ? ' · installed' : ''), /*#__PURE__*/React.createElement("span", {
    style: {
      display: 'block',
      color: 'var(--text-mid)',
      fontSize: 12
    }
  }, variant.hint))))), targets.map(t => /*#__PURE__*/React.createElement("label", {
    key: t.name,
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 10,
      cursor: 'pointer',
      fontFamily: 'var(--font-mono)',
      fontSize: 13,
      color: 'var(--text-hi)'
    }
  }, /*#__PURE__*/React.createElement("input", {
    type: "checkbox",
    checked: !!picked[t.name],
    onChange: e => setPicked(p => _objectSpread(_objectSpread({}, p), {}, {
      [t.name]: e.target.checked
    }))
  }), t.name, t.installed ? /*#__PURE__*/React.createElement(Badge, {
    tone: "ok",
    variant: "soft"
  }, "registered \u2713") : t.present ? /*#__PURE__*/React.createElement(Badge, {
    tone: "accent",
    variant: "outline"
  }, "detected") : null)), /*#__PURE__*/React.createElement(Button, {
    variant: "primary",
    onClick: install,
    disabled: busy || skill === 'skip' && !Object.keys(picked).some(n => picked[n]),
    style: {
      alignSelf: 'flex-start'
    }
  }, busy ? 'Registering…' : 'Register')));
}
window.InstallTargets = InstallTargets;
function InstallPanel({
  onClose
}) {
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
      maxHeight: 'calc(100dvh - 40px)',
      overflowY: 'auto'
    }
  }, /*#__PURE__*/React.createElement(Card, {
    raised: true,
    pad: 0,
    style: {
      borderRadius: 'var(--r-lg)'
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
      fontSize: 17,
      fontWeight: 600,
      color: 'var(--text-hi)'
    }
  }, "Register in your coding tools"), /*#__PURE__*/React.createElement("button", {
    onClick: onClose,
    style: {
      background: 'transparent',
      border: 'none',
      color: 'var(--text-lo)',
      cursor: 'pointer',
      fontSize: 18,
      lineHeight: 1,
      padding: 4
    }
  }, "\u2715")), /*#__PURE__*/React.createElement("div", {
    style: {
      padding: 20
    }
  }, /*#__PURE__*/React.createElement(InstallTargets, null)), /*#__PURE__*/React.createElement("div", {
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
  }, "Close")))));
}
function GettingStarted() {
  return window.fxHosted ? /*#__PURE__*/React.createElement(HostedGettingStarted, null) : /*#__PURE__*/React.createElement(LocalGettingStarted, null);
}
function LocalGettingStarted() {
  const [hidden, setHidden] = React.useState(() => localStorage.getItem('fx-gs-dismissed') === '1');
  const [installOpen, setInstallOpen] = React.useState(false);
  const [modalProv, setModalProv] = React.useState(null);
  const [registered, setRegistered] = React.useState(null); // null until the targets probe lands

  React.useEffect(() => {
    if (hidden) return;
    apiGet('/api/install/targets').then(d => {
      if (d) {
        const skillInstalled = (Array.isArray(d.skills) ? d.skills : [d.skill]).some(skill => ['both', 'mcp', 'cli'].includes(skill));
        setRegistered(skillInstalled || (d.targets || []).some(t => t.installed));
      }
    });
  }, [installOpen]); // re-check after the install panel closes

  if (hidden) return null;
  const accounts = window.FX.accounts || [];
  const items = [{
    label: 'Connect a provider',
    hint: 'search + read quota for the router',
    done: accounts.length > 0,
    action: () => setModalProv('tavily')
  }, {
    label: 'Add a web session',
    hint: 'gemini / grok / chatgpt login — unlocks deep research + images',
    done: accounts.some(a => a.web && a.loggedIn),
    action: () => setModalProv('gemini_web')
  }, {
    label: 'Register fetchira in your coding tools',
    hint: 'one click into Claude Code, Codex, Cursor, …',
    done: !!registered,
    action: () => setInstallOpen(true)
  }];
  const doneCount = items.filter(i => i.done).length;
  const dismiss = () => {
    localStorage.setItem('fx-gs-dismissed', '1');
    setHidden(true);
  };
  // All steps done -> self-dismiss for good (wait for the targets probe so we don't misjudge step 3).
  if (registered !== null && doneCount === items.length && !installOpen && !modalProv) {
    localStorage.setItem('fx-gs-dismissed', '1');
    return null;
  }
  return /*#__PURE__*/React.createElement(Card, {
    pad: 0,
    style: {
      overflow: 'hidden'
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 8,
      padding: '12px 14px',
      borderBottom: '1px solid var(--border-faint)'
    }
  }, /*#__PURE__*/React.createElement(StatusDot, {
    tone: "accent",
    size: 7
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-display)',
      fontSize: 14,
      fontWeight: 600,
      color: 'var(--text-hi)'
    }
  }, "Getting started"), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: 'var(--text-faint)'
    }
  }, doneCount, "/", items.length), /*#__PURE__*/React.createElement("span", {
    style: {
      flex: 1
    }
  }), /*#__PURE__*/React.createElement("button", {
    "aria-label": "Dismiss getting started",
    onClick: dismiss,
    title: "dismiss",
    style: {
      background: 'transparent',
      border: 'none',
      color: 'var(--text-faint)',
      cursor: 'pointer',
      fontSize: 14,
      lineHeight: 1,
      padding: 2
    }
  }, "\u2715")), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      flexDirection: 'column'
    }
  }, items.map((it, i) => /*#__PURE__*/React.createElement("button", {
    key: it.label,
    onClick: it.action,
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 10,
      padding: '10px 14px',
      background: 'transparent',
      border: 'none',
      borderTop: i ? '1px solid var(--border-faint)' : 'none',
      cursor: 'pointer',
      textAlign: 'left',
      width: '100%'
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      width: 16,
      height: 16,
      flexShrink: 0,
      borderRadius: '50%',
      display: 'inline-flex',
      alignItems: 'center',
      justifyContent: 'center',
      fontSize: 10,
      color: it.done ? 'var(--green-500)' : 'transparent',
      border: it.done ? '1px solid rgba(70,209,122,0.5)' : '1px solid var(--border-strong)',
      background: it.done ? 'var(--green-dim)' : 'transparent'
    }
  }, it.done ? '✓' : ''), /*#__PURE__*/React.createElement("span", {
    style: {
      minWidth: 0
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      display: 'block',
      fontFamily: 'var(--font-mono)',
      fontSize: 12.5,
      color: it.done ? 'var(--text-faint)' : 'var(--text-hi)',
      textDecoration: it.done ? 'line-through' : 'none'
    }
  }, it.label), /*#__PURE__*/React.createElement("span", {
    style: {
      display: 'block',
      fontFamily: 'var(--font-ui)',
      fontSize: 11.5,
      color: 'var(--text-faint)'
    }
  }, it.hint)), /*#__PURE__*/React.createElement("span", {
    style: {
      marginLeft: 'auto',
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: 'var(--text-faint)'
    }
  }, "\u2192")))), installOpen && /*#__PURE__*/React.createElement(InstallPanel, {
    onClose: () => setInstallOpen(false)
  }), modalProv && /*#__PURE__*/React.createElement(window.AddAccountModal, {
    initialProvider: modalProv,
    onClose: () => {
      setModalProv(null);
      if (window.fxRefresh) window.fxRefresh();
    }
  }));
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
    window.hostedAdminJSON('/admin/keys').then(data => {
      if (cancelled) return;
      const hasActiveKey = (data.keys || []).some(candidate => {
        if (candidate.revoked) return false;
        return (candidate.scopes || []).includes('accounts:manage') && (!candidate.expiresAt || new Date(candidate.expiresAt).getTime() > Date.now());
      });
      setVisibility(hasActiveKey ? 'hidden' : 'show');
    }).catch(() => {
      if (!cancelled) setVisibility('show');
    });
    return () => {
      cancelled = true;
    };
  }, []);
  if (hidden || visibility !== 'show') return null;
  const endpoint = `${location.origin}/mcp`;
  const createKey = async () => {
    setBusy(true);
    setError('');
    try {
      const id = `local-${Math.random().toString(36).slice(2, 8)}`;
      const result = await window.hostedAdminJSON('/admin/keys', {
        method: 'POST',
        body: {
          id,
          name: 'Local Fetchira',
          scopes: ['mcp', 'usage:read', 'accounts:manage'],
          rpm: 60,
          daily_limit: 0,
          monthly_limit: 0,
          concurrency_limit: 4,
          expires_at: null
        }
      });
      setKey(result.key);
    } catch (e) {
      setError(e.message || 'Unable to create API key');
    } finally {
      setBusy(false);
    }
  };
  const command = `fetchira remote set ${endpoint} --key '${key || 'your-key'}'`;
  const copy = async (value, name) => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(name);
      setTimeout(() => setCopied(''), 1600);
    } catch (_) {}
  };
  const dismiss = () => {
    localStorage.setItem('fx-hosted-gs-dismissed', '1');
    setHidden(true);
  };
  return /*#__PURE__*/React.createElement(Card, {
    pad: 0,
    style: {
      overflow: 'hidden'
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 8,
      padding: '12px 14px',
      borderBottom: '1px solid var(--border-faint)'
    }
  }, /*#__PURE__*/React.createElement(StatusDot, {
    tone: "accent",
    size: 7
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-display)',
      fontSize: 14,
      fontWeight: 600,
      color: 'var(--text-hi)'
    }
  }, "Connect a local Fetchira"), /*#__PURE__*/React.createElement("span", {
    style: {
      flex: 1
    }
  }), /*#__PURE__*/React.createElement("button", {
    "aria-label": "Dismiss getting started",
    onClick: dismiss,
    title: "dismiss",
    style: {
      background: 'transparent',
      border: 0,
      color: 'var(--text-faint)',
      cursor: 'pointer',
      fontSize: 14
    }
  }, "\u2715")), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'grid',
      gap: 14,
      padding: 14
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      color: 'var(--text-lo)',
      fontSize: 12,
      lineHeight: 1.5
    }
  }, "Install Fetchira locally, generate a scoped key, then run the command below. Your local MCP tools will route through this hosted server."), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'grid',
      gap: 7
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      color: 'var(--text-faint)',
      font: '600 10px var(--font-mono)',
      letterSpacing: '.1em'
    }
  }, "HOSTED ENDPOINT"), /*#__PURE__*/React.createElement("div", {
    className: "secret-row"
  }, /*#__PURE__*/React.createElement("code", null, endpoint), /*#__PURE__*/React.createElement(Button, {
    variant: "secondary",
    size: "sm",
    onClick: () => copy(endpoint, 'endpoint')
  }, copied === 'endpoint' ? 'Copied' : 'Copy'))), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'grid',
      gap: 7
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      color: 'var(--text-faint)',
      font: '600 10px var(--font-mono)',
      letterSpacing: '.1em'
    }
  }, "RUN THIS LOCALLY"), /*#__PURE__*/React.createElement("div", {
    className: "secret-row"
  }, /*#__PURE__*/React.createElement("code", null, command), /*#__PURE__*/React.createElement(Button, {
    variant: "secondary",
    size: "sm",
    onClick: () => copy(command, 'command')
  }, copied === 'command' ? 'Copied' : 'Copy')), /*#__PURE__*/React.createElement("div", {
    style: {
      color: 'var(--text-faint)',
      font: '11px var(--font-mono)'
    }
  }, "Run it in your terminal, then use ", /*#__PURE__*/React.createElement("code", null, "fetchira remote check"), " to verify version, schema and access.")), !key && /*#__PURE__*/React.createElement(Button, {
    variant: "primary",
    onClick: createKey,
    disabled: busy
  }, busy ? 'Generating…' : 'Generate API key'), error && /*#__PURE__*/React.createElement("div", {
    role: "alert",
    style: {
      color: 'var(--red-500)',
      font: '12px var(--font-mono)'
    }
  }, error)));
}
window.GettingStarted = GettingStarted;
})();
