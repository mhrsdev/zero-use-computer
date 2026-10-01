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
   gave for its stroke (one per piece if it says the shape is cut). Check
   each with `pick`. Shapes that overlap others: paint them with `fill`
   instead, back to front.
5. Text box last. Save as PNG.

## A badge or emblem (Paint, Photoshop, GIMP, Krita)

Spec example, 1000 x 1150 canvas: a ring, a disc with a star in it, 12 dots
round it, a title under it.

1. Put it on the board: the `design` call in [board.md](board.md) is this
   badge. Look at the picture and clear its checks: change the numbers,
   not the app.
2. New document 1000 x 1150. Fill it with the background colour. Measure
   the canvas box on a grid screenshot.
3. Paint the steps the `design` result lists, in order. For each: set the
   colour, then
   `draw(canvas={"box": [...], "size": [1000, 1150]}, strokes=[{"design":
   "badge", "step": n, "fill": 10}], preview=true)`. Check the preview,
   then draw it without `preview`. A lines step uses a brush as wide as
   the step says, and no `fill`.
4. Type the title with the text tool where its layer says.
5. `screenshot(app, canvas=..., cells=true)`: compare cell by cell with
   the design's picture; `cell="E2"` looks closely at the top dot and the
   ring.

In Photoshop or GIMP you can instead `export: "png"` (or `"svg"` for
Illustrator, Inkscape and Figma) and place the file as a layer.

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

## A 3D model (Blender, SketchUp, any 3D app)

Example: a stool, as in [board.md](board.md).

1. Spec: the parts as solids with sizes in metres, their centres, their
   rotations and colours.
2. Build it in `scene` first. Look at the four views: legs where they
   belong, proportions right. Clear every check: nothing floats or sinks,
   and parts only run into each other where they are joined.
3. Into the app, one of:
   - `export: "obj"` and import it at once (Blender: File > Import >
     Wavefront (.obj)); then save the app's own file;
   - by numbers: per object, the primitive, then Location, Rotation and
     Dimensions from the scene's listing
     ([apps/blender.md](apps/blender.md), "From a scene").
4. Check the app's front, right and top views (Blender `Numpad1`,
   `Numpad3`, `Numpad7`) against the scene's.
5. Materials: one per colour, Base Color by hex. Camera and light from the
   spec; `Numpad0` looks through the camera.
6. `F12` to render, save the image, `cmd+s` for the `.blend`.

## Copying a design (logo, poster, UI)

Flat designs with exact shapes and text:

1. Open the reference on screen. Measure it as in
   [reference-images.md](reference-images.md): its box, every element's box,
   `palette` and `pick` colours, the text.
2. Write the spec from those numbers, converted to the canvas size.
3. Either build from the spec (preferred) or trace over it at low opacity.
4. Compare side by side at the end; fix the largest difference first.

## Copying a photo (Paint, Photoshop, GIMP, Krita)

A photo has light, shade and texture: shapes drawn by eye come out as a
generic cartoon. Let `trace_image` turn it into flat colours instead.
Example: a photo of a cat, 960 x 960, copied into Paint.

1. Canvas with the photo's proportions: 800 x 800. Zoom to fit and read
   the canvas box off a grid screenshot.
2. `trace_image(path="C:/Users/me/Pictures/cat.jpg", detail="high")`.
   Look at the picture it returns:
   - a feature you need is missing: trace again with more `colors` (up to
     16);
   - too busy: fewer colours, or `detail: "medium"`.
3. Brush: a round brush or the pencil, about 1% of the canvas (8 px on
   800), smaller for small features. In Paint use `speed` 2500.
4. Step 1 is the background ("all of it"): set colour 1 to its hex and
   click the empty canvas with the bucket.
5. Each next step, in order:
   1. Set colour 1 to the step's hex.
   2. `draw(app, canvas={"box": [...], "size": [800, 800]},
      strokes=[{"trace": "cat", "step": n, "fill": 8}], speed=2500)`.
6. `screenshot(app, canvas=..., compare="cat")`. Each place it lists names
   the colour it should be: a step was missed, or a colour is wrong. Redo
   that step.
7. Save as PNG.

In Photoshop, GIMP and Krita, turn brush smoothing off and use `speed`
300–500, or 1000+ if the app keeps up. Vector apps have their own tracing,
which gives cleaner paths: Illustrator's Object ▸ Image Trace, Inkscape's
Path ▸ Trace Bitmap.
