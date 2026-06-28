/* @ds-bundle: {"format":3,"namespace":"FetchiraDesignSystem_6526df","components":[{"name":"Badge","sourcePath":"components/core/Badge.jsx"},{"name":"Button","sourcePath":"components/core/Button.jsx"},{"name":"Card","sourcePath":"components/core/Card.jsx"},{"name":"StatusDot","sourcePath":"components/core/StatusDot.jsx"},{"name":"RouteLogLine","sourcePath":"components/feed/RouteLogLine.jsx"},{"name":"Input","sourcePath":"components/forms/Input.jsx"},{"name":"Select","sourcePath":"components/forms/Select.jsx"},{"name":"QuotaMeter","sourcePath":"components/meters/QuotaMeter.jsx"},{"name":"Tabs","sourcePath":"components/navigation/Tabs.jsx"},{"name":"ProviderCard","sourcePath":"components/providers/ProviderCard.jsx"}],"sourceHashes":{"components/core/Badge.jsx":"2b1d5331a768","components/core/Button.jsx":"059a9a4065b0","components/core/Card.jsx":"a1abe5b8198f","components/core/StatusDot.jsx":"59d324080ba3","components/feed/RouteLogLine.jsx":"5e6cd4c81939","components/forms/Input.jsx":"14a1d73dbcb7","components/forms/Select.jsx":"5e2e7a45dcc5","components/meters/QuotaMeter.jsx":"871b97431407","components/navigation/Tabs.jsx":"1069b34ae167","components/providers/ProviderCard.jsx":"1c28ce7551ed","ui_kits/dashboard/AccountsTab.jsx":"276259fabc30","ui_kits/dashboard/ActivityTab.jsx":"1a6046d603e1","ui_kits/dashboard/AddAccountModal.jsx":"d05ec973a9ee","ui_kits/dashboard/OverviewTab.jsx":"e37311b2c10e","ui_kits/dashboard/TopBar.jsx":"e92bc68c95d1","ui_kits/dashboard/data.js":"6f38b93f3b7d"},"inlinedExternals":[],"unexposedExports":[]} */

(() => {

const __ds_ns = (window.FetchiraDesignSystem_6526df = window.FetchiraDesignSystem_6526df || {});

const __ds_scope = {};

(__ds_ns.__errors = __ds_ns.__errors || []);

// components/core/Badge.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/**
 * Badge — compact status pill / chip. Used for health pills, reset-window badges,
 * login chips, key-set chips, failover tags.
 * tone: "neutral" | "ok" | "low" | "out" | "off" | "accent" | "cyan"
 * variant: "soft" (tinted) | "outline" | "solid"
 */
function Badge({
  tone = 'neutral',
  variant = 'soft',
  mono = true,
  dot = false,
  uppercase = false,
  children,
  style = {},
  ...rest
}) {
  const palette = {
    neutral: {
      fg: 'var(--text-mid)',
      tint: 'var(--surface-3)',
      solid: 'var(--surface-3)',
      line: 'var(--border-hairline)'
    },
    ok: {
      fg: 'var(--green-500)',
      tint: 'var(--green-dim)',
      solid: 'var(--green-500)',
      line: 'rgba(70,209,122,0.35)'
    },
    low: {
      fg: 'var(--amber-500)',
      tint: 'var(--amber-dim)',
      solid: 'var(--amber-500)',
      line: 'rgba(245,165,36,0.35)'
    },
    out: {
      fg: 'var(--red-500)',
      tint: 'var(--red-dim)',
      solid: 'var(--red-500)',
      line: 'rgba(242,85,90,0.35)'
    },
    off: {
      fg: 'var(--grey-500)',
      tint: 'var(--grey-dim)',
      solid: 'var(--grey-500)',
      line: 'rgba(91,101,118,0.35)'
    },
    accent: {
      fg: 'var(--lime-500)',
      tint: 'var(--lime-dim)',
      solid: 'var(--lime-500)',
      line: 'var(--border-accent)'
    },
    cyan: {
      fg: 'var(--cyan-500)',
      tint: 'var(--cyan-dim)',
      solid: 'var(--cyan-500)',
      line: 'rgba(52,214,230,0.35)'
    }
  }[tone];
  const v = {
    soft: {
      background: palette.tint,
      color: palette.fg,
      border: `1px solid ${palette.line}`
    },
    outline: {
      background: 'transparent',
      color: palette.fg,
      border: `1px solid ${palette.line}`
    },
    solid: {
      background: palette.solid,
      color: 'var(--text-on-accent)',
      border: '1px solid transparent'
    }
  }[variant];
  return /*#__PURE__*/React.createElement("span", _extends({
    style: {
      display: 'inline-flex',
      alignItems: 'center',
      gap: 5,
      height: 20,
      padding: '0 8px',
      borderRadius: 'var(--r-pill)',
      fontFamily: mono ? 'var(--font-mono)' : 'var(--font-ui)',
      fontSize: 11,
      fontWeight: 500,
      lineHeight: 1,
      letterSpacing: uppercase ? '0.06em' : 0,
      textTransform: uppercase ? 'uppercase' : 'none',
      whiteSpace: 'nowrap',
      ...v,
      ...style
    }
  }, rest), dot && /*#__PURE__*/React.createElement("span", {
    style: {
      width: 6,
      height: 6,
      borderRadius: '50%',
      background: variant === 'solid' ? 'var(--text-on-accent)' : palette.fg
    }
  }), children);
}
Object.assign(__ds_scope, { Badge });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/Badge.jsx", error: String((e && e.message) || e) }); }

// components/core/Button.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/**
 * Button — instrument-panel button. Primary uses the lime accent with dark text.
 * variant: "primary" | "secondary" | "ghost" | "danger"
 */
function Button({
  variant = 'secondary',
  size = 'md',
  iconLeft = null,
  iconRight = null,
  disabled = false,
  children,
  style = {},
  ...rest
}) {
  const sizes = {
    sm: {
      h: 28,
      px: 10,
      fs: 12
    },
    md: {
      h: 34,
      px: 14,
      fs: 13
    },
    lg: {
      h: 40,
      px: 18,
      fs: 14
    }
  }[size];
  const base = {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 7,
    height: sizes.h,
    padding: `0 ${sizes.px}px`,
    fontSize: sizes.fs,
    fontFamily: 'var(--font-ui)',
    fontWeight: 600,
    letterSpacing: '0.01em',
    borderRadius: 'var(--r-sm)',
    border: '1px solid transparent',
    cursor: disabled ? 'not-allowed' : 'pointer',
    opacity: disabled ? 0.45 : 1,
    transition: 'background var(--dur-fast) var(--ease-out), border-color var(--dur-fast), transform var(--dur-fast)',
    userSelect: 'none',
    whiteSpace: 'nowrap',
    outline: 'none'
  };
  const variants = {
    primary: {
      background: 'var(--lime-500)',
      color: 'var(--text-on-accent)',
      borderColor: 'var(--lime-600)'
    },
    secondary: {
      background: 'var(--surface-2)',
      color: 'var(--text-hi)',
      borderColor: 'var(--border-hairline)'
    },
    ghost: {
      background: 'transparent',
      color: 'var(--text-mid)',
      borderColor: 'transparent'
    },
    danger: {
      background: 'var(--red-dim)',
      color: 'var(--red-500)',
      borderColor: 'rgba(242,85,90,0.35)'
    }
  }[variant];
  const onEnter = e => {
    if (disabled) return;
    if (variant === 'primary') e.currentTarget.style.background = 'var(--lime-400)';else if (variant === 'secondary') {
      e.currentTarget.style.background = 'var(--surface-3)';
      e.currentTarget.style.borderColor = 'var(--border-strong)';
    } else if (variant === 'ghost') {
      e.currentTarget.style.background = 'var(--surface-2)';
      e.currentTarget.style.color = 'var(--text-hi)';
    } else if (variant === 'danger') e.currentTarget.style.background = 'rgba(242,85,90,0.22)';
  };
  const onLeave = e => {
    Object.assign(e.currentTarget.style, {
      background: variants.background,
      borderColor: variants.borderColor,
      color: variants.color
    });
  };
  const onDown = e => {
    if (!disabled) e.currentTarget.style.transform = 'translateY(1px)';
  };
  const onUp = e => {
    e.currentTarget.style.transform = 'none';
  };
  const onFocus = e => {
    if (!disabled) e.currentTarget.style.boxShadow = 'var(--glow-accent)';
  };
  const onBlur = e => {
    e.currentTarget.style.boxShadow = 'none';
  };
  return /*#__PURE__*/React.createElement("button", _extends({
    style: {
      ...base,
      ...variants,
      ...style
    },
    disabled: disabled,
    onMouseEnter: onEnter,
    onMouseLeave: onLeave,
    onMouseDown: onDown,
    onMouseUp: onUp,
    onFocus: onFocus,
    onBlur: onBlur
  }, rest), iconLeft && /*#__PURE__*/React.createElement("span", {
    style: {
      display: 'inline-flex',
      marginLeft: -2
    }
  }, iconLeft), children, iconRight && /*#__PURE__*/React.createElement("span", {
    style: {
      display: 'inline-flex',
      marginRight: -2
    }
  }, iconRight));
}
Object.assign(__ds_scope, { Button });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/Button.jsx", error: String((e && e.message) || e) }); }

// components/core/Card.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/**
 * Card — layered surface panel with hairline border. The base container for the
 * dashboard's instrument tiles and panels.
 */
function Card({
  as: Tag = 'div',
  raised = false,
  inset = false,
  pad = 16,
  accent = null,
  // null | 'ok' | 'low' | 'out' | 'off' | 'accent' — top hairline tint
  interactive = false,
  children,
  style = {},
  ...rest
}) {
  const accentColor = accent && {
    ok: 'var(--green-500)',
    low: 'var(--amber-500)',
    out: 'var(--red-500)',
    off: 'var(--grey-500)',
    accent: 'var(--lime-500)'
  }[accent];
  const base = {
    position: 'relative',
    background: inset ? 'var(--surface-well)' : 'var(--surface-1)',
    border: '1px solid var(--border-hairline)',
    borderRadius: 'var(--r-md)',
    boxShadow: raised ? 'var(--elev-raised)' : 'var(--elev-card)',
    padding: typeof pad === 'number' ? pad : pad,
    transition: interactive ? 'border-color var(--dur-fast), background var(--dur-fast)' : 'none',
    ...style
  };
  const onEnter = interactive ? e => {
    e.currentTarget.style.borderColor = 'var(--border-strong)';
    e.currentTarget.style.background = 'var(--surface-2)';
  } : undefined;
  const onLeave = interactive ? e => {
    e.currentTarget.style.borderColor = 'var(--border-hairline)';
    e.currentTarget.style.background = inset ? 'var(--surface-well)' : 'var(--surface-1)';
  } : undefined;
  return /*#__PURE__*/React.createElement(Tag, _extends({
    style: base,
    onMouseEnter: onEnter,
    onMouseLeave: onLeave
  }, rest), accentColor && /*#__PURE__*/React.createElement("span", {
    style: {
      position: 'absolute',
      top: 0,
      left: 12,
      right: 12,
      height: 2,
      background: accentColor,
      borderRadius: '0 0 2px 2px',
      opacity: 0.9,
      boxShadow: `0 0 10px -1px ${accentColor}`
    }
  }), children);
}
Object.assign(__ds_scope, { Card });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/Card.jsx", error: String((e && e.message) || e) }); }

// components/core/StatusDot.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/**
 * StatusDot — a health indicator dot. Optionally pulses for "live" / real-time.
 * tone: "ok" | "low" | "out" | "off" | "accent"
 */
function StatusDot({
  tone = 'ok',
  size = 8,
  pulse = false,
  label = null,
  style = {},
  ...rest
}) {
  const color = {
    ok: 'var(--green-500)',
    low: 'var(--amber-500)',
    out: 'var(--red-500)',
    off: 'var(--grey-500)',
    accent: 'var(--lime-500)'
  }[tone];
  const dot = /*#__PURE__*/React.createElement("span", {
    style: {
      position: 'relative',
      display: 'inline-flex',
      width: size,
      height: size
    }
  }, pulse && /*#__PURE__*/React.createElement("span", {
    style: {
      position: 'absolute',
      inset: 0,
      borderRadius: '50%',
      background: color,
      animation: 'fx-pulse 1.8s var(--ease-out) infinite'
    }
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      position: 'relative',
      width: size,
      height: size,
      borderRadius: '50%',
      background: color,
      boxShadow: tone === 'off' ? 'none' : `0 0 8px -1px ${color}`
    }
  }));
  if (!label) return /*#__PURE__*/React.createElement("span", _extends({
    style: style
  }, rest), dot);
  return /*#__PURE__*/React.createElement("span", _extends({
    style: {
      display: 'inline-flex',
      alignItems: 'center',
      gap: 7,
      ...style
    }
  }, rest), dot, /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: 'var(--text-mid)'
    }
  }, label));
}
Object.assign(__ds_scope, { StatusDot });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/StatusDot.jsx", error: String((e && e.message) || e) }); }

