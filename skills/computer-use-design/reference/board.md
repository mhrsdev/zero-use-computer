# The board: `design` and `scene`

See the work before building it. A fix on the board is one call; in the app
it is undo and redraw. Both tools keep what you make under its `name`, so
later calls change it. A call that fails changes nothing.

## `design`: a page of layers (2D)

Start with a name, a size in the result's pixels, and a background:

```json
{"name": "badge", "size": [1000, 1150], "background": "#F4EDE4",
 "add": [
   {"id": "ring", "ellipse": [500, 500, 400, 400], "fill": "none", "stroke": "#1E3A5F", "width": 24},
   {"id": "disc", "ellipse": [500, 500, 330, 330], "fill": "#1E3A5F"},
   {"id": "star", "star": [500, 500, 240, 100, 5], "fill": "#F2C14E"},
   {"id": "dot", "ellipse": [500, 135, 12, 12], "fill": "#1E3A5F",
    "repeat": {"count": 12, "rotate": 30, "about": [500, 500]}},
   {"id": "title", "text": "EST. 2024", "at": [500, 960], "size": 64, "bold": true,
    "align": "center", "fill": "#1E3A5F"}]}
```

- **Layers** are the shapes `draw` takes (`rect`, `ellipse`, `polygon`,
  `star`, `arc`, `bezier`, `points`, curves in `t`, `rotate`, `repeat`) or a
  `text` with `at`, `size`, `font`, `bold`, `align`. Colours: `fill`,
  `stroke` (hex or `"none"`), `width` and `opacity`. A new closed shape is
  filled black and an open line drawn 2 wide, unless you say otherwise.
  Later layers cover earlier ones; `below`/`above` put a new one elsewhere.
