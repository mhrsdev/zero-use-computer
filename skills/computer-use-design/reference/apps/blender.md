# Blender

Blender draws its whole interface itself: the accessibility tree is almost
empty. Work from screenshots (with `grid`) and OCR text, and do nearly
everything with the keyboard and typed numbers.

## The two rules

1. **Keys go to the area under the mouse.** Give every `press_key` and
   `type_text` an `x`/`y` inside the area that should get them: the 3D
   viewport for modelling, the Properties editor for its fields, the
   Python console for code.
2. **Type numbers, don't drag.** Every transform takes a typed value. Send
   the whole command in one `press_key`:
   - `"g x 2 Return"`: move 2 m along X.
   - `"g z minus 1 period 5 Return"`: move -1.5 m along Z.
   - `"r z 4 5 Return"`: rotate 45° about Z.
   - `"s 2 Return"`: scale ×2.
   - `"s x 0 period 5 Return"`: scale X by 0.5.

## Basics (pointer over the 3D viewport)

Blender uses Ctrl for its modelling shortcuts on every system (also on a
Mac): write `ctrl+`, not `cmd+`, for them. Only file commands (save,
undo) also take Cmd on a Mac.

- **Find any command:** `F3`, `type_text` its name ("Add Cube", "Shade
  Smooth", "Apply Scale"), then `Return`.
- **Add an object:**
  1. `shift+a` opens the Add menu; typing searches it.
  2. Or `F3` with "Add Cylinder" and the like.
  3. New objects appear at the 3D cursor. Reset the cursor to the origin
     first: `shift+c`.
- **Delete** the selection: `Delete`. Select all / none: `a` /
  `alt+a`.
- **Exact values:**
  1. `n` opens the sidebar. On its Item tab are Location, Rotation, Scale
     and Dimensions.
  2. Double-click a field (its value gets selected), type the value, then
     `Return`.
  3. After setting Dimensions, apply the scale: `ctrl+a` in the viewport,
     then Scale.
- **Views:**
  - `Numpad1` front, `Numpad3` right, `Numpad7` top.
  - `Numpad0` the camera.
  - `NumpadDecimal` frames the selection; `Home` frames everything.
  - Without a working keypad, use View ▸ Viewpoint, or ask the user to
    turn on "Emulate Numpad".
- **Edit mode:**
  - `Tab` toggles it; `1` / `2` / `3` select vertices / edges / faces.
  - Extrude `"e 0 period 5 Return"`, inset `i`.
  - Loop cut `ctrl+r`, bevel `ctrl+b` (type the width, `Return`).
- **Modifiers:** Properties editor ▸ wrench tab ▸ Add Modifier. Or `F3`
  "Add Modifier"; then set its fields by double-clicking them.
- **Smooth look:** `F3` "Shade Smooth". Subdivision: `ctrl+1` or `ctrl+2`.

## Materials, camera, light, render

- **Material:**
  1. Properties ▸ Material tab ▸ New.
  2. Click Base Color; in the colour picker, type the Hex value.
  3. Set Roughness and Metallic by double-clicking and typing.
- **Camera:** select it, then type Location and Rotation in the sidebar.
  Or frame the view and press `ctrl+alt+Numpad0` (camera to view).
- **Light:** add with `shift+a` ▸ Light. Set Power in the Light
  properties.
- **Output size:** Properties ▸ Output tab ▸ Resolution X/Y.
- **Render:**
  1. `F12`.
  2. Wait for it to finish (`screenshot` until the image stops changing).
  3. In the render window, Image ▸ Save As (`shift+alt+s`), type the path.
- **Save:** `cmd+s` saves the `.blend`.
- **Export:** File ▸ Export (glTF, FBX, OBJ, STL).

## Strokes in the viewport

`draw` works where Blender expects the mouse to paint: Grease Pencil
Draw mode, Sculpt mode and Texture Paint.

- Give `draw` the viewport's element or a `box` round it, and
  `speed` 300–500.
- Each stroke is one brush stroke; `repeat` lays out many (dabs of clay
  round a centre, a row of grooves).
- Sizes and positions of objects still go in as typed values, not strokes.

## References

- Add ▸ Image ▸ Reference loads a photo or drawing into the viewport.
- Align it in the front or side view, and model over it.

## The Python console: the most exact way, only with permission

The Scripting workspace has a Python console. One line builds an exact
object. It runs code on the user's computer: ask first (security rule 2).
With the user's OK, point at the console and `type_text` one line at a
time:

```
bpy.ops.mesh.primitive_cube_add(size=1, location=(0, 0, 0.375)); o = bpy.context.object; o.dimensions = (1.2, 0.8, 0.75)
m = bpy.data.materials.new("Wood"); m.diffuse_color = (0.43, 0.27, 0.16, 1); o.data.materials.append(m)
```

For longer scripts use the Text Editor: New, type the script, then Run
Script (`alt+p` with the mouse over the editor). Read the console or the
Info area for errors.
