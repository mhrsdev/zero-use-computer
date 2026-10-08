# Scripts

`script` runs a small program inside the server. Use it when the tools
can't do the job in one call:

- loops and conditions over tool calls: go through every row of a list,
  retry until something appears, fill a form from a table;
- maths and data: positions, charts, statistics, text to pick apart;
- data the task needs from files, the web or earlier runs;
- pictures built on graph paper (a page with named cells) and then drawn
  into an app;
- a capability the tools lack: save the script, and it becomes a tool of
  its own for later.

A script's tool calls are ordinary tool calls. The security rules, the
stop key, the pause while the user works and the masking of private data
all apply to them. Writing a script never makes anything allowed that
wasn't.

## Running one

```json
{"code": "let n = 0; for i in range(1, 11) { n += i * i; } n"}
```

- `args` (a map) and `data` (any JSON: a table, points, text) are passed
  in as the variables `args` and `data`.
- The result says how long it took and how many tool calls it made, then
  what it printed (`print(x)`) and its last value.
- A failure names the line and shows it:
  `Function not found: sin (string) (line 4, position 9)` and
  `4 | let y = sin(name);`. What it printed before is kept.
- Misspelt variables are caught before anything runs.
- At most `[script] max_seconds` (default 300 s), tool calls included.
  The stop key ends a script at once, even inside `try`.
- One picture comes back: the last page the script touched, or what it
  chose with `show(...)`.

## The language (Rhai) in short

It reads like JavaScript:

```rhai
let total = 0;                         // every variable needs let
let names = ["a", "b"];                // arrays
let box = #{x: 10, y: 20};             // maps: box.x, box["y"]
for name in names { print(name); }
for i in range(0, 5) { }               // 0 to 4; also 0..5 and 0..=4
for (item, i) in names { }             // with the index
while total < 10 { total += 1; }
if total > 5 { print("big") } else if total > 2 { } else { }
let size = if total > 5 { "big" } else { "small" };
let double = |x| x * 2;                // closures see the variables around them
fn area(w, h) { w * h }                // fn sees only its arguments
print(`total is ${total}`);            // text with values in it
let who = args.name ?? "you";          // ?? : a default for a missing value
try { tool("click", #{app: "X", element_index: 3}) } catch (e) { print(e) }
throw "stop here: no file";            // fail on purpose
```

Watch out for:

- `7 / 2` is `3` (whole numbers). Write `7 / 2.0` or `x.to_float()`.
- `fn` functions can't see outer variables or `PI`: pass values in, use
  `PI()`, or use a closure.
- `try { } catch (e) { }` is a statement: set a variable inside it.
- `a.sort()`, `a.reverse()` and `a.push(x)` change `a` and give nothing.
  `s.trim()` and `s.replace(a, b)` give the new text.
- A missing map key is `()` (nothing), not an error: use `??`.

Useful built-ins: `a.len()`, `a.map(|x| ..)`, `a.filter(|x| ..)`,
`a.reduce(|sum, x| sum + x, 0)`, `a.some(..)`, `a.all(..)`,
`a.index_of(x)`, `a.contains(x)`, `a.sort(|a, b| a.n - b.n)`,
`m.keys()`, `m.values()`, `s.split(",")`, `s.contains("x")`,
`s.to_upper()`, `s.index_of("x")`, `s.sub_string(start, len)`,
`parse_int(s)`, `parse_float(s)`, `x.to_int()`, `x.to_string()`,
`type_of(x)`, `min(a, b)`, `max(a, b)`, `abs(x)`.

## Tools and the screen

