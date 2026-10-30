# What a host may ask: Stop state, Resume, the app being controlled

> Since protocol 2 (`computer_use/stop`, events, `--hub-home`,
> `--settings-home`, a stop key that only stops), the full list of what
> Zero Desktop relies on is `docs/zero-host-contract.md`. Below: R1 to R3
> as first built; `protocol` is now `2` and the state also says `agent`.

Requests R1 to R3 of the Zero Desktop request document, as built here. They
are JSON-RPC on the server's own stdio. Nothing is a tool, so the agent never
reaches them.

## `computer_use/state` (R1, R3)

Any mode. Read-only, instant, answered by the stdio reader thread, so it does
not wait for a call that is running (a `wait_for` of twenty seconds, a drag).

```json
{"jsonrpc":"2.0","id":7,"method":"computer_use/state","params":{}}
{"jsonrpc":"2.0","id":7,"result":{
  "protocol": 1,
  "stopped": false,
  "hotkey": { "key": "ctrl+alt+escape", "registered": true },
  "controlling": null
}}
```

| field | meaning |
|---|---|
| `protocol` | `1` |
| `stopped` | the user's Stop is held (stop key or the overlay's Stop button) and has not been let go. While it is true every tool call is refused, looking included |
| `hotkey.registered` | `true` only when the system accepted the stop key **and the helper that listens for it is running now**. Not known yet, refused, no key configured, helper gone: `false`. Never left out |
| `hotkey.key` | `control.stop_hotkey` as set; left out when the user chose none |
| `controlling` | `{"app": {"id", "name"}}` while a call acts on an app and for four seconds after the last one; `null` otherwise. `id` and `name` are the app's, as `list_apps` gives them |

`controlling` covers `click`, `perform_secondary_action`, `set_value`,
`select_text`, `scroll`, `drag`, `press_key` and `type_text`, also as steps of
a `batch` or a script. It does not cover `launch_app`, `window` actions or the
drawing tools: while only those run it says `null`.

## `computer_use/resume` (R2)

Only with `--host`; without it `-32601`. Lets the stop go (as Continue on
the overlay does; the stop key only ever stops), and answers
`{"stopped": false}`. Idempotent. A host
should ask `computer_use/state` afterwards and believe that, not this answer.

## Where this differs from the request document

- **Base.** The document describes the host mode (`--host zero`, the settings
  API). That mode is not on `main`; this work is built on the commits
  `0ff0cc1..866e7b1` (the `zero-integration-base` branch), which carry it.
- **`hotkey.registered` is read live.** The document's sketch refreshed it when
  a call ended, so a helper that died between two calls would still read
  `true`. Here the answer asks the helper's own state each time.
- **`controlling` names the resolved app,** not the `app` text the caller
  typed (the sketch's `Shortcut:`). A name that matches no app, a look, and a
  call refused for the Stop all leave it `null`.
- **The hold survives a restart (not in the document).** In `--host` mode the
  program writes the file `stop-held` in its folder (`COMPUTER_USE_HOME`) while
  the stop is held, within about 50 ms of a change, and starts stopped when it
  finds one. A file that cannot be examined counts as held. Without `--host`
  nothing changes: the stop is in memory only, as before. If the file cannot be
  written the stop still holds but a restart would lose it; the log says so.
- **Not tried here:** the real stop key, the overlay's Stop button and the
  system's acceptance of a key on Windows, macOS and Wayland. The tests use a
  stand-in helper and the in-memory desktop (`--backend mock`), where the key is
  never registered.
