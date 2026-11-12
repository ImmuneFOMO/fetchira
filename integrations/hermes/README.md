# Fetchira for Hermes

Web search, page reading and deep research for Hermes Agent through fetchira, which routes each call across the provider keys you save and four providers that need no key. The plugin starts the [fetchira](https://github.com/ImmuneFOMO/fetchira) binary as a local MCP server and adds a skill that tells the agent which tool to call.

## What it adds

Fetchira puts 21 search and scraping APIs behind five tools: `search`, `read`, `deep_research`, `browser` and `usage`. Keenable, Jina Reader, OpenAlex and GDELT need no key, so search and read work right after install. The other 17 providers start working once you save their key.

Fetchira counts each provider's quota over that provider's own period: a day, a month, or once for one-time credits. When a provider reports its real remaining balance, fetchira routes on that number. A call goes to the first provider in priority order with quota left and moves to the next one after an error. `usage` shows what is left.

Deep research runs through Parallel, Exa, Tavily, Tinyfish, Linkup or You.com. Pages that need JavaScript load through Hyperbrowser, Browserless or Steel.

Hermes' own web backends also cover Tavily, Exa, Parallel, Firecrawl and Keenable. The other 16 providers are only in fetchira.

## Install

You need Hermes 0.22.3 or newer.

1. Install fetchira 0.1.24 or newer on macOS or Linux with one of the commands under [Install](https://github.com/ImmuneFOMO/fetchira/blob/main/docs/setup.md#install) and check `fetchira --version`. Only the install is needed: keys saved with `fetchira add`, `fetchira setup` or the dashboard belong to a separate install that this plugin does not read. Hermes offers the tools only while `fetchira` is on the PATH it runs with.
2. Install and enable the plugin:

   ```sh
   hermes plugins install fetchira
   hermes plugins enable fetchira
   ```

3. Start a new session, or run `/reload-mcp`, and ask the agent to search for something.

## Provider keys

Save a key in Hermes, not in a chat message:

- Desktop: Settings → Plugins, open fetchira and paste the key into the provider's field. Hermes reconnects the server after you save.
- Terminal: `hermes config set TAVILY_API_KEY <key>`, then restart Hermes or run `/reload-mcp`.

Each variable is the provider id in upper case plus `_API_KEY`: `SERPER_API_KEY`, `TAVILY_API_KEY`, `EXA_API_KEY`, `FIRECRAWL_API_KEY`, `PARALLEL_API_KEY`, `STEEL_API_KEY`, `TINYFISH_API_KEY`, `LINKUP_API_KEY`, `YOU_API_KEY`, `OCTEN_API_KEY`, `NIMBLE_API_KEY`, `SERPAPI_API_KEY`, `LANGSEARCH_API_KEY`, `SCRAPINGANT_API_KEY`, `ZENROWS_API_KEY`, `HYPERBROWSER_API_KEY` and `BROWSERLESS_API_KEY`. Tavily, Exa, Parallel and Firecrawl use the same names as Hermes' own web backends, so a key you already saved for one of them reaches fetchira as well. Hermes starts MCP servers with a filtered environment, so fetchira sees these variables, when set, and not the rest of your `.env`.

## Before you install

- Network calls go to the providers you saved keys for and to the four keyless ones: Keenable, Jina Reader, OpenAlex and GDELT.
- Hermes starts `fetchira serve` from your PATH as a long-running local process. The binary comes from your own install. This package holds no executable code and downloads nothing, and its skill runs no shell commands.
- Calls draw on each provider's quota or balance. Search tries Parallel and Exa first when their keys are set, so a key you share with Hermes' own web backend draws on the same balance. `deep_research` with `depth: "deep"` may spend a paid balance.
- Hermes stops waiting for a fetchira call after 7 minutes. A longer `deep_research` run is cut off, and the provider may still charge for it.
- Fetchira keeps `usage.db` in its own folder under Hermes' plugin data. Removing that folder clears it. With fetchira 0.1.24 or newer the request debug log stays off, so the file holds no queries, URLs or results. It holds call counts per account and period, the last balance each provider reported, per-account retry times, and about the last 1,000 answered calls with time, tool, provider, account, status and latency. A screenshot or PDF that a `read` call asks for is saved to `images/` in the same folder and stays until you remove it.
- Keys stay in Hermes' `.env`. Fetchira reads them from its environment and does not save them.
- The plugin starts fetchira with `FETCHIRA_MANAGED_BY=hermes`, so fetchira 0.1.24 or newer does not update itself or check for new releases, keeps no request debug log, does not touch other agents' files and reads keys only from the environment Hermes gives it. `FETCHIRA_DISABLE` turns off its ChatGPT, Gemini and Grok web-session providers and image generation.
- Fetchira has no telemetry or usage reporting.
