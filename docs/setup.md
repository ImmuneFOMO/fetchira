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

Run the guided setup:

```sh
fetchira setup
```

1. Choose **On this computer** to use local API keys and browser sessions, or **Connect to a
   server** to use an existing hosted Fetchira.
2. For hosted access, enter the server URL and API key. Fetchira checks access and compatibility
   before saving; existing local accounts stay in your configuration. Switching back to local
   clears the saved server URL and key, so keep the key if you plan to reconnect.
3. Choose **CLI only**, **MCP**, or **MCP with CLI fallback**, then select your agents.
   **Later** leaves integrations unchanged.

`fetchira ui` opens the dashboard. To configure local providers directly:

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

After configuring local or hosted access, choose how your agents invoke Fetchira:

```sh
fetchira install
```

The interactive picker installs the selected skill and, for MCP choices, registers the server
in the selected clients. CLI only installs shell instructions without registering MCP.
Restart the selected agent after installation.

You can also paste the snippet for your client. Use the absolute path from `command -v fetchira` (Homebrew: `/opt/homebrew/bin/fetchira` or `/usr/local/bin/fetchira`; `curl | sh`: `$HOME/.local/bin/fetchira`; `cargo install`: `$HOME/.cargo/bin/fetchira`).

### Agent skills

`fetchira install` offers three variants:

Already installed? `fetchira install --refresh` updates existing skills without changing their
variant; it also repairs outdated Homebrew MCP launchers. See [upgrading from 0.1.13](configuration.md#upgrading-from-0113-to-0114)
for migration and optional MCP-to-CLI conversion.

- `fetchira` — MCP first, CLI fallback (the default)
- `fetchira-mcp` — MCP tools only
- `fetchira-cli` — CLI only, using local accounts or a hosted server

Choose the integration first, then select detected agents. **Later** leaves integrations unchanged.
Skill texts are embedded in the binary; no Git checkout is needed. Installed CLI instructions
include the executable and configuration directory, so agents use the same setup as MCP.
Installation roots:

| Agent | Skill directory |
|---|---|
| Claude Code | `~/.claude/skills` |
| Cursor | `~/.cursor/skills` |
| Codex | `~/.agents/skills`; a custom `CODEX_HOME` uses `$CODEX_HOME/skills` |
| Gemini | Shares `~/.agents/skills` when available; otherwise `~/.gemini/skills` |

[Cursor also discovers](https://cursor.com/docs/context/skills) skills in `.agents`, `.claude`,
and `.codex` user directories. These shared discovery paths can make an installed skill
available to more than the selected agent.
If a conflicting Fetchira variant is found in an unselected shared discovery directory,
installation stops before registering MCP. Select the agent that owns that directory too
to migrate its skill safely.

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

The dashboard offers the same connection and integration choices. CLI and MCP provide the
same six operations in local and hosted mode, including sessions and saved artifacts.
Hosted laptop attachments are unsupported through both interfaces. The local dashboard shows
this computer’s accounts and activity; use `fetchira usage` for the configured server’s quotas.

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
