@echo off
rem Double-click to register computer-use-mcp with Claude Code.
rem Extra options are passed on, e.g.  install.cmd -Scope project
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
echo.
pause
