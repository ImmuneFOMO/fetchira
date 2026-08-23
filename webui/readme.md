# fetchira — Design System

> **fetchira** is a developer tool that routes web search / scrape / research calls
> across many free-tier providers behind one quota-aware router. It ships as an MCP
> server + CLI used by AI coding agents. This design system is for the **local web
> dashboard** — the human control panel that runs on `127.0.0.1` and lets a developer
> watch live quota, manage accounts, and see the router route in real time.

This is a from-brief build: there is **no upstream codebase or Figma** for the product
yet. The visual language here *is* the source of truth. If/when a real app exists,
reconcile against it and update this folder.

---

## 1 · Project personality & direction

An **instrument panel, not a marketing page.** fetchira's dashboard should feel like a
piece of lab equipment a developer keeps open next to their terminal — fast, precise,
quietly alive. Deep near-black slate, machined hairline edges, and one electric-lime
"fuel" accent that ties everything back to the core metaphor: *free quota is fuel, and
the router is the gauge cluster.* Numbers are monospace and tabular; copy is terse and
lowercase-leaning. Motion is mechanical — meters sweep to value, log lines slide in,
live updates pulse once. Nothing bounces, nothing gradients, nothing decorates.

The three nouns to design toward: **gauge, log, ledger.**

---

## 2 · Sources

- **Brief:** product + visual brief supplied by the user (instrument-panel dark
  dashboard for the fetchira quota router). Real provider data baked into
  `ui_kits/dashboard/data.js`.
- **Fonts:** Space Grotesk, Hanken Grotesk, JetBrains Mono — all Google Fonts, loaded
  via `tokens/fonts.css`. ⚠️ *Substitution note:* these are loaded from the Google
  Fonts CDN, not self-hosted. For a fully-offline local app, download the `woff2`
  binaries and swap the `@import` for local `@font-face src` rules.
- **Icons:** Phosphor Icons (CDN) as the documented system — see §7. The dashboard
  currently uses a small set of **monospace glyphs and unicode marks** (`+ ✓ ⚠ ▾ ✕ ◧
  ❚❚ ▶`) rather than an icon font, which suits the terminal aesthetic; Phosphor is the
  sanctioned set when richer icons are needed.

---

## 3 · Content fundamentals (voice & copy)

fetchira talks like a CLI `--help` page written by someone who respects your time.

- **Casing:** lowercase for product nouns and provider names (`serper`, `perplexity_web`,
  `deep_research`), **Sentence case** for UI labels and buttons (`Add account`, `Provider
  health`, `Log in with browser`). Never Title Case headings. ALL-CAPS only for tiny
  mono eyebrow labels with wide tracking (`REQ REMAINING`, `ACCOUNT`, `QUOTA`).
- **Person:** address the user as **you** ("waiting for you to log in…"). The system
  refers to itself as **fetchira** or "the router", never "we".
- **Numbers are first-class.** Show real figures, tabular and mono: `423 / 2,500`,
  `533 / 1,000,000`, `198ms`, `resets in 3d`. Abbreviate large remainders compactly
  (`1.38M req remaining`). Pad nothing with fake precision.
- **Status is a verb-y adjective:** `healthy`, `running low`, `exhausted`,
  `needs login`. Errors read like log lines: `429 rate_limited — failed over to
  tavily-1`, `503 quota_exhausted — 300/300 monthly, resets in 3d`.
- **Tone:** factual, present-tense, no exclamation marks, no hype. A success message is
  `Account added · serper-1 is live in the router rotation.` — not "🎉 Success!".
- **Emoji:** none. Unicode status marks (`✓`, `⚠`, `→`) are allowed *inline in mono
  contexts* as terminal-style glyphs, not as decorative emoji.

**Correct:** `perplexity-1 · exhausted — resets in 3d` · `❚❚ pause` ·
`•••• key set`
**Incorrect:** `Perplexity Account #1 is Currently Unavailable 😔` ·
`Your Quota Journey` · `Click here to get started!`

---

## 4 · Visual foundations

**Color & vibe.** Cool near-black slate canvas (`--bg-base #0A0C11`) over a deeper void
(`--bg-void #06070A`) for recessed wells (the log feed, meter tracks). Surfaces layer up
in three steps (`--surface-1/2/3`) rather than relying on shadow. One accent: **electric
lime** (`--lime-500 #C6F94A`) for fuel/quota, CTAs, focus, and live pulses. Semantic
gauge colors are unambiguous: **green** healthy, **amber** running low, **red**
exhausted, **grey** disabled / needs-login. A single **cyan** (`#34D6E6`) marks
read/scrape + captured web sessions. Imagery, if any, stays cool and low-chroma — this
is an instrument, not a photo gallery.

