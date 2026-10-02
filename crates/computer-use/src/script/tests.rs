use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

use crate::config::{Config, ConfigStore, ScriptFiles};
use crate::engine::Engine;
use crate::mock::MockBackend;
use crate::tools::ToolOutput;

/// A folder of its own for each test's saved scripts and files.
fn library(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cu-script-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn engine_with(dir: &std::path::Path, cfg: impl FnOnce(&mut Config)) -> Engine<MockBackend> {
    let mut backend = MockBackend::new();
    backend.add_app(MockBackend::text_editor(4242));
    let mut config = Config::default();
    config.script.dir = Some(dir.to_path_buf());
    cfg(&mut config);
    Engine::new(backend, ConfigStore::in_memory(config)).with_time(std::time::Instant::now, |_| {})
}

fn run(e: &mut Engine<MockBackend>, code: &str) -> ToolOutput {
    e.call_tool("script", json!({ "code": code }))
}

#[test]
fn scripts_compute_print_and_return() {
    let dir = library("basics");
    let mut e = engine_with(&dir, |_| {});
    let out = run(
        &mut e,
        r#"
        let total = 0;
        for i in range(1, 5) { total += i; }
        print(`total ${total}`);
        // Whole numbers work in maths, PI is there, ints and decimals mix.
        let s = round(sin(0) + sqrt(16) + PI - 3.0, 3);
        #{total: total, s: s, half: 7 / 2.0, ok: hypot(3, 4) == 5.0}
        "#,
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.starts_with("The script ran ("), "{}", out.text);
    assert!(out.text.contains("Output:\ntotal 10"), "{}", out.text);
    assert!(
        out.text
            .contains(r#"Result: {"half":3.5,"ok":true,"s":4.142,"total":10}"#),
        "{}",
        out.text
    );
    assert!(out.image.is_none());
}

#[test]
fn mistakes_are_reported_with_their_line() {
    let dir = library("mistakes");
    let mut e = engine_with(&dir, |_| {});
    // A typo in a name is found before anything runs.
    let out = run(&mut e, "let a = 1;\nprint(b);");
    assert!(out.is_error);
    assert!(out.text.contains("Syntax error"), "{}", out.text);
    assert!(out.text.contains("2 | print(b);"), "{}", out.text);
    // A runtime error names the line and keeps the output before it.
    let out = run(&mut e, "print(\"one\");\nlet x = [1, 2];\nx[5]");
    assert!(out.is_error);
    assert!(out.text.contains("3 | x[5]"), "{}", out.text);
    assert!(
        out.text.contains("Output before that:\none"),
        "{}",
        out.text
    );
    // An unknown function points to the list of functions.
    let out = run(&mut e, "frobnicate(1)");
    assert!(out.text.contains("script(help=true)"), "{}", out.text);
    // Exactly one thing to do.
    let out = e.call_tool("script", json!({"code": "1", "list": true}));
    assert!(out.is_error && out.text.contains("one of"), "{}", out.text);
    let help = e.call_tool("script", json!({"help": true}));
    assert!(help.text.starts_with("# Scripts"), "{}", help.text);
}

#[test]
fn scripts_call_tools_and_read_elements() {
    let dir = library("tools");
    let mut e = engine_with(&dir, |_| {});
    let out = run(
        &mut e,
        r#"
        let apps = tool("list_apps");
        set_app("TextEdit");
        let state = tool("get_app_state");
        let buttons = elements("TextEdit", #{role: "button"});
        let doc = elements("TextEdit", #{editable: true})[0];
        let failed = try_tool("click", #{element_index: 999});
        #{
            apps: apps.contains("TextEdit"),
            state: state.contains("Bold"),
            buttons: buttons.map(|b| b.name),
            doc: [doc.role, doc.value, doc.editable],
            failed: failed.ok,
            why: failed.text.contains("999"),
        }
        "#,
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text.contains(
            r#"Result: {"apps":true,"buttons":["Bold"],"doc":["text area","Hello",true],"failed":false,"state":true,"why":true}"#
        ),
        "{}",
        out.text
    );
    assert!(out.text.contains("5 tool calls"), "{}", out.text);
    // tool() stops the script at a failure; the script tool can't call itself.
    let out = run(
        &mut e,
        r#"tool("click", #{app: "TextEdit", element_index: 999}); print("never")"#,
    );
    assert!(out.is_error && !out.text.contains("Output"), "{}", out.text);
    let out = run(&mut e, r#"tool("script", #{code: "1"})"#);
    assert!(out.text.contains("run(name, args)"), "{}", out.text);
}

#[test]
fn pages_are_drawn_on_named_cells_and_shown() {
    let dir = library("pages");
    let mut e = engine_with(&dir, |_| {});
    let out = run(
        &mut e,
        r##"
        let p = page("board", 800, 800, #{cell: 100});
        for row in 0..8 {
            for col in 0..8 {
                if (row + col) % 2 == 1 {
                    p.fill_cell(p.cells().name(col, row), "#222222");
                }
            }
        }
        p.text_in("D4", "K", #{fill: "#ffffff"});
        let c4 = p.cell("C4");
        #{cell: [c4.x, c4.y, c4.w, c4.cx], at: p.at(250, 350), steps: p.steps().len(), page: `${p}`}
        "##,
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text.contains(r#""cell":[200,300,100,250]"#),
        "{}",
        out.text
    );
    assert!(out.text.contains(r#""at":"C4""#), "{}", out.text);
    assert!(
        out.text.contains("cells of 100: columns A to H"),
        "{}",
        out.text
    );
    // The page is the design board's: it is shown, and draw can paint it.
    assert!(out.image.is_some(), "the page is shown at the end");
    let d = e.call_tool("design", json!({"name": "board"}));
    assert!(d.text.contains("33 layers"), "{}", d.text);
    // Running it again draws the same page, not a second copy.
    let again = run(
        &mut e,
        r#"let p = page("board", 800, 800, #{cell: 100}); p.rect(0, 0, 100, 100); ()"#,
    );
    assert!(!again.is_error, "{}", again.text);
    let d = e.call_tool("design", json!({"name": "board"}));
    assert!(d.text.contains("1 layer,"), "{}", d.text);
    // A page made in this script can be opened again by name in it, and
    // names are the board's ("My Page" is "my-page").
    let out = run(
        &mut e,
        r##"let a = page("My Page", 200, 100); a.rect(0, 0, 10, 10); let b = page("my page"); b.circle(50, 50, 5); b.name"##,
    );
    assert!(out.text.contains("Result: my-page"), "{}", out.text);
    let d = e.call_tool("design", json!({"name": "my-page"}));
    assert!(d.text.contains("2 layers"), "{}", d.text);
    // Bad style keys are caught where they are written.
    let out = run(
        &mut e,
        "let p = page(\"x\", 100, 100);\np.rect(0, 0, 5, 5, #{colour: \"red\"})",
    );
    assert!(
        out.text.contains("not a style key") && out.text.contains("2 |"),
        "{}",
        out.text
    );
    // Cells for any canvas.
    let out = run(
        &mut e,
        r#"let g = cells(#{range: [-3, 3, -2, 2]}); [g.cols, g.rows, g.at(0.5, -0.5), g.cell("A1").y]"#,
    );
    assert!(out.text.contains(r#"Result: [6,4,"D3",2]"#), "{}", out.text);
}

#[test]
fn files_stay_where_the_settings_allow() {
    let dir = library("files");
    let mut e = engine_with(&dir, |_| {});
    let out = run(
        &mut e,
        r#"
        let path = write_csv("out/table.csv", [#{name: "a", n: 1}, #{name: "b, c", n: 2.5}]);
        let rows = read_csv("out/table.csv", true);
        write_json("x.json", #{list: [1, 2]});
        #{rows: rows, json: read_json("x.json").list, files: list_files("out"), here: exists("x.json")}
        "#,
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text
            .contains(r#""rows":[{"n":1,"name":"a"},{"n":2.5,"name":"b, c"}]"#),
        "{}",
        out.text
    );
    assert!(
        out.text.contains(r#""files":["table.csv"]"#),
        "{}",
        out.text
    );
    assert!(dir.join("files/out/table.csv").exists());
    // Outside the scripts' folder: reading yes, writing no (files = "read").
    let outside = std::env::temp_dir().join(format!("cu-outside-{}.txt", std::process::id()));
    std::fs::write(&outside, "from the user").unwrap();
    let code = format!("read_text({:?})", outside.display().to_string());
    let out = run(&mut e, &code);
    assert!(out.text.contains("Result: from the user"), "{}", out.text);
    let code = format!("write_text({:?}, \"x\")", outside.display().to_string());
    let out = run(&mut e, &code);
    assert!(
        out.is_error && out.text.contains("only in their own folder"),
        "{}",
        out.text
    );
    let out = run(&mut e, r#"read_text("../../secret")"#);
    assert!(
        out.is_error && out.text.contains("leaves the scripts' folder"),
        "{}",
        out.text
    );
    // files = "none": nothing.
    let mut none = engine_with(&dir, |c| c.script.files = ScriptFiles::None);
    let out = run(&mut none, r#"read_text("x.json")"#);
    assert!(
        out.is_error && out.text.contains("may not use files"),
        "{}",
        out.text
    );
    // The web, switched off, or not a web address.
    let mut offline = engine_with(&dir, |c| c.script.web = false);
    let out = run(&mut offline, r#"fetch("https://example.com")"#);
    assert!(out.text.contains("may not use the web"), "{}", out.text);
    let out = run(&mut e, r#"fetch("file:///etc/passwd")"#);
    assert!(out.text.contains("not a web address"), "{}", out.text);
    std::fs::remove_file(outside).ok();
    // Devices and pipes never end: only regular files are read.
    #[cfg(unix)]
    {
        let mut all = engine_with(&dir, |c| c.script.files = ScriptFiles::All);
        let out = run(&mut all, r#"read_text("/dev/zero")"#);
        assert!(
            out.is_error && out.text.contains("not a regular file"),
            "{}",
            out.text
        );
        let out = run(&mut all, r#"write_text("/dev/null", "x")"#);
        assert!(
            out.is_error && out.text.contains("not a regular file"),
            "{}",
            out.text
        );
    }
}

#[test]
fn a_replace_that_would_fill_the_memory_is_refused() {
    let dir = library("replace");
    let mut e = engine_with(&dir, |_| {});
    let out = run(
        &mut e,
        r#"
        let big = "a";
        for i in 0..20 { big += big; }
        big.replace("a", big)
        "#,
    );
    assert!(
        out.is_error && out.text.contains("replace would make a text"),
        "{}",
        out.text
    );
    let out = run(&mut e, r#""a-b-c".replace("-", "+")"#);
    assert!(out.text.contains("a+b+c"), "{}", out.text);
}

#[test]
fn memory_and_text_helpers() {
    let dir = library("memory");
    let mut e = engine_with(&dir, |_| {});
    let out = run(
        &mut e,
        r#"remember("count", recall("count", 0) + 1); recall("count")"#,
    );
    assert!(out.text.contains("Result: 1"), "{}", out.text);
    let out = run(
        &mut e,
        r#"remember("count", recall("count", 0) + 1); recall("count")"#,
    );
    assert!(out.text.contains("Result: 2"), "{}", out.text);
    let out = run(
        &mut e,
        r##"
        seed(7);
        let a = random(1, 6);
        seed(7);
        #{
            same: a == random(1, 6),
            nums: numbers("moved 3 right and 2.5 up, -4"),
            found: regex_find("a1 b22 c333", "[a-z](\\d+)"),
            groups: regex_groups("w=3 h=4", "(\\w)=(\\d)").map(|g| g[2]),
            csv: parse_csv("x;y\n1;2"),
            colour: [hsl(0, 1, 0.5), mix("#000000", "#ffffff", 0.5), rgb(255, 128, 0)],
            fixed: fixed(PI, 2),
            json: parse_json(to_json([1, #{a: "b"}])),
            text: [" x ".trim(), "a-b".replace("-", "+")],
        }
        "##,
    );
    assert!(!out.is_error, "{}", out.text);
    for want in [
        r#""same":true"#,
        r#""nums":[3,2.5,-4]"#,
        r#""found":["a1","b22","c333"]"#,
        r#""groups":["3","4"]"#,
        r#""csv":[["x","y"],[1,2]]"#,
        r##""colour":["#ff0000","#808080","#ff8000"]"##,
        r#""fixed":"3.14""#,
        r#""json":[1,{"a":"b"}]"#,
        r#""text":["x","a+b"]"#,
    ] {
        assert!(out.text.contains(want), "{want}: {}", out.text);
    }
    assert_eq!(super::api::utc_date(0), "1970-01-01 00:00:00");
    assert_eq!(super::api::utc_date(1_700_000_000), "2023-11-14 22:13:20");
}

#[test]
fn saved_scripts_become_tools() {
    let dir = library("saved");
    let mut e = engine_with(&dir, |c| {
        c.tools.manager = crate::config::ToolManager::Off;
    });
    let saved = e.call_tool(
        "script",
        json!({
            "save": "greet",
            "code": "let who = args.who ?? \"world\";\n`hello ${who}`",
            "description": "Say hello",
            "params": {"who": {"type": "string", "description": "who to greet"}}
        }),
    );
    assert!(!saved.is_error, "{}", saved.text);
    assert!(
        saved.text.contains("Saved the script \"greet\""),
        "{}",
        saved.text
    );
    let file = std::fs::read_to_string(dir.join("greet.rhai")).unwrap();
    assert!(
        file.starts_with("// description: Say hello\n// params: {"),
        "{file}"
    );
    // A tool of its own, listed with its arguments.
    assert!(e.has_tool("greet"));
    let def = e
        .tool_definitions()
        .into_iter()
        .find(|d| d.name == "greet")
        .expect("listed as a tool");
    assert!(def.description.starts_with("Say hello"));
    assert_eq!(def.input_schema["properties"]["who"]["type"], "string");
    let out = e.call_tool("greet", json!({"who": "you"}));
    assert!(
        out.text.contains("The script \"greet\" ran") && out.text.contains("Result: hello you"),
        "{}",
        out.text
    );
    // Run by name, and inside another script.
    let out = e.call_tool("script", json!({"run": "greet"}));
    assert!(out.text.contains("Result: hello world"), "{}", out.text);
    let out = run(&mut e, r#"run("greet", #{who: "again"}).to_upper()"#);
    assert!(out.text.contains("Result: HELLO AGAIN"), "{}", out.text);
    let list = e.call_tool("script", json!({"list": true}));
    assert!(
        list.text.contains("- greet (who): Say hello"),
        "{}",
        list.text
    );
    // Names of built-in tools, bad names and broken code are refused.
    for (name, code) in [("click", "1"), ("Bad Name", "1"), ("broken", "let = ;")] {
        let out = e.call_tool(
            "script",
            json!({"save": name, "code": code, "description": "d"}),
        );
        assert!(out.is_error, "{name}: {}", out.text);
    }
    // Off as tools: still runs by name, but no longer listed.
    let mut quiet = engine_with(&dir, |c| c.script.saved_as_tools = false);
    assert!(!quiet.has_tool("greet"));
    assert!(quiet.tool_definitions().iter().all(|d| d.name != "greet"));
    let out = quiet.call_tool("script", json!({"run": "greet"}));
    assert!(out.text.contains("hello world"), "{}", out.text);
    let gone = e.call_tool("script", json!({"delete": "greet"}));
    assert!(!gone.is_error, "{}", gone.text);
    assert!(!e.has_tool("greet"));
}

#[test]
fn the_stop_key_and_the_time_limit_end_a_script() {
    let dir = library("stop");
    let mut e = engine_with(&dir, |_| {});
    let stop = e.stop_handle();
    let presser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    // try/catch can't hold it either.
    let out = run(
        &mut e,
        "print(\"started\"); try { loop { } } catch { print(\"caught\") }",
    );
    presser.join().unwrap();
    assert!(out.is_error, "{}", out.text);
    assert!(out.text.contains("stopped"), "{}", out.text);
    assert!(
        out.text.contains("started") && !out.text.contains("caught"),
        "{}",
        out.text
    );
    let mut slow = engine_with(&dir, |c| c.script.max_seconds = 1);
    let out = run(&mut slow, "let n = 0; loop { n += 1; }");
    assert!(
        out.is_error && out.text.contains("ran out of time"),
        "{}",
        out.text
    );
}

#[test]
fn script_values_become_tool_arguments() {
    // Whole decimals become integers (element_index needs one).
    let d = rhai::Dynamic::from_map(
        [
            ("i".into(), rhai::Dynamic::from_float(3.0)),
            ("f".into(), rhai::Dynamic::from_float(2.5)),
        ]
        .into_iter()
        .collect(),
    );
    assert_eq!(super::api::to_json(&d).unwrap(), json!({"i": 3, "f": 2.5}));
    assert_eq!(
        super::api::to_json(&rhai::Dynamic::UNIT).unwrap(),
        Value::Null
    );
}

#[test]
fn the_reference_examples_parse() {
    let mut blocks = 0;
    for block in super::HELP.split("```rhai\n").skip(1) {
        let code = block.split("```").next().unwrap();
        super::check(code).unwrap_or_else(|e| panic!("{e}\n{code}"));
        blocks += 1;
    }
    assert!(blocks >= 5, "{blocks} examples");
}

#[test]
fn the_reference_chart_draws() {
    let dir = library("chart");
    let mut e = engine_with(&dir, |_| {});
    let chart = super::HELP
        .split("```rhai\n")
        .find(|b| b.contains("page(\"chart\""))
        .and_then(|b| b.split("```").next())
        .unwrap();
    let out = e.call_tool(
        "script",
        json!({"code": chart, "data": [["Mon", 12], ["Tue", 18], ["Wed", 7]]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.image.is_some());
    let d = e.call_tool("design", json!({"name": "chart"}));
    assert!(d.text.contains("7 layers"), "{}", d.text);
}
