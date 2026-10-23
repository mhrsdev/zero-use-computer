//! The host's pointer picture, sound test and profiles (`cursor*`, `sound_test`, `profile*`),
//! through its public requests, on a temporary folder. A sound that plays is tested over the
//! real program's stdio (`computer-use-mcp/tests/host_stdio.rs`): nothing here can make a noise.

use std::path::Path;

use base64::Engine as _;
use serde_json::{Value, json};

use super::host_tests::{ask, code, host_with};
use super::schema;

/// A picture of one point (GIF), as the overlay reads it.
const GIF_1X1: &str = "R0lGODlhAQABAIAAAAUEBAAAACwAAAAAAQABAAACAkQBADs=";
const PICTURE: [&str; 2] = ["overlay.cursor_image", "overlay.cursor_style"];

fn base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(width, height)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

/// No folder of this computer is in the answer.
fn names_no_folder(answer: &Value, dir: &Path) {
    let text = answer.to_string();
    let dir = serde_json::to_string(&dir.display().to_string()).unwrap();
    assert!(!text.contains(&dir[1..dir.len() - 1]), "{text}");
}

fn kept_pictures(dir: &Path) -> usize {
    std::fs::read_dir(dir.join("cursors")).map_or(0, |rd| rd.flatten().count())
}

