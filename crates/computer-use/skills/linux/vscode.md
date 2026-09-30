# VS Code (Linux)

Visual Studio Code is an Electron app: menus, the sidebar, tabs and the status
bar are in the accessibility tree; the **editor text** is only exposed when
screen-reader mode is on (see below). Use the **Command Palette** for almost
everything — it is one text box you can `type_text` into.

## Do it with the keyboard
- Command Palette: `ctrl+shift+p` — type a command ("Format Document", "Git:
  Commit", "Preferences: Open Settings (JSON)"), `Return`. Quick Open a file:
  `ctrl+p`. Go to line: `ctrl+g`. Go to symbol: `ctrl+shift+o`.
- Open a folder as workspace: `ctrl+k ctrl+o`; open a file by name: `ctrl+p`.
- Electron apps on Linux expose accessibility only when asked: if the tree is
  nearly empty, turn on screen-reader support (Command Palette ▸ "Toggle Screen
  Reader Accessibility Mode", `shift+alt+F1`), or ask the user to start VS Code
  with `--force-renderer-accessibility`.
- Launch with `launch_app("code")`; under Wayland some shortcuts may be grabbed
  by the desktop.
- New file `ctrl+n`; save `ctrl+s`; save all `ctrl+k s`; close tab `ctrl+w`; next tab
  `ctrl+Tab`; reopen closed tab `ctrl+shift+t`.
- Sidebar: Explorer `ctrl+shift+e`, Search `ctrl+shift+f`, Source Control `ctrl+shift+g`,
  Extensions `ctrl+shift+x`; toggle sidebar `ctrl+b`.
- Editing: toggle comment `ctrl+/`; move line `alt+Up/Down`; copy line up/down
  `ctrl+shift+alt+Up/Down`; delete line `ctrl+shift+k`; select next occurrence `ctrl+d`;
  multi-cursor `alt+click`; format document `ctrl+shift+i`; rename symbol `F2`;
  go to definition `F12`; quick fix `ctrl+.`; find `ctrl+f`; replace `ctrl+h`; find in files `ctrl+shift+f`.
- Problems panel `ctrl+shift+m`; split editor `ctrl+\`; zen mode `ctrl+k z`.
- Settings `ctrl+,`.

## Reading and changing code
- **To read a file, use `read_file`** (cheap and exact). Use the editor UI only
  to see what is open or selected.
- To make a large edit, it is more reliable to: open the file, `ctrl+a`, then
  `set_clipboard` with the new content and `ctrl+v` — or `type_text` for short
  edits. Editors auto-indent and auto-close brackets, so typed code can gain
  extra characters: after typing, re-read the result and fix it.
- Auto-save may be on; check the tab for a dot (●) meaning unsaved changes.
- Verify by reading the file back with `read_file` after saving.

## The integrated terminal and running code
- The integrated terminal (`ctrl+grave`, the backtick key) and Run/Debug execute arbitrary commands.
  Treat them like a terminal: don't use them unless the user asked for it and
  the sensitive-app settings allow it; ask first.
- Extensions and workspace-trust prompts ("Do you trust the authors…") are the
  user's decision: don't accept them on your own.
- Sign-in flows (GitHub, Microsoft) and credential prompts are blocked on
  purpose; stop and ask.

## Quirks
- Notifications appear bottom-right; dismiss with the ✕ (they can cover buttons).
- Dialogs ("Do you want to save…?", "Delete?") — read the buttons; Don't Save
  and Delete are destructive.
- Multi-root workspaces and remote (SSH/WSL/Dev Container) windows look like
  normal windows but the files are elsewhere — `read_file` can't see them.
