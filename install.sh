#!/bin/sh
# Install fetchira: download the prebuilt release binary for this platform (verified),
# fall back to a source build when run from a checkout, then seed config so MCP clients
# can launch it from anywhere. Re-runnable; never overwrites existing config/sessions.
#
#   curl -fsSL https://raw.githubusercontent.com/ImmuneFOMO/fetchira/main/install.sh | sh
#
# Env: BIN_DST (install dir, default ~/.local/bin), FETCHIRA_VERSION (pin a tag like v0.2.0),
#      FETCHIRA_FROM_SOURCE=1 or `--from-source` (force building from a checkout).
set -eu

REPO="ImmuneFOMO/fetchira"
BIN_DST="${BIN_DST:-$HOME/.local/bin}"
TMP_ROOT="$(mktemp -d)"
cleanup() { rm -rf "$TMP_ROOT"; }
trap cleanup 0
trap 'exit 1' HUP INT TERM

# Source checkout only if this script really sits in fetchira's own repo. Under curl|sh, $0 is
# "sh" (not a file) and dirname resolves to the cwd — guard against a stray Cargo.toml there by
# requiring $0 to be a file and the Cargo.toml to actually be fetchira's.
SCRIPT_DIR="$(cd "$(dirname "$0")" 2>/dev/null && pwd || true)"
IN_REPO=""
if [ -f "$0" ] && [ -n "$SCRIPT_DIR" ] && grep -q '^name = "fetchira"' "$SCRIPT_DIR/Cargo.toml" 2>/dev/null; then
  IN_REPO="$SCRIPT_DIR"
fi

STAGED=""  # path to the freshly built/downloaded binary

target_triple() {
  case "$(uname -s)/$(uname -m)" in
    Darwin/arm64)  echo "aarch64-apple-darwin" ;;
    Darwin/x86_64) echo "x86_64-apple-darwin" ;;
    Linux/aarch64) echo "aarch64-unknown-linux-gnu" ;;
    Linux/x86_64)  echo "x86_64-unknown-linux-gnu" ;;
    *) echo "" ;;
  esac
}

valid_sha256() {
  [ "${#1}" -eq 64 ] || return 1
  case "$1" in
    *[!0123456789abcdefABCDEF]*) return 1 ;;
  esac
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    digest_output="$(sha256sum "$1")" || return 1
  else
    digest_output="$(shasum -a 256 "$1")" || return 1
  fi
  set -- $digest_output
  valid_sha256 "${1:-}" || return 1
  printf '%s\n' "$1"
}

download_prebuilt() {
  triple="$1"
  dir="$TMP_ROOT/dl"
  # Resolve the latest tag from the release redirect (no GitHub API rate limit).
  if [ -n "${FETCHIRA_VERSION:-}" ]; then
    tag="$FETCHIRA_VERSION"
  else
    latest_url="$(curl -sIL -o /dev/null -w '%{url_effective}' \
      "https://github.com/$REPO/releases/latest" 2>/dev/null)" || {
      echo "    could not resolve latest release (set FETCHIRA_VERSION=vX.Y.Z to pin)"
      return 1
    }
    tag="$(printf '%s\n' "$latest_url" | sed -n 's#.*/releases/tag/##p')"
  fi
  [ -n "$tag" ] || { echo "    could not resolve latest release (set FETCHIRA_VERSION=vX.Y.Z to pin)"; return 1; }
  mkdir -p "$dir"
  url="https://github.com/$REPO/releases/download/$tag/fetchira-$triple.tar.xz"
  echo "==> downloading fetchira $tag ($triple)…"
  curl -fsSL "$url" -o "$dir/f.tar.xz" || return 1
  if curl -fsSL "$url.sha256" -o "$dir/f.sha256" 2>/dev/null && [ -s "$dir/f.sha256" ]; then
    expected_checksum="$(awk 'NF { print $1; exit }' "$dir/f.sha256")" || {
      echo "    could not read checksum — aborting download" >&2
      return 1
    }
    actual_checksum="$(sha256_of "$dir/f.tar.xz")" || {
      echo "    could not calculate checksum — aborting download" >&2
      return 1
    }
    valid_sha256 "$expected_checksum" || {
      echo "    invalid checksum — aborting download" >&2
      return 1
    }
    [ "$expected_checksum" = "$actual_checksum" ] || {
      echo "    checksum mismatch — aborting download" >&2
      return 1
    }
  else
    echo "    checksum unavailable — refusing to install unverifiable artifact" >&2
    return 1
  fi
  tar -xJf "$dir/f.tar.xz" -C "$dir" || {
    echo "    could not extract downloaded archive" >&2
    return 1
  }
  STAGED="$(find "$dir" -type f -name fetchira | head -1)"
  [ -n "$STAGED" ] || {
    echo "    downloaded archive does not contain fetchira" >&2
    return 1
  }
}

