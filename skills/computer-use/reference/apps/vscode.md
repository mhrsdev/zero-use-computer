# VS Code and other code editors

Electron editors (VS Code, Cursor) show menus, sidebar, tabs and status bar in
the tree, but the editor text only with screen-reader support on. `cmd+`
below is Cmd on a Mac, Ctrl elsewhere.

- Almost empty tree: Command Palette ▸ "Toggle Screen Reader Accessibility
  Mode" (`shift+alt+F1`), then read again. On Linux, if that isn't enough,
  ask the user to start it with `--force-renderer-accessibility`.
- The **Command Palette** (`cmd+shift+p`) does almost everything: type the
  command ("Format Document", "Preferences: Open Settings (JSON)"), `Return`.
  Open a file by name `cmd+p`, go to line `ctrl+g`, symbol `cmd+shift+o`.
- Save `cmd+s`, close tab `cmd+w`, reopen `cmd+shift+t`, toggle sidebar
  `cmd+b`, Explorer `cmd+shift+e`, Search `cmd+shift+f`, Source Control
  `ctrl+shift+g`.
- Editing: find / replace `cmd+f` / `cmd+h` (Mac: `cmd+alt+f`), toggle
  comment `cmd+/`, select next match `cmd+d`, rename `F2`, go to definition
  `F12`, quick fix `cmd+.`.
- Larger edits: select all, `set_clipboard` with the new text, `cmd+v`. The
  editor auto-indents and closes brackets as you type, so after `type_text`
  read the result back and fix it.
- A dot on a tab means unsaved changes.

## Leave alone unless the user asked

- The integrated terminal (`ctrl+grave`), Run / Debug and tasks run
  commands: they count as a terminal (security rule 2).
- Workspace-trust prompts, installing extensions, sign-in (GitHub,
  Microsoft): the user's decision.
- Remote windows (SSH, WSL, containers) edit files on another machine.
