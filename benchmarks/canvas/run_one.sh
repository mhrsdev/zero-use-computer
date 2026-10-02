#!/usr/bin/env bash
# One benchmark run from a clean start: a new X server, D-Bus and AT-SPI bus,
# a fresh Dia profile with the starting diagram, a new computer-use-mcp
# process (empty screen memory) and a new `claude -p` session.
#
#   run_one.sh ARM OUTDIR
#
# ARM: baseline | current   the agent run, with configs/ARM.toml
#      oracle               no model: a scripted click on the target, Delete,
#                           Ctrl+S, to check the task can be done here at all
#      calibrate            the diagram with each box filled in its own colour,
#                           to locate the boxes on screen; no agent
set -euo pipefail

arm="$1"
out="$(mkdir -p "$2" && cd "$2" && pwd)"
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
bin="${CU_BIN:-$repo/target/release/computer-use-mcp}"
# shellcheck source=prompts.sh
source "$here/prompts.sh"

# A fixed, short working path: Dia shows it in the window title, so every
# run must use the same one; the D-Bus socket path must stay short.
work=/tmp/cubench
display=:91
rm -rf "$work"
mkdir -p "$work/home" "$work/cu" "$work/xdg"
chmod 700 "$work/xdg"

colors=()
[ "$arm" = calibrate ] && colors=(--colors)
python3 "$here/make_diagram.py" "$work/home/services.dia" "${colors[@]}"

pids=()
cleanup() {
  for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
  pkill -f "dbus-daemon --session --address=unix:path=$work" 2>/dev/null || true
  pkill -x dia 2>/dev/null || true
  pkill -f at-spi-bus-launcher 2>/dev/null || true
  pkill -f at-spi2-registryd 2>/dev/null || true
}
trap cleanup EXIT

Xvfb "$display" -screen 0 1280x1024x24 -nolisten tcp >"$out/xvfb.log" 2>&1 &
pids+=($!)
sleep 1
export DISPLAY="$display"
export NO_AT_BRIDGE=0 ACCESSIBILITY_ENABLED=1 GNOME_ACCESSIBILITY=1
export XDG_RUNTIME_DIR="$work/xdg"
export DBUS_SESSION_BUS_ADDRESS="unix:path=$work/xdg/bus"
dbus-daemon --session --address="$DBUS_SESSION_BUS_ADDRESS" --fork
/usr/libexec/at-spi-bus-launcher --launch-immediately >"$out/atspi.log" 2>&1 &
pids+=($!)
sleep 1
/usr/libexec/at-spi2-registryd >"$out/atspi-reg.log" 2>&1 &
pids+=($!)
sleep 1

(cd "$work/home" && HOME="$work/home" exec dia -n services.dia) >"$out/dia.log" 2>&1 &
pids+=($!)
win="$(timeout 30 xdotool search --sync --name '^services\.dia' | head -1)"
# The same size and place every run (no window manager in Xvfb).
xdotool windowmove "$win" 0 0 windowsize "$win" 1200 900
sleep 3
import -window root "$out/initial.png"
# The canvas content must be invisible to accessibility in every run.
timeout 60 /usr/bin/python3.12 "$here/atspi_dump.py" >"$out/atspi.json" 2>/dev/null || true

if [ "$arm" = calibrate ]; then
  python3 "$here/calibrate.py" "$out/initial.png" >"$out/boxes.json"
  exit 0
fi

python3 "$here/click_logger.py" "$out/clicks.jsonl" 2>"$out/click_logger.err" &
pids+=($!)
sleep 0.5
start="$(date +%s.%N)"
echo "$start" >"$out/start"

if [ "$arm" = oracle ]; then
  # Centre of the Cache box from the calibration, then Delete and Ctrl+S,
  # through the same server (default config).
  read -r cx cy < <(python3 -c "import json;b=json.load(open('$here/boxes.json'))['Cache'];print((b[0]+b[2])//2,(b[1]+b[3])//2)")
  COMPUTER_USE_HOME="$work/cu" python3 "$here/mcp_call.py" "$bin" "$here/configs/current.toml" \
    "get_app_state {\"app\":\"dia\"}" \
    "click {\"app\":\"dia\",\"x\":$cx,\"y\":$cy}" \
    "press_key {\"app\":\"dia\",\"key\":\"Delete\"}" \
    "press_key {\"app\":\"dia\",\"key\":\"ctrl+s\"}" >"$out/oracle.txt" 2>&1 || true
  sleep 2
else
  cat >"$out/mcp.json" <<EOF
{"mcpServers": {"cu": {"command": "$bin",
  "args": ["--config", "$here/configs/$arm.toml"],
  "env": {"COMPUTER_USE_HOME": "$work/cu"}}}}
EOF
  # The agent: the same model, prompt, tools and limits in both arms; only
  # the server's config differs. Session identity and output mirroring of
  # any parent Claude Code session are dropped so the run is self-contained.
  set +e
  env -u CLAUDECODE -u CLAUDE_CODE_SESSION_ID -u CLAUDE_CODE_REMOTE_SESSION_ID \
    -u CLAUDE_CODE_TEE_SDK_STDOUT -u CLAUDE_CODE_MESSAGING_SOCKET \
    -u CLAUDE_CODE_MESSAGING_TOKEN -u CLAUDE_CODE_SYNC_SESSION_REFS \
    -u CLAUDE_CODE_SYNC_SKILLS -u CLAUDE_CODE_DIAGNOSTICS_FILE -u CLAUDE_CODE_DEBUG \
    -u CLAUDE_CODE_REMOTE_SEND_KEEPALIVES -u CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD \
    -u CLAUDE_ADDITIONAL_DIRECTORIES -u CLAUDE_EFFORT \
    timeout "$RUN_TIMEOUT" claude -p "$TASK_PROMPT" \
      --model "$MODEL" --effort "$EFFORT" --max-turns "$MAX_TURNS" \
      --system-prompt "$SYSTEM_PROMPT" \
      --tools "" --allowedTools "mcp__cu" \
      --mcp-config "$out/mcp.json" --strict-mcp-config \
      --setting-sources "" --no-session-persistence \
      --session-id "$(python3 -c 'import uuid; print(uuid.uuid4())')" \
      --output-format stream-json --verbose \
    < /dev/null 2>"$out/claude.err" | python3 "$here/stamp.py" >"$out/stream.jsonl"
  echo "${PIPESTATUS[0]}" >"$out/claude.exit"
  set -e
  sleep 1
fi

end="$(date +%s.%N)"
echo "$end" >"$out/end"
import -window root "$out/final.png"
python3 "$here/verify.py" "$work/home/services.dia" "$start" >"$out/verify.json"
cp "$work/home/services.dia" "$out/saved.dia" 2>/dev/null || true
cp "$work/cu/audit.log" "$out/audit.log" 2>/dev/null || true
python3 -c "import json;d=json.load(open('$out/verify.json'));print('success' if d['success'] else 'FAIL', d['problems'])"
