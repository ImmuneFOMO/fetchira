# Accessibility

Fetchira should be usable from a keyboard, a screen reader and an agent alike. Every feature
is reachable without the web UI: the CLI and MCP tools cover search, reading, research,
images, accounts and usage.

## Supported

- **CLI.** Plain text output that works with screen readers and terminal magnifiers.
  Setup commands have non-interactive flags (`fetchira install --mcp --agents detected`,
  `fetchira add PROVIDER --key ...`), so no arrow-key menus are required.
- **MCP.** Six tools with text results, usable from any MCP client.
- **Local dashboard and hosted admin.** Visible keyboard focus, ARIA labels on icon-only
  controls and dialogs, and `prefers-reduced-motion` support. Targets current Chrome,
  Firefox and Safari.
- **Docs.** Markdown on GitHub and [fetchira.dev](https://fetchira.dev); images have
  alt text.

## Known limitations

- The dashboard has a dark theme only and no high-contrast mode.
- Quota meters and status dots rely partly on color; check the numeric values next to them
  or use `fetchira usage` in the terminal.
- Browser-session logins (`gemini_web`, ChatGPT, Grok) open the provider's own sign-in
  page, which Fetchira does not control.
- No formal WCAG audit has been done yet. The target is WCAG 2.2 AA.

## Report a barrier

Open a [bug report](https://github.com/ImmuneFOMO/fetchira/issues/new?template=bug_report.yml)
and start the title with `a11y:`. Include the screen or command, your assistive technology,
browser and OS. Accessibility bugs are treated like functional bugs.
