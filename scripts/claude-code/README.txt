computer-use-mcp for Claude Code
================================

1. Put this folder somewhere permanent (the path is stored in Claude Code's
   settings), e.g. ~/tools/computer-use-mcp  or  C:\Tools\computer-use-mcp.
2. Register it:
     macOS / Linux :  ./install.sh
     Windows       :  double-click install.cmd   (or: powershell -ExecutionPolicy Bypass -File .\install.ps1)
   Options: --approval allow-all (Windows: -Approval allow-all), --scope project|user|local, --uninstall
3. Check:  claude mcp list     (inside Claude Code: /mcp)
4. macOS: give Accessibility + Screen Recording to the app that runs Claude Code.
   Linux: needs an X11/XWayland session with the AT-SPI accessibility bus.

As a plugin: this folder is also a valid Claude plugin (.claude-plugin/plugin.json + .mcp.json).
   If "Upload failed" appears and a computer-use plugin is already installed, remove the old one first.
   Only if the app refuses the program inside, use the small computer-use-plugin-<os>.zip from the release
   (no program inside) after running  install.cmd -NoRegister  /  ./install.sh --no-register.
   Claude Code:  claude --plugin-dir <this folder>
   Apps with "Upload local plugin": upload the original .zip.

Manual alternative (no script):
   claude mcp add --scope user computer-use -- /full/path/to/computer-use-mcp serve

More: docs/CONNECT.md, README.md. Health check any time: computer-use-mcp doctor
