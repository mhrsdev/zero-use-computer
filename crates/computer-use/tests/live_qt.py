"""Linux: a Qt app and a crashing app against the real server (live test).

Run inside a desktop session with the Qt fixture and the crashing fixture
running (see run_live_qt.sh):

  python3 live_qt.py <computer-use-mcp> <qt app pid> <crash notes file>

Qt's accessibility bridge (up to Qt 6.9) crashes on a `Properties.GetAll`,
which the server sent to every app it listed, KWin and Plasma included:
one list_apps took a KDE session down. This lists the apps, reads the Qt
app's tree, clicks its button and types in its field, then checks that the
app is still running and that the crashing app crashed once, not once per
restart. Exits 1 on any failure.
"""
import json
import os
import subprocess
import sys
import time

binary, qt_pid, notes = sys.argv[1], int(sys.argv[2]), sys.argv[3]
failures = []


def check(ok, what):
    print(("ok   " if ok else "FAIL ") + what)
    if not ok:
        failures.append(what)


server = subprocess.Popen(
    [binary, "serve"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    text=True,
)
ids = iter(range(1, 1000))


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


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


call(
    "initialize",
    {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "live-qt", "version": "1"}},
)
send({"jsonrpc": "2.0", "method": "notifications/initialized"})

apps, _ = tool("list_apps")
print(apps)
check("QtFixture" in apps, "list_apps names the Qt app (by its own name)")
check(alive(qt_pid), "the Qt app is running after list_apps")

state, err = tool("get_app_state", app="QtFixture", screenshot=False)
print(state)
check(not err and '"Press me"' in state, "the Qt app's tree shows its button")
check(alive(qt_pid), "the Qt app is running after its tree is read")


def index_of(text, label):
    for line in text.splitlines():
        if f'"{label}"' in line:
            return int(line.split()[0])
    return None


button, field = index_of(state, "Press me"), index_of(state, "Qt field")
out, err = tool("click", app="QtFixture", element_index=button)
print(out)
check(not err and '"Pressed"' in out, "a click on the Qt button presses it")
out, err = tool("set_value", app="QtFixture", element_index=field, value="hello")
print(out)
check(not err and "hello" in out, "set_value types in the Qt field")
check(alive(qt_pid), "the Qt app is running after acting on it")

# The crashing app is restarted every 0.3 s: listed again and again, it must
# be asked once and then left alone.
for _ in range(5):
    tool("list_apps")
    time.sleep(0.5)
crashes = open(notes).read().splitlines() if os.path.exists(notes) else []
print(f"crashing app crashed {len(crashes)} time(s): {crashes}")
check(len(crashes) == 1, "a program that quit while answering is left alone (one crash, not one per restart)")

server.stdin.close()
server.wait(timeout=20)
print("all passed" if not failures else f"{len(failures)} failed")
sys.exit(1 if failures else 0)
