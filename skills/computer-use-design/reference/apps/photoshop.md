# Photoshop

Panels, menus and most dialogs are in the tree; the canvas is not. `cmd+`
is Cmd on a Mac, Ctrl elsewhere; `alt` is Option on a Mac.

## Set up

- New document: `cmd+n`. Type Width, Height (Pixels), Resolution (72 for
  screens, 300 for print), Background, then Create.
- Canvas size later: Image ▸ Canvas Size (`cmd+alt+c`). Image size:
  Image ▸ Image Size (`cmd+alt+i`).
- Rulers `cmd+r`. Exact guides: View ▸ Guides ▸ New Guide (orientation +
  position). Many at once: View ▸ Guides ▸ New Guide Layout (columns,
  margins).
- Keep the zoom fixed while drawing (View ▸ 100% or Fit on Screen,
  `cmd+0`). Measure the canvas box on a grid screenshot after each zoom
  change.

## Shapes with exact sizes

1. Rectangle / Ellipse tool: `u` (`shift+u` cycles the shape tools).
2. **Click once** on the canvas, don't drag. A dialog asks for Width and
   Height (and corner radius). Type them, OK.
3. Position: Window ▸ Properties shows the shape's W, H, X, Y. Type X and
   Y from the spec. (Or Free Transform `cmd+t`: X/Y/W/H in the options bar
   at the top, then `Return`.)
4. Fill and stroke: in Properties (or the options bar). Click the fill
   swatch, then the colour picker. Type the hex into its `#` field.

## Colours, fills, selections

- Foreground colour: click the top swatch in the toolbar. In the Color
  Picker, type the hex into `#`, OK. `d` resets to black/white, `x` swaps.
- Exact selection: Rectangular Marquee `m`. In the options bar, set Style
  to Fixed Size, type Width/Height, then click once where the top-left
  should be.
- Fill a selection: Edit ▸ Fill (`shift+F5`), Contents: Foreground Color.
  Deselect: `cmd+d`.

## Text

1. Type tool `t`.
2. Click on the canvas, then `type_text` the exact text.
3. Commit with `cmd+Return`.
4. Font, size, colour: Window ▸ Character (select the text layer first,
   then type values in the fields).
5. Paragraph text: drag a box with `draw` (two points), then type.
   Alignment: options bar.

## Layers

- New layer `cmd+shift+n` (a dialog asks the name). Rename: double-click
  the layer name, type, `Return`. Group `cmd+g`.
- Select a layer by clicking its row in the Layers panel (in the tree).
  Hide/show: its eye. Lock: the lock icon.
- Align: select the layers (`cmd`+click rows), then Layer ▸ Align. Align to
  the canvas: `cmd+a` first, then Layer ▸ Align Layers to Selection.

## Painting and drawing

- Brush `b` (or Pencil, `shift+b` cycles them; the pencil has hard edges).
  Size `[` / `]` or the options bar. Pick the colour first.
- Set Smoothing in the options bar to 0%, or `draw` with `speed` 300–500:
  smoothing makes the line lag behind the pointer and round off corners.
- Strokes: `draw` with `canvas` in canvas pixels. A straight line is two
  points; a smooth freehand line uses `smooth: true`; stars, polygons,
  arcs, Bézier curves, plots and `repeat` patterns work as in any app.
  `preview: true` first.
- `bezier` is a path for the mouse to follow with the brush. It does not
  make Pen-tool anchor points.
- Fill a drawn outline: Paint Bucket (`g`, `shift+g` cycles it with the
  gradient) at the point the `draw` result gives. Or Magic Wand (`w`)
  there, then Edit ▸ Fill.
- Regular polygons and stars as shape layers: the Polygon tool (in the
  `u` group). Click once; its dialog takes the size, the number of sides
  and a Star Ratio.

## References

- File ▸ Place Embedded puts an image file in as a layer (`Return` to
  place).
- Set Opacity (Layers panel field) to 30–50% and lock it to trace over.

## Save and export

- `cmd+s` saves the `.psd`.
- File ▸ Export ▸ Export As (`cmd+alt+shift+w`): PNG or JPG, size, then
  Export and the file dialog. Or File ▸ Save a Copy to choose a format.

## Undo and history

- `cmd+z` steps back (several times).
- Window ▸ History lists the steps.

## Leave alone unless asked

- Generative Fill and other cloud features: they upload the image and may
  cost credits.
- File ▸ Scripts: runs code, so only with the user's OK (security rule 2).
