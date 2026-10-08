#!/usr/bin/env bash
# Live Qt test harness: a headless X server, a private D-Bus session and
# accessibility bus, the Qt fixture and the crashing fixture (restarted
# whenever it quits), then live_qt.py against the server.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
display="${CU_DISPLAY:-:97}"

py=""
for cand in "${PYTHON:-}" python3 python3.12 python3.11; do
  [ -n "$cand" ] || continue
  if command -v "$cand" >/dev/null 2>&1 && "$cand" -c 'import PyQt6.QtWidgets; from gi.repository import Gio' >/dev/null 2>&1; then
    py="$cand"; break
  fi
done
[ -n "$py" ] || { echo "no Python with PyQt6 and PyGObject found"; exit 1; }
echo "using python: $py"

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
  py=$1 here=$2 bin=$3 work=$4
  /usr/libexec/at-spi-bus-launcher --launch-immediately >"$work/atspi.log" 2>&1 &
  launcher=$!
  /usr/libexec/at-spi2-registryd >"$work/registry.log" 2>&1 &
  registry=$!
  sleep 1
  "$py" "$here/fixtures/qt_app.py" >"$work/qt.log" 2>&1 &
  qt=$!
  # Restarted whenever it quits, as a crashed compositor is.
  ( while true; do
      "$py" "$here/fixtures/crashing_app.py" "$work/crashes" 2>>"$work/crashing.log" &
      echo $! >"$work/crashing.pid"
      wait $! || true
      sleep 0.3
    done ) &
  loop=$!
  sleep 3
  status=0
  "$py" "$here/live_qt.py" "$bin" "$qt" "$work/crashes" || status=$?
  kill "$loop" 2>/dev/null || true
  kill "$(cat "$work/crashing.pid")" "$qt" "$registry" "$launcher" 2>/dev/null || true
  exit $status
' _ "$py" "$here" "$bin" "$work"
