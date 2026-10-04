"""Windows: does the overlay show while the server works? (diagnosis)

Runs a released computer-use-mcp.exe as an MCP server against Notepad and,
while it works, lists the overlay's windows (class ComputerUseOverlay):
visible, cloaked by DWM, where, and where in the z-order. Screenshots leave
the overlay out (WDA_EXCLUDEFROMCAPTURE), so the window state is what we
can read.

  python scripts/overlay_diag.py path\\to\\computer-use-mcp.exe
"""
import ctypes, ctypes.wintypes as wt, glob, json, os, subprocess, sys, tempfile, threading, time

exe = sys.argv[1]
if os.path.isdir(exe):
    exe = glob.glob(os.path.join(exe, "**", "computer-use-mcp.exe"), recursive=True)[0]
user32 = ctypes.WinDLL("user32", use_last_error=True)
dwm = ctypes.WinDLL("dwmapi")
WNDENUMPROC = ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)

def windows():
    out = []
    def cb(h, _):
        buf = ctypes.create_unicode_buffer(256)
        user32.GetClassNameW(h, buf, 256)
        out.append((h, buf.value))
        return True
    user32.EnumWindows(WNDENUMPROC(cb), 0)
    return out  # top of the z-order first

def info(h):
    r = wt.RECT(); user32.GetWindowRect(h, ctypes.byref(r))
    cloaked = ctypes.c_int(0)
    dwm.DwmGetWindowAttribute(h, 14, ctypes.byref(cloaked), 4)
    pid = wt.DWORD(); user32.GetWindowThreadProcessId(h, ctypes.byref(pid))
    ex = user32.GetWindowLongW(h, -20) & 0xFFFFFFFF
    return dict(visible=bool(user32.IsWindowVisible(h)), cloaked=cloaked.value,
                rect=(r.left, r.top, r.right - r.left, r.bottom - r.top), pid=pid.value, ex=hex(ex))

samples, stop = [], threading.Event()
def sampler():
    t0 = time.time()
    while not stop.is_set():
        ws = windows()
        ov = [(i, h) for i, (h, c) in enumerate(ws) if c == "ComputerUseOverlay"]
        note = [i for i, (h, c) in enumerate(ws) if c == "Notepad"]
        shown = [(i, info(h)) for i, h in ov]
        samples.append((round(time.time() - t0, 2), len(ov), [s for s in shown if s[1]["visible"]], note[:1]))
        time.sleep(0.1)

home = tempfile.mkdtemp()
env = dict(os.environ, COMPUTER_USE_HOME=home, COMPUTER_USE_LOG="debug")
subprocess.Popen(["notepad.exe"]); time.sleep(3)
p = subprocess.Popen([exe, "serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=open(os.path.join(home, "server.err"), "w"), env=env, text=True, bufsize=1)
n = 0
def call(method, params):
    global n; n += 1
    p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": n, "method": method, "params": params}) + "\n"); p.stdin.flush()
    while True:
        m = json.loads(p.stdout.readline())
        if m.get("id") == n:
            return m
def tool(name, args):
    r = call("tools/call", {"name": name, "arguments": args})
    text = r.get("result", {}).get("content", [{}])[0].get("text", str(r))
    print(f"> {name} {args}\n  {text[:300]!r}")
    return text

print(call("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "diag", "version": "1"}})["result"]["serverInfo"])
threading.Thread(target=sampler, daemon=True).start()
tool("list_apps", {})
for i in range(3):
    tool("get_app_state", {"app": "notepad"}); time.sleep(1.5)
tool("click", {"app": "notepad", "x": 200, "y": 200}); time.sleep(1.0)
tool("type_text", {"app": "notepad", "text": "hello overlay"}); time.sleep(4)
stop.set(); time.sleep(0.3)

print("\n== overlay windows over time (t, count, visible ones, notepad z-index) ==")
last = None
for t, count, vis, note in samples:
    key = (count, [(i, v["rect"], v["cloaked"]) for i, v in vis], note)
    if key != last:
        print(t, count, [(i, v["rect"], "cloaked=%d" % v["cloaked"], v["ex"]) for i, v in vis], "notepad z:", note)
        last = key
print("max visible overlay windows at once:", max((len(v) for _, _, v, _ in samples), default=0))
p.stdin.close(); time.sleep(2)
print("\n== server.err ==\n" + open(os.path.join(home, "server.err"), errors="replace").read()[-6000:])
for f in glob.glob(os.path.join(home, "*.log")):
    print(f"\n== {f} ==\n" + open(f, errors="replace").read()[-4000:])
