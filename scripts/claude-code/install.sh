#!/usr/bin/env bash
# Register computer-use-mcp with Claude Code (macOS / Linux).
#
#   ./install.sh                       # user scope
#   ./install.sh --scope project       # only for the current project
#   ./install.sh --no-register         # only copy the program (for the plugin route)
#   ./install.sh --uninstall
#
# Run it from the folder that contains the `computer-use-mcp` binary.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source_bin="$here/computer-use-mcp"
no_register=0
uninstall=0
scope=user
name=computer-use
# Newer Claude Code keeps "computer-use" for itself ("this name is
# reserved"): then the program goes in under this name, as the panel's
# Connect page does.
alt_name=zero-use-computer
name_given=0

while [ $# -gt 0 ]; do
  case "$1" in
    --scope) scope="${2:?--scope needs local|project|user}"; shift 2 ;;
    --name) name="${2:?--name needs a value}"; name_given=1; shift 2 ;;
    --no-register) no_register=1; shift ;;
    --uninstall) uninstall=1; shift ;;
    -h|--help) sed -n '2,9p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

# After every option is read, so `--uninstall --scope project` removes the
# project one.
if [ "$uninstall" = 1 ]; then
  command -v claude >/dev/null || { echo "claude (Claude Code) is not on PATH" >&2; exit 1; }
  names=("$name")
  [ "$name_given" = 1 ] || names+=("$alt_name")
  removed=0
  for n in "${names[@]}"; do
    if claude mcp remove "$n" --scope "$scope" >/dev/null 2>&1; then
      echo "Removed '$n' ($scope scope)."
      removed=1
    fi
  done
  [ "$removed" = 1 ] && exit 0
  echo "Nothing removed: '$name' wasn't found in the $scope scope (see claude mcp list)." >&2
  exit 1
fi

[ -x "$source_bin" ] || { echo "computer-use-mcp not found next to this script ($source_bin)" >&2; exit 1; }

# Install the program in a stable place, so this folder can be moved or deleted
# and the plugin route (which runs it from there) finds it.
bin_dir="${COMPUTER_USE_HOME:-$HOME/.computer-use}/bin"
mkdir -p "$bin_dir"
bin="$bin_dir/computer-use-mcp"
# A new file moved into place, never the old one written over: macOS kills a
# signed program whose file changed under it ("Killed: 9"), and a running
# server keeps its old copy until it restarts.
tmp_bin="$bin.new.$$"
cp -f "$source_bin" "$tmp_bin" && chmod +x "$tmp_bin" && mv -f "$tmp_bin" "$bin" \
  || { rm -f "$tmp_bin"; echo "could not install $bin" >&2; exit 1; }
echo "Installed: $bin"
if [ -n "${COMPUTER_USE_HOME:-}" ] && [ "$bin_dir" != "$HOME/.computer-use/bin" ]; then
  echo "Note: COMPUTER_USE_HOME is set, so the program is in $bin_dir; the plugin zip looks in ~/.computer-use/bin, so register it with this script instead (without --no-register)."
fi
if [ "$no_register" = 1 ]; then
  echo "Done (not registered with Claude Code). Upload the plugin zip, or run this again without --no-register."
  exit 0
fi
command -v claude >/dev/null || { echo "claude (Claude Code) is not on PATH. Install it first: https://docs.claude.com/claude-code" >&2; exit 1; }

args=(serve)

echo "== doctor =="
"$bin" doctor || echo "(doctor reported problems; see docs/CONNECT.md)"

# Make the settings file easy to find (and edit) if it doesn't exist yet.
cfg="$("$bin" config path 2>/dev/null | head -n1 || true)"
if [ -n "$cfg" ]; then
  [ -e "$cfg" ] || "$bin" config init >/dev/null 2>&1 || true
  echo "Settings file: $cfg"
fi

claude mcp remove "$name" --scope "$scope" >/dev/null 2>&1 || true
status=0
added="$(claude mcp add --scope "$scope" "$name" -- "$bin" "${args[@]}" 2>&1)" || status=$?
if [ "$status" != 0 ] && [ "$name_given" = 0 ] && printf '%s' "$added" | grep -qi reserved; then
  echo "Claude Code keeps the name '$name' for itself: adding the program as '$alt_name'."
  name="$alt_name"
  claude mcp remove "$name" --scope "$scope" >/dev/null 2>&1 || true
  status=0
  added="$(claude mcp add --scope "$scope" "$name" -- "$bin" "${args[@]}" 2>&1)" || status=$?
fi
[ -n "$added" ] && printf '%s\n' "$added"
if [ "$status" != 0 ]; then
  echo "could not add '$name' to Claude Code (see above)" >&2
  exit "$status"
fi
echo
echo "Added '$name' ($scope scope). Check with:  claude mcp list"
case "$(uname -s)" in
  Darwin) echo "macOS: grant Accessibility and Screen Recording to the app that runs Claude Code (Terminal / your IDE), then restart it." ;;
esac
echo "Give the agent the skills in skills/ (computer-use, computer-use-security, and computer-use-design for design work): the server asks no one for permission, the security skill sets the rules."

