# Changelog

## v2.5.0

Compared with v2.0.0.

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
- **Skills over MCP**: every skill is offered as an MCP prompt, and each
  of its files as a resource, for clients without skill files. They are
  served with LF line endings on every OS.

### Changed

- **No approvals in the server.** The on-screen approval window and
  `change_setting` are gone.
  - Which apps and actions need the user's OK is set by the
    `computer-use-security` skill. Every skill prompt brings it along.
  - The server still keeps input on the app it is meant for. `launch_app`
    never runs a command line. Passwords and card numbers are masked. The
    emergency stop key works.
- **Settings**: `computer-use-mcp config` on the command line.
- **Removed tools**: `create_folder`, `list_folder`, `read_file` and
  `skill`.
  - For files, use the MCP client's own file tools.
  - Skills come as MCP prompts and resources (above).
