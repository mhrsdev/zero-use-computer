# Control panel: specification for v4.8

Status: built in v4.8 (see the changelog). Where it differs from the plan
below:

- The update setting "install when idle" was left out: an update goes in
  only when a server starts, never while an agent may be working, and that
  is the safe way to avoid two versions of the program talking to one hub.
  The other update controls are as planned.
- `update.check_every_hours` stays (it is used when `check_every_mins` is
  0), rather than being replaced.
- The clients after the first five (Windsurf, Gemini CLI, Cline, Zed,
  Continue, OpenCode) are not added: their settings files weren't checked.
- Not yet on the panel: an editor for `overlay.agent_cursors` (a table,
  which the settings keys don't list; write it in the settings file) and a
  per-app picture setting (it is kept per session, not in a file).
- The Windows and macOS locations of each agent's file are from their
  documentation and the project's own, not seen on those systems.

All text in the panel, its help pages, this spec, the changelog entry and
the code comments is English only.

## Release plan

| version | what |
|---|---|
| v4.0.1 | GitHub release worker (separate work, not part of this spec) |
| v4.5 | debugging and stabilising what exists; no new features |
| v4.8 | everything below: panel, settings coverage, update controls, one-button client install, docs, new effect and mouse intensity settings |
| v4.8.x | debugging the panel itself |

## What exists today

- A settings page for the decision model (`decision/page.rs`,
  `decision/page.html`): loopback only, a random port, a 128-bit token in
  the path, `Host` check against DNS rebinding, same-origin JSON `POST`
  only, closes after 15 idle minutes, the saved API key is never sent back.
  Opened by `control.settings_hotkey` (Ctrl+Alt+J) or
  `computer-use-mcp settings`.
- `config::edit_file_many` edits the settings file in place and keeps its
  comments; `hot_reload` applies the change to running servers.
- `config::known_keys()` and `Config::validate()`.
- The hub port (`hub.port`, 47381) speaks the hub's own line protocol, not
  HTTP. It ends 3 seconds after the last agent leaves. The panel must not
  use it.

## 1. Server

- The panel grows out of `decision/page.rs` into its own module (`panel/`),
  so the work does not collide with changes to `config.rs` and the engine.
- `panel.port` (default 47382). If it is taken, a random port is used and
  the address is printed. The token is kept in a file only the user can
  read (like `hub-<port>.token`), so the address is the same every time.
- The listener opens only when the user asks for it (the settings key,
  `computer-use-mcp settings`, a button in the tray/CLI), and closes after
  `panel.idle_minutes` (default 15, range 1 to 240) without a request. No
  always-open port.
- `computer-use-mcp settings` works with no agent running.
- Ctrl+Alt+J and `computer-use-mcp settings` keep working as they do now,
  and open the whole panel.
- Hard limits that the panel cannot raise: request size, simultaneous
  connections, minimum update interval.

## 2. Keeping the agent out of the panel

The agent controls the desktop, so it could open the panel and change
`control.stop_hotkey`, `control.pause_on_user_input` or `privacy.*`.

- The panel window has a fixed title (`Zero panel`). The engine refuses
  actions on a window with that title.
- The security skill says the agent must never touch it.
- Sensitive keys (`control.*`, `privacy.*`, `update.*`, `server.http_*`,
  `audit.*`) ask for a confirmation dialog that shows the exact change.
- The panel's address is never put in a tool result.

## 3. Design

- Material Design 3: hand-written components, no CDN, no web fonts from the
  network (the page is served from the binary and must work offline).
  Components: top app bar, navigation rail (drawer on narrow screens),
  cards, switches, sliders with value fields, segmented buttons, chips,
  text fields, dialogs, snackbars.
- Themes: light, dark and follow the system, and an accent colour (a seed
  colour that produces the palette, as in Material's dynamic colour). The
  choice is saved in the settings file (`panel.theme`, `panel.accent`),
  not in browser storage, so a changed port does not lose it.
- Every setting shows: name, one-line help, default, a reset-to-default
  button, and a badge when it needs a restart.
- Search across all settings, and a filter for "changed from default".
- Keyboard operable, visible focus, labels for screen readers, respects
  `prefers-reduced-motion`.

## 4. Settings coverage

Rule: every key returned by `known_keys()` has an entry in the panel's
schema (key, type, default, range or choices, group, help text). A test
fails when a key is missing, so a setting added elsewhere cannot be
forgotten.

Groups: Pointer, Real mouse, Overlay, Screenshots, Accessibility tree,
Tools and tokens, Timing, Screen memory, OCR, Decision model and API,
Privacy, Control, Agents and hub, Notifications, Scripts, Verification,
Audit, Server and HTTP, Updates, Platform, Panel.

Also exposed (today fixed in code), each with a safe range:
hub `MAX_HOLD` and `LINGER`, the agent message rate, the panel idle time.

Safety limits (the stop key itself, request size, connection cap, the
minimum update interval) stay fixed.

### New settings

Today `overlay.cursor_motion` and `natural_mouse` are on/off only.

Pointer effects (v4.8):

- `overlay.trail` (on/off), `overlay.trail_strength` (0 to 100)
- `overlay.lean_strength` (0 to 100)
- `overlay.breathe` (on/off), `overlay.breathe_strength` (0 to 100)
- `cursor_motion` stays as the master switch.

Real mouse (v4.8):

- `mouse_speed` (0.5x to 2x)
- `mouse_overshoot` (0 to 100: how often it goes past and back)
- `mouse_jitter` (0 to 100: hand tremor)
- `natural_mouse` and `mouse_path` stay.

These need engine and overlay changes, so they go in last, after the other
work on those files has landed.

## 5. Updates

Today: `update.check_after_mins`, `update.check_every_hours` (whole hours,
one hour minimum), `update.install = restart | start | manual`.

Panel controls:

| control | values |
|---|---|
| Automatic updates | on / off. Off is "never update". |
| Check every | `update.check_every_mins`: 5, 10, 30, 60 minutes or custom (minimum 5). Replaces `check_every_hours`; the old key is still read. |
| First check after | `check_after_mins` |
| Install | on restart (today's default), at next start, manual, and new: when idle (no agent working, not while a tool call runs) |
| Channel | stable or pre-release |
| Pin a version | stay on one version |
| Skip a version | `update.skip_version` |
| Roll back | keep the previous version and offer a Roll back button |
| Status | last check, result, pending version, its release notes, a Check now button |

GitHub allows 60 unauthenticated API requests an hour per IP. Every 10
minutes is 6 an hour per computer (servers share the check). Conditional
requests (`ETag`, `If-None-Match`) keep the cost down, and the interval
backs off when the API says the limit is reached.

## 6. One-button install into agents

A "Connect" tab lists the clients it finds on this computer. For each:
status (not installed, installed, installed with a different path), and
Install, Reinstall, Remove and Copy config buttons.

Clients to cover first, using what `docs/CONNECT.md` already documents:

| client | how |
|---|---|
| Claude Code | `claude mcp add --scope user computer-use -- <path> serve` when the `claude` command exists |
| Claude Desktop | `claude_desktop_config.json` (`mcpServers`) |
| Codex | `codex mcp add`, or `~/.codex/config.toml` (`[mcp_servers.computer-use]`) |
| Cursor | `~/.cursor/mcp.json` (`mcpServers`) |
| VS Code | user `mcp.json` (the key is `servers`) |

Candidates to add after their config locations and formats are checked on
every OS (Windows, macOS, Linux): Windsurf, Gemini CLI, Cline, Zed,
Continue, OpenCode. None of these paths is confirmed yet.

Rules:

- Before writing, show the file and the exact change. A confirmation is
  required.
- Back up the file (`.bak`), write atomically, and touch only the
  `computer-use` entry; other servers stay as they are. Preserve the file's
  formatting where the format allows it (TOML through `toml_edit`).
- Use a path that stays valid across self-updates. Warn when the program
  runs from a temporary or Downloads folder.
- Also offer to install the skills (`skills/`) for clients that load them
  (Claude Code), and point to the MCP skills for the others.
- After installing, show what is left to do: restart the client, and on
  macOS grant Accessibility and Screen Recording to the app that starts
  the server. A "Run doctor" button shows the same checks as
  `computer-use-mcp doctor`.
- A command line twin, `computer-use-mcp install --client <name>` (and
  `--remove`, `--list`), so it can be tested without a desktop and used on
  servers.
- Only the panel can do this; the install endpoints sit behind the same
  checks as every write (token, `Host`, same-origin, JSON), and the agent
  has no tool for them.

## 7. Documentation and tutorial inside the panel

Short pages written by hand, served from the binary (`include_str!`):
getting started, each settings group, connecting each client, pointers and
mouse paths with live previews, multiple agents, updates, privacy and
safety, troubleshooting. The page for a setting is one click from the
setting. `docs/GUIDE.md` and `docs/CONNECT.md` get the matching sections.

## 8. More

- Profiles: Low tokens, Balanced, Best quality, Showcase, and saved custom
  profiles; import and export (never including the API key or tokens).
- A raw TOML editor with live validation.
- Live previews: the six pointers, mouse path shapes, border and glow
  colours, and a "show on screen" button that draws the real overlay for a
  few seconds.
- A token-cost hint next to settings that change it (`max_dimension`,
  `attach`, `manager`), from the benchmark's estimates.
- Tool list with a switch per tool and its token cost.
- A per-app page for `pictures` (apps.json).
- Audit log and server log viewers.
- Status header: version, update state, hub and agents connected, stop key.

## 9. Tests

- Schema covers every `known_keys()` key.
- No non-ASCII-script text in the panel's files (a check limited to the new
  panel and docs, not to OCR and typing tests that use other scripts on
  purpose).
- Endpoint tests: wrong host, wrong origin, wrong content type, missing
  token, oversized body, and a secret never echoed back.
- Install: temp-home tests for each client's file format, with an existing
  config that must not be damaged, repeat runs that change nothing, and
  remove.
- Update: interval floor, back-off, skip, pin, rollback.
- Fuzz the request parser and the TOML edits.

## 10. Order of work

1. Panel module, server, token file, theme shell, schema and its test,
   decision model tab moved over, agent protection.
2. All settings groups, profiles, search, raw editor, previews.
3. Update controls.
4. Connect tab and the `install` command.
5. Docs and tutorial pages.
6. New effect and mouse settings (touches the engine and overlay).
7. Changelog, README, GUIDE, CONNECT, ROADMAP.
