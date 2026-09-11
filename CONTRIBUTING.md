# Contributing to fetchira

PRs and issues welcome.

## Dev

Rust, cmake, Perl, pkg-config, and a C/C++ toolchain (Xcode CLT on Mac, `build-essential` on Linux). The TLS-impersonation dep bundles BoringSSL.

```sh
cargo build --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all
sh tests/install-installer.sh
python3 tests/cli_install.py
python3 tests/upgrade_013.py
node tools/build_webui.js
node tools/check_webui_assets.js
git diff --exit-code -- webui
```

On a memory-limited Linux VPS, run `cargo test --locked --all -j 1` and finish the Docker build first; concurrent test linkers can exhaust RAM.

The Rust suite uses local mocks; the two ignored browser/provider tests require real sessions
and are not release gates. The installer/API checks use temporary homes. Never point tests at
your normal Fetchira home. CI runs the checks above on pull requests and pushes to `main`.

After editing `.jsx`, rebuild the checked-in `.js` with `node tools/build_webui.js` and review
both changes. A nonempty webui diff is expected while developing; the final diff check should
pass after committing the generated files. No npm install or runtime Babel is needed.

For a local MCP smoke check, create an empty `fetchira.toml` in a temporary directory and run:

```sh
python3 tests/mcp_smoke.py --binary target/debug/fetchira --home /absolute/path/to/test-home
```

For Docker runtime checks without provider calls:

```sh
docker build -t fetchira:smoke .
python3 tests/hosted_container.py fetchira:smoke
```

This checks the read-only container, hosted auth/assets and headful Chrome on Xvfb. The image
workflow runs it on Linux amd64 before publishing. Deployment: [docs/hosted.md](docs/hosted.md).

## Pull requests

- One change per PR.
- Match the surrounding code. Smallest diff that works.
- [Conventional Commits](https://www.conventionalcommits.org/), short subject only (`feat:`, `fix:`, `docs:`).
- Contributions are under Apache-2.0, same as the repo.
