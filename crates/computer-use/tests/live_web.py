"""A React form edited through the server, and what was really saved (live test).

A record editor written as React apps are (fixtures/react_form: a controlled
input whose value lives in React's state, and Save sends that state) is
opened in a browser. Through the real server the test sets the Title field,
presses Save, then reopens the record. It checks what the backend got, not
only what the field shows: a value set behind React's back (an
accessibility "set value" that fires no input event) shows in the field and
in the accessibility tree, yet the old one is what is saved.

  python live_web.py <computer-use-mcp> <browser> [port]

Starts the fixture's backend (react_form_server.py) and the browser itself;
on Linux run it inside a desktop session with the accessibility bus (see
run_live_web.sh). Exits 1 on any failure.
"""
import json
import os
import re
import subprocess
import sys
import tempfile
import time

binary, browser = sys.argv[1], sys.argv[2]
port = int(sys.argv[3]) if len(sys.argv) > 3 else 18777
here = os.path.dirname(os.path.abspath(__file__))
work = tempfile.mkdtemp(prefix="cu-web-")
store = os.path.join(work, "record.json")
failures = []


def check(ok, what):
    print(("ok   " if ok else "FAIL ") + what, flush=True)
    if not ok:
        failures.append(what)


def record():
    with open(store) as f:
        return json.load(f)


backend = subprocess.Popen([sys.executable, os.path.join(here, "fixtures", "react_form_server.py"), str(port), store])
for _ in range(50):
    if os.path.exists(store):
        break
    time.sleep(0.1)
url = f"http://127.0.0.1:{port}/"
browser_proc = subprocess.Popen(
    [
        browser,
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-gpu",
        "--no-proxy-server",
        "--test-type",
        "--force-renderer-accessibility",
        "--disable-features=Translate",
        f"--user-data-dir={os.path.join(work, 'profile')}",
        "--window-size=1000,700",
        "--window-position=0,0",
        f"--app={url}",
    ]
    + (["--no-sandbox"] if os.name != "nt" and os.geteuid() == 0 else []),
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
)

server = subprocess.Popen([binary, "serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, encoding="utf-8")
ids = iter(range(1, 10000))


def send(message):
    server.stdin.write(json.dumps(message) + "\n")
    server.stdin.flush()


def call(method, params):
    i = next(ids)
    send({"jsonrpc": "2.0", "id": i, "method": method, "params": params})
    while True:
        line = server.stdout.readline()
        if not line:
            raise SystemExit("the server ended")
        reply = json.loads(line)
        if reply.get("id") == i:
            return reply


def tool(name, **args):
    reply = call("tools/call", {"name": name, "arguments": args})
    result = reply.get("result", {})
    text = "".join(c.get("text", "") for c in result.get("content", []) if c.get("type") == "text")
    return text, bool(result.get("isError"))


def index(tree, pattern):
    m = re.search(r"^\s*(\d+) " + pattern, tree, re.M)
    return int(m.group(1)) if m else None


def find_app():
    """The browser, by the window that shows the form."""
    deadline = time.time() + 60
    last = ""
    while time.time() < deadline:
        apps, _ = tool("list_apps")
        # The browser's own entries first: the others are only read when
        # its name is not one of these.
        found = re.findall(r"^- (.*?) \(.*pid: (\d+)", apps, re.M)
        found.sort(key=lambda a: not re.search(r"edge|chrom", a[0], re.I))
        for _, pid in found:
            text, err = tool("get_app_state", app=pid, rebase=True)
            if not err and index(text, r'text field "Title"') is not None:
                return pid, text
            last = text
        time.sleep(1)
    raise SystemExit("the form never showed in the accessibility tree:\n" + apps + "\n" + last[:2000])


try:
    call("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "live-web", "version": "1"}})
    send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    app, tree = find_app()
    print(tree[:1500])
    check(record()["title"] == "Old title", "the record starts as \"Old title\"")
    field, save = index(tree, r'text field "Title"'), index(tree, r'button "Save"')
    check('value="Old title"' in tree, "the field shows the saved title")

    # Change the field the way an agent does.
    out, err = tool("set_value", app=app, element_index=field, value="New title")
    print(out)
    check(not err, "set_value answers without an error")
    check('value="New title"' in out, "the field shows the new title after set_value")

    # Save, and look at what the backend got: the record, not the field.
    out, err = tool("click", app=app, element_index=save, expect="Saved record")
    print(out)
    check(not err and "Expected Saved record: confirmed" in out, "Save is pressed and the page says it saved")
    for _ in range(50):
        if record()["saves"] > 0:
            break
        time.sleep(0.1)
    saved = record()
    check(saved["saves"] == 1, f"the backend got one save (got {saved['saves']})")
    check(saved["title"] == "New title", f"what was saved is the new title (saved: {saved['title']!r})")

    # Reopen the record: a fresh page, filled from the backend.
    out, err = tool("press_key", app=app, key="F5")
    check(not err, "the page is reloaded")
    reopened = ""
    for _ in range(40):
        reopened, _ = tool("get_app_state", app=app, rebase=True)
        if index(reopened, r'text field "Title"') is not None and "Saved record" not in reopened:
            break
        time.sleep(0.5)
    check('value="New title"' in reopened, "the reopened record shows the new title")
finally:
    for p in (server, browser_proc, backend):
        try:
            p.kill()
        except Exception:
            pass

if failures:
    print(f"{len(failures)} check(s) failed")
    sys.exit(1)
print("all checks passed")
