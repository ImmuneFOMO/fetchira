(function () {
/* Add-account modal + guided browser-login flow.
   Key providers → POST the key. Web providers use the local CLI when hosted and the local
   browser flow otherwise. On success the dashboard refreshes. */
const {
  Card,
  Button,
  Input,
  Select,
  Badge,
  StatusDot
} = window.FetchiraDesignSystem_6526df;
const PROVIDER_CATALOG = [{
  id: 'serper',
  kind: 'key',
  note: 'Web search API'
}, {
  id: 'tavily',
  kind: 'key',
  note: 'Search + extract API'
}, {
  id: 'exa',
  kind: 'key',
  note: 'Neural search API'
}, {
  id: 'parallel',
  kind: 'key',
  note: 'Search API'
}, {
  id: 'firecrawl',
  kind: 'key',
  note: 'Crawl + scrape API'
}, {
  id: 'steel',
  kind: 'key',
  note: 'Headless browser sessions'
}, {
  id: 'gemini_web',
  kind: 'web',
  note: 'Browser session · search + #dr'
}, {
  id: 'grok_web',
  kind: 'web',
  note: 'Browser session · search + #dr'
}, {
  id: 'chatgpt_web',
  kind: 'web',
  note: 'Browser session · search + #dr'
}];
function Overlay({
  children,
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
      padding: 20,
      animation: 'fx-log-in var(--dur-mid) var(--ease-out)'
    }
  }, /*#__PURE__*/React.createElement("div", {
    onClick: e => e.stopPropagation(),
    style: {
      width: 460,
      maxWidth: '100%'
    }
  }, children));
}
function Field({
  label,
  children
}) {
  return /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      flexDirection: 'column',
      gap: 6
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      letterSpacing: '0.04em',
      textTransform: 'uppercase',
      color: 'var(--text-lo)'
    }
  }, label), children);
}

