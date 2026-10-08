#!/usr/bin/env bash
# Rebuild plugin/: the Claude plugin the Claude directory installs from this
# repository (Plugin path: plugin). The repository root can't be the plugin:
# the promo films are larger than the directory allows (5 MiB a file).
# It carries no program: it runs `computer-use-mcp` from PATH, where
# install.cmd / install.sh put it. CI fails if plugin/ is not what this makes,
# so run it after changing skills/ or the version.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
rm -rf "$root/plugin/.claude-plugin" "$root/plugin/.mcp.json" "$root/plugin/skills"
bash "$root/scripts/claude-code/plugin-files.sh" "$root/plugin" computer-use-mcp "" path
