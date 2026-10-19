//! The host's settings API, through its public requests, on a temporary
//! folder: nothing here touches a screen or a real settings file.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::host::{Host, METHOD_PREFIX, SCHEMA_VERSION};

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cu-host-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn ask(host: &Host, name: &str, params: Value) -> Result<Value, (String, Value)> {
    host.handle(&format!("{METHOD_PREFIX}{name}"), &params)
        .expect("a request of this API")
        .map_err(|e| (e.code.to_string(), e.data()))
}

fn code(host: &Host, name: &str, params: Value) -> String {
    ask(host, name, params).expect_err("refused").0
}

/// A host that named these settings as the user's.
fn host_with(name: &str, user_keys: &[&str], managed_rest: &str) -> (Host, PathBuf) {
    let dir = folder(name);
    let keys: Vec<String> = user_keys.iter().map(|k| format!("\"{k}\"")).collect();
    std::fs::write(
        dir.join("managed.toml"),
        format!(
            "{managed_rest}\n[host]\nuser_keys = [{}]\n",
            keys.join(", ")
        ),
    )
    .unwrap();
    (Host::in_home(&dir), dir)
}

#[test]
fn a_host_that_named_no_setting_has_made_none_the_users() {
    let dir = folder("none");
    let host = Host::in_home(&dir);
    for key in ["mouse_speed", "overlay.scale", "screenshot.attach"] {
        assert_eq!(
            code(&host, "set", json!({"key": key, "value": 1})),
            "managedByHost",
            "{key}"
        );
    }
    assert!(!dir.join("user.toml").exists(), "nothing was written");
}

#[test]
fn a_setting_the_host_named_is_written_to_the_users_file_and_read_back() {
    let (host, dir) = host_with("write", &["mouse_speed"], "");
    let reply = ask(&host, "set", json!({"key": "mouse_speed", "value": 1.5})).unwrap();
    assert_eq!(reply["value"], json!(1.5));
    assert_eq!(reply["restart"], json!(false));
    let user = std::fs::read_to_string(dir.join("user.toml")).unwrap();
    assert!(user.contains("mouse_speed = 1.5"), "{user}");
    let values = ask(&host, "values", json!({})).unwrap();
    assert_eq!(values["values"]["mouse_speed"], json!(1.5));
    assert_eq!(values["writable"], json!(["mouse_speed"]));
}

#[test]
fn a_setting_the_host_did_not_name_is_refused_and_the_file_is_not_touched() {
    let (host, dir) = host_with("locked", &["mouse_speed"], "");
    // `clipboard` is real, and not the user's here.
    assert_eq!(
        code(&host, "set", json!({"key": "clipboard", "value": true})),
        "managedByHost"
    );
    assert_eq!(
        code(&host, "reset", json!({"key": "clipboard"})),
        "managedByHost"
    );
    assert!(!dir.join("user.toml").exists());
}

#[test]
fn a_setting_the_host_sets_itself_wins_and_cannot_be_changed() {
    // The host names the key as the user's and also sets it: its own value stands.
    let (host, dir) = host_with("wins", &["mouse_speed"], "mouse_speed = 1.0\n");
    std::fs::write(dir.join("user.toml"), "mouse_speed = 2.0\n").unwrap();
    assert_eq!(
        code(&host, "set", json!({"key": "mouse_speed", "value": 1.5})),
        "managedByHost"
    );
    let values = ask(&host, "values", json!({})).unwrap();
    // 1.0 is the default speed: the effective value is the default, not the user's 2.0.
    assert!(values["values"].get("mouse_speed").is_none(), "{values}");
    assert_eq!(values["writable"], json!([]));
}

#[test]
fn a_locked_setting_written_into_the_users_file_by_hand_is_not_read() {
    let (host, dir) = host_with("hand", &["mouse_speed"], "");
    std::fs::write(
        dir.join("user.toml"),
        "mouse_speed = 1.5\nclipboard = true\n[screenshot]\nattach = \"always\"\n",
    )
    .unwrap();
    let values = ask(&host, "values", json!({})).unwrap();
    assert_eq!(values["values"]["mouse_speed"], json!(1.5));
    assert!(values["values"].get("clipboard").is_none(), "{values}");
    assert!(
        values["values"].get("screenshot.attach").is_none(),
        "{values}"
    );
}

