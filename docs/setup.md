# Installation and agent integration

[Home](../README.md) · [CLI commands](cli.md) · [Configuration](configuration.md) · [Hosted](hosted.md)

## Install

**Mac**

```sh
brew install ImmuneFOMO/tap/fetchira
```

**Linux x86_64**

```sh
curl -fsSL https://raw.githubusercontent.com/ImmuneFOMO/fetchira/main/install.sh | sh
```

**Build from source** (also works when a release has no prebuilt for your platform)

```sh
cargo install --locked --git https://github.com/ImmuneFOMO/fetchira
```

`curl | sh` installs the available release archive into `~/.local/bin`. Run
`export PATH="$HOME/.local/bin:$PATH"` if the command is missing. Release targets are macOS
arm64/x86_64 and Linux arm64/x86_64 (glibc >= 2.34); older releases may lack an arm64 archive.

A source build needs Rust, cmake, Perl, pkg-config and a C/C++ toolchain, and installs into
`~/.cargo/bin`. From an existing checkout, use `cargo install --locked --path .`. Check
`fetchira --version` if a documented command is missing from an older installed release.

`fetchira ui` opens the local dashboard and keeps running; use another terminal for CLI setup:

```sh
fetchira providers                # see every provider and its capability
fetchira add serper --key KEY     # API-key provider; never omit --key
fetchira add gemini_web            # web-session provider; a browser opens for login
fetchira list                     # expect key or session, not NO KEY / NEEDS LOGIN
```

Config lives in `$FETCHIRA_HOME`, otherwise `$XDG_CONFIG_HOME/fetchira` (default `~/.config/fetchira`).

### Choose providers

Every provider is optional. Start with one or two that match the job, then add more as needed. API-key providers use the signup link below; web providers use a browser login and do not need an API key.

| Provider | Auth | Useful for | Start here |
|---|---|---|---|
| `serper` | API key | Google search and page reads | [serper.dev](https://serper.dev) |
| `tavily` | API key | Search, answers and page reads | [app.tavily.com](https://app.tavily.com) |
| `exa` | API key | Semantic search and research | [exa.ai](https://exa.ai/?ref=immunefomo) |
| `firecrawl` | API key | Search, scrape and crawl pages | [firecrawl.link](https://firecrawl.link/immunefomo) |
| `parallel` | API key | Search and deep research | [parallel.ai](https://parallel.ai) |
| `steel` | API key | Browser and JavaScript page reads | [steel.dev](https://steel.dev) |
| `gemini_web` | Browser login | Search, research, images and file Q&A | Google account |
| `grok_web` | Browser login | Search, research, images and file Q&A | X/Grok account |
| `chatgpt_web` | Browser login + Chrome/Chromium | Search, research, images and file Q&A | OpenAI account |

The provider's own plan and quota are authoritative; free tiers and prices change. `fetchira providers` lists the providers supported by the installed binary.

## Use it

The binary speaks MCP over stdio. For a human in a terminal, the interactive installer can register it with detected coding tools:

```sh
fetchira install
```

The picker writes the selected clients' configs and is not suitable for an agent. Restart the selected tool after registration.

You can also paste the snippet for your client. Use the absolute path from `command -v fetchira` (Homebrew: `/opt/homebrew/bin/fetchira` or `/usr/local/bin/fetchira`; `curl | sh`: `$HOME/.local/bin/fetchira`; `cargo install`: `$HOME/.cargo/bin/fetchira`).

### Agent skills

`fetchira install` offers three variants:

- `fetchira` — MCP first, CLI fallback (the default)
- `fetchira-mcp` — MCP tools only
- `fetchira-cli` — local CLI only

The installer asks for the variant after the MCP client selection; leave that selection empty to skip MCP.
Skill texts are embedded in the binary; no Git checkout is needed. Installation roots:

| Agent | Skill directory |
|---|---|
| Claude | `~/.claude/skills` |
| Cursor | `~/.cursor/skills` |
| Codex | `~/.agents/skills`; a custom `CODEX_HOME` uses `$CODEX_HOME/skills` |
| Gemini | Shares `~/.agents/skills` when available; otherwise `~/.gemini/skills` |

The agent parent directory must exist. Detecting `~/.codex` also permits creating the current
`~/.agents/skills` directory. Existing Fetchira variants under the legacy `~/.codex/skills`
root are backed up during migration. When Codex and Gemini share `.agents`, the installer
writes one copy and backs up conflicting `.gemini/skills` variants. A custom `CODEX_HOME`
must already exist; setting it to the default `~/.codex` still uses the canonical shared root. Choosing `skip` leaves skills
unchanged; choosing a variant writes `SKILL.md` and `references.md`, and removes the other
Fetchira variants in that destination after the new one is written. Previous folders, including custom files and edits, are preserved outside the skills directory
in the agent's `fetchira-skill-backups/` folder; the installer prints the backup path. An unchanged
reinstall does not create another backup. Symlinked skill destinations are reported for manual
installation instead of being replaced. The shared reference covers provider options, session
affinity, Gemini plan→run, ChatGPT polling and model catalog, file attachments, and errors.

The dashboard's install form has the same variant choice. A CLI-only install can be registered
without selecting any MCP client. The MCP server and CLI use the same router and provider
capabilities; only the transport differs.

**Claude Code**

```sh
claude mcp add fetchira -s user -- "$(command -v fetchira)"
```

Skill (when to call the tools): `fetchira install` can write the default `fetchira` skill, or copy
`skills/fetchira/SKILL.md` to `~/.claude/skills/fetchira/SKILL.md` and
`skills/shared.md` to `~/.claude/skills/fetchira/references.md` manually.

**Cursor** — `~/.cursor/mcp.json`

**Windsurf** — `~/.codeium/windsurf/mcp_config.json`

**Gemini CLI** — `~/.gemini/settings.json`

**Claude Desktop** (macOS) — `~/Library/Application Support/Claude/claude_desktop_config.json`

```json
{ "mcpServers": { "fetchira": { "command": "/absolute/path/to/fetchira" } } }
```

**Codex CLI** — `$CODEX_HOME/config.toml` (default `~/.codex/config.toml`)

```toml
[mcp_servers.fetchira]
command = "/absolute/path/to/fetchira"
args = []
```

**OpenCode** — `~/.config/opencode/opencode.json`

```json
{
  "mcp": {
    "fetchira": { "type": "local", "command": ["/absolute/path/to/fetchira"], "enabled": true }
  }
}
```

Paths under `~/.config` use `$XDG_CONFIG_HOME` when set to an absolute path.

**VS Code** — macOS `~/Library/Application Support/Code/User/mcp.json`; Linux `$XDG_CONFIG_HOME/Code/User/mcp.json` (default `~/.config/Code/User/mcp.json`)

```json
{ "servers": { "fetchira": { "type": "stdio", "command": "/absolute/path/to/fetchira" } } }
```


## Unattended setup

Give an agent the provider name and an API key through your normal secret-handling flow.
Use `fetchira add PROVIDER --key 'KEY'`; omitting `--key` prompts interactively.
For a web provider, `fetchira add PROVIDER` opens login and waits for completion.

Use the client registration command or configuration above. `fetchira install` and
`fetchira setup` are interactive terminal commands. Bare `fetchira` opens the dashboard in
a terminal and waits for MCP messages when stdin is piped; an agent should use an explicit
command. Restart the client after registration, then ask it to search a current topic using
Fetchira and check that the tool returns a result.