// components/feed/RouteLogLine.jsx
try { (() => {
/**
 * RouteLogLine — one line of the live route log. Monospace, dense.
 * Shape: timestamp · capability · provider+account · latency, with an optional
 * failover hop badge ("exa-1 429 → tavily-1").
 */
const CAP_COLOR = {
  search: 'var(--lime-500)',
  read: 'var(--cyan-500)',
  deep_research: '#C792EA',
  browser: 'var(--amber-500)'
};
function RouteLogLine({
  time,
  capability = 'search',
  provider,
  account,
  latencyMs,
  status = 200,
  failover = null,
  // { from: 'exa-1', code: 429, to: 'tavily-1' }
  fresh = false,
  // animate-in for newly streamed lines
  style = {}
}) {
  const capColor = CAP_COLOR[capability] || 'var(--text-mid)';
  const err = status >= 400;
  return /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'grid',
      gridTemplateColumns: 'auto 92px 1fr auto',
      alignItems: 'center',
      gap: 10,
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      lineHeight: 1.2,
      padding: '5px 10px',
      borderRadius: 'var(--r-xs)',
      borderLeft: `2px solid ${failover ? 'var(--amber-500)' : 'transparent'}`,
      background: fresh ? 'var(--surface-2)' : 'transparent',
      animation: fresh ? 'fx-log-in var(--dur-mid) var(--ease-out)' : 'none',
      ...style
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--text-faint)'
    }
  }, time), /*#__PURE__*/React.createElement("span", {
    style: {
      color: capColor,
      fontWeight: 500
    }
  }, capability), /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--text-mid)',
      overflow: 'hidden',
      textOverflow: 'ellipsis',
      whiteSpace: 'nowrap'
    }
  }, failover ? /*#__PURE__*/React.createElement(React.Fragment, null, /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--text-lo)'
    }
  }, failover.from, " "), /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--red-500)'
    }
  }, failover.code), /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--text-faint)'
    }
  }, " \u2192 "), /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--text-hi)'
    }
  }, failover.to)) : /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--text-hi)'
    }
  }, provider, account != null ? `-${account}` : '')), /*#__PURE__*/React.createElement("span", {
    style: {
      display: 'inline-flex',
      alignItems: 'center',
      gap: 8,
      justifySelf: 'end'
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      color: err ? 'var(--red-500)' : 'var(--text-faint)'
    }
  }, status), /*#__PURE__*/React.createElement("span", {
    style: {
      color: latencyMs > 800 ? 'var(--amber-500)' : 'var(--text-lo)',
      minWidth: 48,
      textAlign: 'right'
    }
  }, latencyMs, "ms")));
}
Object.assign(__ds_scope, { RouteLogLine });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/feed/RouteLogLine.jsx", error: String((e && e.message) || e) }); }

// components/forms/Input.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/**
 * Input — text field on the dark well surface. Supports a leading prefix
 * (e.g. icon or "https://") and monospace mode for keys/URLs/proxies.
 */
