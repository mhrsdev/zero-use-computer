# Image editing on Linux (Photoshop isn't available — GIMP, Krita, Photopea)

Adobe Photoshop has no Linux version. Use **GIMP** (`launch_app("gimp")`), Krita
(`krita`), or Photopea (a Photoshop-like editor in a browser; see the `browser`
skills). GIMP is described here.

GTK UI is mostly exposed through AT-SPI (menus, dialogs, tool options); the
canvas is not — use `screenshot` (and `get_app_state(ocr: true)`) to see it.

- **Work on a copy.** Before editing an existing image, export to a new name
  (File ▸ Export As, `ctrl+shift+e`). Never overwrite the original
  (`ctrl+e` = Overwrite) unless asked.
- File: new `ctrl+n`; open `ctrl+o`; save project (.xcf) `ctrl+s`; close `ctrl+w`.
- Undo `ctrl+z`; redo `ctrl+y`; Undo History: Edit ▸ Undo History.
- Tools (canvas focused): Move `m`, Rectangle select `r`, Free select `f`, Fuzzy
  select `u`, Crop `shift+c`, Paintbrush `p`, Eraser `shift+e`, Text `t`, Bucket
  fill `shift+b`, Color picker `o`, Zoom `z`; `d` default colours, `x` swap;
  `[` `]` brush size.
- Layers: new `ctrl+shift+n`; duplicate `ctrl+shift+d`; merge down: Layer ▸
  Merge Down (destructive); check the active layer before editing.
- Selection: all `ctrl+a`; none `ctrl+shift+a`; invert `ctrl+i`.
- Image: Image ▸ Scale Image, Canvas Size; fit in window `shift+ctrl+j`; zoom
  `+` / `-`. Resizing/cropping is destructive: confirm pixel sizes.
- Text: Text tool `t`, click the canvas, `type_text`.
- Wait for progress bars with `wait_for`; don't re-click.
- Dialogs ("Discard changes?", "Overwrite?") — read them.
