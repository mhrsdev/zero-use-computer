---
workflow: general-video
flow: automation
storyboard: no
message: "An agent shouldn't just receive screenshots: Zero Use Computer lets it see, understand, act, verify and stay under your control — and it's open source."
destination: web-launch
aspect: 1920x1080
language: en
length: 44s
audience: developers who build or use coding agents
angle: the computer itself is the canvas; Zero's real overlay cursor is the protagonist
---

## Intent

A 35–45 s cinematic motion-graphics launch film announcing that Zero Use
Computer (github.com/mhrsdev/zero-use-computer) is open source.
Progression: SEE → UNDERSTAND → ACT → CONTROL → VERIFY → OPEN SOURCE, each
shown through the system itself (Zero's overlay cursor, glow and label;
the numbered accessibility tree; the change report; screen memory), never
as slide headings. Premium, precise, engineering-driven, slightly
intimidating; dark graphite with restrained Zero blue.

## Customizations

- Every on-screen technical claim is traced to the repository in
  `VERIFIED_FEATURES.md` and `CLAIMS.md`; only IMPLEMENTED features appear.
- The overlay (cursor, tag, glow, label, state colours and texts) is
  reproduced from the overlay source and `config_template.toml`.
- One or two dry Codex jokes, under a second each, no claims or numbers.
- Sound: procedurally generated in `audio/synth.py` (no third-party audio),
  cued from the same `cues.json` the picture uses.
- The user asked for autonomous production through the final render, with
  a beat sheet, contact sheets and a claims map as deliverables. The
  storyboard is a deliverable, not a review gate.

## Notes

- Do not modify the product; all production files live in
  `promo/launch-film/`.
- The name on screen is "Zero Use Computer" (from the repository name), not
  "Z-MCP" — changed at the user's request after the first cut.
- License wording must match Apache-2.0 (no "free for anything").
- Banned: AI brains, purple gradients, glass cards, particles, code rain,
  HUD chrome, fake terminals, fake benchmarks, fake comparisons.
