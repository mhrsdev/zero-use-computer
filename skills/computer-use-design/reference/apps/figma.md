# Figma (desktop app or browser)

UI designs, mockups and social graphics. The panels are ordinary web
controls, in the tree; the canvas is drawn and is not. `cmd+` is Cmd on a
Mac, Ctrl elsewhere.

## Build exactly

1. **Frame:** press `f`, then pick a preset in the right panel (Desktop,
   iPhone, Instagram post…), or draw one and type W/H.
2. **Shapes:**
   1. Rectangle `r`, Ellipse `o`, Line `l`.
   2. Drag roughly with `draw` (two points).
   3. Then in the right Design panel type X, Y, W, H. X/Y are relative to
      the frame.
3. **Fill:** in the Design panel, click the fill row's hex value and type
   the hex. Stroke and corner radius are in the same panel.
4. **Text:**
   1. `t`, then click inside the frame and `type_text`.
   2. `Escape` to finish.
   3. Font, size, weight, line height and alignment in the Design panel.
5. **Layout:**
   - Auto layout `shift+a` stacks children with exact gaps and padding:
     type them in the panel.
   - Align buttons are at the top of the Design panel.
6. **Names:** double-click a layer in the Layers panel (left) and type
   the spec's name.

## Components and pages

- Create component: `cmd+alt+k`. Instances update with the main
  component.
- Pages are listed at the top of the left panel.

## Export

- Select the frame. In the Design panel, Export section: add a format (PNG,
  JPG, SVG, PDF) and scale (1x/2x), then Export.

## Notes

- It saves to the cloud by itself; there is no "save".
- Version history is in the file menu: use it rather than undoing many
  steps.
- Don't share files, invite people or change permissions unless asked
  (security rule 3).
- Plugins can run code and reach the network: only with the user's OK.