**Type.** Space Grotesk for display/wordmark/headings (technical grotesk character),
Hanken Grotesk for UI body and labels (warm, legible), JetBrains Mono for **all numbers,
URLs, provider names, log lines, and eyebrow labels**. Tabular figures everywhere numbers
align. Tight tracking on display (`-0.02em`), wide tracking on uppercase mono micro-labels
(`0.12em`).

**Backgrounds.** Flat slate plus an optional faint **dot-grid backplate**
(`.fx-backplate`, `--grid-line` at 2.2% opacity, 22px cells) — the "oscilloscope screen"
texture. **No gradients** as surface fills. The only gradient permitted is the subtle
area-fill under Activity sparklines (provider color → transparent).

**Borders.** Hairline is the primary separator: `rgba(255,255,255,0.06–0.15)`. A
provider card's left edge gets a 3px semantic accent strip (health color). Dividers are
1px `--border-faint`.

**Elevation.** Depth comes from *layering + a 1px inset top highlight* (reads as machined
metal), not big soft drop-shadows. `--elev-card` = `inset 0 1px 0 rgba(255,255,255,.04),
0 1px 2px rgba(0,0,0,.5)`. Overlays (`--elev-overlay`) go deeper for modals only.

**Corner radii.** Tight and mechanical. Cards/panels `7px` (`--r-md`), buttons/inputs
`5px` (`--r-sm`), tags/segments `3px` (`--r-xs`), dialogs `10px` (`--r-lg`), pills/dots
`999px`. **Never 12–24px on cards.**

**Cards.** `--surface-1` fill, 1px `--border-hairline`, `--r-md`, `--elev-card`. Inset
("well") variant uses `--surface-inset` for the log feed and table bodies. On hover,
interactive cards lift to `--surface-2` + `--border-strong` — no scale, no shadow bloom.

**Motion & easing.** `--ease-out` (`cubic-bezier(.22,1,.36,1)`) for settling; durations
`120ms` (hover/press), `220ms` (fades), `700ms` (meter sweep). Keyframes: `fx-meter-fill`
(meters animate to value), `fx-log-in` (new log lines slide down + fade), `fx-pulse`
(one-shot ring on live status dot). Respect `prefers-reduced-motion`.

**Hover / press states.** Hover = lighter surface and/or brighter border (`--surface-2/3`,
`--border-strong`); primary button hover brightens lime to `--lime-400`. Press = translate
**down 1px** (`translateY(1px)`) — a physical key-press, never a scale. Focus = 2px lime
glow ring (`--glow-accent`), never a browser outline.

**Transparency / blur.** Used only on the **sticky top bar and tab bar**
(`rgba(10,12,17,0.8)` + `backdrop-filter: blur(12px)`) so content scrolls under them, and
on the **modal scrim**. Card bodies are opaque.

**Layout rules.** Max width `--container-max 1320px`. Top bar `56px`, sticky. Overview is
a two-column grid — provider tiles (auto-fill, min 290px) + a sticky 380px live-log rail.
Provider tiles are grouped by capability with a wide-tracked mono group header and a hairline
rule. Hit targets ≥ 34px (dense rows) / 44px (primary controls). Asymmetry is fine and
encouraged — the log rail is intentionally narrower than the grid.

---

## 5 · Tokens (foundation)

All tokens live in `tokens/` and are reachable from the root `styles.css`:

| File | What |
|---|---|
| `tokens/colors.css` | canvas, surfaces, borders, text, lime accent, cyan, semantic gauge colors, aliases |
| `tokens/typography.css` | font families, type scale, line-heights, letter-spacing, weights |
| `tokens/spacing.css` | 4px-based spacing scale, radii, layout dimensions |
| `tokens/effects.css` | elevation, glow, motion durations/easings, keyframes, grid texture |
| `tokens/fonts.css` | Google Fonts `@import` (Space Grotesk / Hanken Grotesk / JetBrains Mono) |
| `tokens/base.css` | element resets, `.fx-backplate`, `.mono`, `.eyebrow`, scrollbars, focus |

Use **semantic aliases** in product code where they exist (`--surface-card`,
`--health-ok`, `--text-heading`) and base tokens (`--lime-500`, `--space-6`) otherwise.

---

## 6 · Components

Reusable primitives live in `components/<group>/`. Import in card HTML / kits via
`const { X } = window.FetchiraDesignSystem_6526df` after loading `_ds_bundle.js`.

| Component | Group | Purpose |
|---|---|---|
| `Button` | core | primary / secondary / ghost / danger · sm·md·lg · iconLeft |
| `Badge` | core | status pills, reset-window, key/login chips · 7 tones · soft/outline/solid |
| `Card` | core | base surface · inset / raised / interactive · semantic accent strip |
| `StatusDot` | core | health dot · ok/low/out/off/accent · optional pulse + label |
| `QuotaMeter` | meters | **the hero fuel gauge** · segments / bar / radial · color driven by remaining |
| `ProviderCard` | providers | Overview tile composing Card + QuotaMeter + Badge + StatusDot + #dr meter |
| `RouteLogLine` | feed | one monospace route-log line · capability color · failover hop |
| `Input` | forms | dark-well text field · label / prefix / mono / invalid / hint |
| `Select` | forms | native select styled to the well surface |
| `Tabs` | navigation | segmented tab control with count badges |

