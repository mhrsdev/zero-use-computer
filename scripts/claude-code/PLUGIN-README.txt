computer-use plugin (no program inside — on purpose)
====================================================

This zip is small (about 50 KB) because it has NO program in it. If you upload it
alone the plugin loads but has nothing to run. Use the full computer-use-mcp-<os>.zip
unless the app refused that one.

This archive is only the Claude plugin: the manifest, the MCP entry and the
skills (the agent guide, its safety rules and the design guide). It contains no
executable, because some hosts refuse executables in uploaded plugins. It runs the
program from:

    Windows       %USERPROFILE%\.computer-use\bin\computer-use-mcp.exe
    macOS/Linux   ~/.computer-use/bin/computer-use-mcp

Put the program there first: download computer-use-mcp-<your-os>.zip from the
same release, extract it, and run   install.cmd -NoRegister   (Windows) or
./install.sh --no-register   (macOS / Linux). Then upload THIS zip as the plugin.
