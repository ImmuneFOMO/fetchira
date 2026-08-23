(function () {
/* Top bar: wordmark + global status pills + total remaining + Add account. */
const {
  Button,
  Badge,
  StatusDot
} = window.FetchiraDesignSystem_6526df;
function fmtCompact(n) {
  if (n >= 1e6) return (n / 1e6).toFixed(2).replace(/\.?0+$/, '') + 'M';
  if (n >= 1e3) return (n / 1e3).toFixed(1).replace(/\.0$/, '') + 'K';
  return String(n);
}
function Wordmark() {
  return /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 9
    }
  }, /*#__PURE__*/React.createElement("img", {
    src: window.fxHosted ? '/admin/assets/assets/logo-mark.svg' : '../../assets/logo-mark.svg',
    alt: "",
    style: {
      width: 26,
      height: 26
    }
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-display)',
      fontSize: 19,
      fontWeight: 600,
      letterSpacing: '-0.03em',
      color: 'var(--text-hi)'
    }
  }, "fetchira"), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 10,
      color: 'var(--text-faint)',
      border: '1px solid var(--border-hairline)',
      borderRadius: 'var(--r-xs)',
      padding: '1px 5px',
      marginLeft: 2
    }
  }, window.fxHosted ? 'hosted' : '127.0.0.1:7878'));
}
function TopBar({
  onAdd
}) {
  const total = window.FX.totalRemaining;
  const s = window.FX.summary || {
    accounts: (window.FX.accounts || []).length,
    healthy: 0,
    needsLogin: 0,
    exhausted: 0
  };
  return /*#__PURE__*/React.createElement("header", {
    style: {
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'space-between',
      gap: 16,
      height: 'var(--topbar-h)',
      padding: '0 20px',
      borderBottom: '1px solid var(--border-hairline)',
      background: 'rgba(10,12,17,0.8)',
      backdropFilter: 'blur(12px)',
      position: 'sticky',
      top: 0,
      zIndex: 20
    }
  }, /*#__PURE__*/React.createElement(Wordmark, null), /*#__PURE__*/React.createElement("div", {
    className: "fx-top-actions",
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 14
    }
  }, /*#__PURE__*/React.createElement("div", {
    className: "fx-top-status",
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 7,
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: 'var(--text-mid)'
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--text-hi)',
      fontWeight: 600
    }
  }, s.accounts), /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--text-faint)'
    }
  }, "accounts"), /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--border-strong)'
    }
  }, "\xB7"), /*#__PURE__*/React.createElement(StatusDot, {
    tone: "ok",
    size: 6
  }), /*#__PURE__*/React.createElement("span", null, s.healthy, " healthy"), /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--border-strong)'
    }
  }, "\xB7"), /*#__PURE__*/React.createElement(StatusDot, {
    tone: "off",
    size: 6
  }), /*#__PURE__*/React.createElement("span", null, s.needsLogin, " need login"), /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--border-strong)'
    }
  }, "\xB7"), /*#__PURE__*/React.createElement(StatusDot, {
    tone: "out",
    size: 6
  }), /*#__PURE__*/React.createElement("span", null, s.exhausted, " exhausted")), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      flexDirection: 'column',
      alignItems: 'flex-end',
      paddingLeft: 14,
      borderLeft: '1px solid var(--border-hairline)'
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontVariantNumeric: 'tabular-nums',
      fontSize: 16,
      fontWeight: 600,
      color: 'var(--lime-500)',
      lineHeight: 1
    }
  }, fmtCompact(total)), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 10,
      color: 'var(--text-faint)',
      letterSpacing: '0.04em',
      textTransform: 'uppercase'
    }
  }, "req remaining")), /*#__PURE__*/React.createElement(Button, {
    "aria-label": "Add account",
    variant: "primary",
    iconLeft: /*#__PURE__*/React.createElement("span", {
      style: {
        fontFamily: 'var(--font-mono)',
        fontWeight: 700
      }
    }, "+"),
    onClick: onAdd
  }, "Add account")));
}
window.TopBar = TopBar;
window.fmtCompact = fmtCompact;
})();
