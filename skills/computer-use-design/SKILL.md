---
name: computer-use-design
description: >-
  Design work with the computer-use tools: images, posters, logos, icons,
  UI mockups, vector art, 3D models and scenes, floor plans and CAD
  drawings in Photoshop, Paint, GIMP, Krita, Illustrator, Inkscape, Figma,
  Blender, Revit, AutoCAD, SketchUp and similar apps, from a text brief or a
  reference image. Load it with computer-use and computer-use-security.
---

# Design with computer use

Good results come from numbers, not from eyeballing. Before you touch the
app, turn the request into a spec of sizes, positions and colours. Then
build it with the most exact method the app has, and check every step
against the spec.

## The method

1. **Spec first.** Write a SPEC block in your reply before acting. Use the
   template for the kind of work in [reference/spec.md](reference/spec.md):
   canvas or units, every element back to front with position, size and
   colour, the text, and the output file. If the user gave a reference
   image, measure it first:
   [reference/reference-images.md](reference/reference-images.md). If the
   brief is vague, choose sensible values, list them in the spec and go on.
   Ask only for what you can't choose yourself (the text of a logo, a brand
   colour).
2. **Set up the document exactly**: size, units and background, typed
   into the app's New / Document Setup / Units dialogs. Never set them by
   dragging.
3. **Build in passes**, back to front:
   1. background;
   2. big shapes;
   3. smaller shapes;
   4. colours;
   5. text;
   6. details.

   One element at a time. Name each layer or object after the spec
   ("bg", "title").
4. **Check after every pass**: [reference/checks.md](reference/checks.md).
   Fix the first difference before going on. Undo (`cmd+z`) rather than
   painting over a mistake.
5. **Save versions**: `name_v1`, `name_v2`, … in the app's own format.
   Never overwrite the user's original file (security rule 3).
6. **Deliver**: export the format and size the spec says, then tell the
   user where the file is and what the spec was.

## The most exact way first

Use the first of these that the app offers for the step:

1. **Numeric fields**: New-document size; X/Y/W/H in a properties or
   transform panel; hex colour fields; Properties palettes. Click the field,
   select all (`cmd+a`), type the value, then `Return`.
2. **Typed values in a command**:
   - AutoCAD's command line;
   - Blender's `g x 2 Return`;
   - SketchUp's Measurements box;
   - Revit's temporary dimensions.
3. **`draw` with exact shapes** (`rect`, `ellipse`, `points`, curves).
   Give coordinates in the document's own pixels with
   `canvas={"box": [l, t, r, b], "size": [w, h]}`; measure the box on a
   grid screenshot.
4. **The app's script console** (Blender Python, Photoshop scripts, Revit
   Dynamo or pyRevit, AutoCAD LISP). This is the most exact of all, but it
   runs code, so use it only after the user says yes (security rule 2).
5. **Clicks and drags at estimated positions.** Last resort; take a grid
   screenshot first.

## Seeing exactly

- `screenshot(app, grid=100)` draws a labelled grid in the same x/y that
  `click`, `drag` and `draw` use. Read positions off it; never guess them.
- `screenshot(app, pick=[[x,y], ...])` gives the exact colour at points:
  check a fill. `palette=true` lists the main colours: compare with the
  spec.
- `screenshot(app, element_index=i)` zooms in to read small text and
  values.
- Drawn pixels are not in the accessibility tree; panels and fields
  usually are. Read values from fields, not from the picture.

## Keys and the pointer

- **Keys follow the mouse.** Some apps send keys to whatever is under the
  mouse: Blender, and parts of CAD apps. Give `press_key` and `type_text`
  an `x`/`y` inside the right area (the 3D viewport, the drawing area).
- **The pointer sets the direction.** Typed lengths in Revit and SketchUp
  go the way the pointer points. Give the `type_text` that types the
  length an `x`/`y` a little way in that direction.
- **One call per command.** The pointer goes back after each call, so run
  a whole modal command in one `press_key`: `"g x 2 Return"`.
- **Keypad keys** are `Numpad0`…`Numpad9` and `NumpadDecimal`.

## When it goes wrong

- Nothing happened: look (screenshot) before trying again. A dialog may
  be open, the wrong tool may be active, or the canvas may not have focus.
- Wrong result: undo, re-read the spec, redo that one step.
- Lost: reopen the last saved version.
- The same step failed twice: use the next method down the list above,
  or ask the user.

## Apps

Read the playbook for the app before starting:

- [Photoshop](reference/apps/photoshop.md);
- [Paint](reference/apps/paint.md);
- [GIMP and Krita](reference/apps/gimp-krita.md);
- [Illustrator and Inkscape](reference/apps/vector.md);
- [Figma](reference/apps/figma.md);
- [Blender](reference/apps/blender.md);
- [Revit](reference/apps/revit.md);
- [AutoCAD and other 2D CAD](reference/apps/cad.md);
- [SketchUp](reference/apps/sketchup.md).

Worked examples, step by step: [reference/recipes.md](reference/recipes.md).
