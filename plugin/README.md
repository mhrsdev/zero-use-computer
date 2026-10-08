# computer-use plugin

The Claude plugin for [Zero Use Computer](https://github.com/mhrsdev/zero-use-computer):
the MCP server's entry, its launcher, and the skills (the agent guide, its
safety rules and the design guide). This is the folder the Claude directory
installs (Plugin path: `plugin`).

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