function Input({
  label = null,
  hint = null,
  prefix = null,
  mono = false,
  invalid = false,
  size = 'md',
  style = {},
  containerStyle = {},
  ...rest
}) {
  const [focus, setFocus] = React.useState(false);
  const h = size === 'sm' ? 30 : size === 'lg' ? 40 : 34;
  return /*#__PURE__*/React.createElement("label", {
    style: {
      display: 'flex',
      flexDirection: 'column',
      gap: 6,
      ...containerStyle
    }
  }, label && /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      letterSpacing: '0.04em',
      textTransform: 'uppercase',
      color: 'var(--text-lo)'
    }
  }, label), /*#__PURE__*/React.createElement("span", {
    style: {
      display: 'flex',
      alignItems: 'center',
      height: h,
      background: 'var(--surface-well)',
      border: `1px solid ${invalid ? 'var(--red-500)' : focus ? 'var(--lime-500)' : 'var(--border-hairline)'}`,
      borderRadius: 'var(--r-sm)',
      boxShadow: focus && !invalid ? 'var(--glow-accent)' : 'none',
      transition: 'border-color var(--dur-fast), box-shadow var(--dur-fast)',
      padding: '0 10px',
      gap: 7
    }
  }, prefix && /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--text-lo)',
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      display: 'inline-flex'
    }
  }, prefix), /*#__PURE__*/React.createElement("input", _extends({
    onFocus: e => {
      setFocus(true);
      rest.onFocus?.(e);
    },
    onBlur: e => {
      setFocus(false);
      rest.onBlur?.(e);
    }
  }, rest, {
    style: {
      flex: 1,
      minWidth: 0,
      height: '100%',
      border: 'none',
      outline: 'none',
      background: 'transparent',
      color: 'var(--text-hi)',
      fontFamily: mono ? 'var(--font-mono)' : 'var(--font-ui)',
      fontSize: 13,
      letterSpacing: mono ? '0.01em' : 0,
      ...style
    }
  }))), hint && /*#__PURE__*/React.createElement("span", {
    style: {
      fontSize: 11,
      color: invalid ? 'var(--red-500)' : 'var(--text-lo)',
      fontFamily: 'var(--font-ui)'
    }
  }, hint));
}
Object.assign(__ds_scope, { Input });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/forms/Input.jsx", error: String((e && e.message) || e) }); }

// components/forms/Select.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/**
 * Select — native select styled to match the dark well surface.
 */
function Select({
  label = null,
  hint = null,
  size = 'md',
  children,
  style = {},
  containerStyle = {},
  ...rest
}) {
  const [focus, setFocus] = React.useState(false);
  const h = size === 'sm' ? 30 : size === 'lg' ? 40 : 34;
  return /*#__PURE__*/React.createElement("label", {
    style: {
      display: 'flex',
      flexDirection: 'column',
      gap: 6,
      ...containerStyle
    }
  }, label && /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      letterSpacing: '0.04em',
      textTransform: 'uppercase',
      color: 'var(--text-lo)'
    }
  }, label), /*#__PURE__*/React.createElement("span", {
    style: {
      position: 'relative',
      display: 'flex',
      height: h,
      background: 'var(--surface-well)',
      border: `1px solid ${focus ? 'var(--lime-500)' : 'var(--border-hairline)'}`,
      borderRadius: 'var(--r-sm)',
      boxShadow: focus ? 'var(--glow-accent)' : 'none',
      transition: 'border-color var(--dur-fast)'
    }
  }, /*#__PURE__*/React.createElement("select", _extends({
    onFocus: () => setFocus(true),
    onBlur: () => setFocus(false)
  }, rest, {
    style: {
      flex: 1,
      height: '100%',
      appearance: 'none',
      WebkitAppearance: 'none',
      border: 'none',
      outline: 'none',
      background: 'transparent',
      color: 'var(--text-hi)',
      fontFamily: 'var(--font-ui)',
      fontSize: 13,
      padding: '0 28px 0 10px',
      cursor: 'pointer',
      ...style
    }
  }), children), /*#__PURE__*/React.createElement("span", {
    style: {
      position: 'absolute',
      right: 10,
      top: '50%',
      transform: 'translateY(-50%)',
      pointerEvents: 'none',
      color: 'var(--text-lo)',
      fontSize: 10
    }
  }, "\u25BE")), hint && /*#__PURE__*/React.createElement("span", {
    style: {
      fontSize: 11,
      color: 'var(--text-lo)'
    }
  }, hint));
}
Object.assign(__ds_scope, { Select });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/forms/Select.jsx", error: String((e && e.message) || e) }); }

// components/meters/QuotaMeter.jsx
try { (() => {
/**
 * QuotaMeter — the core "fuel gauge" for a provider's quota.
 *
 * The fill represents USAGE (used / quota): a near-empty meter means lots of free
 * quota remains (healthy); a full meter means exhausted. Color is driven by how
 * much is LEFT — green (plenty) → amber (running low) → red (exhausted).
 *
 * variant: "segments" (default, segmented bar) | "bar" (continuous) | "radial"
 */
function QuotaMeter({
  used = 0,
  quota = 100,
  variant = 'segments',
  state,
  // optional override: 'ok' | 'low' | 'out' | 'off'
  size = 'md',
  // sm | md | lg
  showValues = true,
  animate = true,
  segments = 24,
  className = '',
  style = {}
}) {
  const pctUsed = quota > 0 ? Math.min(1, used / quota) : 0;
  const remaining = Math.max(0, quota - used);
  const remFrac = quota > 0 ? remaining / quota : 0;
  const auto = state ? state : remaining <= 0 ? 'out' : remFrac < 0.15 ? 'low' : remFrac < 0.4 ? 'low' : 'ok';
  const color = auto === 'off' ? 'var(--health-off)' : auto === 'out' ? 'var(--health-out)' : auto === 'low' ? 'var(--health-low)' : 'var(--health-ok)';
  const off = auto === 'off';
  const trackH = size === 'lg' ? 12 : size === 'sm' ? 6 : 9;
  const radial = size === 'lg' ? 120 : size === 'sm' ? 64 : 92;
  const fmt = n => n.toLocaleString('en-US');
  const Values = showValues ? /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      justifyContent: 'space-between',
      alignItems: 'baseline',
      fontFamily: 'var(--font-mono)',
      fontVariantNumeric: 'tabular-nums',
      marginBottom: 6
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      fontSize: size === 'lg' ? 22 : 15,
      color: off ? 'var(--text-faint)' : 'var(--text-hi)',
      fontWeight: 600,
      letterSpacing: '-0.01em'
    }
  }, off ? '—' : fmt(remaining), /*#__PURE__*/React.createElement("span", {
    style: {
      fontSize: 11,
      color: 'var(--text-lo)',
      fontWeight: 400,
      marginLeft: 5
    }
  }, "left")), /*#__PURE__*/React.createElement("span", {
    style: {
      fontSize: 11,
      color: 'var(--text-lo)'
    }
  }, fmt(used), " / ", fmt(quota))) : null;
  if (variant === 'radial') {
    const r = radial / 2 - 8;
    const c = 2 * Math.PI * r;
    const dash = c * (off ? 0 : pctUsed);
    return /*#__PURE__*/React.createElement("div", {
      className: className,
      style: {
        position: 'relative',
        display: 'inline-flex',
        width: radial,
        height: radial,
        ...style
      }
    }, /*#__PURE__*/React.createElement("svg", {
      width: radial,
      height: radial,
      style: {
        transform: 'rotate(-90deg)'
      }
    }, /*#__PURE__*/React.createElement("circle", {
      cx: radial / 2,
      cy: radial / 2,
      r: r,
      fill: "none",
      stroke: "var(--surface-3)",
      strokeWidth: size === 'lg' ? 10 : 7
    }), /*#__PURE__*/React.createElement("circle", {
      cx: radial / 2,
      cy: radial / 2,
      r: r,
      fill: "none",
      stroke: color,
      strokeWidth: size === 'lg' ? 10 : 7,
      strokeLinecap: "round",
      strokeDasharray: `${dash} ${c}`,
      style: {
        transition: animate ? `stroke-dasharray var(--dur-meter) var(--ease-out)` : 'none'
      }
    })), /*#__PURE__*/React.createElement("div", {
      style: {
        position: 'absolute',
        inset: 0,
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        gap: 1,
        fontFamily: 'var(--font-mono)',
        fontVariantNumeric: 'tabular-nums'
      }
    }, /*#__PURE__*/React.createElement("span", {
      style: {
        fontSize: size === 'lg' ? 24 : 16,
        fontWeight: 600,
        lineHeight: 1,
        color: off ? 'var(--text-faint)' : 'var(--text-hi)'
      }
    }, off ? '—' : Math.round(pctUsed * 100) + '%'), /*#__PURE__*/React.createElement("span", {
      style: {
        fontSize: 10,
        color: 'var(--text-lo)',
        letterSpacing: '.06em'
      }
    }, "used")));
  }
  if (variant === 'segments') {
    const lit = off ? 0 : Math.round(pctUsed * segments);
    return /*#__PURE__*/React.createElement("div", {
      className: className,
      style: style
    }, Values, /*#__PURE__*/React.createElement("div", {
      role: "meter",
      "aria-valuenow": used,
      "aria-valuemax": quota,
      style: {
        display: 'grid',
        gridTemplateColumns: `repeat(${segments}, 1fr)`,
        gap: 2,
        height: trackH
      }
    }, Array.from({
      length: segments
    }).map((_, i) => /*#__PURE__*/React.createElement("span", {
      key: i,
      style: {
        borderRadius: 1,
        background: i < lit ? color : 'var(--surface-3)',
        boxShadow: i < lit && auto === 'out' ? 'none' : i < lit ? `0 0 6px -1px ${color}` : 'none',
        opacity: i < lit ? 1 : 0.5,
        transition: animate ? `background var(--dur-meter) var(--ease-out)` : 'none',
        transitionDelay: animate ? `${i * 8}ms` : '0ms'
      }
    }))));
  }

  // continuous bar
  return /*#__PURE__*/React.createElement("div", {
    className: className,
    style: style
  }, Values, /*#__PURE__*/React.createElement("div", {
    role: "meter",
    "aria-valuenow": used,
    "aria-valuemax": quota,
    style: {
      height: trackH,
      borderRadius: 'var(--r-xs)',
      background: 'var(--surface-well)',
      border: '1px solid var(--border-faint)',
      overflow: 'hidden'
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      height: '100%',
      width: off ? '0%' : `${pctUsed * 100}%`,
      background: color,
      borderRadius: 'var(--r-xs)',
      boxShadow: auto !== 'out' ? `0 0 10px -2px ${color}` : 'none',
      transition: animate ? `width var(--dur-meter) var(--ease-out)` : 'none'
    }
  })));
}
Object.assign(__ds_scope, { QuotaMeter });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/meters/QuotaMeter.jsx", error: String((e && e.message) || e) }); }

