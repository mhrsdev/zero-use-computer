---
name: computer-use-design
description: >-
  Design work with the computer-use tools: images, posters, logos, icons,
  UI mockups, vector art, 3D models and scenes, floor plans and CAD
  drawings in Photoshop, Paint, GIMP, Krita, Illustrator, Inkscape, Figma,
  Blender, Revit, AutoCAD, SketchUp and similar apps, from a text brief or a
  reference image. Load it with computer-use and computer-use-security.
---

# Design with computer use

Good results come from numbers, not from eyeballing. Before you touch the
app, turn the request into a spec of sizes, positions and colours, and see
it on the design board (2D) or in a scene (3D). Then build it with the most
exact method the app has, and check every step against the spec.

`design`, `scene`, `draw`, `trace_image` and `locate` may not be in your
tool list: `find_tools(category="design")` shows their arguments once, and
`use_tool(name, arguments)` runs them.

## The method

1. **Spec first.** Write a SPEC block in your reply before acting. Use the
   template for the kind of work in [reference/spec.md](reference/spec.md):
   canvas or units, every element back to front with position, size and
   colour, the text, and the output file. If the user gave a reference
   image, measure it first:
   [reference/reference-images.md](reference/reference-images.md). To copy
   a photo or picture in a paint app, trace it ("Copying a photo" below).
   If the brief is vague, choose sensible values, list them in the spec and go on.
   Ask only for what you can't choose yourself (the text of a logo, a brand
   colour).
2. **See it before you build it.** Put the spec on the board: `design`
   for anything flat, `scene` for anything 3D (both below). Look at the
   picture, fix every check it reports, and change numbers until it looks
   right. A fix there is one call; in the app it is undo and redraw.
3. **Set up the document exactly**: size, units and background, typed
   into the app's New / Document Setup / Units dialogs. Never set them by
   dragging.
4. **Build in passes** from the board, back to front:
   1. background;
   2. big shapes;
   3. smaller shapes;
   4. colours;
   5. text;
   6. details.

   One element at a time. Name each layer or object after the spec
   ("bg", "title").
5. **Check after every pass**: [reference/checks.md](reference/checks.md).
   Fix the first difference before going on. Undo (`cmd+z`) rather than
   painting over a mistake.
6. **Save versions**: `name_v1`, `name_v2`, … in the app's own format.
   Never overwrite the user's original file (security rule 3).
7. **Deliver**: export the format and size the spec says, then tell the
   user where the file is and what the spec was.

## The board: `design` (2D) and `scene` (3D)

Both keep what you build under a name: later calls with the same name
change it. Both return a picture, the parts with their exact extents, and
checks. Details and examples: [reference/board.md](reference/board.md).

- **`design`** is a page of layers, like Canva: shapes as `draw` takes
  them (rect, ellipse, polygon, star, arc, bezier, points, curves, repeat)
  with `fill`, `stroke` and `width`, and text. Add, change, mirror (the
  other eye), align, distribute and reorder them. The checks catch shapes
  off the page or nearly centred, pairs that are not quite symmetric, text
  that is hard to read or overlaps. The picture has named cells (A1
  top-left; `cell_size` sets their size); `show: {"cell": "C4"}`
  magnifies one.
