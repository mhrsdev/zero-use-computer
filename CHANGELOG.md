# Changelog

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
