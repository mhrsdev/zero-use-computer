//! The panel's Connect page: which agents are on this computer, whether
//! this program is in them, and a button to add or take it out.

use std::path::Path;

use serde_json::{Value, json};

use super::settings::fail;
use crate::connect::{self, Client, Env, State};

fn state_json(s: &State) -> Value {
    match s {
        State::NotFound => json!({"kind": "not_found"}),
        State::NotInstalled => json!({"kind": "not_installed"}),
        State::Installed => json!({"kind": "installed"}),
        State::Different(p) => json!({"kind": "different", "points_at": p}),
        State::Unreadable(why) => json!({"kind": "unreadable", "why": why}),
    }
}

/// Every agent, and the program as it would be registered.
pub fn list(env: &Env, exe: &Path) -> Value {
    let program = connect::stable_program_path(env);
    let clients: Vec<Value> = connect::detect(env, &program)
        .into_iter()
        .map(|i| {
            json!({
                "id": i.client.id(),
                "label": i.client.label(),
                "state": state_json(&i.state),
                "where": i.where_,
                "entry": connect::entry_text(i.client, &program),
                "after": i.client.after(),
            })
        })
        .collect();
    json!({
        "ok": true,
        "clients": clients,
        "program": program.display().to_string(),
        "running_from": exe.display().to_string(),
        "will_copy": !same(&program, exe),
        "unstable": connect::unstable_place(exe),
    })
}

fn same(a: &Path, b: &Path) -> bool {
    a == b
        || std::fs::canonicalize(a)
            .ok()
            .zip(std::fs::canonicalize(b).ok())
            .is_some_and(|(x, y)| x == y)
}

/// `{client, action: "install" | "remove", confirmed}`.
pub fn act(env: &Env, exe: &Path, body: &Value) -> Value {
    let Some(client) = body
        .get("client")
        .and_then(Value::as_str)
        .and_then(Client::parse)
    else {
        return fail("choose an agent");
    };
    let remove = match body.get("action").and_then(Value::as_str) {
        Some("install") => false,
        Some("remove") => true,
        _ => return fail("say install or remove"),
    };
    let confirmed = body.get("confirmed").and_then(Value::as_bool) == Some(true);
    let program = connect::stable_program_path(env);
    let info = connect::info(env, client, &program);
    if let State::Unreadable(why) = &info.state {
        return fail(&format!(
            "{}'s settings can't be changed safely: {why}. Copy the entry and add it by hand.",
            client.label()
        ));
    }
    if matches!(info.state, State::NotFound) && !remove && client != Client::Codex {
        return fail(&format!(
            "{} doesn't seem to be installed here",
            client.label()
        ));
    }
    if !confirmed {
        let text = if remove {
            format!(
                "Take Computer Use out of {}? This changes {} (a copy is kept next to it as .bak).",
                client.label(),
                info.where_
            )
        } else {
            let copy = if same(&program, exe) {
                String::new()
            } else {
                format!(
                    " A copy of this program is kept at {} so it can be found later.",
                    program.display()
                )
            };
            format!(
                "Add Computer Use to {}? This changes {} (a copy is kept next to it as .bak) and adds:\n\n{}\n{copy}",
                client.label(),
                info.where_,
                connect::entry_text(client, &program)
            )
        };
        return json!({"ok": false, "error": "this needs the user's confirmation first", "confirm_text": text});
    }
    let done = if remove {
        connect::remove(env, client).map(|m| (m, None))
    } else {
        let (registered, note) = connect::program_for_clients(env, exe);
        connect::install(env, client, &registered).map(|m| (m, note))
    };
    match done {
        Ok((message, note)) => json!({"ok": true, "message": message, "note": note}),
        Err(e) => fail(&e.to_string()),
    }
}