- **Changes:**
  - `change`: by id, any field, plus `move` [dx, dy] or `to` [x, y] (the
    layer's box starts there).
  - `remove`: ids.
  - `mirror`: `{"id": "eye-l", "as": "eye-r"}`, a copy flipped about the
    page's middle (`axis` x or y; `line` puts the mirror elsewhere).
  - `align`: `{"ids": [...], "x": "center", "to": "page"}` (or the margins,
    each other, or a layer).
  - `distribute`: equal gaps between 3 or more layers.
  - `order`: to front, back, up or down.
- **Lines:** a layer can be written as the listing shows it, which is
  shorter: `"disc ellipse 500 500 330 330 fill #1E3A5F"`, `"title text
  \"EST. 2024\" at 500 960 size 64 bold align center fill #1E3A5F"`,
  `"ring ellipse 500 500 400 400 fill none line #1E3A5F 24"`; a change:
  `"star fill #E0B040"`.
- **What comes back:**
  - the picture, with named cells (A1 top-left);
  - every layer with its box and colours;
  - checks;
  - the paint steps.

  After that, only what changed: the layers changed, added or removed and
  the new order, "as before" for the rest, and no picture when it looks
  the same. A call with only `name` (and `show`) shows everything again.
  When the paint steps are left out ("N step(s) to paint it"), `show:
  {"steps": true}` lists them.

  `show` adds a `grid` (in the design's units), the layers' `ids` and
  `guides` (margins, centre, thirds); `"cell": "C4"` shows that cell
  magnified, with the layers in it.
- **Checks** to clear before building:
  - a layer off the page or into the margin;
  - nearly centred ("is 6 left of the page's centre": align it);
  - a pair not quite symmetric (mirror one from the other);
  - text that is hard to read on what is under it, or that overlaps other
    text;
  - too many colours.

### Into the app

1. **Import** (vector apps, Figma, Photoshop): `export: "svg"` (or
   `"png"`). The file is temporary and is deleted when the server stops,
   so import or place it at once, then save in the app.
2. **Fields** (apps with X/Y/W/H and hex fields): type each layer's
   numbers from the listing.
3. **Paint** (Paint, raster apps): the steps go back to front. Each names
   its colour, its kind (solid, lines n wide, or text) and its layers.
   Fill the background first. Then, for each step:
   - **solid:** set the colour, pick a brush `w` wide, and
     `draw(canvas=..., strokes=[{"design": "badge", "step": 1, "fill": w}])`;
   - **lines:** a brush that wide, and the same stroke without `fill`;
   - **text:** the app's text tool at the layer's `at`, its font and size.

   The design is fitted into the `canvas` with its proportions kept.

## `scene`: solids in 3D

Metres (or any one unit), Z up, the ground at z 0.

```json
{"name": "stool", "add": [
   {"id": "seat", "shape": "cylinder", "size": [0.4, 0.04], "at": [0, 0, 0.47], "color": "#8B5A2B"},
   {"id": "leg", "shape": "box", "size": [0.04, 0.04, 0.45], "at": [0.12, 0.12, 0.225]}],
 "mirror": [{"id": "leg", "as": "leg-b"}],
 "repeat": [{"id": "leg", "count": 2, "offset": [0, -0.24, 0]},
            {"id": "leg-b", "count": 2, "offset": [0, -0.24, 0]}]}
```

- **Objects:**
  - `shape`: `box`, `cylinder`, `sphere`, `cone`, `torus` or `plane`.
  - `size`: box `[x, y, z]`; cylinder and cone `[diameter, height]`;
    sphere `[diameter]`; torus `[outer diameter, thickness]`; plane
    `[x, y]`. Three numbers give any shape its own size per axis.
  - `at`: the centre. Without it, the object stands on the ground at the
    middle. On the ground, `at` z is half the height.
  - `rotate`: degrees about x, then y, then z, like Blender. A cylinder or
    cone stands along z until turned: `[90, 0, 0]` lays it along y, `[0,
    90, 0]` along x.
  - `on`: `"seat"` sets its height so it rests on the seat's top;
    `"ground"` sets it on the floor.
  - `move`: `[dx, dy, dz]`.
- **Copies:**
  - `mirror`: `{"id", "as", "axis": "x"}` flips about x = `at` (default
    0). Use it for the other leg, arm, wing or wheel.
  - `repeat`: `count` in all, the original first, named `id-2`,
    `id-3`…; each copy `offset` further (stairs, a fence), or turned
    `around` [x, y] (spokes, chairs round a table; `angle` defaults to
    360).
- **The picture** shows the front (x right, z up), right (y right, z up)
  and top (x right, y up) views to one scale with a grid, and a
  perspective view from the front right with shadows straight down. Use
  `view` for one view, bigger, and `look: [turn, tilt]` to turn the camera
  (turn 90 looks from the right, tilt 90 from above).
- **Checks:**
  - "floats … 0.06 above seat": nothing holds it. Set it down with `on`,
    or say what holds it.
  - "sinks 0.1 below the ground": raise it, unless it is meant to be
    buried.
  - "run into each other … 0.02 deep": fine for joints (a leg set into a
    top), a mistake for parts that should only touch.
  - With `ground: false` (things in the air, space), it names the groups
    that do not touch.

### Into the 3D app

1. **Import:** `export: "obj"` writes the model and its colours as
   temporary files. Import at once (Blender: File > Import > Wavefront
   (.obj), default settings), then save in the app.
2. **Build by numbers** (any app): each object is one primitive with
   Location = `at`, Rotation = `rotate`, Dimensions = `size`. In Blender:
   Add > Mesh > Cube (box), Cylinder, UV Sphere, Cone, Torus or Plane, then
   type the three fields in the sidebar (`n`). For apps that draw from a
   corner (SketchUp), a box's corner is `at - size / 2`. See
   [apps/blender.md](apps/blender.md) and [apps/sketchup.md](apps/sketchup.md).
3. **Compare:** the app's front, right and top views (Blender `Numpad1`,
   `Numpad3`, `Numpad7`) should look like the scene's. A difference is a
   typed number that went wrong.

## Cells: graph paper

Every drawing has its cells: columns A, B, C… left to right, rows 1, 2,
3… from the top, sized so what is drawn spans about eight. A small drawing
gets small cells.

- The `draw` preview shows them and the result says which cells a drawing
  covers ("It covers cells B2 to D3, cells of 100").
- `screenshot(app, canvas=..., cells=true)` lays them over the document;
  `cell="C4"` shows one cell magnified, with a fine grid in the
  document's units and its main colours.
- `design` shows them on its picture; `show: {"cell": "C4"}` magnifies one.
- `cell_size` (on `design`, `draw` and `screenshot`) fixes their size in
  the document's units, so all three name the same cells: an 800 x 800
  board with `cell_size: 100` is an 8 x 8 chessboard, A1 to H8.
- A `script` page is a design whose cells it can fill and label by name
  (`p.fill_cell("C4", "#222222")`, `p.cell("C4")`), for drawings that
  follow a rule: pixel art, boards, charts, patterns.

Use them to talk about places ("the ear in C2 is too low") and to check a
drawing one cell at a time where precision matters: an eye, a joint, a
corner where two lines must meet.
