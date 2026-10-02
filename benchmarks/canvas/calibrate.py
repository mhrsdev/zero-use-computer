#!/usr/bin/env python3
"""Locate each box on screen from a screenshot of the colour-coded diagram
(make_diagram.py --colors, same window size and place as the runs).
Prints {"label": [left, top, right, bottom], ...} in screen pixels, the fill
area (searched inside the canvas only) widened by 3 px to include the box's border."""
import json
import sys

from PIL import Image

from make_diagram import BOXES, COLORS

img = Image.open(sys.argv[1]).convert("RGB")
w, h = img.size
px = img.load()
want = {tuple(int(c[i:i + 2], 16) for i in (1, 3, 5)): label
        for c, (label, _, _) in zip(COLORS, BOXES)}
found = {}
# Only the canvas: the toolbox and toolbar have coloured icons.
CANVAS = (232, 147, 1180, 840)
for y in range(CANVAS[1], CANVAS[3]):
    for x in range(CANVAS[0], CANVAS[2]):
        label = want.get(px[x, y])
        if label:
            b = found.setdefault(label, [x, y, x, y])
            b[0], b[1] = min(b[0], x), min(b[1], y)
            b[2], b[3] = max(b[2], x), max(b[3], y)
pad = 3
print(json.dumps({k: [b[0] - pad, b[1] - pad, b[2] + pad, b[3] + pad]
                  for k, b in found.items()}, indent=1))
