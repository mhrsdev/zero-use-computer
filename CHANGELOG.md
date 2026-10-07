# Changelog

## v4.0.1

- **The user's pointer followed the agent's** on Windows and Linux: where
  an action used the real mouse, v4.0.0 moved the user's pointer the whole
  way from where it was, up to 0.8 s, before bringing it back. An app sees
  the pointer only over its own window, so the way across the rest of the
  screen showed it nothing and only took the pointer from the user. Now
  the pointer goes at once to a short stretch (70–140 px) from the target
  and travels only that, in a hand's way, then goes back: under traced
  X11, 0.34 s away from where the user left it (it was 0.6–0.8 s).
  `natural_mouse = false` still makes it a jump there and back.
- **The pointers looked rough at their real size**: a picture shrunk about
  sixfold in one step lost its edges and its fine light to speckle (the
  crystal most), and some vanished on some backgrounds (ice on white,
  orbit on dark, chrome on grey). Each picture is now drawn from a halved
  copy near its size on screen, with a thin light edge and a soft shadow
  cut from its solid part, so it stays clear on light, grey and dark.

## v4.0.0

New pointers for the agent that move like a hand, a mouse that moves like
one too in five ways, updates that install themselves after a restart,
and the overlay kept out of screenshots on X11
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v3.9.7...v4.0.0)).

- **Six new pointers, each clicking its own way**: crystal, paper, jelly,
  ice, liquid metal and orbit, 3D renders cut out with their glow (about
  65 KB each, in `assets/cursors`). A click moves the picture itself:
  - jelly: it squashes flat and wobbles back;
  - ice: it runs down in drips, drops fall from its points, then it
    freezes back;
  - paper: the wing opens out from the fold and folds back;
  - metal: it gives a little and drops of chrome splash out;
  - crystal: light floods through it and breaks out in colours;
  - orbit: the pearl leaves its place and races once round the ring.

  Each session picks one at random; agents working at once each get a
  different one while there are pointers to go round. The state colour
  glows behind the pointer, and the name tag takes the pointer's own
  look (clear glass with a rainbow edge, satin paper, jelly, frost,
  chrome edged in gold, dark crystal); a click shows
  for 0.7 s (was 0.45). `overlay.cursor_style` names one (`"jelly"`, …),
  `"classic"` keeps the plain arrow, `"random"` is the default.
- **The pointer moves like a hand** (`overlay.cursor_motion`, on): it
  swings to where it acts in a gentle arc, leans into the move and leaves
  a trail of its own material (sparkles of colour, gold flecks, a gooey
  tail, frost, drops of chrome, a glowing streak); waiting, it breathes
  (the jelly wobbles, the paper sways, a glint runs over glass, ice and
  chrome). A drag draws its line along the way it went, in the pointer's
  material, and fades; a scroll shows arrows the way it goes.
- **Keys and typing by the pointer** (`overlay.show_keys`, on): keys
  pressed show as keycaps in the pointer's material that go down and come
  up ("Ctrl" "S"); typed text runs out a letter at a time in a bubble
  under the name tag, with a caret. Into a password field it shows dots.
- **A pointer per agent** (`overlay.agent_cursors`): `{ codex = "metal",
  "claude-code" = "jelly" }` gives an agent its pointer by its MCP
  client's name or its tag, whatever `cursor_style` says.
- **The real mouse moves like a hand** (`natural_mouse`, on): where an
  action falls back to the mouse (a click, a drag, a scroll, a hover, the
  way to a drawing's strokes), the pointer no longer jumps there or goes
  in a straight line at an even speed, which some apps notice. It goes
  along a gentle curve to one side, quick to start and slower to settle,
  with a slight tremor, a long reach sometimes a touch past and back
  (Fitts's law timing: about 0.3 s for a short reach, 0.6 s across the
  screen); the wheel turns a notch at a time. On X11, Wayland (wlroots) and Windows the
  real pointer moves so; on macOS the app gets the same path of events
  without the user's cursor moving. Afterwards the pointer still goes
  straight back where the user left it (`restore_pointer`). `natural_mouse
  = false` brings back the jump.
- **Five ways to move** (`mouse_path` for the real mouse,
  `overlay.cursor_path` for the agent's pointer; "mixed", the default,
  picks one at random for each move): a hand's curve, a sine wave (one or
  two swings across the way, calm at both ends), a circular arc, a spring
  (an underdamped step: 6–10% past the target and back, settling) and a
  spiral in to the target (a sixth to a third of a turn). Each takes a
  hand's time and ends exactly on the target. A drag goes straight
  whatever the setting, with a hand's timing and a faint tremor: it may be
  drawing a line in a paint program.
- **Updates** (`[update]`, on): five minutes after the server starts,
  then every 12 hours (shared by the servers on the computer), it asks
  GitHub for the latest release. A newer one is downloaded, checked
  against GitHub's SHA-256 for it (none, no update; only the repository's
  own release files, no pre-releases), unpacked into
  `~/.computer-use/updates/` and its program asked its version, then left
  waiting: nothing is replaced while an agent may be working. The first
  time the server starts after the computer restarts, the new version
  takes the program's place (and the package's skills and files, when it
  runs from an unpacked package or plugin) and the server goes on as it.
  `install = "start"` or `"manual"` changes when; `computer-use-mcp update`
  looks now, `update --install` puts it in now; `doctor` shows what waits.
- **The overlay could be in the agent's screenshots on X11** without a
  compositor: the engine waited 150 ms for the overlay to say it was
  hidden, and a helper busy drawing (a glide, a fade) answered later, so
  the picture was taken with the border in it, about one time in two in
  the live test. It now waits up to a second for that answer.

### Checked

- 522 tests in the library (new: every pointer keeps its tip on the spot
  through a click and while it breathes; agents at once get different
  pointers; every path style ends on the target in a hand's time without
  jumps, has its own shape, and a drag goes straight; the swing, lean and
  trail; keys, typing, scrolls and drags show and go; an agent's own
  pointer; versions, SHA-256 and CRC-32 against known values, only a newer
  checked release of the repository is taken, a zip unpacks and a bad or
  escaping one is refused, an update goes in as `install` says and
  replaces the program and the package), 55 in the server; clippy on Linux,
  Windows and macOS; Rust 1.88; the live Linux tests 10 of 10 with the
  natural mouse on, the overlay test 10 runs of 10 (5 of 10 before); four
  agents on one hub under Xvfb showed four different pointers. Under Xvfb
  the real pointer was traced through a click: 650 px along a curve up to
  54 px off the straight line in about 0.55 s, then back where it was;
  keycaps, the typed bubble and a drag's line were seen on the live
  overlay. Updates end to end: a build calling itself 3.9.6 downloaded
  the real v3.9.7 from GitHub (its SHA-256 matched), waited while the
  computer had not restarted, and with `install = "start"` put v3.9.7 and
  its package in place and went on as it; the look five minutes in found
  this version the latest. Not seen on a Windows or macOS desktop, nor on
  Wayland.

## v3.9.7

A debugging release: the whole project read twice, line by line, and what
was wrong fixed ([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v3.9.6...v3.9.7)).
The first read found about 100 problems, the second (of the fixed code)
about 50 more, some of them in the first read's fixes. Most are small; the
ones that matter:

### Security and privacy

- **Anyone who could reach the HTTP port could stop the server**, without
  the token: a request declaring a huge body made the HTTP library
  allocate that much and abort, and requests that never sent their body
  could hold every slot for good. The HTTP library (tiny_http 0.12) is now
  a patched copy in `vendor/` (each change in `vendor/tiny_http/PATCHED.md`):
  unread bodies are drained within limits, header lines are capped, and a
  request that stops arriving times out after 10 s. A body left unread is
  no longer read as the next request.
- **Text read off the screen inside a private field reached the model**
  (a verification code in a field whose value the app doesn't expose):
  the screenshot was blacked out, the tree still had it. OCR lines inside
  private areas are left out, also after the window moves.
- **The audit log kept values**: the first line of a `set_value` result
  holds the value set, passwords included. The log keeps only what comes
  before any quoted text or expectation.
- **Scripts**: a path going up (`..`) out of a folder not made yet got past
  the private-folder check (with `[script] files = "all"` a script could
  rewrite the server's settings); a script importing itself crashed the
  server; regular expressions, CSV and numbers could use up all memory or
  run past the time limit; a failed download deleted the file it was to
  replace; `elements()`, `colors()` and `page()` ignored tools switched off.
- A settings file that holds the HTTP token is now readable by its owner
  only, and `config show` / `config get` show only the token's end.
- Exports go to a folder only the user can open (`computer-use-exports-<uid>`
  on Linux and macOS), not a shared one.
- Windows: text a password manager marks as concealed is not read from
  the clipboard (as on macOS).

### Wrong results

- After `window` moved a window, a click by element index landed where
  the element used to be (perhaps in another app), and full-screen
  screenshots blacked out private areas at their old places.
- Text read off a window (OCR) was reused at its old place after the window
  moved; it now moves with the window.
- `decide pick`: a number in the model's answer was taken as a position
  (the 57th option, not element 57), and 65, 129, … candidates made the
  whole pick fail.
- The two OCR readings were merged badly when they split words differently
  (a word lost or doubled); "7 8 9" and "1 2 3 4 5" were taken for a ruler
  and dropped; a lone digit read unsurely was kept as text.
- macOS: text with a broken emoji (a lone half of a pair) crashed reading a
  window or the clipboard. Windows: such text was dropped.
- Windows: a console program launched with `launch_app` shared the
  server's (often hidden) console instead of getting its own window.
- Linux: windows of apps without accessibility could be confused (two
  terminals titled "~"); a missing modifier key (no Super on the layout)
  was silently left out of a shortcut; a hung session bus could switch
  accessibility off for good; minimize on window managers that keep
  minimized windows mapped was reported as refused.
- Wayland overlay: a cursor moved to another monitor went invisible.
- The hub: a working stop key could be reported as not working, and an
  agent could miss that its key was replaced; a stop could undo the user's
  "continue" a moment after it.
- `press_key "ctrl + s"` (with spaces) failed; a batch step written as
  `{"tool": "get_app_state", "app": "Safari"}` dropped the app; a zoom on the
  last column of a screenshot was refused; a notification's code was not
  masked when only its title said "PIN".
- The `long` bench scenario's rare failure: the test app filtered its table
  150 ms late, and the scripted run reused an old element number.
- Two servers writing `apps.json` at once lost counts; new apps were never
  kept once 200 were recorded.
- CI could publish a release with failing tests; it now waits for them.

### Not fixed

- On Hyprland with `force_zero_scaling` and a scaled monitor, element
  positions in X11 apps may be off (needs that desktop to fix safely).
- A client with the token that stops reading answers can still slow the
  other sessions (the HTTP library gives no write timeout).
- The hub doesn't prove to an agent that it is the real one (a protocol
  change).

### Checked

- 501 tests in the library, 55 in the server (each fix with a test where it
  can run without a desktop); clippy on Linux, Windows and macOS; Rust 1.88;
  the live Linux tests under Xvfb (10 of 10); the scripted `long` and `table`
  benchmark 40 of 40 runs. The Windows and macOS fixes were checked by
  reading and by CI (the Windows overlay live test), not on real desktops.

## v3.9.6

The stop key on a Mac, named so it is pressed
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v3.9.5...v3.9.6)).