| Function | Gives |
|---|---|
| `tool(name)`, `tool(name, #{...})` | the tool's text; a failure stops the script with its message |
| `try_tool(name, #{...})` | `#{ok, text, image}` and never stops the script |
| `set_app("Paint")` | later calls without `app` use this app |
| `elements(app)`, `elements(app, #{role, name, text, editable, window, max})` | the matching elements as maps: `index`, `role`, `name`, `value`, `enabled`, `focused`, `selected`, `checked`, `expanded`, `editable`, `actions`, and once a screenshot was taken `x`, `y`, `w`, `h`, `cx`, `cy` (the x/y `click` takes) |
| `colors(app, [[x, y], ...])` | the exact colour at each point (`"#rrggbb"`, or `()` off the window) |
| `color_at(app, x, y)` | one colour |
| `show(result)` | return that tool result's picture (from `try_tool`) |
| `show_image(path)` | return a PNG or JPEG file as the picture |
| `sleep(ms)` | wait (the stop key still works) |
| `run(name)`, `run(name, #{...})` | run a saved script and give its last value |

Element indices are those of the latest `get_app_state` or `elements` of
that app, so read them again after anything that changes the screen.
`colors` needs a `get_app_state` with a screenshot first.

## The page: graph paper

A page is a design on the design board (the `design` tool), so what a
script puts on it can be seen, checked, exported or painted into an app
step by step. Its cells are named like a chessboard: columns A, B…, rows 1,
2… from the top left.

```rhai
let p = page("chess", 800, 800, #{cell: 100});   // 8 x 8 cells of 100
p.fill_cell("B1", "#769656");
let c = p.cell("C4");      // #{name, col, row, x, y, w, h, cx, cy}
p.at(250, 350)             // "C4"
```

