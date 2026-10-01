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
2. Build the symbol from exact shapes (circles, rectangles, polygons with
   `steps`). Combine with Pathfinder / Path ▸ Union and Difference.
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
3. Shapes, back to front: each is one `draw` with `canvas` and two points
   (corner to corner) or a `rect`.
4. Fill closed areas with the bucket. Check each with `pick`.
5. Text box last. Save as PNG.

## A plotted graph on any canvas

1. Spec:
   - plot box (left, top, width, height);
   - x range a to b, y range c to d;
   - the function.
2. Axes: two strokes of `points`.
3. Curve: one `draw` stroke.
   - `x "left + (t - a) / (b - a) * width"`
   - `y "top + height - (f(t) - c) / (d - c) * height"`
   - `t [a, b]`

   Pass `canvas` with the document box so the numbers are canvas pixels.
4. Tick marks: short `points` strokes. Labels: text tool.

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