- **On a Mac the stop key was called "Ctrl+Option+Esc"** (and "Ctrl+Alt+Esc"
  in the README and in `doctor`). Read on a Mac keyboard, "Ctrl" passes for
  Cmd, and Cmd+Option+Esc is the system's Force Quit window: pressed in an
  emergency, it opened that window and the agent went on. The key itself
  was always Control+Option+Esc (the two don't clash). Now the overlay,
  what the agent is told and `doctor` name it as the Mac keyboard does,
  **Control+Option+Esc**, and the README and the guide say it isn't
  Cmd+Option+Esc. Windows and Linux keep "Ctrl+Alt+Esc".
- `doctor` shows both keys as the keyboard names them (it printed the
  setting as written, "ctrl+alt+escape").

### Checked

- 429 tests in the library (a new one for the names on each system), 41
  in the server; clippy on Linux, Windows and macOS; Rust 1.88. Not seen on
  a Mac: CI only type-checks macOS.

## v3.9.5

An urgent fix for Windows: the overlay and the stop key went away during a
session ([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v3.9.4...v3.9.5)).

- **The overlay disappeared, and with it the emergency stop key**, on
  Windows, from v3.9.0 (when the hub came) to v3.9.4. Joining the hub, the
  server waits at most 3 s for its welcome and then clears that wait; but
  Windows keeps a socket's read timeout per handle, and it was cleared on
  another handle than the one that reads. So every 3 quiet seconds (a
  model thinking between two calls) read as "the hub went away": the
  server joined again, three times, then counted it as a failure, and
  after a few of those stopped trying. From then on the agent worked
  with no overlay and Ctrl+Alt+Esc did nothing ("the emergency stop key
  is not working" in the server's log). Now the wait is cleared on every
  handle, a read that times out is never taken for the end, and a hub
  connection that held for a minute starts the count of losses again.
  Linux and macOS share a socket's settings between handles and weren't
  affected.
- **Found and checked on Windows.** A new live test runs the server on a
  Windows runner against Notepad, with calls 4 s apart as a model's
  come, and lists the overlay's windows after each call. v3.9.4 lost the
  hub three times in 12 s and showed no overlay after the fifth call; this
  release kept the hub and showed it after every call, on Windows Server
  2025 (the Windows 11 24H2 core) and 2022. It is now part of CI.
- Not yet seen on the Windows 11 desktop where it was reported.

## v3.9.4

Screenshots by need, and the model's say over them
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v3.9.3...v3.9.4)).

