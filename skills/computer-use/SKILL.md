---
name: computer-use
description: >-
  Operate desktop apps (macOS, Windows, Linux) through their accessibility
  tree and screenshots with the computer-use tools. Use when a task needs a
  GUI app that has no API or CLI path.
---

# Computer use

Load `computer-use-security` too: nothing asks the user for permission, so
its rules are yours to follow. For design work (images, logos, 3D, plans in
Photoshop, Paint, Blender, Revit…) also load `computer-use-design`.

## The loop

1. `get_app_state(app)`: the app's numbered tree (`<index> <role> "<name>"
   [flags] [actions]`), plus a screenshot when it adds something.
   Look-alike siblings come as records: `7 × button:` then `2 "Select" ·
   3 "Pen"`, or `3 × list item › text:` then `10 (selected) › 11
   "General"`; a table's cells a row a line under its column names. Each
   number is an element_index; the header gives the roles. A text field
   is editable unless it says read-only.
2. Act by `element_index`: `click`, `set_value` (fields, sliders,
   checkboxes; prefer it over typing), `type_text` (`\n` presses Return),
   `press_key` (`"Return"`, `"cmd+s"`: Cmd on a Mac, Ctrl elsewhere),
   `perform_secondary_action` (an entry of `actions=[…]`), `select_text`,
   `scroll`, `drag`, `draw`.
3. Read the "state after the action" each action returns; call
   `get_app_state` only when you need more. Later calls return a diff, or
   one line when nothing changed.

`list_apps` finds an app; `launch_app` opens one by name, or a web address
(`https://…`) in the default browser. If `find_tools` is in your tools,
the others (design, windows, scripts, clipboard…) are found with it.

## Rules

- Prefer `element_index` to `x`/`y` (screenshot pixels, for custom-drawn UI).
- An index unknown or from a screen that's gone is refused: call
  `get_app_state` and use the new numbers.
- "Nothing on screen changed" or "the field doesn't show the new text yet":
  look before acting again. Never type the same text twice without looking.
- "The picture changed … where the tree reports no change": trust the
  screenshot there.
- Input refused, app not brought to the front, user busy: don't work around
  it; wait or ask the user.
- The user stopped the agent (stop key): stop and ask how to go on. The stop
  key is not working: tell the user at once.
- Masked data (`••••`) is masked on purpose: ask the user instead.

## Spend few tokens

Every result stays in the conversation, so ask only for what you need:

- Search with `find_element(role/name/text)` (`offset` for more) instead of
  re-reading a tree; `get_app_state(within=index)` shows one part. Folded
  lists (`[… N more "list item" folded]`) are found by `find_element`;
  `get_app_state(max_tokens=0)` returns a whole tree only when you must.
- Leave `screenshot` unset: one comes when it adds something, and a picture
  you already have isn't sent again. `screenshot: true` for a fresh look;
  `screenshot(app, element_index)` zooms into one element.
- `wait_for` instead of polling; `batch` for a known sequence of steps;
  one `script` for work that repeats or branches.
- Trust `screen #N (seen before)`: what you learnt about it still holds.

## The decision model (optional)

If the user set one up, `decide` answers judgments fast and cheaply: many
items at once (`decide(question, items=[...])`), a condition on screen
(`decide(app, question)`, `wait_for(app, until=…)`), the element a
description means (`decide(app, pick=…)`). When there is none and a task
has many such judgments, tell the user once: "Press Ctrl+Alt+J
(Ctrl+Option+J on a Mac) to add a decision model (like Jev); your key stays
out of the chat". Then carry on without it. Details, and setting it up from
the chat (only when the user asks): [reference/decisions.md](reference/decisions.md).

## More, only when you need it

- [reference/screens.md](reference/screens.md): screen numbers, diffs,
  records, dialogs, partial and overview screenshots.
- [reference/tools.md](reference/tools.md): `find_element`, `wait_for`,
  `batch`, `screenshot` modes (grid, cells, zoom), `locate` and `snap` for
  exact aiming, `window`, clipboard, notifications, `find_tools`.
- [reference/special-content.md](reference/special-content.md): apps with
  little in their tree (OCR text), text in any script or direction, masked
  data, the on-screen indicator.
- [reference/decisions.md](reference/decisions.md): `decide` (questions,
  items, pick, setup), `wait_for(until)`, decisions in scripts.
- [reference/scripts.md](reference/scripts.md): `script`, and saved
  scripts as new tools.
- [reference/drawing.md](reference/drawing.md): `draw`, `design`, `scene`
  and `trace_image`.
- Shortcuts and quirks of common apps:
  [browsers](reference/apps/browsers.md), [office](reference/apps/office.md),
  [mail and chat](reference/apps/mail-and-chat.md),
  [code editors](reference/apps/vscode.md),
  [image editors](reference/apps/image-editors.md).
