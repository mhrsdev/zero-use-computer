#!/usr/bin/env python3
"""Negative controls for verify.py, on edited copies of the starting diagram
(no Dia needed): only the correct edit may pass."""
import json
import os
import re
import subprocess
import sys
import tempfile

here = os.path.dirname(os.path.abspath(__file__))
tmp = tempfile.mkdtemp()
start = os.path.join(tmp, "start.dia")
subprocess.check_call([sys.executable, os.path.join(here, "make_diagram.py"), start])
src = open(start).read()


def drop(label, text):
    obj = next(o for o in re.findall(r"<dia:object .*?</dia:object>", text, re.S) if f"#{label}#" in o)
    return text.replace(obj, "")


cases = [
    ("Cache deleted (correct)", drop("Cache", src), True),
    ("nothing changed", src, False),
    ("Cache Monitor deleted instead", drop("Cache Monitor", src), False),
    ("both Cache boxes deleted", drop("Cache Monitor", drop("Cache", src)), False),
    ("Cache deleted, a box moved", re.sub(r'(name="elem_corner"><dia:point val=")2.0,2.0', r"\g<1>3.0,2.0", drop("Cache", src), count=1), False),
    ("Cache deleted, a label edited", drop("Cache", src).replace("#Queue#", "#Queu#"), False),
]
ok = True
for name, text, want in cases:
    path = os.path.join(tmp, "case.dia")
    open(path, "w").write(text)
    got = json.loads(subprocess.check_output([sys.executable, os.path.join(here, "verify.py"), path, "0"]))
    good = got["success"] == want
    ok &= good
    print(f"{'ok ' if good else 'BAD'} {name}: success={got['success']} {got['problems']}")
sys.exit(0 if ok else 1)
