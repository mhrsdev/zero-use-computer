#!/usr/bin/env python3
"""Turn the raw run directories into per-run metrics and an arm summary.

  analyze.py RESULTS_DIR

RESULTS_DIR/raw/<arm>-<n>/ holds what run_one.sh wrote (or the committed copy,
where stream.jsonl has its images replaced by size and hash). Writes
RESULTS_DIR/runs.json (one record per run), RESULTS_DIR/summary.md (tables),
and RESULTS_DIR/transcripts/<arm>-<n>.jsonl: the stream with image data
replaced by its size and hash, small enough to keep in the repository.

Measured vs estimated:
  - model tokens are EXACT: the per-model usage the API returned, as the
    Claude CLI sums it in its final `result` event (modelUsage), with the
    per-request input from each assistant event's usage;
  - tool-result tokens are ESTIMATED with the repository's own formula
    (text = characters / 4, image = width x height / 750), the method behind
    the earlier 46k -> 6.8k figure, so the two can be compared.
"""
import base64
import hashlib
import json
import re
import statistics
import struct
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
BOXES = json.loads((HERE / "boxes.json").read_text())
CANVAS = (232, 147, 1180, 840)  # Dia's drawing area on screen (x0, y0, x1, y1)
MAIN_MODEL = "claude-sonnet-5-5"

FEATURES = {
    "ocr_auto": "line(s) of text were read off the screen",
    "ocr_asked": "more line(s) of text off the screen",
    "ocr_unavailable": "[Text recognition unavailable",
    "shot_deduped": ["Screenshot: unchanged since you last saw it", "Screenshot: unchanged, not re-sent",
                     "not re-sent (mode=full sends it anyway)"],
    "shot_changed_part": ["only the part that changed", "changed part only",
                          "only the part of the screen that changed"],
    "shot_not_attached": "Screenshot: not attached",
    "shot_overview": ["(An overview;", "(overview)"],
    "screen_seen_before": "(seen before)",
    "change_report": "State after the action:",
}


def png_size(data: bytes):
    if data[:8] == b"\x89PNG\r\n\x1a\n":
        return struct.unpack(">II", data[16:24])
    if data[:2] == b"\xff\xd8":  # JPEG: walk to the SOF marker
        i = 2
        while i < len(data):
            marker, length = data[i + 1], struct.unpack(">H", data[i + 2:i + 4])[0]
            if 0xC0 <= marker <= 0xCF and marker not in (0xC4, 0xC8, 0xCC):
                h, w = struct.unpack(">HH", data[i + 5:i + 9])
                return w, h
            i += 2 + length
    return 0, 0