- **A picture whose news the text already says is left out**
  (`screenshot.smart`, on). When all that changed on screen is where the
  tree reports a change (a number, a line read off the screen), the look
  says "Screenshot: not sent (the change is in the tree)". One is still
  sent after an action at x/y, after an `expect` that wasn't met, after 3
  left out in a row, and whenever asked (`screenshot: true`). A big element
  changing (the window's title, a document) doesn't count as saying what
  changed in it.
- **The model sets pictures per app**: `get_app_state` with `pictures:
  "always"`, `"never"` (only `screenshot: true` then) or `"auto"`, kept for
  that app until changed.
- **Which apps needed pixels is kept** (`screenshot.record_apps`, on): per
  app, looks, looks with little in the tree, text read off the screen,
  screenshots sent and left out, in `apps.json` in the server's folder.
  `doctor` lists the apps whose tree said least.
- **Fixed: a painted app got a whole new picture for every change.** In a
  window of a few elements (an app read only by OCR), one line changing was
  taken for a new screen, so the whole window was sent each time. Now one
  line replaced is a change on the same screen (a big change needs at least
  3 changed elements as well as a third of them).
- **Fixed: "Nothing on screen changed after it"** was said after an action
  whose change only the text read off the screen showed (settling doesn't
  read it again); the report that followed showed the change.
- `screenshot.attach = "always"` now always attaches one: `adaptive` held
  some back there too.

### Measured (scripted benchmark; token figures are estimates)

A new task, `counter`: an app with nothing for accessibility (a count and
three painted buttons, 600x380), pressing PLUS three times and looking
after each press, then DONE. Two runs each, the same figures:

| | pictures | result tokens | input tokens (8 requests) |
|---|---|---|---|
| v3.9.3 | 4 whole | 1,788 | 53,928 |
| v3.9.4, `smart = false` | 4 parts | 1,071 | 51,456 |
| v3.9.4 | 1 | 870 (−51%) | 50,640 (−6%) |

The input falls less because tool definitions and skills (≈4,500 tokens)
go with every request. A whole 1280-px window is ≈1,700 tokens, so in a
bigger painted app each picture left out saves more.

The six earlier tasks: the same result tokens (shapes 1 fewer); ≈35 more
input tokens a request (+0.5%), `get_app_state`'s `pictures` option.

**Not measured:** whether a real model does as well with fewer pictures
(that needs runs with a model, not done). `long` fails 1 run in 10 with
v3.9.3 and with v3.9.4 alike (GTK updates the filtered table late; the
script looks once more, then gives up).

### Checked

- 427 tests in the library (8 new: pictures left out, sent after x/y and
  an unmet `expect`, the model's setting, big elements, the record, a
  small window's change), 41 in the server, the fuzz test; clippy on
  Linux, Windows and macOS; Rust 1.88; the live X11 test.

## v3.9.3

The six bugs v3.9.2 found and left ([not fixed yet](#not-fixed-yet)),
fixed ([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v3.9.2...v3.9.3)).
Nothing is added; what the tools return for the benchmark's six tasks is
the same as v3.9.2's, to the token.

- **Windows: a minimized or suspended Store app keeps its window.** Its
  own window leaves the frame Windows draws around it, so the frame read
  as ApplicationFrameHost's and the app had no window to restore or
  focus. The server now remembers which app each frame held and keeps
  that while the app runs. A frame the server never saw with its app in
  it (the app was minimized before the server started) still reads as
  ApplicationFrameHost's.
- **X11: window moves, resizes, maximize, minimize, full screen and
  desktop moves are checked.** They were requests the window manager
  may refuse or ignore (a tiling one keeps its layout), and success was
  reported without a look. Now the server waits up to 1.5 s for the
  change and says so if it doesn't come. Focus and restore were already
  checked; close is not (the app may first ask about saving).
- **Wayland: a layer the compositor closed comes back.** The overlay
  dropped it and it stayed hidden until it next changed; now it is made
  again at once, where it was.
- **Windows: each overlay layer is drawn at its own monitor's scale.**
  All of them used the main monitor's, so on a 100% screen beside a 150%
  one the label and cursor came out too big or too small.
- **OCR keeps one-character lines** that are words on their own: a CJK
  or Hangul character, or a digit, when the reading is sure of it (80%,
  90% for a digit) and the box is the size of a letter. One Latin letter
  alone is still dropped (it is usually noise).
- **First-time explanations aren't used up by results the model never
  sees.** A look inside a batch (only its summary reaches the model) and
  a brief report used them, so the model never got them.

### Checked

- 419 tests in the library (2 new: one-character OCR words, a look
  inside a batch), 41 in the server and the fuzz test; clippy on Linux,
  Windows and macOS; Rust 1.88; the live X11 test (window moves, resize,
  tile, minimize and restore checked on a real X server without a window
  manager); the benchmark's six tasks, the same tokens as v3.9.2.
- **Not seen on real desktops:** the Windows fixes (a Store app, two
  monitors at different scales), the Wayland one (a compositor that
  closes a layer) and the X11 check under a window manager that refuses.
  They build and pass clippy, but CI can't show them.

## v3.9.2

A debugging release: five reviews, one for each part of the code (the
engine, drawing and design, scripts and the decision model, the
platforms, the hub and the server), a fuzz test of every tool and runs
on a real desktop found the bugs below; each is fixed, most with a test
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v3.9.1...v3.9.2)).
Nothing is added; what the tools return for the benchmark's tasks is
the same, to the token.

### Security

- **The decision model's key could be sent elsewhere.** The settings
  page kept the saved key when only the model's address changed, and
  `decide setup="open"` showed the agent the page's secret address, so
  a script could post a new address there and test it with the key.
  Now a new address needs its key typed again, and the address isn't
  shown.
- **Scripts could read the keys.** In the default `files = "read"` a
  script could read the settings file (the key, the HTTP token) or
  `/proc/self/environ`. Now scripts never read or write the server's
  folder (but their own `files/`), the settings file or `/proc`, under
  any setting.
- **A script could start a program it downloaded** (on Windows a `.exe`
  or `.bat` runs as it is): `launch_app` never starts one from the
  scripts' folder.
- A decision model turned off in `[tools]` is off for scripts too, and
  `decide_each` takes at most 500 things, like the tool.

### Crashes and hangs

- **A script could end the whole server**: a value nested thousands
  deep overflowed the stack when it was turned into data or printed (a
  stack overflow can't be caught). Refused past 128 levels.
- `regex_replace` and `regex_groups` refuse a result too big before
  building it (one match could ask for gigabytes).
- Axis ticks far from 0 looped forever; cells sized for a tiny target
  asked for billions; both ended the server.
- Linux: connecting to the accessibility or session bus gives up after
  5 s (a hung bus daemon hung the server).
- A script after a picture in a batch emptied the batch's pictures (a
  panic, the steps' report lost).

### Wrong results and wrong actions

- **`draw` never presses outside the window**: an element bigger than
  its window (a zoomed canvas) or a canvas size too small to map pressed
  and dragged over other apps.
- `set_value`: 2004 was taken for 2024 (a 1% tolerance); a field now
  holds exactly what was typed, sliders keep their tolerance.
- Linux `set_value` on a field or button that takes no value pressed or
  activated it (Enter on a read-only field) and said it worked.
- `select_text` (Linux, macOS) said "selected" when it couldn't read
  the text.
- Changes the model's own actions caused were hidden as "keeps changing
  on its own" (the page number after pressing Next three times).
- Going to a new screen told the host it could drop the last screen's
  look, which a return shows "as it was".
- A batch's `app` went to tools that refuse or misread it (design,
  scene, script, notifications, screenshot); a batch stopped on an
  `expect` its step's tool never checks.
- Designs painted with `draw` were stretched when their size wasn't
  whole; SVG rounded corners came out oval.
- OCR: 35 more language codes (an unknown one, like `sv`, stopped all
  OCR); capitals of every script compared as lowercase.

### Several agents and the overlay

- A turn the hub took back is enforced: the action stops instead of
  typing on into another agent's turn.
- A stop key the system refuses no longer drops the one that works (the
  desktop was left with none).
- `doctor` no longer joins the running hub (it split the agents'
  screen and moved their windows).
- The hub shares out the screen the last agent saw; a host's "working"
  ends after two minutes with no call; at most 32 connections wait to
  say hello, within 5 s; the engine never sends a message the hub drops.
- Windows: the overlay's layers go back on top in paint order (the
  cursor stays above the label).

### Typing and windows

- **X11: Persian and other characters off the keyboard** are typed with
  32 spare keys and 150 ms for a busy app to read each (30 ms lost or
  changed letters in Electron and office apps). Checked: a 70-character
  Persian sentence typed whole in a real GTK field.
- Windows: a window moved or resized lands where asked (7-11 px off and
  smaller each time before); with Caps Lock on, `press_key` of a letter
  gives that letter; a click that went in only in part, and an X11 drag
  that failed half way, let go of the button.
- X11: windows partly off the screen can be captured; Latin-1 titles
  are read right.

### Checked

- Every tool called with odd arguments built from its own schema
  (`tests/fuzz_tools.rs`, a new test): no panics.
- 417 tests in the library and the fuzz test, 41 in the server; clippy on Linux, Windows
  and macOS; Rust 1.88; the benchmark's six tasks, same tokens as
  v3.9.0.

### Not fixed yet

Found, but left for a later release (each is narrow, or needs a real
desktop to check): a minimized or suspended Store app on Windows loses
its window; X11 window moves and closes report success without checking;
on Wayland a layer the compositor closed stays hidden until it changes;
Windows draws every layer at one monitor's scale; one-character OCR
lines (a CJK label, a digit) are dropped; a few first-time explanations
are used up by results the model never sees.

## v3.9.1

A fix for Windows: the indicator no longer slips behind the taskbar
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v3.9.0...v3.9.1);
[issue #5](https://github.com/mhrsdev/zero-use-computer/issues/5)).

- **The overlay stays above the taskbar.** It put itself on top only when
  it was shown or moved, so once the user clicked the taskbar (or brought
  any other always-on-top window forward), Windows placed that window
  above the overlay, and part of the glow and label stayed behind it until
  the next move. Now the overlay goes back on top as soon as the
  foreground window changes, and at least once a second while it is
  shown. It needs no administrator rights.
- **Not covered:** the Start menu, the notification centre and other
  system surfaces sit in a band above every program's windows. Only an
  app with UI access (signed and installed under Program Files) can be
  drawn above them, which this release doesn't do.
- **Checked:** builds and passes clippy for Windows, as CI does. Not yet
  seen on a real Windows desktop: CI can't show which window is on top.
  It should be checked there before it is called fixed.

## v3.9.0

Several agents on one desktop: a client's subagents, or Claude Code
beside Codex, each with a numbered cursor, its own part of the screen and
turns at the keyboard and mouse, and one stop key for all of them
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v3.8.5...v3.9.0);
[how it works](docs/GUIDE.md#several-agents-on-one-desktop-v39);
[upgrading](docs/MIGRATING.md#upgrading-to-v390)). It includes
[v3.8.5](#v385), never released on its own: one MCP core for stdio and
HTTP, batches, progress, Streamable HTTP, per-tool annotations, and
`engine.rs` in parts.

![Four agents on one desktop](docs/images/hub-four-agents.png)

### One hub for every server on the desktop

- **Numbers in turn.** The first server to start runs a hub
  (`computer-use-mcp hub`) and is agent 1; every other server joins it, 2,
  3… in the order they come, whichever client started them. A number an
  agent left is given to the next. The hub goes a few seconds after the
  last agent.
- **A cursor each.** One overlay draws them all: the purple cursor tagged
  with its number once there are two or more ("Zero" when alone, as
  before), and each agent's glow and label ("2 · Zero is thinking…") in
  its own part of the screen.
- **One stop key for all.** Before, a second server's overlay couldn't
  even register the key (the system gives a key to one program), so
  Ctrl+Alt+Esc stopped only the first agent.
- **The screen shared out:** halves, thirds, a 2×2 grid. An agent may ask
  for a full, half, third or quarter screen; it gets it when it fits
  beside the others, else the hub shares the screen evenly and says so.
  The window an agent works with is moved into its part when it first
  looks at it (`hub.arrange`).
- **Turns at the keyboard and mouse.** An action waits while another agent
  types or clicks, so one's keys never land in another's field; reading
  never waits. A turn kept over 30 s ends by itself, and another agent's
  input is never taken for the user's (no pause for it).
- **Messages between agents** (`hub.chat`, off): short notes that come
  with the other agent's next result, marked as another agent's words,
  never instructions. The user switches them on and off on the settings
  page (Ctrl+Alt+J), which has a new "Several agents" switch, or with
  `computer-use-mcp config set hub.chat true`.
- **The `agents` tool** (in `find_tools`, category `agents`): `list`
  (numbers, clients, apps, parts of the screen), `area`, `send`, `read`,
  `wait`. The first result after the number of agents changes says so.
- **Safety:** the hub listens on this computer only (`hub.port`, 47381)
  and answers only those that show its token, a file in the server's
  folder that only this user can read; `doctor` says whether it runs.
  The security skill counts other agents' messages as data.

### Which clients

Checked in their documentation and source code (October 2026):

- **Codex** starts each subagent's servers anew: each subagent is an
  agent of its own, with nothing to set up.
- **Claude Code**'s subagents share the main agent's server unless their
  definition starts one of its own:
  [`examples/claude-code-agents/desktop-worker.md`](examples/claude-code-agents/desktop-worker.md).
- **Two clients side by side** (Claude Code and Codex): each is an agent.
- VS Code, Zed, Gemini CLI and OpenCode share one server between their
  subagents: one agent, whose calls take turns.

### Debugged before release

Three reviews (the hub, the engine's side, the MCP and HTTP side) and
stress runs found these; each is fixed, most with a test:

- **The stop key could be lost for the hub's life:** the first key asked
  for was kept even when the system refused it, so a later agent's
  working key was never tried, and a key changed in the settings never
  took. Now a key that failed, or another, is registered anew.
- **Two agents could type at once:** a turn held over 30 s (a long
  typing, a drawing, a wait for the user) was given to the next agent
  without a word. Now an agent says every few seconds that it still acts;
  only one gone quiet (stuck) loses the turn, and it is told. The pause
  while the user works comes before the turn, not inside it.
- **A hub that went away** (killed, or ended between agents) left its
  agents alone for 10 s or more, without a stop key, and counted against
  the overlay's five tries: now they join a new one at once, keeping
  their numbers (seen with two real servers: back within a second).
- An agent that was stopped stops the others when a new hub starts under
  them; a turn given up, or an action that sent nothing, no longer makes
  the others take the user's input for an agent's.
- A window is moved into its part only in a turn, and its real size is
  read back (a window may keep a minimum size); the first actions after
  joining wait for the list of agents; asking for the same part again
  doesn't wait 1.5 s; a change to `[hub]` in the settings joins (or
  leaves) at once; the hub is joined for its turns even with the
  indicator off; a message over the limit is refused instead of lost;
  results that bring messages are never marked as superseded.
- The token file is per port (`hub-<port>.token`) and a hub removes only
  its own; the hub reads at most 64 KB before the token; its log is
  appended to, not cut, by a second hub that lost the race.
- HTTP: each session keeps the protocol version it agreed on (a second
  client on an older one took `structuredContent` from the first); a
  request body is read on a thread of its own, so a slow upload holds no
  one up; sessions are dropped least recently used first, with their
  event streams; HTTP/1.0 gets 505 for a stream; a cancel for a request
  that had already ended no longer skips a later one with the same id.
- stdio: a client that goes away just as a call starts no longer gets it
  run.

### Checked

- The hub's bookkeeping on its own (numbers, layouts, turns, keeping and
  losing a turn, the message limit), a stop key registered again, agents
  over its socket (numbers, turns, messages, parts of the screen, a wrong
  token, the token file's mode), and eight agents coming, going and
  taking 144 turns, never two at once (14 new tests; 407 in the library,
  41 in the server, all passing three runs in a row).
- Two engines on one hub: told of each other, each window moved into its
  half, an action waiting for the other's turn, messages when allowed and
  refused when not.
- Seen under Xvfb with one, three and four agents, and with two real
  servers (one saying it is Claude Code, one Codex): the first started
  the hub, both were numbered and given their half, and the hub ended
  after them.
- Clippy on Linux, Windows and macOS; Rust 1.88.
- The hub killed under two real servers: both back on a new hub within a
  second, with their numbers.
- Cost: nothing per request. The new category's words were cut to the
  bone, and two categories whose names say it all lost theirs: 2,163 →
  2,160 tokens of tool definitions. What results say about the other
  agents is a short line, once per change, and none on `agents`' own.

### Not done

- Real Windows and macOS desktops, as before: the hub draws with the same
  code as the overlay there, but it hasn't been seen on them.
- Not tried with the real clients (Claude Code, Codex) driving subagents.
- An agent's own subagents that share its server (Claude Code without the
  worker definition, VS Code, Zed) are one agent, not one each.
- The parts of the screen are on the main display only.

## v3.8.5

The MCP side brought up to the protocol, and the code made easier to work
on. What the tools return is the same as v3.8.3, unless a client asks for
the new things (progress, results as data)
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v3.8.3...v3.8.5);
[upgrading](docs/MIGRATING.md#upgrading-to-v385)).

### The protocol

- **One core for stdio and HTTP.** Each transport had its own copy of
  `initialize`, `tools/list`, `tools/call` and the status method, and they
  had drifted (HTTP never told the client its tool list changed). Now both
  answer through `core.rs`.
- **Batches** (MCP 2025-03-26): a JSON-RPC array is answered with one
  array, in order, nothing for notifications; an empty one is an error.
  They used to be refused.
- **Progress:** a `tools/call` with `_meta.progressToken` gets
  `notifications/progress` while it runs: `batch` per step, `draw` by how
  far the pen has gone, `wait_for` by the time waited, `script` per tool
  it calls. Never backwards, at most every 100 ms, the last always.
- **A ping is answered at once,** even while a long call runs (it waited
  behind the call).
- **Annotations per tool.** They were one of two shapes: every tool that
  acts destructive and open-world, every other read-only. Now `scroll`
  and `select_text` aren't destructive, `set_value` and `set_clipboard`
  are idempotent, `launch_app` isn't destructive, `window` stays on the
  desktop, and `decide` reaches beyond it (it asks a model).
- **Results as data** (`server.structured_output`, off): `list_apps`,
  `find_element` and `get_clipboard` also return `structuredContent`,
  with an `outputSchema` in the tool list, for clients on MCP 2025-06-18.
  Off, as the roadmap asks of anything that changes what results hold:
  models read the text, and some clients would send both.

### HTTP: Streamable HTTP

- **A cancel reaches the running call.** Requests were answered one after
  another on one thread, so a cancel waited until the call it was meant
  to stop had ended. Now they are read on a thread of their own.
- **Sessions:** `initialize` is answered with an `Mcp-Session-Id`; an
  unknown one gets 404 (the client starts again), DELETE ends one. A
  client that sends none is served as before.
- **Event streams:** a call that asks for progress, from a client that
  takes `text/event-stream`, is answered with its progress and then its
  result; a GET opens a stream on which a changed tool list is announced.
  The events are written to the connection as they come (`tiny_http`
  held small ones back).
- **Checks:** `MCP-Protocol-Version` must be one the server speaks;
  bound to this machine, a `Host` naming another is refused (DNS
  rebinding, with the `Origin` check that was there); a refusal is a
  JSON-RPC error, and 401 says `WWW-Authenticate: Bearer`. A body that
  isn't JSON gets 400.
- Tested end to end over a socket: sessions, batches, refusals, progress
  events, a cancel from a second request, the GET stream.

### The code

- **`engine.rs` in parts.** It held 13,368 lines: every tool handler, the
  drawing helpers and 4,900 lines of tests. The handlers are now in
  `engine/` by what they do (`actions`, `looking`, `observe`, `drawing`,
  `boards`, `screenshot`, `finding`, `batch`, `system`, `expect`,
  `overlay`, `manager`, `progress`), next to `deciding` and `scripting`;
  the tests are in `engine/tests.rs`. `engine.rs` is 2,100 lines.
- **A call's state in one place.** What lives only while a call runs (its
  depth, a batch's quiet steps, the element it aims at, what its `expect`
  found, the pictures it took) is one `CallState`. A call that panics
  resets all of it. It used to miss three: after a batch step panicked,
  the tools a later script called reported no changes. A test with a key
  that panics fails on the old reset.

### Checked

All 394 library tests (5 new) and 40 server tests (14 new) pass, and the
live tests build; clippy is clean with and without `http`; Rust 1.88
builds it; it type-checks for Windows and macOS. Not checked: real MCP
clients over HTTP (Claude Code, Cursor), and real Windows and macOS
desktops, as before.

## v3.8.3

Faster, and nothing else: the tool results are the same text and the
same pictures as v3.8.2. The six benchmark tasks take 15% less time on the
server (13.9 → 11.9 s, scripted, median of 3 runs;
[measured](bench/results/v3.8.3-speed/compare.md)).

| | v3.8.2 | v3.8.3 |
|---|---:|---:|
| a click that changed nothing, in an app that shows changes at once | 736 ms | 363 ms |
| a look whose text is read off the picture (OCR) | 600 ms | 458 ms |
| a click in the long task (mean) | 342 ms | 203 ms |
| reading a 270-element window again (Linux) | 2148 calls, 117 ms | 1336 calls, 78–87 ms |
| the six tasks, server time | 13.9 s | 11.9 s |

- **Fewer questions to the app (Linux).** An element's role and
  interfaces don't change while it lives, so they are asked for once, not
  on every one of the reads that follow each action (only for apps that
  never give a gone element's name to another: GTK); an element that has
  no children isn't asked for them, and one without actions isn't asked
  for its actions. Each element's children are asked for as soon as its
  own answers are in. A window read again: 38% fewer calls.
- **"Nothing changed" sooner, where it is safe.** After an action, reads
  that still show the state from before were waited on for 500 ms, for
  apps (browsers, Electron) that show a change a little after making it.
  An app that has shown every change at once so far (3 or more, none
  late) now gets 200 ms; one change shown late and it gets 500 again, for
  good. `timing.adaptive_grace = false` keeps 500 for every app.
- **OCR:** the picture enlarged for the second reading was made with a
  general resize that took a third of the time; now a dedicated one gives
  the very same pixels (tested against it) in a few milliseconds.

Not faster: a window's first read (nothing is remembered about it yet),
and actions whose app shows a change late (the wait is the app's).

## v3.8.2

Stable across Windows, macOS and Linux, and predictable on setups that
aren't the usual one: other keyboard layouts and languages, Store and
Electron apps, apps run as administrator, Wayland desktops without the
usual tools, headless and remote sessions, broken settings files. Four
reviews (one per system, one of the shared core) looked for the places
where the server hung, did the wrong thing or failed without saying why;
each fix has a test where one can be written off the real desktop
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v3.8.1...v3.8.2)).

### Everywhere

- **A broken settings file no longer stops the server.** A typo, a value
  of the wrong type or a stop key written `Ctrl + Alt + Esc` made the
  client show only "server failed to start". Now it serves with the
  default settings and the first tool result says what is wrong; the
  fixed file is used as soon as it is saved. A file saved as UTF-16
  (Windows PowerShell 5.1's `>`) or with a byte-order mark is read, and
  `Ctrl + Alt + Esc` is a valid key.
- **The client going away stops the work.** When the client closes the
  connection, the call that is running stops (as the stop key would) and
  the ones waiting don't run, instead of acting on the desktop for nobody
  until the client kills the server mid-drag. Cancels always get through,
  however many messages are waiting.
- **Folders don't depend on where the server was started** (`/` for some
  hosts, the project for others): an empty `COMPUTER_USE_HOME` counts as
  unset, `~` is the home folder, and a relative `script.dir` or
  `audit.path` is in the server's folder.
- **Apps the agent opens are not closed with it:** on macOS and Linux
  they get a process group of their own, so Ctrl+C in the client's
  terminal no longer closes the user's editor or browser.
- **Text matching** ignores accents however they are written (`é` as one
  letter, or `e` and an accent mark, as macOS file names store it), and
  Korean written as its parts matches the syllables.
- **No more hangs:** card-number masking took seconds to minutes on a
  long line of digit groups; now it is linear.
- **Settings are checked:** timing values the stop key can't end
  (`timing.settle_ms = 600000`) and misspelt log levels are refused with
  the reason. The log level comes from `server.log` or
  `$COMPUTER_USE_LOG`, no longer `$RUST_LOG` (one exported for another
  program flooded the client's log).
- Saved scripts can't be named `find_tools` or `use_tool` (the tool list
  named them twice, which clients refuse for the whole session). Two
  settings saves at once no longer share a temporary file, and on
  Windows a save is retried while an antivirus holds the file. Tesseract
  opens no console window. HTTP accepts `bearer` in any case. A
  byte-order mark before the first message is skipped. The on-screen
  indicator draws Chinese, Japanese and Korean app names.
- **Installing over a running copy:** the install scripts move a new
  program into place instead of writing over the old one (macOS killed
  the signed program, "Killed: 9"; Windows refused to replace the running
  file).

### Windows

- **Keys on other layouts:** `press_key` of a plain character uses the
  user's layout (on AZERTY `1` typed `&`; on a Russian layout `a` typed
  `ф`), and a dead key (`^`, `` ` ``, `~`) is typed, never left waiting to
  combine with the next key. Shortcuts (`ctrl+s`, `ctrl+1`) work by key
  position as before.
- **Swapped mouse buttons** (left-handed setups): a click was a
  right-click; now a click is a click.
- **Store apps** (Calculator, Settings, Photos) are their own apps, not
  all "ApplicationFrameHost", so `launch_app("calc")` finds its window.
- **Non-English Windows:** a click used the mouse instead of the
  element's own action, because the action was named in the system's
  language ("Drücken"); Start Menu shortcuts are found by their
  translated names.
- **Apps run as administrator:** Windows drops input to them without an
  error; now the agent is told to run the server as administrator to
  control them.
- **Nothing waits forever:** a busy window (not yet "not responding") no
  longer blocks the capture; opening a link can't hang; a clipboard whose
  owner hangs is an error. `list_windows` asks the app nothing, so a hung
  app no longer slows it down or loses its windows.
- **Errors that say what happened:** a move, resize, maximize or minimize
  that didn't happen is an error (a window moved to a monitor of another
  scale is sized again); a capture that fails falls back to the screen
  or says why (a locked screen, a UAC prompt, a disconnected remote
  session); a window that closed is an error, not an empty tree.
- Console programs (`cmd`, `powershell`, `python`) open in a console of
  their own instead of exiting at once. Per-monitor scaling on Windows
  8.1 and 10 before 1703. `.EXE` in capitals. The indicator's fonts come
  from wherever Windows is installed and are drawn at the monitor's
  scale.

### macOS

- **Shortcuts on other layouts:** keys were sent as US key codes, so on
  AZERTY `cmd+a` was Cmd+Q (the app quit) and `cmd+z` was Cmd+W (the
  window closed); QWERTZ swapped `y` and `z`. Keys now come from the
  current layout. Text is typed with an ASCII layout for the moment a
  Japanese, Chinese or Korean input method is on, so it isn't converted.
- **Electron and Chromium apps** (Slack, VS Code, Discord, Teams) show
  their whole tree, not only the window frame.
- **The tree:** an element that answers "can't complete" at once no
  longer cuts the walk short; a window that closed is an error, not a
  fake empty node.
- **OCR in the user's languages:** Vision read English only unless
  `ocr.languages` was set; now the system's preferred languages it
  supports, and Vision's own reason when it fails.
- **`launch_app`** finds apps whose process is named otherwise ("Visual
  Studio Code" runs as "Code") at once, instead of waiting out the launch
  timeout; an app whose windows are all on another Space or in full
  screen is explained.
- Clicks name the target window (one behind another no longer gets the
  click in the wrong window). Several large displays no longer make a
  screenshot of hundreds of megabytes. Accessibility errors are in words,
  not numbers, and a short `macos.messaging_timeout_secs` no longer
  breaks the detection of a hung app. Notifications without the
  Accessibility permission, a clipboard holding only a picture, and a
  window that refused a resize are errors that say so; empty images no
  longer crash a call.

### Linux

- **Accessibility is switched on:** most desktops (KDE, XFCE, sway,
  Hyprland, often GNOME) start with it off, and Qt apps, Firefox and
  Chromium then show no tree. The server switches it on and starts apps
  it opens with it on; `doctor` says whether it is.
- **No accessibility bus** (ssh, a container, a headless test machine):
  the server used not to start. Now screenshots, input and windows work,
  element calls say why they can't, and the bus is looked for again
  later (also where `AT_SPI_BUS_ADDRESS` or the X root says).
- **Wayland on GNOME and KDE:** X11 apps' windows were captured black; a
  native app in front was reported as "Desktop". A leftover
  `WAYLAND_DISPLAY` in an X11 session no longer makes it act as Wayland.
- **No hangs on a remote or dead X server:** connecting gives up after 2
  seconds instead of about two minutes, again on every retry.
- **Clipboard:** a picture on it was returned as bytes and an empty one
  was an error; now text only, and empty is "".
- A window manager that quit is no longer taken for running; Flatpak
  apps' windows are matched by the real process id; GTK apps at 2× on
  X11 are clicked where they are; GNOME's own and Flatpak notifications
  are recorded; up to 16 characters not on the keyboard are typed
  without one being re-mapped while an app still reads it; coordinates
  out of range are refused instead of wrapping; XTest missing leaves
  screenshots working; the work area is the current desktop's.

### Not done

- Real Windows and macOS desktops: the fixes for them are checked by CI
  (build, clippy and unit tests on both) and by reading, not on the
  hardware.
- Linux: an X server that stops answering (not one that is gone) can
  still hold a screenshot or pointer query.
- Clients that negotiate MCP 2025-03-26 and send JSON-RPC batches still
  get an error.

## v3.8.1

The design board (`design`, the Canva-like tool) is cheaper, faster and
more accurate: two thirds fewer tokens over a typical session, and what
it shows, exports, checks and paints now agrees with what was asked. And
the on-screen cursor no longer lags behind the action or skips it
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v3.8.0...v3.8.1);
[measured](bench/results/v3.8.1-design/compare.md)).

| | v3.8.0 | v3.8.1 |
|---|---:|---:|
| a badge built and fixed (10 calls): tokens | 10,994 | **3,750** (−66%) |
| its time on the server | 141 ms | **90 ms** |
| a page of 150 layers (3 calls): tokens | 7,371 | **4,619** (−37%) |
| its time on the server | 93 ms | **38 ms** |

### Fewer tokens

- **Only the part of the picture that changed.** After the first
  picture, a change sends just the part whose pixels changed, with where
  it is ("x 640–760, y 440–560 (G5 to H6)"). Recolouring a star:
  130×125 px, 22 tokens, instead of the whole page's 1,049. A change over
  more than half the page, a look (only `name`), or other marks (`show`)
  still get all of it. `screenshot.scope = "full"` turns it off, as for
  screenshots.
- **Export shows nothing new:** `export` alone no longer sends the
  picture and the listing again.
- **The picture is remembered through a look:** zooming into a cell no
  longer makes the next change send the whole page.
- **Look-alike layers in a row are one record** in the listing (`3 ×
  ellipse w 16 h 16 fill #1D3557: d1 x 292 y 532 · d2 x 382 y 532 · d3
  x 492 y 532`), as `get_app_state` lists look-alike elements: 150 layers
  of a pattern in 1,216 tokens instead of 2,089.

### Faster

- **Each shape is worked out once.** Its lines were worked out again for
  every box, check, step and picture (about five times a call); now
  once, until it changes. A change on a page of 150 layers: 28 → 10 ms.

### More accurate

What the board shows, exports, checks and paints now agrees with what
was asked. Each of these has a test that failed before:

- **Turning a moved or mirrored shape:** `rotate` with `about` after a
  `move`, `align` or `distribute` turned about the wrong point (by the
  move), and a mirrored copy turned the wrong way.
- **Bold with no font** was measured and painted regular, while the SVG
  and the app make it bold: boxes, centring and checks were too narrow.
- **SVG export matches the board:**
  - open lines (an arc) are never filled;
  - text sits on the board's baseline, keeps its opacity, its outline
    and its spaces, and names a sans font when none is given;
  - lines are as wide as the board draws them.
- **A line added by `change`** is 2 wide, as on a new layer (it was 0:
  invisible in the SVG).
- **Text with `fill: "none"` and a line** is outlined, not painted black.
- **Checks:**
  - a layer wider than most of the page is still reported when it runs
    off it (only a layer covering the page edge to edge is meant to
    reach the edges, and only on that axis);
  - contrast is measured against what really shows behind the text,
    see-through layers and the text's own opacity included.
- **Curves given only `y` (or only `x`)** run across the page, not five
  pages: their boxes, `to`, `align`, `distribute` and the SVG were wrong.
- **Paint steps** give a see-through layer the colour it shows (a 20%
  black shadow is a light grey step, not black).
- **`distribute`** spaces layers up to the furthest end, so a wide one
  isn't pushed off the page.
- **A cell past the page** (`show: {"cell": …}`) ends at the page.
- **The listing tells rounded corners, turns and repeats**, so a change
  to them is reported instead of "as before".
- The skill no longer says a layer can be written as the listing shows
  it (a line takes the shape's own numbers).

### The cursor keeps up with the work

The on-screen indicator's cursor showed the action late, or not at all:

- **The action waited for nothing.** The engine told the indicator to
  glide the cursor there and acted at once, so the click landed before
  the cursor arrived (a 220 ms glide, plus the indicator's own delay).
  Now the indicator says when the cursor is shown at the point, and the
  action waits for that (at most `overlay.move_ms` and a little more; a
  cursor already there, or hidden, answers at once).
- **No cursor for an element without its own box** (accessibility trees
  have many): it now goes to the nearest box around the element, else to
  the window.
- **`type_text` and `press_key` without an element** now show the cursor
  at the focused element, where the keys go.
- **A drawing** is followed by the cursor as the pen moves, instead of the
  cursor waiting at the start and jumping to the end. A drag glides along
  with the drag.
- **A click glided twice** (the second glide restarted the first); now
  once.

When the cursor is shown, each action takes up to `overlay.move_ms` (220
ms) longer: a lower value is quicker, and `overlay.show_cursor = false`
waits for nothing.

### Measured

`examples/design_bench.rs` builds and fixes a badge and a page of 150
layers, and reports each call's tokens and time; run it on any release.

## v3.8.0

The decision model, built in so that the server never depends on one and
gains from one: a **decision layer** between the engine and the model
asks it where that saves the agent a turn or a read, answers the same
question once, sends many questions together, and never lets a slow or
failing model hold the agent up
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v3.7.5...v3.8.0);
[how it works](docs/GUIDE.md#optional-and-a-gain-when-its-there-v38)).
Without a decision model nothing changes.

### The server asks on its own, where it saves a turn

On by default once a model is set up (`decision.auto`); without one, or
with it off, the server judges these itself, as before.

- **`expect` in other words.** An expected text that isn't on screen
  word for word ("saved successfully", the app says "Saved") is
  confirmed by the model reading the window once, instead of "not
  seen", which costs the agent a look. Kinds (`dialog`, `change`,
  `value`…) and texts found as written are never asked about.
- **`get_app_state(about=…)`** sends all the parts of a window in one
  request, a question each, instead of one request per part. The words
  of `about` are matched first and stand wherever the model doesn't
  answer.
- **`find_tools(query=…)`**, when no word of the query is in a tool's
  name, asks the model which tool is meant ("pinpoint a tiny icon" →
  `locate`). A query that names a tool is answered as before, with no
  request.

### Never in the way

- **Time limit:** the server's own questions wait at most
  `decision.auto_timeout_ms` (3 s), then it judges by itself.
- **Rest after failures:** three failures in a row and the server stops
  asking on its own for a minute. The agent's own questions (`decide`,
  `wait_for` until, `pick`) always go, and their errors are shown.
- **Nothing at start:** no request, process or wait until a question is
  asked.

### Fewer requests, less cost

- **Answers kept:** the same question about the same state, for the same
  model, is answered from memory for `decision.cache_seconds` (5 min; 0
  turns it off). A `wait_for` that asks again, the same look, a pick
  repeated: no request.
- **One curl for many requests:** `items`, `decide_each` and the parts of
  a window go out through one `curl --parallel` instead of one process
  each. An HTTPS API that speaks HTTP/2 gets one connection. A plain-HTTP
  server (a model on this computer) still gets them at once. A curl
  before 7.66 gets one request each, as before. Measured here over local
  HTTP/1.1, as fast as before (32 requests: 364 ms against 332 ms). The
  gain, fewer TLS handshakes and fewer processes to start, is with a
  remote API and on Windows, which this test can't show.

### Counted

- `decide(setup="status")` says what the layer did this session:
  - requests, and how many the server sent on its own;
  - answers from the cache;
  - the average wait and failures;
  - roughly how much text the model read instead of the agent;
  - whether it is resting after failures.

### Settings

`[decision]` gains `auto` (true), `auto_timeout_ms` (3000) and
`cache_seconds` (300).

## v3.7.5

A debugging release: 32 bugs found by reviewing every part of the code
and running it against a real GTK app, each fixed with a test that
failed before
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v3.7.0...v3.7.5)).
Nothing is added or taken away; a few things that were unsafe now
refuse ([upgrading](docs/MIGRATING.md#upgrading-to-v375)).

### Security and privacy

- **The decision model's key stays with its model.**
  - `decide setup={"base_url": …}` from the chat sent the saved key to
    the new address. Now a new address or provider without a key in the
    same call forgets the old key.
  - `setup` from the chat no longer takes `api_key_env`. It could send
    any of the server's environment variables to any address. It is
    still a setting in the file.
  - On the settings page, switching provider without typing a key no
    longer sends the old provider's key to the new one.
- **Scripts:**
  - `import` takes the names of saved scripts only. A path (absolute,
    or with `..`) loaded any `.rhai` file on the disk, even with
    `[script] files = "none"`.
  - On Windows, `\Windows\x` and `C:x` count as "relative" but left
    the scripts' folder when joined to it, so a script could write
    anywhere on the drive. They now follow the rules for absolute paths.
- **Card numbers next to other numbers are masked.** A card followed
  or preceded by an expiry date, a CVV or a quantity ("4111 1111 1111
  1111 12/29") made a run too long for the check, and nothing was
  masked, in text or in screenshots.
- **Code words count only as whole words.** "Shipping in 2024" or
  "opinion … 1998" no longer mask a year as if it were a PIN.
- **X11:**
  - A screenshot of a minimized window, or of one on another
    workspace, is refused, as it already was on Wayland. It showed
    whatever was in its place, maybe another app.
  - **Wayland without X11:** when the compositor can't say which window
    is in front, keys are no longer typed into whatever has the focus.
    The target is brought forward first, or nothing is sent.

### Screen memory and reports

- **`batch` and `script` no longer use up what the model sees.**
  - Screens that steps inside them reached, but the model never saw,
    were later called "seen before" or "identical".
  - A screen the model had seen was replaced in memory by a newer view
    it never saw.
  - First-time explanations, such as a diff's intro, were spent on
    steps whose text never reaches the model.
- **The model's own typing is no longer "an element that keeps
  changing on its own."** With `tree.quiet_volatile` on, the default, a
  field typed into without an `element_index` was hidden from reports
  after two actions.
- **The "keep changing" note is no longer counted** as one more change
  elsewhere in a relevant-changes report.

### Server and command line

- **Over HTTP, `tools.manager = "list_changed"` works as `"dispatch"`.**
  HTTP can't tell a client that its tool list changed, so found tools
  were added to a list the client never asked for again.
- **`find_tools(name="click")`** (a base tool) no longer adds the
  scripts category and changes the tool list. That cost the client's
  prompt cache for nothing.
- **A request with `"id": null`** gets an error. Before, it was taken
  for a notification and silently dropped.
- **`computer-use-mcp tools` and `doctor`** show the tools the model is
  actually served: the tool manager's list, saved scripts included.
  Before, they showed every tool.
- **`--http-token`** wins over `$COMPUTER_USE_HTTP_TOKEN`, as an
  explicit flag should.
- **`config get script.dir`** (a setting with no value) prints nothing,
  instead of "unknown setting".
- **`config set server.http_token 123456789`** keeps it as text.
- **`config set` and `unset`** work on sections written as inline
  tables.
- **Lean and compact schemas:**
  - `design` and `scene` `change` items are typed like `add` items
    (objects, or lines). Clients that check arguments refused the
    documented line form.
  - `design` layers no longer list `axes`, which they refuse.

### Smaller fixes

- **Script functions:**
  - `regex_replace` refuses a result that would fill the memory before
    building it. Before, one call could end the server.
  - `random(lo, hi)` works on the widest range.
  - Cell names longer than four letters are an error, not an overflow.
- **Text and colours:**
  - `design` colours that aren't ASCII are an error, not a panic.
  - Matching folds Latin combining marks: "İptal" finds "iptal", and a
    decomposed "é" finds "e".
- **Screenshots:**
  - A zoom into an element partly outside the window no longer wraps
    around.
  - OCR in Traditional Chinese (`zh-TW`, `zh-HK`, `zh-Hant`) uses
    Tesseract's `chi_tra`.
- **Linux internals:**
  - X11 clicks, drags and scrolls round coordinates as moves and
    drawing do, instead of cutting them.
  - Finding the accessibility bus has a time limit, so a hung bus
    launcher can't hang the server.
  - The on-screen indicator hides for a capture even before its first
    frame, so it can't fade into that capture.

## v3.7.0

Fewer tokens on every request, less work done twice, and every
token-saving setting on by default. The tool list a model gets with
every request is less than half what it was, without taking any tool
away: the tools most tasks don't need are found when they are needed.
Upgrading: [docs/MIGRATING.md](docs/MIGRATING.md#upgrading-to-v37).

Measured (scripted, three runs each, every figure an estimate; the
releases run as they ship, over MCP, with their own skills:
[bench/results/v3.7-releases](bench/results/v3.7-releases/step.md)):

| | v0.1.0 | v3.0.0 | v3.6.0 | v3.7.0 |
|---|---|---|---|---|
| sent with every request (instructions + skills + tools) | 4,063 | 6,718 | 7,783 | **4,938** |
| of it, the tool definitions | 1,834 (22 tools) | 4,363 (24) | 4,990 (25) | **2,167** (17 listed) |
| input over form, table, orders and long (no cache) | 248,228 | 349,873 | 367,006 | **250,701** |

Against v3.6.0: −37% per request, −32% over those tasks (−31% over all
six with `batch`); against v3.0.0 −26% and −28%. v0.1.0 had a third of
the tools (no drawing, design, 3D, `locate` or scripts: it can't do the
two canvas tasks) and sends about as much (+1% for v3.7).

Against Codex's behaviour, simulated on this server (a screenshot with
every look, no screen memory, no change report, every tool listed; not a
run of Codex): over five tasks −33% input, 5 screenshots instead of 11,
and −50% with `batch` ([bench/results/v3.7-codex](bench/results/v3.7-codex/compare.md)).

### On by default

What v3.6 added behind settings is on now; each can be turned off:
lean tool schemas (`tools.descriptions = "lean"`), action reports of the
changes around what was acted on (`tree.report = "relevant"`), elements
that keep changing summed up (`tree.quiet_volatile`), fewer automatic
pictures while the model doesn't use pixels (`screenshot.adaptive`),
`locate` without its picture (`screenshot.locate_picture = false`), blind
areas read and watched (`ocr.blind_regions`), paint steps when asked
(`tools.design_steps = "asked"`) and `_meta` for hosts that trim their
context (`server.result_meta`).

### The tool manager, on by default (`tools.manager = "dispatch"`)

- The model gets the tools most tasks use (looking, finding, waiting,
  clicking, typing, keys, scrolling, dragging, `batch`, `screenshot`),
  plus `find_tools` and `use_tool`. Drawing, the design board, 3D,
  `locate`, windows, scripts, the clipboard, notifications and decisions
  are found by name, category or what they do, and run through
  `use_tool`. The list never changes, so a client's prompt cache holds;
  nothing hidden is forbidden.
- `find_tools` names every tool it can find, by category, so one can be
  asked for by name (`find_tools(name="locate")`); a query ignores words
  that name nothing and returns the best three; a tool's arguments are
  shown once (`again=true` repeats them).
- A hidden tool called with wrong arguments gets its arguments with the
  error, once: no `find_tools` needed first.
- `decide` is listed once a decision model is set up (with compact
  descriptions too). `window` no longer repeats its list of actions.
- A small preset, or an `enabled` list of base tools only, gets no
  manager. `tools.manager = "off"` lists every tool, as before.

### Instructions and skills

- The instructions and `get_app_state` no longer say to look "on every
  turn": look first, then read the state each action returns, as the
  skill says. A look the report already gave costs a whole request.
- `--instructions full|short|off`; the Claude Code plugin, which brings
  the skills, starts the server with short ones (−394 tokens a request).
- The `computer-use` skill: the tool manager in one place, the rules the
  security skill makes referred to, the decision model in a few lines
  (details in `reference/decisions.md`). The design skill says where its
  tools are.

### Results said once

- A diff's intro: in full once, its legend once more, then "Changes:".
- "get_app_state shows them", "call get_app_state for the rest", draw's
  preview legend, the loupe's and a zoomed screenshot's notes: in full the
  first time, then short (`tree.brief_repeats = false` keeps them whole).

### Less work done twice

- The tool lists are built once and kept until a setting, a saved script
  or a found category changes them; the server no longer rebuilds them
  after every request (a ping included).
- Waiting after an action: reads that still show the state from before
  come further apart, so an action that changed nothing walks the tree
  fewer times.
- Blind areas' OCR is kept per area, by its exact pixels: a caret or a
  clock elsewhere no longer makes every area be read again. Tesseract's
  two readings of an area run side by side, one thread each
  (`OMP_THREAD_LIMIT=1`, unless set: two processes' OpenMP threads
  spinning against each other took 20 s for a 0.15 s reading).
- `wait_for(until)`: the window just looked at is the one asked about,
  and the same text isn't asked about again for 5 s.
- `batch` doesn't make the steps' own change reports (never shown).
- `screenshot(app)` uses the picture this call's look already took.

Time (tools only, scripted): orders 1.5 s → 1.2 s, board 1.5 s → 1.2 s
(a look that reads painted text ~0.9 s → ~0.6 s), the others as before.

### Benchmark

- `agent_bench --server BIN [--server-args …] [--server-config FILE]
  [--skills DIR]` runs the scenarios against any release over MCP, with
  its own instructions and skills, and reports the fixed prefix by part.
- `bench/compare.py` puts runs side by side, section by section.

### Fixes

- Text read off the screen that is the on-screen indicator's own label
  ("Zero is thinking…", even misread) is never taken for the app's text:
  where a capture catches the indicator, blind areas read it.
- A just-started indicator helper gets up to a second to hide before the
  first picture (then 150 ms, as before).

Costs, published with the gains:

- Blind areas on by default read canvases and painted text on every look:
  with `batch`, the six tasks take 15.7 s of tool time instead of 13.7 s
  (`ocr.blind_regions = false` for the old speed). Step by step, 11.1 s
  against 11.0 s.

- A tool the model doesn't see takes one `find_tools` call the first time
  (about 155 tokens of result): shapes and board need 4–5 calls instead
  of 3–4 when they use `locate`. Wrong calls and retries a real model
  makes with the manager are still to be counted (real-model runs).
- The full instructions grew by 53 tokens (they say how hidden tools are
  found).
- Board's OCR reading varies from run to run in v3.6 and v3.7 alike (one
  run in three, or two, falls back to `locate`).

## v3.6.0

Everything the [roadmap](docs/ROADMAP.md) planned for v3.6 to v4.0, in
one release, on top of v3.5's measuring: trees and reports that say each
thing once, OCR that suits the content, a lighter tool surface, actions
that check their result and need fewer round trips, design answers that
say what changed, and optional layers no feature depends on. Upgrading:
[docs/MIGRATING.md](docs/MIGRATING.md).

What it doesn't settle yet, and the roadmap keeps open: the real-model
A/B runs that decide which of the opt-in settings become defaults (the
benchmark is ready for them, a key is all it needs), and tests on real
Windows and macOS hardware (the Windows and macOS code builds and passes
its unit tests in CI, as before).

### Trees and reports: each thing said once (`tree.compact`, on)

- Look-alike siblings are **records**: the roles once (`7 × button:`,
  `12 × list item › text:`), then each one's index and what is its own;
  a **table** is its column names once and a row a line. Folding keeps
  whole rows.
- Flags and actions the role implies are left out: a text field is
  editable unless it says `read-only`; a checkbox toggles.
- Diffs list elements added together under their parent once, and many
  removed ones as ranges of indices. A look at a window as it was names
  only the app and window; a look that changed nothing is one line.
- `set_value` doesn't echo a value the field now shows.
- `tree::expand` and `tree::index_of` read records back one a line, for
  code that reads trees.

### Looking at less

- `get_app_state(within=index)`: one element and what is in it, without
  moving the base of later diffs.
- `get_app_state(about="…")`: only the parts of the window about that,
  the others folded to a line each; by the words (names and values), or
  by the decision model's judgment when one is set up.
- `find_element(offset)` pages through many matches.
- `tree.report`: `"full"` (default), `"relevant"` (the changes around what
  the action acted on, added elements, the focused one, and a count of the
  rest) or `"brief"` (how many, and a new screen). `tree.quiet_volatile`
  (off): elements that keep changing on their own summed up in a line.
- When the pixels change where the tree reports nothing (GTK keeps a
  filtered table's old cell text over AT-SPI), `get_app_state` says where,
  and to trust the screenshot there.

### OCR

- Every reading takes out thin lines that cross most of the picture (graph
  paper, rulers, table borders), then reads it as it is and enlarged,
  keeping the surer line of each place: the benchmark's board now reads
  all eight labels.
- A strip of one or two lines is read as a line (Tesseract mode 7), the
  rest as sparse text; shapes and rulers' evenly stepped numbers are left
  out of every reading; a line read with low confidence says `unsure
  reading`.

### Pictures

- `screenshot.adaptive` (off): once the model has looked at an app a few
  times without using its pixels, a well-described window's automatic
  picture is held back (and the result says so once); any use of pixels
  brings them back.
- `locate` can answer without its picture (`picture: false`, or
  `screenshot.locate_picture = false`).
- `screenshot.icon_sprite` (off, experimental): with no screenshot
  attached, a strip of the buttons that have no name, numbered with their
  indices, each shown once.

### The tool surface

- `tools.descriptions = "lean"`: nested objects as the list of their keys,
  no `window` (still accepted) and no defaults in the schemas, `decide`
  only once a decision model is set up.
- `tools.manager` (off): `"dispatch"` shows the base tools and
  `find_tools`, which returns the others (by category or by what they do)
  with their arguments, run through `use_tool`; the list never changes, so
  any client and its prompt cache keep working. `"list_changed"`: a
  category found joins the list for good, and the client is told. Hidden
  is not forbidden: a direct call still runs, and the settings still
  decide what may run.
- `tools.preset = "small"`: ten tools for smaller models.
- `tools.default_app`: a call without `app` acts on the app last named.
- `server.instructions`: `"short"` or `"off"` for clients that load the
  skills. The computer-use skill is shorter and says the new ways.
- Definitions per request (estimated): full 13,088 tokens, compact 4,986,
  lean 3,715, small 1,049.

### Actions: checked, and fewer round trips

- **`expect`** on `click`, `set_value`, `type_text`, `press_key` and
  `perform_secondary_action`: `"dialog"`, `"menu"`, `"change"`,
  `"value"`, `"gone"`, or a text that should then be on screen. The server
  waits for it (up to `timing.expect_wait_ms`, 2 s) and says on the
  action's line: confirmed, not seen, or uncertain (and not to repeat what
  isn't confirmed without looking).
- **`click(name, role)`**: when one element has that name (an exact name
  wins over a part of one); several are listed and nothing is clicked.
- **`batch` lines**: `click 12`, `click "Save"`, `double 12`, `right 12`,
  `set 4 "Ada"`, `type "text"`, `key cmd+s`, `scroll 7 down 2`, `select 4
  "word"`, `action 9 show_menu`, `wait "Saved"`, `find "Total"`, `look`,
  each with an optional `expect …`. A batch stops when a step brings up a
  window it didn't expect (`through_windows: true` goes on) or what a step
  expected isn't confirmed, and ends with **one report** of what the steps
  changed.
- **`launch_app`** returns the app's first state (`tools.launch_look`).
- A **table cell** whose press changed nothing is clicked with the mouse
  (GTK's press doesn't select the row; selecting is safe to repeat).
- **Long conversations**: `cache.rebase_after_tokens` (off) sends a whole
  tree and a picture again once that many tokens have gone by since the
  app's tree was last sent whole; `get_app_state(rebase=true)` asks for it.

### Design and scene

- After the first answer, only what changed: the layers (objects)
  changed, added or removed and the new order; checks and paint steps
  "as before"; no picture when its pixels are the ones sent last. A call
  that changes nothing gets everything.
- `tools.design_steps = "asked"` (default `"always"`): the paint steps
  only with `show: {"steps": true}`.
- Layers, scene objects and `draw` strokes can be **lines**, the way the
  listings show them: `"sun ellipse 80 20 12 12 fill #ffcc00"`, `"seat box
  0.5 0.5 0.05 at 0 0 0.45 color #884422"`, `"rect 10 10 50 30"`.

### Optional layers and hosts

- `decide(app, pick, read=true)`: the element's whole text or value, read
  by the server (extraction without reading the window).
- Linux: a control without a name gets the name of the label it is
  labelled by (AT-SPI relation; GTK's mnemonic labels). The benchmark's
  form fields are `text field "Full name"` now, not a nameless field.
- `server.result_meta` (off): each tool result's `_meta` has a number and
  the earlier results it repeats whole
  (`zero-use-computer/supersedes`) or whose pictures it replaces
  (`zero-use-computer/supersedes-images`), so a host that trims its
  context can drop them.

### Benchmark

- `--plan batch`: scripted ways through that use batch lines, clicks by
  name and `expect`, next to the one-action-a-call way (`--plan step`).
- `bench/configs/lean.toml` (every opt-in that saves tokens) and
  `manager.toml` (the same with the tool manager), for A/B runs.
- Scripted results (estimates, all 90 runs successful): over the six
  tasks, result tokens −25% with the defaults and −35% with lean settings
  and batches against v3.2; calls 42 → 27 with batches; the 300-row
  table −39%, the canvas with painted labels −41% (one picture instead of
  two). Costs: the fixed prefix +5% with the defaults (~6,941 → ~7,309
  tokens; lean ~6,040, with the manager ~4,397), half a second per look
  at a canvas with blind areas on. [bench/results](bench/results/README.md).

## v3.5.0 (released as part of v3.6.0)

The first step of the road to v4.0 ([roadmap](docs/ROADMAP.md)): spend
fewer tokens on finishing a task, measured with the real model before
anything is turned on by default.

### Measured, not assumed

- **`bench/`**: an agent benchmark. Six tasks in real apps (a form, a
  300-row table, a canvas with painted labels under a full toolbar, a
  canvas without text, a mixed form-and-dialog task, a longer session),
  from one GTK fixture app that records what was done: success is checked
  from the app, never from what the model says. Each run starts the app
  afresh.
- **Real runs** drive Claude through the Messages API (`curl`, the key on
  its input) and record every request's input, output, cache-read and
  cache-write tokens, the model that served it and the cost; `--calibrate`
  counts each tool result's real tokens to check the server's estimates.
  **Scripted runs** need no key and give estimates, labelled as such.
- `bench/results/v3.2` is the scripted v3.2 baseline, taken before any
  change below; `bench/results/README.md` sets every change against it,
  costs included.
- The README's comparison with Codex is labelled for what it is: a
  simulation of Codex's behaviour with this server, with estimated tokens.

### Screenshots

- `x`/`y`/`width`/`height` without a mode is a region of the screen. With
  another mode, or with `app`, it is an error that says what to use
  instead; they used to be silently ignored.
- `screenshot(app)` goes the way `get_app_state`'s picture goes: nothing
  when the window looks as in the model's last picture of that screen,
  only the part that changed when that is small, else all of it, which
  then is the picture `x`/`y` refer to. `mode: "window"` still always
  sends all of it.
- Every window and full-screen screenshot is numbered ("Screenshot #7"),
  and a changed part names the one it patches.

### Blind areas (opt-in: `ocr.blind_regions`)

- How many elements a window has no longer decides alone whether its text
  is read: the areas the tree says nothing about (an element with nothing
  informative in or under it, such as a drawing area or a picture, and
  what no element covers) are found however full the rest of the window
  is, and kept only if they show something, never the empty margins of a
  form.
- Just those areas are read off the screen; Tesseract reads each as it is
  and enlarged and keeps the surer line of each place (enlarged alone, a
  canvas's framed labels came out as "Ecce"), and leaves out lines that
  look like shapes rather than text.
- `get_app_state` says once per screen that part of the window has no
  accessibility information, and checks those pixels on every look (an
  unchanged picture is still not sent again).
- Scripted benchmark: the canvas task takes 2 calls and 1 picture instead
  of 4 and 2 (−47% result tokens), the mixed task 4 calls instead of 5;
  it costs about half a second of reading on canvas windows. Off until a
  real-model run shows it doesn't cost success.

## v3.2.0

A decision model for speed, set up with Ctrl+Alt+J; apps without
accessibility on Linux; fixes from testing on Hyprland with Firefox
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v3.1.0...v3.2.0)).

### Decision model (Jev)

- **`decide`**: typed answers from a fast decision model — yes/no (the
  probability of yes), one of some options, a score on a scale — about
  text or JSON, **each of many items in parallel** (with a summary), or an
  app's window (which never enters the conversation). `pick` returns the
  `element_index` of the element a description means.
- **`wait_for(until=…)`**: waits until the model answers yes about the
  window ("have the results loaded?").
- **Scripts**: `ask`, `choose`, `score`, `decide`, `decide_each`.
- Speaks **TypeSafe's System One API** (Jev, and servers that speak it:
  local-jev, jeff, LiteLLM) and any **OpenAI-compatible** chat API (a JSON
  answer from a small, fast model).
