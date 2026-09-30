# Settings and system tasks (Linux)

- GNOME: `launch_app("gnome-control-center")` (Settings); search with `ctrl+f`.
  KDE: `systemsettings`. Xfce: `xfce4-settings-manager`.
- Activities / app search (GNOME): press `super`, `type_text` the name, read the
  first result with `get_app_state`, then `Return`.
- Quick settings (GNOME): top-right system menu — Wi-Fi, volume, brightness.
- Display scale / resolution: Settings ▸ Displays. Default apps: Settings ▸
  Default Applications. Keyboard shortcuts: Settings ▸ Keyboard.
- Installing software (Software / Discover / Synaptic) asks for a password:
  polkit and credential prompts are blocked on purpose — stop and ask the user.
- Uninstalling is destructive: confirm with the user first.
- Wayland vs X11: under Wayland some window-management and global-key features
  are restricted; if an action fails with "not supported", say so and try the
  app's own UI instead.
- Lock `super+l` (don't); switch workspace `ctrl+alt+Left/Right`.
