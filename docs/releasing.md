# Releasing fetchira

Push to `main` with conventional commits → **release-plz** keeps one "Release PR" open
(version bump + changelog) → you **merge it** when you want to ship → it tags `vX.Y.Z` →
**dist** builds macOS + Linux binaries, publishes a GitHub Release (notes = the changelog),
and pushes the Homebrew formula to the tap.

```
commit (feat:/fix:/…) → push main
   └─ release-plz: opens/updates Release PR (bump + CHANGELOG)
merge the Release PR
   └─ release-plz: tags vX.Y.Z   (needs RELEASE_PLZ_TOKEN, else the tag won't trigger dist)
        └─ dist: build mac arm/x64 + linux x64 → GitHub Release + Homebrew formula
```

## One-time setup (on GitHub — do this before the first release)

1. **Create the tap repo** `ImmuneFOMO/homebrew-tap` (public, can be empty). dist pushes
   `Formula/fetchira.rb` into it on every release.

2. **Add two repo secrets** in `ImmuneFOMO/fetchira` → Settings → Secrets and variables → Actions:
   | Secret | What | How to make it |
   |---|---|---|
   | `RELEASE_PLZ_TOKEN` | so the tag release-plz pushes actually triggers the dist build (a tag from the default `GITHUB_TOKEN` does **not** start another workflow) | fine-grained PAT on this repo with **Contents: Read/Write** + **Pull requests: Read/Write** |
   | `HOMEBREW_TAP_TOKEN` | lets dist push the formula into the tap repo | classic PAT with `repo` scope (or fine-grained **Contents: Write** on `homebrew-tap`) |

3. **Allow Actions to open PRs**: Settings → Actions → General → Workflow permissions →
   enable "Allow GitHub Actions to create and approve pull requests".

4. **Cut the first release** by tagging the current version. This both gives release-plz its
   baseline (so the next bump is computed from here) and — because the dist workflow triggers
   on any version tag — **publishes a real `v0.1.0` release** (binaries + Homebrew formula).
   Make sure steps 1–3 are done first.
   ```sh
   git tag v0.1.0 && git push origin v0.1.0
   ```

## Day-to-day

- Commit with [Conventional Commits](https://www.conventionalcommits.org/) (already the
  house style): `feat:` → minor bump, `fix:` → patch, `feat!:`/`BREAKING CHANGE:` → major.
- Push to `main`. A "Release PR" appears/updates. Ignore it as long as you like.
- When ready to ship, **merge the Release PR**. The release builds and publishes itself.

## How users install / update

| Channel | Install | Update |
|---|---|---|
| Homebrew | `brew install ImmuneFOMO/tap/fetchira` | `brew upgrade fetchira` |
| curl \| sh | `curl -fsSL https://raw.githubusercontent.com/ImmuneFOMO/fetchira/main/install.sh \| sh` | re-run the same line, or `fetchira update` |
| manual | download the `fetchira-<target>.tar.xz` from the Release | download the newer one (`xattr -d com.apple.quarantine` on macOS if blocked) |

`install.sh` downloads the prebuilt binary for the platform, verifies its checksum, and
replaces `~/.local/bin/fetchira` atomically (won't break a running agent). With no prebuilt
for the platform it builds from source (from a checkout).

## Notes

- **License**: `Apache-2.0` (`Cargo.toml` + `LICENSE` file). Used in the Homebrew formula.
- **Targets**: macOS arm64/x64 + Linux arm64/x64 (glibc). `musl` is still deferred —
  `wreq` pulls BoringSSL, which makes those cross-builds fiddly.
- **macOS signing**: ad-hoc only (no Apple Developer account). brew and `curl|sh` don't set
  the quarantine flag, so Gatekeeper doesn't block; only a manual browser download does.
- **Regenerate CI** after editing `dist-workspace.toml`: `dist init --yes` (or `dist generate`).
