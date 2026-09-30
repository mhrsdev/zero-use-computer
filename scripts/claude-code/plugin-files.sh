#!/usr/bin/env bash
# Make a release folder a valid Claude plugin as well as a plain MCP bundle:
#   <dir>/.claude-plugin/plugin.json   manifest (what "Upload local plugin" looks for)
#   <dir>/.mcp.json                    starts the bundled binary via ${CLAUDE_PLUGIN_ROOT}
#   <dir>/skills/computer-use/SKILL.md the agent guide
# Usage: plugin-files.sh <dir> <binary-file-name> [version]
set -euo pipefail
dir="${1:?folder}"; exe="${2:?binary file name}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
version="${3:-$(grep -m1 '^version' "$root/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')}"

mkdir -p "$dir/.claude-plugin" "$dir/skills/computer-use"
cat > "$dir/.claude-plugin/plugin.json" <<JSON
{
  "name": "computer-use",
  "version": "$version",
  "description": "Control desktop apps (Windows, macOS, Linux) through their accessibility tree plus screenshots: an MCP server with on-screen indicator, per-app approvals and built-in how-to skills.",
  "author": { "name": "mhrsdev" },
  "homepage": "https://github.com/mhrsdev/zero-use-computer-",
  "repository": "https://github.com/mhrsdev/zero-use-computer-",
  "license": "MIT OR Apache-2.0",
  "keywords": ["mcp", "computer-use", "desktop", "accessibility", "automation"]
}
JSON
cat > "$dir/.mcp.json" <<'JSON'
{
  "mcpServers": {
    "computer-use": {
      "command": "${CLAUDE_PLUGIN_ROOT}/EXE_NAME",
      "args": ["serve"]
    }
  }
}
JSON
sed -i.bak "s/EXE_NAME/$exe/" "$dir/.mcp.json" && rm -f "$dir/.mcp.json.bak"
cp "$root/skill/SKILL.md" "$dir/skills/computer-use/SKILL.md"
# A root SKILL.md would be read as a second, conflicting manifest.
rm -f "$dir/SKILL.md"
