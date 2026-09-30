# Starting apps and switching windows (macOS)

- `list_apps` shows what runs; `launch_app("<name or bundle id>")` starts an app
  and waits for its window. If the name isn't found: `cmd+space`, `type_text`
  the name, read the top hit with `get_app_state`, then `Return`.
- Switch apps: `cmd+Tab`; windows of the same app: `cmd+grave`. The `window` tool can
  focus, move, resize or tile a window directly.
- Menus are in the **menu bar** (top of the screen), not in the window: read
  the app's state to see its menu items, or use the shortcuts shown there.
- Close window `cmd+w`; quit app `cmd+q` (make sure nothing is unsaved);
  minimise `cmd+m`; full screen `ctrl+cmd+f`.
- Preferences/Settings of an app: `cmd+,`.
- Native (AppKit/SwiftUI) apps expose rich trees; Electron apps need a second
  read; custom-drawn apps expose little — use `screenshot` and
  `get_app_state` with `ocr: true`.
- A new window (sheet, alert, popover) may appear after an action: the result
  says so; read its state.
