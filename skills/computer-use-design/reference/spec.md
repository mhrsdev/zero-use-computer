# Writing the spec

Write the spec in your reply before you act, and keep to it. Every number
you will type or draw comes from here, so a later step never has to guess.

## 2D: images, posters, logos, icons, UI

```
SPEC
Goal: <one line: what it is and who it is for>
App: <app>   Output: <file name>.<png|jpg|svg|pdf>, <W> x <H> px
Canvas: <W> x <H> px, background <#hex>, margin <m> px
Palette: <#hex> main, <#hex> accent, <#hex> text, <#hex> light
Fonts: <family> for headings, <family> for body (installed ones only)
Elements, back to front:
  1. <name>: <rect|ellipse|line|path|image|text> x <x> y <y> w <w> h <h>,
     fill <#hex>, stroke <#hex> <n> px, radius <r>, opacity <n>%
  2. ...
Text:
  "<exact text>": <font> <size> px <weight>, <#hex>, left|center|right,
  box x <x> y <y> w <w>
Checks: <what must be true at the end>
```

- Positions are the top-left corner in canvas pixels, (0, 0) top-left.
  For a centred element: x = (W - w) / 2. Write down the number, not the
  formula.
- Keep everything at least the margin away from the edges.
- Fonts: use ones the app lists. If the requested font is missing, say so
  and pick a similar one.

## 3D: models and scenes (Blender)

```
SPEC
Goal: ...   Units: metres   Output: <name>.blend + <name>.png <W>x<H>
Objects:
  1. <name>: <cube|cylinder|sphere|plane|...> size <x y z> m,
     location <x y z>, rotation <x y z> deg, material <name>
Materials: <name> base colour <#hex>, roughness <0-1>, metallic <0-1>
Camera: location <x y z>, rotation <x y z> deg, focal length <mm>
Lights: <sun|point|area> location <x y z>, strength <n>
Render: engine <Eevee|Cycles>, <W> x <H> px
```

Z is up. The floor is z = 0. An object of height h stands on the floor at
z = h / 2 (Blender places origins at the centre).

## Buildings and CAD (Revit, AutoCAD, SketchUp)

```
SPEC
Goal: ...   Units: <mm|m|in>   Output: <file> + <pdf|dwg|ifc>
Levels: Level 1 at 0, Level 2 at <h>
Grid / origin: (0, 0) at <corner>
Walls: <type>, thickness <t>, height <h>:
  W1 (0, 0) -> (<x>, 0); W2 ...    (centre lines, in order)
Openings: <door|window> <type> <w> x <h> in W<n>, <d> from its start,
  sill <s>
Rooms: <name> bounded by W1-W4
Annotations: dimensions on <...>, tags, title
```

Draw a sketch in coordinates first: list the corners in order, check that
the walls close, and add up the lengths.

## Sizes to use when the brief gives none

| Use | Size |
|---|---|
| Square social post | 1080 x 1080 px |
| Portrait social post | 1080 x 1350 px |
| Story / phone screen | 1080 x 1920 px |
| Slide / video frame | 1920 x 1080 px |
| A4 print | 2480 x 3508 px (300 dpi), 210 x 297 mm |
| Logo | 1000 x 1000 px, transparent background |
| App icon | 1024 x 1024 px |
| Web banner | 1200 x 630 px |

## Design rules that make it look right

- **Margins.** 5 to 8% of the short side, the same on every edge.
- **Alignment.** Align things to the same few x positions: left margin,
  centre, right margin. Equal gaps between repeated items.
- **Colour.** One main colour (about 60%), one secondary (30%), one accent
  (10%). Text contrasts strongly with what is behind it: dark on light or
  light on dark.
- **Type.**
  - At most two fonts.
  - Title about 2 to 3 times the body size.
  - Body at least 24 px on a 1080 px canvas.
  - Line height about 1.2 to 1.4.
- **Hierarchy.** One thing is clearly the biggest; the eye goes title,
  then image, then details.
- **Space.** When in doubt, make things smaller and leave more space.

## Example: from a text brief

Brief: "Make an Instagram post for a coffee shop opening on May 3."

```
SPEC
Goal: opening announcement for a coffee shop, warm and simple
App: Photoshop   Output: opening_post.png, 1080 x 1350 px
Canvas: 1080 x 1350, background #F4EDE4, margin 80
Palette: #6F4E37 main, #C08552 accent, #2B1D14 text, #F4EDE4 light
Fonts: Georgia Bold headings, Arial body
Elements, back to front:
  1. bg: rect 0,0 1080x1350 fill #F4EDE4
  2. cup: ellipse centre (540, 560) radius 220 x 220 fill #6F4E37
  3. saucer: ellipse centre (540, 800) radius 300 x 40 fill #C08552
Text:
  "GRAND OPENING": Georgia Bold 96 px, #2B1D14, center, box x 80 y 950 w 920
  "May 3 - 8 am": Arial 48 px, #6F4E37, center, box x 80 y 1080 w 920
Checks: margins >= 80 px; title one line; colours match the palette
```
