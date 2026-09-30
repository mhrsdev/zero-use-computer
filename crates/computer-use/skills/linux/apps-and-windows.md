# Starting apps and switching windows (Linux)

- `list_apps` shows what runs; `launch_app("<name or executable>")` starts an app
  and waits for its window (use the binary name: `firefox`, `gedit`,
  `libreoffice`, `gnome-calculator`). If not found, open the app launcher
  (GNOME: the Activities button, found with `find_element`; a lone `super` key
  can't be sent), `type_text` the name, read the result with `get_app_state`,
  then `Return`.
- Switch windows: `alt+Tab`; windows of the same app `alt+grave`. The `window` tool
  can focus, move, resize or tile a window (window-manager support varies).
- Close a window `alt+F4` / `ctrl+w`; quit an app `ctrl+q` (check nothing is
  unsaved first).
- GTK and Qt apps expose trees through AT-SPI; Electron apps need a second
  read; custom-drawn apps and some Qt apps expose little — use `screenshot`
  and `get_app_state` with `ocr: true`.
- Menus: GNOME apps use a hamburger/"app menu" button; Qt apps have a menu bar.
  Read the state to find them.
- A new window (dialog, popover) may appear after an action: the result says
  so; read its state.
