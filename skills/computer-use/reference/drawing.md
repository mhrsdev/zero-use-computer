# Drawing with the mouse

`draw` presses the button, moves along each stroke and releases: a pen or
brush in a paint app, a whiteboard, a chart editor. Pick the tool, colour and
size in the app first, then draw, then check with `screenshot`: drawn pixels
don't show in the accessibility tree.

## Where

- Default: screenshot pixels, like `click`'s x/y (y grows downwards).
- `canvas: {"box": [left, top, right, bottom], "size": [w, h]}`: the
  document's own units. `box` is where the document is in the screenshot
  (read it off `screenshot(app, grid=50)`), `size` its size in its units
  (e.g. 1080 x 1350 px). Then every coordinate is a document pixel,
  whatever the zoom. With `element_index` instead of `box`, the element's
  box is the document.
- `element_index`: fractions of that element's box, (0,0) its top-left and
  (1,1) its bottom-right. Handy for "the middle of the canvas"; a circle in
  fractions is an ellipse unless the box is square, so use pixels for exact
  shapes.
- Parts of a curve outside the area are not drawn; points outside it are an
  error.

## Strokes

Each stroke is one press-move-release; several strokes lift the pen between
them (axes, then a curve).

- Rectangle: `{"rect": [x, y, width, height]}`. Ellipse or circle:
  `{"ellipse": [center x, center y, radius x, radius y]}`.
- Lines and polygons: `{"points": [[100,100],[300,100],[300,200]], "closed": true}`.
- Freehand curves: `"smooth": true` draws a smooth curve through the points.
- Parametric curves: `x` and `y` are expressions in `t`, from `t[0]` to
  `t[1]` (default 0 to 1); `steps: n` makes n straight pieces instead.
  - Circle / ellipse: `x "400+90*cos(t)"`, `y "300+60*sin(t)"`,
    `t [0, "2*pi"]`; an arc is part of that range.
  - Regular polygon: the circle with `steps: 6`. Star:
    `x "400+90*cos(t*4*pi/5)"`, `y "300+90*sin(t*4*pi/5)"`, `t [0, 5]`,
    `steps: 5`.
  - Spiral: `x "400+8*t*cos(t)"`, `y "300+8*t*sin(t)"`, `t [0, "6*pi"]`.
  - Heart: `x "400+6*16*sin(t)^3"`,
    `y "300-6*(13*cos(t)-5*cos(2*t)-2*cos(3*t)-cos(4*t))"`, `t [0, "2*pi"]`.
- Plotting y = f(u) for u from a to b into a box (left, top, width,
  height): `x "left + (t-a)/(b-a)*width"`, `y "baseline - scale*f(t)"`,
  `t [a, b]`. Screen y grows downwards, hence the minus. Where f jumps or
  is undefined (`tan`, `1/t`) the pen lifts.

Expressions: `+ - * / % ^` (also `**`), parentheses, `2t` and `2pi` mean
products; `sin cos tan asin acos atan atan2 sinh cosh tanh sqrt cbrt abs exp
ln log10 log2 floor ceil round trunc sign min max pow hypot mod clamp`;
`pi tau e`.

## Speed and stopping

- `speed` is in pixels per second (default 800). Slow it down for apps that
  draw jagged lines or miss parts.
- The stop key ends a drawing midway; the button is always released.
- Drawing a signature is acting in the user's name: security rule 3.
