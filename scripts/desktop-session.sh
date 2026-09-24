#!/usr/bin/env bash
# Run a command inside a throwaway headless Linux desktop: Xvfb + a private
# D-Bus session + the AT-SPI accessibility bus, with some apps pre-launched.
#
#   APPS="gtk3-widget-factory;python3 fixture.py" scripts/desktop-session.sh <command...>
#
# APPS is a ';'-separated list of commands started (in the background) before
# <command> runs. SETTLE seconds (default 3) are given for them to register.
set -euo pipefail

display="${CU_DISPLAY:-:95}"
size="${CU_SCREEN:-1280x1024x24}"
settle="${SETTLE:-3}"

Xvfb "$display" -screen 0 "$size" -nolisten tcp >/tmp/cu-xvfb.log 2>&1 &
xvfb_pid=$!
trap 'kill "$xvfb_pid" 2>/dev/null || true' EXIT
sleep 1

export DISPLAY="$display"
export NO_AT_BRIDGE=0 ACCESSIBILITY_ENABLED=1 GNOME_ACCESSIBILITY=1
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp/cu-xdg-session}"
mkdir -p "$XDG_RUNTIME_DIR"
export CU_APPS="${APPS:-}" CU_SETTLE="$settle"

dbus-run-session -- bash -c '
  for c in /usr/libexec/at-spi-bus-launcher /usr/lib/at-spi2-core/at-spi-bus-launcher; do
    [ -x "$c" ] && { "$c" --launch-immediately >/tmp/cu-atspi.log 2>&1 & break; }
  done
  for r in /usr/libexec/at-spi2-registryd /usr/lib/at-spi2-core/at-spi2-registryd; do
    [ -x "$r" ] && { "$r" >/tmp/cu-atspi-reg.log 2>&1 & break; }
  done
  sleep 1
  IFS=";" read -ra apps <<< "$CU_APPS"
  for a in "${apps[@]}"; do
    [ -n "$a" ] && { bash -c "$a" >/tmp/cu-app.log 2>&1 & }
  done
  sleep "$CU_SETTLE"
  "$@"
' _ "$@" 2> >(grep -v "dbus-daemon" >&2)
