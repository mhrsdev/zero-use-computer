#!/usr/bin/python3.12
"""Independent check (not the server's code): walk Dia's raw AT-SPI tree and
report every node, the drawing area's children, and whether any box label
appears in any name, description or text. Run with the system python3.12
(python3-gi, gir1.2-atspi-2.0) inside the benchmark's desktop session."""
import json
import sys

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi  # noqa: E402

LABELS = ["Client", "API Gateway", "Auth Service", "Cache Monitor", "Queue",
          "Cache", "Database", "Logs", "Metrics"]


def text_of(acc):
    try:
        t = acc.get_text_iface()
        if t is not None:
            return t.get_text(0, t.get_character_count())
    except Exception:
        pass
    return ""


def main():
    desktop = Atspi.get_desktop(0)
    app = next(desktop.get_child_at_index(i) for i in range(desktop.get_child_count())
               if desktop.get_child_at_index(i).get_name() == "dia")
    nodes, hits, drawing = 0, [], []
    stack = [(app, 0)]
    while stack:
        acc, depth = stack.pop()
        nodes += 1
        role = acc.get_role_name()
        blob = " ".join([acc.get_name() or "", acc.get_description() or "", text_of(acc)])
        for label in LABELS:
            if label in blob:
                hits.append({"label": label, "role": role, "text": blob.strip()[:80]})
        if role == "drawing area":
            drawing.append({"children": acc.get_child_count(), "name": acc.get_name(),
                            "interfaces": acc.get_interfaces()})
        for i in range(acc.get_child_count()):
            stack.append((acc.get_child_at_index(i), depth + 1))
    json.dump({"raw_nodes": nodes, "drawing_areas": drawing, "label_hits": hits},
              sys.stdout, indent=1)
    print()


main()
