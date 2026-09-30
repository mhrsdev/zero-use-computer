#!/usr/bin/env bash
# Register computer-use-mcp with Claude Code (macOS / Linux).
#
#   ./install.sh                       # user scope, approvals asked per app
#   ./install.sh --approval allow-all  # every app not blocked by policy
#   ./install.sh --scope project       # only for the current project
#   ./install.sh --no-register         # only copy the program (for the plugin route)
#   ./install.sh --uninstall
#
# Run it from the folder that contains the `computer-use-mcp` binary.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source_bin="$here/computer-use-mcp"
no_register=0
scope=user
approval=""
name=computer-use

while [ $# -gt 0 ]; do
  case "$1" in
    --scope) scope="${2:?--scope needs local|project|user}"; shift 2 ;;
    --approval) approval="${2:?--approval needs prompt|allowlist|allow-all}"; shift 2 ;;
    --name) name="${2:?--name needs a value}"; shift 2 ;;
    --no-register) no_register=1; shift ;;
    --uninstall)
      command -v claude >/dev/null || { echo "claude (Claude Code) is not on PATH" >&2; exit 1; }
      claude mcp remove "$name" --scope "$scope" || true
      echo "Removed '$name'."; exit 0 ;;
    -h|--help) sed -n '2,10p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

[ -x "$source_bin" ] || { echo "computer-use-mcp not found next to this script ($source_bin)" >&2; exit 1; }

# Install the program in a stable place, so this folder can be moved or deleted
# and the plugin route (which runs it from there) finds it.
bin_dir="${COMPUTER_USE_HOME:-$HOME/.computer-use}/bin"
mkdir -p "$bin_dir"
bin="$bin_dir/computer-use-mcp"
cp -f "$source_bin" "$bin" && chmod +x "$bin"
echo "Installed: $bin"
if [ "$no_register" = 1 ]; then
  echo "Done (not registered with Claude Code). Upload the plugin zip, or run this again without --no-register."
  exit 0
fi
command -v claude >/dev/null || { echo "claude (Claude Code) is not on PATH. Install it first: https://docs.claude.com/claude-code" >&2; exit 1; }

args=()
[ -n "$approval" ] && args+=(--approval "$approval")
args+=(serve)

echo "== doctor =="
"$bin" doctor || echo "(doctor reported problems; see docs/CONNECT.md)"

# Make the settings file easy to find (and edit) if it doesn't exist yet.
cfg="$("$bin" config path 2>/dev/null | head -n1 || true)"
if [ -n "$cfg" ]; then
  [ -e "$cfg" ] || "$bin" config init >/dev/null 2>&1 || true
  echo "Settings file: $cfg"
fi

claude mcp remove "$name" --scope "$scope" >/dev/null 2>&1 || true
claude mcp add --scope "$scope" "$name" -- "$bin" "${args[@]}"
echo
echo "Added '$name' ($scope scope). Check with:  claude mcp list"
case "$(uname -s)" in
  Darwin) echo "macOS: grant Accessibility and Screen Recording to the app that runs Claude Code (Terminal / your IDE), then restart it." ;;
esac
