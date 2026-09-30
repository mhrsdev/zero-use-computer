# Adobe Photoshop (macOS)

Photoshop draws most of its UI itself: the accessibility tree is partial (the
menu bar and many panels are exposed; the canvas is not). Use menus and
`find_element` for commands; `screenshot` (and `ocr: true`) to see the canvas;
coordinates only for canvas drawing.

- **Work on a copy.** Before changing an existing file, Save As a new name
  (`cmd+shift+s`). Never overwrite the original or flatten/merge unless asked.
- File: new `cmd+n`; open `cmd+o`; save `cmd+s`; Save As `cmd+shift+s`; export as
  `cmd+alt+shift+w`; close `cmd+w`.
- Undo `cmd+z` (toggles); step back `cmd+alt+z`; History panel shows steps.
- Tools (single keys with the canvas focused): Move `v`, Marquee `m`, Lasso `l`,
  Quick Selection `w`, Crop `c`, Eyedropper `i`, Brush `b`, Eraser `e`, Type `t`,
  Gradient/Bucket `g`, Hand `h`, Zoom `z`; `d` default colours, `x` swap; `[` `]`
  brush size.
- Layers: new `cmd+shift+n`; duplicate `cmd+j`; group `cmd+g`; merge down `cmd+e`
  (destructive); check which layer is selected before editing.
- Selection: all `cmd+a`; deselect `cmd+d`; invert `cmd+shift+i`; free transform
  `cmd+t` (`Return` commit, `Escape` cancel).
- Image: size `cmd+alt+i`; canvas size `cmd+alt+c`; fit on screen `cmd+0`; zoom
  `cmd+plus/minus`. Resizing/cropping is destructive: confirm pixel sizes.
- Text: Type tool `t`, click the canvas, `type_text`, then `cmd+Return` to commit.
- Filters and Generative Fill may upload the image to Adobe's cloud and need
  sign-in/credits — don't use unless asked.
- Wait for progress bars with `wait_for`; don't re-click.
- Dialogs ("Save changes before closing?", "Replace existing?") — read them.