build_from_source() {
  [ -n "$IN_REPO" ] || {
    echo "prebuilt unavailable or download failed for $(uname -s)/$(uname -m)" >&2
    echo "source install: cargo install --locked --git https://github.com/ImmuneFOMO/fetchira" >&2
    echo "(needs Rust, cmake, Perl, pkg-config, and a C/C++ toolchain; put ~/.cargo/bin on PATH)" >&2
    exit 1
  }
  echo "==> building fetchira (release)…"
  ( cd "$IN_REPO" && cargo build --release )
  STAGED="$IN_REPO/target/release/fetchira"
}

install_binary() {
  mkdir -p "$BIN_DST"
  tmpbin="$BIN_DST/.fetchira.new.$$"
  cp "$STAGED" "$tmpbin"
  chmod 0755 "$tmpbin"
  if [ "$(uname -s)" = "Darwin" ]; then
    xattr -d com.apple.quarantine "$tmpbin" 2>/dev/null || true
    # Apple Silicon SIGKILLs unsigned binaries; ad-hoc sign is enough (no Developer ID).
    command -v codesign >/dev/null 2>&1 && codesign --force --sign - "$tmpbin" 2>/dev/null || true
  fi
  verify_home="$TMP_ROOT/verify-home"
  mkdir -p "$verify_home"
  if ! FETCHIRA_HOME="$verify_home" "$tmpbin" --version >/dev/null 2>&1; then
    echo "    new binary failed to start on this system; keeping existing installation" >&2
    rm -f "$tmpbin"
    exit 1
  fi
  # Atomic replace via rename within the same dir: a running agent keeps the old inode,
  # new spawns get the new binary. Avoids the pkill/corruption dance.
  mv -f "$tmpbin" "$BIN_DST/fetchira"
  echo "==> installed binary: $BIN_DST/fetchira"
}

# Migrate/seed config — only meaningful from a checkout (examples live there); curl|sh
# users configure via `fetchira setup`.
seed_config() {
  HOME_DIR="${FETCHIRA_HOME:-${XDG_CONFIG_HOME:-$HOME/.config}/fetchira}"
  mkdir -p "$HOME_DIR"
  [ -n "$IN_REPO" ] || return 0
  for f in fetchira.toml .env usage.db usage.db-shm usage.db-wal; do
    if [ ! -e "$HOME_DIR/$f" ] && [ -e "$IN_REPO/$f" ]; then
      cp "$IN_REPO/$f" "$HOME_DIR/$f" && echo "    migrated $f -> $HOME_DIR/"
    fi
  done
  [ -e "$HOME_DIR/fetchira.toml" ] || [ ! -e "$IN_REPO/fetchira.toml.example" ] || { cp "$IN_REPO/fetchira.toml.example" "$HOME_DIR/fetchira.toml"; echo "    seeded fetchira.toml"; }
  [ -e "$HOME_DIR/.env" ] || [ ! -e "$IN_REPO/.env.example" ] || { cp "$IN_REPO/.env.example" "$HOME_DIR/.env"; echo "    seeded .env"; }
}

triple="$(target_triple)"
if [ "${1:-}" = "--from-source" ] || [ "${FETCHIRA_FROM_SOURCE:-}" = "1" ]; then
  build_from_source
elif [ -n "$triple" ] && download_prebuilt "$triple"; then
  :
else
  echo "==> no prebuilt available (or download failed); falling back to source build…"
  build_from_source
fi

install_binary
seed_config

echo "==> fetchira home: $HOME_DIR"
echo "    register as MCP later:  claude mcp add fetchira -s user -- $BIN_DST/fetchira"
case ":$PATH:" in
  *":$BIN_DST:"*) ;;
  *) echo "    not on PATH yet:     export PATH=\"$BIN_DST:\$PATH\"" ;;
esac

# Launch the interactive setup TUI when run in a real terminal.
if [ -t 0 ] && [ -t 1 ]; then
  echo
  "$BIN_DST/fetchira" setup
else
  echo "    run \`$BIN_DST/fetchira setup\` to configure providers."
fi
