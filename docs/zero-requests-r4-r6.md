# Pointer picture, sound test and profiles for a host (R4 to R6)

Requests of a host started with `--host zero`, all under
`computer_use/settings/`, on the server's own stdio. A request that is not
answered because there is no host is `-32601`. A refusal is JSON-RPC error
`-32010` with `error.data.code` a fixed word, and `error.data.detail.reason`
words of the program's own (text only, at most 300 characters, with the
folders of this computer left out). Nothing here is a tool of the agent, and
none of it is done on a model's say-so: the host sends it when the user
presses a button.

| Request | Params | Answer | Refusals (`data.code`) |
|---|---|---|---|
| `cursor` | none | `set`, `kept`, `usable`, `format`, `width`, `height`, `frames`, `note`, `limits {fileMb, side, frames, seconds}`; `usable` to `note` are `null` when no picture is set | |
| `cursor_set` | `data`: base64 PNG or GIF | `{"ok": true}` | `badParams` (no `data`), `notAPicture`, `refused` (could not be kept or set), `managedByHost` |
| `cursor_clear` | none | `{"ok": true}` (also when nothing was set) | `refused`, `managedByHost` |
| `sound_test` | `event`: `click` `key` `done` `error` `stopped`; `dryRun`: bool | `dryRun: true`: `{"ready": true, "player"}`; otherwise `{"played": true, "player"}` | `badParams` (unknown event, `dryRun` not a bool), `noPlayer`, `tooSoon`, `playFailed` |
| `profiles` | none | `{"profiles": [{id, label, blurb, builtin, changes: [{key, value}]}]}`; `value: null` is the default | |
| `profile_save` | `label` | `{"id"}` | `badName`, `saveFailed` |
| `profile_delete` | `id` | `{}` | `badParams`, `notAllowed` |

## What the program guarantees

- `limits` are the overlay reader's own (`4`, `512`, `150`, `5` today), and
  stay inside what Zero believes (16, 8192, 10000, 3600).
- No path is in any answer. `cursor_set` answers `{"ok": true}` only (the
  settings' own answer holds the kept file's path).
- `dryRun: true` starts no player. `dryRun` absent or `false` plays one
  sound; the same sound within one second is `tooSoon`. A `dryRun` that is not
  a bool is `badParams`, never "play". `noPlayer` is told for a check and for
  a play alike.
- A profile holds no secret, no setting that asks for a yes or a restart, no
  `panel.*`, and no setting that names a place on this computer (the
  picture, the sound files, the font, the OCR program, the scripts folder).
  A profile file written by hand is filtered the same way when listed.
  `profile_save` keeps only settings the host made the user's (`user_keys`)
  and not ones its own layer sets. At most 40 of the user's own profiles
  (44 listed with the built-in ones, under the 50 Zero reads).
- `profile_save` is `badName` for an empty name, more than 60 characters, a name
  with no letter or digit, or a built-in profile's name; `saveFailed` when
  nothing differs from the defaults, at the 40 limit, or when the file could
  not be written.

## Deviations from `computer-component-requests.md`

- **Base.** The document assumes the host mode (`--host zero`, `panel/host.rs`,
  `host_stdio.rs`) is in the repository. It was only on the local branch
  `zero-integration` (4 commits above `main`), not on `origin`. This branch
  starts from that branch's tip (`866e7b1`) so R4 to R6 have a host to live in;
  it adds nothing of R1 to R3.
- **`cursor_set` write failure** is `refused` (the document lists only
  `notAPicture`, `badParams`, `managedByHost`); `refused` is what
  `cursor_clear` already uses.
- **Typed refusals instead of the sketch's text matching.** `cursor::keep`
  tells a missing picture, a non-picture and a failure to keep apart;
  `tooSoon` is the host's own limiter (the program's player removes its gap
  for a panel test), and `noPlayer` is found before playing.
- **`playFailed`'s reason** is a fixed sentence (the player's own words name
  its path and the sounds' folder); the full words go to the log.
- **The sketch's `profiles` listed whatever the files held;** this lists
  only what a profile may hold, and the page's profiles no longer hold a
  place either (a profile saved before may have held `overlay.cursor_image`;
  that value is no longer applied).
- **Persian profile names.** An id is made from ASCII letters and digits, as
  the page always did, so a name with none (for example only Persian letters)
  is `badName`. Zero can show any label, but must send one with a letter or
  digit in it.
- **`values` and `set` (unchanged, from before R4) still return the value of
  `overlay.cursor_image`,** which after `cursor_set` is the path of the kept
  copy. Zero should not show or log that value; the picture goes through
  `cursor`, which has no path.

## Not tested here

Windows, macOS and Wayland were not run (this machine is Linux with no
screen): the sound player there is `PlaySound` or `afplay`, found by the same
`Program::find`, and the raw-protocol sound test uses a stand-in `paplay` or
`afplay` script on Unix only. The tests run with `--features mock-backend`.
