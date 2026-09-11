# Configuration and troubleshooting

[Home](../README.md) · [Setup](setup.md) · [CLI](cli.md) · [Hosted](hosted.md)

## Files and environment

Fetchira uses `$FETCHIRA_HOME`, otherwise `$XDG_CONFIG_HOME/fetchira`, otherwise
`~/.config/fetchira`. The directory holds `fetchira.toml`, `usage.db`, captured sessions,
image outputs and update state. A relative `db_path` is resolved inside that directory.

Environment variables already set in the process take precedence over `.env`. Fetchira
loads its home `.env` first, then fills unset variables from a `.env` found from the current
working directory. Use absolute paths when setting `FETCHIRA_HOME` or `XDG_CONFIG_HOME`.

A minimal configuration using an environment-backed key:

```toml
[[account]]
provider = "serper"
label = "serper-1"
api_key = "env:SERPER_API_KEY"
```

Keep `SERPER_API_KEY` in the process environment or the Fetchira home `.env`. See
[fetchira.toml.example](../fetchira.toml.example) and [.env.example](../.env.example) for the
supported fields; enable only the accounts you use. Local config can contain plaintext API
keys. Hosted mode encrypts stored provider credentials with its persistent master key.
Treat the data directory and backups as private: debug logs can contain queries and results.

## Accounts and routing

```sh
fetchira providers
fetchira add serper --key 'KEY' --label serper-work
fetchira add gemini_web
fetchira list
fetchira usage serper
```

`list` (alias `accounts`) shows account readiness. `usage` shows the compact model/quota
snapshot; `usage PROVIDER` adds supported operations and options. Web sessions expire:
`fetchira login LABEL` captures a fresh session for an existing account. For a session file,
use `fetchira session LABEL --file session.json`. Remove an account with
`fetchira remove LABEL` only when its saved configuration and session are no longer needed.

The router tries configured providers in priority order and chooses an account by remaining
allowance. Unsupported providers are skipped. Change the order with:

```sh
fetchira priority
fetchira priority search serper,tavily,exa
fetchira priority deep_research gemini_web,parallel
fetchira priority search reset
```

Capabilities with configurable priority are `search`, `read`, `deep_research`, and `image`.
Unlisted providers follow the listed ones in built-in order. Restart running MCP/server
processes after changing configuration through the CLI. Dashboard changes reload its router.

A `--provider` argument restricts the request to that backend; it does not fall back to a
different provider. A returned session also pins the account, so a follow-up cannot silently
continue another conversation. Provider plans, balances and limits are authoritative;
configured quotas are local routing ceilings, not grants of free usage. Live balances update
the display but do not raise those ceilings: set `quota`/`reset` and, for web research,
`dr_quota`/`dr_reset` to match your plan. A forced provider still obeys these limits.

Dollar balances are real balances; the displayed `≈` call counts are estimates that vary by
operation and plan. Steel requests use its proxy: the Launch plan requires a paid $10 deposit
to enable it, even with free credits ([Steel pricing](https://docs.steel.dev/overview/pricinglimits)).

Temporary rate limits cause failover and a retry delay, not monthly quota exhaustion.
Confirmed insufficient-credit responses trigger a separate quota check after a one-minute
cooldown. A fresh positive balance restores routing after a top-up; providers without a readable
balance get a single probe per cooldown. Configured local ceilings still apply. If no account
can serve the request, Fetchira returns an error with the available provider information.

## Proxies and logging

```sh
fetchira proxy LABEL http://user:password@proxy.example:8080
fetchira proxy LABEL pool
fetchira proxy LABEL direct
```

`pool` needs `[proxy_pool]` in the config; see the example file. Accounts keep a stable pool
assignment. Provider availability can depend on the account's region and proxy.

Logs go to stderr; set `RUST_LOG=fetchira=debug` for diagnostics. The dashboard Debug tab
uses a bounded request/response log (24-hour retention by default). Disable capture with:

```toml
[debug_log]
enabled = false
```

## Troubleshooting

| Symptom | Check or fix |
|---|---|
| `fetchira: command not found` | Add the installation directory to `PATH`; see [setup](setup.md#install). |
| Bare command waits or opens a dashboard | Use `fetchira search ...` for CLI, `fetchira serve` for MCP, or `fetchira ui` for the dashboard. |
| `NO KEY`, `NEEDS LOGIN`, or no usable accounts | Check the same `FETCHIRA_HOME` is used by the shell and agent; add the missing key or run `fetchira login LABEL`. |
| Temporary `429` | Respect the returned retry delay or use another configured provider; do not reset monthly counters for a minute limit. |
| Provider reports insufficient credits | Check `fetchira usage PROVIDER` and the provider dashboard; top up or choose another account/provider. |
| Hosted call rejects a file attachment | Laptop attachments require local mode for both CLI and MCP. Disconnect the remote or use a separate local home. |
| Unknown model or mode | Read `fetchira usage PROVIDER`; omit the model to use its default. |
| Parse error during installation or configuration | Fix the named file; Fetchira refuses to overwrite malformed config. |
| Agent cannot find MCP or the skill | Check the absolute binary/config path, selected skill variant, and restart the agent; see [setup](setup.md). |
| Research/image is pending | Pass the newest returned session to the same tool; see [CLI sessions](cli.md#sessions-and-research). |
| Hosted auth, browser or update failure | Follow [hosted verification](hosted.md#verification) and check the server logs. |

## Updates and removal

Run `fetchira update` (alias `fetchira upgrade`) or click **Update** in the dashboard.
The updater uses Homebrew for a Homebrew installation and release archives for a standalone
binary. It reports progress or an actionable error in place. Hosted updates have a separate
[backup and rollback procedure](hosted.md#updates-and-rollback).

### Upgrading from 0.1.13 to 0.1.14

Your existing Fetchira home is retained: accounts, sessions, usage and settings migrate there.
Do not delete the database or re-add working accounts. Updated skills are refreshed as part
of finishing the upgrade; old files and custom edits are archived under `fetchira-skill-backups/`.
Versioned Homebrew MCP launchers are repaired without changing your other server options.

The upgrade offers agent setup in the CLI or dashboard. Choose **CLI only**, select your
agents and confirm removal to replace their Fetchira MCP registrations with the CLI skill.
Removal happens only after that skill installs; original configs are backed up. Declining
keeps MCP available. Project-level MCP settings are separate: only detected user-level
registrations are managed.

The already-released 0.1.13 CLI updater cannot run the new setup after replacing itself:
setup finishes on the next interactive Fetchira run. Its dashboard updater already restarts
into the new version. Its Homebrew path still displays the existing `brew upgrade fetchira`
instruction; the new updater runs Homebrew directly. No GitHub visit is needed.

Agents that hold a running MCP process must reload it through their own controls; Fetchira
shows restart hints instead of terminating active work. An old dashboard must close before
a hosted connection is saved, because its old config writer could erase the server settings.
For a deliberate refresh outside the update flow, use `fetchira install --refresh` or
**Setup & agents → Refresh existing skills**.

To stop using Fetchira, remove its MCP entry from the clients you configured, remove the
installed Fetchira skill folder, and uninstall the binary using its installation method.
Keep the data directory until you have saved any sessions, images or backups you need.