#[test]
fn a_wrong_value_is_refused_with_a_code_and_what_the_host_needs() {
    let (host, dir) = host_with(
        "wrong",
        &[
            "mouse_speed",
            "screenshot.attach",
            "overlay.show_label",
            "screenshot.max_dimension",
        ],
        "",
    );
    let (c, data) = ask(&host, "set", json!({"key": "mouse_speed", "value": 9})).unwrap_err();
    assert_eq!(c, "outOfRange");
    assert_eq!(
        data["data"],
        Value::Null,
        "the code is in `code`, not nested"
    );
    assert_eq!(data["key"], json!("mouse_speed"));
    assert_eq!(data["detail"], json!({"min": 0.5, "max": 2.0}));
    let (c, data) = ask(
        &host,
        "set",
        json!({"key": "screenshot.attach", "value": "sometimes"}),
    )
    .unwrap_err();
    assert_eq!(c, "notAChoice");
    assert_eq!(
        data["detail"]["choices"],
        json!(["auto", "always", "never"])
    );
    assert_eq!(
        code(
            &host,
            "set",
            json!({"key": "overlay.show_label", "value": "yes"})
        ),
        "notOnOff"
    );
    assert_eq!(
        code(
            &host,
            "set",
            json!({"key": "screenshot.max_dimension", "value": 1000.5})
        ),
        "notAWholeNumber"
    );
    assert_eq!(
        code(
            &host,
            "set",
            json!({"key": "screenshot.max_dimension", "value": "ten"})
        ),
        "notANumber"
    );
    assert_eq!(
        code(&host, "set", json!({"key": "nope", "value": 1})),
        "unknownKey"
    );
    assert_eq!(code(&host, "set", json!({"value": 1})), "badParams");
    assert_eq!(
        code(&host, "set", json!({"key": "mouse_speed"})),
        "badParams"
    );
    assert!(
        !dir.join("user.toml").exists(),
        "a refused change writes nothing"
    );
}

#[test]
fn a_setting_that_asks_for_confirmation_needs_it_said() {
    let (host, _dir) = host_with("confirm", &["privacy.redact_passwords"], "");
    let params = json!({"key": "privacy.redact_passwords", "value": false});
    assert_eq!(code(&host, "set", params.clone()), "needsConfirmation");
    let mut confirmed = params;
    confirmed["confirmed"] = json!(true);
    assert!(ask(&host, "set", confirmed).is_ok());
}

#[test]
fn a_setting_changed_on_its_own_page_in_the_panel_is_not_one_here() {
    let (host, _dir) = host_with("custom", &["decision.model", "decision.api_key"], "");
    assert_eq!(
        code(&host, "set", json!({"key": "decision.model", "value": "x"})),
        "notEditable"
    );
    assert_eq!(
        code(
            &host,
            "set",
            json!({"key": "decision.api_key", "value": "x"})
        ),
        "notEditable"
    );
}

#[test]
fn reset_puts_the_setting_back_to_its_default() {
    let (host, dir) = host_with("reset", &["mouse_speed"], "");
    ask(&host, "set", json!({"key": "mouse_speed", "value": 1.5})).unwrap();
    let reply = ask(&host, "reset", json!({"key": "mouse_speed"})).unwrap();
    assert_eq!(reply["value"], json!(1.0));
    let user = std::fs::read_to_string(dir.join("user.toml")).unwrap();
    assert!(!user.contains("mouse_speed"), "{user}");
    let values = ask(&host, "values", json!({})).unwrap();
    assert!(values["values"].get("mouse_speed").is_none());
}

#[test]
fn the_hosts_own_table_is_not_a_setting_and_does_not_break_reading() {
    let (host, _dir) = host_with("table", &["mouse_speed"], "text_only = true\n");
    let store = host.load().expect("both layers read");
    assert!(
        store.config.text_only,
        "the host's layer is laid over the defaults"
    );
}

#[test]
fn a_broken_layer_is_an_error_not_the_defaults() {
    let (host, dir) = host_with("broken", &["mouse_speed"], "");
    std::fs::write(dir.join("user.toml"), "mouse_speed = [").unwrap();
    assert_eq!(code(&host, "values", json!({})), "settingsUnreadable");
    // Writing does not hide it either.
    assert_eq!(
        code(&host, "set", json!({"key": "mouse_speed", "value": 1.5})),
        "writeFailed"
    );
}

