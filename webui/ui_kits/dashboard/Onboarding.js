(function () {
/* First-run welcome: connect one provider fast (validate on paste, show live quota),
   try a real routed search, then hand over to the dashboard. Everything is skippable. */
const {
  Card,
  Button,
  Input,
  Badge,
  StatusDot
} = window.FetchiraDesignSystem_6526df;

// All key providers, best-first: tavily covers search+read+deep-research on a renewable
// monthly tier, serper has the biggest instant grant, firecrawl leads the read chain.
const OB_ORDER = ['tavily', 'serper', 'firecrawl', 'exa', 'parallel', 'steel'];
const OB_KEY_HINTS = {
  tavily: 'tvly-…',
  serper: 'paste your serper.dev key',
  exa: 'paste your exa key',
  firecrawl: 'fc-…',
  parallel: 'paste your parallel key',
  steel: 'ste-…'
};
function obCatalog() {
  return window.FX && window.FX.catalog || [];
}
function SectionLabel({
  children
}) {
  return /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 10,
      margin: '26px 0 12px'
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      fontWeight: 600,
      letterSpacing: '0.12em',
      textTransform: 'uppercase',
      color: 'var(--text-lo)'
    }
  }, children), /*#__PURE__*/React.createElement("span", {
    style: {
      flex: 1,
      height: 1,
      background: 'var(--border-faint)'
    }
  }));
}

// One key-paste provider: signup deep link, paste field, validate with a real call on connect.
function KeyProviderCard({
  p,
  onOpenModal
}) {
  const [key, setKey] = React.useState('');
  const [phase, setPhase] = React.useState('form'); // form | adding | testing | done | bad
  const [label, setLabel] = React.useState('');
  const [ms, setMs] = React.useState(null);
  const [err, setErr] = React.useState(null);
  const connect = async () => {
    if (key.trim().length < 8 || phase === 'adding' || phase === 'testing') return;
    setErr(null);
    setPhase('adding');
    let added;
    try {
      added = await window.apiPost('/api/account/add', {
        provider: p.id,
        label: '',
        key: key.trim()
      });
    } catch (e) {
      setErr(String(e.message || e));
      setPhase('form');
      return;
    }
    setLabel(added.label);
    if (window.fxRefresh) window.fxRefresh();
    setPhase('testing');
    try {
      const t = await window.apiPost('/api/account/test', {
        label: added.label
      });
      if (t.ok) {
        setMs(t.latencyMs);
        setPhase('done');
      } else {
        setErr(t.error || 'test call failed');
        setPhase('bad');
      }
    } catch (e) {
      setErr(String(e.message || e));
      setPhase('bad');
    }
    if (window.fxRefresh) window.fxRefresh();
  };
  const removeRetry = async () => {
    try {
      await window.apiPost('/api/account/remove', {
        label
      });
    } catch (e) {}
    setPhase('form');
    setErr(null);
    setKey('');
    if (window.fxRefresh) window.fxRefresh();
  };
  const acct = (window.FX.accounts || []).find(a => a.label === label);
  if (phase === 'done') {
    return /*#__PURE__*/React.createElement(Card, {
      accent: "ok",
      pad: 16,
      style: {
        display: 'flex',
        flexDirection: 'column',
        gap: 8
      }
    }, /*#__PURE__*/React.createElement("div", {
      style: {
        display: 'flex',
        alignItems: 'center',
        gap: 8
      }
    }, /*#__PURE__*/React.createElement(StatusDot, {
      tone: "ok",
      size: 7
    }), /*#__PURE__*/React.createElement("span", {
      style: {
        fontFamily: 'var(--font-mono)',
        fontSize: 14,
        fontWeight: 600,
        color: 'var(--text-hi)'
      }
    }, p.id), /*#__PURE__*/React.createElement(Badge, {
      tone: "ok",
      variant: "soft"
    }, "connected \u2713")), /*#__PURE__*/React.createElement("span", {
      style: {
        fontFamily: 'var(--font-mono)',
        fontSize: 12,
        color: 'var(--text-mid)'
      }
    }, /*#__PURE__*/React.createElement("span", {
      style: {
        color: 'var(--lime-500)'
      }
    }, label), " answered a live call in ", ms, "ms", acct && /*#__PURE__*/React.createElement(React.Fragment, null, " \xB7 ", /*#__PURE__*/React.createElement("b", {
      style: {
        color: 'var(--text-hi)'
      }
    }, Math.max(0, (acct.quota || 0) - (acct.used || 0)).toLocaleString()), " requests in the tank")));
  }
  return /*#__PURE__*/React.createElement(Card, {
    pad: 16,
    style: {
      display: 'flex',
      flexDirection: 'column',
      gap: 10
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'baseline',
      justifyContent: 'space-between',
      gap: 8,
      flexWrap: 'wrap'
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 8,
      minWidth: 0
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 14,
      fontWeight: 600,
      color: 'var(--text-hi)'
    }
  }, p.id), p.free && /*#__PURE__*/React.createElement(Badge, {
    tone: "neutral",
    variant: "outline"
  }, p.free)), p.signup && /*#__PURE__*/React.createElement("a", {
    href: p.signup,
    target: "_blank",
    rel: "noreferrer",
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: 'var(--lime-500)',
      textDecoration: 'none',
      whiteSpace: 'nowrap'
    }
  }, "get a free key \u2197")), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-ui)',
      fontSize: 12,
      color: 'var(--text-lo)',
      flex: 1
    }
  }, p.blurb), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      gap: 8
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      flex: 1
    }
  }, /*#__PURE__*/React.createElement(Input, {
    placeholder: OB_KEY_HINTS[p.id] || 'paste API key',
    value: key,
    mono: true,
    type: "password",
    onChange: e => setKey(e.target.value),
    onKeyDown: e => {
      if (e.key === 'Enter') connect();
    }
  })), /*#__PURE__*/React.createElement(Button, {
    variant: "primary",
    onClick: connect,
    disabled: key.trim().length < 8 || phase === 'adding' || phase === 'testing'
  }, phase === 'adding' ? 'Adding…' : phase === 'testing' ? 'Testing…' : 'Connect')), phase === 'bad' && /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      flexDirection: 'column',
      gap: 8
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: 'var(--red-500)',
      background: 'var(--red-dim)',
      border: '1px solid rgba(242,85,90,0.3)',
      borderRadius: 'var(--r-sm)',
      padding: '8px 10px'
    }
  }, "key saved as ", label, ", but the test call failed: ", err), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      gap: 8
    }
  }, /*#__PURE__*/React.createElement(Button, {
    variant: "ghost",
    onClick: removeRetry
  }, "Remove & retry"), /*#__PURE__*/React.createElement(Button, {
    variant: "ghost",
    onClick: () => onOpenModal(p.id)
  }, "Open full form"))), err && phase === 'form' && /*#__PURE__*/React.createElement("div", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: 'var(--red-500)',
      background: 'var(--red-dim)',
      border: '1px solid rgba(242,85,90,0.3)',
      borderRadius: 'var(--r-sm)',
      padding: '8px 10px'
    }
  }, err));
}

