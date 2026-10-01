# Screens, diffs and screenshots

## Screen numbers

Every view is labelled with a screen number:

- `screen #N (new)`: you haven't seen it; read the tree (and the screenshot,
  if one came).
- `screen #N (seen before)` / "back on screen #N": you were here before (you
  went back a page, a dialog closed, a panel reopened). Don't re-analyse it:
  what you learnt still holds, and its element indices are the ones you saw
  then. Only the listed changes are new; if its pixels changed, a new
  screenshot (or the changed part) comes with it.
- When an action opens a dialog or menu, the report after it switches to that
  window ("now on screen #N (new), window …"). When it closes, you're told
  which screen you're back on.

## Trees and diffs

- After the first `get_app_state` of a screen, later calls return a diff:
  `+ added`, `~ changed` (with the old line), `- removed`. Unchanged
  elements keep their indices. `disable_diff: true` gives the whole tree.
- Each result has a generous token budget. A tree over it has its long
  lists folded to the first and last items, never the focused or selected
  one (`[… N more "row" folded; find_element finds them]`), and if still
  too big, it is cut (`[… N more lines not shown]`). Folded and cut
  elements still have indices: `find_element` returns them.
  `get_app_state(max_tokens=0)` returns one whole tree, nothing folded; the
  user can also lower or switch off this shortening in their settings.
- Explanations (what a diff means, what a partial screenshot is) come in
  full the first time and in a short form after that.

## Screenshots

- Without `screenshot`, one is attached when it adds something: the first
  view of a window, a big change, a window with little in its tree, or a
  screen whose pixels changed. (If the user turned on overview screenshots,
  an automatic one of a well-described window is smaller; pass
  `screenshot: true` for full detail.)
- "Screenshot: unchanged, not re-sent": the picture you have is current.
- A follow-up screenshot may be only the part that changed. The text says
  where it sits in your earlier screenshot; `x`/`y` still refer to that
  whole screenshot.
- `x`/`y` always refer to the latest screenshot you got of that window,
  whatever its size.
- A screenshot said to be "one flat colour": the app doesn't draw while in
  the background. Use the tree, or `window(action="focus")` and look again.
