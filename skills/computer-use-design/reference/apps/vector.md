# Vector apps: Illustrator and Inkscape

Logos, icons, illustrations and print layouts. Vector objects keep exact
numbers, so set every position and size in fields, not by dragging. `cmd+`
is Cmd on a Mac, Ctrl elsewhere.

## Illustrator

- **New document:** `cmd+n`. Type Width, Height and units, then Create.
  Change later: File ▸ Document Setup, or the Artboard tool (`shift+o`)
  and its fields.
- **Exact shapes:**
  1. Rectangle `m` or Ellipse `l`.
  2. **Click once** on the artboard; a dialog asks Width and Height.
- **Position and size:** Window ▸ Transform (`shift+F8`). Type X, Y, W,
  H. Check the reference point square (top-left is easiest to match the
  spec).
- **Colour:**
  - Double-click the Fill swatch in the toolbar, type the hex in `#`.
  - Stroke the same way; stroke width in Window ▸ Stroke.
- **Align:** Window ▸ Align (`shift+F7`). Align To: Artboard to centre on
  the page.
- **Text:**
  1. Type tool `t`.
  2. Click and `type_text`.
  3. `Escape` to finish.
  4. Font and size in Window ▸ Type ▸ Character (`cmd+t`).
  5. Outline text for logos: Type ▸ Create Outlines (`cmd+shift+o`), after
     the text is final.
- **Stars and polygons:** the Star and Polygon tools (in the Rectangle
  group). Click once; the dialog takes the radii and the number of points
  or sides.
- **Paths:** Pen `p`. Clicks make corners. Alternatively `draw` a curve or
  plot with the Pencil `n`: set its Fidelity to Accurate (double-click the
  tool) and `draw` at `speed` 300–500. The result is a path you can
  style.
- **Combine shapes:** Window ▸ Pathfinder (Unite, Minus Front).
- **Save and export:**
  - `cmd+s` saves the `.ai`.
  - File ▸ Export ▸ Export As (PNG, SVG), or Export for Screens
    (`cmd+alt+e`).
  - File ▸ Save As for PDF or SVG.

## Inkscape

- **Page size:** File ▸ Document Properties (`cmd+shift+d`). Type width,
  height and units.
- **Shapes:**
  1. Rectangle `r`, Ellipse `e`, Star/Polygon `*`.
  2. Drag with `draw` (two points).
  3. Type exact values in the tool controls bar: W, H (rectangle) or Rx,
     Ry (ellipse).
- **Position and size:** the X, Y, W, H fields in the toolbar of the
  Select tool (`s`), with the unit chosen beside them.
- **Most exact of all:** Edit ▸ XML Editor (`cmd+shift+x`). Select the
  object and edit its attributes directly: `x`, `y`, `width`, `height`,
  `rx`, `cx`, `cy`, `r`, and `style` (`fill:#1e88e5;stroke:none`).
- **Colour:** Object ▸ Fill and Stroke (`cmd+shift+f`). Type the RGBA hex
  (`1e88e5ff`).
- **Align:** Object ▸ Align and Distribute (`cmd+shift+a`), relative to
  Page.
- **Text:**
  1. Text tool `t`.
  2. Click and `type_text`.
  3. `Escape` to finish.
  4. Font and size in the tool controls.
- **Star/Polygon `*`:** in the tool controls choose star or polygon and
  type Corners and Spoke ratio, then drag with `draw` (two points, from
  the centre out).
- **Curves and plots:** Pencil `p` with `draw`, at `speed` 300–500, makes
  a path. Extensions ▸ Render ▸ Function Plotter draws y = f(x) as an exact
  path in a selected rectangle.
- **Combine:** Path ▸ Union / Difference. Text to paths: Path ▸ Object to
  Path.
- **Save and export:**
  - `cmd+s` saves Inkscape SVG.
  - File ▸ Export (`cmd+shift+e`) writes PNG at a chosen size.
