#!/usr/bin/env bash
# Live web form test harness: a headless X server, a private D-Bus session
# and accessibility bus, then live_web.py (which starts the React fixture's
# backend and the browser) against the server.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
display="${CU_DISPLAY:-:96}"

browser="${CU_BROWSER:-}"
for cand in google-chrome google-chrome-stable chromium chromium-browser /opt/pw-browsers/chromium; do
  [ -n "$browser" ] && break
  command -v "$cand" >/dev/null 2>&1 && browser="$(command -v "$cand")"
done
[ -n "$browser" ] || { echo "no Chromium or Chrome found (set CU_BROWSER)"; exit 1; }
echo "using browser: $browser"

(cd "$root" && cargo build -q -p computer-use-mcp)
bin="$root/target/debug/computer-use-mcp"
work="$(mktemp -d)"
export COMPUTER_USE_HOME="$work/home" XDG_RUNTIME_DIR="$work/xdg" DISPLAY="$display"
mkdir -p "$COMPUTER_USE_HOME" "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"

Xvfb "$display" -screen 0 1280x1024x24 -nolisten tcp >"$work/xvfb.log" 2>&1 &
xvfb=$!
trap 'kill $xvfb 2>/dev/null || true' EXIT
sleep 1

dbus-run-session -- bash -euc '
  here=$1 bin=$2 browser=$3 work=$4
  /usr/libexec/at-spi-bus-launcher --launch-immediately >"$work/atspi.log" 2>&1 &
  launcher=$!
  /usr/libexec/at-spi2-registryd >"$work/registry.log" 2>&1 &
  registry=$!
  sleep 1
  # Chromium exposes its pages over AT-SPI only when accessibility is on
  # as it starts.
  dbus-send --session --print-reply --dest=org.a11y.Bus /org/a11y/bus \
    org.freedesktop.DBus.Properties.Set string:org.a11y.Status string:IsEnabled variant:boolean:true >/dev/null
  export ACCESSIBILITY_ENABLED=1
  status=0
  python3 "$here/live_web.py" "$bin" "$browser" || status=$?
  kill "$registry" "$launcher" 2>/dev/null || true
  exit $status
' _ "$here" "$bin" "$browser" "$work"
