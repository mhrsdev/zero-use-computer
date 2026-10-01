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
app, turn the request into a spec of sizes, positions and colours. Then
build it with the most exact method the app has, and check every step
against the spec.

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
2. **Set up the document exactly**: size, units and background, typed
   into the app's New / Document Setup / Units dialogs. Never set them by
   dragging.
3. **Build in passes**, back to front:
   1. background;
   2. big shapes;
   3. smaller shapes;
   4. colours;
   5. text;
   6. details.

   One element at a time. Name each layer or object after the spec
   ("bg", "title").
4. **Check after every pass**: [reference/checks.md](reference/checks.md).
   Fix the first difference before going on. Undo (`cmd+z`) rather than
   painting over a mistake.
5. **Save versions**: `name_v1`, `name_v2`, … in the app's own format.
   Never overwrite the user's original file (security rule 3).
6. **Deliver**: export the format and size the spec says, then tell the
   user where the file is and what the spec was.

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
   shown over the canvas with a grid in your units. Compare with the spec,
   fix the numbers, then draw for real.
4. **Solid shapes**: with a brush or pencil, add `"fill": w` (`w` = the
   brush size you set in the app). Paint back to front: each shape covers
   the ones before it, so overlapping shapes come out right. This is the
   safest way in apps without layers (Paint).
5. **Bucket fills** (outlines only): the result says where a click fills
   each closed outline. When other lines cut it into pieces, it gives a
   point per piece; when the outline has a gap, it says where: close it
   first. Never bucket-fill a shape that other outlines cross: use `fill`.
6. **Check** the drawn pixels with `screenshot` (with the same `canvas`,
   `grid=true` and `pick`).

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
