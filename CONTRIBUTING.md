# Contributing to fetchira

PRs and issues welcome.

## Dev

Rust, cmake, Perl, pkg-config, and a C/C++ toolchain (Xcode CLT on Mac, `build-essential` on Linux). The TLS-impersonation dep bundles BoringSSL.

```sh
cargo build --release
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all
sh tests/install-installer.sh
node tools/check_webui_assets.js
```

On a memory-limited Linux VPS, run `cargo test --locked --all -j 1` and finish the Docker build first; concurrent test linkers can exhaust RAM.

Local config: `~/.config/fetchira`.

## Pull requests

- One change per PR.
- Match the surrounding code. Smallest diff that works.
- [Conventional Commits](https://www.conventionalcommits.org/), short subject only (`feat:`, `fix:`, `docs:`).
- Contributions are under Apache-2.0, same as the repo.
