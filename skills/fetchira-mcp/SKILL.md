---
name: fetchira-mcp
description: Use the configured Fetchira MCP tools for current web search, URL reading, deep research, JavaScript page reading, image generation, and provider usage. Use only when Fetchira MCP is exposed by the host.
---

# Fetchira MCP

For an image poll, use `prompt: ""` if the listed schema requires the field.

Use only Fetchira MCP tools that the host actually lists. If the host defers tool definitions, use
its tool-search mechanism to load the Fetchira server or namespace first. Then call the exact listed
name and schema. Do not shell out to the CLI or invent a tool.

Cite inspected source URLs. Empty results and fetched error pages provide no evidence.

Read `references.md` only when the request needs provider-specific options, sessions, deep research
planning or polling, image editing, setup, or error recovery.

Typical listed tools are `search`, `read`, `deep_research`, `browser`, `create_image`, and
`usage`; the host may expose them with a namespace. Fetchira `browser` reads JavaScript-heavy
pages; use a listed browser-control capability for interaction with the user's existing browser.
