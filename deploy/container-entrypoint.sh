#!/bin/sh
set -eu

# Keep the executable on the writable data volume so hosted self-update can atomically replace
# it even though the rest of the image is read-only. The first boot seeds the volume copy.
# Headful mode needs a display; the image ships Xvfb for exactly this. Started below, before
# exec, so the fetchira process (and every Chrome it spawns) inherits DISPLAY.
runtime=/data/fetchira
image_version=$(/usr/local/bin/fetchira --version 2>/dev/null | awk 'NR == 1 {print $2}')
runtime_version=$([ -x "$runtime" ] && "$runtime" --version 2>/dev/null | awk 'NR == 1 {print $2}' || true)
image_sha=$(sha256sum /usr/local/bin/fetchira | awk '{print $1}')
installed_image_sha=$(cat /data/.fetchira-image.sha256 2>/dev/null || true)

# Seed first boot and roll forward when the image changed. Keep a newer self-updated volume
# binary across restarts; the marker also makes same-version source/dev image rollouts work.
if [ ! -x "$runtime" ] || {
  [ "$image_sha" != "$installed_image_sha" ] && {
    [ -z "$runtime_version" ] || [ -z "$image_version" ] ||
      [ "$(printf '%s\n' "$runtime_version" "$image_version" | sort -V | tail -n 1)" = "$image_version" ]
  }
}; then
  tmp="$runtime.tmp.$$"
  cp /usr/local/bin/fetchira "$tmp"
  chmod 0755 "$tmp"
  mv -f "$tmp" "$runtime"
  printf '%s\n' "$image_sha" > /data/.fetchira-image.sha256
fi

# Xvfb only listens on its unix socket (-nolisten tcp): nothing network-facing is added.
if [ -n "${DISPLAY:-}" ] && command -v Xvfb >/dev/null 2>&1; then
  Xvfb "$DISPLAY" -screen 0 1280x1024x24 -nolisten tcp -nolisten local &
fi
exec "$runtime" "$@"
