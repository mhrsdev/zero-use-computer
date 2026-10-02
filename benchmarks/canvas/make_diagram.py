#!/usr/bin/env python3
"""Write the benchmark's starting diagram: nine labelled Flowchart boxes in a
3x3 grid, as an uncompressed Dia file.

  make_diagram.py OUT.dia            the diagram the agent works on
  make_diagram.py OUT.dia --colors   same geometry, each box filled with its
                                     own colour (only used to locate the
                                     boxes on screen for the click analysis)
"""
import sys

# label, column, row. "Cache Monitor" is a deliberate near-miss for "Cache".
BOXES = [
    ("Client", 0, 0), ("API Gateway", 1, 0), ("Auth Service", 2, 0),
    ("Cache Monitor", 0, 1), ("Queue", 1, 1), ("Cache", 2, 1),
    ("Database", 0, 2), ("Logs", 1, 2), ("Metrics", 2, 2),
]
X0, Y0, DX, DY, W, H = 2.0, 2.0, 9.0, 5.0, 7.0, 2.0  # cm (wide enough that Dia never resizes a box)
# One distinct colour per box for --colors (RGB hex, index = BOXES order).
COLORS = ["#e6194b", "#3cb44b", "#ffe119", "#4363d8", "#f58231",
          "#911eb4", "#42d4f4", "#f032e6", "#bfef45"]


def box(i, label, col, row, fill):
    x, y = X0 + col * DX, Y0 + row * DY
    return f"""    <dia:object type="Flowchart - Box" version="0" id="O{i}">
      <dia:attribute name="obj_pos"><dia:point val="{x},{y}"/></dia:attribute>
      <dia:attribute name="obj_bb"><dia:rectangle val="{x - 0.05},{y - 0.05};{x + W + 0.05},{y + H + 0.05}"/></dia:attribute>
      <dia:attribute name="elem_corner"><dia:point val="{x},{y}"/></dia:attribute>
      <dia:attribute name="elem_width"><dia:real val="{W}"/></dia:attribute>
      <dia:attribute name="elem_height"><dia:real val="{H}"/></dia:attribute>
      <dia:attribute name="border_width"><dia:real val="0.1"/></dia:attribute>
      <dia:attribute name="inner_color"><dia:color val="{fill}"/></dia:attribute>
      <dia:attribute name="show_background"><dia:boolean val="true"/></dia:attribute>
      <dia:attribute name="padding"><dia:real val="0.5"/></dia:attribute>
      <dia:attribute name="text">
        <dia:composite type="text">
          <dia:attribute name="string"><dia:string>#{label}#</dia:string></dia:attribute>
          <dia:attribute name="font"><dia:font family="sans" style="0" name="Helvetica"/></dia:attribute>
          <dia:attribute name="height"><dia:real val="0.8"/></dia:attribute>
          <dia:attribute name="pos"><dia:point val="{x + W / 2},{y + H / 2 + 0.25}"/></dia:attribute>
          <dia:attribute name="color"><dia:color val="#000000"/></dia:attribute>
          <dia:attribute name="alignment"><dia:enum val="1"/></dia:attribute>
        </dia:composite>
      </dia:attribute>
    </dia:object>
"""


def main():
    out = sys.argv[1]
    colors = "--colors" in sys.argv[2:]
    objs = "".join(
        box(i, label, col, row, COLORS[i] if colors else "#ffffff")
        for i, (label, col, row) in enumerate(BOXES)
    )
    doc = f"""<?xml version="1.0" encoding="UTF-8"?>
<dia:diagram xmlns:dia="http://www.lysator.liu.se/~alla/dia/">
  <dia:layer name="Background" visible="true" active="true">
{objs}  </dia:layer>
</dia:diagram>
"""
    with open(out, "w") as f:
        f.write(doc)


if __name__ == "__main__":
    main()
