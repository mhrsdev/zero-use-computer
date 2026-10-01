# Drawing with the mouse

`draw` presses the button, moves along each stroke and releases: a pen or
brush in a paint app, a whiteboard, a chart editor. Pick the tool, colour and
size in the app first. Then `preview`, then draw, then check with
`screenshot`: drawn pixels don't show in the accessibility tree.

## Coordinates

| How | Coordinates |
|---|---|
| default | screenshot pixels, like `click`'s x/y (y down) |
| `element_index` | fractions of that element's box: (0,0) top-left, (1,1) bottom-right |
| `canvas: {"box": [l, t, r, b], "size": [w, h]}` | the document's own units (e.g. 1080 x 1350 px), y down, whatever the zoom |
| `canvas: {"box": [l, t, r, b], "range": [x0, x1, y0, y1]}` | math coordinates across the box, **y up**: for plots |

- `box` is where the document (or plot area) is in the screenshot. Read it
  off `screenshot(app, grid=50)`. With `element_index` instead of `box`,
  that element's box is used.
- Use the same `canvas` in `screenshot(app, canvas=..., grid=true,
  pick=[...])`. The grid then covers just the document, labelled in its
  units, and `pick` reads colours at document coordinates.
- Explicit points outside the area are an error. Parts of a curve outside
  it are skipped (the pen lifts).
- If 1 unit of x and of y differ on screen, circles look like ellipses (the
  result says so). Give the range the same proportions as its box.

## Strokes

Each stroke is one press-move-release; several strokes lift the pen between
them.

| Stroke | Meaning |
|---|---|
| `{"rect": [x, y, w, h]}` | rectangle; a 5th number rounds the corners |
| `{"ellipse": [cx, cy, rx, ry]}` | ellipse or circle |
| `{"polygon": [cx, cy, r, n]}` | regular polygon, first corner up |
| `{"star": [cx, cy, R, r, n]}` | star with n points, outer R, inner r |
| `{"arc": [cx, cy, r, from, to]}` | arc, degrees from +x towards +y |
| `{"bezier": [[x,y], [c1], [c2], [x,y], ...]}` | cubic curves: start, then control, control, end per segment |
| `{"points": [[x,y], ...], "closed": true}` | straight lines; `"smooth": true` for a smooth curve through them (freehand) |
| `{"x": "...", "y": "...", "t": [a, b]}` | parametric curve; `"steps": n` makes n straight pieces |
| `{"y": "sin(x)"}` | plot of y = f(x) over the whole x range (or `t: [a, b]`) |
| `{"axes": [xstep, ystep]}` | axes through 0 with ticks; needs `canvas.range` |

Any stroke can also take:

- `"rotate": deg`: clockwise on screen, about `"about": [x, y]` (default:
  its centre).
- `"repeat": {"count": n, "offset": [dx, dy], "rotate": deg, "about": [x, y]}`:
  copy k is moved by k × offset and turned by k × rotate. Use it for rows
  of dots, petals around a centre, or a spiral of squares.

Examples:

- **Circle:** `{"ellipse": [400, 300, 90, 90]}`.
- **Flower:** 8 petals round (400, 300):
  `{"ellipse": [400, 240, 20, 50], "repeat": {"count": 8, "rotate": 45, "about": [400, 300]}}`.
- **Spiral:** `x "400+8*t*cos(t)"`, `y "300+8*t*sin(t)"`, `t [0, "6*pi"]`.
- **Heart:** `x "400+6*16*sin(t)^3"`,
  `y "300-6*(13*cos(t)-5*cos(2*t)-2*cos(3*t)-cos(4*t))"`, `t [0, "2*pi"]`.
- **A plot with axes:**
  `canvas {"box": [l, t, r, b], "range": [-3.2, 3.2, -1.5, 1.5]}`, then
  strokes `{"axes": [1, 0.5]}` and `{"y": "sin(x)"}`.

Expressions:

- operators `+ - * / % ^` (also `**`), parentheses, and products written
  together (`2t`, `2pi`);
- functions `sin cos tan asin acos atan atan2 sinh cosh tanh sqrt cbrt abs
  exp ln log10 log2 floor ceil round trunc sign min max pow hypot mod clamp`;
- constants `pi tau e`. Where f jumps or is undefined (`tan`, `1/x`), the
  pen lifts.

## Preview, then draw

- `"preview": true` draws nothing. It shows the strokes in red over a
  screenshot, with a grid in the coordinates you used. Check the placement,
  then call again without it.
- The result gives, for each closed shape, a point to click with a fill
  (bucket) tool or a magic wand, in click coordinates and named after its
  stroke. The point avoids the shapes drawn inside it: a ring's (one circle
  in another) lies between the two circles.

## Shape tools versus brushes

- With a brush, pencil or pen **tool** active, the stroke is the line that
  gets painted: `rect`, `ellipse` and the others draw their outlines.
- With an app's **shape tool** (Paint's rectangle, Figma's `r`), the app
  makes the shape from one drag. Give a stroke of two points, corner to
  corner: `{"points": [[x, y], [x + w, y + h]]}`.

## Speed and stopping

- `speed` is in pixels per second (default 800). Use 300–500 for apps that
  smooth strokes (Photoshop, Krita) or that draw jagged lines.
- The stop key ends a drawing midway; the button is always released.
- Drawing a signature is acting in the user's name: security rule 3.
