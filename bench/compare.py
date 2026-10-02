#!/usr/bin/env python3
"""Compare scripted benchmark runs of several releases, section by section.

    bench/compare.py DIR LABEL [LABEL...] [--base LABEL] [--out FILE.md]

DIR holds one folder per label (agent_bench --out DIR --label LABEL), each
with its JSON lines. Every figure is a scripted estimate (text: 4
characters a token, an image: width x height / 750; input: no prompt cache,
one call a turn), the median over a label's runs.

The tokens a task sends the model are split into what goes with every
request (the fixed prefix: the benchmark's framing, the server's MCP
instructions, the skills, the tool definitions; sent once per turn) and the
conversation (the calls' arguments and results, read again on every later
turn).
"""
import glob
import json
import os
import statistics
import sys

SECTIONS = ["framing", "instructions", "skills", "tools"]


def load(root, label):
    runs = []
    for f in sorted(glob.glob(os.path.join(root, label, "*.jsonl"))):
        with open(f) as fh:
            runs.extend(json.loads(line) for line in fh if line.strip())
    return runs


def med(values):
    return statistics.median(values) if values else 0


def per_scenario(runs):
    out = {}
    for sc in dict.fromkeys(r["scenario"] for r in runs):
        rs = [r for r in runs if r["scenario"] == sc]
        e = [r["estimated"] for r in rs]
        turns = med([r["calls"] + 1 for r in rs])
        prefix = rs[0]["estimated"]["prefix"]
        fixed = rs[0]["estimated"]["fixed"]
        input_ = med([x["input_no_cache"] for x in e])
        out[sc] = {
            "ok": sum(r["success"] for r in rs),
            "n": len(rs),
            "calls": med([r["calls"] for r in rs]),
            "images": med([r["images"] for r in rs]),
            "results": med([x["results"] for x in e]),
            "result_text": med([x["result_text"] for x in e]),
            "result_images": med([x["result_images"] for x in e]),
            "args": med([x["args"] for x in e]),
            "input": input_,
            "seconds": med([r["seconds"] for r in rs]),
            # Sent with every request, over the task.
            "sections": {k: prefix[k] * turns for k in SECTIONS},
            "fixed_total": fixed * turns,
            # The conversation: earlier calls read again on each turn.
            "history": input_ - fixed * turns,
        }
    return out


def pct(new, old):
    if not old:
        return "–"
    d = (new - old) / old * 100
    return f"{d:+.0f}%"


def main(argv):
    out_file = None
    base = None
    args = []
    it = iter(argv)
    for a in it:
        if a == "--out":
            out_file = next(it)
        elif a == "--base":
            base = next(it)
        else:
            args.append(a)
    if len(args) < 2:
        print(__doc__)
        return 2
    root, labels = args[0], args[1:]
    data = {l: load(root, l) for l in labels}
    data = {l: r for l, r in data.items() if r}
    labels = list(data)
    base = base or labels[-1]
    scen = {l: per_scenario(r) for l, r in data.items()}
    md = []
    w = md.append
    w("## The fixed prefix: sent with every request\n")
    w("| | " + " | ".join(labels) + " |")
    w("|---|" + "---|" * len(labels))
    first = {l: data[l][0]["estimated"] for l in labels}
    for k in SECTIONS:
        w(f"| {k} | " + " | ".join(str(first[l]["prefix"][k]) for l in labels) + " |")
    w("| **total** | " + " | ".join(f"**{first[l]['fixed']}**" for l in labels) + " |")
    w("| tools listed | " + " | ".join(str(first[l]["prefix"]["tool_count"]) for l in labels) + " |")
    w("")
    common = [s for s in scen[labels[0]] if all(
        s in scen[l] and scen[l][s]["ok"] == scen[l][s]["n"] for l in labels)]
    for title, keys in [("every scenario all of them finish", common)]:
        if not keys:
            continue
        w(f"## A task's tokens by part: {title} ({', '.join(keys)}), summed\n")
        w("| | " + " | ".join(labels) + " |")
        w("|---|" + "---|" * len(labels))

        def total(l, f):
            return sum(f(scen[l][s]) for s in keys)

        rows = [(f"{k} (x turns)", lambda x, k=k: x["sections"][k]) for k in SECTIONS]
        rows += [
            ("**prefix, all turns**", lambda x: x["fixed_total"]),
            ("results: text", lambda x: x["result_text"]),
            ("results: pictures", lambda x: x["result_images"]),
            ("arguments the model wrote", lambda x: x["args"]),
            ("conversation read again (history)", lambda x: x["history"]),
            ("**input, no cache**", lambda x: x["input"]),
            ("tool calls", lambda x: x["calls"]),
            ("pictures", lambda x: x["images"]),
            ("seconds (tools only)", lambda x: x["seconds"]),
        ]
        for name, f in rows:
            vals = []
            for l in labels:
                v = total(l, f)
                vals.append(f"{v:.1f}" if isinstance(v, float) and not v.is_integer() else f"{int(v)}")
            w(f"| {name} | " + " | ".join(vals) + " |")
        w("")
        w(f"Against {base}:\n")
        w("| | " + " | ".join(l for l in labels if l != base) + " |")
        w("|---|" + "---|" * (len(labels) - 1))
        for name, f in [
            ("prefix per request", None),
            ("input, no cache", lambda x: x["input"]),
            ("tool results", lambda x: x["results"]),
            ("tool calls", lambda x: x["calls"]),
            ("seconds", lambda x: x["seconds"]),
        ]:
            cells = []
            for l in labels:
                if l == base:
                    continue
                if f is None:
                    cells.append(pct(first[base]["fixed"], first[l]["fixed"]))
                else:
                    cells.append(pct(total(base, f), total(l, f)))
            w(f"| {name} | " + " | ".join(cells) + " |")
        w(f"\n(The change from each release to {base}: negative is less.)\n")
    w("## Each scenario\n")
    w("| scenario | " + " | ".join(labels) + " |")
    w("|---|" + "---|" * len(labels))
    for s in dict.fromkeys(k for l in labels for k in scen[l]):
        cells = []
        for l in labels:
            x = scen[l].get(s)
            if not x:
                cells.append("–")
            elif x["ok"] < x["n"]:
                cells.append(f"fails ({x['ok']}/{x['n']})")
            else:
                cells.append(f"{int(x['input'])} in · {int(x['results'])} res · {int(x['calls'])} calls · {x['seconds']:.1f} s")
        w(f"| {s} | " + " | ".join(cells) + " |")
    w("")
    text = "\n".join(md)
    if out_file:
        with open(out_file, "w") as fh:
            fh.write(text + "\n")
    print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
