# Adobe Photoshop (Windows)

Photoshop draws most of its UI itself: the accessibility tree is partial (menus
and panels are often exposed, the canvas is not). Use menus and `find_element`
for commands; use `screenshot` (and `ocr: true` for panel text) to see the
canvas, and coordinates only for canvas drawing.

- **Work on a copy.** Before changing an existing file, File ▸ Save As a new
  name (`ctrl+shift+s`). Never overwrite the original or flatten/merge unless asked.
- File: new `ctrl+n`; open `ctrl+o`; save `ctrl+s`; Save As `ctrl+shift+s`; export
  as `ctrl+alt+shift+w`; close `ctrl+w`.
- Undo `ctrl+z` (toggles); step back `ctrl+alt+z`; History panel shows steps.
- Tools (single keys with the canvas focused): Move `v`, Marquee `m`, Lasso `l`,
  Quick Selection `w`, Crop `c`, Eyedropper `i`, Brush `b`, Eraser `e`, Type `t`,
  Gradient/Bucket `g`, Hand `h`, Zoom `z`; `d` default colours, `x` swap; `[` `]`
  brush size.
- Layers: new `ctrl+shift+n`; duplicate `ctrl+j`; group `ctrl+g`; merge down
  `ctrl+e` (destructive); layer panel visibility eye toggles — check which layer
  is selected before editing.
- Selection: all `ctrl+a`; deselect `ctrl+d`; invert `ctrl+shift+i`; free
  transform `ctrl+t` (`Return` to commit, `Escape` to cancel).
- Image: size `ctrl+alt+i`; canvas size `ctrl+alt+c`; fit on screen `ctrl+0`; zoom
  `ctrl+plus/minus`. Resizing/cropping is destructive: confirm pixel sizes.
- Text: Type tool `t`, click the canvas, `type_text`, then `ctrl+Return` to commit.
- Filters and Generative Fill may upload the image to Adobe's cloud and need
  sign-in/credits — don't use unless asked.
- Slow operations show a progress bar; `wait_for` it to vanish, don't re-click.
- Dialogs ("Save changes before closing?", "Replace existing?") — read them.
