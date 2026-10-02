#!/usr/bin/env python3
"""Log every mouse button press on the X display as a JSON line
{"t": epoch, "button": n}: an independent count of the physical clicks the
server made (a double click is two presses).

It listens to XI2 *raw* button events on the root window, which reach every
listener, so the app still gets its clicks. Raw events carry no position, and
the server puts the pointer back after clicking, so where a click landed is
taken from the server's own result text instead (see analyze.py)."""
import json
import struct
import sys
import time

from Xlib import display
from Xlib.ext import xinput

RAW_BUTTON_PRESS = 15



def button(data):
    # Raw event body after the generic header: deviceid u16, time u32, detail u32.
    if isinstance(data, bytes) and len(data) >= 10:
        return struct.unpack_from("<HII", data)[2]
    return getattr(data, "detail", None)


d = display.Display()
root = d.screen().root
root.xinput_select_events([(xinput.AllMasterDevices, 1 << RAW_BUTTON_PRESS)])
out = open(sys.argv[1], "w", buffering=1)
while True:
    ev = d.next_event()
    if getattr(ev, "evtype", None) == RAW_BUTTON_PRESS:
        out.write(json.dumps({"t": time.time(), "button": button(ev.data)}) + "\n")
