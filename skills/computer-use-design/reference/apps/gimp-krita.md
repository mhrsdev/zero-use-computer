# GIMP and Krita

Free raster editors on all three systems. Menus, dialogs and tool options
are in the tree; the canvas is not. `cmd+` is Cmd on a Mac, Ctrl elsewhere.

## GIMP

- **New image:** `cmd+n`. Type Width and Height (px); under Advanced
  Options, set Fill with.
- **Canvas size:** Image ▸ Canvas Size. **Image size:** Image ▸ Scale Image.
- **Exact areas:**
  1. Rectangle Select `r` or Ellipse Select `e`.
  2. Draw roughly with `draw` (two points).
  3. In Tool Options, type Position (x, y) and Size (w, h).
- **Fill:** Edit ▸ Fill with FG Color (`cmd+,`) or BG Color (`cmd+.`). Set
  the colours by clicking the FG/BG swatch and typing the HTML notation
  (hex) field.
- **Deselect:** `cmd+shift+a`.
- **Text:**
  1. Text tool `t`.
  2. Click the canvas and `type_text`.
  3. Set font, size and colour in Tool Options.
  4. Move it with the Move tool, or type its offsets in Layer ▸ Layer
     Attributes.
- **Layers:**
  - New `cmd+shift+n`, with a name.
  - Exact position: Layer ▸ Layer Attributes (Offset X/Y).
  - Order: Layer ▸ Stack.
- **Guides:** Image ▸ Guides ▸ New Guide (by Percent / New Guide).
- **Drawn outlines:**
  1. Pencil `n` (hard edges) or Paintbrush `p`.
  2. `draw` the shape, plot or pattern with `canvas`, after a preview.
  3. Bucket Fill `shift+b` at the point the `draw` result gives.

  Turn off Smooth stroke in Tool Options if it is on.
- **Save and export:**
  - `cmd+s` saves the `.xcf`.
  - File ▸ Export As (`cmd+shift+e`) writes PNG or JPG.
  - File ▸ Overwrite replaces the original: don't, unless asked.

## Krita

- **New document:** `cmd+n`. Type Width and Height, then Create.
- **Shapes:**
  - Rectangle, Ellipse and Polygon tools.
  - Drag with `draw` (two points for rectangles and ellipses).
  - Tool Options has the fill and outline.
- **Exact transforms:** Transform tool `cmd+t`. Its Tool Options have X/Y,
  width/height and rotation fields.
- **Colours:** Advanced Color Selector or Settings ▸ Dockers ▸ Palette.
  The colour dialog has a hex field.
- **Brushes:**
  - Pick a preset and size.
  - Set Brush Smoothing in Tool Options to None, or `draw` with `speed`
    300–500.
  - `draw` with `smooth: true` for freehand strokes; stars, polygons,
    curves, plots and `repeat` patterns as in any app.
  - For straight lines, a stroke of two points.
- **Fill:** Fill tool `f`, at the point the `draw` result gives.
- **Text:** Text tool. Drag a box, then type in the text editor dialog.
  Save closes it.
- **Save and export:**
  - `cmd+s` saves the `.kra`.
  - File ▸ Export writes PNG or JPG.