fn profile_keys(list: &Value, id: &str) -> Vec<String> {
    let row = list["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id)
        .unwrap_or_else(|| panic!("no profile {id}: {list}"));
    let mut keys: Vec<String> = row["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["key"].as_str().unwrap().to_string())
        .collect();
    keys.sort();
    keys
}

#[test]
fn with_no_picture_the_pointer_reads_as_none_and_the_limits_are_the_overlays_own() {
    let (host, dir) = host_with("pic-none", &PICTURE, "");
    let v = ask(&host, "cursor", json!({})).unwrap();
    assert_eq!(v["set"], false);
    assert_eq!(v["kept"], false);
    for key in ["usable", "format", "width", "height", "frames", "note"] {
        assert!(v[key].is_null(), "{key}: {v}");
    }
    assert!(v.get("path").is_none());
    names_no_folder(&v, &dir);
    // What the overlay's reader takes, and inside what a host believes at most.
    for (key, ceiling) in [
        ("fileMb", 16),
        ("side", 8192),
        ("frames", 10_000),
        ("seconds", 3_600),
    ] {
        let n = v["limits"][key].as_u64().unwrap();
        assert!((1..=ceiling).contains(&n), "{key}: {n}");
    }
    assert_eq!(v["limits"]["fileMb"], 4);
}

#[test]
fn a_picture_is_kept_set_read_back_and_taken_away_without_naming_a_folder() {
    let (host, dir) = host_with("pic-life", &PICTURE, "");
    ask(
        &host,
        "set",
        json!({"key": "overlay.cursor_style", "value": "metal"}),
    )
    .unwrap();
    // Only {ok}: the settings' own answer holds the kept file's path.
    let set = ask(&host, "cursor_set", json!({"data": GIF_1X1})).unwrap();
    assert_eq!(set, json!({"ok": true}));
    let now = ask(&host, "cursor", json!({})).unwrap();
    assert_eq!(
        (
            now["set"].clone(),
            now["kept"].clone(),
            now["usable"].clone()
        ),
        (json!(true), json!(true), json!(true))
    );
    assert_eq!(now["format"], "gif");
    assert_eq!(
        (now["width"].clone(), now["height"].clone()),
        (json!(1), json!(1))
    );
    assert_eq!(now["frames"], 1);
    names_no_folder(&now, &dir);
    let values = ask(&host, "values", json!({})).unwrap();
    assert_eq!(values["values"]["overlay.cursor_style"], "custom");

    // A PNG replaces it: one kept picture at a time.
    ask(&host, "cursor_set", json!({"data": base64(&png(8, 4))})).unwrap();
    let now = ask(&host, "cursor", json!({})).unwrap();
    assert_eq!(now["format"], "png");
    assert_eq!(
        (now["width"].clone(), now["height"].clone()),
        (json!(8), json!(4))
    );
    assert_eq!(
        kept_pictures(&dir),
        2,
        "the picture and the style it replaced"
    );

    // Away: the pointer it replaced comes back, and the kept file goes.
    assert_eq!(
        ask(&host, "cursor_clear", json!({})).unwrap(),
        json!({"ok": true})
    );
    assert_eq!(ask(&host, "cursor", json!({})).unwrap()["set"], false);
    let values = ask(&host, "values", json!({})).unwrap();
    assert_eq!(values["values"]["overlay.cursor_style"], "metal");
    assert_eq!(kept_pictures(&dir), 0);
    // Again: nothing to take away is not a failure.
    assert_eq!(
        ask(&host, "cursor_clear", json!({})).unwrap(),
        json!({"ok": true})
    );
}

#[test]
fn what_is_not_a_picture_is_refused_with_a_code_and_nothing_is_kept_or_changed() {
    let (host, dir) = host_with("pic-bad", &PICTURE, "");
    let over = base64(&vec![0u8; 4 * 1024 * 1024 + 1]);
    let wide = base64(&png(513, 1));
    for (data, why) in [
        (json!("###"), "not base64"),
        (json!(base64(b"not a picture")), "text"),
        (json!(over), "over the file limit"),
        (json!(wide), "over the side limit"),
    ] {
        let (word, answer) = ask(&host, "cursor_set", json!({"data": data})).unwrap_err();
        assert_eq!(word, "notAPicture", "{why}");
        assert!(
            answer["detail"]["reason"]
                .as_str()
                .is_some_and(|r| !r.is_empty())
        );
        names_no_folder(&answer, &dir);
    }
    for params in [json!({}), json!({"data": 5}), json!({"data": null})] {
        assert_eq!(
            code(&host, "cursor_set", params.clone()),
            "badParams",
            "{params}"
        );
    }
    assert!(!dir.join("user.toml").exists(), "no setting was written");
    assert_eq!(kept_pictures(&dir), 0);
}

#[test]
fn a_picture_that_cannot_be_kept_is_refused_without_the_folder_in_its_words() {
    let (host, dir) = host_with("pic-keep", &PICTURE, "");
    std::fs::write(dir.join("cursors"), "a file where the folder should be").unwrap();
    let (word, answer) = ask(&host, "cursor_set", json!({"data": GIF_1X1})).unwrap_err();
    assert_eq!(word, "refused");
    assert!(answer["detail"]["reason"].is_string(), "{answer}");
    names_no_folder(&answer, &dir);
    assert!(!dir.join("user.toml").exists());
}

#[test]
fn a_picture_is_the_users_only_when_the_host_gave_both_of_its_settings_to_the_user() {
    for (name, keys, managed) in [
        ("pic-none-given", vec!["mouse_speed"], ""),
        ("pic-style-only", vec!["overlay.cursor_style"], ""),
        ("pic-image-only", vec!["overlay.cursor_image"], ""),
        (
            "pic-host-style",
            PICTURE.to_vec(),
            "[overlay]\ncursor_style = \"metal\"\n",
        ),
    ] {
        let (host, dir) = host_with(name, &keys, managed);
        for (request, params) in [
            ("cursor_set", json!({"data": GIF_1X1})),
            ("cursor_clear", json!({})),
        ] {
            let (word, answer) = ask(&host, request, params).unwrap_err();
            assert_eq!(word, "managedByHost", "{name} {request}");
            assert!(answer["key"].is_string(), "{name}: the key it is about");
        }
        assert!(!dir.join("user.toml").exists(), "{name}");
        assert_eq!(kept_pictures(&dir), 0, "{name}");
    }
}

#[test]
fn a_sound_test_with_a_request_that_is_not_clear_plays_nothing() {
    let (host, _dir) = host_with("sound-bad", &[], "");
    // (Each is refused before any player is looked for, so none can play here.)
    for params in [
        json!({}),
        json!({"event": "boom"}),
        json!({"event": 5}),
        json!({"event": "click", "dryRun": "false"}),
        json!({"event": "click", "dryRun": 0}),
        json!({"event": "click", "dryRun": null}),
    ] {
        assert_eq!(
            code(&host, "sound_test", params.clone()),
            "badParams",
            "{params}"
        );
    }
}

#[test]
fn the_profiles_that_come_with_the_program_say_what_they_set_and_set_only_what_a_host_may() {
    let (host, _dir) = host_with("prof-list", &[], "");
    let list = ask(&host, "profiles", json!({})).unwrap();
    let rows = list["profiles"].as_array().unwrap();
    assert!(rows.len() <= 50);
    let ids: Vec<&str> = rows.iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["balanced", "low-tokens", "best-quality", "showcase"]);
    for row in rows {
        assert_eq!(row["builtin"], true);
        assert!(row["label"].is_string() && row["blurb"].is_string());
        let changes = row["changes"].as_array().unwrap();
        assert!(!changes.is_empty() && changes.len() <= 400);
        for change in changes {
            let key = change["key"].as_str().unwrap();
            let entry = schema::entry(key).unwrap_or_else(|| panic!("{key}"));
            // (A value is always given: null is "back to the default".)
            assert!(change.get("value").is_some(), "{key}");
            assert!(!entry.confirm && !entry.restart, "{key}");
        }
    }
    let balanced = &rows[0]["changes"];
    assert!(
        balanced
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["value"].is_null())
    );
}

