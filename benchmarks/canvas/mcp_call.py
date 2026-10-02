#!/usr/bin/env python3
"""Minimal MCP stdio client, used for the oracle run (no model):
  mcp_call.py SERVER_BIN CONFIG 'tool {json}' ['tool {json}' ...]
Prints each result's text and the size of any image."""
import base64, json, subprocess, sys
p = subprocess.Popen([sys.argv[1], "--config", sys.argv[2]], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
n = 0
def rpc(method, params=None, notify=False):
    global n
    msg = {"jsonrpc": "2.0", "method": method}
    if params is not None: msg["params"] = params
    if not notify:
        n += 1; msg["id"] = n
    p.stdin.write(json.dumps(msg) + "\n"); p.stdin.flush()
    if notify: return None
    while True:
        r = json.loads(p.stdout.readline())
        if r.get("id") == n: return r
rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "x", "version": "0"}})
rpc("notifications/initialized", notify=True)
k = 0
for spec in sys.argv[3:]:
    tool, _, args = spec.partition(" ")
    r = rpc("tools/call", {"name": tool, "arguments": json.loads(args or "{}")})
    print(f"=== {tool} {args}")
    for c in r.get("result", {}).get("content", []):
        if c["type"] == "text": print(c["text"])
        elif c["type"] == "image":
            k += 1; print(f"[image {len(base64.b64decode(c['data']))} bytes]")
    if "error" in r: print("ERROR", r["error"])
p.stdin.close(); p.wait(timeout=10)
