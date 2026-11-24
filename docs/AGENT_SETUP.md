# Set up Fetchira for the user

Install Fetchira and connect the user's selected clients without a terminal prompt. Keep existing client settings and provider accounts.

## 1. Install the binary

Check `command -v fetchira` and `fetchira --version` first. On macOS with Homebrew:

```sh
brew install ImmuneFOMO/tap/fetchira
```

On macOS or Linux without Homebrew:

```sh
curl -fsSL https://raw.githubusercontent.com/ImmuneFOMO/fetchira/main/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"
```

For unsupported release platforms, see [source builds and requirements](setup.md#install).

## 2. Configure providers

```sh
fetchira providers
fetchira list
fetchira add serper --key "$SERPER_API_KEY"
```

Choose a provider for the user's task from `fetchira providers`; signup links are in [setup](setup.md#choose-providers). Run `add PROVIDER --key` only with a key already supplied through a protected execution environment. Never ask for secrets in chat or print them in logs. Omitting `--key` prompts interactively for API-key providers.

The user creates accounts, approves payments, and completes web-session logins themselves. For web providers, `fetchira add PROVIDER` opens browser login; use the exact identifier from `fetchira providers`. Do not copy unrelated browser profiles or cookies. Keyless providers can cover some operations without an account.

## 3. Register clients without prompts

```sh
fetchira install --mcp --agents detected
```

This registers MCP and installs its usage skill where supported. `detected` selects clients with existing Fetchira registrations if any exist; otherwise it selects present clients. It is the default when a mode flag is given. No detected clients produces an error; select names explicitly:

```sh
fetchira install --both --agents codex-cli,cursor
fetchira install --mcp --agents "Claude Code,Gemini CLI"
```

Use `--both` for MCP with CLI fallback, or `--cli` for shell instructions only. `--agents all` selects every supported target for that mode. Names and hyphenated slugs are case-insensitive; an unknown name prints valid slugs. CLI mode accepts only skill-supported clients, whose directories must already exist. Add `--remove-mcp` only with `--cli` to remove selected Fetchira registrations after their skills install successfully. Other servers are preserved. Without it, existing MCP registrations remain.

Use the same Fetchira home for all commands: `$FETCHIRA_HOME`, otherwise `$XDG_CONFIG_HOME/fetchira`, otherwise `~/.config/fetchira`. The installer records that home in client registrations. `fetchira install --refresh` updates existing skills and launchers; bare `fetchira install` remains interactive.

If automatic registration fails, use the [client configuration examples](setup.md#use-it): back up the config, merge the Fetchira entry, preserve other settings, and use absolute executable and home paths. Separate users, profiles, containers, and remote machines need their own registrations.

## 4. Verify

```sh
fetchira usage
fetchira search "Rust release notes" --provider serper --max 1
```

Use a search provider configured in step 2. Restart each selected client, check its Fetchira tools, and run `usage` followed by a short `search`. A result verifies the request path; a quota error requires checking the provider account. For missing tools, check the executable, shared home, and client restart. Image tools appear only after a supporting web provider is logged in.

## 5. Connect a team to a host

The administrator configures providers on the host and follows [hosted deployment](hosted.md) for persistent state, master key, private admin setup, and HTTPS. In the host's service configuration environment:

```sh
fetchira server
```

In another shell using that same configuration, issue one access key per teammate:

```sh
fetchira server key create teammate "Team member"
```

Supply the URL and key securely to each teammate. On their computer:

```sh
fetchira remote set https://fetchira.example.com/mcp --key "$FETCHIRA_API_KEY"
fetchira remote check
fetchira install --mcp --agents detected
fetchira usage
```

CLI and MCP calls now use the host's configured providers, subscriptions, and shared quota. Teammates receive Fetchira keys, not provider credentials. Provider sharing terms still apply. `fetchira serve` is the local stdio launcher; `fetchira server` is the HTTP host. `fetchira remote disconnect` clears the saved connection and returns to local accounts.
