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
   is `<index> <role> "<name>" [flags] [actions]`), plus a screenshot when one
   adds information (the first view of a window, a big change, or custom-drawn
   UI). Pass `screenshot: true` when you need to see the pixels anyway. The
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
3. **Re-check.** Mutating tools already append "state after the action" showing
   what changed, so you usually don't need a separate `get_app_state`. When you
   do call it, after the first call it returns a **diff** (`+ added`,
   `~ changed`, `- removed`); pass `disable_diff: true` for the full tree.

## Screens you've already seen

Every view is labelled with a screen number. Trust these labels — they save
you from re-reading and re-analysing:

- `screen #N (new)` — you haven't seen this one; read the tree (and the
  screenshot, if attached).
- `screen #N (seen before)` / "back on screen #N" — you were here earlier (you
  went back a page, a dialog closed, a panel reopened). **Don't re-analyse it:**
  your earlier understanding of screen #N still holds, its element indices are
  exactly the ones you saw then, and your earlier screenshot of it still
  applies. Only the listed changes (if any) are new.
- When an action opens a dialog or menu, the report after it switches to that
  window automatically ("now on screen #N (new), window …"); act on it, and
  when it closes you'll be told which screen you're back on.
- "Screenshot: unchanged … not re-sent" means the picture you already have is
  current. Pass `screenshot: true` only if you truly need a fresh image (for
  example, you no longer have the earlier one).
- An index from a screen that is gone is refused ("unknown element_index")
  rather than hitting something else — call `get_app_state` and use the
  current numbers.

Use `list_apps` to find the exact app, and `launch_app` to start one that isn't
running (then `get_app_state`).

## Helpers that save turns

- `find_element(app, role/name/text)` — get just the elements you need with
  their indices, instead of reading the whole tree.
- `wait_for(app, role/name/text, state, timeout_ms)` — after something that
  takes time (loading, a dialog opening), wait for the element instead of
  polling `get_app_state` yourself.
- `batch(steps=[{tool, arguments}, …])` — run several actions in one call
  (e.g. focus a field, type, then press a button).
- `screenshot(mode)` — capture the `full` screen, a `region` (x/y/width/height),
  or a `window`; add `annotate: true` on a window to see each element's index
  drawn on the image.
- `get_clipboard` / `set_clipboard` — move text between apps (set it, then
  `press_key` "cmd+v" / "ctrl+v").

## Rules

- **Prefer `element_index` over `x`/`y` coordinates.** Coordinates (in
  screenshot pixels) are a fallback for canvases and custom-drawn UI.
- **Indices are per-turn.** If an action fails with "unknown element_index",
  call `get_app_state` again and use the new numbers.
- **The first use of an app may ask the user for approval.** If access is
  denied, don't retry — ask the user how to proceed.
- **Sensitive apps are blocked by default** — terminals, password managers,
  OS security & login prompts, and the agent's own app. The user can allow
  these in their settings; until they do, don't try to drive them — ask the
  user to do that step or to enable it.
- **Consequential actions may ask for a second confirmation** (Send, Delete,
  Pay …), in your client or on the user's screen; the action waits until the
  user answers. Still pause and confirm intent yourself before anything that
  sends, purchases, deletes, or changes important data.
- The user watches you work through an on-screen indicator (your own cursor,
  a glow and a status label); their real mouse is never moved, and it is not
  in your screenshots, so ignore it.
- Action results are checked for you. If one says "Nothing on screen
  changed after it", look (`get_app_state` with `screenshot: true`) before
  trying again; don't just repeat it. A note that a value or typed text
  didn't take means check the field before going on.
- **The user can stop you at any moment** (an emergency stop key). If a call
  fails saying the user stopped the agent, stop: don't retry or work around
  it; ask the user what to do.
- If an action fails because the user is using the mouse or keyboard, they
  are busy — wait a little or ask; don't hammer it.
- Password fields, card numbers and codes are masked (`••••`) and blacked
  out of screenshots on purpose. Don't try to read them another way; ask the
  user if you need such a value.
- Keep tasks narrow; when the tree is ambiguous, ask for a screenshot
  (`get_app_state` with `screenshot: true`, or the `screenshot` tool).
