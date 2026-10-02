"""Exact token costs, from the API's own counts.

Each measurement is one `claude -p` request (no tools run) whose input-token
count the API reports. The difference between two requests that differ in
one thing is that thing's exact cost:

* tool definitions: the same request with the server exposing different tools
  (`tools.enabled`), against a server exposing only `list_apps`;
* text: the same request with and without a text in the user message.

usage:
  measure.py tools <computer-use-mcp> <out.json> [tool ...]   per-tool and whole-list costs
  measure.py texts <out.json> <name=file> ...                  texts' costs

Run inside a desktop session (the server needs an accessibility bus).
"""
import json, os, subprocess, sys, tempfile, uuid

MODEL = os.environ.get("MODEL", "claude-sonnet-5-5")
SYSTEM = "Reply with the single word OK."
STRIP = ["CLAUDECODE", "CLAUDE_CODE_SESSION_ID", "CLAUDE_CODE_REMOTE_SESSION_ID",
         "CLAUDE_CODE_TEE_SDK_STDOUT", "CLAUDE_CODE_MESSAGING_SOCKET",
         "CLAUDE_CODE_MESSAGING_TOKEN", "CLAUDE_CODE_SYNC_SESSION_REFS",
         "CLAUDE_CODE_SYNC_SKILLS", "CLAUDE_CODE_DIAGNOSTICS_FILE", "CLAUDE_CODE_DEBUG",
         "CLAUDE_EFFORT"]


def ask(prompt, mcp=None):
    """Input tokens of one request (input + cache reads + cache writes)."""
    env = {k: v for k, v in os.environ.items() if k not in STRIP}
    cmd = ["claude", "-p", prompt, "--model", MODEL, "--max-turns", "1",
           "--system-prompt", SYSTEM, "--tools", "", "--setting-sources", "",
           "--no-session-persistence", "--session-id", str(uuid.uuid4()),
           "--output-format", "json"]
    if mcp:
        cmd += ["--mcp-config", mcp, "--strict-mcp-config"]
    else:
        cmd += ["--strict-mcp-config"]
    out = subprocess.run(cmd, capture_output=True, text=True, env=env,
                         stdin=subprocess.DEVNULL, timeout=300)
    r = json.loads(out.stdout)
    u = r["usage"]
    return u["input_tokens"] + u["cache_creation_input_tokens"] + u["cache_read_input_tokens"]


def server(binary, config_text, home):
    cfg = os.path.join(home, f"cfg-{uuid.uuid4().hex[:8]}.toml")
    open(cfg, "w").write(config_text)
    mcp = os.path.join(home, f"mcp-{uuid.uuid4().hex[:8]}.json")
    json.dump({"mcpServers": {"cu": {"command": binary, "args": ["--config", cfg],
                                     "env": {"COMPUTER_USE_HOME": home}}}}, open(mcp, "w"))
    return mcp


def tools(binary, out, names):
    home = tempfile.mkdtemp(prefix="cu-tok-")
    res = {"model": MODEL}
    res["no_server"] = ask("Go.")
    res["only_list_apps"] = ask("Go.", server(binary, '[tools]\nenabled = ["list_apps"]\n', home))
    res["default"] = ask("Go.", server(binary, "", home))
    res["full_descriptions"] = ask("Go.", server(binary, '[tools]\ndescriptions = "full"\n', home))
    per = {}
    for n in names:
        if n == "list_apps":
            continue
        cfg = f'[tools]\nenabled = ["list_apps", "{n}"]\n'
        if n in ("get_clipboard", "set_clipboard"):
            cfg += "clipboard = true\n"
        if n == "get_notifications":
            cfg += "[notifications]\nenabled = true\n"
        per[n] = ask("Go.", server(binary, cfg, home)) - res["only_list_apps"]
    res["per_tool"] = per
    json.dump(res, open(out, "w"), indent=1)
    print(json.dumps(res, indent=1))


def texts(out, pairs):
    base = ask("Text:\n\n")
    res = {"model": MODEL, "texts": {}}
    for p in pairs:
        name, path = p.split("=", 1)
        t = open(path).read()
        n = ask("Text:\n\n" + t) - base
        res["texts"][name] = {"tokens": n, "chars": len(t), "chars_per_token": round(len(t) / max(n, 1), 3)}
        print(name, res["texts"][name], flush=True)
    json.dump(res, open(out, "w"), indent=1)


if __name__ == "__main__":
    if sys.argv[1] == "tools":
        tools(sys.argv[2], sys.argv[3], sys.argv[4:])
    else:
        texts(sys.argv[2], sys.argv[3:])