#[test]
fn the_schema_describes_every_setting_and_says_which_shape_it_is() {
    let host = Host::in_home(&folder("schema"));
    let schema = ask(&host, "schema", json!({})).unwrap();
    assert_eq!(schema["schemaVersion"], json!(SCHEMA_VERSION));
    let entries = schema["entries"].as_array().unwrap();
    assert_eq!(entries.len(), super::schema::ENTRIES.len());
    let speed = entries.iter().find(|e| e["key"] == "mouse_speed").unwrap();
    assert_eq!(speed["type"], json!("range"));
    assert_eq!(speed["min"], json!(0.5));
    assert_eq!(speed["default"], json!(1.0));
    let languages = schema["languages"].as_array().unwrap();
    assert!(
        languages
            .iter()
            .any(|l| l["code"] == "fa" && l["rtl"] == true)
    );
}

#[test]
fn the_texts_come_in_the_language_asked_and_in_english_when_one_is_missing() {
    let host = Host::in_home(&folder("texts"));
    let en = ask(&host, "texts", json!({})).unwrap();
    assert_eq!(en["language"], json!("en"));
    assert_eq!(en["rtl"], json!(false));
    assert_eq!(en["settings"]["mouse_speed"][0], json!("Mouse speed"));
    let fa = ask(&host, "texts", json!({"language": "fa"})).unwrap();
    assert_eq!(fa["rtl"], json!(true));
    assert_ne!(
        fa["settings"]["clipboard"][0],
        en["settings"]["clipboard"][0]
    );
    assert_eq!(
        fa["groups"]["Pointer"],
        json!("\u{646}\u{634}\u{627}\u{646}\u{6AF}\u{631}")
    );
    assert_eq!(
        code(&host, "texts", json!({"language": "xx"})),
        "unknownLanguage"
    );
}

#[test]
fn a_request_that_is_not_of_this_api_is_not_answered_by_it() {
    let host = Host::in_home(&folder("other"));
    assert!(host.handle("tools/list", &json!({})).is_none());
    assert!(
        host.handle(&format!("{METHOD_PREFIX}nothing"), &json!({}))
            .is_none()
    );
}

#[test]
fn the_path_request_gives_the_routes_of_the_programs_own_path_maker() {
    let (host, _dir) = host_with("path", &[], "");
    for style in ["hand", "sine", "arc", "spring", "spiral", "mixed"] {
        let v = ask(&host, "path", json!({ "style": style })).unwrap();
        let paths = v["paths"].as_array().expect("routes");
        assert_eq!(paths.len(), 3, "{style}");
        for path in paths {
            let points = path["points"].as_array().unwrap();
            assert!(points.len() >= 2, "{style}: a route has a start and an end");
            assert!(path["ms"].as_f64().unwrap() > 0.0);
        }
        assert_eq!(v["jumps"], json!(false));
    }
    // The real mouse's glide is the one the settings describe, and says it jumps.
    let real = ask(&host, "path", json!({ "style": "hand", "real": true })).unwrap();
    assert_eq!(real["jumps"], json!(true));
    // A style that is not one is refused with a code, and the default is "mixed".
    assert_eq!(
        code(&host, "path", json!({ "style": "zigzag" })),
        "badParams"
    );
    assert!(ask(&host, "path", json!({})).is_ok());
}

#[test]
fn the_tools_request_lists_every_tool_with_whether_it_is_offered_and_its_cost() {
    let (host, _dir) = host_with("tools", &[], "[tools]\nenabled = [\"screenshot\"]\n");
    let v = ask(&host, "tools", json!({})).unwrap();
    let tools = v["tools"].as_array().expect("tools");
    assert!(tools.len() > 5);
    assert!(tools.iter().all(|t| t["tokens"].as_u64().unwrap() > 0));
    let offered: Vec<&str> = tools
        .iter()
        .filter(|t| t["offered"] == json!(true))
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        offered,
        ["screenshot"],
        "only what the host's layer allows is offered"
    );
}
