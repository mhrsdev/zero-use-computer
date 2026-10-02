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
2. Act by `element_index`: `click`, `set_value` (fields, sliders,
   checkboxes; prefer it over typing), `type_text` (`\n` presses Return),
   `press_key` (`"Return"`, `"cmd+s"`: Cmd on a Mac, Ctrl elsewhere),
   `perform_secondary_action` (an entry of `actions=[…]`), `select_text`,
   `scroll`, `drag`, `draw` (shapes and curves with the mouse held down).
3. Read the "state after the action" each action returns; call
   `get_app_state` only when you need more. Later calls return a diff.

`list_apps` finds an app; `launch_app` opens one by name, or a web address
(`https://…`) in the default browser.

## Rules

- Prefer `element_index` to `x`/`y` (screenshot pixels, for custom-drawn UI).
- An index unknown or from a screen that's gone is refused: call
  `get_app_state` and use the new numbers.
- "Nothing on screen changed" or "the field doesn't show the new text yet":
  look before acting again. Never type the same text twice without looking.
- Input refused, app not brought to the front, user busy: don't work around
  it; wait or ask the user.
- The user stopped the agent (stop key): stop and ask how to go on. The stop
  key is not working: tell the user at once.
- Masked data (`••••`) is masked on purpose: ask the user instead.

## Spend few tokens

Every result stays in the conversation, so ask only for what you need:

- Search with `find_element(role/name/text)` instead of re-reading a tree.
  Very long lists come folded (`[… N more "list item" folded]`):
  find_element finds the folded items too. Only if you really need every
  element at once, `get_app_state(max_tokens=0)` returns the whole tree.
- Leave `screenshot` unset: one comes when it adds something, and a picture
  you already have isn't sent again. Ask `screenshot: true` when you need a
  fresh look; `screenshot(app, element_index)` zooms into one element.
- Don't pass `disable_diff: true` unless a diff confused you.
- Use `wait_for` instead of polling, and `batch` for a known sequence of
  steps (then call `get_app_state`: a batch shows one line per step).
- Work that repeats or branches (every row of a table, retry until
  something appears, positions to compute) is one `script` call instead
  of many turns.
- Trust `screen #N (seen before)`: what you learnt about it still holds.

## Decide fast: the decision model

`decide` hands judgments to a fast decision model (TypeSafe's Jev, or a
small OpenAI-compatible model): yes/no (a probability), one of some
options, or a score on a scale. It is quicker and cheaper than reading and
judging yourself:

- many things to judge (reviews, search results, rows, messages):
  `decide(question, items=[...])` judges each at once and sums them up;
- a condition on screen: `decide(app, question)`, or wait for it with
  `wait_for(app, until="Have the results loaded?")`;
- the element a description means: `decide(app, pick="the add-to-cart
  button of the cheapest one")` returns its `element_index`;
- in scripts: `ask`, `choose`, `score`, `decide_each`.

When none is set up and the task has many such judgments, tell the user
once, briefly: "Press Ctrl+Alt+J to add a decision model (like Jev); it
makes this faster, and your key stays out of the chat". Then carry on
without it; don't ask again. `decide(setup="open")` opens that page for
them. Set it from the chat (`setup={provider, base_url, model, api_key}`)
only when the user themself asks you to and gives the details; say that a
key typed in the chat stays in its history. Details:
[reference/decisions.md](reference/decisions.md).

## More, only when you need it

- [reference/screens.md](reference/screens.md): screen numbers, diffs,
  dialogs, partial and overview screenshots.
- [reference/tools.md](reference/tools.md): `find_element`, `wait_for`,
  `batch`, `screenshot` modes (grid, cells, zoom), `locate` and `snap` for
  exact aiming, `window`, clipboard, notifications.
- [reference/special-content.md](reference/special-content.md): apps with
  little in their tree (OCR text), text in any script or direction, masked
  data, the on-screen indicator.
- [reference/decisions.md](reference/decisions.md): `decide` (questions,
  items, pick, setup), `wait_for(until)`, and decisions in scripts.
- [reference/scripts.md](reference/scripts.md): `script`: a small
  program for what the tools can't do in one call (loops over tool calls,
  maths, data from files or the web, pictures on a graph-paper page), and
  saved scripts as new tools.
- [reference/drawing.md](reference/drawing.md): `draw`: lines, shapes,
  stars, curves, function plots with axes, repeats, solid fills,
  previews on named cells, planning with `design` and `scene`, and
  copying a picture with `trace_image`.
- Shortcuts and quirks of common apps:
  [browsers](reference/apps/browsers.md), [office](reference/apps/office.md),
  [mail and chat](reference/apps/mail-and-chat.md),
  [code editors](reference/apps/vscode.md),
  [image editors](reference/apps/image-editors.md).
