#!/bin/sh
set -eu

url=${FETCHIRA_URL:-http://127.0.0.1:7879}
password=${FETCHIRA_ADMIN_PASSWORD_PLAIN:?set FETCHIRA_ADMIN_PASSWORD_PLAIN}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM

expect() {
  want=$1
  shift
  got=$(curl -sS -o "$work/body" -w '%{http_code}' "$@")
  [ "$got" = "$want" ] || { echo "expected HTTP $want, got $got: $(cat "$work/body")" >&2; exit 1; }
}

expect 200 "$url/healthz"
expect 200 "$url/readyz"
expect 200 "$url/version"

# A hosted deployment must expose its browser runtime, not defer the failure until the
# first ChatGPT/Gemini/Grok request. The endpoint is intentionally internal; this checks the
# process-level startup guard through the successful readiness response and keeps the contract
# in one place for image CI.

# Every browser asset referenced by the hosted entrypoint must be reachable. A missing
# embedded asset otherwise degrades to a blank page with only a browser-console error.
expect 200 "$url/admin"
cp "$work/body" "$work/index.html"
sed -n 's/.*\(src\|href\)="\(\/admin\/assets\/[^"?#]*\)".*/\2/p' "$work/index.html" \
  | sort -u > "$work/assets"
while IFS= read -r asset; do
  [ -n "$asset" ] || continue
  expect 200 "$url$asset"
done < "$work/assets"

if grep -Eq 'text/babel|vendor/babel|min\.jsx|\.jsx"' "$work/index.html"; then
  echo 'hosted entrypoint still references runtime Babel or JSX' >&2
  exit 1
fi

expect 401 "$url/admin/keys"
expect 401 -X POST -H 'content-type: application/json' --data '{"id":"must-not-exist","name":"csrf-check"}' "$url/admin/keys"

expect 200 -D "$work/headers" -c "$work/cookies" -X POST -H 'content-type: application/json' \
  --data "$(printf '%s' "$password" | sed 's/\\/\\\\/g;s/"/\\"/g' | awk '{printf "{\"password\":\"%s\"}",$0}')" "$url/admin/login"
[ "$(grep -ic '^set-cookie:' "$work/headers")" -ge 2 ] || { echo 'admin login must return separate Set-Cookie headers' >&2; exit 1; }
csrf=$(awk '$6 == "fetchira_csrf" {print $7}' "$work/cookies")
[ -n "$csrf" ] || { echo 'missing fetchira_csrf cookie' >&2; exit 1; }
expect 200 -b "$work/cookies" "$url/admin/keys"
expect 401 -b "$work/cookies" -X POST -H 'content-type: application/json' --data '{"id":"must-not-exist","name":"csrf-check"}' "$url/admin/keys"
expect 400 -b "$work/cookies" -X POST -H 'content-type: application/json' -H "x-csrf-token: $csrf" --data '{"id":"bad_id","name":"csrf-check"}' "$url/admin/keys"

echo "hosted smoke passed: $url"