Each directory has a `<group>.card.html` (`@dsCard group="Components"`) showing key
states. Each component has a `.d.ts` (props) and `.prompt.md` (when/how + example).

---

## 7 · Iconography

The dashboard's native icon vocabulary is **terminal glyphs**: monospace `+`, `✓`, `⚠`,
`→`, `▾`, `✕`, `◧`, and the transport marks `❚❚` / `▶`. This is deliberate — it keeps the
instrument-panel feel and avoids a heavy icon dependency.

When richer iconography is needed (toolbars, settings, future surfaces), use
**[Phosphor Icons](https://phosphoricons.com/)** at the **regular** weight (1.5px stroke)
to match the hairline aesthetic; use **bold** only for active/selected states. Load from
CDN:

```html
<script src="https://unpkg.com/@phosphor-icons/web"></script>
<i class="ph ph-gauge"></i>   <!-- regular -->
<i class="ph-bold ph-gauge"></i>
```

Sanctioned Phosphor glyphs for fetchira concepts: `gauge` (quota), `pulse` (activity),
`plugs-connected` (provider), `key` (API key), `globe` (web session), `arrows-split`
(failover/route), `shield-check` (logged in). **No emoji.** Color icons with text
tokens (`--text-mid` default, `--lime-500` for active).

Assets in `assets/`: `logo-mark.svg` (the gauge mark), `logo-wordmark.svg` (mark +
"fetchira" wordmark). Do not recolor the mark outside the lime/slate family.

---

## 8 · UI kit

`ui_kits/dashboard/` — the full local dashboard, the primary deliverable. JSX source lives in
the adjacent `.jsx` files; `tools/build_webui.js` compiles it to the checked-in `.js` assets
used at runtime. Babel is a build-only tool and is never shipped or loaded by the browser.

| File | Surface |
|---|---|
| `index.html` | interactive shell: top bar + tabs + three tabs + add-account modal + empty state |
| `TopBar.jsx` | wordmark, `127.0.0.1` chip, global status pills, total remaining, Add account |
| `OverviewTab.jsx` | provider grid grouped by capability + pinned **live route log** (streams) |
| `AccountsTab.jsx` | management table — masked key chips, proxy, status, row actions |
| `ActivityTab.jsx` | filterable route log + per-provider usage sparklines + provider health list |
| `AddAccountModal.jsx` | add flow — key vs browser-login providers, **guided login** sim, success |
| `data.js` | real mock data (providers, accounts, log, health) on `window.FX` |

States rendered explicitly: exhausted provider (red, "resets in 3d"), needs-login web
provider (greyed meter, amber CTA), failover log line, and the empty state
("Add your first provider"). Toggle the empty state with the corner `demo:` button.

---

## 9 · Index / manifest

```
styles.css                  ← consumers link this (imports only)
tokens/                     ← colors, typography, spacing, effects, fonts, base
components/                 ← core · meters · providers · feed · forms · navigation
guidelines/                 ← foundation @dsCard specimens (Type · Colors · Spacing · Brand)
ui_kits/dashboard/          ← the fetchira router dashboard (UI kit + index.html)
assets/                     ← logo-mark.svg, logo-wordmark.svg
SKILL.md                    ← Agent-Skill front matter for download/Claude Code use
readme.md                   ← this file
```

---

## 10 · Anti-slop rules (read before generating anything)

1. **No Inter, no system-ui as the brand face.** Space Grotesk / Hanken Grotesk / JetBrains Mono only.
2. **No gradients as fills.** Flat slate + hairlines + the dot-grid backplate. (Sparkline area-fill is the only exception.)
3. **No 12–24px card radii + soft glow shadows.** Tight radii (`5–7px`) + inset top-highlight depth.
4. **No purple/blue "AI" palette.** The accent is electric lime; semantics are green/amber/red/grey + one cyan.
5. **No emoji, no rounded-corner-with-left-color-border "callout" cards** (the accent strip is 3px and structural, not decorative).
6. **No perfectly symmetric 3-up card walls** as a default. Group by capability, vary column counts, keep the asymmetric log rail.
7. **Numbers are mono + tabular, always.** Never set figures in the sans UI face.
8. **Motion settles, never bounces.** No spring easing on chrome; meters sweep, lines slide, dots pulse once.
9. **Title Case is banned** for headings; provider nouns stay lowercase.
10. **Don't invent semantic colors.** If a state needs a color, it's one of the five gauge tones.
