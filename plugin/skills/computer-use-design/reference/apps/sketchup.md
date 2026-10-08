# SketchUp

Quick 3D massing, rooms and furniture. The 3D view is not in the tree;
the Measurements box (bottom right) takes typed values for almost every
tool. Type them with `type_text` right after a click or a `press_key`.
You don't click the box itself. When a value goes "in the direction of the
pointer" (a line, a move, a push/pull), give that `type_text` an `x`/`y` a
little way in that direction: the pointer sets it.

## Tools (single keys)

| Key | Tool |
|---|---|
| `l` | Line |
| `r` | Rectangle |
| `c` | Circle |
| `p` | Push/Pull |
| `m` | Move |
| `q` | Rotate |
| `s` | Scale |
| `f` | Offset |
| `t` | Tape Measure |
| `e` | Eraser |
| `Space` | Select |
| `o` | Orbit |
| `h` | Pan |
| `z` | Zoom |
| `shift+z` | Zoom extents |

## Exact geometry

- **Rectangle:**
  1. `r`.
  2. Click the first corner (the origin where the red, green and blue
     axes meet is a good start; find it on a grid screenshot).
  3. Move a little, then `type_text "4000,3000\n"`. Units come from the
     model template; you can type them, as in `4m,3m`.
- **Line:**
  1. `l`, click a start point.
  2. Point along the direction. While drawing, the arrow keys lock an axis:
     Right = red (X), Left = green (Y), Up = blue (Z).
  3. Type the length, then `\n`.
- **Push/Pull:** `p`, click a face, move a little, type the distance,
  then `\n` (e.g. `2700` for a wall height in mm).
- **Move:**
  1. `m`, click the object, click a base point.
  2. Point in the direction, type the distance, then `\n`.
- **Copy:** select it, `cmd+c`, Edit ▸ Paste In Place, then Move the copy
  by a typed distance as above.
- **Offset:** `f`, click a face's edge, type the distance, then `\n`.
- **Circle:** `c`, click the centre, type the radius, then `\n`. Before
  clicking, type the number of segments and `\n`.

## From a scene

Plan the model in `scene` first (the design skill's `reference/board.md`), then build
each object from its numbers:

- **A box** (not rotated): its corner is `at - size / 2`. `r`, click that
  corner (or the origin, then Move the group there), type
  `"<x size>,<y size>\n"`, then `p` and the z size. Make it a group.
- **A cylinder** standing up: `c`, click the centre of its bottom
  (`at`, with z lowered by half the height), type the radius, then `p`
  and the height.
- **Rotated parts**: build them unrotated at the origin, make a group,
  rotate it with `q` and a typed angle, then Move it into place.
- SketchUp's red, green and blue axes are the scene's x, y and z. Check
  with Camera ▸ Standard Views ▸ Front, Right and Top against the scene's
  views.

## Organise and finish

- Make groups of finished parts: select them, then Edit ▸ Make Group.
  Name them in Entity Info.
- **Materials:** Window ▸ Default Tray ▸ Materials (the Paint Bucket `b`).
  Edit a colour's values in the material editor.
- **Views:** Camera ▸ Standard Views (Top, Front, Iso).
- **Save:** `cmd+s` saves the `.skp`.
- **Export:** File ▸ Export ▸ 2D Graphic (PNG), or 3D Model.

## Notes

- Extensions and the Ruby console run code: only with the user's OK.