def image_info(data: str):
    """Size and hash of a base64 image, or of the placeholder strip_images
    left for it (so the committed, image-free streams give the same numbers)."""
    m = re.fullmatch(r"<(\d+) bytes, (\d+)x(\d+), sha256 (\w+)>", data)
    if m:
        return {"bytes": int(m.group(1)), "w": int(m.group(2)), "h": int(m.group(3)), "sha256": m.group(4)}
    raw = base64.b64decode(data)
    w, h = png_size(raw)
    return {"w": w, "h": h, "bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()[:16]}


# What the committed transcripts keep: the conversation, the CLI's result and
# post-turn summary, and the parts of the init event that describe the setup.
# Account and rate-limit events and local CLI paths are left out.
INIT_KEYS = ("type", "subtype", "model", "tools", "mcp_servers", "permissionMode",
             "claude_code_version", "cwd", "_t")


def keep_event(ev):
    t = ev.get("type")
    if t == "system" and ev.get("subtype") == "init":
        return {k: ev[k] for k in INIT_KEYS if k in ev}
    if t in ("assistant", "user", "result") or (t == "system" and ev.get("subtype") == "post_turn_summary"):
        return ev
    return None


def strip_images(o):
    """Replace base64 image data anywhere in an event (the CLI also keeps a
    copy under tool_use_result) by its size and hash."""
    if isinstance(o, dict):
        if o.get("type") == "base64" and isinstance(o.get("data"), str) and not o["data"].startswith("<"):
            raw = base64.b64decode(o["data"])
            w, h = png_size(raw)
            return {**o, "data": f"<{len(raw)} bytes, {w}x{h}, sha256 {hashlib.sha256(raw).hexdigest()[:16]}>"}
        return {k: strip_images(v) for k, v in o.items()}
    if isinstance(o, list):
        return [strip_images(v) for v in o]
    return o


def where(x, y):
    for label, (l, t, r, b) in BOXES.items():
        if l <= x <= r and t <= y <= b:
            return "target" if label == "Cache" else f"other box: {label}"
    if CANVAS[0] <= x <= CANVAS[2] and CANVAS[1] <= y <= CANVAS[3]:
        return "canvas, no box"
    return "outside canvas (menus, toolbars)"


def has(text, pat):
    pats = pat if isinstance(pat, list) else [pat]
    return any(p in text for p in pats)


def analyze_run(d: Path, transcripts: Path):
    arm, n = d.name.rsplit("-", 1)
    rec = {"run": d.name, "arm": arm, "n": int(n)}
    verify = json.loads((d / "verify.json").read_text())
    rec["success"] = verify["success"]
    rec["problems"] = verify["problems"]
    rec["wrong_box_deleted"] = verify.get("wrong_deleted", [])
    start, end = float((d / "start").read_text()), float((d / "end").read_text())
    rec["wall_s"] = round(end - start, 2)
    rec["claude_exit"] = int((d / "claude.exit").read_text().strip() or -1)

    calls = {}  # message id -> input tokens of that request
    tool_uses, results, final_text = [], {}, ""
    result_ev = None
    slim = []
    for line in (d / "stream.jsonl").read_text().splitlines():
        ev = json.loads(line)
        t = ev.get("type")
        if t == "assistant":
            m = ev["message"]
            if m.get("model") == MAIN_MODEL or m.get("model") is None:
                u = m.get("usage") or {}
                calls[m["id"]] = (u.get("input_tokens", 0) + u.get("cache_read_input_tokens", 0)
                                  + u.get("cache_creation_input_tokens", 0))
            for c in m["content"]:
                if c["type"] == "tool_use":
                    tool_uses.append({"id": c["id"], "name": c["name"].removeprefix("mcp__cu__"),
                                      "input": c["input"], "t": ev["_t"]})
                elif c["type"] == "text":
                    final_text = c["text"]
        elif t == "user":
            for c in ev["message"]["content"]:
                if isinstance(c, dict) and c.get("type") == "tool_result":
                    blocks = c["content"] if isinstance(c["content"], list) else [
                        {"type": "text", "text": str(c["content"])}]
                    text, images = "", []
                    for b in blocks:
                        if b["type"] == "text":
                            text += b["text"]
                        elif b["type"] == "image":
                            images.append(image_info(b["source"]["data"]))
                    results[c["tool_use_id"]] = {"text": text, "images": images,
                                                 "is_error": c.get("is_error", False), "t": ev["_t"]}
        elif t == "result":
            result_ev = ev
        kept = keep_event(ev)
        if kept is not None:
            slim.append(strip_images(kept))
    (transcripts / f"{d.name}.jsonl").write_text("".join(json.dumps(e) + "\n" for e in slim))

    usage = (result_ev or {}).get("modelUsage", {})
    mu = usage.get(MAIN_MODEL, {})
    rec["model_input_tokens"] = (mu.get("inputTokens", 0) + mu.get("cacheReadInputTokens", 0)
                                 + mu.get("cacheCreationInputTokens", 0))
    rec["model_output_tokens"] = mu.get("outputTokens", 0)
    rec["model_cache_read"] = mu.get("cacheReadInputTokens", 0)
    rec["model_cache_write"] = mu.get("cacheCreationInputTokens", 0)
    rec["model_cost_usd"] = round(mu.get("costUSD", 0.0), 4)
    rec["api_requests"] = len(calls)
    rec["per_request_input"] = list(calls.values())
    rec["per_request_input_sum"] = sum(calls.values())
    rec["first_request_input"] = next(iter(calls.values()), 0)
    other = {k: v for k, v in usage.items() if k != MAIN_MODEL}
    rec["harness_side_model_tokens"] = {k: {"in": v.get("inputTokens", 0) + v.get("cacheReadInputTokens", 0)
                                           + v.get("cacheCreationInputTokens", 0),
                                           "out": v.get("outputTokens", 0)} for k, v in other.items()}
    rec["cli_duration_s"] = round((result_ev or {}).get("duration_ms", 0) / 1000, 2)
    rec["cli_api_s"] = round((result_ev or {}).get("duration_api_ms", 0) / 1000, 2)
    rec["num_turns"] = (result_ev or {}).get("num_turns")
    rec["result_subtype"] = (result_ev or {}).get("subtype")
    rec["final_text"] = final_text.strip()[:200]
    rec["claimed_done"] = final_text.strip().startswith("DONE")
    rec["false_claim"] = rec["claimed_done"] and not rec["success"]

    # Tool calls, images, estimated tokens, features.
    by_tool, est_text, est_img, n_img, img_bytes = {}, 0, 0, 0, 0
    feats = {k: 0 for k in FEATURES}
    looks = []
    clicks = []
    errors = 0
    for tu in tool_uses:
        by_tool[tu["name"]] = by_tool.get(tu["name"], 0) + 1
        r = results.get(tu["id"], {"text": "", "images": [], "is_error": False})
        errors += bool(r["is_error"])
        est_text += -(-len(r["text"]) // 4)
        for im in r["images"]:
            n_img += 1
            img_bytes += im["bytes"]
            est_img += -(-im["w"] * im["h"] // 750)
        for k, pat in FEATURES.items():
            feats[k] += has(r["text"], pat)
        if tu["name"] == "get_app_state":
            looks.append({"image": bool(r["images"]), "size": [(i["w"], i["h"]) for i in r["images"]],
                          "args": tu["input"]})
        if tu["name"] in ("click", "double_click", "right_click", "drag"):
            m = re.search(r"Clicked the point at \((-?\d+), (-?\d+)\)", r["text"])
            if m:
                x, y = int(m.group(1)), int(m.group(2))
                clicks.append({"x": x, "y": y, "where": where(x, y), "args": tu["input"]})
            else:
                clicks.append({"x": None, "y": None, "where": "element/other: " + r["text"][:80],
                               "args": tu["input"]})
    rec["tool_calls"] = len(tool_uses)
    rec["tool_calls_by_name"] = by_tool
    rec["tool_errors"] = errors
    rec["images_to_model"] = n_img
    rec["image_bytes"] = img_bytes
    rec["est_tool_result_tokens"] = est_text + est_img
    rec["est_tool_text_tokens"] = est_text
    rec["est_tool_image_tokens"] = est_img
    rec["features"] = feats
    rec["get_app_state"] = looks
    rec["clicks"] = clicks
    rec["clicks_on_target"] = sum(c["where"] == "target" for c in clicks)
    rec["clicks_on_other_box"] = sum(c["where"].startswith("other box") for c in clicks)
    rec["clicks_canvas_no_box"] = sum(c["where"] == "canvas, no box" for c in clicks)
    rec["clicks_outside_canvas"] = sum(c["where"].startswith("outside") for c in clicks)
    presses = (d / "clicks.jsonl").read_text().splitlines() if (d / "clicks.jsonl").exists() else []
    rec["physical_button_presses"] = len(presses)
    # The starting state: canvas invisible to accessibility, same picture.
    at = json.loads((d / "atspi.json").read_text()) if (d / "atspi.json").exists() else {}
    rec["atspi_raw_nodes"] = at.get("raw_nodes")
    rec["atspi_drawing_area_children"] = [x["children"] for x in at.get("drawing_areas", [])]
    rec["atspi_box_label_hits"] = [h for h in at.get("label_hits", [])
                                   if h["role"] not in ("menu item",)]
    png = d / "initial.png"
    rec["initial_png_sha256"] = (hashlib.sha256(png.read_bytes()).hexdigest()[:16] if png.exists()
                                 else (d / "initial.sha256").read_text().strip())
    return rec


def stats(xs):
    xs = [x for x in xs if x is not None]
    if not xs:
        return "–"
    if len(xs) == 1:
        return f"{xs[0]:,.0f}"
    return f"{statistics.mean(xs):,.0f} (median {statistics.median(xs):,.0f}, {min(xs):,.0f}–{max(xs):,.0f})"


def stats_f(xs):
    xs = [x for x in xs if x is not None]
    if not xs:
        return "–"
    return f"{statistics.mean(xs):.1f} (median {statistics.median(xs):.1f}, {min(xs):.1f}–{max(xs):.1f})"


def main():
    root = Path(sys.argv[1])
    transcripts = root / "transcripts"
    transcripts.mkdir(exist_ok=True)
    runs = [analyze_run(d, transcripts) for d in sorted((root / "raw").iterdir(), key=lambda p: (p.name.rsplit("-", 1)[0], int(p.name.rsplit("-", 1)[1])))
            if (d / "verify.json").exists()]
    (root / "runs.json").write_text(json.dumps(runs, indent=1))

    arms = ["baseline", "current"]
    rows = []

    def row(name, fn):
        rows.append(f"| {name} | " + " | ".join(fn([r for r in runs if r["arm"] == a]) for a in arms) + " |")

    row("runs", lambda rs: str(len(rs)))
    row("**task success** (verified from the saved file)", lambda rs: f"**{sum(r['success'] for r in rs)}/{len(rs)}**")
    row("claimed DONE but failed", lambda rs: str(sum(r["false_claim"] for r in rs)))
    row("runs with a wrong box deleted", lambda rs: str(sum(bool(r["wrong_box_deleted"]) for r in rs)))
    row("clicks on the target box (total)", lambda rs: str(sum(r["clicks_on_target"] for r in rs)))
    row("clicks on another box (total)", lambda rs: str(sum(r["clicks_on_other_box"] for r in rs)))
    row("clicks on empty canvas (total)", lambda rs: str(sum(r["clicks_canvas_no_box"] for r in rs)))
    row("clicks outside the canvas (total)", lambda rs: str(sum(r["clicks_outside_canvas"] for r in rs)))
    row("model input tokens per run, EXACT", lambda rs: stats([r["model_input_tokens"] for r in rs]))
    row("model output tokens per run, EXACT", lambda rs: stats([r["model_output_tokens"] for r in rs]))
    row("model input tokens per *successful* run, EXACT", lambda rs: stats([r["model_input_tokens"] for r in rs if r["success"]]))
    row("first request (system prompt + tool definitions + task), EXACT", lambda rs: stats([r["first_request_input"] for r in rs]))
    row("model requests per run", lambda rs: stats_f([r["api_requests"] for r in rs]))
    row("tool-result tokens per run, ESTIMATED (chars/4 + w·h/750)", lambda rs: stats([r["est_tool_result_tokens"] for r in rs]))
    row("  of which images, ESTIMATED", lambda rs: stats([r["est_tool_image_tokens"] for r in rs]))
    row("images sent to the model per run", lambda rs: stats_f([r["images_to_model"] for r in rs]))
    row("tool calls per run", lambda rs: stats_f([r["tool_calls"] for r in rs]))
    row("task time per run, s (wall clock, agent start to exit)", lambda rs: stats_f([r["wall_s"] for r in rs]))
    row("  of which model API time, s", lambda rs: stats_f([r["cli_api_s"] for r in rs]))
    row("model cost per run, USD (list price, depends on prompt-cache hits)", lambda rs: f"{statistics.mean([r['model_cost_usd'] for r in rs]):.3f}" if rs else "–")

    feat_rows = []
    for k in FEATURES:
        feat_rows.append(f"| {k} | " + " | ".join(
            f"{sum(r['features'][k] for r in runs if r['arm'] == a)} results in {sum(bool(r['features'][k]) for r in runs if r['arm'] == a)} runs"
            for a in arms) + " |")
    tools = sorted({t for r in runs for t in r["tool_calls_by_name"]})
    tool_rows = [f"| {t} | " + " | ".join(str(sum(r["tool_calls_by_name"].get(t, 0) for r in runs if r["arm"] == a)) for a in arms) + " |" for t in tools]

    per_run = ["| run | success | model in (exact) | model out (exact) | requests | tool calls | images | est. tool-result tokens | wall s | clicks: target / other box / empty canvas / outside |",
               "|---|---|---|---|---|---|---|---|---|---|"]
    for r in runs:
        per_run.append(f"| {r['run']} | {'yes' if r['success'] else 'NO: ' + '; '.join(r['problems'])} | {r['model_input_tokens']:,} | {r['model_output_tokens']:,} | {r['api_requests']} | {r['tool_calls']} | {r['images_to_model']} | {r['est_tool_result_tokens']:,} | {r['wall_s']} | {r['clicks_on_target']} / {r['clicks_on_other_box']} / {r['clicks_canvas_no_box']} / {r['clicks_outside_canvas']} |")

    hashes = {r["initial_png_sha256"] for r in runs}
    checks = [
        f"- initial screenshots identical across all runs: {'yes' if len(hashes) == 1 else 'NO, ' + str(len(hashes)) + ' distinct'}",
        f"- Dia's drawing area had 0 accessible children in every run: {'yes' if all(set(r['atspi_drawing_area_children']) == {0} for r in runs) else 'NO'}",
        f"- no box label in any accessible name/description/text (besides the unrelated 'Database' menu item): {'yes' if not any(r['atspi_box_label_hits'] for r in runs) else 'NO'}",
        f"- raw AT-SPI nodes for Dia: {sorted({r['atspi_raw_nodes'] for r in runs})}",
        f"- per-request input sums equal the CLI's modelUsage totals: {'yes' if all(r['per_request_input_sum'] == r['model_input_tokens'] for r in runs) else 'NO (see runs.json)'}",
    ]

    md = ["## Results by arm", "", "| | baseline (screenshot every call) | current (zero-use-computer defaults) |", "|---|---|---|", *rows, "",
          "## When the savings features fired (number of tool results showing each)", "", "| feature | baseline | current |", "|---|---|---|", *feat_rows, "",
          "## Tool calls (totals over all runs)", "", "| tool | baseline | current |", "|---|---|---|", *tool_rows, "",
          "## Every run", "", *per_run, "", "## Consistency checks", "", *checks, ""]
    (root / "summary.md").write_text("\n".join(md))
    print("\n".join(md))


if __name__ == "__main__":
    main()
