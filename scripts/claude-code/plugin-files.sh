#!/usr/bin/env bash
# Make a release folder a valid Claude plugin as well as a plain MCP bundle:
#   <dir>/.claude-plugin/plugin.json   manifest (what "Upload local plugin" looks for)
#   <dir>/.mcp.json                    starts the bundled binary via ${CLAUDE_PLUGIN_ROOT}
#                                      (short instructions: the plugin brings the skills)
#   <dir>/skills/computer-use/...      the agent guide (SKILL.md + reference/)
#   <dir>/skills/computer-use-security/... the safety rules (load first)
# Usage: plugin-files.sh <dir> <binary-file-name> [version] [bundled|installed]
#   bundled   (default) the binary sits in the plugin folder (${CLAUDE_PLUGIN_ROOT})
#   installed the plugin has no binary (some hosts refuse executables in plugin
#             archives); it runs the one install.cmd / install.sh copied to
#             ~/.computer-use/bin
#   path      no binary either; it runs `computer-use-mcp` from PATH, which
#             install.cmd / install.sh put there (one folder for every OS:
#             the plugin/ folder the Claude directory lists)
set -euo pipefail
dir="${1:?folder}"; exe="${2:?binary file name}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
version="${3:-$(grep -m1 '^version' "$root/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')}"
mode="${4:-bundled}"

mkdir -p "$dir/.claude-plugin" "$dir/skills"
cat > "$dir/.claude-plugin/plugin.json" <<JSON
{
  "name": "computer-use",
  "version": "$version",
  "description": "Control desktop apps (Windows, macOS, Linux) through their accessibility tree plus screenshots: an MCP server with an on-screen indicator, an emergency stop key, and skills for using it and for staying safe.",
  "author": { "name": "mhrsdev" },
  "homepage": "https://github.com/mhrsdev/zero-use-computer",
  "repository": "https://github.com/mhrsdev/zero-use-computer",
  "license": "Apache-2.0",
  "keywords": ["mcp", "computer-use", "desktop", "accessibility", "automation"]
}
JSON
case "$mode" in
  bundled) command="\${CLAUDE_PLUGIN_ROOT}/$exe" ;;
  installed)
    case "$exe" in
      *.exe) command="\${USERPROFILE}/.computer-use/bin/$exe" ;;
      *) command="\${HOME}/.computer-use/bin/$exe" ;;
    esac ;;
  path) command="${exe%.exe}" ;;
  *) echo "unknown mode: $mode" >&2; exit 2 ;;
esac
cat > "$dir/.mcp.json" <<JSON
{
  "mcpServers": {
    "computer-use": {
      "command": "$command",
      "args": ["serve", "--instructions", "short"]
    }
  }
}
JSON
# The layered skills: every skills/<name>/ folder of the repo, as is.
cp -R "$root/skills/." "$dir/skills/"
# A root SKILL.md would be read as a second, conflicting manifest.
rm -f "$dir/SKILL.md"