// Browser-session providers (gemini/grok/chatgpt): sets expectations, then the shared modal
// runs the guided login (browser picker, session-paste fallback).
function WebProviderCard({
  p,
  onOpenModal
}) {
  const connected = (window.FX.accounts || []).some(a => a.provider === p.id && a.loggedIn);
  const hosted = !!window.fxHosted;
  return /*#__PURE__*/React.createElement(Card, {
    accent: connected ? 'ok' : undefined,
    pad: 16,
    style: {
      display: 'flex',
      flexDirection: 'column',
      gap: 10
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 8
    }
  }, connected && /*#__PURE__*/React.createElement(StatusDot, {
    tone: "ok",
    size: 7
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 14,
      fontWeight: 600,
      color: 'var(--text-hi)'
    }
  }, p.id), /*#__PURE__*/React.createElement(Badge, {
    tone: "cyan",
    variant: "outline"
  }, "browser login"), connected && /*#__PURE__*/React.createElement(Badge, {
    tone: "ok",
    variant: "soft"
  }, "connected \u2713")), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-ui)',
      fontSize: 12,
      color: 'var(--text-lo)',
      flex: 1
    }
  }, p.blurb), /*#__PURE__*/React.createElement(Button, {
    variant: "secondary",
    onClick: () => onOpenModal(p.id),
    style: {
      alignSelf: 'flex-start'
    }
  }, connected ? 'Add another account' : 'Connect — sign in via browser'), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-ui)',
      fontSize: 11,
      color: 'var(--text-faint)'
    }
  }, hosted ? 'Run the one-time command shown next on your computer. Your local Fetchira opens the browser and uploads an encrypted session to this server.' : 'Opens a browser window for you to sign in (~30s). Cookies stay on this machine — no password is stored.'));
}