- `page(name, width, height)` starts a page, or starts it again empty
  (running a script twice draws the same page). Options: `cell` (the
  cells' size; default about 8 across), `background`, `margin`.
  `page(name)` opens a page or design that is there and adds to it.
- Shapes, each with an optional style map last; each gives the layer's id:
  `p.rect(x, y, w, h)`, `p.circle(cx, cy, r)`, `p.ellipse(cx, cy, rx, ry)`,
  `p.line(x1, y1, x2, y2)`, `p.path([[x, y], ...])`,
  `p.polygon([[x, y], ...])` (closed), `p.bezier([[x, y], ...])`,
  `p.regular(cx, cy, r, sides)`, `p.star(cx, cy, outer, inner, tips)`,
  `p.arc(cx, cy, r, from, to)`, `p.curve("x(t)", "y(t)", t0, t1)`,
  `p.text("Hi", x, y)` (x, y: its top left), `p.layer(#{...})` (any layer
  the design tool takes).
- Styles: `fill`, `stroke` (`"#rrggbb"` or `"none"`), `width`,
  `opacity`, `rotate`, `about`, `repeat`, `closed`, `smooth`, `steps`,
  `size`, `font`, `bold`, `align`, `id`, `below`, `above`, `move`, `to`,
  and `radius` for a rectangle's corners. Shapes without colours are
  filled black; lines are drawn black, 2 wide.
- Cells: `p.fill_cell("C4", colour)`, `p.text_in("C4", "K")` (centred),
  `p.cell("C4")` or `p.cell(col, row)` (from 0), `p.at(x, y)`,
  `p.cells()` (the page's cells: see below).
- Changes: `p.change(id, #{fill: "#ff0000"})`, `p.remove(id)`,
  `p.clear()`.
- `p.show()` returns the picture and the board's text (layers, checks,
  paint steps); `p.steps()` gives the paint steps
  (`#{step, color, kind, width, layers}`); `p.layers()` each layer's id and
  box; `p.export("png")` or `"svg"` writes a temporary file and gives its
  path. `p.name`, `p.width`, `p.height`.

Paint it into an app: per step, set the app's colour to the step's
`color`, then `tool("draw", #{strokes: [#{design: "chess", step: s.step,
fill: 8}], canvas: #{box: [l, t, r, b], size: [800, 800]}})`. Give `draw`
and `screenshot` the same `cell_size` (100 here) and they name the same
cells.

### Cells for any canvas

`cells(width, height)`, `cells(width, height, size)`, or
`cells(#{range: [x0, x1, y0, y1], cell: 0.5})` (a maths range, y up) give
the same arithmetic without a page: `g.cell("C4")`, `g.cell(col, row)`,
`g.at(x, y)`, `g.name(col, row)`, `g.all()` (every cell, row by row),
`g.cols`, `g.rows`, `g.size`, `g.left`, `g.top`.

## Data

| Function | Does |
|---|---|
| `read_text(path)`, `read_json(path)` | a file's text, or its JSON as maps and arrays |
| `read_csv(path)`, `read_csv(path, true)` | rows as arrays, or (with a header row) as maps; numbers become numbers; `,`, `;` or tabs |
| `write_text(path, text)`, `append_text(path, text)`, `write_json(path, value)`, `write_csv(path, rows)` | write a file; gives its full path |
| `exists(path)`, `list_files()`, `list_files(path)`, `workspace()` | is it there; a folder's files (folders end in `/`); the scripts' own folder |
| `fetch(url)`, `fetch(url, #{method, headers, body, timeout})` | a web page or API's text (`body` as a map is sent as JSON) |
| `fetch_json(url)`, `fetch_json(url, #{...})` | its JSON |
| `download(url, path)` | save it to a file (an image for `trace_image`, a CSV…) |
| `remember(key, value)`, `recall(key)`, `recall(key, default)`, `forget(key)`, `memory()` | values kept between runs |
| `parse_json(text)`, `to_json(value)`, `pretty_json(value)`, `parse_csv(text)`, `to_csv(rows)` | data to and from text |
| `regex_test(text, pattern)`, `regex_find(text, pattern)`, `regex_groups(text, pattern)`, `regex_replace(text, pattern, with)` | patterns: does it match, every match, each match's groups (`[whole, 1, 2…]`), replace (`$1` in `with`) |
| `numbers(text)` | every number in a text: `numbers("moved 3 right, 2.5 up")` is `[3, 2.5]` |
| `fixed(x, digits)` | `"3.14"` |
| `now()`, `date()`, `date(seconds)`, `elapsed()` | seconds since 1970; `"2026-10-01 12:00:00"` (UTC); seconds since the script began |

Paths: a relative path is in the scripts' own folder
(`~/.computer-use/scripts/files`); `~/` is the home folder. What a script
may touch is the user's setting `[script] files`: by default it reads any
file and writes only in its own folder. `fetch` and `download` use `curl`
and can be switched off (`[script] web`).

## Decisions

With a decision model set up (see [decisions.md](decisions.md); the user
adds one with Ctrl+Alt+J), a script can judge as it goes, fast, without
reading everything into the conversation:

| Function | Does |
|---|---|
| `ask(state, question)` | the probability (0 to 1) that the answer is yes |
| `choose(state, question, options)` | one of the options (`["a", "b"]`, or `#{a: "what a means"}`) |
| `score(state, question, levels)` | a number on the scale (0 = the first, lowest level) |
| `decide(state, #{name: #{type, question, options \| scale}})` | several answers at once, as a map |
| `decide_each(states, #{...})` | the same questions about each of many states, in parallel |

`state` is text, or any value (sent as JSON). Without a decision model
these stop the script with a message saying how to add one.

```rhai
// Keep the reviews that recommend the product, and how sure.
let reviews = data;
let answers = decide_each(reviews, #{rec: #{question: "Does it recommend the product?"}});
let kept = [];
for i in range(0, reviews.len()) {
    if answers[i].rec.answer { kept.push(#{text: reviews[i], p: answers[i].rec.yes}); }
}
print(`${kept.len()} of ${reviews.len()} recommend it`);
kept
```

## Maths, chance and colour

- Every maths function takes whole numbers too: `sin`, `cos`, `tan`,
  `asin`, `acos`, `atan`, `sqrt`, `exp`, `ln`, `log` (base 10), `floor`,
  `ceiling`, `round`, `cbrt`, `hypot(a, b)`, `atan(y, x)`; `PI`, `TAU`
  (`PI()` inside `fn`); `round(x, digits)`, `clamp(x, lo, hi)`,
  `lerp(a, b, t)`, `dist(x1, y1, x2, y2)`, `deg(radians)`, `rad(degrees)`.
- `random()` (0 to 1), `random(1, 6)` (whole numbers, both ends
  included) or `random(0.0, 2.5)`, `seed(n)` (the same numbers every run),
  `shuffle(array)`, `choice(array)`.
- `rgb(255, 128, 0)` and `hsl(h, s, l)` (h in degrees, s and l 0 to 1)
  give `"#rrggbb"`; `color_rgb("#ff8000")` gives `[255, 128, 0]`;
  `mix(a, b, t)` blends two colours.

## Saving a script as a tool

```json
{"save": "chessboard", "description": "Paint a chessboard page", "params": {"size": {"type": "integer", "description": "squares per side"}}, "code": "let n = args.size ?? 8; ..."}
```

- The script is kept in `~/.computer-use/scripts/chessboard.rhai` (the
  user can read and edit it). It is checked first: a script that doesn't
  parse isn't saved.
- `script(run="chessboard", args={"size": 10})` runs it; so does the tool
  `chessboard` itself once the client has refreshed its tool list (unless
  the user set `[script] saved_as_tools = false`). Its arguments arrive as
  `args`.
- `run("chessboard", #{size: 10})` runs it inside another script.
- A saved script of `fn` functions only is a library:
  `import "shapes" as shapes;` makes them usable as `shapes::name(...)`.
  Importing runs the script's top-level code, and it has no `args`.
- `list=true` lists saved scripts with their arguments, `show=name` shows
  one, `delete=name` deletes it. Saving under the same name replaces it.

Save a script when the user wants a tool for something they will ask
again (when the same work keeps coming back, offer it); not for one-off
jobs.

## Examples

Fill a form for every row of a table the user gave:

```rhai
set_app("Firefox");
for row in read_csv("~/Documents/guests.csv", true) {
    let name = elements("Firefox", #{role: "text field", name: "Name"})[0];
    tool("set_value", #{element_index: name.index, value: row.name});
    let email = elements("Firefox", #{role: "text field", name: "Email"})[0];
    tool("set_value", #{element_index: email.index, value: row.email});
    let add = elements("Firefox", #{role: "button", name: "Add"})[0];
    tool("click", #{element_index: add.index});
    tool("wait_for", #{role: "text field", name: "Name", state: "enabled"});
}
```

A bar chart of `data` (`[["Mon", 12], ["Tue", 18], ...]`) on a page:

```rhai
let p = page("chart", 640, 400, #{cell: 40});
let top = data.reduce(|m, d| max(m, d[1]), 1);
for (d, i) in data {
    let h = 300.0 * d[1] / top;
    p.rect(40 + i * 80, 340 - h, 60, h, #{fill: hsl(i * 40, 0.6, 0.5)});
    p.text(d[0], 70 + i * 80, 350, #{size: 18, align: "center"});
}
p.line(30, 340, 620, 340);
```

Wait until a download finishes, then say how long it took:

```rhai
let start = elapsed();
while elements("Firefox", #{name: "Download complete"}).len() == 0 {
    if elapsed() - start > 120 { throw "not finished after 2 minutes"; }
    sleep(1000);
}
`done in ${fixed(elapsed() - start, 1)} s`
```

Today's weather from a web API, typed into a note:

```rhai
let w = fetch_json("https://api.open-meteo.com/v1/forecast?latitude=52.52&longitude=13.41&current=temperature_2m");
tool("type_text", #{app: "Notes", text: `Now: ${w.current.temperature_2m} °C\n`});
```

## Rules

- Scripts follow the security rules like any other call: a loop of
  clicks is as many clicks, and a consequential one needs the user's
  go-ahead first, however the script gets to it.
- Read only files the user gave or the task needs. Write in the scripts'
  own folder unless the user asked for a file somewhere else.
- `fetch` only what the task needs, from sites the user would expect.
  Never send the user's data (screen text, files, clipboard) to a site
  unless that is the task.
- No passwords, keys or codes in saved scripts or in `remember`.
