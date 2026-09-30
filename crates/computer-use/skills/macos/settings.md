# System Settings and system tasks (macOS)

- Open System Settings: `launch_app("System Settings")` (older macOS:
  "System Preferences"). Use its search field (`cmd+f`) and `type_text` the
  setting name.
- Spotlight: `cmd+space`. Control Center lives in the menu bar (top right).
- Privacy & Security ▸ Accessibility / Screen Recording: these grant this agent
  its powers. If actions fail with a permission error, ask the user to enable
  them — don't try to toggle them yourself.
- Display, Sound, Network, Bluetooth, Notifications, Keyboard: sidebar entries
  in System Settings.
- Uninstall an app: drag it to the Trash from `/Applications` (confirm with the
  user first — destructive).
- Keychain, password prompts and the security prompts are blocked on purpose;
  stop and ask the user.
- Force quit dialog: `cmd+alt+Escape` (ending an app is destructive — ask first).
- Mission Control `ctrl+Up`; switch spaces `ctrl+Left/Right`; hide app `cmd+h`.