// The aha: one real routed search, showing which provider the router picked.
function TrySearch() {
  const [q, setQ] = React.useState('');
  const [busy, setBusy] = React.useState(false);
  const [res, setRes] = React.useState(null);
  const run = async () => {
    if (!q.trim() || busy) return;
    setBusy(true);
    setRes(null);
    try {
      setRes(await window.apiPost('/api/try', {
        q: q.trim()
      }));
    } catch (e) {
      setRes({
        ok: false,
        error: String(e.message || e)
      });
    }
    setBusy(false);
    if (window.fxRefresh) window.fxRefresh();
  };
  return /*#__PURE__*/React.createElement(Card, {
    raised: true,
    pad: 16,
    style: {
      display: 'flex',
      flexDirection: 'column',
      gap: 10
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 8
    }
  }, /*#__PURE__*/React.createElement(StatusDot, {
    tone: "accent",
    pulse: true,
    size: 7
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-display)',
      fontSize: 15,
      fontWeight: 600,
      color: 'var(--text-hi)'
    }
  }, "Try it \u2014 run a real search")), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      gap: 8
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      flex: 1
    }
  }, /*#__PURE__*/React.createElement(Input, {
    placeholder: "e.g. latest rust 1.80 release notes",
    value: q,
    mono: true,
    onChange: e => setQ(e.target.value),
    onKeyDown: e => {
      if (e.key === 'Enter') run();
    }
  })), /*#__PURE__*/React.createElement(Button, {
    variant: "primary",
    onClick: run,
    disabled: busy || !q.trim()
  }, busy ? 'Routing…' : 'Search')), res && res.ok && /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      flexDirection: 'column',
      gap: 8
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 8,
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: 'var(--text-mid)'
    }
  }, /*#__PURE__*/React.createElement(Badge, {
    tone: "ok",
    variant: "soft"
  }, "200"), "routed via ", /*#__PURE__*/React.createElement("b", {
    style: {
      color: 'var(--lime-500)'
    }
  }, res.provider || 'router'), res.label && /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--text-faint)'
    }
  }, "(", res.label, ")"), "\xB7 ", res.latencyMs, "ms"), /*#__PURE__*/React.createElement("pre", {
    style: {
      margin: 0,
      maxHeight: 220,
      overflowY: 'auto',
      whiteSpace: 'pre-wrap',
      wordBreak: 'break-word',
      fontFamily: 'var(--font-mono)',
      fontSize: 11.5,
      lineHeight: 1.5,
      color: 'var(--text-mid)',
      background: 'var(--surface-sunken, rgba(255,255,255,0.03))',
      border: '1px solid var(--border-hairline)',
      borderRadius: 'var(--r-sm)',
      padding: '10px 12px'
    }
  }, res.text)), res && !res.ok && /*#__PURE__*/React.createElement("div", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: 'var(--red-500)',
      background: 'var(--red-dim)',
      border: '1px solid rgba(242,85,90,0.3)',
      borderRadius: 'var(--r-sm)',
      padding: '8px 10px'
    }
  }, res.error));
}
function Onboarding({
  onDone
}) {
  return window.fxHosted ? /*#__PURE__*/React.createElement(HostedOnboarding, {
    onDone: onDone
  }) : /*#__PURE__*/React.createElement(LocalOnboarding, {
    onDone: onDone
  });
}
function LocalOnboarding({
  onDone
}) {
  const [modalProv, setModalProv] = React.useState(null);
  const [connection, setConnection] = React.useState(() => {
    var _window$FX$setup;
    return ((_window$FX$setup = window.FX.setup) === null || _window$FX$setup === void 0 ? void 0 : _window$FX$setup.mode) === 'hosted' ? 'hosted' : null;
  });
  const catalog = obCatalog();
  const accounts = window.FX.accounts || [];
  const connected = accounts.length;
  const keys = OB_ORDER.map(id => catalog.find(c => c.id === id)).filter(Boolean).concat(catalog.filter(c => !c.web && !OB_ORDER.includes(c.id)));
  const webs = catalog.filter(c => c.web);
  return /*#__PURE__*/React.createElement("div", {
    style: {
      maxWidth: 860,
      margin: '5vh auto 60px',
      padding: '0 20px'
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 10,
      marginBottom: 14
    }
  }, /*#__PURE__*/React.createElement("img", {
    src: window.fxHosted ? '/admin/assets/assets/logo-mark.svg' : '../../assets/logo-mark.svg',
    alt: "",
    width: "34",
    height: "34",
    style: {
      width: 34,
      height: 34
    }
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-display)',
      fontSize: 24,
      fontWeight: 600,
      letterSpacing: '-0.03em',
      color: 'var(--text-hi)'
    }
  }, "fetchira")), /*#__PURE__*/React.createElement("div", {
    style: {
      fontFamily: 'var(--font-display)',
      fontSize: 20,
      fontWeight: 600,
      color: 'var(--text-hi)',
      letterSpacing: '-0.02em',
      marginBottom: 6
    }
  }, "One search router for all your agents"), /*#__PURE__*/React.createElement("div", {
    style: {
      fontFamily: 'var(--font-ui)',
      fontSize: 14,
      color: 'var(--text-mid)',
      lineHeight: 1.55,
      maxWidth: 620
    }
  }, "fetchira gives your AI tools web search, scraping and deep research \u2014 routed across providers with quota-aware failover. Use accounts on this computer or connect to an existing server."), /*#__PURE__*/React.createElement(Card, {
    pad: 16,
    style: {
      marginTop: 22
    }
  }, connection === null ? /*#__PURE__*/React.createElement(window.ConnectionSetup, {
    onReady: setConnection
  }) : /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'space-between',
      gap: 12
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--text-hi)',
      fontSize: 13
    }
  }, connection === 'hosted' ? 'Connected to your server' : 'Using accounts on this computer'), /*#__PURE__*/React.createElement(Button, {
    variant: "ghost",
    onClick: () => setConnection(null)
  }, "Change"))), connection === 'local' && /*#__PURE__*/React.createElement(React.Fragment, null, /*#__PURE__*/React.createElement(SectionLabel, null, "free api keys \u2014 no credit card, ~60 seconds"), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'grid',
      gridTemplateColumns: 'repeat(auto-fit, minmax(320px, 1fr))',
      gap: 14
    }
  }, keys.map(p => /*#__PURE__*/React.createElement(KeyProviderCard, {
    key: p.id,
    p: p,
    onOpenModal: setModalProv
  }))), /*#__PURE__*/React.createElement(SectionLabel, null, "your ai subscriptions \u2014 unlock deep research + images"), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'grid',
      gridTemplateColumns: 'repeat(auto-fit, minmax(250px, 1fr))',
      gap: 14
    }
  }, webs.map(p => /*#__PURE__*/React.createElement(WebProviderCard, {
    key: p.id,
    p: p,
    onOpenModal: setModalProv
  })))), (connection === 'hosted' || connection === 'local' && connected > 0) && /*#__PURE__*/React.createElement("div", {
    style: {
      marginTop: 26,
      display: 'flex',
      flexDirection: 'column',
      gap: 14
    }
  }, connection === 'local' && /*#__PURE__*/React.createElement(TrySearch, null), /*#__PURE__*/React.createElement(Card, {
    raised: true,
    pad: 16,
    style: {
      display: 'flex',
      flexDirection: 'column',
      gap: 10
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 8
    }
  }, /*#__PURE__*/React.createElement(StatusDot, {
    tone: "accent",
    size: 7
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-display)',
      fontSize: 15,
      fontWeight: 600,
      color: 'var(--text-hi)'
    }
  }, "Add fetchira to your coding agents")), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-ui)',
      fontSize: 12,
      color: 'var(--text-lo)'
    }
  }, "Choose CLI only, MCP, or MCP with CLI fallback, then select your agents."), /*#__PURE__*/React.createElement(window.InstallTargets, null))), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'space-between',
      marginTop: 30,
      paddingTop: 16,
      borderTop: '1px solid var(--border-faint)'
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: connected ? 'var(--lime-500)' : 'var(--text-faint)'
    }
  }, connection === 'hosted' ? 'hosted connection ready' : connected ? `${connected} ${connected === 1 ? 'provider' : 'providers'} connected` : 'nothing connected yet'), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      gap: 8
    }
  }, /*#__PURE__*/React.createElement(Button, {
    variant: "ghost",
    onClick: onDone
  }, "Skip for now"), /*#__PURE__*/React.createElement(Button, {
    variant: "primary",
    onClick: onDone,
    disabled: connection !== 'hosted' && !(connection === 'local' && connected)
  }, "Go to dashboard \u2192"))), modalProv && /*#__PURE__*/React.createElement(window.AddAccountModal, {
    initialProvider: modalProv,
    onClose: () => {
      setModalProv(null);
      if (window.fxRefresh) window.fxRefresh();
    }
  }));
}
function HostedOnboarding({
  onDone
}) {
  const [modalProv, setModalProv] = React.useState(null);
  const [key, setKey] = React.useState(null);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState('');
  const [commandCopied, setCommandCopied] = React.useState(false);
  const [connectionReady, setConnectionReady] = React.useState(false);
  const [checkingConnection, setCheckingConnection] = React.useState(false);
  const catalog = obCatalog();
  const accounts = window.FX.accounts || [];
  const endpoint = `${location.origin}/mcp`;
  const createKey = async () => {
    setBusy(true);
    setError('');
    try {
      const result = await window.hostedAdminJSON('/admin/keys', {
        method: 'POST',
        body: {
          id: `local-${Math.random().toString(36).slice(2, 8)}`,
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
  const copy = async (value, kind) => {
    var _navigator$clipboard;
    await ((_navigator$clipboard = navigator.clipboard) === null || _navigator$clipboard === void 0 ? void 0 : _navigator$clipboard.writeText(value));
    if (kind === 'setup') setCommandCopied(true);
  };
  const command = 'fetchira setup';
  const checkCommand = 'fetchira remote check';
  const keyId = key ? key.split('_')[2] : '';
  const refreshConnection = React.useCallback(async () => {
    if (!key) return false;
    try {
      const result = await window.hostedAdminJSON('/admin/keys', {
        cache: 'no-store'
      });
      const current = (result.keys || []).find(item => item.id === keyId);
      const ready = Boolean(current === null || current === void 0 ? void 0 : current.remoteChecked);
      setConnectionReady(ready);
      return ready;
    } catch (_) {
      return false;
    }
  }, [key, keyId]);
  React.useEffect(() => {
    if (!key || connectionReady) return undefined;
    const poll = () => refreshConnection();
    poll();
    const timer = window.setInterval(poll, 2000);
    return () => window.clearInterval(timer);
  }, [key, connectionReady, refreshConnection]);
  const checkNow = async () => {
    setCheckingConnection(true);
    const ready = await refreshConnection();
    if (!ready) setError('No successful remote check received yet. Run fetchira remote check in the terminal.');
    setCheckingConnection(false);
  };
  return /*#__PURE__*/React.createElement("div", {
    style: {
      maxWidth: 860,
      margin: '5vh auto 60px',
      padding: '0 20px'
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 10,
      marginBottom: 14
    }
  }, /*#__PURE__*/React.createElement("img", {
    src: "/admin/assets/assets/logo-mark.svg",
    alt: "",
    style: {
      width: 34,
      height: 34
    }
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-display)',
      fontSize: 24,
      fontWeight: 600,
      color: 'var(--text-hi)'
    }
  }, "fetchira"), /*#__PURE__*/React.createElement(Badge, {
    tone: "accent",
    variant: "outline"
  }, "hosted setup")), /*#__PURE__*/React.createElement("div", {
    style: {
      fontFamily: 'var(--font-display)',
      fontSize: 20,
      fontWeight: 600,
      color: 'var(--text-hi)',
      marginBottom: 6
    }
  }, "Connect this server to your local Fetchira"), /*#__PURE__*/React.createElement("div", {
    style: {
      fontFamily: 'var(--font-ui)',
      fontSize: 14,
      color: 'var(--text-mid)',
      lineHeight: 1.55
    }
  }, "Create a scoped API key, configure your local CLI, then add provider credentials here. Browser sessions are captured on your computer and encrypted before upload."), /*#__PURE__*/React.createElement(SectionLabel, null, "1 \xB7 connect your local cli"), /*#__PURE__*/React.createElement(Card, {
    pad: 16,
    style: {
      display: 'grid',
      gap: 12
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      color: 'var(--text-lo)',
      fontSize: 12
    }
  }, "The CLI checks the server protocol and schema before every MCP connection."), /*#__PURE__*/React.createElement("div", {
    className: "secret-row"
  }, /*#__PURE__*/React.createElement("code", null, endpoint), /*#__PURE__*/React.createElement(Button, {
    variant: "secondary",
    size: "sm",
    onClick: () => copy(endpoint)
  }, "Copy endpoint")), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'grid',
      gap: 8
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
    disabled: !key,
    onClick: () => copy(command, 'setup')
  }, commandCopied ? 'Copied' : 'Copy command')), /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--text-faint)',
      font: '11px var(--font-mono)'
    }
  }, key ? 'Choose Connect to a server, then enter the endpoint and API key shown here. Setup verifies access before saving.' : 'Generate an API key below to begin setup.')), commandCopied && /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'grid',
      gap: 8,
      padding: '12px 0 0',
      borderTop: '1px solid var(--border-faint)'
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      color: 'var(--lime-500)',
      font: '600 10px var(--font-mono)',
      letterSpacing: '.1em'
    }
  }, "VERIFY CONNECTION"), /*#__PURE__*/React.createElement("div", {
    style: {
      color: 'var(--text-mid)',
      fontSize: 12
    }
  }, "After the setup command finishes, copy and paste this command into the same terminal:"), /*#__PURE__*/React.createElement("div", {
    className: "secret-row"
  }, /*#__PURE__*/React.createElement("code", null, checkCommand), /*#__PURE__*/React.createElement(Button, {
    variant: "secondary",
    size: "sm",
    onClick: () => copy(checkCommand)
  }, "Copy check command")), /*#__PURE__*/React.createElement(Button, {
    variant: "ghost",
    size: "sm",
    onClick: checkNow,
    disabled: checkingConnection
  }, checkingConnection ? 'Checking…' : connectionReady ? 'Connected ✓' : 'Check connection')), key && /*#__PURE__*/React.createElement("div", {
    className: "secret-row"
  }, /*#__PURE__*/React.createElement("code", null, key), /*#__PURE__*/React.createElement(Button, {
    variant: "secondary",
    size: "sm",
    onClick: () => copy(key)
  }, "Copy API key")), !key && /*#__PURE__*/React.createElement(Button, {
    variant: "primary",
    onClick: createKey,
    disabled: busy
  }, busy ? 'Generating…' : 'Generate API key'), error && /*#__PURE__*/React.createElement("div", {
    role: "alert",
    style: {
      color: 'var(--red-500)',
      font: '12px var(--font-mono)'
    }
  }, error)), connectionReady ? /*#__PURE__*/React.createElement(React.Fragment, null, /*#__PURE__*/React.createElement(SectionLabel, null, "2 \xB7 add providers"), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'grid',
      gap: 12
    }
  }, catalog.filter(p => p.web).map(p => /*#__PURE__*/React.createElement(WebProviderCard, {
    key: p.id,
    p: p,
    onOpenModal: setModalProv
  }))), catalog.filter(p => !p.web).length > 0 && /*#__PURE__*/React.createElement(React.Fragment, null, /*#__PURE__*/React.createElement(SectionLabel, null, "api-key providers"), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'grid',
      gap: 12
    }
  }, catalog.filter(p => !p.web).slice(0, 3).map(p => /*#__PURE__*/React.createElement(KeyProviderCard, {
    key: p.id,
    p: p,
    onOpenModal: setModalProv
  }))))) : /*#__PURE__*/React.createElement(Card, {
    pad: 16,
    style: {
      marginTop: 22,
      borderStyle: 'dashed'
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      color: 'var(--text-mid)',
      fontSize: 13
    }
  }, "Finish the local setup and run ", /*#__PURE__*/React.createElement("code", null, "fetchira remote check"), ". Provider login unlocks after this server confirms the connection.")), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      justifyContent: 'space-between',
      alignItems: 'center',
      marginTop: 28,
      paddingTop: 16,
      borderTop: '1px solid var(--border-faint)'
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      color: accounts.length ? 'var(--lime-500)' : 'var(--text-faint)',
      font: '12px var(--font-mono)'
    }
  }, accounts.length ? `${accounts.length} provider${accounts.length === 1 ? '' : 's'} connected` : connectionReady ? 'ready for provider setup' : 'waiting for local connection'), /*#__PURE__*/React.createElement(Button, {
    variant: "primary",
    onClick: onDone,
    disabled: !connectionReady
  }, "Open dashboard \u2192")), modalProv && /*#__PURE__*/React.createElement(window.AddAccountModal, {
    initialProvider: modalProv,
    onClose: () => {
      setModalProv(null);
      if (window.fxRefresh) window.fxRefresh();
    }
  }));
}
window.Onboarding = Onboarding;
})();
