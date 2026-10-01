# Microsoft Paint (Windows)

The toolbar, menus and dialogs are in the tree; the canvas is not. Simple
and predictable: good for flat shapes, icons and quick sketches.

## Set up

- Canvas size: File ▸ Image properties (`ctrl+e`). Choose Pixels, then
  type Width and Height, OK.
- Scale an existing picture: Resize and skew (`ctrl+w`), by Pixels with
  "Maintain aspect ratio" as needed.
- Keep the zoom at 100% (`ctrl+0` or the zoom control in the status bar).
  The status bar shows the canvas size and the pointer position in canvas
  pixels.
- Measure the canvas box once on a `screenshot(app, grid=50)`. Then draw
  with `canvas={"box": [l, t, r, b], "size": [W, H]}` and canvas pixels.

## Colours

- Colour 1 (primary) and colour 2 (secondary) are in the toolbar. Select
  one, then Edit colors (the multicolour button). Type the Hex value, OK.
- Shapes: outline uses colour 1, fill uses colour 2. Set Fill to "Solid
  color" in the shape options; Outline to "No outline" if needed.

## Shapes

Rectangles, ovals and lines: use Paint's shape tools.

1. Shapes ▸ Rectangle / Oval / Line / … Set the Size (outline width).
2. Drag from the top-left corner to the bottom-right corner of the box:
   `draw` with one stroke of two points,
   `{"points": [[x, y], [x + w, y + h]]}` in canvas pixels.
3. While the shape still has its handles, its outline/fill options can
   change. Clicking elsewhere fixes it in place.

Stars, regular polygons, arcs, curves, plots and patterns: Paint's own
star and polygon shapes are stretched to the drag, not regular. Draw the
outline instead:

1. Pencil (1 px, crisp edges for fills) or a brush, and colour 1.
2. `draw` with `canvas` and the stroke: `{"star": [...]}`,
   `{"polygon": [...]}`, `repeat` for many, `"range"` for a plot.
   `preview: true` first.
3. Fill each with the bucket at the point the result gives.

Paint draws exactly where the pointer goes: the default `speed` is fine.

## Fill, text, brushes

- Fill (bucket): click inside a closed area; it uses colour 1. After a
  `draw`, click the point the result gives for that stroke. Check with
  `pick` afterwards. A fill that spreads too far means a gap in the
  outline: undo and redraw it closed.
- Text (A):
  1. Drag a text box with `draw` (two points).
  2. Set font and size in the text toolbar.
  3. `type_text`.
  4. Click outside the box to finish.
- Brushes: choose one and a size, then `draw` strokes (`smooth: true` for
  freehand curves).

## Layers and background

- Newer Paint versions have Layers (toolbar button) and Remove background.
  Use them if present; otherwise draw back to front.

## Save

- `ctrl+s`. Choose PNG in "Save as type" for drawings (JPEG blurs flat
  colours).
- Undo: `ctrl+z`.