- **Ctrl+Alt+J** (`control.settings_hotkey`) opens a settings page in the
  browser, from any app: kind of model, address, model, key; Test and
  Save; used at once. Registered on X11, Hyprland and sway (a compositor
  binding), Windows and macOS like the stop key. `computer-use-mcp
  settings` opens it too. The skill suggests it once when a task would
  gain from it; the agent sets the model from the chat only when the user
  asks.
- The key stays private: kept in `config.toml` (then readable by its owner
  only), given to `curl` on its input, never shown back (the page,
  `doctor`, `config show/get` and the tool show its last four characters).
  The page lives on 127.0.0.1 at a random address, refuses other sites and
  host names (DNS rebinding), and closes after 15 minutes unused.
- `doctor` reports the settings key and asks the model a test question.

### Linux

- **Apps without accessibility** (terminals such as foot, kitty, xterm;
  some Electron apps) are listed from the compositor (Hyprland, sway) or
  the window manager (X11, including apps that never say their pid, found
  through XRes), and their windows are used through screenshots (and OCR
  when Tesseract is installed), the mouse and the keyboard. Before, they
  only showed up when in front, and then as an error.
- "Desktop" (listed when no app window has the keyboard) now says what it
  is and what to do instead, not "not supported on this platform".
- Browsers' pages show their address (Firefox's `DocURL`, Chromium's
  `URI`) as the value of the web area.
