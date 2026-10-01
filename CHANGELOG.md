# Changelog

## v2.5.7

See a drawing before it is built: a design board for 2D, a scene for 3D,
graph-paper cells to place and check every part, and exact aiming at small
targets
([all commits](https://github.com/mhrsdev/zero-use-computer-/compare/v2.5.1...v2.5.7)).

### New

- **`design`**: a design board, like Canva. A picture is built from layers
  (the shapes `draw` takes, and text) that can be added, changed, removed,
  mirrored (the other eye or ear), aligned, distributed and reordered. Each
  call returns:
  - the rendered picture;
  - every layer with its box and colours;
  - checks: off the page or into the margin, nearly centred, pairs not
    quite symmetric, hard-to-read or overlapping text, too many colours;
  - the steps to paint it.

  `show` adds a grid, the layers' ids or guides.
- **`draw` strokes `{"design": name, "step": n, "fill": w}`** paint one
  colour step of a design, fitted into the canvas.
- **`scene`**: a 3D model planned as solids before it is built in an app.
  - Solids: box, cylinder, sphere, cone, torus and plane, each with a
    size, a centre and a rotation (Z up, the ground at z 0).
  - Editing: add, change, remove, mirror and repeat (in a row, or around
    an axis). `on` sets one part on top of another.
  - The picture: the front, right and top views to one scale with a grid,
    and a perspective view with shadows straight down.
  - Checks: parts that float (and how far above what), sink into the
    ground or run into each other (and how deep).
  - Building it: the numbers for Blender's Location, Rotation and
    Dimensions fields, or for any other app.
- **Exports are temporary.**
  - `design` writes SVG or PNG; `scene` writes OBJ (with an MTL file of
    its colours) or PNG.
  - They go to `computer-use-exports` in the system's temp folder and are
    deleted when the server stops.
  - Files older than a day are removed, the folder is kept under 200 MB,
    and nothing is ever overwritten.
- **Cells: graph paper for drawing.** The area is cut into square cells
  named like a chessboard (columns A, B…, rows 1, 2… from the top left),
  sized so what is drawn spans about eight of them.
  - The `draw` preview shows them, and every `draw` result says which
    cells the drawing covers.
  - `screenshot` with `canvas` takes `cells: true` to lay them over the
    document, and `cell: "C4"` to magnify one cell with a fine grid in the
    document's units and its main colours.
  - The design board shows them too; `show: {"cell": "C4"}` opens one with
    the layers in it.
- **Pixel targeting:**
  - `screenshot` `zoom: [x, y]` magnifies around a point, each screen
    pixel a square, with a crosshair and a grid in click coordinates.
  - `locate` finds exact places in a window: every area of a colour, every
    look-alike of an icon or marker, or the exact corner, edge or centre
    next to a rough point.
  - `click` and `drag` take `snap` (`corner`, `edge`, `center` or a
    colour) to move onto that point before acting.

### Changed

- **The design skill** plans every drawing on the board first: `design`
  for 2D, `scene` for 3D.
  - A new reference page, `reference/board.md`.
  - Recipes for a badge painted from a design and a 3D model built from a
    scene.
  - Checks for the board, cell by cell and in 3D.
  - "From a scene" sections in the Blender and SketchUp playbooks.
- **The computer-use skill** covers cells, `locate`, `snap`, `zoom`,
  `design` and `scene`.
- **The security skill**: exported files are temporary; finished work is
  saved only where the user asked.

## v2.5.1

Better drawing and copying of pictures, from a test where a smaller model
copied a photo of a cat in Paint
([all commits](https://github.com/mhrsdev/zero-use-computer-/compare/v2.5.0...v2.5.1)).

### New

- **`trace_image`**: turns a reference picture (an image file, or part of
  a window) into a few flat colours and shapes, as numbered steps to paint
  back to front. It returns the steps and a picture of the result; `colors`
  and `detail` set how close it comes.
- **`draw` strokes `{"trace": name, "step": n, "fill": w}`** paint one step
  of a trace, fitted into the canvas with its proportions kept.
- **`fill: w` on any closed `draw` stroke** paints it solid with a brush
  `w` wide instead of its outline. The brush stays inside the edge, and
  shapes painted back to front cover each other. No bucket fill, so
  overlapping shapes come out right even in apps without layers. The
  result warns when shapes are narrower than the brush.
- **`screenshot` `compare: name`** (with `canvas`) compares the document
  with a trace: how many cells look alike, and the most different places
  with the colour each should be.

### Changed

- **Fill points are checked on the real pixels.** After drawing (or on the
  preview), `draw` floods the picture like a bucket fill does. It gives
  one click per piece when other lines cut a shape into pieces, and says
  where an outline has a gap a fill would leak through. It used to give
  one point from the shapes alone.
- **The design skill** paints overlapping shapes solid (Paint's playbook
  explains why outlines and bucket fills go wrong there), copies photos
  with `trace_image` (a new recipe), and checks with `compare`.
- **The security skill**: `trace_image` reads only a picture the user gave
  or pointed to.

## v2.5.0

Compared with v2.0.0
([all commits](https://github.com/mhrsdev/zero-use-computer-/compare/v2.0.0...v2.5.0)).

### New

- **`draw`**: draws with the mouse button held down.
  - Shapes: rectangles (rounded corners too), ellipses, arcs, regular
    polygons, stars, Bézier curves, straight or smooth freehand lines.
  - Parametric curves `x(t)`, `y(t)` and function plots `y = f(x)` with
    ticked axes.
  - Any stroke can be rotated and repeated (rows, radial patterns).
  - Coordinates in screenshot pixels, an element's box, a document's own
    units, or a math range with y up (`canvas`).
  - `preview: true` shows the strokes over a screenshot without drawing.
  - The result names a point inside each closed shape to click for a
    bucket fill (between the circles for a ring).
  - Paced by `speed`; the stop key ends it midway and releases the button.
- **`screenshot`**:
  - `grid`: a labelled grid in the coordinates `click` and `draw` use;
  - `palette`: the main colours;
  - `pick`: the exact colour at points;
  - `canvas`: grid and `pick` in a document's units or a plot's range.
- **`press_key` / `type_text` with `x`/`y`**: the mouse points there
  first, for apps that send keys to what is under the pointer (Blender,
  CAD). Keypad keys: `Numpad0`…`Numpad9`, `NumpadDecimal` and the keypad
  operators.
- **Design skill** (`skills/computer-use-design`) for Photoshop, Paint,
  GIMP, Krita, Illustrator, Inkscape, Figma, Blender, Revit, AutoCAD and
  SketchUp, from a text brief or a reference image:
  - a spec of exact numbers first;
  - the most exact method each app has;
  - checks after every pass;
  - an app playbook for each app, and recipes for common jobs.
  - Offered over MCP like the other skills, as the `computer-use-design`
    prompt and its files as resources.

### Fixed

- Skill files served over MCP had Windows line endings (CRLF) on Windows.
  They now have LF on every OS.

### Changed

- **No approvals in the server.** Per-app approvals, the sensitive-app
  categories, the on-screen approval window (and the indicator's "waiting
  for approval" state) and `change_setting` are gone.
  - Which apps and actions need the user's OK is set by the
    `computer-use-security` skill. Every skill prompt brings it along.
  - The server still keeps input on the app it is meant for. Passwords
    and card numbers are masked. The emergency stop key works.
- **`launch_app` takes no command-line arguments** (`args` is gone). It
  starts an app by its name, bundle id or executable, and never runs a
  command line.
- **Settings**: `computer-use-mcp config` on the command line.
- **Removed tools**: `create_folder`, `list_folder`, `read_file` and
  `skill`.
  - For files, use the MCP client's own file tools.
  - Skills still come as MCP prompts and resources, as in v2.0.0.
