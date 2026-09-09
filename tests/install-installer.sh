#!/bin/sh
set -eu
unset FETCHIRA_FROM_SOURCE 2>/dev/null || :

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TMP_ROOT"' 0 HUP INT TERM

STUB_DIR="$TMP_ROOT/stubs"
mkdir "$STUB_DIR"
for command in uname curl sha256sum tar; do
  ln -s "$ROOT/tests/install-stub.sh" "$STUB_DIR/$command"
done

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

run_success() {
  mode="$1"
  os="$2"
  arch="$3"
  expected="$4"
  dst="$TMP_ROOT/dst-$mode"
  home="$TMP_ROOT/home-$mode"
  log="$TMP_ROOT/log-$mode"
  mkdir -p "$dst" "$home"
  : > "$log"

  if env PATH="$STUB_DIR:$PATH" HOME="$home" BIN_DST="$dst" \
    FETCHIRA_HOME="$home/config" FETCHIRA_VERSION=v0.0.0 \
    STUB_MODE="$mode" STUB_UNAME_S="$os" STUB_UNAME_M="$arch" STUB_LOG="$log" \
    sh -s < "$ROOT/install.sh" >/dev/null 2>&1; then
    status=0
  else
    status=$?
  fi
  [ "$status" -eq 0 ] || fail "$mode should install successfully"
  [ -x "$dst/fetchira" ] || fail "$mode did not install fetchira"
  grep -F "fetchira-$expected.tar.xz" "$log" >/dev/null || fail "$mode used the wrong target"
}

run_failure() {
  mode="$1"
  output="$TMP_ROOT/output-$mode"
  dst="$TMP_ROOT/dst-$mode"
  home="$TMP_ROOT/home-$mode"
  mkdir -p "$dst" "$home"

  if env PATH="$STUB_DIR:$PATH" HOME="$home" BIN_DST="$dst" \
    FETCHIRA_HOME="$home/config" FETCHIRA_VERSION=v0.0.0 \
    STUB_MODE="$mode" STUB_UNAME_S=Linux STUB_UNAME_M=aarch64 STUB_LOG="$TMP_ROOT/log-$mode" \
    sh -s < "$ROOT/install.sh" >"$output" 2>&1; then
    status=0
  else
    status=$?
  fi
  [ "$status" -ne 0 ] || fail "$mode should fail"
  [ ! -e "$dst/fetchira" ] || fail "$mode installed an invalid artifact"
  case "$mode" in
    checksum-mismatch) grep -F "checksum mismatch" "$output" >/dev/null || fail "$mode has no checksum error" ;;
    checksum-missing) grep -F "checksum unavailable" "$output" >/dev/null || fail "$mode has no missing-checksum error" ;;
    checksum-tool-failure) grep -F "could not calculate checksum" "$output" >/dev/null || fail "$mode has no hash-tool error" ;;
    extract-failure) grep -F "could not extract" "$output" >/dev/null || fail "$mode has no extract error" ;;
  esac
}

run_existing_binary_failure() {
  mode=binary-verification-failure
  output="$TMP_ROOT/output-$mode"
  dst="$TMP_ROOT/dst-$mode"
  home="$TMP_ROOT/home-$mode"
  mkdir -p "$dst" "$home"
  printf 'original fetchira\n' > "$dst/fetchira"
  chmod 0755 "$dst/fetchira"

  if env PATH="$STUB_DIR:$PATH" HOME="$home" BIN_DST="$dst" \
    FETCHIRA_HOME="$home/config" FETCHIRA_VERSION=v0.0.0 \
    STUB_MODE="$mode" STUB_UNAME_S=Linux STUB_UNAME_M=aarch64 STUB_LOG="$TMP_ROOT/log-$mode" \
    sh -s < "$ROOT/install.sh" >"$output" 2>&1; then
    status=0
  else
    status=$?
  fi
  [ "$status" -ne 0 ] || fail "$mode should fail"
  [ "$(cat "$dst/fetchira")" = "original fetchira" ] || fail "$mode replaced the working binary"
  [ -z "$(find "$dst" -maxdepth 1 -name '.fetchira.new.*' -print)" ] || fail "$mode left a temporary binary"
  grep -F "failed to start" "$output" >/dev/null || fail "$mode has no startup error"
}

sh -n "$ROOT/install.sh"
run_success linux-aarch64 Linux aarch64 aarch64-unknown-linux-gnu
run_success linux-x86_64 Linux x86_64 x86_64-unknown-linux-gnu
run_success macos-arm64 Darwin arm64 aarch64-apple-darwin
run_success macos-x86_64 Darwin x86_64 x86_64-apple-darwin
run_failure checksum-mismatch
run_failure checksum-missing
run_failure checksum-tool-failure
run_failure extract-failure
run_existing_binary_failure

echo "installer checks passed"
