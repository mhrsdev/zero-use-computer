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
   `scroll`, `drag`, `draw`. `click(name="Save")` works when one element
   has that name.
3. Read the "state after the action" each action returns; call
   `get_app_state` only when you need more. Later calls return a diff, or
   one line when nothing changed.

`list_apps` finds an app; `launch_app` opens one by name (and returns its
first state), or a web address (`https://…`) in the default browser. With
`find_tools` in your tools, the others (`draw`, `design`, `scene`,
`locate`, `window`, `script`, clipboard…) are there too: `find_tools(name=
"locate")` shows a tool's arguments, `use_tool(name, arguments)` runs it.

## Rules

- Prefer `element_index` to `x`/`y` (screenshot pixels, for custom-drawn UI).
- An index unknown or from a screen that's gone is refused: call
  `get_app_state` and use the new numbers.
- "Nothing on screen changed" or "the field doesn't show the new text yet":
  look before acting again. Never type the same text twice without looking.
- "The picture changed … where the tree reports no change": trust the
  screenshot there.
- Input refused, the app not brought to the front, the user busy or the
  stop key: security rule 8. Masked data (`••••`): rule 5.

## Spend few tokens

Every result stays in the conversation, so ask only for what you need:

- Search with `find_element(role/name/text)` (`offset` for more) instead of
  re-reading a tree; `get_app_state(within=index)` shows one part. Folded
  lists (`[… N more "list item" folded]`) are found by `find_element`;
  `get_app_state(max_tokens=0)` returns a whole tree only when you must.
- Leave `screenshot` unset: one comes when it adds something, and a picture
  you already have isn't sent again. `screenshot: true` for a fresh look;
  `screenshot(app, element_index)` zooms into one element.
- Steps you already know go in one `batch` of lines: `["set 4 \"Ada\"",
  "click \"Japan\"", "click \"Save\""]` (`click 12`, `double 12`, `type
  "text"`, `key cmd+s`, `scroll 7 down`, `look`…); it stops when a window
  comes up that a step didn't `expect`, and ends with one report.
- `expect` on an action (`"dialog"`, `"change"`, `"value"`, `"gone"`, or a
  text to see) waits for it and says confirmed, not seen or uncertain:
  look before repeating anything not confirmed.
- `get_app_state(about="shipping address")` shows just the parts about
  that. `wait_for` instead of polling; one `script` for work that repeats
  or branches.
- Trust `screen #N (seen before)`: what you learnt about it still holds.

## The decision model (optional)

`decide` is in your tools when the user set up a fast decision model:
hand it judgments over many items, conditions on screen and "which
element is…" ([reference/decisions.md](reference/decisions.md)). Without
one, a task with many such judgments: tell the user once that Ctrl+Alt+J
(Ctrl+Option+J on a Mac) adds one, then carry on.

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
