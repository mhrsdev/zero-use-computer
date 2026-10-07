//! The panel's page for its own shortcut: on the desktop and with the
//! system's apps, made, made again or taken away from here.

use std::path::Path;

use serde_json::{Value, json};

use super::settings::fail;
use crate::connect::{self, Env};
use crate::shortcut::{self, Place, Places};

pub fn list(p: &Places, env: &Env, exe: &Path) -> Value {
    let program = connect::stable_program_path(env);
    let items: Vec<Value> = shortcut::status(p)
        .into_iter()
        .map(|s| {
            json!({
                "id": s.place.id(),
                "label": s.place.label(p.os),
                "path": s.path.as_ref().map(|f| f.display().to_string()),
                "exists": s.exists,
                "ours": s.ours,
                "starts": s.starts,
                "current": s.starts.as_deref().is_none_or(|x| Path::new(x) == program),
            })
        })
        .collect();
    json!({
        "ok": true,
        "shortcuts": items,
        "program": program.display().to_string(),
        "running_from": exe.display().to_string(),
    })
}

/// `{place, action: "create" | "remove", confirmed}`.
pub fn act(p: &Places, env: &Env, exe: &Path, body: &Value) -> Value {
    let Some(place) = body
        .get("place")
        .and_then(Value::as_str)
        .and_then(Place::parse)
    else {
        return fail("say desktop or menu");
    };
    let remove = match body.get("action").and_then(Value::as_str) {
        Some("create") => false,
        Some("remove") => true,
        _ => return fail("say create or remove"),
    };
    let Some(path) = p.path(place) else {
        return fail(&format!("this system has no {}", place.label(p.os)));
    };
    if body.get("confirmed").and_then(Value::as_bool) != Some(true) {
        let text = if remove {
            format!("Take the panel's shortcut away from {}?", path.display())
        } else {
            format!(
                "Put a shortcut to this panel at {}? It starts {} settings, which opens the panel in your browser.",
                path.display(),
                connect::stable_program_path(env).display()
            )
        };
        return json!({"ok": false, "error": "this needs the user's confirmation first", "confirm_text": text});
    }
    if remove {
        return match shortcut::remove(p, place) {
            Ok(true) => json!({"ok": true, "message": format!("Removed {}.", path.display())}),
            Ok(false) => json!({"ok": true, "message": "There was none."}),
            Err(e) => fail(&e.to_string()),
        };
    }
    let (program, note) = connect::program_for_clients(env, exe);
    match shortcut::create(p, place, &program) {
        Ok(made) => {
            json!({"ok": true, "message": format!("Made {}.", made.display()), "note": note})
        }
        Err(e) => fail(&e.to_string()),
    }
}
