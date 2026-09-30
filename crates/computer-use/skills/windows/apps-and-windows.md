# Starting apps and switching windows (Windows)

- `list_apps` shows what runs; `launch_app("<name>")` starts an app and waits for
  its window. For something not found by name, press `win`, `type_text` the name,
  check the first result with `get_app_state`, then `Return`.
- Switch windows: `alt+Tab`; with the `window` tool you can focus, move, resize or
  tile a window directly (no shortcuts needed).
- Snap: `win+Left` / `win+Right`; maximise `win+Up`; minimise `win+Down`.
- Virtual desktops: `win+ctrl+d` new, `win+ctrl+Left/Right` switch.
- Close a window `alt+F4` (closes the app if it's the last window — make sure
  nothing is unsaved first).
- UWP / Store apps expose rich accessibility trees; Electron apps need a second
  read after launch; games and custom-drawn apps expose little — use `screenshot`
  and `ocr`.
- A new window (dialog, menu, flyout) appears after an action: the result tells
  you; call `get_app_state` for the new window.
