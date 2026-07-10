---
name: fetchira
description: Web search, page reading, deep research, and headless browsing via the fetchira MCP server. Use whenever you need current/external information — search the web, read a URL as clean markdown, run a multi-source deep research report, or fetch a JS-heavy page. Routes across many free providers (incl. logged-in Gemini/Perplexity/Grok web sessions) with automatic quota-aware failover.
---

# fetchira

fetchira is an MCP server that fronts many web-search/scrape providers behind one quota-aware
router. Call a capability; it picks the least-exhausted account and fails over on error. Register it
first (see the project README); these tools then appear as `search`, `read`, `deep_research`,
`browser`, `create_image`, `usage`.

## When to use
- **`search`** — find current info / answer a factual question. API providers return ranked
  title+url+snippet; web providers (`perplexity_web`/`gemini_web`/`grok_web`) return a synthesized
  answer with sources.
- **`read`** — fetch ONE known URL as clean markdown (article, doc, README).
- **`deep_research`** — a thorough, multi-source report with citations. Slower (seconds to minutes).
- **`browser`** — load a JS-heavy page in a headless browser and get its content.
- **`create_image`** — generate an image from a prompt via a logged-in web account
  (gemini_web / grok_web / chatgpt_web). The image is saved to disk and the result names the
  file — pass `path` (absolute) to save it where you need it, e.g. into the repo.
- **`usage`** — show remaining free quota per account (and, for chatgpt_web, live per-tool limits).

Prefer `search` for quick facts and `read` when you already have the URL. Reach for `deep_research`
only when the user wants depth/coverage, not a one-line answer.

## Useful args (search / deep_research)
- `provider` — force a backend (e.g. `"serper"`, `"perplexity_web"`, `"gemini_web"`). Omit to let
  the router choose. Web providers give answers+sources; API providers give SERP rows.
- `model` / `mode` — provider-specific tuning. grok `mode:"auto"|"fast"|"expert"|"heavy"` (search
  defaults to fast, deep_research to heavy→expert); perplexity `mode:"reasoning"`; gemini
  `model:"pro"|"flash"`; chatgpt_web `model` = a picker model + optional thinking level (see below).
  Optional; defaults are fine.
- `session` — continue a previous web-provider conversation **with history**. Every web result ends
  with `⟦session: <token>⟧`; pass that token back as `session` to ask a follow-up in the same thread.

## Conversation continuity
```
search { query: "...", provider: "perplexity_web" }     -> answer + ⟦session: perplexity_web:…⟧
search { query: "a follow-up question", session: "perplexity_web:…" }   -> continues the thread
```

## Gemini Deep Research (plan → run)
```
deep_research { query: "history of X", provider: "gemini_web" }   -> a research PLAN + ⟦session: gemini_web:dr|…⟧
deep_research { query: "start", session: "gemini_web:dr|…" }      -> runs ~1-3 min, returns the full report
```
Send an adjustment instead of `"start"` to refine the plan before running.

## ChatGPT (`chatgpt_web`): model, thinking level, tools
`chatgpt_web` is a logged-in ChatGPT session driven through the composer. `search` is a chat turn
(web search **on by default**; pass `mode:"chat"` to answer from the model alone without browsing).

Pick the model/level with `model` (case/dots/dashes don't matter — `gpt-5.5` == `gpt-5-5`):
- **models**: `gpt-5.5`, `gpt-5.4`, `gpt-5.3`, `o3`
- **thinking level**: `instant`, `medium`, `high` — **varies per model**: gpt-5.5 & gpt-5.4 have all
  three; gpt-5.3 has instant only; o3 has medium only.
- pass a model, a level, or both: `model:"gpt-5.4 high"`, `model:"o3"`, or just `model:"high"`
  (applies to the current model).
- **Discover the live catalog**: pass an unknown value (e.g. `model:"?"`) — the error lists the
  actual models and that model's available levels. Don't guess API-style slugs like
  `gpt-5-5-thinking` — the picker uses the names above.

`deep_research` with `provider:"chatgpt_web"` runs ChatGPT Deep Research — its own research model, so
`model` is **ignored**. `create_image` with `provider:"chatgpt_web"` generates an image — **no model
choice** (uses ChatGPT's own image model). `usage` shows chatgpt_web's live per-tool limits
(deep_research / image_gen / …) so you can see what's left before calling.

## Helping the user set up
If a tool fails with "no available account" / "NO KEY", the user hasn't configured providers. You can
drive setup from a shell (the `fetchira` binary is on PATH):
- `fetchira providers` — list every provider and whether it needs an API key or a browser login.
- `fetchira list` — show configured accounts + remaining quota + status.
- Ask the user which providers they want and for any API keys, then run
  `fetchira add <provider> --key <KEY>` (key-based) per account.
- For web providers run `fetchira add <provider>` or `fetchira login <provider>` — a browser opens
  for the user to log in (you can't complete this for them; tell them to finish in the window).
- `fetchira remove <label>` removes an account.
Keys are stored in the user's global config (`~/.config/fetchira`), never in the project.

## Notes
- Grok (`grok_web`) is rate/anti-bot sensitive and may intermittently 403 — the router fails over.
- If a web provider returns "session expired / run fetchira login", tell the user to re-run
  `fetchira login <provider>`; don't retry blindly.