// Live catalog from /api/state when served; the hardcoded list keeps the standalone preview working.
function catalogNow() {
  const live = window.FX && window.FX.catalog;
  if (!live || !live.length) return PROVIDER_CATALOG;
  return live.map(c => ({
    id: c.id,
    kind: c.web ? 'web' : 'key',
    note: c.blurb,
    signup: c.signup
  }));
}
function AddAccountModal({
  onClose,
  initialProvider,
  initialLabel = '',
  loginOnly = false
}) {
  const [providerId, setProviderId] = React.useState(initialProvider || 'serper');
  const [label, setLabel] = React.useState(initialLabel);
  const [apiKey, setApiKey] = React.useState('');
  const [proxy, setProxy] = React.useState('');
  const [sessionJson, setSessionJson] = React.useState('');
  const [browser, setBrowser] = React.useState('chrome');
  const [touched, setTouched] = React.useState(false);
  const [phase, setPhase] = React.useState('form'); // form | logging-in | success
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState(null);
  const [addedLabel, setAddedLabel] = React.useState('');
  const [loginChallenge, setLoginChallenge] = React.useState(null);
  const hosted = !!window.fxHosted;
  React.useEffect(() => {
    if (!loginChallenge || !window.fxHosted) return undefined;
    let stopped = false;
    const poll = async () => {
      try {
        const r = await fetch(`/admin/login-challenges/${loginChallenge.challenge}`);
        if (!r.ok) return;
        const d = await r.json();
        if (!stopped && (d.status === 'complete' || d.consumed)) {
          setLoginChallenge(null);
          setPhase('success');
          if (window.fxRefresh) window.fxRefresh();
        }
      } catch (_) {}
    };
    poll();
    const id = setInterval(poll, 1500);
    return () => {
      stopped = true;
      clearInterval(id);
    };
  }, [loginChallenge]);
  const catalog = catalogNow();
  const provider = catalog.find(p => p.id === providerId) || catalog[0];
  const isWeb = provider.kind === 'web';
  const keyMissing = !isWeb && apiKey.trim().length < 8;
  const submitKey = async () => {
    setTouched(true);
    if (keyMissing || busy) return;
    setError(null);
    setBusy(true);
    try {
      const res = await window.apiPost('/api/account/add', {
        provider: providerId,
        label: label.trim(),
        key: apiKey,
        proxy: proxy.trim()
      });
      setAddedLabel(res && res.label || label.trim());
      setPhase('success');
      if (window.fxRefresh) window.fxRefresh();
    } catch (e) {
      setError(String(e.message || e));
    }
    setBusy(false);
  };
  const startLogin = async () => {
    if (busy) return;
    setError(null);
    setPhase('logging-in');
    try {
      const res = await window.apiPost(loginOnly ? '/api/account/login' : '/api/account/add', {
        provider: providerId,
        label: label.trim(),
        proxy: proxy.trim(),
        browser
      });
      setAddedLabel(res && res.label || label.trim() || provider.id);
      if (res && res.challenge) {
        setLoginChallenge(res);
        setPhase('logging-in');
        return;
      }
      setPhase('success');
      if (window.fxRefresh) window.fxRefresh();
    } catch (e) {
      setError(String(e.message || e));
      setPhase('form');
    }
  };

  // Paste a session captured elsewhere (any browser) — the only path on a headless box.
  const submitSession = async () => {
    if (busy || !sessionJson.trim()) return;
    setError(null);
    setBusy(true);
    try {
      const res = await window.apiPost(loginOnly ? '/api/account/session' : '/api/account/add', {
        provider: providerId,
        label: label.trim(),
        proxy: proxy.trim(),
        session: sessionJson
      });
      setAddedLabel(res && res.label || label.trim() || provider.id);
      setPhase('success');
      if (window.fxRefresh) window.fxRefresh();
    } catch (e) {
      setError(String(e.message || e));
    }
    setBusy(false);
  };

  // ---- Success ----
  if (phase === 'success') {
    return /*#__PURE__*/React.createElement(Overlay, {
      onClose: onClose
    }, /*#__PURE__*/React.createElement(Card, {
      raised: true,
      pad: 0,
      style: {
        borderRadius: 'var(--r-lg)'
      }
    }, /*#__PURE__*/React.createElement("div", {
      style: {
        padding: '36px 28px',
        textAlign: 'center'
      }
    }, /*#__PURE__*/React.createElement("div", {
      style: {
        width: 52,
        height: 52,
        margin: '0 auto 16px',
        borderRadius: '50%',
        background: 'var(--green-dim)',
        border: '1px solid rgba(70,209,122,0.4)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        color: 'var(--green-500)',
        fontSize: 24
      }
    }, "\u2713"), /*#__PURE__*/React.createElement("div", {
      style: {
        fontFamily: 'var(--font-display)',
        fontSize: 19,
        fontWeight: 600,
        color: 'var(--text-hi)',
        marginBottom: 6
      }
    }, loginOnly ? 'Session updated' : 'Account added'), /*#__PURE__*/React.createElement("div", {
      style: {
        fontFamily: 'var(--font-mono)',
        fontSize: 13,
        color: 'var(--text-mid)'
      }
    }, /*#__PURE__*/React.createElement("span", {
      style: {
        color: 'var(--lime-500)'
      }
    }, addedLabel), " is ", loginOnly ? 'ready in' : 'live in', " the router rotation.")), /*#__PURE__*/React.createElement("div", {
      style: {
        display: 'flex',
        gap: 8,
        padding: '14px 20px',
        borderTop: '1px solid var(--border-hairline)',
        justifyContent: 'flex-end'
      }
    }, /*#__PURE__*/React.createElement(Button, {
      variant: "primary",
      onClick: onClose
    }, "Done"))));
  }

  // ---- Guided login (web providers — real Chrome capture happens server-side) ----
  if (phase === 'logging-in') {
    return /*#__PURE__*/React.createElement(Overlay, {
      onClose: () => {}
    }, /*#__PURE__*/React.createElement(Card, {
      raised: true,
      pad: 0,
      style: {
        borderRadius: 'var(--r-lg)'
      }
    }, /*#__PURE__*/React.createElement("div", {
      style: {
        padding: '28px'
      }
    }, /*#__PURE__*/React.createElement("div", {
      style: {
        display: 'flex',
        alignItems: 'center',
        gap: 10,
        marginBottom: 16
      }
    }, /*#__PURE__*/React.createElement("span", {
      style: {
        width: 16,
        height: 16,
        borderRadius: '50%',
        border: '2px solid var(--lime-dim)',
        borderTopColor: 'var(--lime-500)',
        display: 'inline-block',
        animation: 'fx-spin 0.8s linear infinite'
      }
    }), /*#__PURE__*/React.createElement("span", {
      style: {
        fontFamily: 'var(--font-display)',
        fontSize: 16,
        fontWeight: 600,
        color: 'var(--text-hi)'
      }
    }, "Guided login")), /*#__PURE__*/React.createElement("div", {
      style: {
        fontFamily: 'var(--font-mono)',
        fontSize: 13,
        color: 'var(--text-mid)',
        lineHeight: 1.5
      }
    }, loginChallenge ? /*#__PURE__*/React.createElement(React.Fragment, null, window.fxHosted ? /*#__PURE__*/React.createElement(React.Fragment, null, "Copy this command to a terminal on your computer. Your local Fetchira will open ", browser === 'firefox' ? 'Firefox' : 'Chrome', ", capture the login, and upload the encrypted session to this server.") : /*#__PURE__*/React.createElement(React.Fragment, null, "A ", browser === 'firefox' ? 'Firefox' : 'Chrome', " window is opening for ", /*#__PURE__*/React.createElement("span", {
      style: {
        color: 'var(--lime-500)'
      }
    }, provider.id), ". Sign in there \u2014 fetchira captures the session automatically."), /*#__PURE__*/React.createElement("div", {
      style: {
        marginTop: 12,
        padding: 10,
        background: 'var(--surface-inset)',
        border: '1px solid var(--border-hairline)',
        borderRadius: 'var(--r-sm)',
        color: 'var(--lime-500)',
        wordBreak: 'break-all'
      }
    }, /*#__PURE__*/React.createElement("code", null, "fetchira remote login ", loginChallenge.challenge, hosted && browser === 'firefox' ? ' --browser firefox' : '')), /*#__PURE__*/React.createElement("div", {
      style: {
        marginTop: 10,
        color: 'var(--text-lo)'
      }
    }, "This one-time challenge expires in 10 minutes. Keep this window open; the account appears after upload.")) : /*#__PURE__*/React.createElement(React.Fragment, null, "Preparing the ", browser === 'firefox' ? 'Firefox' : 'Chrome', " login for ", /*#__PURE__*/React.createElement("span", {
      style: {
        color: 'var(--lime-500)'
      }
    }, provider.id), "\u2026")))));
  }

  // ---- Form ----
  return /*#__PURE__*/React.createElement(Overlay, {
    onClose: onClose
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
      color: 'var(--text-hi)',
      letterSpacing: '-0.01em'
    }
  }, loginOnly ? 'Re-login account' : 'Add account'), /*#__PURE__*/React.createElement("button", {
    "aria-label": "Close",
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
      padding: 20,
      display: 'flex',
      flexDirection: 'column',
      gap: 16
    }
  }, /*#__PURE__*/React.createElement(Field, {
    label: "Provider"
  }, /*#__PURE__*/React.createElement(Select, {
    value: providerId,
    disabled: loginOnly,
    onChange: e => {
      setProviderId(e.target.value);
      setTouched(false);
      setError(null);
    }
  }, catalog.map(p => /*#__PURE__*/React.createElement("option", {
    key: p.id,
    value: p.id
  }, p.id))), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 8,
      marginTop: 2
    }
  }, /*#__PURE__*/React.createElement(Badge, {
    tone: isWeb ? 'cyan' : 'accent',
    variant: "outline"
  }, isWeb ? 'browser login' : 'API key'), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-ui)',
      fontSize: 12,
      color: 'var(--text-lo)'
    }
  }, provider.note), !isWeb && provider.signup && /*#__PURE__*/React.createElement("a", {
    href: provider.signup,
    target: "_blank",
    rel: "noreferrer",
    style: {
      marginLeft: 'auto',
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: 'var(--lime-500)',
      textDecoration: 'none',
      whiteSpace: 'nowrap'
    }
  }, "get a free key \u2197"))), !loginOnly && /*#__PURE__*/React.createElement(Input, {
    label: "Label \xB7 optional",
    placeholder: `${provider.id.replace(/_web$/, '')}-1 (auto)`,
    value: label,
    mono: true,
    onChange: e => setLabel(e.target.value),
    hint: "Leave blank to auto-name"
  }), !isWeb ? /*#__PURE__*/React.createElement(Input, {
    label: "API key",
    placeholder: "paste secret key",
    value: apiKey,
    mono: true,
    type: "password",
    onChange: e => setApiKey(e.target.value),
    invalid: touched && keyMissing,
    hint: touched && keyMissing ? 'Enter a valid API key' : 'Stored locally · never displayed again'
  }) : /*#__PURE__*/React.createElement(Field, {
    label: "Authentication"
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      gap: 6
    }
  }, ['chrome', 'firefox'].map(b => /*#__PURE__*/React.createElement("button", {
    key: b,
    type: "button",
    onClick: () => setBrowser(b),
    style: {
      flex: 1,
      padding: '7px 0',
      cursor: 'pointer',
      textTransform: 'capitalize',
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: browser === b ? 'var(--text-hi)' : 'var(--text-lo)',
      background: browser === b ? 'var(--surface-2)' : 'transparent',
      border: '1px solid',
      borderColor: browser === b ? 'var(--lime-dim)' : 'var(--border-hairline)',
      borderRadius: 'var(--r-sm)'
    }
  }, b))), /*#__PURE__*/React.createElement(Button, {
    variant: "secondary",
    onClick: startLogin,
    style: {
      width: '100%',
      justifyContent: 'center'
    },
    iconLeft: /*#__PURE__*/React.createElement("span", {
      style: {
        fontSize: 13
      }
    }, "\u25E7")
  }, "Log in with browser"), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-ui)',
      fontSize: 12,
      color: 'var(--text-lo)'
    }
  }, hosted ? 'Copy the command shown next and run it on your computer. Your local Fetchira opens the browser and uploads the encrypted session here.' : 'Opens the chosen browser so you can sign in. The session is captured locally — no password is stored.'), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 8,
      margin: '4px 0 2px'
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      flex: 1,
      height: 1,
      background: 'var(--border-hairline)'
    }
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 10,
      letterSpacing: '0.08em',
      textTransform: 'uppercase',
      color: 'var(--text-faint)'
    }
  }, "or paste a session"), /*#__PURE__*/React.createElement("div", {
    style: {
      flex: 1,
      height: 1,
      background: 'var(--border-hairline)'
    }
  })), /*#__PURE__*/React.createElement("textarea", {
    value: sessionJson,
    onChange: e => setSessionJson(e.target.value),
    spellCheck: false,
    placeholder: '[{"name":"sso","value":"…","domain":".grok.com"}]',
    style: {
      width: '100%',
      minHeight: 76,
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
  }), /*#__PURE__*/React.createElement(Button, {
    variant: "ghost",
    onClick: submitSession,
    disabled: busy || !sessionJson.trim(),
    style: {
      width: '100%',
      justifyContent: 'center'
    }
  }, busy ? 'Saving…' : 'Use pasted session'), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-ui)',
      fontSize: 12,
      color: 'var(--text-lo)'
    }
  }, hosted ? 'Fallback: paste an exported session if the local browser is unavailable.' : 'Export cookies from any logged-in browser (e.g. a “Cookie Editor” extension → JSON). Works on headless servers.')), /*#__PURE__*/React.createElement(Input, {
    label: "Proxy \xB7 optional",
    placeholder: "pool, direct, ip:port:user:pass or http://user:pass@host:port",
    value: proxy,
    mono: true,
    onChange: e => setProxy(e.target.value),
    hint: "Leave blank for a direct connection"
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
  }, "Cancel"), !isWeb && !loginOnly && /*#__PURE__*/React.createElement(Button, {
    variant: "primary",
    onClick: submitKey
  }, busy ? 'Adding…' : 'Add account'))));
}
window.AddAccountModal = AddAccountModal;
})();
