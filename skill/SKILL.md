---
name: computer-use
description: >-
  Control desktop apps (macOS, Windows, Linux) through their accessibility tree
  plus screenshots, the way OpenAI's Codex computer use works. Use when a task
  needs a GUI app that has no API or CLI path: clicking, typing, reading on-screen
  state, testing a desktop UI, or a multi-app workflow.
---

# Computer use

You can see and operate graphical desktop apps. Each app is described to you as
a numbered **accessibility tree** plus a **screenshot**; you act on elements by
their number. Prefer this over pixel-hunting — it is precise and works even when
a window is in the background.

## The loop (do this every turn)

1. **`get_app_state(app)` first.** It returns the app's current tree (each line
   is `<index> <role> "<name>" [flags] [actions]`) and a screenshot. The
   element indices it prints are only valid until the **next** `get_app_state`.
2. **Act** on an element by its `element_index`:
   - `click` — press a button, focus a field, open a menu item.
   - `set_value` — replace a text field's contents, set a slider, or set a
     checkbox/switch (`"true"`/`"false"`). Prefer this over typing for fields
     marked `editable`/`settable`.
   - `type_text` — type into the focused element (pass `element_index` to focus
     first). Newlines press Return.
   - `press_key` — a key or shortcut: `"Return"`, `"Escape"`, `"cmd+s"`,
     `"ctrl+shift+t"`, `"Down Down Return"`.
   - `perform_secondary_action` — a non-click action listed for the element in
     `actions=[…]` (e.g. `show_menu`, `increment`, `expand`, `toggle`).
   - `select_text` — select a substring (or all) inside a text element.
   - `scroll` / `drag` — scroll a list/area, or drag between elements/points.
3. **Re-check with `get_app_state`** after actions that change the UI. After the
   first call it returns a **diff** (`+ added`, `~ changed`, `- removed`);
   unchanged elements keep their indices. Pass `disable_diff: true` for the full
   tree again.

Use `list_apps` to find the exact app, and `launch_app` to start one that isn't
running (then `get_app_state`).

## Rules

- **Prefer `element_index` over `x`/`y` coordinates.** Coordinates (in
  screenshot pixels) are a fallback for canvases and custom-drawn UI.
- **Indices are per-turn.** If an action fails with "unknown element_index",
  call `get_app_state` again and use the new numbers.
- **The first use of an app may ask the user for approval.** If access is
  denied, don't retry — ask the user how to proceed.
- **You cannot control** terminals, password managers / OS security & login
  prompts, or the agent's own app. Ask the user to do those steps.
- **Pause before consequential actions** — sending, purchasing, deleting,
  or anything that changes important data. Confirm intent first.
- Keep tasks narrow and check the screenshot when the tree is ambiguous.