#[test]
fn a_profile_file_cannot_make_a_profile_hold_a_secret_a_protected_setting_or_a_place() {
    let (host, dir) = host_with("prof-evil", &["mouse_speed"], "");
    std::fs::create_dir_all(dir.join("profiles")).unwrap();
    let values = json!({
        "mouse_speed": 2.0,
        "overlay.cursor_image": "/somewhere/else.png",
        "overlay.sound_click_file": "/somewhere/else.wav",
        "overlay.font": "/somewhere/else.ttf",
        "ocr.tesseract_path": "/somewhere/a-program",
        "script.dir": "/somewhere",
        "script.files": "all",
        "audit.path": "/somewhere/log",
        "server.http_token": "secret",
        "server.log": "trace",
        "not.a.setting": 1,
    });
    std::fs::write(
        dir.join("profiles/evil.json"),
        json!({"label": "Evil", "values": values}).to_string(),
    )
    .unwrap();
    let list = ask(&host, "profiles", json!({})).unwrap();
    assert_eq!(profile_keys(&list, "evil"), ["mouse_speed"]);
    names_no_folder(&list, &dir);
}

#[test]
fn a_profile_is_kept_from_what_the_user_changed_and_may_change_nothing_else() {
    // The host lets the user change these two and itself sets a third.
    let managed = "[screenshot]\nmax_dimension = 1000\n";
    let mut keys = vec!["mouse_speed", "overlay.scale"];
    keys.extend(PICTURE);
    let (host, dir) = host_with("prof-save", &keys, managed);

    // A name that is not one: nothing is written.
    let long = "x".repeat(61);
    for label in [
        json!("   "),
        json!(long),
        json!("!!!"),
        json!("Balanced"),
        json!("Low tokens"),
        json!("best quality"),
        json!(5),
        Value::Null,
    ] {
        assert_eq!(
            code(&host, "profile_save", json!({"label": label})),
            "badName",
            "{label}"
        );
    }
    assert_eq!(code(&host, "profile_save", json!({})), "badName");
    // Nothing the user changed (what the host set is not theirs).
    assert_eq!(
        code(&host, "profile_save", json!({"label": "Mine"})),
        "saveFailed"
    );
    assert!(!dir.join("profiles").exists());

    ask(&host, "set", json!({"key": "mouse_speed", "value": 1.5})).unwrap();
    ask(&host, "set", json!({"key": "overlay.scale", "value": 1.2})).unwrap();
    ask(&host, "cursor_set", json!({"data": GIF_1X1})).unwrap();
    let saved = ask(&host, "profile_save", json!({"label": "  My look "})).unwrap();
    assert_eq!(saved, json!({"id": "my-look"}));
    let list = ask(&host, "profiles", json!({})).unwrap();
    // Not what the host set, and not the kept picture's place.
    assert_eq!(
        profile_keys(&list, "my-look"),
        ["mouse_speed", "overlay.cursor_style", "overlay.scale"]
    );
    let mine = list["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "my-look")
        .unwrap();
    assert_eq!(
        (mine["builtin"].clone(), mine["label"].clone()),
        (json!(false), json!("My look"))
    );
    names_no_folder(&list, &dir);
    // Kept again under the same name: replaced.
    assert_eq!(
        ask(&host, "profile_save", json!({"label": "My look"})).unwrap()["id"],
        "my-look"
    );
}

#[test]
fn only_a_profile_the_user_kept_can_be_deleted() {
    let (host, _dir) = host_with("prof-delete", &["mouse_speed"], "");
    ask(&host, "set", json!({"key": "mouse_speed", "value": 1.5})).unwrap();
    ask(&host, "profile_save", json!({"label": "Mine"})).unwrap();
    for id in ["balanced", "showcase", "nope", "../user", "", "MINE"] {
        assert_eq!(
            code(&host, "profile_delete", json!({"id": id})),
            "notAllowed",
            "{id}"
        );
    }
    assert_eq!(code(&host, "profile_delete", json!({})), "badParams");
    assert_eq!(
        ask(&host, "profile_delete", json!({"id": "mine"})).unwrap(),
        json!({})
    );
    let list = ask(&host, "profiles", json!({})).unwrap();
    assert!(
        list["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["id"] != "mine")
    );
    assert_eq!(
        code(&host, "profile_delete", json!({"id": "mine"})),
        "notAllowed"
    );
    // The ones that come with the program are still all there.
    assert_eq!(list["profiles"].as_array().unwrap().len(), 4);
}

#[test]
fn a_host_never_gets_more_profiles_than_it_reads() {
    let (host, _dir) = host_with("prof-many", &["mouse_speed"], "");
    ask(&host, "set", json!({"key": "mouse_speed", "value": 1.5})).unwrap();
    for n in 0..super::profiles::MAX_KEPT {
        ask(&host, "profile_save", json!({"label": format!("p{n}")})).unwrap();
    }
    assert_eq!(
        code(&host, "profile_save", json!({"label": "one more"})),
        "saveFailed"
    );
    let list = ask(&host, "profiles", json!({})).unwrap();
    assert!(list["profiles"].as_array().unwrap().len() <= 50);
    // One that is kept already is replaced, at the limit as well.
    ask(&host, "profile_save", json!({"label": "p1"})).unwrap();
    ask(&host, "profile_delete", json!({"id": "p0"})).unwrap();
    ask(&host, "profile_save", json!({"label": "one more"})).unwrap();
}