// components/navigation/Tabs.jsx
try { (() => {
/**
 * Tabs — segmented top-level navigation (Overview / Accounts / Activity).
 * Controlled: pass `value` + `onChange`, with `items: [{id,label,count?}]`.
 */
function Tabs({
  items = [],
  value,
  onChange,
  style = {}
}) {
  return /*#__PURE__*/React.createElement("div", {
    role: "tablist",
    style: {
      display: 'inline-flex',
      gap: 2,
      padding: 3,
      background: 'var(--surface-inset)',
      border: '1px solid var(--border-faint)',
      borderRadius: 'var(--r-sm)',
      ...style
    }
  }, items.map(it => {
    const active = it.id === value;
    return /*#__PURE__*/React.createElement("button", {
      key: it.id,
      role: "tab",
      "aria-selected": active,
      onClick: () => onChange?.(it.id),
      style: {
        display: 'inline-flex',
        alignItems: 'center',
        gap: 7,
        height: 30,
        padding: '0 14px',
        border: 'none',
        cursor: 'pointer',
        borderRadius: 'var(--r-xs)',
        fontFamily: 'var(--font-ui)',
        fontSize: 13,
        fontWeight: 600,
        letterSpacing: '0.01em',
        background: active ? 'var(--surface-2)' : 'transparent',
        color: active ? 'var(--text-hi)' : 'var(--text-lo)',
        boxShadow: active ? 'inset 0 1px 0 rgba(255,255,255,0.05), 0 1px 2px rgba(0,0,0,0.4)' : 'none',
        transition: 'color var(--dur-fast), background var(--dur-fast)'
      },
      onMouseEnter: e => {
        if (!active) e.currentTarget.style.color = 'var(--text-mid)';
      },
      onMouseLeave: e => {
        if (!active) e.currentTarget.style.color = 'var(--text-lo)';
      }
    }, it.label, it.count != null && /*#__PURE__*/React.createElement("span", {
      style: {
        fontFamily: 'var(--font-mono)',
        fontSize: 11,
        fontWeight: 500,
        color: active ? 'var(--lime-500)' : 'var(--text-faint)',
        background: active ? 'var(--lime-dim)' : 'var(--surface-3)',
        padding: '1px 6px',
        borderRadius: 'var(--r-pill)'
      }
    }, it.count));
  }));
}
Object.assign(__ds_scope, { Tabs });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/navigation/Tabs.jsx", error: String((e && e.message) || e) }); }

// components/providers/ProviderCard.jsx
try { (() => {
/**
 * ProviderCard — the Overview tile for one provider. Composes Card + QuotaMeter +
 * Badge + StatusDot. Handles healthy / low / exhausted / needs-login states, an
 * accounts count, a reset-window badge, and (for web-session providers) a login
 * chip + a small deep-research (#dr) daily budget meter.
 */
function ProviderCard({
  name,
  desc,
  used = 0,
  quota = 100,
  resetWindow = 'monthly',
  // monthly | daily | lifetime
  resetsIn = null,
  // e.g. "4d"
  accounts = 1,
  state,
  // optional override of derived health
  webSession = false,
  loggedIn = false,
  dr = null,
  // { used, quota } deep-research daily budget
  style = {}
}) {
  const remaining = Math.max(0, quota - used);
  const remFrac = quota > 0 ? remaining / quota : 0;
  const needsLogin = webSession && !loggedIn;
  const health = state ? state : needsLogin ? 'off' : remaining <= 0 ? 'out' : remFrac < 0.15 ? 'low' : 'ok';
  const accentMap = {
    ok: 'ok',
    low: 'low',
    out: 'out',
    off: 'off'
  };
  return /*#__PURE__*/React.createElement(__ds_scope.Card, {
    accent: accentMap[health],
    interactive: true,
    pad: 14,
    style: {
      display: 'flex',
      flexDirection: 'column',
      gap: 12,
      ...style
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'flex-start',
      justifyContent: 'space-between',
      gap: 8
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      minWidth: 0
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 7
    }
  }, /*#__PURE__*/React.createElement(__ds_scope.StatusDot, {
    tone: health,
    pulse: health === 'low',
    size: 7
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 14,
      fontWeight: 600,
      color: 'var(--text-hi)',
      letterSpacing: '-0.01em'
    }
  }, name)), /*#__PURE__*/React.createElement("div", {
    style: {
      fontFamily: 'var(--font-ui)',
      fontSize: 12,
      color: 'var(--text-lo)',
      marginTop: 3,
      whiteSpace: 'nowrap',
      overflow: 'hidden',
      textOverflow: 'ellipsis'
    }
  }, desc)), /*#__PURE__*/React.createElement(__ds_scope.Badge, {
    tone: resetWindow === 'lifetime' ? 'neutral' : 'neutral',
    variant: "outline",
    uppercase: true
  }, resetWindow)), /*#__PURE__*/React.createElement(__ds_scope.QuotaMeter, {
    used: used,
    quota: quota,
    variant: "segments",
    state: needsLogin ? 'off' : undefined,
    segments: 28
  }), dr && /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 10
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: 'var(--text-lo)',
      minWidth: 34
    }
  }, "#dr"), /*#__PURE__*/React.createElement("div", {
    style: {
      flex: 1
    }
  }, /*#__PURE__*/React.createElement(__ds_scope.QuotaMeter, {
    used: dr.used,
    quota: dr.quota,
    variant: "bar",
    size: "sm",
    showValues: false,
    state: needsLogin ? 'off' : undefined
  })), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: 'var(--text-mid)'
    }
  }, dr.used, "/", dr.quota, " ", /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--text-faint)'
    }
  }, "daily"))), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'space-between',
      gap: 8,
      paddingTop: 10,
      borderTop: '1px solid var(--border-faint)'
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 8
    }
  }, health === 'out' ? /*#__PURE__*/React.createElement(__ds_scope.Badge, {
    tone: "out",
    dot: true
  }, "exhausted", resetsIn ? ` · resets in ${resetsIn}` : '') : needsLogin ? /*#__PURE__*/React.createElement(__ds_scope.Badge, {
    tone: "off",
    dot: true
  }, "needs login") : /*#__PURE__*/React.createElement(__ds_scope.Badge, {
    tone: health,
    dot: true
  }, health === 'low' ? 'running low' : 'healthy', resetsIn ? ` · ${resetsIn}` : '')), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 8
    }
  }, webSession && /*#__PURE__*/React.createElement(__ds_scope.Badge, {
    tone: loggedIn ? 'cyan' : 'low',
    variant: "soft"
  }, loggedIn ? 'logged in ✓' : 'log in ⚠'), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: 'var(--text-lo)'
    }
  }, accounts, " ", accounts === 1 ? 'acct' : 'accts'))));
}
Object.assign(__ds_scope, { ProviderCard });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/providers/ProviderCard.jsx", error: String((e && e.message) || e) }); }

