#!/usr/bin/env bash
# Run a command inside a throwaway headless Wayland desktop: sway (wlroots,
# the same protocols Hyprland speaks) with XWayland, a private D-Bus session
# and the AT-SPI accessibility bus, with some apps pre-launched.
#
#   APPS="gtk3-widget-factory" scripts/wayland-session.sh <command...>
#
# APPS is a ';'-separated list of commands started (in the background, as
# native Wayland clients) before <command> runs. SETTLE seconds (default 3)
# are given for them to register. CU_SCREEN sets the output size (default
# 1280x800), CU_SCALE its scale (default 1).
set -euo pipefail

size="${CU_SCREEN:-1280x800}"
scale="${CU_SCALE:-1}"
settle="${SETTLE:-3}"

export XDG_RUNTIME_DIR
XDG_RUNTIME_DIR="$(mktemp -d /tmp/cu-wl-XXXXXX)"
chmod 700 "$XDG_RUNTIME_DIR"
export WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman
export XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=sway
export NO_AT_BRIDGE=0 ACCESSIBILITY_ENABLED=1 GNOME_ACCESSIBILITY=1
export GDK_BACKEND=wayland QT_QPA_PLATFORM=wayland
export CU_APPS="${APPS:-}" CU_SETTLE="$settle"

conf="$XDG_RUNTIME_DIR/sway.conf"
cat >"$conf" <<EOF
output HEADLESS-1 resolution $size position 0 0 scale $scale
xwayland enable
default_border none
focus_follows_mouse no
EOF

cleanup() {
  [ -n "${sway_pid:-}" ] && kill "$sway_pid" 2>/dev/null || true
  rm -rf "$XDG_RUNTIME_DIR"
}
trap cleanup EXIT

dbus-run-session -- bash -c '
  sway -c "$0" >/tmp/cu-sway.log 2>&1 &
  echo $! > "$XDG_RUNTIME_DIR/sway.pid"
  for _ in $(seq 100); do
    ls "$XDG_RUNTIME_DIR"/wayland-[0-9] >/dev/null 2>&1 && ls "$XDG_RUNTIME_DIR"/sway-ipc.* >/dev/null 2>&1 && break
    sleep 0.1
  done
  WAYLAND_DISPLAY="$(basename "$(ls "$XDG_RUNTIME_DIR"/wayland-[0-9] | head -1)")"
  SWAYSOCK="$(ls "$XDG_RUNTIME_DIR"/sway-ipc.* | head -1)"
  export WAYLAND_DISPLAY SWAYSOCK
  # XWayland starts on demand; its DISPLAY is in the sway log.
  export DISPLAY="${DISPLAY:-:0}"
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
  shift
  status=0
  "$@" || status=$?
  kill "$(cat "$XDG_RUNTIME_DIR/sway.pid")" 2>/dev/null || true
  exit $status
' "$conf" _ "$@" 2> >(grep -v "dbus-daemon" >&2)
