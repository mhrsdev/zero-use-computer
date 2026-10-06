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
  # sway's pid is written by the session below (it is started in there).
  if [ -s "$XDG_RUNTIME_DIR/sway.pid" ]; then
    kill "$(cat "$XDG_RUNTIME_DIR/sway.pid")" 2>/dev/null || true
  fi
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
  # This sway'"'"'s own XWayland display (sway gives it to what it starts).
  swaymsg -q exec "printenv DISPLAY > $XDG_RUNTIME_DIR/display"
  for _ in $(seq 50); do [ -s "$XDG_RUNTIME_DIR/display" ] && break; sleep 0.1; done
  DISPLAY="$(cat "$XDG_RUNTIME_DIR/display" 2>/dev/null || true)"
  export DISPLAY
  pids=()
  for c in /usr/libexec/at-spi-bus-launcher /usr/lib/at-spi2-core/at-spi-bus-launcher; do
    [ -x "$c" ] && { "$c" --launch-immediately >"$XDG_RUNTIME_DIR/atspi.log" 2>&1 & pids+=($!); break; }
  done
  for r in /usr/libexec/at-spi2-registryd /usr/lib/at-spi2-core/at-spi2-registryd; do
    [ -x "$r" ] && { "$r" >"$XDG_RUNTIME_DIR/atspi-reg.log" 2>&1 & pids+=($!); break; }
  done
  sleep 1
  IFS=";" read -ra apps <<< "$CU_APPS"
  n=0
  for a in "${apps[@]}"; do
    n=$((n + 1))
    [ -n "$a" ] && { bash -c "$a" >"$XDG_RUNTIME_DIR/app$n.log" 2>&1 & pids+=($!); }
  done
  sleep "$CU_SETTLE"
  shift
  status=0
  "$@" || status=$?
  kill "${pids[@]}" 2>/dev/null || true
  kill "$(cat "$XDG_RUNTIME_DIR/sway.pid")" 2>/dev/null || true
  exit $status
' "$conf" _ "$@" 2> >(grep -v "dbus-daemon" >&2)