// ui_kits/dashboard/AccountsTab.jsx
try { (() => {
/* Accounts: management table. API keys never shown — masked chip only. */
const {
  Card,
  Badge,
  Button,
  QuotaMeter,
  StatusDot
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
function Th({
  children,
  style
}) {
  return /*#__PURE__*/React.createElement("th", {
    style: {
      textAlign: 'left',
      fontFamily: 'var(--font-mono)',
      fontSize: 10,
      fontWeight: 600,
      letterSpacing: '0.1em',
      textTransform: 'uppercase',
      color: 'var(--text-faint)',
      padding: '0 14px 10px',
      ...style
    }
  }, children);
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
  }, "Actions"))), /*#__PURE__*/React.createElement("tbody", null, rows.map((r, i) => {
    const needsLogin = r.status === 'needs-login';
    return /*#__PURE__*/React.createElement("tr", {
      key: r.label,
      style: {
        borderTop: '1px solid var(--border-faint)'
      },
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
      style: {
        padding: '12px 14px'
      }
    }, /*#__PURE__*/React.createElement("div", {
      style: {
        fontFamily: 'var(--font-mono)',
        fontSize: 13,
        color: 'var(--text-hi)',
        fontWeight: 600
      }
    }, r.label), /*#__PURE__*/React.createElement("div", {
      style: {
        fontFamily: 'var(--font-mono)',
        fontSize: 11,
        color: 'var(--text-faint)'
      }
    }, r.provider)), /*#__PURE__*/React.createElement("td", {
      style: {
        padding: '12px 14px'
      }
    }, /*#__PURE__*/React.createElement(QuotaMeter, {
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
    }, "/ ", r.quota.toLocaleString()))), /*#__PURE__*/React.createElement("td", {
      style: {
        padding: '12px 14px'
      }
    }, /*#__PURE__*/React.createElement(Badge, {
      tone: "neutral",
      variant: "outline",
      uppercase: true
    }, r.resetWindow)), /*#__PURE__*/React.createElement("td", {
      style: {
        padding: '12px 14px',
        fontFamily: 'var(--font-mono)',
        fontSize: 12,
        color: r.proxy === 'direct' ? 'var(--text-faint)' : 'var(--text-mid)'
      }
    }, r.proxy === 'pool' ? /*#__PURE__*/React.createElement(Badge, {
      tone: "cyan",
      variant: "outline"
    }, "pool") : r.proxy), /*#__PURE__*/React.createElement("td", {
      style: {
        padding: '12px 14px'
      }
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
      style: {
        padding: '12px 14px'
      }
    }, statusBadge(r.status)), /*#__PURE__*/React.createElement("td", {
      style: {
        padding: '12px 14px'
      }
    }, /*#__PURE__*/React.createElement("div", {
      style: {
        display: 'flex',
        gap: 6,
        justifyContent: 'flex-end'
      }
    }, /*#__PURE__*/React.createElement(Button, {
      size: "sm",
      variant: "ghost"
    }, "Test"), r.web && /*#__PURE__*/React.createElement(Button, {
      size: "sm",
      variant: needsLogin ? 'primary' : 'secondary'
    }, needsLogin ? 'Login' : 'Re-login'), /*#__PURE__*/React.createElement(Button, {
      size: "sm",
      variant: "ghost",
      style: {
        color: 'var(--text-faint)'
      }
    }, "Remove"))));
  })))));
}
window.AccountsTab = AccountsTab;
})(); } catch (e) { __ds_ns.__errors.push({ path: "ui_kits/dashboard/AccountsTab.jsx", error: String((e && e.message) || e) }); }

// ui_kits/dashboard/ActivityTab.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/* Activity: filterable full route log + usage sparkline charts + health list. */
const {
  Card,
  Badge,
  Button,
  StatusDot,
  RouteLogLine
} = window.FetchiraDesignSystem_6526df;
function Sparkline({
  data,
  color
}) {
  const w = 132,
    h = 38,
    max = Math.max(...data, 1);
  const step = w / (data.length - 1);
  const pts = data.map((v, i) => [i * step, h - v / max * (h - 6) - 2]);
  const line = pts.map((p, i) => `${i === 0 ? 'M' : 'L'}${p[0].toFixed(1)} ${p[1].toFixed(1)}`).join(' ');
  const area = `${line} L${w} ${h} L0 ${h} Z`;
  return /*#__PURE__*/React.createElement("svg", {
    width: w,
    height: h,
    style: {
      display: 'block'
    }
  }, /*#__PURE__*/React.createElement("defs", null, /*#__PURE__*/React.createElement("linearGradient", {
    id: `g-${color.replace(/[^a-z]/gi, '')}`,
    x1: "0",
    y1: "0",
    x2: "0",
    y2: "1"
  }, /*#__PURE__*/React.createElement("stop", {
    offset: "0%",
    stopColor: color,
    stopOpacity: "0.30"
  }), /*#__PURE__*/React.createElement("stop", {
    offset: "100%",
    stopColor: color,
    stopOpacity: "0"
  }))), /*#__PURE__*/React.createElement("path", {
    d: area,
    fill: `url(#g-${color.replace(/[^a-z]/gi, '')})`
  }), /*#__PURE__*/React.createElement("path", {
    d: line,
    fill: "none",
    stroke: color,
    strokeWidth: "1.5",
    strokeLinejoin: "round"
  }));
}
const USAGE = [{
  provider: 'serper-1',
  color: 'var(--lime-500)',
  series: [12, 18, 9, 22, 30, 19, 41, 28, 33, 47]
}, {
  provider: 'jina-1',
  color: 'var(--cyan-500)',
  series: [40, 55, 38, 62, 48, 71, 59, 66, 52, 80]
}, {
  provider: 'exa-1',
  color: 'var(--green-500)',
  series: [5, 9, 14, 6, 11, 8, 19, 22, 7, 12]
}, {
  provider: 'perplexity-1',
  color: 'var(--red-500)',
  series: [22, 30, 41, 38, 50, 44, 33, 0, 0, 0]
}];
function FilterChip({
  label,
  active,
  onClick
}) {
  return /*#__PURE__*/React.createElement("button", {
    onClick: onClick,
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      padding: '4px 10px',
      borderRadius: 'var(--r-pill)',
      cursor: 'pointer',
      border: `1px solid ${active ? 'var(--border-accent)' : 'var(--border-hairline)'}`,
      background: active ? 'var(--lime-dim)' : 'transparent',
      color: active ? 'var(--lime-500)' : 'var(--text-lo)'
    }
  }, label);
}
function HealthRow({
  h
}) {
  const tone = h.state === 'exhausted' ? 'out' : h.state === 'needs-login' ? 'off' : 'ok';
  return /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'flex-start',
      gap: 12,
      padding: '11px 0',
      borderTop: '1px solid var(--border-faint)'
    }
  }, /*#__PURE__*/React.createElement(StatusDot, {
    tone: tone,
    size: 8,
    style: {
      marginTop: 3
    }
  }), /*#__PURE__*/React.createElement("div", {
    style: {
      flex: 1,
      minWidth: 0
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'space-between',
      gap: 8
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 13,
      color: 'var(--text-hi)'
    }
  }, h.provider), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: 'var(--text-faint)'
    }
  }, h.lastSuccess)), h.lastError && /*#__PURE__*/React.createElement("div", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: tone === 'out' ? 'var(--red-500)' : 'var(--text-lo)',
      marginTop: 3,
      lineHeight: 1.4
    }
  }, h.lastError)));
}
function ActivityTab() {
  const [filter, setFilter] = React.useState('all');
  const caps = ['all', 'search', 'read', 'deep_research', 'browser', 'failures'];
  const all = window.FX.log;
  const lines = all.filter(l => filter === 'all' ? true : filter === 'failures' ? !!l.failover : l.capability === filter);
  return /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'grid',
      gridTemplateColumns: 'minmax(0,1fr) 360px',
      gap: 20,
      alignItems: 'start'
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      flexDirection: 'column',
      gap: 20
    }
  }, /*#__PURE__*/React.createElement(Card, {
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
      borderBottom: '1px solid var(--border-faint)',
      flexWrap: 'wrap'
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-display)',
      fontSize: 14,
      fontWeight: 600,
      color: 'var(--text-hi)',
      marginRight: 4
    }
  }, "Route log"), caps.map(c => /*#__PURE__*/React.createElement(FilterChip, {
    key: c,
    label: c.replace('_', ' '),
    active: filter === c,
    onClick: () => setFilter(c)
  }))), /*#__PURE__*/React.createElement("div", {
    style: {
      padding: 8,
      display: 'flex',
      flexDirection: 'column',
      gap: 1
    }
  }, lines.length ? lines.map((l, i) => /*#__PURE__*/React.createElement(RouteLogLine, _extends({
    key: i
  }, l))) : /*#__PURE__*/React.createElement("div", {
    style: {
      padding: 24,
      textAlign: 'center',
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: 'var(--text-faint)'
    }
  }, "No matching calls"))), /*#__PURE__*/React.createElement("div", null, /*#__PURE__*/React.createElement("div", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      fontWeight: 600,
      letterSpacing: '0.12em',
      textTransform: 'uppercase',
      color: 'var(--text-lo)',
      marginBottom: 12
    }
  }, "Usage \xB7 calls per day"), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'grid',
      gridTemplateColumns: 'repeat(auto-fill, minmax(220px,1fr))',
      gap: 14
    }
  }, USAGE.map(u => /*#__PURE__*/React.createElement(Card, {
    key: u.provider,
    pad: 14
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      justifyContent: 'space-between',
      alignItems: 'baseline',
      marginBottom: 8
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: 'var(--text-hi)'
    }
  }, u.provider), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: 'var(--text-faint)'
    }
  }, u.series.reduce((a, b) => a + b, 0), " total")), /*#__PURE__*/React.createElement(Sparkline, {
    data: u.series,
    color: u.color
  })))))), /*#__PURE__*/React.createElement(Card, {
    pad: 16,
    style: {
      position: 'sticky',
      top: 84
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      fontFamily: 'var(--font-display)',
      fontSize: 14,
      fontWeight: 600,
      color: 'var(--text-hi)',
      marginBottom: 4
    }
  }, "Provider health"), /*#__PURE__*/React.createElement("div", {
    style: {
      fontFamily: 'var(--font-ui)',
      fontSize: 12,
      color: 'var(--text-lo)',
      marginBottom: 8
    }
  }, "Last success \xB7 last failover error"), window.FX.health.map(h => /*#__PURE__*/React.createElement(HealthRow, {
    key: h.provider,
    h: h
  }))));
}
window.ActivityTab = ActivityTab;
})(); } catch (e) { __ds_ns.__errors.push({ path: "ui_kits/dashboard/ActivityTab.jsx", error: String((e && e.message) || e) }); }