- Actions without a proper name (Firefox gives some a key binding as their
  name, `;;`) are named from their description, or dropped; an unnamed
  default action is still clickable.
- A Wayland desktop that keeps input and screenshots from other programs
  (GNOME, KDE Plasma) is named once in `list_apps`, with what still works.

### Everywhere

- `launch_app("https://…")` opens a web address in the default browser.
- Browser notes in the skill: one way to enter an address (never set and
  typed), the address in the tree, Wayland, and never working around the
  tools with ydotool or wtype.

## v3.1.0

Wayland: Hyprland, sway and the other wlroots-family compositors work in
full, and the overlay stops flickering under compositors on every system
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v3.0.0...v3.1.0)).

### Hyprland, sway (Wayland)

Before, in a Wayland session only accessibility actions worked for native
Wayland apps: clicks at a place, typing, screenshots and window changes
only reached XWayland apps, element positions were off (a Wayland app
only knows where things are inside its own window), and the stop key
mostly didn't fire.

- **Windows come from the compositor** (Hyprland's or sway's IPC): where
  each window is, which has the keyboard, and focusing, moving, resizing,
  maximizing, minimizing, full screen, closing and moving to a workspace.
  Element positions are placed on screen from there, allowing for the
  shadow apps like GTK draw around their windows.
