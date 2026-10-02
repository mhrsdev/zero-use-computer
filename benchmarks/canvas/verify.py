#!/usr/bin/env python3
"""Check the task's outcome from the file Dia saved.

  verify.py SAVED.dia START_EPOCH  ->  prints one JSON object, exit 0

Success means all of:
  - the file was written after the run started (it was saved);
  - it holds exactly the eight boxes other than "Cache", nothing else;
  - each of them keeps its label and position (within 0.01 cm).
"""
import gzip
import json
import os
import sys
import xml.etree.ElementTree as ET

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from make_diagram import BOXES, DX, DY, X0, Y0  # noqa: E402

NS = {"dia": "http://www.lysator.liu.se/~alla/dia/"}
TARGET = "Cache"


def load(path):
    raw = open(path, "rb").read()
    if raw[:2] == b"\x1f\x8b":
        raw = gzip.decompress(raw)
    return ET.fromstring(raw)


def attr(obj, name):
    return obj.find(f"dia:attribute[@name='{name}']", NS)


def objects(root):
    out = []
    for obj in root.iter(f"{{{NS['dia']}}}object"):
        label = None
        text = attr(obj, "text")
        if text is not None:
            s = text.find(".//dia:attribute[@name='string']/dia:string", NS)
            if s is not None and s.text is not None:
                label = s.text.strip("#")
        corner = attr(obj, "elem_corner")
        pos = None
        if corner is not None:
            p = corner.find("dia:point", NS).get("val").split(",")
            pos = (float(p[0]), float(p[1]))
        out.append({"type": obj.get("type"), "label": label, "pos": pos})
    return out


def main():
    path, start = sys.argv[1], float(sys.argv[2])
    res = {"saved": False, "success": False, "problems": []}
    if not os.path.exists(path):
        res["problems"].append("file missing")
        print(json.dumps(res))
        return
    res["saved"] = os.path.getmtime(path) > start
    if not res["saved"]:
        res["problems"].append("not saved during the run")
    objs = objects(load(path))
    res["objects"] = objs
    expected = {
        label: (X0 + c * DX, Y0 + r * DY) for label, c, r in BOXES if label != TARGET
    }
    labels = [o["label"] for o in objs]
    res["target_deleted"] = TARGET not in labels
    res["wrong_deleted"] = sorted(set(expected) - set(labels))
    res["extra"] = [o for o in objs if o["label"] not in expected]
    if not res["target_deleted"]:
        res["problems"].append("Cache still present")
    if res["wrong_deleted"]:
        res["problems"].append(f"missing: {res['wrong_deleted']}")
    extras = [o for o in res["extra"] if o["label"] != TARGET]
    if extras:
        res["problems"].append(f"unexpected objects: {extras}")
    moved = []
    for o in objs:
        want = expected.get(o["label"])
        if want and o["pos"] and max(abs(a - b) for a, b in zip(o["pos"], want)) > 0.01:
            moved.append(o["label"])
    res["moved"] = moved
    if moved:
        res["problems"].append(f"moved: {moved}")
    if len([l for l in labels if l in expected]) != len(expected):
        res["problems"].append("box count differs (duplicates?)")
    res["success"] = not res["problems"]
    print(json.dumps(res))


if __name__ == "__main__":
    main()
