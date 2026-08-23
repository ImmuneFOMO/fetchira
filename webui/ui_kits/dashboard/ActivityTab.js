(function () {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
function ownKeys(e, r) { var t = Object.keys(e); if (Object.getOwnPropertySymbols) { var o = Object.getOwnPropertySymbols(e); r && (o = o.filter(function (r) { return Object.getOwnPropertyDescriptor(e, r).enumerable; })), t.push.apply(t, o); } return t; }
function _objectSpread(e) { for (var r = 1; r < arguments.length; r++) { var t = null != arguments[r] ? arguments[r] : {}; r % 2 ? ownKeys(Object(t), !0).forEach(function (r) { _defineProperty(e, r, t[r]); }) : Object.getOwnPropertyDescriptors ? Object.defineProperties(e, Object.getOwnPropertyDescriptors(t)) : ownKeys(Object(t)).forEach(function (r) { Object.defineProperty(e, r, Object.getOwnPropertyDescriptor(t, r)); }); } return e; }
function _defineProperty(e, r, t) { return (r = _toPropertyKey(r)) in e ? Object.defineProperty(e, r, { value: t, enumerable: !0, configurable: !0, writable: !0 }) : e[r] = t, e; }
function _toPropertyKey(t) { var i = _toPrimitive(t, "string"); return "symbol" == typeof i ? i : i + ""; }
function _toPrimitive(t, r) { if ("object" != typeof t || !t) return t; var e = t[Symbol.toPrimitive]; if (void 0 !== e) { var i = e.call(t, r || "default"); if ("object" != typeof i) return i; throw new TypeError("@@toPrimitive must return a primitive value."); } return ("string" === r ? String : Number)(t); }
/* Activity: one stream for everything the router did — every attempt (success or failure) with
   a response preview, expandable to the full request/response/error and raw HTTP. Filterable by
   capability / errors, live-tailing, with usage sparklines and provider health alongside.
   (Absorbed the old Debug tab — same data, one place.) */
const {
  Card,
  Badge,
  StatusDot,
  RouteLogLine
} = window.FetchiraDesignSystem_6526df;
const CAP_COLOR = {
  search: 'var(--lime-500)',
  read: 'var(--cyan-500)',
  deep_research: '#C792EA',
  browser: 'var(--amber-500)',
  create_image: 'var(--amber-500)'
};
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
function pretty(s) {
  try {
    return JSON.stringify(JSON.parse(s), null, 2);
  } catch (e) {
    return s;
  }
}

// Headers/body may arrive as a JSON object (header map) or a plain string; render both readably.
function fmtTrace(v) {
  if (v == null) return '—';
  if (typeof v === 'string') return v || '—';
  return JSON.stringify(v, null, 2);
}
function RawHttp({
  trace,
  pre,
  cap
}) {
  const [open, setOpen] = React.useState(false);
  if (!Array.isArray(trace) || !trace.length) return null;
  const sub = _objectSpread(_objectSpread({}, cap), {}, {
    marginTop: 6
  });
  return /*#__PURE__*/React.createElement("div", null, /*#__PURE__*/React.createElement("div", {
    onClick: () => setOpen(o => !o),
    style: _objectSpread(_objectSpread({}, cap), {}, {
      marginBottom: open ? 6 : 0,
      cursor: 'pointer'
    })
  }, open ? '▾' : '▸', " raw HTTP"), open && trace.map((rt, i) => /*#__PURE__*/React.createElement("div", {
    key: i,
    style: {
      display: 'flex',
      flexDirection: 'column',
      gap: 4,
      marginBottom: 10
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: 'var(--text-hi)',
      wordBreak: 'break-all'
    }
  }, rt.method, " ", rt.url, " \u2192 ", rt.status), /*#__PURE__*/React.createElement("div", {
    style: sub
  }, "req headers"), /*#__PURE__*/React.createElement("pre", {
    style: pre
  }, fmtTrace(rt.reqHeaders)), /*#__PURE__*/React.createElement("div", {
    style: sub
  }, "req body"), /*#__PURE__*/React.createElement("pre", {
    style: pre
  }, fmtTrace(rt.reqBody)), /*#__PURE__*/React.createElement("div", {
    style: sub
  }, "resp headers"), /*#__PURE__*/React.createElement("pre", {
    style: pre
  }, fmtTrace(rt.respHeaders)), /*#__PURE__*/React.createElement("div", {
    style: sub
  }, "resp body"), /*#__PURE__*/React.createElement("pre", {
    style: _objectSpread(_objectSpread({}, pre), {}, {
      maxHeight: 320,
      overflowY: 'auto'
    })
  }, fmtTrace(rt.respBody)))));
}
function AttemptDetail({
  ok,
  full
}) {
  if (!full) return /*#__PURE__*/React.createElement("div", {
    style: {
      padding: '6px 10px',
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: 'var(--text-faint)'
    }
  }, "loading\u2026");
  const body = full.response != null ? full.response : full.error;
  const label = full.response != null ? 'response' : 'error';
  const pre = {
    fontFamily: 'var(--font-mono)',
    fontSize: 12,
    whiteSpace: 'pre-wrap',
    wordBreak: 'break-word',
    margin: 0,
    padding: 10,
    background: 'var(--surface-well)',
    borderRadius: 'var(--r-xs)',
    color: 'var(--text-mid)'
  };
  const cap = {
    fontFamily: 'var(--font-mono)',
    fontSize: 10,
    letterSpacing: '0.1em',
    textTransform: 'uppercase',
    color: 'var(--text-faint)',
    marginBottom: 4
  };
  return /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'flex',
      flexDirection: 'column',
      gap: 8,
      padding: '2px 10px 10px'
    }
  }, /*#__PURE__*/React.createElement("div", null, /*#__PURE__*/React.createElement("div", {
    style: cap
  }, "request"), /*#__PURE__*/React.createElement("pre", {
    style: pre
  }, pretty(full.request))), /*#__PURE__*/React.createElement("div", null, /*#__PURE__*/React.createElement("div", {
    style: cap
  }, label), /*#__PURE__*/React.createElement("pre", {
    style: _objectSpread(_objectSpread({}, pre), {}, {
      maxHeight: 320,
      overflowY: 'auto',
      borderLeft: ok ? 'none' : '2px solid var(--red-500)'
    })
  }, body != null ? body : '—')), /*#__PURE__*/React.createElement(RawHttp, {
    trace: full.httpTrace,
    pre: pre,
    cap: cap
  }));
}
function AttemptRow({
  row,
  open,
  full,
  onToggle
}) {
  const capColor = CAP_COLOR[row.capability] || 'var(--text-mid)';
  return /*#__PURE__*/React.createElement("div", {
    style: {
      borderRadius: 'var(--r-xs)',
      background: open ? 'var(--surface-2)' : row.fresh ? 'var(--surface-2)' : 'transparent',
      animation: row.fresh ? 'fx-log-in var(--dur-mid) var(--ease-out)' : 'none'
    }
  }, /*#__PURE__*/React.createElement("div", {
    onClick: onToggle,
    style: {
      cursor: 'pointer',
      padding: '5px 10px'
    }
  }, /*#__PURE__*/React.createElement("div", {
    style: {
      display: 'grid',
      gridTemplateColumns: '10px auto 100px 1fr auto',
      alignItems: 'center',
      gap: 10,
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      lineHeight: 1.2
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      fontSize: 10,
      color: 'var(--text-faint)'
    }
  }, open ? '▾' : '▸'), /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--text-faint)'
    }
  }, row.time), /*#__PURE__*/React.createElement("span", {
    style: {
      color: capColor,
      fontWeight: 500
    }
  }, row.capability === 'create_image' ? 'image' : row.capability.replace('_', ' ')), /*#__PURE__*/React.createElement("span", {
    style: {
      color: 'var(--text-hi)',
      overflow: 'hidden',
      textOverflow: 'ellipsis',
      whiteSpace: 'nowrap'
    }
  }, row.provider, row.account != null ? `-${row.account}` : ''), /*#__PURE__*/React.createElement("span", {
    style: {
      display: 'inline-flex',
      alignItems: 'center',
      gap: 8,
      justifySelf: 'end'
    }
  }, /*#__PURE__*/React.createElement("span", {
    style: {
      color: row.ok ? 'var(--text-faint)' : 'var(--red-500)'
    }
  }, row.status), /*#__PURE__*/React.createElement("span", {
    style: {
      color: row.latencyMs > 800 ? 'var(--amber-500)' : 'var(--text-lo)',
      minWidth: 48,
      textAlign: 'right'
    }
  }, row.latencyMs, "ms"))), row.preview && /*#__PURE__*/React.createElement("div", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: row.ok ? 'var(--text-lo)' : 'var(--red-500)',
      marginTop: 3,
      paddingLeft: 20,
      overflow: 'hidden',
      textOverflow: 'ellipsis',
      whiteSpace: 'nowrap'
    }
  }, row.preview)), open && /*#__PURE__*/React.createElement(AttemptDetail, {
    ok: row.ok,
    full: full
  }));
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
  const [rows, setRows] = React.useState([]);
  const [loaded, setLoaded] = React.useState(false);
  const [paused, setPaused] = React.useState(false);
  const [openId, setOpenId] = React.useState(null);
  const [details, setDetails] = React.useState({});
  const lastId = React.useRef(0);
  const fetchRows = async after => {
    try {
      return await window.apiGet(`/api/debug?after=${after}&limit=200`);
    } catch (e) {/* offline / opened as a static file */}
  };
  React.useEffect(() => {
    let alive = true;
    (async () => {
      const d = await fetchRows(0);
      if (!alive) return;
      if (d) {
        lastId.current = d.maxId;
        setRows(d.rows.slice().reverse().map(r => _objectSpread(_objectSpread({}, r), {}, {
          fresh: false
        }))); // newest first
      }
      setLoaded(true);
    })();
    return () => {
      alive = false;
    };
  }, []);
  React.useEffect(() => {
    if (paused) return;
    const id = setInterval(async () => {
      const d = await fetchRows(lastId.current);
      if (!d || !d.rows.length) return;
      lastId.current = d.maxId;
      setRows(prev => [...d.rows.slice().reverse().map(r => _objectSpread(_objectSpread({}, r), {}, {
        fresh: true
      })), ...prev].slice(0, 300));
    }, 1500);
    return () => clearInterval(id);
  }, [paused]);
  const toggle = async row => {
    const closing = openId === row.id;
    setOpenId(closing ? null : row.id);
    if (closing || details[row.id]) return;
    try {
      const full = await window.apiGet(`/api/debug/${row.id}`);
      setDetails(prev => _objectSpread(_objectSpread({}, prev), {}, {
        [row.id]: full
      }));
    } catch (e) {/* offline */}
  };
  const caps = ['all', 'search', 'read', 'deep_research', 'browser', 'image', 'errors'];
  const shown = rows.filter(r => filter === 'all' ? true : filter === 'errors' ? !r.ok : filter === 'image' ? r.capability === 'create_image' : r.capability === filter);
  // Full capture disabled (debug_log.enabled = false) or already expired: fall back to the
  // route-log snapshot so the tab still tells the routing story.
  const fallback = loaded && !rows.length && (window.FX.log || []).length > 0;
  const usage = window.FX.usage && window.FX.usage.length ? window.FX.usage : [];
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
  }, /*#__PURE__*/React.createElement(StatusDot, {
    tone: paused ? 'off' : 'accent',
    pulse: !paused,
    size: 7
  }), /*#__PURE__*/React.createElement("span", {
    style: {
      fontFamily: 'var(--font-display)',
      fontSize: 14,
      fontWeight: 600,
      color: 'var(--text-hi)',
      marginRight: 4
    }
  }, "Activity"), caps.map(c => /*#__PURE__*/React.createElement(FilterChip, {
    key: c,
    label: c.replace('_', ' '),
    active: filter === c,
    onClick: () => setFilter(c)
  })), /*#__PURE__*/React.createElement("span", {
    style: {
      flex: 1
    }
  }), /*#__PURE__*/React.createElement("button", {
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
    style: {
      padding: 8,
      display: 'flex',
      flexDirection: 'column',
      gap: 1,
      maxHeight: 'calc(100vh - 230px)',
      overflowY: 'auto'
    }
  }, fallback ? /*#__PURE__*/React.createElement(React.Fragment, null, /*#__PURE__*/React.createElement("div", {
    style: {
      padding: '4px 10px 10px',
      fontFamily: 'var(--font-mono)',
      fontSize: 11,
      color: 'var(--text-faint)'
    }
  }, window.fxHosted ? 'Detailed capture is disabled or expired on this server' : 'Full capture is off or expired (debug_log in fetchira.toml)', " \u2014 showing the routed-call log"), window.FX.log.map((l, i) => /*#__PURE__*/React.createElement(RouteLogLine, _extends({
    key: i
  }, l)))) : shown.length ? shown.map(row => /*#__PURE__*/React.createElement(AttemptRow, {
    key: row.id,
    row: row,
    open: openId === row.id,
    full: details[row.id],
    onToggle: () => toggle(row)
  })) : /*#__PURE__*/React.createElement("div", {
    style: {
      padding: 24,
      textAlign: 'center',
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: 'var(--text-faint)'
    }
  }, loaded ? 'No matching calls yet' : 'loading…'))), /*#__PURE__*/React.createElement("div", null, /*#__PURE__*/React.createElement("div", {
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
  }, usage.length ? usage.map(u => /*#__PURE__*/React.createElement(Card, {
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
  }))) : /*#__PURE__*/React.createElement("div", {
    style: {
      fontFamily: 'var(--font-mono)',
      fontSize: 12,
      color: 'var(--text-faint)',
      padding: '8px 2px'
    }
  }, "No calls recorded yet \u2014 usage fills in as the router serves requests.")))), /*#__PURE__*/React.createElement(Card, {
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
})();
