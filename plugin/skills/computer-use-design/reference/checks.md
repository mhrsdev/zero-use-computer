# Checking the work

Check after every pass, not only at the end. A check is a list of yes/no
questions answered from a screenshot, `pick` colours and the app's own
fields. Never answer from memory of what you meant to do.

## On the board, before the app

- `design`: the result says "Checks: nothing off", or each check left is
  one you meant (a shape that bleeds off the page on purpose).
- `scene`: nothing floats or sinks, and parts run into each other only
  where they are joined. The front, right and top views show the
  proportions of the spec.
- The picture looks like what was asked for. If it doesn't, change the
  numbers there: it costs one call.

## Before each `draw`

Call it with `preview: true` first. Nothing is painted; you get the red
strokes over the canvas on named cells (lines labelled in your units) and
green dots where each stroke starts. Check:

- every stroke inside the canvas, where the spec puts it, at its size;
- the count: one stroke per spec element (repeats included);
- the cells it covers (the result names them) are where the board's
  picture has it;
- round things round. If the result says 1 unit of x and y differ on
  screen, fix the `range` or `size` first.

## After every pass

1. `screenshot(app, grid=100)`: is everything from this pass there, and
   nothing else? Count the elements. With the `canvas` you drew with
   (`screenshot(app, canvas=..., grid=true)`) the grid reads in the spec's
   own units.
2. Positions and sizes: select the element and read X/Y/W/H in the app's
   fields, or read them off the grid. Within 1–2 px of the spec (or 1% for
   3D and CAD)?
3. Colours: `pick` at the centre of each new element (for a drawn shape,
   the inside point the `draw` result gave). Same hex as the spec? (Within
   a few units is fine for anti-aliased edges, not for fills.)
   After painting a traced picture: `screenshot(app, canvas=...,
   compare=name)`.
4. Fine parts: `screenshot(app, canvas=..., cell="C4")` for each cell
   where something must be exact (an eye, a joint, a corner where lines
   meet). Lines meet, nothing spills over, the colours are right.
5. Text: zoom in. Every character right? Font, size, colour and alignment
   as in the spec? Not cut off?
6. Layers or objects: named, in the right order, nothing left selected
   that the next step could change by accident.
7. Save (`cmd+s`). Every few passes, save as the next version.
8. 3D: the app's front, right and top views against the scene's views;
   the sizes in its fields against the scene's listing.

## Before delivering

- The whole spec, element by element: tick each one.
- Margins equal and at least the spec's. Centred things really centred:
  left gap = right gap.
- Nothing outside the canvas by accident, no stray strokes or empty
  layers, no visible guides or reference layer in the export.
- Export: the format, size and file name in the spec. Open the exported
  file, or check its size and dimensions in the app, if you can.
- Tell the user:
  - where the file is;
  - the spec you followed;
  - anything you changed from the brief, and why.

## Common mistakes and fixes

| You see | Likely cause | Fix |
|---|---|---|
| Nothing drawn | Wrong tool, layer locked or hidden, canvas not focused | Pick the tool again, select an unlocked visible layer, click the canvas once |
| Drawn in the wrong place | Coordinates from an old screenshot, or zoom changed | New grid screenshot, measure the canvas box again |
| Shape filled with the wrong colour | Foreground/background (colour 1/2) swapped | Set the colour in the shape's own fill field |
| Text typed into the wrong place | The text tool wasn't active, or a field had focus | Undo, select the text tool, click inside the canvas |
| A shortcut did nothing | Focus in a panel or field; in Blender the mouse wasn't over the viewport | Click an empty part of the canvas; give press_key x/y in the viewport |
| A dialog appeared | The step needs confirmation or a value | Read it; fill it from the spec |
| One big box or oval instead of an outline | A shape tool was active, so the app made its own shape from the drag | Undo; pick the brush or pencil, or give the shape tool a two-point stroke |
| Rounded corners or wobbly lines | The app smooths strokes, or `speed` too high | Undo; `speed` 300–500, or turn the brush's smoothing off |
| Circles came out as ellipses | `range` or `size` not in the box's proportions | Fix the canvas numbers; the `draw` result warns about this |
| The fill spilled outside the shape | A gap in the outline (not `closed`, or a stroke cut at the canvas edge); the `draw` result names where | Undo; close the outline there (or draw the shape again with `closed: true` inside the canvas), then fill |
| A fill covered only part of a shape | Other outlines cross it and cut it into pieces | Click each point the `draw` result gave, or undo and paint it with `fill`, back to front |
| A copied photo looks like a different picture | Drawn by eye | Use `trace_image` and its steps; `screenshot(compare=...)` shows what still differs |
| Small details vanished in a traced picture | The brush is wider than them (the `draw` result says so) | Draw that step again with a smaller brush and `fill` |
| A click lands next to a small target | Estimated from the picture | `screenshot(app, zoom=[x, y])`, or `locate`, or `click` with `snap` |
| A 3D part floats or sits inside another | `at` is the centre, not the bottom | Plan it in `scene`; `on: "<part>"` sets it on top |
| A cylinder lies the wrong way | Cylinders and cones stand along z until rotated | `rotate` [90, 0, 0] lays it along y, [0, 90, 0] along x |
| An imported OBJ is on its side | Import axes changed | Import again with forward -Z, up Y (the defaults) |
