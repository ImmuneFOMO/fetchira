# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.12](https://github.com/ImmuneFOMO/fetchira/compare/v0.1.11...v0.1.12) - 2026-07-16

### Fixed

- *(router)* bound live usage reads so a stalled provider can't hang usage
- *(ui)* drop double focus outline on Input/Select fields

## [0.1.11](https://github.com/ImmuneFOMO/fetchira/compare/v0.1.10...v0.1.11) - 2026-07-13

### Added

- *(update)* schema-aware gate with instance registry + idle waiter
- *(ui)* self-restart after update + periodic update check

### Fixed

- *(ui)* auto-dismiss getting started once all steps are done

## [0.1.10](https://github.com/ImmuneFOMO/fetchira/compare/v0.1.9...v0.1.10) - 2026-07-13

### Fixed

- *(ui)* keep account row menu above the table card clip

## [0.1.9](https://github.com/ImmuneFOMO/fetchira/compare/v0.1.8...v0.1.9) - 2026-07-11

### Added

- *(mcp)* save generated images to disk and return the path

### Other

- *(readme)* problem/solution intro + agent setup prompt

## [0.1.8](https://github.com/ImmuneFOMO/fetchira/compare/v0.1.7...v0.1.8) - 2026-07-09

### Added

- *(ui)* first-run onboarding, ghost provider cards, one-click update
- *(proxy)* accept raw webshare lines in both field orders

### Fixed

- *(ui)* keep checklist items clickable when done
- *(ui)* flag stale-token tabs instead of silent mock fallback

## [0.1.7](https://github.com/ImmuneFOMO/fetchira/compare/v0.1.6...v0.1.7) - 2026-07-07

### Added

- *(ui)* fold the debug firehose into activity, drop the debug tab
- *(ui)* drag-to-reorder priority chips with ranked failover chain
- *(router)* quality-rank image and deep-research defaults
- *(router)* rebalance default provider order around quota economics
- *(ui)* routing priority panel in the dashboard
- *(cli)* priority command to view and set the provider order
- *(router)* user-configurable provider priority per capability
- *(serper)* scrape endpoint for the read tool
- *(accounts)* edit an account's proxy in CLI and UI

### Fixed

- *(ui)* uniform account rows — limits drawer + overflow action menu
- *(ui)* show steel auto-escalation hint on the read priority row

### Other

- *(readme)* quickstart, provider picking guide, full config reference
- *(mcp)* correct router instructions and slim tool descriptions

## [0.1.6](https://github.com/ImmuneFOMO/fetchira/compare/v0.1.5...v0.1.6) - 2026-07-06

### Added

- *(ui)* per-card loader until each provider's live figure lands
- *(accounts)* capture account email — masked chip + duplicate detection
- *(ui)* rename accounts from the dashboard
- *(ui)* list chatgpt_web in the add-account dropdown

### Fixed

- *(login)* browser picker, Firefox WAL cookie capture, and login logs
- *(ui)* drop a new web account when its first login fails

### Other

- *(ui)* boot the live fetch during warm-up so no global spinner shows
- *(ui)* paint accounts instantly, warm live limits in the background

## [0.1.5](https://github.com/ImmuneFOMO/fetchira/compare/v0.1.4...v0.1.5) - 2026-07-05

### Other

- *(ui)* paint dashboard before the cold quota fan-out

## [0.1.4](https://github.com/ImmuneFOMO/fetchira/compare/v0.1.3...v0.1.4) - 2026-07-05

### Added

- *(web)* attach multiple files per turn across grok/gemini/chatgpt
- *(ui)* show the fetchira gauge as the dashboard favicon
- *(ui)* deep-link dashboard tab and add-account modal via ?tab= / ?modal=
- *(ui)* $-balance bar turns red when funds can't cover a single request
- *(ui)* add the full progress bar to $-balance provider cards for parity
- *(ui)* remove top stats panel, masonry-pack the capability matrix
- *(ui)* show real $ balance for exa/parallel/steel, size provider cards to content
- *(providers)* remove jina and perplexity_web providers
- *(ui)* remove savings odometer (can't tell free vs paid), mark estimate quotas with approx sign
- *(debug)* capture raw HTTP request/response per call, redact secrets, show in detail
- *(ui)* newest-first route log (top) + prepend live stream
- *(ui)* open request/response detail from the Activity route log
- *(niche)* native/rewrite history badge + depth end-to-end (exa/grok)
- *(ui)* savings odometer, burn radar, dead-end counter, capability matrix
- *(tools)* usage(provider) disclosure, mode escape hatch, steel proxy fix, upload folded into search
- *(search)* cross-provider niche knobs (topic/recency/domains) + provider fixes
- *(providers)* live per-account balance for all search/scrape providers
- *(web)* full gemini/grok/chatgpt support (search, deep research, images, uploads, live limits)
- *(chatgpt)* add chatgpt_web provider (chat, deep research, image, limits)

### Fixed

- *(gemini)* send full cookie set so /app stops bouncing to /sorry
- *(ui)* masonry-pack provider cards so uneven heights don't leave mid-column gaps
- *(session)* persist rolling NextAuth cookies (exa/parallel) so dashboard sessions don't expire
- *(steel)* count balance as proxied reads (~$0.015, incl. proxy bandwidth), not flat $0.005
- *(login)* auto-close the browser window after capturing the session
- *(chatgpt)* create_image returns image bytes, fix picker scans

### Other

- Merge origin/main; fold grok upload robustness into file attach
- redraw architecture diagram for current providers, drop "free" from the tagline
- refresh dashboard screenshots, update README to current providers and features
- *(ui)* refresh dashboard screenshots for the web-session model/limit catalog

## [0.1.3](https://github.com/ImmuneFOMO/fetchira/compare/v0.1.2...v0.1.3) - 2026-06-29

### Fixed

- live dashboard refresh + diagnostic grok_web error

## [0.1.2](https://github.com/ImmuneFOMO/fetchira/compare/v0.1.1...v0.1.2) - 2026-06-29

### Added

- *(ui)* persistent request/response debug log

### Fixed

- *(grok)* mint real x-statsig-id so research stops 403ing

## [0.1.1](https://github.com/ImmuneFOMO/fetchira/compare/v0.1.0...v0.1.1) - 2026-06-29

### Added

- *(web)* support linux/firefox login and manual session paste

### Other

- add brew/curl install methods and update command
