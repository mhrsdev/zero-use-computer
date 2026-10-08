# Zero Use Computer

**Desktop control for Claude.** Claude reads each app the way a screen
reader does, acts on its real controls, and looks at pixels only when they
matter. Windows, macOS and Linux.

Instead of sending a screenshot every turn and guessing where to click, it
gives Claude each window's accessibility tree with numbered elements, so
"click 12" presses the real Save button through the operating system's own
accessibility API. Most actions work on background windows without moving
your mouse, and the whole loop uses far fewer tokens.

- Reads apps the way a screen reader does; reads canvases and painted text
  off the screen when the tree can't.
- Remembers screens: going back to one Claude has seen sends only what
  changed.
- Each action reports its own result, so Claude knows whether it worked.
- Several agents can share one desktop, each with its own numbered cursor.

**You stay in control:** an on-screen indicator while the agent works; an
emergency stop key (Ctrl+Alt+Esc; on a Mac, Control+Option+Esc); a pause
while you use the mouse; passwords and card numbers masked; keystrokes only
ever go to the app they are meant for. The bundled security skill sets which
apps the agent may touch.

**Setup:** needs Node.js 18 or newer. On first use the plugin downloads the
program for your system from this project's GitHub releases, checks its
SHA-256, and installs it to `~/.computer-use/bin`. On macOS, allow
Accessibility and Screen Recording for the app that runs Claude. Works in
Claude Code and Cowork; it controls your own computer, so it does not run on
claude.ai. [Privacy](https://github.com/mhrsdev/zero-use-computer/blob/main/PRIVACY.md)
· [Guide](https://github.com/mhrsdev/zero-use-computer/blob/main/docs/GUIDE.md)
· [Source](https://github.com/mhrsdev/zero-use-computer) (Apache-2.0).

This folder holds the plugin: the MCP server's entry, its launcher, and the
skills (the agent guide, its safety rules and the design guide). It is the
folder the Claude directory installs (Plugin path: `plugin`).

## How the server starts

The server is a native program, `computer-use-mcp` (written in Rust, one
build per system, published on this repository's
[releases](https://github.com/mhrsdev/zero-use-computer/releases)). The
plugin carries no program; `.mcp.json` starts `launcher/main.cjs` with Node,
which:

1. runs the program in `~/.computer-use/bin` (or `$COMPUTER_USE_HOME/bin`)
   when it is this plugin's version or newer, with Claude's stdin and stdout
   handed straight to it;
2. when it is older, still runs it, and installs this plugin's version in
   the background for the next start;
3. when it is missing, installs it: downloads `computer-use-mcp-<system>.zip`
   of the release named by this plugin's version (or the latest release, if
   that one is not out yet) from `github.com/mhrsdev/zero-use-computer`,
   checks it against the SHA-256 the release gives (`<zip>.sha256`, or
   GitHub's own digest of the file), takes the program out of it, runs it
   once (`--version`), and only then moves it into place. Meanwhile a
   stand-in answers Claude and offers one tool, `setup_status`; the desktop
   tools replace it as soon as the program is ready.

Nothing else is downloaded or run, and of the environment only the variables
below are read, each by name. The launcher is plain Node.js (18 or
newer) with no dependencies; its tests are in
`scripts/claude-code/launcher-test/`.

| Setting (environment) | |
|---|---|
| `COMPUTER_USE_MCP_BIN` | run this program instead; nothing is installed |
| `COMPUTER_USE_HOME` | the program's folder (default `~/.computer-use`) |
| `HTTPS_PROXY`, `NO_PROXY` | an `http://` or `https://` proxy for GitHub (one without a password: the launcher reads no credential) |

To install or update the program without Claude:
`node launcher/main.cjs --install`. Its log is `~/.computer-use/launcher.log`.

## Without Node.js, or without a connection to GitHub

Install the program by hand: download `computer-use-mcp-<system>.zip` from the
[latest release](https://github.com/mhrsdev/zero-use-computer/releases/latest),
unzip it, and run `install.cmd -NoRegister` (Windows) or
`./install.sh --no-register` (macOS, Linux). It copies the program to
`~/.computer-use/bin`, where the launcher finds it. The launcher itself still
needs Node.js; without it, register the program directly instead: run
`install.cmd` / `./install.sh` without the option.

`.claude-plugin/`, `.mcp.json` and `skills/` are made by
`scripts/claude-code/sync-plugin.sh`; edit `skills/` at the repository root,
not here.
