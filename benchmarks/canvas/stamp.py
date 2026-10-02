#!/usr/bin/env python3
"""Copy stream-json lines from stdin to stdout, adding the time each arrived
("_t", epoch seconds), for the run's timeline."""
import json
import sys
import time

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        ev = json.loads(line)
    except json.JSONDecodeError:
        ev = {"_raw": line}
    ev["_t"] = time.time()
    sys.stdout.write(json.dumps(ev) + "\n")
    sys.stdout.flush()
