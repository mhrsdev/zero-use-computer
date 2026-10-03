# Agent benchmark

What it costs an agent to *finish* real tasks with these tools, measured the
same way before and after every change. A smaller tool result is not a win
on its own: a run counts only if the task got done, and the tokens counted
are the ones the model really read and wrote.

```bash
bench/run.sh --scripted --label v3.5                      # no model, no key
ANTHROPIC_API_KEY=… bench/run.sh --runs 5 --label v3.5    # the real model
```

`bench/run.sh` builds the benchmark
(`crates/computer-use/examples/agent_bench`) and runs it inside a throwaway
headless desktop (Xvfb, a private D-Bus session, the AT-SPI bus; Linux,
with the packages the CI's live test installs). Each run starts its own copy
of the scenario's app, so every run begins from the same state.

## Scenarios

The apps are one GTK 3 program, `fixtures/bench_app.py`, with nothing in it
that changes by itself (no clock, no randomness). Success is checked from
what the app recorded (it writes a JSON file on every change), never from
what the model says.

| id | app | task | stands for |
|---|---|---|---|
| `form` | Settings | fill two fields, pick a radio button, tick a box, Save | forms and settings |
| `table` | Inventory | open the row with one SKU out of 300 | tables and lists |
| `board` | Board | click the box labelled DELTA | a canvas whose text only the picture has, under a full toolbar |
| `shapes` | Shapes | click the red circle | a canvas without any text |
| `orders` | Orders | copy a painted order number into a field, submit, confirm | mixed: picture, form, dialog |
| `long` | Inventory | open five rows in turn | a longer session |

## Two ways to run

**Real** (the default): Claude through the Messages API, with this
server's tool definitions and the `computer-use` and `computer-use-security`
skills as the system prompt (as a client that loaded the skills would give
them). It records, for every request, the tokens the API reports
(`input_tokens`, `output_tokens`, `cache_read_input_tokens`,
`cache_creation_input_tokens`) and the model that served it. The prompt is
cached the way a good client caches it: a breakpoint after the system
prompt, and automatic caching of the growing conversation.

- `ANTHROPIC_API_KEY` (or `ANTHROPIC_AUTH_TOKEN`) is needed;
  `ANTHROPIC_BASE_URL` or `BENCH_API_URL` changes the address.
- `--model` (default `claude-opus-5-5`), `--effort` (default `medium`;
  `default` sends none).
- On models that take it, the server-side refusal fallback is on
  (`fallbacks: "default"`); a run a fallback model served part of is marked
  (`fallback`, `served`).
- The cost column uses Anthropic's list prices for the model;
  `BENCH_PRICES=input,output,cache_read,cache_write` (dollars per million
  tokens) sets others.
- `--calibrate` counts every tool result's real tokens with the
  token-counting endpoint, to compare with the estimate (characters / 4)
  the server's budgets use.

**Scripted** (`--scripted`): a fixed way through each task, calling the
tools as a careful agent would, using only what earlier results showed.
Two ways through: `--plan step` (default; one action a call, as an agent
on v3.2 would) and `--plan batch` (v3.6: what is known done in one batch
of lines, clicks by name, `expect` on the step that opens a dialog). No
model, no key, and the same numbers every time, so it shows what a change
did to the tools' results. Its token figures are **estimates** (text ≈ 4
characters a token, an image width × height / 750), and `input` assumes no
prompt cache and one call per turn. Scripted numbers are never mixed with
real ones.

## Options

| option | |
|---|---|
| `--scenarios form,table,…` | which scenarios (default: all) |
| `--runs N` | runs per scenario (default 1; use 5 or more for real runs) |
| `--max-turns N` | give up after this many model requests (default 40) |
| `--config FILE` | the server's settings for the run (default: built-in defaults, never your own `config.toml`) |
| `--plan step\|batch` | scripted runs: one action a call, or v3.6's batches |
| `--preset codex` | the way Codex's computer use behaves, simulated as in `examples/compare.rs` (a screenshot with every look, no screen memory, no picture dedupe, no change report). A simulation, not Codex itself |
| `--label NAME` | the folder the results go in |
| `--out DIR` | where (default `target/bench`) |
| `--python PY` | a Python with GTK 3 (default: the first that imports it) |
| `--verbose` | print every call, its result and the model's text |

The decision model is never set up by the benchmark: it is optional, and a
run measures the tools on their own. A `--config` file can set one up for a
run of its own.

## Earlier releases (`--server`)

The same scenarios against a server of any release, run as it ships and
spoken to over MCP, with that release's own skills and MCP instructions in
front of the model:

```bash
bench/run.sh --scripted --label v3.6.0 --server path/to/v3.6.0/computer-use-mcp \
  --skills path/to/v3.6.0/skills
bench/run.sh --scripted --label v0.1.0 --server path/to/v0.1.0/computer-use-mcp \
  --server-args "--approval allow-all --headless-approve allow" \
  --server-config v010.toml --skills path/to/v0.1.0/skill   # [guard] mode = "allow"
bench/compare.py target/bench v0.1.0 v3.0.0 v3.6.0 v3.7.0 --base v3.7.0
```

| option | |
|---|---|
| `--server BIN` | the server to measure (its own process, a fresh one for every run) |
| `--server-args "…"` | its arguments before `serve` (`--instructions short`, an early release's approvals) |
| `--server-config FILE` | its settings (copied in as its `config.toml`; default: its defaults) |
| `--skills DIR` | the skills the model gets: `computer-use` and `computer-use-security` in it, or an early release's single `SKILL.md` |

The fixed prefix is then reported by part: the benchmark's framing, the
server's instructions, the skills and the tool definitions.
`bench/compare.py` puts labels side by side: the prefix by part, a task's
tokens by part (each part of the prefix times the turns, the results, the
conversation read again), and each scenario. A tool the model doesn't see
goes through `find_tools` and `use_tool` only when the server has a tool
manager; an earlier release without the tool fails as it would.

## Results

Each run is one JSON line in `<out>/<label>/<mode>-<time>.jsonl` (success,
why a check failed, how the agent stopped, turns, every call with its
estimated tokens, real usage and cost); a Markdown summary goes next to it,
median (min–max) over the runs of each scenario.

`results/` keeps the summaries worth comparing against, with what changed
between them: [results/README.md](results/README.md). `results/v3.2/` is
the scripted baseline of v3.2, taken before any v3.5 change.

## A/B

A change to what the tools return is added behind a setting, off, and
measured both ways before it is turned on by default:

```bash
bench/run.sh --runs 5 --label v3.6-default
bench/run.sh --runs 5 --label v3.6-lean --config bench/configs/lean.toml
bench/run.sh --runs 5 --label v3.6-manager --config bench/configs/manager.toml
```

`configs/` holds the settings files for those runs: `blind-regions.toml`
(v3.5's blind areas alone), `lean.toml` (every opt-in that saves tokens:
lean schemas, relevant-change reports, quiet volatile elements, adaptive
pictures, `locate` without its picture, blind areas, paint steps when
asked) and `manager.toml` (the same with the tool manager).

## The design board

`cargo run --release -p computer-use --example design_bench` measures the
design board without a desktop: a badge built and fixed in ten calls and a
page of 150 layers, each call's tokens (text and picture) and time. Run it
on two releases to compare them
([v3.8.0 against v3.8.1](results/v3.8.1-design/compare.md)).
The speed of v3.8.3 against v3.8.2:
[results/v3.8.3-speed/compare.md](results/v3.8.3-speed/compare.md).