// ui_kits/dashboard/AddAccountModal.jsx
try { (() => {
/* Add-account modal + guided browser-login flow.
   Key providers → paste API key. Web providers → "Log in with browser" + a
   simulated "Opening Chrome… ✓ session captured" sequence. Success state. */
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
  id: 'jina',
  kind: 'key',
  note: 'Reader · URL → markdown'
}, {
  id: 'firecrawl',
  kind: 'key',
  note: 'Crawl + scrape API'
}, {
  id: 'steel',
  kind: 'key',
  note: 'Headless browser sessions'
}, {
  id: 'perplexity_web',
  kind: 'web',
  note: 'Browser session · search + deep research'
}, {
  id: 'gemini_web',
  kind: 'web',
  note: 'Browser session · search + #dr'
}, {
  id: 'grok_web',
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
function AddAccountModal({
  onClose
}) {
  const [providerId, setProviderId] = React.useState('serper');
  const [label, setLabel] = React.useState('');
  const [apiKey, setApiKey] = React.useState('');
  const [proxy, setProxy] = React.useState('');
  const [touched, setTouched] = React.useState(false);
  const [phase, setPhase] = React.useState('form'); // form | logging-in | success
  const [loginStep, setLoginStep] = React.useState(0);
  const provider = PROVIDER_CATALOG.find(p => p.id === providerId);
  const isWeb = provider.kind === 'web';
  const keyMissing = !isWeb && apiKey.trim().length < 8;

  // Guided login simulation
  React.useEffect(() => {
    if (phase !== 'logging-in') return;
    const steps = [900, 1600, 1200];
    if (loginStep >= steps.length) {
      setPhase('success');
      return;
    }
    const t = setTimeout(() => setLoginStep(s => s + 1), steps[loginStep]);
    return () => clearTimeout(t);
  }, [phase, loginStep]);
  const submitKey = () => {
    setTouched(true);
    if (keyMissing || !label.trim()) return;
    setPhase('success');
  };
  const startLogin = () => {
    setPhase('logging-in');
    setLoginStep(0);
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
    }, "Account added"), /*#__PURE__*/React.createElement("div", {
      style: {
        fontFamily: 'var(--font-mono)',
        fontSize: 13,
        color: 'var(--text-mid)'
      }
    }, /*#__PURE__*/React.createElement("span", {
      style: {
        color: 'var(--lime-500)'
      }
    }, label.trim() || provider.id + '-1'), " is live in the router rotation.")), /*#__PURE__*/React.createElement("div", {
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

  // ---- Guided login ----
  if (phase === 'logging-in') {
    const steps = [{
      label: 'Opening Chrome…',
      done: loginStep > 0
    }, {
      label: `Waiting for you to log in to ${provider.id.replace('_web', '')}…`,
      done: loginStep > 1
    }, {
      label: 'Capturing session cookies…',
      done: loginStep > 2
    }];
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
        marginBottom: 20
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
        display: 'flex',
        flexDirection: 'column',
        gap: 12
      }
    }, steps.map((s, i) => {
      const active = loginStep === i;
      return /*#__PURE__*/React.createElement("div", {
        key: i,
        style: {
          display: 'flex',
          alignItems: 'center',
          gap: 10,
          opacity: i > loginStep ? 0.4 : 1,
          transition: 'opacity 0.2s'
        }
      }, /*#__PURE__*/React.createElement("span", {
        style: {
          width: 18,
          height: 18,
          borderRadius: '50%',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          fontSize: 11,
          background: s.done ? 'var(--green-dim)' : 'transparent',
          color: s.done ? 'var(--green-500)' : 'var(--text-faint)',
          border: `1px solid ${s.done ? 'rgba(70,209,122,0.4)' : active ? 'var(--border-accent)' : 'var(--border-hairline)'}`
        }
      }, s.done ? '✓' : active ? /*#__PURE__*/React.createElement("span", {
        style: {
          width: 6,
          height: 6,
          borderRadius: '50%',
          background: 'var(--lime-500)',
          animation: 'fx-pulse 1.4s infinite'
        }
      }) : ''), /*#__PURE__*/React.createElement("span", {
        style: {
          fontFamily: 'var(--font-mono)',
          fontSize: 13,
          color: s.done ? 'var(--text-mid)' : active ? 'var(--text-hi)' : 'var(--text-lo)'
        }
      }, s.label));
    })))));
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
  }, "Add account"), /*#__PURE__*/React.createElement("button", {
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
    onChange: e => {
      setProviderId(e.target.value);
      setTouched(false);
    }
  }, PROVIDER_CATALOG.map(p => /*#__PURE__*/React.createElement("option", {
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
  }, provider.note))), /*#__PURE__*/React.createElement(Input, {
    label: "Label",
    placeholder: `${provider.id}-1`,
    value: label,
    mono: true,
    onChange: e => setLabel(e.target.value),
    invalid: touched && !label.trim(),
    hint: touched && !label.trim() ? 'Give this account a label' : null
  }), !isWeb ? /*#__PURE__*/React.createElement(Input, {
    label: "API key",
    placeholder: "paste secret key",
    value: apiKey,
    mono: true,
    prefix: "\u2022\u2022",
    type: "password",
    onChange: e => setApiKey(e.target.value),
    invalid: touched && keyMissing,
    hint: touched && keyMissing ? 'Enter a valid API key' : 'Stored locally · never displayed again'
  }) : /*#__PURE__*/React.createElement(Field, {
    label: "Authentication"
  }, /*#__PURE__*/React.createElement(Button, {
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
  }, "Opens Chrome so you can sign in. The session is captured locally \u2014 no password is stored.")), /*#__PURE__*/React.createElement(Input, {
    label: "Proxy \xB7 optional",
    placeholder: "pool, direct, or http://host:port",
    value: proxy,
    mono: true,
    onChange: e => setProxy(e.target.value),
    hint: "Leave blank to use the default proxy pool"
  })), /*#__PURE__*/React.createElement("div", {
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
  }, "Cancel"), !isWeb && /*#__PURE__*/React.createElement(Button, {
    variant: "primary",
    onClick: submitKey
  }, "Add account"))));
}
window.AddAccountModal = AddAccountModal;
})(); } catch (e) { __ds_ns.__errors.push({ path: "ui_kits/dashboard/AddAccountModal.jsx", error: String((e && e.message) || e) }); }