- **Input through the compositor**: clicks, drags, drawing and scrolling
  through wlr-virtual-pointer; keys and text through a virtual keyboard
  with a keymap made for what is typed, so any text (Persian, CJK, emoji)
  comes out exactly whatever the keyboard layout. XWayland apps too.
- **Screenshots through wlr-screencopy**, at each output's own scale
  (fractional scales included), across several outputs.
- **The user's idle time** from ext-idle-notify, so the pause while you
  use the mouse or keyboard works; without it the engine doesn't guess.
- **The overlay is a layer-shell surface**: made once, never unmapped
  (hiding for a screenshot shows a transparent buffer, in about 1 ms), so
  Hyprland never replays its open and close animations on it; sharp at
  fractional scales; click-through; on every output. Layer rules can
  target the `computer-use` namespace.
- **The stop key is bound in the compositor** while the server runs (and
  bound again if the compositor reloads its config), never over a binding
  of yours.
- `doctor` says what the compositor offers. Other Wayland desktops (GNOME,
  KDE Plasma) keep the previous behaviour: accessibility actions for every
  app, input and screenshots for XWayland apps.

Tested live on headless sway (the wlroots protocols Hyprland speaks too),
at scales 1, 1.25 and 1.5, and in CI. Hyprland's IPC is tested against a
simulated server: please report anything that behaves differently there.