- **Many parts that follow a rule** (a chessboard, pixel art, a chart from
  data, a pattern, a dial's ticks): write a `script` that builds the
  design as a page, cell by cell or by maths, instead of listing every
  layer by hand. It is the same design afterwards (see the computer-use
  skill's reference/scripts.md).
- **`scene`** is solids in metres, Z up, the ground at z 0: box, cylinder,
  sphere, cone, torus, plane, each with `size`, `at` (its centre) and
  `rotate`. `on: "seat"` sets an object on top of another; `mirror` and
  `repeat` make legs, wheels and rows. The picture shows the front, right
  and top views to one scale and a perspective view with shadows. The
  checks say what floats (and how far above what), sinks into the ground
  or runs into another part.

Then put it into the app, the most exact way first:

1. **Import a file.** `design` `export: "svg"` (vector apps, Figma) or
   `"png"`; `scene` `export: "obj"` (Blender, most 3D apps). The file is
   temporary: import it at once, then save in the app.
2. **Type the numbers** into the app's fields: each layer's x, y, size and
   hex; each object's Location (`at`), Rotation (`rotate`) and Dimensions
   (`size`).
3. **Paint it step by step** (Paint and other raster apps): the result
   lists the steps, one colour each. Set the colour, then
   `draw(canvas=..., strokes=[{"design": name, "step": n, "fill": w}])`.

## The most exact way first

Use the first of these that the app offers for the step:

1. **Numeric fields**: New-document size; X/Y/W/H in a properties or
   transform panel; hex colour fields; Properties palettes. Click the field,
   select all (`cmd+a`), type the value, then `Return`.
2. **Typed values in a command**:
   - AutoCAD's command line;
   - Blender's `g x 2 Return`;
   - SketchUp's Measurements box;
   - Revit's temporary dimensions.
3. **`draw` with exact shapes**: see "Drawing" below.
4. **The app's script console** (Blender Python, Photoshop scripts, Revit
   Dynamo or pyRevit, AutoCAD LISP). This is the most exact of all, but it
   runs code, so use it only after the user says yes (security rule 2).
5. **Clicks and drags at estimated positions.** Last resort; take a grid
   screenshot first.

## Drawing

`draw` moves the mouse along exact shapes (all its options:
`reference/drawing.md` in the computer-use skill). Use it like this:

1. **Measure the canvas** once, and again after any zoom or scroll. Read
   the document's box `[l, t, r, b]` off `screenshot(app, grid=50)`. Then
   pass
   `canvas={"box": [l, t, r, b], "size": [W, H]}` to `draw` and to
   `screenshot`, and use the spec's own numbers. A plot area takes
   `"range": [x0, x1, y0, y1]` instead of `size` (math, y up).
2. **One spec element, one stroke:**

   | Spec | Stroke |
   |---|---|
   | rect x y w h, radius r | `{"rect": [x, y, w, h, r]}` |
   | ellipse centre (cx, cy), radii rx ry | `{"ellipse": [cx, cy, rx, ry]}` |
   | polygon centre, radius, n sides | `{"polygon": [cx, cy, r, n]}` |
   | star centre, outer, inner, n points | `{"star": [cx, cy, R, r, n]}` |
   | arc, line, path | `arc`, `points`, `bezier` |
   | n copies in a row or round a centre | `repeat` on that stroke |
   | plot of f(x), axes | `{"y": "f(x)"}`, `{"axes": [xstep, ystep]}` |

3. **Preview** with `preview: true`. Nothing is drawn; the red strokes are
   shown over the canvas on named cells, sized so the drawing spans about
   eight of them, with their lines labelled in your units. The result says
   which cells the drawing covers. Compare with the spec, fix the numbers,
   then draw for real.
4. **Solid shapes**: with a brush or pencil, add `"fill": w` (`w` = the
   brush size you set in the app). Paint back to front: each shape covers
   the ones before it, so overlapping shapes come out right. This is the
   safest way in apps without layers (Paint).
5. **Bucket fills** (outlines only): the result says where a click fills
   each closed outline. When other lines cut it into pieces, it gives a
   point per piece; when the outline has a gap, it says where: close it
   first. Never bucket-fill a shape that other outlines cross: use `fill`.
6. **Check** the drawn pixels with `screenshot` (with the same `canvas`):
   `cells=true` names the cells, `cell="C4"` magnifies one with a fine grid
   and its colours, `pick` reads exact colours.

The active tool decides what a stroke does. With a brush or pencil, the
outline is painted. With a shape tool, the app makes its own shape from one
drag, so give two points, corner to corner. Apps that smooth strokes
(Photoshop, Krita, Illustrator's Pencil) need `speed` 300–500 or their
smoothing turned off.

### Copying a photo

Don't draw it by eye: a photo has light, shade and texture that shapes
from memory miss.

1. `trace_image(path=...)`, or `app` + `box` when it is on screen. It
   returns numbered steps of flat colour and a picture of the result.
2. Each step, in order: set the colour to its hex, then
   `draw(canvas=..., strokes=[{"trace": name, "step": n, "fill": w}])`.
3. `screenshot(canvas=..., compare=name)` lists what still differs.

Details: [reference/recipes.md](reference/recipes.md), "Copying a photo".

## Seeing exactly

- `screenshot(app, grid=100)` draws a labelled grid in the same x/y that
  `click`, `drag` and `draw` use. Read positions off it; never guess them.
  With `canvas`, the grid covers the document only and is labelled in its
  units.
- `screenshot(app, pick=[[x,y], ...])` gives the exact colour at points:
  check a fill. `palette=true` lists the main colours: compare with the
  spec.
- `screenshot(app, element_index=i)` zooms in to read small text and
  values.
- With `canvas`, `cells=true` lays graph paper over the document (columns
  A, B…, rows 1, 2…) and `cell="C4"` magnifies one cell. Check a drawing
  cell by cell where it matters: a face, a joint, a corner.
- **Aiming at small things:** `screenshot(app, zoom=[x, y])` magnifies
  around a point with a crosshair; `locate` finds exact places (areas of a
  colour, look-alikes of an icon, the corner or edge near a rough point);
  `click` and `drag` take `snap: "corner"` (or `"edge"`, `"center"`, a hex
  colour) to land on it exactly.
- Drawn pixels are not in the accessibility tree; panels and fields
  usually are. Read values from fields, not from the picture.

## Keys and the pointer

- **Keys follow the mouse.** Some apps send keys to whatever is under the
  mouse: Blender, and parts of CAD apps. Give `press_key` and `type_text`
  an `x`/`y` inside the right area (the 3D viewport, the drawing area).
- **The pointer sets the direction.** Typed lengths in Revit and SketchUp
  go the way the pointer points. Give the `type_text` that types the
  length an `x`/`y` a little way in that direction.
- **One call per command.** The pointer goes back after each call, so run
  a whole modal command in one `press_key`: `"g x 2 Return"`.
- **Keypad keys** are `Numpad0`…`Numpad9` and `NumpadDecimal`.

## When it goes wrong

- Nothing happened: look (screenshot) before trying again. A dialog may
  be open, the wrong tool may be active, or the canvas may not have focus.
- Wrong result: undo, re-read the spec, redo that one step.
- Lost: reopen the last saved version.
- The same step failed twice: use the next method down the list above,
  or ask the user.

## Apps

Read the playbook for the app before starting:

- [Photoshop](reference/apps/photoshop.md);
- [Paint](reference/apps/paint.md);
- [GIMP and Krita](reference/apps/gimp-krita.md);
- [Illustrator and Inkscape](reference/apps/vector.md);
- [Figma](reference/apps/figma.md);
- [Blender](reference/apps/blender.md);
- [Revit](reference/apps/revit.md);
- [AutoCAD and other 2D CAD](reference/apps/cad.md);
- [SketchUp](reference/apps/sketchup.md).

Worked examples, step by step: [reference/recipes.md](reference/recipes.md).