// ui_kits/dashboard/OverviewTab.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
/* Overview: provider grid grouped by capability + pinned live route log. */
const {
  ProviderCard,
  RouteLogLine,
  Card,
  StatusDot,
  Badge
} = window.FetchiraDesignSystem_6526df;
function GroupHeader({
  label,
  count
}) {
  return /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 10,
      marginBottom: 12
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
  }, label), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: 'var(--text-faint)'
    }
  }, count), /*#__PURE__*/React.createElement("span", {
    style: {
      flex: 1,
      height: 1,
      background: 'var(--border-faint)'
    }
  }));
}
function LiveLog() {
  const [lines, setLines] = React.useState(() => window.FX.log.map((l, i) => ({
    ...l,
    _id: i,
    fresh: false
  })));
  const idRef = React.useRef(window.FX.log.length);
  const [paused, setPaused] = React.useState(false);
  React.useEffect(() => {
    if (paused) return;
    const t = setInterval(() => {
      const tpl = window.FX.stream[Math.floor(Math.random() * window.FX.stream.length)];
      const now = new Date();
      const time = now.toTimeString().slice(0, 8);
      const id = idRef.current++;
      setLines(prev => [...prev.slice(-40), {
        ...tpl,
        time,
        _id: id,
        fresh: true
      }]);
    }, 2600);
    return () => clearInterval(t);
  }, [paused]);
  const scrollRef = React.useRef(null);
  React.useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [lines]);
  return /*#__PURE__*/React.createElement(Card, {
    inset: true,
    pad: 0,
    style: {
      display: 'flex',
      flexDirection: 'column',
      height: '100%',
      minHeight: 0,
      overflow: 'hidden'
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'space-between',
      padding: '12px 14px',
      borderBottom: '1px solid var(--border-faint)'
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
      fontSize: 14,
      fontWeight: 600,
      color: 'var(--text-hi)'
    }
  }, "Live route log")), /*#__PURE__*/React.createElement("button", {
    onClick: () => setPaused(p => !p),
    style: {
      background: 'transparent',
      border: '1px solid var(--border-hairline)',
      color: 'var(--text-lo)',
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      padding: '3px 8px',
      borderRadius: 'var(--r-xs)',
      cursor: 'pointer'
    }
  }, paused ? '▶ resume' : '❚❚ pause')), /*#__PURE__*/React.createElement("div", {
    ref: scrollRef,
    style: {
      flex: 1,
      overflowY: 'auto',
      padding: 6,
      display: 'flex',
      flexDirection: 'column',
      gap: 1
    }
  }, lines.map(l => /*#__PURE__*/React.createElement(RouteLogLine, _extends({
    key: l._id
  }, l)))));
}
function OverviewTab() {
  const groups = window.FX.groups;
  return /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'grid',
      gridTemplateColumns: 'minmax(0, 1fr) 380px',
      gap: 20,
      alignItems: 'start',
      height: '100%'
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      flexDirection: 'column',
      gap: 24
    }
  }, groups.map(g => /*#__PURE__*/React.createElement("section", {
    key: g.id
  }, /*#__PURE__*/React.createElement(GroupHeader, {
    label: g.label,
    count: `${g.providers.length} ${g.providers.length === 1 ? 'provider' : 'providers'}`
  }), /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'grid',
      gridTemplateColumns: 'repeat(auto-fill, minmax(290px, 1fr))',
      gap: 14
    }
  }, g.providers.map(p => /*#__PURE__*/React.createElement(ProviderCard, _extends({}, p, {
    key: p.name
  }))))))), /*#__PURE__*/React.createElement("div", {
    style: {
      position: 'sticky',
      top: 84,
      height: 'calc(100vh - 104px)'
    }
  }, /*#__PURE__*/React.createElement(LiveLog, null)));
}
window.OverviewTab = OverviewTab;
})(); } catch (e) { __ds_ns.__errors.push({ path: "ui_kits/dashboard/OverviewTab.jsx", error: String((e && e.message) || e) }); }

// ui_kits/dashboard/TopBar.jsx
try { (() => {
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
    src: "../../assets/logo-mark.svg",
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
  }, "127.0.0.1:7878"));
}
function TopBar({
  onAdd
}) {
  const total = window.FX.totalRemaining;
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
    style: {
      display: 'flex',
      alignItems: 'center',
      gap: 14
    }
  }, /*#__PURE__*/React.createElement("div", {
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
  }, "12"), /*#__PURE__*/React.createElement("span", {
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
  }), /*#__PURE__*/React.createElement("span", null, "10 healthy"), /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--border-strong)'
    }
  }, "\xB7"), /*#__PURE__*/React.createElement(StatusDot, {
    tone: "off",
    size: 6
  }), /*#__PURE__*/React.createElement("span", null, "2 need login"), /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--border-strong)'
    }
  }, "\xB7"), /*#__PURE__*/React.createElement(StatusDot, {
    tone: "out",
    size: 6
  }), /*#__PURE__*/React.createElement("span", null, "1 exhausted")), /*#__PURE__*/React.createElement("div", {
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
})(); } catch (e) { __ds_ns.__errors.push({ path: "ui_kits/dashboard/TopBar.jsx", error: String((e && e.message) || e) }); }

