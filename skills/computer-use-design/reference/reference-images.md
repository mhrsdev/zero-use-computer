# Working from a reference image

Turn the reference into numbers, then build from the numbers. Copying "by
eye" drifts.

## 1. Put the reference on screen

- **A file on disk** (the user gave a path, or it is in a folder they
  named): open it.
  - In a separate viewer window, or in the design app as a reference layer
    (see the app's playbook).
  - Then you can measure it and read its exact colours.
- **Only in the chat.** You can see it but not measure it exactly.
  - Describe it carefully in the spec.
  - If exact colours or proportions matter, ask the user to save it and
    give you the path.

## 2. Measure the layout

1. `screenshot(app, grid=50)` of the window showing the reference.
2. Note where the image is: left `L`, top `T`, right `R`, bottom `B` (grid
   coordinates).
3. For each element, back to front, note its box (left, top, right,
   bottom) the same way. Include shapes, photos, text blocks and lines.
4. Convert to canvas pixels for a `W` x `H` canvas:
   - `x = (left - L) / (R - L) * W`
   - `y = (top - T) / (B - T) * H`
   - `w = (right - left) / (R - L) * W`
   - `h = (bottom - top) / (B - T) * H`

   Round to whole pixels and write them into the spec.
5. Keep the reference's aspect ratio unless the brief says otherwise:
   `H = W * (B - T) / (R - L)`.

## 3. Read the colours

- `screenshot(app, palette=true)` while the reference fills most of the
  window gives its main colours. With `element_index` on the image
  element it reads only the image.
- `pick=[[x, y], ...]` at the centre of each flat area gives exact
  colours. Avoid edges: they are mixed with the neighbour.
- Photos and gradients have no single colour: note the two or three
  colours at their ends.

## 4. Read the text and type

- Zoom in (`screenshot(app, element_index=i)` or a region) and copy the
  text exactly, with its capitals and punctuation.
- Note each text's style: serif or sans-serif, weight (regular / bold),
  size as a share of the image height, alignment, colour.
- Choose the closest installed font and say which one you used.

## 5. Trace instead of measuring (2D apps)

1. Place the reference as the bottom layer, scaled to the canvas.
2. Set it to 30–50% opacity and lock it.
3. Draw on new layers above it, with `draw` and `canvas` in canvas
   pixels.
4. Hide or delete the reference layer before exporting.

## 6. 3D from photos (Blender)

- Estimate sizes from things of known size:

  | Object | Height |
  |---|---|
  | Door | 2.0–2.1 m |
  | Table top | 0.72–0.76 m |
  | Chair seat | 0.43–0.47 m |
  | Ceiling | 2.4–3.0 m |
  | Step | 0.17 m |

- Load the photo as a reference image (Add > Image > Reference) behind the
  front or side view, and match the outlines.

## 7. Plans from an image or PDF (Revit, CAD)

1. Import it as an underlay.
2. Scale it with one known dimension (measure a wall whose length you
   know, then scale by known / measured).
3. Trace the walls on top.
4. Write the corner coordinates into the spec.

## 8. Compare at the end

- Show both side by side, or the result and the reference one after the
  other, at the same zoom.
- Compare element by element: position (within about 1% of the canvas),
  size, colour (`pick` both at the same spots), text.
- Fix the biggest difference first, then compare again.
