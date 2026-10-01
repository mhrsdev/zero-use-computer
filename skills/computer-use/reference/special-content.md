# Special content

## Apps with little in their tree

Games, canvases, remote desktops and some custom toolkits show almost
nothing to accessibility APIs. Their tree then contains `ocr text` elements:
lines read off the screen. Click them by `element_index`; they can't be set
or selected. `get_app_state(ocr: true)` asks for them in any app. If the
text can't be read, use the screenshot and `x`/`y`.

## Text in any script or direction

- Text is given in its stored (logical) order. Invisible direction marks
  are removed; zero-width joiners and non-joiners are kept.
- `find_element`, `wait_for` and app or window names match regardless of
  case, spacing, digits of different scripts, letters with two common code
  points, short-vowel marks and zero-width joiners.
- `type_text` types any language, whatever the keyboard layout.
  `press_key` names keys, not characters of a particular layout.

## Masked data

Password fields, card numbers (in any script's digits), fields labelled as
codes, and one-time codes in notifications are masked (`••••`) and blacked
out of screenshots before you see them. Don't try to reveal them; ask the
user for what you need.

## The on-screen indicator

The user sees your own cursor, a glow and a status label while you work.
They are not in your screenshots and the real mouse is not moved: ignore
them.
