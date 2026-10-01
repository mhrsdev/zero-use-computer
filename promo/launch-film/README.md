# Z-MCP — open-source launch film

A 44-second, 1920×1080, 60 fps motion film announcing that Z-MCP / Zero
Use Computer is open source. It is built with
[HyperFrames](https://github.com/heygen-com/hyperframes) (HTML + GSAP,
rendered headless). The sound is synthesized from code. Nothing in the
product was changed; everything lives in this folder.

**Final master:** `renders/zmcp-launch-1080p60.mp4` (H.264, 60 fps, AAC 48 kHz).

## What is in here

| File | What it is |
|---|---|
| `VERIFIED_FEATURES.md` | What the repository actually implements: IMPLEMENTED / PARTIAL / NOT IMPLEMENTED, with file:line and test evidence. The film uses only IMPLEMENTED rows. |
| `docs/feature-audit.md` | The full audit behind it (22 areas, line-level evidence, verbatim strings). |
| `CLAIMS.md` | Every significant claim on screen, timestamped, mapped to its evidence. |
| `STORYBOARD.md` | Beat sheet: seven frames, what causes each transition, and the sound map. |
| `BRIEF.md`, `frame.md` | Brief and design spec (graphite palette, the overlay's real state colours, type). |
| `index.html`, `film.css` | The composition: the screen, the overlay, the panels, the type. |
| `src/timeline.js` | The film's one seekable GSAP timeline, built from measured layout. |
| `src/scene-data.js` | Every string the system "says" (tree, change reports, call path), in the engine's formats. |
| `cues.json` → `src/cues.js` | The single timing source for picture **and** sound (`python3 tools/sync-cues.py`). |
| `audio/synth.py`, `audio/master.sh` | The score and sound design, generated from `cues.json`; mastered to about -16.5 LUFS, -1.3 dBTP. |
| `assets/` | Fonts (Archivo, JetBrains Mono — OFL), GSAP, grain, model-view thumbnails, the mastered score. |
| `review/` | Contact sheets and representative frames from the final render. |
| `docs/reference-renders/` | The overlay as the repo's own code draws it, and real engine output, used as reference. |

## Rebuild

Needs Node 22+, FFmpeg, Python 3 with numpy and scipy, and a Chrome headless shell.

```bash
cd promo/launch-film
export HYPERFRAMES_BROWSER_PATH=/path/to/chrome-headless-shell   # or: npx hyperframes browser ensure

python3 tools/sync-cues.py            # cues.json -> src/cues.js
./audio/master.sh                     # cues.json -> assets/audio/score.wav
npx hyperframes@0.8.105 check .       # lint, runtime, layout, motion, contrast
npx hyperframes@0.8.105 preview       # Studio
npx hyperframes@0.8.105 render -f 60 -q high -o renders/zmcp-launch-1080p60.mp4
```

The two model-view thumbnails are captured from the composition itself
(`tools/capture-thumbs.cjs`, needs `puppeteer-core`).

Retiming a beat: change it in `cues.json`, run `sync-cues.py` and
`audio/master.sh`, and picture and sound move together.

## Decisions worth knowing

- **One continuous canvas.** The frame is the user's screen until the final
  pull-back, and every transition is caused by something the system does:
  a capture freezes the frame, a click's ripple opens the call path, the
  user's own mouse stops everything, a returning screen brings its old
  indices back. Because the camera, the cursor and the overlay persist
  across every scene, the film is one composition rather than
  sub-compositions; `hyperframes check` passes with two lint warnings
  about that structure.
- **The overlay is the real one.** The cursor, tag, glow, label, state
  colours, label texts and timings are taken from
  `crates/computer-use/src/overlay/draw.rs` and `config_template.toml`.
  Glide and ripple are slowed down so they can be followed on video.
- **No confirmation dialog for Zero.** The server has none; the only
  "Confirm" on screen is the mail app's own dialog, for a deletion the user
  asked for.
- **The dry joke.** A dashed node labelled "another screenshot?" that the
  data routes around, landing on the engine's real
  `Screenshot: not attached (pass screenshot=true for one).` Codex also
  appears once, as one of the documented MCP clients.
