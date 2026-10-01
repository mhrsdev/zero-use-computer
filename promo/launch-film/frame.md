---
name: Z-MCP launch film — graphite
canvas: 1920x1080
fps: 60
colors:
  ground: "#0A0B0D"        # near-black, tinted cool toward the accent
  surface-1: "#121418"     # desktop plane
  surface-2: "#181B20"     # app window body
  surface-3: "#20242A"     # app chrome, selected rows
  lead: "#2C3138"          # pencil-lead hairlines, construction lines
  lead-strong: "#46505B"   # active construction lines, dimension marks
  graphite-text: "#8C96A1" # secondary text, mono metadata
  paper: "#ECEFF2"         # primary type (never pure white)
  # Overlay state colours, verbatim from crates/computer-use/src/config_template.toml [overlay]
  zero-blue: "#1E88E5"      # color_working  — "Zero is using the computer"
  thinking-gold: "#D4A017"  # color_thinking — "Zero is thinking…"
  paused-grey: "#78909C"    # color_paused   — "Paused while you use the computer"
  stopped-orange: "#FF6D00" # color_stopped  — "Zero stopped. Press {hotkey} to let it continue"
  done-green: "#2E7D32"     # color_done     — "Zero is done"
  cursor-violet: "#9C27B0"  # cursor_color   — the agent cursor itself (tag "Zero")
typography:
  statement: { family: Archivo, weights: [300, 800, 900], stretch: "75%-125%", tracking: "-0.035em" }
  machine:   { family: JetBrains Mono, weights: [400, 500], tracking: "0" }
  rules:
    - Statements are Archivo; anything the system itself says (tree lines, tool names, change reports, labels) is JetBrains Mono, verbatim.
    - Display 96-150px, statements 72-110px, mono readouts 26-34px, metadata 18-20px.
radius: { window: 10px, everything-else: 0 }
shadows: none (depth from value steps and the overlay glow only)
---

## Overview

The film is drawn on a graphite drafting surface. The computer is the canvas:
an ordinary desktop app, operated by Zero, whose own on-screen indicator
(cursor with name tag, glow, state label) is reproduced from the overlay
source. Everything the system "thinks" is drawn as pencil-lead construction
geometry — hairline boxes with corner ticks, index numbers, leader lines,
dimension marks — that grows out of the app and collapses back into it.

## Colour discipline

- Zero blue appears only where the real overlay shows it: the glow, the
  cursor's ring, the label dot, and data that is moving because of Zero.
- Gold, grey and orange appear only as the real overlay states they are
  (thinking, paused, stopped). Nothing else is ever amber.
- Construction lines are lead (#2C3138) at rest and lead-strong when active.

## Motion grammar

- Every transition has a cause: a click, a capture, a tool call, a human
  input, a returning screen. No free-floating cuts.
- Ease vocabulary: `power3.out` for arrivals, `power2.inOut` for camera,
  `expo.out` for ripples, `steps()` never. Holds are deliberate.
- One focal event at a time. Supporting geometry stays at low contrast.

## Don't

- No gradients on large fields, no neon, no glass cards, no particles,
  no rounded rectangles beyond the app window, no HUD chrome.