// ui_kits/dashboard/data.js
try { (() => {
/* fetchira dashboard — mock data (global, loaded before the screens). */
window.FX = function () {
  // Provider tiles for the Overview, grouped by capability.
  const groups = [{
    id: 'search',
    label: 'Search',
    providers: [{
      name: 'serper',
      desc: 'Web search API',
      used: 423,
      quota: 2500,
      resetWindow: 'lifetime',
      resetsIn: null,
      accounts: 1,
      key: true
    }, {
      name: 'tavily',
      desc: 'Search + extract API',
      used: 36,
      quota: 1000,
      resetWindow: 'monthly',
      resetsIn: '12d',
      accounts: 1,
      key: true
    }, {
      name: 'exa',
      desc: 'Neural search API',
      used: 92,
      quota: 2000,
      resetWindow: 'monthly',
      resetsIn: '12d',
      accounts: 2,
      key: true
    }, {
      name: 'parallel',
      desc: 'Search API',
      used: 0,
      quota: 16000,
      resetWindow: 'monthly',
      resetsIn: '12d',
      accounts: 1,
      key: true
    }]
  }, {
    id: 'read',
    label: 'Read / scrape',
    providers: [{
      name: 'jina',
      desc: 'Reader — URL → markdown',
      used: 533,
      quota: 1000000,
      resetWindow: 'monthly',
      resetsIn: '12d',
      accounts: 1,
      key: true
    }, {
      name: 'firecrawl',
      desc: 'Crawl + scrape API',
      used: 6,
      quota: 1000,
      resetWindow: 'monthly',
      resetsIn: '12d',
      accounts: 1,
      key: true
    }]
  }, {
    id: 'browser',
    label: 'Browser',
    providers: [{
      name: 'steel',
      desc: 'Headless browser sessions',
      used: 0,
      quota: 360000,
      resetWindow: 'monthly',
      resetsIn: '12d',
      accounts: 1,
      key: true
    }]
  }, {
    id: 'web',
    label: 'Web sessions',
    providers: [{
      name: 'perplexity_web',
      desc: 'Browser session · search + deep research',
      used: 300,
      quota: 300,
      resetWindow: 'monthly',
      resetsIn: '3d',
      accounts: 1,
      webSession: true,
      loggedIn: true,
      dr: {
        used: 0,
        quota: 5
      }
    }, {
      name: 'gemini_web',
      desc: 'Browser session · search + #dr',
      used: 10,
      quota: 1000,
      resetWindow: 'monthly',
      resetsIn: '11d',
      accounts: 1,
      webSession: true,
      loggedIn: true,
      dr: {
        used: 0,
        quota: 10
      }
    }, {
      name: 'grok_web',
      desc: 'Browser session · search + #dr',
      used: 7,
      quota: 100,
      resetWindow: 'monthly',
      resetsIn: '11d',
      accounts: 1,
      webSession: true,
      loggedIn: false,
      dr: {
        used: 0,
        quota: 3
      }
    }]
  }];

  // Accounts table rows.
  const accounts = [{
    provider: 'serper',
    label: 'serper-1',
    used: 423,
    quota: 2500,
    resetWindow: 'lifetime',
    proxy: 'direct',
    status: 'healthy',
    key: true,
    web: false
  }, {
    provider: 'tavily',
    label: 'tavily-1',
    used: 36,
    quota: 1000,
    resetWindow: 'monthly',
    proxy: '45.38.78.x:6184',
    status: 'healthy',
    key: true,
    web: false
  }, {
    provider: 'exa',
    label: 'exa-1',
    used: 46,
    quota: 1000,
    resetWindow: 'monthly',
    proxy: 'direct',
    status: 'healthy',
    key: true,
    web: false
  }, {
    provider: 'exa',
    label: 'exa-2',
    used: 46,
    quota: 1000,
    resetWindow: 'monthly',
    proxy: '45.38.91.x:6184',
    status: 'healthy',
    key: true,
    web: false
  }, {
    provider: 'jina',
    label: 'jina-1',
    used: 533,
    quota: 1000000,
    resetWindow: 'monthly',
    proxy: 'direct',
    status: 'healthy',
    key: true,
    web: false
  }, {
    provider: 'firecrawl',
    label: 'firecrawl-1',
    used: 6,
    quota: 1000,
    resetWindow: 'monthly',
    proxy: 'direct',
    status: 'healthy',
    key: true,
    web: false
  }, {
    provider: 'parallel',
    label: 'parallel-1',
    used: 0,
    quota: 16000,
    resetWindow: 'monthly',
    proxy: 'direct',
    status: 'healthy',
    key: true,
    web: false
  }, {
    provider: 'steel',
    label: 'steel-1',
    used: 0,
    quota: 360000,
    resetWindow: 'monthly',
    proxy: 'pool',
    status: 'healthy',
    key: true,
    web: false
  }, {
    provider: 'gemini_web',
    label: 'gemini-1',
    used: 10,
    quota: 1000,
    resetWindow: 'monthly',
    proxy: 'direct',
    status: 'healthy',
    key: false,
    web: true,
    loggedIn: true
  }, {
    provider: 'perplexity_web',
    label: 'perplexity-1',
    used: 300,
    quota: 300,
    resetWindow: 'monthly',
    proxy: 'direct',
    status: 'exhausted',
    key: false,
    web: true,
    loggedIn: true
  }, {
    provider: 'grok_web',
    label: 'grok-1',
    used: 7,
    quota: 100,
    resetWindow: 'monthly',
    proxy: 'direct',
    status: 'needs-login',
    key: false,
    web: true,
    loggedIn: false
  }, {
    provider: 'gemini_web',
    label: 'gemini-2',
    used: 0,
    quota: 1000,
    resetWindow: 'monthly',
    proxy: 'pool',
    status: 'needs-login',
    key: false,
    web: true,
    loggedIn: false
  }];

  // Seed route-log lines (most recent last). Used by the live feed + Activity.
  const log = [{
    time: '14:21:48',
    capability: 'search',
    provider: 'serper',
    account: 1,
    status: 200,
    latencyMs: 198
  }, {
    time: '14:21:50',
    capability: 'read',
    provider: 'jina',
    account: 1,
    status: 200,
    latencyMs: 612
  }, {
    time: '14:21:52',
    capability: 'search',
    provider: 'tavily',
    account: 1,
    status: 200,
    latencyMs: 243
  }, {
    time: '14:21:55',
    capability: 'search',
    failover: {
      from: 'exa-1',
      code: 429,
      to: 'tavily-1'
    },
    status: 200,
    latencyMs: 312
  }, {
    time: '14:21:58',
    capability: 'browser',
    provider: 'steel',
    account: 1,
    status: 200,
    latencyMs: 1430
  }, {
    time: '14:22:01',
    capability: 'deep_research',
    provider: 'gemini',
    account: 1,
    status: 200,
    latencyMs: 4820
  }, {
    time: '14:22:04',
    capability: 'search',
    provider: 'exa',
    account: 2,
    status: 200,
    latencyMs: 276
  }, {
    time: '14:22:06',
    capability: 'read',
    provider: 'firecrawl',
    account: 1,
    status: 200,
    latencyMs: 905
  }, {
    time: '14:22:09',
    capability: 'search',
    failover: {
      from: 'perplexity-1',
      code: 503,
      to: 'serper-1'
    },
    status: 200,
    latencyMs: 221
  }, {
    time: '14:22:12',
    capability: 'search',
    provider: 'parallel',
    account: 1,
    status: 200,
    latencyMs: 188
  }];

  // Candidates for streaming new lines into the live feed.
  const stream = [{
    capability: 'search',
    provider: 'serper',
    account: 1,
    status: 200,
    latencyMs: 201
  }, {
    capability: 'read',
    provider: 'jina',
    account: 1,
    status: 200,
    latencyMs: 588
  }, {
    capability: 'search',
    provider: 'tavily',
    account: 1,
    status: 200,
    latencyMs: 267
  }, {
    capability: 'search',
    failover: {
      from: 'exa-1',
      code: 429,
      to: 'tavily-1'
    },
    status: 200,
    latencyMs: 334
  }, {
    capability: 'browser',
    provider: 'steel',
    account: 1,
    status: 200,
    latencyMs: 1622
  }, {
    capability: 'deep_research',
    provider: 'grok',
    account: 1,
    status: 200,
    latencyMs: 5210
  }, {
    capability: 'search',
    provider: 'exa',
    account: 2,
    status: 200,
    latencyMs: 254
  }, {
    capability: 'read',
    provider: 'firecrawl',
    account: 1,
    status: 200,
    latencyMs: 844
  }];

  // Per-provider health for the Activity tab.
  const health = [{
    provider: 'serper-1',
    state: 'healthy',
    lastSuccess: '2s ago',
    lastError: null
  }, {
    provider: 'tavily-1',
    state: 'healthy',
    lastSuccess: '6s ago',
    lastError: null
  }, {
    provider: 'exa-1',
    state: 'healthy',
    lastSuccess: '19s ago',
    lastError: '429 rate_limited — failed over to tavily-1'
  }, {
    provider: 'exa-2',
    state: 'healthy',
    lastSuccess: '4s ago',
    lastError: null
  }, {
    provider: 'jina-1',
    state: 'healthy',
    lastSuccess: '8s ago',
    lastError: null
  }, {
    provider: 'parallel-1',
    state: 'healthy',
    lastSuccess: '1s ago',
    lastError: null
  }, {
    provider: 'perplexity-1',
    state: 'exhausted',
    lastSuccess: '2h ago',
    lastError: '503 quota_exhausted — 300/300 monthly, resets in 3d'
  }, {
    provider: 'grok-1',
    state: 'needs-login',
    lastSuccess: '1d ago',
    lastError: 'session expired — browser login required'
  }];
  const totalRemaining = groups.flatMap(g => g.providers).reduce((s, p) => s + Math.max(0, p.quota - p.used), 0);
  return {
    groups,
    accounts,
    log,
    stream,
    health,
    totalRemaining
  };
}();
})(); } catch (e) { __ds_ns.__errors.push({ path: "ui_kits/dashboard/data.js", error: String((e && e.message) || e) }); }

__ds_ns.Badge = __ds_scope.Badge;

__ds_ns.Button = __ds_scope.Button;

__ds_ns.Card = __ds_scope.Card;

__ds_ns.StatusDot = __ds_scope.StatusDot;

__ds_ns.RouteLogLine = __ds_scope.RouteLogLine;

__ds_ns.Input = __ds_scope.Input;

__ds_ns.Select = __ds_scope.Select;

__ds_ns.QuotaMeter = __ds_scope.QuotaMeter;

__ds_ns.Tabs = __ds_scope.Tabs;

__ds_ns.ProviderCard = __ds_scope.ProviderCard;

})();
