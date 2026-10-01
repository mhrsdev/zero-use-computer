# Checking the work

Check after every pass, not only at the end. A check is a list of yes/no
questions answered from a screenshot, `pick` colours and the app's own
fields. Never answer from memory of what you meant to do.

## After every pass

1. `screenshot(app, grid=100)`: is everything from this pass there, and
   nothing else? Count the elements.
2. Positions and sizes: select the element and read X/Y/W/H in the app's
   fields, or read them off the grid. Within 1–2 px of the spec (or 1% for
   3D and CAD)?
3. Colours: `pick` at the centre of each new element. Same hex as the
   spec? (Within a few units is fine for anti-aliased edges, not for
   fills.)
4. Text: zoom in. Every character right? Font, size, colour and alignment
   as in the spec? Not cut off?
5. Layers or objects: named, in the right order, nothing left selected
   that the next step could change by accident.
6. Save (`cmd+s`). Every few passes, save as the next version.

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
