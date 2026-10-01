# AutoCAD and other 2D CAD (LibreCAD, QCAD, BricsCAD, ZWCAD)

CAD is the easiest place to be exact: every command takes typed
coordinates on the command line. With the drawing area focused, typing
goes to the command line. Use `type_text` with `\n` for Enter, and give
`x`/`y` inside the drawing area if keys seem to go nowhere.

## Coordinates

- `x,y`: absolute, from the origin.
- `@dx,dy`: relative to the last point.
- `@length<angle`: polar. Angles are in degrees, counter-clockwise from
  +X.
- If Dynamic Input is on (`F12` toggles it), typed values near the cursor
  are relative by default. Start with `#` for absolute (`#0,0`), or turn
  Dynamic Input off and use the command line as above.

## Commands (type the name, then `\n`)

| Command | Typed input |
|---|---|
| Line | `LINE\n0,0\n@5000,0\n@0,4000\n@-5000,0\nC\n` (C closes) |
| Rectangle | `RECTANG\n0,0\n@5000,4000\n` |
| Circle | `CIRCLE\n2500,2000\n600\n` (centre, then radius) |
| Arc | `ARC\n` then three points |
| Polyline | `PLINE\n` then points, `C\n` to close |
| Offset | `OFFSET\n200\n`, click the object, click the side |
| Move / Copy | `MOVE\n`, select, `\n`, base point, `@dx,dy\n` |
| Rotate | `ROTATE\n`, select, `\n`, base point, angle |
| Trim | `TRIM\n`, then click the parts to cut |
| Fillet | `FILLET\nR\n100\n`, then click two lines |
| Dimension | `DIMLINEAR\n`, two points, then where the line goes |
| Text | `TEXT\n`, start point, height, angle, then the text and `\n` |
| Layers | `LAYER\n` (dialog), or `-LAYER\n` for typed options |
| See everything | `ZOOM\nE\n` |
| Units | `UNITS\n` |

`Escape` cancels a command. Space also acts as Enter, except inside text.

## Workflow

1. `UNITS` first: millimetres or metres, as in the spec.
2. Make layers for walls, doors, dimensions and text. Draw each on its
   layer.
3. Draw the outline from the spec's corner list with `LINE` or `PLINE`
   and relative coordinates. Then add offsets (wall thickness), openings
   (`TRIM`), dimensions and text.
4. `ZOOM E` and take a `screenshot` to check.
5. Measure: `DIST\n` and two points. Or `LIST\n` on an object.

## Save and output

- `cmd+s` saves the `.dwg`.
- PDF: `EXPORTPDF\n` (AutoCAD), or `PLOT\n` with a PDF printer.
- LibreCAD and QCAD use the same coordinate syntax in their command
  widget, with similar command names (`line`, `rectangle`, `circle`).

## Notes

- LISP and scripts (`APPLOAD`, `.scr` files) run code: only with the
  user's OK (security rule 2).
