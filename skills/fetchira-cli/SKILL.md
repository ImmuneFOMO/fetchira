---
name: fetchira-cli
description: Use the local Fetchira CLI for current web search, URL reading, deep research, JavaScript page reading, image generation, and provider usage through a shell.
---

# Fetchira CLI

Run the `fetchira` executable on `PATH`. This skill is shell-only: do not look for or invent MCP
tools.

Cite inspected source URLs. Empty results and fetched error pages provide no evidence.

Read `references.md` only when the request needs provider-specific options, sessions, deep research
planning or polling, image editing, setup, or error recovery.

```text
fetchira search "QUERY" [--provider P] [--max N] [--session S] [--model M] [--mode M] [--topic T] [--recency R] [--domain D]... [--file PATH]...
fetchira read "URL" [--provider P] [--mode M]
fetchira deep_research "QUERY" [same search flags] [--depth standard|deep]
fetchira browser "URL"
fetchira create_image "PROMPT" [--provider P] [--path DEST] [--session S] [--file PATH]...
fetchira usage [PROVIDER]
```

Positional query and prompt words are joined. Flags may be mixed with them; `--` ends flag
parsing. `deep_research` also accepts `dr`. Quote every query, URL, path, and session token. CLI
mode needs local accounts and rejects a configured remote endpoint. For a JavaScript-heavy page use
`browser`; use a separate listed browser-control capability for interaction with the user's
existing browser.

For a ChatGPT research/image poll, text may be omitted: `fetchira deep_research --session 'TOKEN'`
or `fetchira create_image --session 'TOKEN'`. Other calls need a query or prompt.

For setup, use `fetchira providers`, then `fetchira add PROVIDER --key KEY` for a key account or
`fetchira add PROVIDER` for a browser account. Do not run interactive setup or login unattended.
