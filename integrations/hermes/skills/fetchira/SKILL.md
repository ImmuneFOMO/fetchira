---
name: fetchira
description: Web search, page reading and deep research through the fetchira MCP server, which sends each call to a provider the user has quota on. Use for current information, reading a known URL, cited multi-source research, checking provider quota, or when the user wants to add a search provider key.
---

# Fetchira

The fetchira server gives Hermes five tools, listed as `mcp__fetchira__<tool>`: `search`, `read`, `deep_research`, `browser` and `usage`. Each call goes to the first provider in fetchira's priority order that has quota left and moves on to the next one after an error. Pass `provider` only when the user asks for a specific backend, because a forced provider has no fallback.

Hermes' own `web_search` and `web_extract` may also be on. Use the fetchira tools when the user asks for fetchira or one of its providers, for `deep_research` and `browser`, or when the built-in tools fail.

## Which tool to call

- `search` for current facts and sources. It returns ranked title, URL and snippet results. Narrow it with `topic` (`web`, `news` or `academic`), `recency` (`day`, `week`, `month`, `year` or an ISO date) and `domains` (a `-` prefix excludes a domain).
- `read` when the URL is known. It returns the page as Markdown.
- `deep_research` when coverage and citations matter more than speed. It needs a key for Parallel, Exa, Tavily, Tinyfish, Linkup or You.com. `depth: "deep"` is slower and may spend a paid balance. A run can take several minutes, so say so before starting one. Hermes stops waiting after 7 minutes.
- `browser` for a page that needs JavaScript. It needs a Hyperbrowser, Browserless or Steel key.
- `usage` for the quota left per provider. With `provider` set it returns that backend's options and example calls; read it before passing a provider-specific `mode`.

Cite only URLs that came back from a successful call. An empty result or an error is not a source; report the provider's error as it came.

## Setup

The user installs the fetchira binary, version 0.1.24 or newer, with one of the commands at https://github.com/ImmuneFOMO/fetchira/blob/main/docs/setup.md#install. Do not download or install it yourself. Hermes offers these tools only while `fetchira` is on its PATH, so when they are missing, send the user to that page.

With no keys, `search` uses Keenable (OpenAlex with `topic: "academic"`, GDELT with `topic: "news"`) and `read` uses Jina Reader and Keenable. They are free and rate-limited. Each key adds a provider and its quota.

## Adding a provider key

When the user wants another provider, give them its variable and signup page from the table and ask them to save the key in Hermes:

- Hermes Desktop: Settings → Plugins, open fetchira and paste the key into that provider's field. Hermes reconnects the server after the save.
- Terminal: the user runs `hermes config set <VARIABLE> <key>` in their own terminal, then restarts Hermes or runs `/reload-mcp`.

Never ask for a key in chat, and never put a key into a command or tool call yourself.

| Provider | Variable | Signup | Tools |
|---|---|---|---|
| Serper | `SERPER_API_KEY` | https://serper.dev | search, read |
| Tavily | `TAVILY_API_KEY` | https://app.tavily.com | search, read, deep_research |
| Exa | `EXA_API_KEY` | https://exa.ai | search, read, deep_research |
| Firecrawl | `FIRECRAWL_API_KEY` | https://firecrawl.dev | search, read |
| Parallel | `PARALLEL_API_KEY` | https://parallel.ai | search, deep_research |
| Steel | `STEEL_API_KEY` | https://steel.dev | browser, read |
| Tinyfish | `TINYFISH_API_KEY` | https://tinyfish.ai | search, read, deep_research |
| Linkup | `LINKUP_API_KEY` | https://app.linkup.so | search, read, deep_research |
| You.com | `YOU_API_KEY` | https://you.com/platform | search, read, deep_research |
| Octen | `OCTEN_API_KEY` | https://octen.ai/platform | search |
| Nimble | `NIMBLE_API_KEY` | https://online.nimbleway.com | search |
| SerpApi | `SERPAPI_API_KEY` | https://serpapi.com | search |
| LangSearch | `LANGSEARCH_API_KEY` | https://langsearch.com | search |
| ScrapingAnt | `SCRAPINGANT_API_KEY` | https://app.scrapingant.com | read |
| ZenRows | `ZENROWS_API_KEY` | https://www.zenrows.com | read |
| Hyperbrowser | `HYPERBROWSER_API_KEY` | https://hyperbrowser.ai | browser, read |
| Browserless | `BROWSERLESS_API_KEY` | https://www.browserless.io | browser, read |

## Check

After setup or a new key:

1. Call `usage` and confirm the provider is listed with quota left.
2. Make one small call. After a new key, set `provider` to that provider: `search` with `query: "fetchira"` and `max_results: 3` for a search provider, `read` of `https://example.com` for any other. With no keys, run the same `search` without `provider`.
