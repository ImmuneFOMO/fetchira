---
name: fetchira
description: Use Fetchira for current web search, URL reading, deep research, JavaScript page reading, image generation, and provider quota routing. Prefer its listed MCP tools and use the local CLI only when MCP is unavailable.
---

# Fetchira

Use the transport that is actually available and preserve the exact tool names, arguments, and
session tokens supplied by the host.

## Choose a transport

1. If Fetchira MCP tools are listed, call them and do not shell out. This includes namespaced
   tools such as `mcp__fetchira__search`; use the exact listed schema.
2. If the host supports deferred tools or tool search, search for/load the Fetchira MCP server or
   namespace before deciding that MCP is unavailable. Never invent a tool name or schema.
3. Only when no Fetchira MCP tool is exposed after that discovery, run the `fetchira` executable
   from `PATH` using the CLI forms below. Quote queries, URLs, paths, and session tokens.
4. If neither transport is available, report the setup problem instead of guessing.

Cite inspected source URLs. Empty results and fetched error pages provide no evidence.

Read `references.md` only when the request needs provider-specific options, sessions, deep research
planning or polling, image editing, setup, or error recovery. Defaults for a one-shot search,
known-URL read, or page read are already covered here.

## CLI fallback

```text
fetchira search "QUERY" [--provider P] [--max N] [--session S] [--model M] [--mode M] [--topic T] [--recency R] [--domain D]... [--file PATH]...
fetchira read "URL" [--provider P] [--mode M]
fetchira deep_research "QUERY" [same search flags] [--depth standard|deep]
fetchira browser "URL"
fetchira create_image "PROMPT" [--provider P] [--path DEST] [--session S] [--file PATH]...
fetchira usage [PROVIDER]
```

Positional query and prompt words are joined. Flags may be mixed with words; `--` ends flag
parsing. `deep_research` also accepts `dr`. `usage` is the compact quota snapshot, or a provider
sheet when given a provider; `list` and `accounts` are the human account tables. CLI mode needs
local accounts and rejects a configured remote endpoint.

ChatGPT research/image polls may omit CLI text when a polling session is supplied. For an MCP
image poll, use `prompt: ""` if its listed schema requires the field.

For a page that needs JavaScript, use Fetchira `browser`. For actions in the user's existing logged
in browser, use a listed browser-control capability instead.

For sessions, polling, file attachments, setup, and provider options, read the relevant section of
`references.md` before calling.