### Overlay on every system

- **X11**: under a compositor (picom, xcompmgr, KWin…) the overlay faded
  in and out around every screenshot (sometimes still visible in it) and
  got shadows; without one, it flashed black when shown again. Its
  windows now stay mapped and are emptied instead; the screenshot waits
  until the compositor has drawn that; images are in place before a
  window shows; compositors are asked for no shadow, and the windows are
  named `computer-use-overlay` for compositor rules.
- **macOS**: a border around a window on a secondary display was cut off
  entirely; the overlay now spans every display. Window animations off,
  and it stays up when another app hides the others.
- **Windows**: no show/hide animation and no rounded corners on
  Windows 11.
- Reloading the settings no longer hides and re-shows everything.

## v3.0.0

Stability first: no call can hang the server or run for hours, a busy or
hung app is told apart from one that failed, and an action that was sent
but not answered is never repeated. Fewer tokens and less memory on top
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v2.6.0...v3.0.0)).

### Stability, everywhere

- **An action sent but not answered is never done twice.** A press, value
  or secondary action the app didn't answer in time (busy, hung, a dialog
  opened) is reported as "sent, may or may not have happened; look before
  repeating it", and is not retried with a mouse click or by typing.
- **Clients can cancel.** The server reads its input on a thread of its
  own: `notifications/cancelled` ends the call that is running (a long
  drawing, a script, a wait) as the stop key would, and a request cancelled
  before it starts never runs. What a dropped answer would have shown is
  sent in full by the next look.
- **Every call is bounded**: `type_text` up to 100,000 characters (typed in
  pieces, the stop key checked between them), `press_key` 500 presses,
  `scroll` 50 pages, `get_clipboard` 30,000 characters, design pages 500
  layers, pictures 50 megapixels; huge angles, brushes and noise pictures
  no longer stall drawing, scenes or pixel searches; calls nest at most 8
  deep and a script can't start another script.
- **Subprocesses can't hang it**: Tesseract runs with a 60 s limit, `curl`
  is killed by the stop key or a cancel, and every child is waited for.
- **Files**: scripts read and write only regular files (never a device or
  a pipe); settings are written atomically, and a settings file caught
  half-saved is not taken; the audit log rotates at 10 MB.
- **Coordinates follow a window that moved**, so the model's screenshot
  still names the right places.
- The overlay helper's command queue is bounded; a helper that stops
  reading is replaced. Refused HTTP requests are answered off the main
  loop. On Windows, apps the server starts no longer inherit its stdio
  pipes (the client always sees the server end).

### Windows

- UI Automation calls are bounded (CUIAutomation8 timeouts, a 10 s budget
  per tree read that returns what it has); VARIANTs are freed.
- A hung window (`IsHungAppWindow`) is never waited on: window changes are
  asynchronous and checked, captures of it come off the screen (only when
  nothing covers it), and its actions report "not responding".
- Focus works past the foreground lock without stray key presses; a
  minimized window must be restored before input goes to it.
- Points off every monitor are refused; the wheel is clamped; ghost
  windows (cloaked, overlays, untitled tool popups) are left out; the
  pointer is put back after the click lands; handles of closed apps are
  dropped; capture fixes (DC leak, GdiFlush, overflow); notification reads
  are bounded; COM runs multithreaded, with shell launches on a short STA
  thread.

### macOS

- Accessibility calls are bounded on every element (a system-wide
  messaging timeout); an app that times out ends the tree read at once
  with what was read, and presses that time out are "sent, not answered".
- Every Objective-C call runs in an autorelease pool (no growing memory in
  a long session); Screen Recording permission is checked and reported.
- Clicks, drags and scrolls carry no held modifier keys, and scrolls land
  at the target point; focus also sets AXFrontmost and waits for full
  screen to end before moving a window.
- Values are type-checked before use; pixel formats are read as they are;
  concealed clipboard content (passwords) is refused; the overlay follows
  screen changes and exits if its parent is gone.

### Linux

- Keys go only to the app they are for: the window that has the keyboard
  comes from the window manager (`_NET_ACTIVE_WINDOW`), a terminal or
  other app without accessibility counts as in front, and focusing checks
  the window really got the keyboard.
- Typing works whatever keyboard layout is active (the group is locked to
  the first layout and Caps Lock released while typing, then restored).
- AT-SPI calls are bounded: a press or value the app doesn't answer is
  "sent, not answered"; slow elements are retried in smaller batches; a
  tree read returns what it has after 8 s; a busy app (a modal dialog
  open) stays listed.
- Huge lists (a 100,000-row table) are read by index, first 256 rows:
  0.97 s instead of 8.2 s.
- A broken AT-SPI or X connection is replaced on the next call; the X
  event queue is drained and the keymap reloaded when it changes.
- Clipboard helpers give up after 3 s and are always reaped; `wl-*` only
  under Wayland. Under Wayland, input to apps without an XWayland window
  is refused instead of going astray.
- Actions a toolkit names with a sentence (GTK's table cells) get short
  names.

### Fewer tokens

- After an action's result showed the start of a new screen, the next
  `get_app_state` sends only the rest.
- A changed line in a diff ends with only what differed (`…unchecked)`),
  not the whole old line.
- How-to paragraphs in `design`, `trace_image` and OCR headers are said
  once per session.
- Tool definitions share repeated schemas (compact mode).

Measured with `examples/compare.rs` (gtk3-widget-factory, 10 round trips,
61 calls): **6,821 tokens** against 46,280 the way Codex's computer use
behaves (**6.8x fewer, 85% saved**; v2.6.0: 7,164), 2 screenshots instead
of 21, `get_app_state` in ~5.6 ms instead of ~17 ms. Tool definitions:
~4,400 tokens per request (v2.6.0: ~4,900).

### Less memory

- Fonts are memory-mapped instead of read into the heap; freed memory is
  given back to the system after big calls.
- Measured over a session (look, screenshots, design, scene, script):
  the server holds **14.7 MiB** (v2.6.0: 24.3), the overlay helper 10.9
  MiB (13.1), peak 22 MiB.

### Notes

- Linux is tested live (Xvfb, AT-SPI, GTK). The Windows and macOS changes
  are type-checked and reviewed but not yet run on real hardware: please
  report anything that behaves differently.

## v2.6.0

Scripts: the model writes a small program and the server runs it, for
what the tools can't do in one call, on the graph-paper page or anywhere
else; and a saved script becomes a tool of its own
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v2.5.7...v2.6.0)).

