# Recipes

Each recipe is the method of the computer-use-design skill applied to one
job.
Follow the steps in order; after each numbered step, run the checks in
[checks.md](checks.md).

## A poster or social post (Photoshop, GIMP, Krita, Figma)

1. Write the spec (see the example in [spec.md](spec.md)): canvas, palette,
   elements back to front, text.
2. New document at the spec size. Fill the background colour.
3. For each shape in the spec:
   1. Create it at its exact size (shape tool click dialog, or fields).
   2. Type X and Y.
   3. Set its fill hex.
   4. Name the layer.
4. For each text:
   1. Click with the text tool and type it.
   2. Set font, size and colour in the fields.
   3. Position it: centred text uses the canvas centre.
5. `screenshot(app, grid=100, pick=[...])`: check positions and colours.
6. Save the native file, export PNG, report.

## A logo (Illustrator, Inkscape)

1. Spec:
   - artboard 1000 x 1000;
   - 2 colours + black/white;
   - a symbol built from 2–4 simple shapes;
   - the name in one font.
2. Build the symbol from exact shapes typed into the shape tools' dialogs
   and fields (ellipses, rectangles, polygons, stars). Combine with
   Pathfinder / Path ▸ Union and Difference. Curves that no tool makes:
   the Pencil and a `draw` stroke (`bezier`, or `x`/`y` in t).
3. Type the name. Set its size so its width is about 1.5–2× the symbol's.
   Align centres.
4. Check:
   - small: zoom out to about 64 px wide. Is it still readable?
   - one colour: does it still work in black only?
5. Outline the text, export SVG + PNG (transparent background).

## A simple drawing in Paint

1. Canvas size via Image properties. Measure the canvas box on a grid
   screenshot.
2. Set colour 1 (outline) and colour 2 (fill) by hex.
3. Shapes, back to front:
   - rectangles and ovals: the shape tool and a two-point `draw`, corner
     to corner;
   - stars, regular polygons, arcs, curves: the pencil or a brush and the
     matching stroke (`star`, `polygon`, `arc`, …).

   `preview: true` first, with the same `canvas`.
4. Fill each closed outline with the bucket at the point the `draw` result
   gave for its stroke. Check each with `pick`.
5. Text box last. Save as PNG.

## A badge or emblem (Paint, Photoshop, GIMP, Krita)

Spec example, 1000 x 1000 canvas: a ring, a star in it, 12 dots round it.

1. Pencil or hard brush, 6 px, colour `#1E3A5F`. Then one `draw` with
   `canvas={"box": [...], "size": [1000, 1000]}` and `preview: true`:
   - `{"ellipse": [500, 500, 400, 400]}` and
     `{"ellipse": [500, 500, 340, 340]}` (the ring);
   - `{"star": [500, 500, 260, 110, 5]}`;
   - `{"ellipse": [500, 60, 14, 14], "repeat": {"count": 12, "rotate": 30, "about": [500, 500]}}`.
2. Check the preview, then draw without it.
3. Bucket fill at the points the result gave, each in its spec colour:
   stroke 1 is the ring (its point is between the circles), stroke 2 the
   disc round the star, stroke 3 the star.
4. `screenshot(app, canvas=..., grid=true, pick=[[500, 500], [500, 130]])`:
   the star's centre and the ring.

## A radial pattern (mandala, flower, clock face)

1. Spec: centre (cx, cy), the number of copies n, and one element drawn
   pointing up from the centre.
2. One stroke with `"repeat": {"count": n, "rotate": 360 / n, "about":
   [cx, cy]}`. Write the angle as a number (30, not 360/12).
3. Layers of the pattern are more strokes with their own repeat: petals,
   then dots, then a ring.
4. Clock face: ticks are `{"points": [[cx, cy - R], [cx, cy - R + 20]],
   "repeat": {"count": 12, "rotate": 30, "about": [cx, cy]}}`.

## A plotted graph on any canvas

1. Spec: see "Graphs and function plots" in [spec.md](spec.md): plot box
   in screenshot pixels, x and y range, tick steps, the curves.
2. One `draw` with `canvas={"box": [l, t, r, b], "range": [x0, x1, y0,
   y1]}` and `preview: true`:
   - `{"axes": [xstep, ystep]}`;
   - `{"y": "f(x)"}` per function (over the whole x range), or `x`, `y`
     and `t` for a parametric curve.
3. Check the preview, then draw. Use a different colour per curve: one
   `draw` call per colour.
4. Labels: the tick at x = v is at screenshot x
   `l + (v - x0) / (x1 - x0) * (r - l)`; the tick at y = v at screenshot y
   `b - (v - y0) / (y1 - y0) * (b - t)`. Click just beside it with the
   text tool and type the number.
5. Check with `screenshot(app, canvas=<the same>, grid=true)`: the grid is
   labelled in the plot's numbers, so each curve can be read against it.

## A one-room floor plan (Revit, AutoCAD, SketchUp)

1. Spec: corners in order (for example a 5000 x 4000 mm room starting at
   0,0), wall thickness, height, door and window positions.
2. Walls:
   - Revit: `WA`, then click and type each length (5000, 4000, 5000,
     4000).
   - AutoCAD: `RECTANG\n0,0\n@5000,4000\n` and `OFFSET` for the thickness.
   - SketchUp: `r`, `5000,4000`, then `p` and the height.
3. Door and window: place, then set the offset from the corner exactly
   (temporary dimension, `MOVE` with `@dx,0`, or Move and a typed
   distance).
4. Dimensions on all four sides; check they read the spec's numbers.
5. Save, export PDF.

## A small 3D scene in Blender

1. Spec: objects with sizes and locations in metres, materials with hex
   colours, camera, one light, render size.
2. Delete the default cube (`Delete` with the pointer over the
   viewport); `shift+c` to centre the cursor.
3. For each object:
   1. `F3` "Add Cube" (or Cylinder, …).
   2. Set Dimensions and Location in the sidebar (`n`).
   3. Apply scale (`ctrl+a` ▸ Scale).
   4. Rename it (`F2`, type the name, `Return`).
4. Materials: one per colour, set Base Color by hex.
5. Camera and light from the spec. `Numpad0` to look through the camera,
   then check the framing with a screenshot.
6. `F12` to render, save the image, `cmd+s` for the `.blend`.

## Copying a reference image

1. Open the reference on screen. Measure it as in
   [reference-images.md](reference-images.md): its box, every element's box,
   `palette` and `pick` colours, the text.
2. Write the spec from those numbers, converted to the canvas size.
3. Either build from the spec (preferred) or trace over it at low opacity.
4. Compare side by side at the end; fix the largest difference first.
