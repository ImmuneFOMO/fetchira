/* fetchira dashboard — mock data (global, loaded before the screens). */
window.FX = (function () {
  // Provider tiles for the Overview, grouped by capability.
  const groups = [
    {
      id: 'search', label: 'Search', providers: [
        { name: 'serper', desc: 'Web search API', used: 423, quota: 2500, resetWindow: 'lifetime', resetsIn: null, accounts: 1, key: true },
        { name: 'tavily', desc: 'Search + extract API', used: 36, quota: 1000, resetWindow: 'monthly', resetsIn: '12d', accounts: 1, key: true },
        { name: 'exa', desc: 'Neural search API', used: 92, quota: 2000, resetWindow: 'monthly', resetsIn: '12d', accounts: 2, key: true },
        { name: 'parallel', desc: 'Search API', used: 0, quota: 16000, resetWindow: 'monthly', resetsIn: '12d', accounts: 1, key: true },
      ],
    },
    {
      id: 'read', label: 'Read / scrape', providers: [
        { name: 'firecrawl', desc: 'Crawl + scrape API', used: 6, quota: 1000, resetWindow: 'monthly', resetsIn: '12d', accounts: 1, key: true },
      ],
    },
    {
      id: 'browser', label: 'Browser', providers: [
        { name: 'steel', desc: 'Headless browser sessions', used: 0, quota: 360000, resetWindow: 'monthly', resetsIn: '12d', accounts: 1, key: true },
      ],
    },
    {
      id: 'web', label: 'Web sessions', providers: [
        { name: 'gemini_web', desc: 'Browser session · search + #dr', used: 10, quota: 1000, resetWindow: 'monthly', resetsIn: '11d', accounts: 1, webSession: true, loggedIn: true, dr: { used: 0, quota: 10 } },
        { name: 'grok_web', desc: 'Browser session · search + #dr', used: 7, quota: 100, resetWindow: 'monthly', resetsIn: '11d', accounts: 1, webSession: true, loggedIn: false, dr: { used: 0, quota: 3 } },
      ],
    },
  ];

  // Accounts table rows.
  const accounts = [
    { provider: 'serper', label: 'serper-1', used: 423, quota: 2500, resetWindow: 'lifetime', proxy: 'direct', status: 'healthy', key: true, web: false },
    { provider: 'tavily', label: 'tavily-1', used: 36, quota: 1000, resetWindow: 'monthly', proxy: '45.38.78.x:6184', status: 'healthy', key: true, web: false },
    { provider: 'exa', label: 'exa-1', used: 46, quota: 1000, resetWindow: 'monthly', proxy: 'direct', status: 'healthy', key: true, web: false },
    { provider: 'exa', label: 'exa-2', used: 46, quota: 1000, resetWindow: 'monthly', proxy: '45.38.91.x:6184', status: 'healthy', key: true, web: false },
    { provider: 'firecrawl', label: 'firecrawl-1', used: 6, quota: 1000, resetWindow: 'monthly', proxy: 'direct', status: 'healthy', key: true, web: false },
    { provider: 'parallel', label: 'parallel-1', used: 0, quota: 16000, resetWindow: 'monthly', proxy: 'direct', status: 'healthy', key: true, web: false },
    { provider: 'steel', label: 'steel-1', used: 0, quota: 360000, resetWindow: 'monthly', proxy: 'pool', status: 'healthy', key: true, web: false },
    { provider: 'gemini_web', label: 'gemini-1', used: 10, quota: 1000, resetWindow: 'monthly', proxy: 'direct', status: 'healthy', key: false, web: true, loggedIn: true },
    { provider: 'grok_web', label: 'grok-1', used: 7, quota: 100, resetWindow: 'monthly', proxy: 'direct', status: 'needs-login', key: false, web: true, loggedIn: false },
    { provider: 'gemini_web', label: 'gemini-2', used: 0, quota: 1000, resetWindow: 'monthly', proxy: 'pool', status: 'needs-login', key: false, web: true, loggedIn: false },
  ];

  // Seed route-log lines (most recent last). Used by the live feed + Activity.
  const log = [
    { time: '14:21:48', capability: 'search', provider: 'serper', account: 1, status: 200, latencyMs: 198 },
    { time: '14:21:52', capability: 'search', provider: 'tavily', account: 1, status: 200, latencyMs: 243 },
    { time: '14:21:55', capability: 'search', failover: { from: 'exa-1', code: 429, to: 'tavily-1' }, status: 200, latencyMs: 312 },
    { time: '14:21:58', capability: 'browser', provider: 'steel', account: 1, status: 200, latencyMs: 1430 },
    { time: '14:22:01', capability: 'deep_research', provider: 'gemini', account: 1, status: 200, latencyMs: 4820 },
    { time: '14:22:04', capability: 'search', provider: 'exa', account: 2, status: 200, latencyMs: 276 },
    { time: '14:22:06', capability: 'read', provider: 'firecrawl', account: 1, status: 200, latencyMs: 905 },
    { time: '14:22:12', capability: 'search', provider: 'parallel', account: 1, status: 200, latencyMs: 188 },
  ];

  // Candidates for streaming new lines into the live feed.
  const stream = [
    { capability: 'search', provider: 'serper', account: 1, status: 200, latencyMs: 201 },
    { capability: 'search', provider: 'tavily', account: 1, status: 200, latencyMs: 267 },
    { capability: 'search', failover: { from: 'exa-1', code: 429, to: 'tavily-1' }, status: 200, latencyMs: 334 },
    { capability: 'browser', provider: 'steel', account: 1, status: 200, latencyMs: 1622 },
    { capability: 'deep_research', provider: 'grok', account: 1, status: 200, latencyMs: 5210 },
    { capability: 'search', provider: 'exa', account: 2, status: 200, latencyMs: 254 },
    { capability: 'read', provider: 'firecrawl', account: 1, status: 200, latencyMs: 844 },
  ];

  // Per-provider health for the Activity tab.
  const health = [
    { provider: 'serper-1', state: 'healthy', lastSuccess: '2s ago', lastError: null },
    { provider: 'tavily-1', state: 'healthy', lastSuccess: '6s ago', lastError: null },
    { provider: 'exa-1', state: 'healthy', lastSuccess: '19s ago', lastError: '429 rate_limited — failed over to tavily-1' },
    { provider: 'exa-2', state: 'healthy', lastSuccess: '4s ago', lastError: null },
    { provider: 'parallel-1', state: 'healthy', lastSuccess: '1s ago', lastError: null },
    { provider: 'grok-1', state: 'needs-login', lastSuccess: '1d ago', lastError: 'session expired — browser login required' },
  ];

  const totalRemaining = groups.flatMap(g => g.providers).reduce((s, p) => s + Math.max(0, p.quota - p.used), 0);

  return { groups, accounts, log, stream, health, totalRemaining };
})();
