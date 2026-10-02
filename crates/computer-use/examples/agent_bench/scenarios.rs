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
    },
    Scenario {
        id: "table",
        fixture: "table",
        app: "Inventory",
        task: "In the Inventory app, open the item whose SKU is K-0137: select its row, then press Open.",
        about: "a 300-row table with a filter",
        check: check_table,
        scripted: scripted_table,
    },
    Scenario {
        id: "board",
        fixture: "board",
        app: "Board",
        task: "In the Board app, click the box labelled DELTA on the canvas.",
        about: "a canvas of labelled boxes under a full toolbar (text the tree doesn't have)",
        check: check_board,
        scripted: scripted_board,
    },
    Scenario {
        id: "shapes",
        fixture: "shapes",
        app: "Shapes",
        task: "In the Shapes app, click the red circle on the canvas.",
        about: "a canvas of shapes without any text",
        check: check_shapes,
        scripted: scripted_shapes,
    },
    Scenario {
        id: "orders",
        fixture: "orders",
        app: "Orders",
        task: "In the Orders app, type the order number shown on the notice board into the Order number field, press Submit, then confirm.",
        about: "mixed: painted text, a field, a confirmation dialog",
        check: check_orders,
        scripted: scripted_orders,
    },
    Scenario {
        id: "long",
        fixture: "table",
        app: "Inventory",
        task: "In the Inventory app, open these items one after another, in this order: K-0012, K-0250, K-0137, K-0099, K-0201. To open an item, select its row and press Open.",
        about: "a longer session: the same table, five items in turn",
        check: check_long,
        scripted: scripted_long,
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

/// Filter the table to one SKU, select its row and press Open.
fn open_sku(s: &mut Session, sku: &str) -> Result<(), String> {
    let app = s.app.clone();
    let filter = need(s.index(|l| l.contains("\"Filter\"")), "filter field")?;
    s.call(
        "set_value",
        json!({"app": app, "element_index": filter, "value": sku}),
    )?;
    let quoted = format!("\"{sku}\"");
    let cell = match s.index(|l| l.contains(&quoted) && !l.contains("value=")) {
        Some(i) => i,
        None => {
            // Not in the change report: look again. GTK 3 can keep a
            // filtered table's old cell text over AT-SPI while the picture
            // shows the one row left; that row's first cell is the one.
            s.call("get_app_state", json!({"app": app}))?;
            match s.index(|l| l.contains(&quoted) && !l.contains("value=")) {
                Some(i) => i,
                None => need(s.index(|l| l.starts_with("cell \"K-")), sku)?,
            }
        }
    };
    // A GTK table cell's accessibility action doesn't select its row: a
    // double click with the mouse does (and an agent learns that after
    // "nothing changed").
    s.call(
        "click",
        json!({"app": app, "element_index": cell, "click_count": 2}),
    )?;
    let open = need(s.index(|l| l.starts_with("button \"Open\"")), "Open")?;
    s.call("click", json!({"app": app, "element_index": open}))?;
    Ok(())
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
    // The labels are painted: ask for the text read off the screen.
    s.call("get_app_state", json!({"app": app, "ocr": true}))?;
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
    s.call("get_app_state", json!({"app": app}))?;
    let text = s.call("get_app_state", json!({"app": app, "ocr": true}))?;
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
