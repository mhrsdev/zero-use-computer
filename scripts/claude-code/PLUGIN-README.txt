computer-use plugin (no program inside)
=======================================

This archive is only the Claude plugin: the manifest, the MCP entry and the
agent guide. It contains no executable, because some hosts refuse executables
in uploaded plugins. It runs the program from:

    Windows       %USERPROFILE%\.computer-use\bin\computer-use-mcp.exe
    macOS/Linux   ~/.computer-use/bin/computer-use-mcp

Put the program there first: download computer-use-mcp-<your-os>.zip from the
same release, extract it, and run   install.cmd -NoRegister   (Windows) or
./install.sh --no-register   (macOS / Linux). Then upload THIS zip as the plugin.
