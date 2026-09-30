# VS Code (macOS)

Visual Studio Code is an Electron app: menus, the sidebar, tabs and the status
bar are in the accessibility tree; the **editor text** is only exposed when
screen-reader mode is on (see below). Use the **Command Palette** for almost
everything — it is one text box you can `type_text` into.

## Do it with the keyboard
- Command Palette: `cmd+shift+p` — type a command ("Format Document", "Git:
  Commit", "Preferences: Open Settings (JSON)"), `Return`. Quick Open a file:
  `cmd+p`. Go to line: `ctrl+g`. Go to symbol: `cmd+shift+o`.
- Open a folder as workspace: `cmd+o`; open a file by name: `cmd+p`.
- Menus are in the macOS menu bar (File, Edit, Selection, View, Go, Run…).
- If `get_app_state` shows almost no elements, turn on screen-reader support:
  Command Palette ▸ "Toggle Screen Reader Accessibility Mode"
  (`shift+alt+F1` in the editor), then read again.
- New file `cmd+n`; save `cmd+s`; save all `cmd+alt+s`; close tab `cmd+w`; next tab
  `ctrl+Tab`; reopen closed tab `cmd+shift+t`.
- Sidebar: Explorer `cmd+shift+e`, Search `cmd+shift+f`, Source Control `ctrl+shift+g`,
  Extensions `cmd+shift+x`; toggle sidebar `cmd+b`.
- Editing: toggle comment `cmd+/`; move line `alt+Up/Down`; duplicate line
  `shift+alt+Up/Down`; delete line `cmd+shift+k`; select next occurrence `cmd+d`;
  multi-cursor `alt+click`; format document `shift+alt+f`; rename symbol `F2`;
  go to definition `F12`; quick fix `cmd+.`; find `cmd+f`; replace `cmd+h` (mac:
  `cmd+alt+f`); find in files `cmd+shift+f`.
- Problems panel `cmd+shift+m`; split editor `cmd+\`; zen mode `cmd+k z`.
- Settings `cmd+,`.

## Reading and changing code
- **To read a file, use `read_file`** (cheap and exact). Use the editor UI only
  to see what is open or selected.
- To make a large edit, it is more reliable to: open the file, `cmd+a`, then
  `set_clipboard` with the new content and `cmd+v` — or `type_text` for short
  edits. Editors auto-indent and auto-close brackets, so typed code can gain
  extra characters: after typing, re-read the result and fix it.
- Auto-save may be on; check the tab for a dot (●) meaning unsaved changes.
- Verify by reading the file back with `read_file` after saving.

## The integrated terminal and running code
- The integrated terminal (`ctrl+\``) and Run/Debug execute arbitrary commands.
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
