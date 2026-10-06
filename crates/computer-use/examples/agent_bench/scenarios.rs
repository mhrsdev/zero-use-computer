//! The scenarios: what the agent is asked, which app it gets, how success
//! is checked (from the state the app writes, never from what the model
//! says), and a scripted way through for runs without a model.

use serde_json::{Value, json};

use crate::session::Session;

pub struct Scenario {
    pub id: &'static str,
    /// `--scenario` of bench/fixtures/bench_app.py.
    pub fixture: &'static str,
    /// The app's name (list_apps) and its window's title.
    pub app: &'static str,
    pub task: &'static str,
    /// What the scenario stands for, for the report.
    pub about: &'static str,
    pub check: fn(&Value) -> Result<(), String>,
    pub scripted: fn(&mut Session) -> Result<(), String>,
    /// The way through with v3.6's fewer round trips (`--plan batch`):
    /// batch lines, clicks by name, `expect`.
    pub batched: fn(&mut Session) -> Result<(), String>,
}

pub const ALL: &[Scenario] = &[
    Scenario {
        id: "form",
        fixture: "form",
        app: "Settings",
        task: "In the Settings app, set Full name to \"Ada Lovelace\" and Email to \"ada@example.com\", choose the country Japan, turn on \"Subscribe to newsletter\", then press Save.",
        about: "a form: named fields, radio buttons, a check box",
        check: check_form,
        scripted: scripted_form,
        batched: batched_form,
    },
    Scenario {
        id: "table",
        fixture: "table",
        app: "Inventory",
        task: "In the Inventory app, open the item whose SKU is K-0137: select its row, then press Open.",
        about: "a 300-row table with a filter",
        check: check_table,
        scripted: scripted_table,
        batched: batched_table,
    },
    Scenario {
        id: "board",
        fixture: "board",
        app: "Board",
        task: "In the Board app, click the box labelled DELTA on the canvas.",
        about: "a canvas of labelled boxes under a full toolbar (text the tree doesn't have)",
        check: check_board,
        scripted: scripted_board,
        batched: scripted_board,
    },
    Scenario {
        id: "shapes",
        fixture: "shapes",
        app: "Shapes",
        task: "In the Shapes app, click the red circle on the canvas.",
        about: "a canvas of shapes without any text",
        check: check_shapes,
        scripted: scripted_shapes,
        batched: scripted_shapes,
    },
    Scenario {
        id: "orders",
        fixture: "orders",
        app: "Orders",
        task: "In the Orders app, type the order number shown on the notice board into the Order number field, press Submit, then confirm.",
        about: "mixed: painted text, a field, a confirmation dialog",
        check: check_orders,
        scripted: scripted_orders,
        batched: batched_orders,
    },
    Scenario {
        id: "counter",
        fixture: "counter",
        app: "Counter",
        task: "In the Counter app, press PLUS until the count is 3, checking the count after each press, then press DONE.",
        about: "all painted, nothing for accessibility: a count and three painted buttons",
        check: check_counter,
        scripted: scripted_counter,
        batched: scripted_counter,
    },
    Scenario {
        id: "long",
        fixture: "table",
        app: "Inventory",
        task: "In the Inventory app, open these items one after another, in this order: K-0012, K-0250, K-0137, K-0099, K-0201. To open an item, select its row and press Open.",
        about: "a longer session: the same table, five items in turn",
        check: check_long,
        scripted: scripted_long,
        batched: batched_long,
    },
];

// --------------------------------------------------------------------------
// checks

fn check_form(s: &Value) -> Result<(), String> {
    let want = json!({
        "name": "Ada Lovelace",
        "email": "ada@example.com",
        "country": "Japan",
        "subscribe": true,
    });
    match s.get("saved") {
        Some(saved) if *saved == want => Ok(()),
        Some(saved) => Err(format!("saved {saved}")),
        None => Err("never saved".into()),
    }
}

