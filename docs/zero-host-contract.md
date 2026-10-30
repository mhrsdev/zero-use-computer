# What Zero Desktop relies on (the host contract)

Zero Desktop starts this program as a child process and talks to it over
its stdio. This page lists **everything** Zero depends on. Anything not here
is free to change; anything here should only change together with a new
`protocol` number (and a note to Zero). Zero pins one version of this
program and checks the version and server name at the handshake; it never
reads the program's internals, files other than the ones below, or log text.

Zero starts **one program per agent** (each conversation that uses the
computer) plus one *primary* program that serves Zero's Settings page and is
the hub's first member. All of them are the same file and the same version.

## Command line

```
computer-use-mcp --host zero [--hub-home DIR] [--settings-home DIR] serve
computer-use-mcp --host zero --backend mock serve      # Zero's tests only (feature mock-backend)
```

| flag | what Zero relies on |
|---|---|
| `--host zero` | no settings page, no port, no self-update, no `config set`; settings come from `user.toml` and `managed.toml` (below); the host-only requests below are answered |
| `--hub-home DIR` | the hub's token and port file are read and written in `DIR`, shared by all of Zero's programs, so they meet in one hub |
| `--settings-home DIR` | `user.toml` and `managed.toml` are in `DIR`, shared by all of Zero's programs; a change to them reaches every running program (hot reload) |
| `COMPUTER_USE_HOME` (environment) | each program's **own** folder: `stop-held`, logs, `apps.json`, and the settings files when there is no `--settings-home`. Never shared between two programs |
| `--backend mock` | the scripted in-memory desktop, real engine and server: no screen, no overlay, no hub, the stop key never registered |

## Files

| file | where | meaning |
|---|---|---|
| `managed.toml` | settings home | written by Zero before every start; wins over `user.toml`; `[host] user_keys` names the settings the user may change |
| `user.toml` | settings home | written only by `computer_use/settings/set` and `reset` |
| `stop-held` | own folder | present while the user's stop is held; a program that finds it at start starts stopped. Only `computer_use/resume` or Continue on the overlay removes it |

## Handshake

`initialize` answers `serverInfo.name = "computer-use"` and
`serverInfo.version` = the crate version. Zero refuses any other name or a
version other than the pinned one.

## Requests (JSON-RPC on stdio, never tools)

| method | when | answer |
|---|---|---|
| `computer_use/state` | any mode; answered at once, even during a call | `{"protocol": 2, "stopped": bool, "hotkey": {"registered": bool, "key"?: string}, "controlling": {"app": {"id", "name"}} \| null, "agent": u32 \| null}` |
| `computer_use/stop` | `--host` only (else `-32601`); answered at once, even during a call | `{"stopped": true, "wasStopped": bool}` |
| `computer_use/resume` | `--host` only (else `-32601`) | `{"stopped": false}` |
| `computer_use/status` | any | `{}` (overlay state from the host) |
| `computer_use/settings/schema`, `values`, `texts`, `set`, `reset`, `tools`, `path`, `cursor`, `cursor_set`, `cursor_clear`, `sound_test`, `profiles`, `profile_save`, `profile_delete` | `--host` only | as described in `CHANGELOG.md` (5.5.0 host mode); refusals carry a fixed `data.code` (`managedByHost`, `outOfRange`, `notAChoice`, `unknownKey`, `needsConfirmation`, ...) |
| `tools/call` | any | for these tools only: `list_apps`, `launch_app`, `get_app_state`, `find_element`, `wait_for`, `screenshot`, `click`, `perform_secondary_action`, `set_value`, `type_text`, `press_key`, `scroll`, `select_text`, `agents` |

Meaning of the state fields:

- `stopped`: the user's stop is held; every tool call is refused, looking
  included.
- `hotkey.registered`: true only while the system has accepted the stop key
  **and** the helper (or hub) listening for it runs now. Zero lets an agent
  operate only with `protocol >= 2`, `stopped: false` and
  `hotkey.registered: true`, asked again before every step that acts.
- `agent`: this program's number in the hub (the `to` of `agents send`),
  `null` when it is in no hub.

## Notifications (program → Zero, `--host` only)

`{"jsonrpc":"2.0","method":"notifications/computer_use/event","params":{...}}`

| `type` | other fields | sent when |
|---|---|---|
| `stopped` | `source`: `hotkey` \| `overlayButton` \| `host`; `all`: bool | the stop went from let go to held. `all` is true for the stop key and Stop all (everyone was told) |
| `resumed` | `source`: `overlayButton` \| `host`; `all`: bool | held to let go |
| `hotkeyLost` | — | a stop key that was registered no longer is (refused, or its helper or hub went away) |
| `message` | `to`: u32 \| null (everyone); `text`: string | this agent sent a message to the others (`agents send`), text as sent |

Every event carries `agent` while the program is in a hub. Events are sent
only on a change and are hints: `computer_use/state` is the truth. Zero
reacts to `stopped` with `all: true` from any program by stopping every
other program it started (`computer_use/stop`), and stores `message`
events to show the agents' talk.

## Rules Zero depends on (do not break)

1. **The stop key only stops.** Pressed again it never lets anyone continue.
   Letting go is only Continue on the overlay or `computer_use/resume`.
2. **A host's stop is that agent's alone.** `computer_use/stop` never stops
   the other agents in a hub; a program that starts held does not stop
   them either.
3. **The stop survives a restart** (`stop-held` in the program's own
   folder), and one program never reads another's.
4. **`computer_use/state` and `computer_use/stop` answer during a call**,
   and a call in progress ends at its next step after a stop.
5. **No event without a change**, and no host-only request without `--host`.

## Not relied on

The hub's own wire protocol, the overlay helper's lines, log text, error
sentences of tools (Zero shows them to the model as untrusted text only),
`userBusy`, `permissionMissing`, `settingsRequested` and `overlayLost` (not
sent by this version), the HTTP transport, the panel, scripts, `decide`.
