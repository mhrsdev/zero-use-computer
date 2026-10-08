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
#   launcher  no binary either; Node runs launcher/main.cjs, which installs
#             the program from the GitHub release (one folder for every OS:
#             the plugin/ folder the Claude directory lists)
set -euo pipefail
dir="${1:?folder}"; exe="${2:?binary file name}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
version="${3:-$(grep -m1 '^version' "$root/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')}"
mode="${4:-bundled}"

mkdir -p "$dir/.claude-plugin" "$dir/skills"
# The release zips keep the name they always had; the directory's plugin
# (launcher mode) goes by the project's name, which no other plugin or
# Claude's own computer use has, and links the pages the directory shows.
repo_url=https://github.com/mhrsdev/zero-use-computer
if [ "$mode" = launcher ]; then
  name=zero-use-computer
  server=zero-use-computer
  links=",
  \"documentationUrl\": \"$repo_url/blob/main/plugin/README.md\",
  \"supportUrl\": \"$repo_url/issues\",
  \"privacyPolicyUrl\": \"$repo_url/blob/main/PRIVACY.md\",
  \"termsOfServiceUrl\": \"$repo_url/blob/main/LICENSE\""
else
  name=computer-use
  server=computer-use
  links=""
fi
cat > "$dir/.claude-plugin/plugin.json" <<JSON
{
  "name": "$name",
  "version": "$version",
  "description": "Control desktop apps (Windows, macOS, Linux) through their accessibility tree plus screenshots: an MCP server with an on-screen indicator, an emergency stop key, and skills for using it and for staying safe.",
  "author": { "name": "mhrsdev" },
  "homepage": "$repo_url",
  "repository": "$repo_url",
  "license": "Apache-2.0",
  "keywords": ["mcp", "computer-use", "desktop", "accessibility", "automation"]$links
}
JSON
case "$mode" in
  launcher) ;;
  bundled) command="\${CLAUDE_PLUGIN_ROOT}/$exe" ;;
  installed)
    case "$exe" in
      *.exe) command="\${USERPROFILE}/.computer-use/bin/$exe" ;;
      *) command="\${HOME}/.computer-use/bin/$exe" ;;
    esac ;;
  *) echo "unknown mode: $mode" >&2; exit 2 ;;
esac
if [ "$mode" = launcher ]; then
  command=node
  args='"${CLAUDE_PLUGIN_ROOT}/launcher/main.cjs", "serve", "--instructions", "short"'
else
  args='"serve", "--instructions", "short"'
fi
cat > "$dir/.mcp.json" <<JSON
{
  "mcpServers": {
    "$server": {
      "command": "$command",
      "args": [$args]
    }
  }
}
JSON
# The layered skills: every skills/<name>/ folder of the repo, as is.
cp -R "$root/skills/." "$dir/skills/"
# A root SKILL.md would be read as a second, conflicting manifest.
rm -f "$dir/SKILL.md"
