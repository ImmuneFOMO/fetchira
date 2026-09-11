# Fetchira reference

Use the capability that matches the job:

- `search` finds current information and returns ranked results or a provider answer with sources.
- `read` fetches one known URL as clean Markdown.
- `deep_research` produces a slower, cited multi-source report.
- `browser` reads a JavaScript-heavy URL.
- `create_image` generates or edits an image and returns a session for follow-up.
- `usage` shows quota and model information.

Prefer `search` for a quick fact, `read` when the URL is known, and `deep_research` when coverage
and citations matter. Use `browser` when a normal read is empty or the page needs JavaScript. A
Fetchira browser call reads a page; it does not operate the user's existing logged-in browser.

## Providers and options

The router chooses an available account in priority order and fails over when a provider is
unavailable. `provider` restricts routing to one backend, so its quota still applies. Before using
a provider-specific `model` or `mode`, call `usage` for that provider and use the live options it
returns. The catalog changes; do not rely on a hard-coded model list or guess API-style model IDs.

`topic` accepts `web`, `news`, or `academic`; `recency` accepts `day`, `week`, `month`, `year`, or
an ISO date; `domains` restrict results and a `-` prefix excludes a domain. Provider-specific
options belong in the provider usage sheet.

## Sessions

Web-provider results end with `⟦session: TOKEN — hint⟧`. Pass only `TOKEN` back as `session`,
without the brackets or hint. Do not decode or rewrite it. A session carries provider and
account affinity, and takes precedence over a new provider choice.

Use the returned session for follow-up questions, Gemini research runs, ChatGPT research polling,
and image edits. If a call is pending, continue the same capability with the newest session,
following any returned retry delay. Stop on a terminal error or cancellation; if repeated polls
show no progress, report the pending state and retain the token. Keep its provider unchanged.

## Deep research

Gemini and ChatGPT research use a plan then run:

1. Call `deep_research` with the request; save the returned plan and session.
2. Refine the plan with that session if needed.
3. Send `start` with the session. Gemini usually returns the report there; ChatGPT may need polling.
4. Poll ChatGPT with the returned session until the report is complete.

Use `depth: "standard"` or `"deep"`; deep can take longer and use more quota. A ChatGPT poll may
accept an empty query. Follow the active interface's schema for whether text is required.

## ChatGPT web

`chatgpt_web` uses a saved browser session and Chrome on the machine running Fetchira. Search browses by default; `mode: "chat"`
answers without web browsing. Its selectable model and thinking-level catalog is live: call
`usage` for `chatgpt_web` before choosing one, or omit `model` and use the default. ChatGPT
deep research ignores `model`, and ChatGPT image generation has no model selector.

## Files, images, and setup

File attachments need local mode and provider support; use an absolute readable path. Search
attachments default to `grok_web` when no provider is forced; research and image calls keep their
own priority order. Hosted calls reject laptop attachments through both interfaces. CLI and
local stdio (including connections to a hosted server) write completed images and PDFs to
the requested path or Fetchira's image folder. Direct hosted HTTP returns inline artifacts and
never writes a caller-supplied server path. Pending results return a status and session.

If no account or key is available, report that setup is required. Do not start an interactive
setup or login flow unattended.

## Errors

Report the provider's actual error. For an expired web session, ask the user to log in again for
that provider. An empty result or failed `read` is not evidence: cite only URLs returned by a
successful search/read/browser call, verify a URL with `read` before relying on it, and say when a
source could not be verified. Do not claim a result for an error or pending operation, and do not
silently switch away from a session's provider.
