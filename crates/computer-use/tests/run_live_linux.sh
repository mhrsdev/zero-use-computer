#!/usr/bin/env bash
# Live Linux backend test harness.
#
# Brings up a headless X server (Xvfb), a private D-Bus session, the AT-SPI
# accessibility bus, and the GTK fixture app, then runs the `live_linux`
# integration test against them. Safe to run repeatedly.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
fixture="$here/fixtures/gtk_app.py"
display="${CU_DISPLAY:-:99}"

# Pick a Python that can import GTK 3.
py=""
for cand in "${PYTHON:-}" python3 python3.12 python3.11 python3.10; do
  [ -n "$cand" ] || continue
  if command -v "$cand" >/dev/null 2>&1 && \
     "$cand" -c 'import gi; gi.require_version("Gtk","3.0"); from gi.repository import Gtk' >/dev/null 2>&1; then
    py="$cand"; break
  fi
done
if [ -z "$py" ]; then
  echo "no Python with GTK 3 (python3-gi + gir1.2-gtk-3.0) found; skipping live test"
  exit 0
fi
echo "using python: $py"

echo "== building the overlay helper =="
(cd "$root" && cargo build -q -p computer-use-mcp)
export COMPUTER_USE_OVERLAY_BIN="$root/target/debug/computer-use-mcp"

echo "== building live test =="
test_bin="$(cd "$root" && cargo test -p computer-use --test live_linux --no-run --message-format=json 2>/dev/null \
  | "$py" -c 'import sys,json
for line in sys.stdin:
    try: m=json.loads(line)
    except Exception: continue
    if m.get("profile",{}).get("test") and m.get("executable") and "live_linux" in m["executable"]:
        print(m["executable"])' | tail -n1)"
echo "test binary: $test_bin"
[ -x "$test_bin" ] || { echo "could not locate the test binary"; exit 1; }

echo "== starting Xvfb on $display =="
Xvfb "$display" -screen 0 1280x1024x24 -nolisten tcp >/tmp/xvfb.log 2>&1 &
xvfb_pid=$!
cleanup() { kill "$xvfb_pid" 2>/dev/null || true; }
trap cleanup EXIT
sleep 1

export DISPLAY="$display"
export NO_AT_BRIDGE=0
export GTK_A11Y=atspi
export ACCESSIBILITY_ENABLED=1
export GNOME_ACCESSIBILITY=1
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp/cu-xdg}"
mkdir -p "$XDG_RUNTIME_DIR"

echo "== launching session (dbus + at-spi + fixture + test) =="
dbus-run-session -- bash -euc '
  launcher=""
  for c in /usr/libexec/at-spi-bus-launcher /usr/lib/at-spi2-core/at-spi-bus-launcher /usr/libexec/at-spi2-registryd; do
    [ -x "$c" ] && launcher="$c" && break
  done
  [ -n "$launcher" ] && "$launcher" --launch-immediately >/tmp/atspi.log 2>&1 &
  # A registry daemon so apps can register.
  for r in /usr/libexec/at-spi2-registryd /usr/lib/at-spi2-core/at-spi2-registryd; do
    [ -x "$r" ] && { "$r" >/tmp/atspi-reg.log 2>&1 & break; }
  done
  sleep 1
  # Optionally a window manager (CU_WM=openbox) to exercise the EWMH paths.
  if [ -n "${CU_WM:-}" ] && command -v "$CU_WM" >/dev/null 2>&1; then
    "$CU_WM" >/tmp/wm.log 2>&1 &
    sleep 1
  fi

  '"$py"' '"$fixture"' >/tmp/gtkapp.log 2>&1 &
  app_pid=$!
  sleep 2

  echo "== running test =="
  COMPUTER_USE_LIVE=1 "'"$test_bin"'" --nocapture --test-threads=1 ${CU_TEST_FILTER:-}
  status=$?
  kill "$app_pid" 2>/dev/null || true
  exit $status
'
