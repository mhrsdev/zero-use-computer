# Image editors

Photoshop (Windows, macOS), GIMP and Krita (all; Photoshop has no Linux
version). Menus, panels and dialogs are in the tree; the canvas isn't. Use
menus and `find_element` for commands, `screenshot` to see the canvas, and
x/y only to draw on it. `cmd+` below is Cmd on a Mac, Ctrl elsewhere.

- **Work on a copy**: Save As / Export As a new name first (GIMP: File ▸
  Export As; its File ▸ Overwrite replaces the original). Flattening, merging
  layers, cropping and resizing lose data: confirm sizes before.
- Undo `cmd+z` (Photoshop steps back with `cmd+alt+z`); the History panel
  shows the steps.
- Check which layer is selected before editing. New layer `cmd+shift+n`;
  select all `cmd+a`; deselect Photoshop `cmd+d`, GIMP `cmd+shift+a`.
- Tool keys need the canvas focused: Move (`v` Photoshop, `m` GIMP), Brush
  (`b` / `p`), Eraser (`e` / `shift+e`), Text `t`, Zoom `z`; `[` `]` brush
  size; `d` default colours, `x` swap them.
- Text: Text tool, click the canvas, `type_text`, commit (Photoshop
  `cmd+Return`).
- Slow filters show a progress bar: the filter dialog closes as soon as OK
  is pressed and the filter goes on running (GIMP's status bar, Photoshop's
  Progress window). Look again (`screenshot` or `get_app_state`) until the
  bar is gone before the next step; with a decision model,
  `wait_for(until="Has the filter finished?")`. Don't press OK again.
- Cloud features (Generative Fill, online filters) upload the image and may
  need sign-in or credits: only when asked.