### New

- **`script`** runs a program in [Rhai](https://rhai.rs), a sandboxed
  language that reads like JavaScript. A script can:
  - run any tool with logic around it: `tool(name, #{...})` gives the
    tool's text and stops the script on a failure, `try_tool` gives
    `#{ok, text, image}`, `set_app` fills in the app;
  - read the screen as data: `elements(app, #{role, name, text})` gives
    the matching elements with their states and boxes, `colors(app,
    points)` the exact pixel colours;
  - draw on **the graph-paper page**: `page(name, w, h, #{cell: size})`
    is a design-board design with named cells. `p.fill_cell("C4",
    colour)`, `p.text_in("C4", text)`, `p.cell("C4")`, `p.at(x, y)` and
    every shape and text the board has (`rect`, `circle`, `line`, `path`,
    `polygon`, `star`, `arc`, `curve`, `text`, `layer`). The picture comes
    back with the result, and `p.steps()` and `p.export("png")` take it
    into an app. `cells(w, h, size)` gives the same cells for any canvas;
  - use data: `data` and `args` from the model; files (`read_text`,
    `read_json`, `read_csv`, `write_text`, `write_json`, `write_csv`,
    `list_files`); the web through `curl` (`fetch`, `fetch_json`,
    `download`); `remember`/`recall` between runs; JSON and CSV, regular
    expressions, `numbers(text)`, maths that takes whole numbers too,
    random numbers, colours (`rgb`, `hsl`, `mix`) and dates;
  - run saved scripts (`run(name, args)`) and import them as libraries.
- **Saved scripts are new tools.** `script(save=name, code, description,
  params)` checks the script and keeps it in
  `~/.computer-use/scripts/<name>.rhai`, plain text the user can edit. It
  is then a tool of its own with its own arguments (the server tells the
  client its tool list changed), and `run`, `list`, `show` and `delete`
  manage saved scripts.
- **Errors say where.** A script that doesn't parse is refused before
  anything runs; misspelt variables are caught then too. A failure gives
  the line and its code, and what the script printed before it.
- **`cell_size`** on `design`, `draw` and `screenshot` fixes the cells'
  size, so all of them (and a script's page) name the same cells: an 800 x
  800 board with `cell_size: 100` is a chessboard, A1 to H8.
- **`[script]` settings**: saved scripts as tools on or off; which files
  scripts may use (`none`, `workspace`, `read` — the default: read any
  file, write in the scripts' own folder — or `all`); web access; the
  time limit (`max_seconds`, 300); the scripts' folder.

### Safety

- A script reaches the computer only through the tools and its listed
  functions. Its tool calls are ordinary calls: the stop key, the pause
  while the user works and private-data masking apply. The stop key and
  the time limit end a script even inside `try`, and a script never
  writes to the server's stdout.
- The security skill covers scripts: read only the files the task needs,
  write in the scripts' folder unless asked, fetch only what the task
  needs and never send the user's data to a site unless that is the task,
  no secrets in saved scripts or memory, and a loop of consequential
  actions still needs the user's go-ahead.

### Other

- **License: Apache-2.0** (it was MIT or Apache-2.0), with a NOTICE file
  that copies and derivative works must keep. The release zips include
  both. The project now lives at
  [github.com/mhrsdev/zero-use-computer](https://github.com/mhrsdev/zero-use-computer).
- What the server writes says where it comes from: exported SVG, PNG and
  OBJ files name computer-use, as do `--help`, `doctor`, the MCP server
  title and the web requests scripts make.
- `examples/compare.rs` measures one agent session in Codex's behaviour
  and with this server's defaults: over 10 round trips the model gets
  ~85% fewer tokens (6.6x) and 2 screenshots instead of 21 (README,
  "Compared with Codex's behaviour").

### Docs

- New reference: `skills/computer-use/reference/scripts.md` (also
  `script(help=true)` and an MCP resource): the language in short, every
  function, the page, saving tools, examples and rules. The README, the
  skills and the server's MCP instructions mention scripts.

## v2.5.7

See a drawing before it is built: a design board for 2D, a scene for 3D,
graph-paper cells to place and check every part, and exact aiming at small
targets
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v2.5.1...v2.5.7)).

### New

- **`design`**: a design board, like Canva. A picture is built from layers
  (the shapes `draw` takes, and text) that can be added, changed, removed,
  mirrored (the other eye or ear), aligned, distributed and reordered. Each
  call returns:
  - the rendered picture;
  - every layer with its box and colours;
  - checks: off the page or into the margin, nearly centred, pairs not
    quite symmetric, hard-to-read or overlapping text, too many colours;
  - the steps to paint it.

  `show` adds a grid, the layers' ids or guides.
- **`draw` strokes `{"design": name, "step": n, "fill": w}`** paint one
  colour step of a design, fitted into the canvas.
- **`scene`**: a 3D model planned as solids before it is built in an app.
  - Solids: box, cylinder, sphere, cone, torus and plane, each with a
    size, a centre and a rotation (Z up, the ground at z 0).
  - Editing: add, change, remove, mirror and repeat (in a row, or around
    an axis). `on` sets one part on top of another.
  - The picture: the front, right and top views to one scale with a grid,
    and a perspective view with shadows straight down.
  - Checks: parts that float (and how far above what), sink into the
    ground or run into each other (and how deep).
  - Building it: the numbers for Blender's Location, Rotation and
    Dimensions fields, or for any other app.
- **Exports are temporary.**
  - `design` writes SVG or PNG; `scene` writes OBJ (with an MTL file of
    its colours) or PNG.
  - They go to `computer-use-exports` in the system's temp folder and are
    deleted when the server stops.
  - Files older than a day are removed, the folder is kept under 200 MB,
    and nothing is ever overwritten.
- **Cells: graph paper for drawing.** The area is cut into square cells
  named like a chessboard (columns A, B…, rows 1, 2… from the top left),
  sized so what is drawn spans about eight of them.
  - The `draw` preview shows them, and every `draw` result says which
    cells the drawing covers.
  - `screenshot` with `canvas` takes `cells: true` to lay them over the
    document, and `cell: "C4"` to magnify one cell with a fine grid in the
    document's units and its main colours.
  - The design board shows them too; `show: {"cell": "C4"}` opens one with
    the layers in it.
- **Pixel targeting:**
  - `screenshot` `zoom: [x, y]` magnifies around a point, each screen
    pixel a square, with a crosshair and a grid in click coordinates.
  - `locate` finds exact places in a window: every area of a colour, every
    look-alike of an icon or marker, or the exact corner, edge or centre
    next to a rough point.
  - `click` and `drag` take `snap` (`corner`, `edge`, `center` or a
    colour) to move onto that point before acting.

### Changed

- **The design skill** plans every drawing on the board first: `design`
  for 2D, `scene` for 3D.
  - A new reference page, `reference/board.md`.
  - Recipes for a badge painted from a design and a 3D model built from a
    scene.
  - Checks for the board, cell by cell and in 3D.
  - "From a scene" sections in the Blender and SketchUp playbooks.
- **The computer-use skill** covers cells, `locate`, `snap`, `zoom`,
  `design` and `scene`.
- **The security skill**: exported files are temporary; finished work is
  saved only where the user asked.

## v2.5.1

Better drawing and copying of pictures, from a test where a smaller model
copied a photo of a cat in Paint
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v2.5.0...v2.5.1)).

### New

- **`trace_image`**: turns a reference picture (an image file, or part of
  a window) into a few flat colours and shapes, as numbered steps to paint
  back to front. It returns the steps and a picture of the result; `colors`
  and `detail` set how close it comes.
- **`draw` strokes `{"trace": name, "step": n, "fill": w}`** paint one step
  of a trace, fitted into the canvas with its proportions kept.
- **`fill: w` on any closed `draw` stroke** paints it solid with a brush
  `w` wide instead of its outline. The brush stays inside the edge, and
  shapes painted back to front cover each other. No bucket fill, so
  overlapping shapes come out right even in apps without layers. The
  result warns when shapes are narrower than the brush.
- **`screenshot` `compare: name`** (with `canvas`) compares the document
  with a trace: how many cells look alike, and the most different places
  with the colour each should be.

### Changed

- **Fill points are checked on the real pixels.** After drawing (or on the
  preview), `draw` floods the picture like a bucket fill does. It gives
  one click per piece when other lines cut a shape into pieces, and says
  where an outline has a gap a fill would leak through. It used to give
  one point from the shapes alone.
- **The design skill** paints overlapping shapes solid (Paint's playbook
  explains why outlines and bucket fills go wrong there), copies photos
  with `trace_image` (a new recipe), and checks with `compare`.
- **The security skill**: `trace_image` reads only a picture the user gave
  or pointed to.

## v2.5.0

Compared with v2.0.0
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v2.0.0...v2.5.0)).

### New

- **`draw`**: draws with the mouse button held down.
  - Shapes: rectangles (rounded corners too), ellipses, arcs, regular
    polygons, stars, Bézier curves, straight or smooth freehand lines.
  - Parametric curves `x(t)`, `y(t)` and function plots `y = f(x)` with
    ticked axes.
  - Any stroke can be rotated and repeated (rows, radial patterns).
  - Coordinates in screenshot pixels, an element's box, a document's own
    units, or a math range with y up (`canvas`).
  - `preview: true` shows the strokes over a screenshot without drawing.
  - The result names a point inside each closed shape to click for a
    bucket fill (between the circles for a ring).
  - Paced by `speed`; the stop key ends it midway and releases the button.
- **`screenshot`**:
  - `grid`: a labelled grid in the coordinates `click` and `draw` use;
  - `palette`: the main colours;
  - `pick`: the exact colour at points;
  - `canvas`: grid and `pick` in a document's units or a plot's range.
- **`press_key` / `type_text` with `x`/`y`**: the mouse points there
  first, for apps that send keys to what is under the pointer (Blender,
  CAD). Keypad keys: `Numpad0`…`Numpad9`, `NumpadDecimal` and the keypad
  operators.
- **Design skill** (`skills/computer-use-design`) for Photoshop, Paint,
  GIMP, Krita, Illustrator, Inkscape, Figma, Blender, Revit, AutoCAD and
  SketchUp, from a text brief or a reference image:
  - a spec of exact numbers first;
  - the most exact method each app has;
  - checks after every pass;
  - an app playbook for each app, and recipes for common jobs.
  - Offered over MCP like the other skills, as the `computer-use-design`
    prompt and its files as resources.

### Fixed

- Skill files served over MCP had Windows line endings (CRLF) on Windows.
  They now have LF on every OS.

### Changed

- **No approvals in the server.** Per-app approvals, the sensitive-app
  categories, the on-screen approval window (and the indicator's "waiting
  for approval" state) and `change_setting` are gone.
  - Which apps and actions need the user's OK is set by the
    `computer-use-security` skill. Every skill prompt brings it along.
  - The server still keeps input on the app it is meant for. Passwords
    and card numbers are masked. The emergency stop key works.
- **`launch_app` takes no command-line arguments** (`args` is gone). It
  starts an app by its name, bundle id or executable, and never runs a
  command line.
- **Settings**: `computer-use-mcp config` on the command line.
- **Removed tools**: `create_folder`, `list_folder`, `read_file` and
  `skill`.
  - For files, use the MCP client's own file tools.
  - Skills still come as MCP prompts and resources, as in v2.0.0.
