#!/usr/bin/env bash
# Build a ready-to-install Claude Code bundle for THIS machine's platform:
#   dist/computer-use-mcp-claude-code-<os>-<arch>.tar.gz   (zip on Windows)
# Usage: scripts/package-claude-code.sh [target-triple]
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

target="${1:-}"
tflag=(); rel=target/release
if [ -n "$target" ]; then tflag=(--target "$target"); rel="target/$target/release"; fi

cargo build --release -p computer-use-mcp --features http "${tflag[@]}"

os="$(uname -s | tr 'A-Z' 'a-z')"; arch="$(uname -m)"
case "$target" in
  *windows*) os=windows; exe=computer-use-mcp.exe ;;
  *apple*) os=macos; exe=computer-use-mcp ;;
  *linux*) os=linux; exe=computer-use-mcp ;;
  *) case "$os" in mingw*|msys*|cygwin*) os=windows; exe=computer-use-mcp.exe ;; darwin) os=macos; exe=computer-use-mcp ;; *) exe=computer-use-mcp ;; esac ;;
esac
case "$target" in aarch64*) arch=arm64 ;; x86_64*) arch=x64 ;; esac
[ "$arch" = x86_64 ] && arch=x64
[ "$arch" = aarch64 ] && arch=arm64

name="computer-use-mcp-claude-code-$os-$arch"
out="dist/$name"
rm -rf "$out"; mkdir -p "$out/docs"
cp "$rel/$exe" "$out/"
cp scripts/claude-code/install.sh scripts/claude-code/install.ps1 scripts/claude-code/install.cmd scripts/claude-code/README.txt "$out/"
cp -r examples "$out/"
cp docs/CONNECT.md "$out/docs/"
cp README.md LICENSE "$out/"
# plugin-files.sh also copies the skills/ folder (computer-use, computer-use-security).
bash scripts/claude-code/plugin-files.sh "$out" "$exe"
chmod +x "$out/$exe" "$out/install.sh" 2>/dev/null || true

( cd dist
  if [ "$os" = windows ]; then
    if command -v 7z >/dev/null; then 7z a "$name.zip" "$name" >/dev/null; else zip -qr "$name.zip" "$name"; fi
    echo "dist/$name.zip"
  else
    tar czf "$name.tar.gz" "$name"; echo "dist/$name.tar.gz"
  fi )