fn opened(s: &Value) -> Vec<String> {
    s.get("opened")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn check_table(s: &Value) -> Result<(), String> {
    let o = opened(s);
    if o == ["K-0137"] {
        Ok(())
    } else {
        Err(format!("opened {o:?}"))
    }
}

const LONG: [&str; 5] = ["K-0012", "K-0250", "K-0137", "K-0099", "K-0201"];

fn check_long(s: &Value) -> Result<(), String> {
    let o = opened(s);
    if o == LONG {
        Ok(())
    } else {
        Err(format!("opened {o:?}"))
    }
}

/// The shapes or boxes clicked (clicks on the empty canvas don't count).
fn hits(s: &Value) -> Vec<String> {
    s.get("clicks")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|c| c.get("hit").and_then(Value::as_str).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn check_board(s: &Value) -> Result<(), String> {
    let h = hits(s);
    if !h.is_empty() && h.iter().all(|b| b == "DELTA") {
        Ok(())
    } else {
        Err(format!("clicked {h:?}"))
    }
}

fn check_shapes(s: &Value) -> Result<(), String> {
    let h = hits(s);
    if !h.is_empty() && h.iter().all(|b| b == "red circle") {
        Ok(())
    } else {
        Err(format!("clicked {h:?}"))
    }
}

fn check_orders(s: &Value) -> Result<(), String> {
    let sub: Vec<&str> = s
        .get("submitted")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if sub == ["58213-QX"] {
        Ok(())
    } else {
        Err(format!("submitted {sub:?}"))
    }
}

// --------------------------------------------------------------------------
// scripted ways through: what a careful agent would call, using only what
// the tools have shown it (indices are read from earlier results)

fn need(i: Option<u32>, what: &str) -> Result<u32, String> {
    i.ok_or_else(|| format!("no {what} in what the tools showed"))
}

fn scripted_form(s: &mut Session) -> Result<(), String> {
    let app = s.app.clone();
    s.call("get_app_state", json!({"app": app}))?;
    let name = need(
        s.index(|l| l.starts_with("text field \"Full name\"")),
        "Full name field",
    )?;
    let email = need(
        s.index(|l| l.starts_with("text field \"Email\"")),
        "Email field",
    )?;
    s.call(
        "set_value",
        json!({"app": app, "element_index": name, "value": "Ada Lovelace"}),
    )?;
    s.call(
        "set_value",
        json!({"app": app, "element_index": email, "value": "ada@example.com"}),
    )?;
    let japan = need(
        s.index(|l| l.starts_with("radio button \"Japan\"")),
        "Japan",
    )?;
    s.call("click", json!({"app": app, "element_index": japan}))?;
    let sub = need(
        s.index(|l| {
            l.starts_with("check box \"Subscribe to newsletter\"")
                || l.starts_with("checkbox \"Subscribe to newsletter\"")
        }),
        "Subscribe check box",
    )?;
    s.call("click", json!({"app": app, "element_index": sub}))?;
    let save = need(s.index(|l| l.starts_with("button \"Save\"")), "Save")?;
    s.call("click", json!({"app": app, "element_index": save}))?;
    Ok(())
}

/// The SKU's cell after the filter was set: from what the tools showed
/// since `mark` (`seen.len()` before setting it), never from an older
/// result, whose indices may name a row the filter replaced. Not there:
/// wait for the cell, as a model would after "nothing changed".
fn sku_cell(s: &mut Session, mark: usize, sku: &str) -> Result<u32, String> {
    let quoted = format!("\"{sku}\"");
    if let Some(i) = s.index_since(mark, |l| l.contains(&quoted) && !l.contains("value=")) {
        return Ok(i);
    }
    let app = s.app.clone();
    let found = s.call(
        "wait_for",
        json!({"app": app, "role": "cell", "name": sku, "timeout_ms": 5000}),
    )?;
    need(found_index(&found), sku)
}

/// The index wait_for found ("Found after waiting: <index> <line>").
fn found_index(text: &str) -> Option<u32> {
    let rest = &text[text.find("Found after waiting: ")? + "Found after waiting: ".len()..];
    rest.split(' ').next()?.parse().ok()
}

/// Pressing Open is checked, not taken on trust: the app says
/// "Opened <sku>", and the action's answer must say it saw that. A server
/// before v3.6 ignores `expect` and reports no expectation: nothing to
/// check there, and the scenario's own check still judges the run.
fn opened_confirmed(text: &str, sku: &str) -> Result<(), String> {
    if !text.contains("Expected ") || text.contains(&format!("Expected Opened {sku}: confirmed")) {
        Ok(())
    } else {
        Err(format!("opening {sku} wasn't confirmed: {text}"))
    }
}

/// Filter the table to one SKU, select its row and press Open.
fn open_sku(s: &mut Session, sku: &str) -> Result<(), String> {
    let app = s.app.clone();
    let filter = need(s.index(|l| l.contains("\"Filter\"")), "filter field")?;
    let mark = s.seen.len();
    s.call(
        "set_value",
        json!({"app": app, "element_index": filter, "value": sku}),
    )?;
    let cell = sku_cell(s, mark, sku)?;
    // A GTK table cell's accessibility action doesn't select its row: a
    // double click with the mouse does (and an agent learns that after
    // "nothing changed").
    s.call(
        "click",
        json!({"app": app, "element_index": cell, "click_count": 2}),
    )?;
    let open = need(s.index(|l| l.starts_with("button \"Open\"")), "Open")?;
    let out = s.call(
        "click",
        json!({"app": app, "element_index": open, "expect": format!("Opened {sku}")}),
    )?;
    opened_confirmed(&out, sku)
}

fn scripted_table(s: &mut Session) -> Result<(), String> {
    let app = s.app.clone();
    s.call("get_app_state", json!({"app": app}))?;
    open_sku(s, "K-0137")
}

fn scripted_long(s: &mut Session) -> Result<(), String> {
    let app = s.app.clone();
    s.call("get_app_state", json!({"app": app}))?;
    for (n, sku) in LONG.iter().enumerate() {
        if n > 0 {
            // Clear the filter first: GTK 3 keeps the old row's cells over
            // AT-SPI when one filter replaces another.
            let filter = need(s.index(|l| l.contains("\"Filter\"")), "filter field")?;
            s.call(
                "set_value",
                json!({"app": app, "element_index": filter, "value": ""}),
            )?;
        }
        open_sku(s, sku)?;
    }
    Ok(())
}

fn scripted_board(s: &mut Session) -> Result<(), String> {
    let app = s.app.clone();
    s.call("get_app_state", json!({"app": app}))?;
    // The labels are painted: unless the look read them already, ask for
    // the text read off the screen.
    if s.index(|l| l.starts_with("ocr text \"DELTA")).is_none() {
        s.call("get_app_state", json!({"app": app, "ocr": true}))?;
    }
    if let Some(delta) = s.index(|l| l.starts_with("ocr text \"DELTA")) {
        s.call("click", json!({"app": app, "element_index": delta}))?;
        return Ok(());
    }
    // Not read: find it in the picture, as a model looking at the
    // screenshot would. The boxes share a fill; DELTA is the bottom-right
    // one.
    let found = s.call("locate", json!({"app": app, "color": "#D9EBFF"}))?;
    let (x, y, _, _) = areas(&found)
        .into_iter()
        .filter(|a| a.2 > 100.0)
        .max_by(|a, b| (a.0 + a.1).total_cmp(&(b.0 + b.1)))
        .ok_or("no boxes located")?;
    s.call("click", json!({"app": app, "x": x, "y": y}))?;
    Ok(())
}

fn check_counter(s: &Value) -> Result<(), String> {
    match s.get("done").and_then(Value::as_i64) {
        Some(3) => Ok(()),
        Some(n) => Err(format!("done at {n}")),
        None => Err("DONE never pressed".into()),
    }
}

/// A careful agent in an app it can only see: press, then look whether
/// the count went up, as a model checks each step there.
fn scripted_counter(s: &mut Session) -> Result<(), String> {
    let app = s.app.clone();
    s.call("get_app_state", json!({"app": app}))?;
    let ocr = |s: &Session, word: &str| s.index(|l| l.starts_with(&format!("ocr text \"{word}")));
    for n in 1..=3 {
        let plus = need(ocr(s, "PLUS"), "PLUS")?;
        s.call("click", json!({"app": app, "element_index": plus}))?;
        let look = s.call("get_app_state", json!({"app": app}))?;
        let shows = format!("Count: {n}");
        if !look.contains(&shows) && !s.seen.iter().rev().take(2).any(|t| t.contains(&shows)) {
            return Err(format!("the count isn't {n} after pressing PLUS: {look}"));
        }
    }
    let done = need(ocr(s, "DONE"), "DONE")?;
    s.call("click", json!({"app": app, "element_index": done}))?;
    Ok(())
}

fn scripted_shapes(s: &mut Session) -> Result<(), String> {
    let app = s.app.clone();
    s.call("get_app_state", json!({"app": app}))?;
    // Every red area; the circle is the lower one on this canvas.
    let found = s.call("locate", json!({"app": app, "color": "#E61A1A"}))?;
    let (x, y, _, _) = areas(&found)
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .ok_or("no red areas located")?;
    s.call("click", json!({"app": app, "x": x, "y": y}))?;
    Ok(())
}

/// The areas in locate's answer: "N at (x, y), box l,t to r,b; …", as
/// (centre x, centre y, width, height).
fn areas(text: &str) -> Vec<(f64, f64, f64, f64)> {
    let nums = |s: &str| -> Vec<f64> {
        s.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
            .filter_map(|t| t.parse().ok())
            .collect()
    };
    text.split(';')
        .filter_map(|part| {
            let at = part.find(" at (")?;
            let close = at + part[at..].find(')')?;
            let c = nums(&part[at + 5..close]);
            let b = nums(&part[part.find("box ")? + 4..]);
            match (&c[..], &b[..]) {
                ([x, y, ..], [l, t, r, b, ..]) => Some((*x, *y, r - l, b - t)),
                _ => None,
            }
        })
        .collect()
}

fn scripted_orders(s: &mut Session) -> Result<(), String> {
    let app = s.app.clone();
    let first = s.call("get_app_state", json!({"app": app}))?;
    // The notice is painted: unless the look read it already, ask for the
    // text read off the screen.
    let text = if first.contains("order number:") {
        first
    } else {
        s.call("get_app_state", json!({"app": app, "ocr": true}))?
    };
    let number = text
        .lines()
        .find_map(|l| {
            let at = l.find("order number:")?;
            let rest = &l[at + "order number:".len()..];
            rest.split(|c: char| c.is_whitespace() || c == '"')
                .find(|t| !t.is_empty())
                .map(String::from)
        })
        .ok_or("the notice wasn't read")?;
    let field = need(
        s.index(|l| l.starts_with("text field \"Order number\"")),
        "Order number field",
    )?;
    s.call(
        "set_value",
        json!({"app": app, "element_index": field, "value": number}),
    )?;
    let submit = need(s.index(|l| l.starts_with("button \"Submit\"")), "Submit")?;
    s.call("click", json!({"app": app, "element_index": submit}))?;
    let confirm = need(s.index(|l| l.starts_with("button \"Confirm\"")), "Confirm")?;
    s.call("click", json!({"app": app, "element_index": confirm}))?;
    Ok(())
}

// --------------------------------------------------------------------------
// the same with fewer round trips (v3.6): what is known is done in one
// batch of lines, by name where the name is plain

fn batched_form(s: &mut Session) -> Result<(), String> {
    let app = s.app.clone();
    s.call("get_app_state", json!({"app": app}))?;
    let name = need(
        s.index(|l| l.starts_with("text field \"Full name\"")),
        "Full name field",
    )?;
    let email = need(
        s.index(|l| l.starts_with("text field \"Email\"")),
        "Email field",
    )?;
    s.call(
        "batch",
        json!({"app": app, "steps": [
            format!("set {name} \"Ada Lovelace\""),
            format!("set {email} \"ada@example.com\""),
            "click \"Japan\"",
            "click \"Subscribe to newsletter\"",
            "click \"Save\"",
        ]}),
    )?;
    Ok(())
}

/// Filter to one SKU, then select its row and press Open in one batch.
fn batched_open(s: &mut Session, sku: &str, clear: bool) -> Result<(), String> {
    let app = s.app.clone();
    let filter = need(s.index(|l| l.contains("\"Filter\"")), "filter field")?;
    if clear {
        // GTK 3 keeps the old row's cells when one filter replaces
        // another: clear it first.
        s.call(
            "set_value",
            json!({"app": app, "element_index": filter, "value": ""}),
        )?;
    }
    let mark = s.seen.len();
    s.call(
        "set_value",
        json!({"app": app, "element_index": filter, "value": sku}),
    )?;
    let cell = sku_cell(s, mark, sku)?;
    let out = s.call(
        "batch",
        json!({"app": app, "steps": [
            format!("double {cell}"),
            format!("click \"Open\" expect \"Opened {sku}\""),
        ]}),
    )?;
    opened_confirmed(&out, sku)
}

fn batched_table(s: &mut Session) -> Result<(), String> {
    let app = s.app.clone();
    s.call("get_app_state", json!({"app": app}))?;
    batched_open(s, "K-0137", false)
}

fn batched_long(s: &mut Session) -> Result<(), String> {
    let app = s.app.clone();
    s.call("get_app_state", json!({"app": app}))?;
    for (n, sku) in LONG.iter().enumerate() {
        batched_open(s, sku, n > 0)?;
    }
    Ok(())
}

fn batched_orders(s: &mut Session) -> Result<(), String> {
    let app = s.app.clone();
    let first = s.call("get_app_state", json!({"app": app}))?;
    let text = if first.contains("order number:") {
        first
    } else {
        s.call("get_app_state", json!({"app": app, "ocr": true}))?
    };
    let number = text
        .lines()
        .find_map(|l| {
            let at = l.find("order number:")?;
            let rest = &l[at + "order number:".len()..];
            rest.split(|c: char| c.is_whitespace() || c == '"')
                .find(|t| !t.is_empty())
                .map(String::from)
        })
        .ok_or("the notice wasn't read")?;
    let field = need(
        s.index(|l| l.starts_with("text field \"Order number\"")),
        "Order number field",
    )?;
    s.call(
        "batch",
        json!({"app": app, "steps": [
            format!("set {field} \"{number}\""),
            "click \"Submit\" expect dialog",
            "click \"Confirm\"",
        ]}),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_index_wait_for_found_is_read_from_its_answer() {
        assert_eq!(
            found_index("Found after waiting: 42 cell \"K-0137\""),
            Some(42)
        );
        assert_eq!(found_index("timed out after 5000ms"), None);
    }

    #[test]
    fn an_open_counts_only_when_its_expectation_was_confirmed() {
        let sku = "K-0137";
        assert!(opened_confirmed("Clicked 7. Expected Opened K-0137: confirmed.", sku).is_ok());
        assert!(opened_confirmed("Clicked 7. Expected Opened K-0137: not seen.", sku).is_err());
        assert!(opened_confirmed("Clicked 7. Expected Opened K-0012: confirmed.", sku).is_err());
    }

    #[test]
    fn an_open_on_a_server_without_expectations_is_not_failed() {
        assert!(opened_confirmed("Clicked 7.\nChanged: label \"Opened K-0137\"", "K-0137").is_ok());
    }
}
