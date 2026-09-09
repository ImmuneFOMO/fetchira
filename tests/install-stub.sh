#!/bin/sh
set -eu

command="$(basename "$0")"
GOOD_DIGEST=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
BAD_DIGEST=fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210

case "$command" in
  uname)
    case "${1:-}" in
      -s) printf '%s\n' "$STUB_UNAME_S" ;;
      -m) printf '%s\n' "$STUB_UNAME_M" ;;
      *) exit 1 ;;
    esac
    ;;
  sha256sum)
    [ "$STUB_MODE" = checksum-tool-failure ] && exit 1
    printf '%s  %s\n' "$GOOD_DIGEST" "$1"
    ;;
  tar)
    if [ "$STUB_MODE" = extract-failure ]; then
      exit 2
    fi
    destination=""
    previous=""
    for argument in "$@"; do
      [ "$previous" = -C ] && destination="$argument"
      previous="$argument"
    done
    [ -n "$destination" ] || exit 2
    mkdir -p "$destination"
    if [ "$STUB_MODE" = binary-verification-failure ]; then
      printf '#!/bin/sh\n[ "${FETCHIRA_HOME##*/}" = verify-home ] || exit 43\nexit 42\n' > "$destination/fetchira"
    else
      printf '#!/bin/sh\nexit 0\n' > "$destination/fetchira"
    fi
    chmod 0755 "$destination/fetchira"
    ;;
  curl)
    url=""
    output=""
    previous=""
    for argument in "$@"; do
      case "$argument" in
        http://*|https://*) url="$argument" ;;
      esac
      [ "$previous" = -o ] && output="$argument"
      previous="$argument"
    done
    [ -n "$url" ] || exit 2
    printf '%s\n' "$url" >> "$STUB_LOG"
    case "$url" in
      */releases/latest)
        printf 'https://github.com/ImmuneFOMO/fetchira/releases/tag/v0.0.0'
        ;;
      *.sha256)
        case "$STUB_MODE" in
          checksum-missing) exit 22 ;;
          checksum-mismatch) printf '%s  f.tar.xz\n' "$BAD_DIGEST" > "$output" ;;
          checksum-tool-failure) printf '\n' > "$output" ;;
          *) printf '%s  f.tar.xz\n' "$GOOD_DIGEST" > "$output" ;;
        esac
        ;;
      *)
        [ -n "$output" ] || exit 2
        printf 'stub archive\n' > "$output"
        ;;
    esac
    ;;
  *)
    echo "unknown stub command: $command" >&2
    exit 1
    ;;
esac
