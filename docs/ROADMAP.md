# Roadmap: v3.5 to v4.0

The goal: fewer tokens spent on **finishing** a task, with the same accuracy
and stability. A smaller tool result is not a win on its own; every change
is measured with the real model on [the benchmark](../bench/README.md).
v3.5, v3.6, … are steps; v4.0 is the stable release of what they proved.

## Rules for every step

- **What counts**: real tokens for a successful task — input, output, cache
  reads and cache writes apart — plus cost, time, turns, tool calls and
  pictures. Estimates are reported apart from real numbers.
- **Behind a setting first**: a change to what the tools return is added
  off, measured both ways (A/B), and turned on by default only if success
  doesn't drop.
- **Every step is compared** with the step before and with the v3.2
  baseline, and its regressions are published with its gains.
- **The decision model (Jev and the like) is optional.** Nothing depends
  on it; a feature that uses it is an extra on top of one that doesn't.
- **Safety doesn't move**: finding a tool never permits running it; the
  masking and the stop key stay as they are.
- A simulation of Codex's behaviour is called a simulation.

| step | focus | status |
|---|---|---|
| v3.5 | measuring for real; the screenshot path; areas the tree says nothing about | done |
| v3.6 | everything planned for v3.6 to v4.0, in one release | done, but for the real-model runs and hardware tests below |
| v3.7 | the tool manager and every token-saving setting on by default; instructions, skills and results said once; work done twice removed; releases measured against each other | done (scripted); real-model runs left, as below |
| v3.7.5 | debugging: every part reviewed and run against a real app; 32 bugs fixed, each with a test | done |
| v3.8 | the decision model built in: a decision layer that asks it where it saves a turn (expect, about, find_tools), answers the same question once, sends many together, and never holds the agent up; nothing changes without one | done (tested with a stand-in model); real-model runs left |
| v3.8.1 | the design board: only the part of the picture that changed, shapes worked out once, look-alike layers as one record; 10 accuracy fixes (turns, bold, SVG, checks, steps); the on-screen cursor waited for before each action | done: −66% tokens and −36% time on the design benchmark |
| v3.8.2 | stable across systems: the fixes from a review of each system and the core (layouts, Store and Electron apps, administrator apps, Wayland without the usual tools, headless sessions, broken settings files), no hangs | done (checked by CI on Windows and macOS, not on the hardware) |
| v3.8.3 | faster only: 38% fewer accessibility calls for a window read again (Linux), "nothing changed" sooner for apps that show changes at once, OCR's enlargement in milliseconds | done: −15% server time on the benchmark, the same results |
| v3.8.5 | the protocol and the code: one MCP core for stdio and HTTP; batches, progress, per-tool annotations, results as data (off); Streamable HTTP (sessions, event streams, cancels that reach the call); engine.rs in parts, a call's state reset whole after a panic | done (tests over stdio and a socket; no real HTTP client) |
| v4.0.1 | the user's pointer travels only the last short stretch to a mouse action (not the whole way, which took it from them for up to 0.8 s); pointers drawn from a picture near their size, with a rim for light, grey and dark backgrounds, 17% bigger, their name tag under them | done (tests; the real pointer traced under Xvfb: 0.34 s away) |
| v5.0 (v5.0.1) | the settings panel (planned as v4.8): every setting on one page with help, previews and reset (Material-style, light and dark); profiles; the tool list with each tool's cost; the settings file as text, import and export; update controls (every 5 minutes or never, channel, pin, skip, a way back); one button, or `install`, to add the program to Claude Code, Claude Desktop, Codex, Cursor and VS Code; a guide inside; the pointer's trail, lean and breathing, and the real mouse's pace, overshoot and tremor, each adjustable; the agent refused the panel's window; a desktop shortcut that opens the panel; the hub sharing the screen by what each agent at work needs; Qt apps (KWin, Plasma) read without crashing them. [Plan](PANEL-SPEC.md) | done (tests; the page driven in headless Chromium; the Windows and macOS file locations of the agents not seen on those systems) |
| v4.0 | six new pointers for the agent (3D renders), each clicking its own way (crystal, paper, jelly, ice, metal, orbit), one at random per session and a different one per agent at once (`overlay.cursor_style`), or one per agent by name (`overlay.agent_cursors`); the pointer swings, leans, leaves a trail, breathes, shows keys, typing, drags and scrolls (`cursor_motion`, `show_keys`); the real mouse moves like a hand, with a hand's timing, in one of five ways (hand, sine, arc, spring, spiral; `natural_mouse`, `mouse_path`); updates download by themselves, checked against GitHub's SHA-256, and go in after the computer restarts (`[update]`); the overlay kept out of screenshots on X11 (the hide answer is waited for up to a second) | done (tests, live under Xvfb: four agents, four pointers, the real pointer traced along a curve; not seen on Windows, macOS or Wayland) |
| v3.9.7 | a debugging release: the whole project read twice line by line; ~150 problems fixed (the HTTP server could be stopped without the token, OCR text from private fields, values in the audit log, script sandbox escapes, clicks after a window moved) | done (tests for each fix that can run without a desktop; Windows and macOS by CI, not on desktops) |
| v3.9.6 | the stop key on a Mac named Control+Option+Esc everywhere ("Ctrl" was read as Cmd: Cmd+Option+Esc is Force Quit) | done (tests; not seen on a Mac) |
| v3.9.5 | Windows: the overlay and the stop key no longer go away during a session (the hub connection dropped every 3 quiet seconds since v3.9.0) | done (a new live test on Windows runners: v3.9.4 fails it, v3.9.5 passes; not yet seen on the reporter's Windows 11 desktop) |
| v3.9.4 | screenshots by need: left out when the tree says what changed, sent after x/y and an unmet expect; the model sets them per app; which apps needed pixels kept (`doctor`); a painted app no longer sends a whole picture per change | done (tests, a new painted-app task: −51% result tokens, 4 pictures → 1; not measured with a real model) |
| v3.9.3 | the six bugs v3.9.2 left: a minimized Store app's window (Windows), X11 window moves checked, a closed Wayland layer made again, each layer at its monitor's scale (Windows), one-character OCR words, first-time explanations kept | done (tests, clippy on three systems, the live X11 test, the benchmark; Windows and Wayland fixes not seen on real desktops) |
| v3.9.2 | debugging: five reviews and a fuzz test; security (keys out of scripts' reach), crashes, wrong results, Persian typing on X11 | done (tests, clippy on three systems, the benchmark; Windows and macOS fixes not seen on real desktops) |
| v3.9.1 | the Windows overlay stays above the taskbar ([#5](https://github.com/mhrsdev/zero-use-computer/issues/5)) | done (builds and clippy for Windows; not yet seen on a real Windows desktop) |
| v3.9.0 | several agents on one desktop: one hub for every server (numbered cursors, one stop key, the screen shared out, turns at the keyboard and mouse, optional messages), the `agents` tool | done (tests, Xvfb, two real servers; not with the real clients' subagents, nor on Windows and macOS) |
| v4.0 | stable: defaults set from real-model A/B runs | open |

The steps first planned as v3.7 (the tool surface), v3.8 (checking
results, fewer round trips) and v3.9 (optional layers) were built
together in v3.6.0; their sections below say what was done and what is
left.

## Left before v4.0

- **v3.7's defaults checked with a real model**: the tool manager
  (wrong calls and retries through `use_tool`; whether a model finds
  `design`, `draw` and `locate` when it needs them), lean schemas,
  relevant-change reports, adaptive pictures and blind areas. They were
  turned on from scripted runs, against the rule above, at the owner's
  request; each has a setting that turns it off.

- **Real-model runs** (5 or more per scenario, `bench/run.sh --runs 5`)
  of v3.2, v3.6 with its defaults, `bench/configs/lean.toml` and
  `manager.toml` (against v3.6: from v3.7 on, both files equal the
  defaults), with `--plan` left to the model; `--calibrate` to check
  the token estimate (Persian and other non-Latin text especially).
  Every number so far is a scripted estimate.
- **Defaults from those runs**: `ocr.blind_regions`, `tree.report`,
  `tree.quiet_volatile`, `screenshot.adaptive`, `screenshot.locate_picture`,
  `tools.descriptions`, `tools.design_steps`, `tools.manager` (and a
  compatibility table of clients for it: Claude Code, which has its own
  tool search, Codex, Cursor, VS Code, Claude Desktop).
- **Real Windows and macOS hardware**: the label relation (done on Linux
  only), the cell fallback and `expect` there; CI builds and unit-tests
  both, which is not the same.
- **Icon labels cached per app** (experimental): the icon strip is done;
  naming its icons needs a labeller that sees pictures, which the
  decision model (text only) is not.
- **The full benchmark on every model**, and v3.2 against v4.0 published,
  real numbers apart from estimates, gains and regressions alike.
- **Where v3.6 differs from the plan**: action reports at a lower
  `tree.report` level say "get_app_state shows them" instead of taking a
  `detail` argument; subtrees are judged with a yes/no question (`about`),
  not a score; an unreadable line gets no picture of its own (the blind
  area's picture comes with the look); wrong calls and retries caused by
  the lighter schemas are still to be counted, in the real-model runs.

## v3.5 — measuring, screenshots, blind areas

Done:

- `bench/`: real-model and scripted runs, six scenarios, success checked
  from the app; the v3.2 scripted baseline in `bench/results/v3.2`.
- Screenshots: region parameters are never ignored; `screenshot(app)`
  shares `get_app_state`'s dedupe and changed-part path; every screenshot
  is numbered and a changed part names the one it patches.
- Blind areas (`ocr.blind_regions`, off): found from the tree and checked
  in the pixels; read alone, at two sizes; watched on every look.

Left (now under "Left before v4.0"): the real-model runs, and
`ocr.blind_regions`'s default from them.

Found while building it (for the steps below):

- GTK 3 table cells: the accessibility action doesn't select the row; a
  mouse click does. *Done: a cell press that changed nothing is clicked.*
- GTK 3 keeps a filtered table's old cell text over AT-SPI while the
  picture shows the new rows. *Done: get_app_state says where the
  picture changed and the tree didn't.*
- Every table cell carries `actions=[expand_or_collapse, edit]`. *Done:
  a table's shared actions are said once.*
- Fields named only by a label come without a name. *Done on Linux (the
  labelled-by relation).*
- `locate` sends a picture of the whole window with every answer. *Done:
  `picture: false`, `screenshot.locate_picture`.*
- Whole-window OCR enlarges everything, which breaks big framed text.
  *Done: every reading at both sizes.*

## v3.6 — OCR and observation that suit the content (done in v3.6.0)

- **OCR on Linux**: line and grid removal, binarizing, cropping to the
  areas that need it, the page-segmentation mode per area; icons and rulers
  filtered out; uncertain lines marked as such; text read off the screen
  never taken for a real button; a picture when OCR can't read it.
- **Observation by scenario**, from simple rules (no extra model): forms
  and settings from accessibility; tables and lists by search, pages and
  the part needed (`get_app_state(within=index)`, `offset` in
  `find_element`); canvases with text by OCR and the area's picture; 3D and
  graphics by a detailed picture; a more complete way when one isn't
  enough.
- **Tables**: column headers once and one line per row; one template for
  look-alike subtrees; no flags or actions the role already implies.
- **Reports in levels**: brief, relevant changes (around the target, new
  windows, errors, and how many others), full; `detail` asks for more.
  The default is relevant changes, measured against extra
  `get_app_state` calls.
- **Smaller reports**: a short header when nothing changed (about 60
  tokens down to 8); no echo of what the model just wrote; removed lines as
  ranges of indices; added lines grouped under their parent; volatile
  elements (clocks, spinners) summarised.
- **Pictures**: attach automatically by how the model works in an app (no
  x/y or screenshots asked for → fewer automatic pictures); measure the
  overview size A/B.

## v3.7 — the tool surface (done in v3.6.0)

- **Compact definitions**: lighter schemas for shapes, no `window` or
  `default` in schemas (still accepted), no repeats; watched by wrong calls
  and retries.
- **`instructions`** full, short or off; skills shortened and freed of
  overlap with them (today about 580 + 2,400 tokens); `decide` shown only
  when a decision model is set up.
- **Tool manager** (experimental, behind a setting): the base tools, a line
  per category and an always-present `find_tools`. Two ways, by client:
  `tools/list_changed` where clients support it, or a fixed list with the
  schema returned in the conversation and a `use_tool` dispatcher (cache
  friendly, any client). The active set stays fixed through a step;
  hidden is not forbidden (permission is still checked where it is today);
  a compatibility table for Claude Code (which has its own tool search),
  Codex, Cursor, VS Code and Claude Desktop.
- **Smaller models**: a fixed small profile with simpler descriptions and
  no dynamic discovery; measured per model.

Before v3.6 the definitions were about 4,650 tokens per request, 46% of
them the five design tools. In v3.6.0: compact 4,986 (it gained `expect`,
names, lines and `about`), lean 3,715, the small preset 1,049, and the
dispatch manager with lean definitions far less (see the benchmark
results).

## v3.8 — checking results, fewer round trips (done in v3.6.0)

- **Targeted checks**: an optional `expect` (a dialog opens, the value
  changes, an element appears); three outcomes (confirmed, not confirmed,
  uncertain); "sent" and "seen to take effect" said apart; nothing
  uncertain repeated.
- **`batch`**: stops on an unexpected change (a dialog, focus to another
  app), ends with one diff, takes a short string syntax; fewer round trips
  measured against more errors.
- **Fewer turns and output tokens**: click by name or role; `launch_app`
  returns the first state; `app` defaults to the last one.
- **References the model may have lost**: the server counts what it has
  sent; past a threshold, references count as stale and the base view is
  sent again; `rebase` asks for it.
- **`design` and `scene`**: diffs of layers and checks, a picture only when
  it changed, steps only when asked, short strings for shapes.

## Optional layers, first planned as v3.9 (done in v3.6.0, but for the defaults and hardware)

- Decision model (optional): extraction by `pick` (the small model picks
  the element, the server reads its value), relevance of subtrees by
  `score`.
- For embedders: older results marked superseded, so a host can drop them
  from its context.
- Experimental: a sprite of unlabelled icons; icon labels cached per app.
- Defaults set from the A/B results; real Windows and macOS hardware
  tested; the full benchmark on every model.

## v4.0 — stable

Final defaults, deprecated behaviour removed, a migration guide, and the
full benchmark published: v3.2 against v4.0, real numbers apart from
estimates, gains and regressions alike.
