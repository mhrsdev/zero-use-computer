//! Profiles: a named set of settings applied together. Four come with the
//! program; the user can keep their own (a snapshot of what they changed),
//! as small files in the server's folder. A profile never holds a secret,
//! never a setting that limits the agent or the updates, and never a place
//! on this computer (a file, a folder or a program).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::schema::{self, Kind};
use super::settings::{as_toml, lookup, shown};
use crate::config::Config;

struct Builtin {
    id: &'static str,
    label: &'static str,
    blurb: &'static str,
    /// (setting, value as JSON).
    set: &'static [(&'static str, &'static str)],
}

const BUILTIN: &[Builtin] = &[
    Builtin {
        id: "low-tokens",
        label: "Low tokens",
        blurb: "Smaller pictures and shorter trees. A full screenshot costs about a third less; very small text may be harder to read.",
        set: &[
            ("screenshot.max_dimension", "1024"),
            ("screenshot.overview_max_dimension", "768"),
            ("tree.max_nodes", "600"),
            ("tree.max_tokens", "6000"),
            ("tree.max_text_len", "100"),
        ],
    },
    Builtin {
        id: "best-quality",
        label: "Best quality",
        blurb: "Big pictures, whole windows, longer trees and no shortening. The most tokens; for hard-to-read apps.",
        set: &[
            ("screenshot.max_dimension", "2048"),
            ("screenshot.scope", "\"full\""),
            ("screenshot.smart", "false"),
            ("screenshot.adaptive", "false"),
            ("tree.max_nodes", "2400"),
            ("tree.max_tokens", "0"),
            ("tree.summarize", "\"off\""),
            ("tree.max_text_len", "400"),
        ],
    },
    Builtin {
        id: "showcase",
        label: "Showcase",
        blurb: "For recording a demo: a bigger pointer and border, longer glides, keys and text shown, and the finished state kept on screen a little longer.",
        set: &[
            ("overlay.scale", "1.3"),
            ("overlay.glow_size", "56"),
            ("overlay.border_width", "4"),
            ("overlay.move_ms", "420"),
            ("overlay.done_linger_ms", "3000"),
            ("overlay.cursor_motion", "true"),
            ("overlay.show_keys", "true"),
            ("overlay.click_effect", "true"),
            ("natural_mouse", "true"),
        ],
    },
];

/// The most profiles of the user's own: with the ones that come with the program, a host's list
/// stays within the 50 it reads.
pub const MAX_KEPT: usize = 40;

/// Settings that name a place on this computer. A profile is a snapshot that can be carried to
/// another computer or applied long after the file went; it must not point the program at a file
/// or at a program.
const PLACES: &[&str] = &[
    "overlay.cursor_image",
    "overlay.sound_click_file",
    "overlay.sound_key_file",
    "overlay.sound_done_file",
    "overlay.sound_error_file",
    "overlay.sound_stopped_file",
    "overlay.font",
    "ocr.tesseract_path",
    "script.dir",
];

const BALANCED: (&str, &str, &str) = (
    "balanced",
    "Balanced",
    "The program's own defaults for every setting the other profiles touch.",
);

/// The settings the built-in profiles touch, once each.
fn touched() -> Vec<&'static str> {
    let mut keys: Vec<&str> = BUILTIN
        .iter()
        .flat_map(|b| b.set.iter().map(|s| s.0))
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// Whether `id` is a profile that comes with the program.
pub fn is_builtin(id: &str) -> bool {
    id == BALANCED.0 || BUILTIN.iter().any(|b| b.id == id)
}

pub fn builtin_list() -> Value {
    let mut list =
        vec![json!({"id": BALANCED.0, "label": BALANCED.1, "blurb": BALANCED.2, "builtin": true})];
    for b in BUILTIN {
        list.push(json!({"id": b.id, "label": b.label, "blurb": b.blurb, "builtin": true}));
    }
    Value::Array(list)
}

/// What applying a profile changes: (setting, value; null = default).
fn target(home: &Path, id: &str) -> Option<Vec<(String, Value)>> {
    if id == BALANCED.0 {
        return Some(
            touched()
                .into_iter()
                .map(|k| (k.to_string(), Value::Null))
                .collect(),
        );
    }
    if let Some(b) = BUILTIN.iter().find(|b| b.id == id) {
        // Whatever another profile set and this one doesn't goes back to
        // its default, so one profile never leaves another's settings.
        let mut out: Vec<(String, Value)> = touched()
            .into_iter()
            .map(|k| {
                let v = b
                    .set
                    .iter()
                    .find(|s| s.0 == k)
                    .and_then(|s| serde_json::from_str(s.1).ok())
                    .unwrap_or(Value::Null);
                (k.to_string(), v)
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        return Some(out);
    }
    let file = read_custom(&custom_path(home, id)?)?;
    // Only what a profile may hold, whatever the file says (it is a file
    // anyone's program could write).
    Some(
        file.values
            .into_iter()
            .filter(|(k, _)| may_hold(k))
            .collect(),
    )
}

pub fn changes_for(home: &Path, id: &str) -> Option<Vec<(String, Value)>> {
    target(home, id)
}

// ---- the user's own -------------------------------------------------------------

fn dir(home: &Path) -> PathBuf {
    home.join("profiles")
}

/// A profile's id from its name: lower case letters, digits and dashes.
pub fn slug(label: &str) -> String {
    let mut s = String::new();
    for c in label.trim().chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    s.trim_end_matches('-').chars().take(40).collect()
}

fn custom_path(home: &Path, id: &str) -> Option<PathBuf> {
    let ok = !id.is_empty()
        && id.len() <= 40
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    ok.then(|| dir(home).join(format!("{id}.json")))
}

struct Custom {
    label: String,
    values: serde_json::Map<String, Value>,
}

fn read_custom(path: &Path) -> Option<Custom> {
    // Small files only: this is read from a folder anyone's program may
    // write to.
    if std::fs::metadata(path).ok()?.len() > 64 * 1024 {
        return None;
    }
    let v: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let label = v.get("label")?.as_str()?.chars().take(60).collect();
    let values = v.get("values")?.as_object()?.clone();
    Some(Custom { label, values })
}

/// All profiles, with how many settings each would change now.
pub fn list(home: &Path, cfg: &Config) -> Value {
    let now = as_toml(cfg);
    let differs = |changes: &[(String, Value)]| {
        changes
            .iter()
            .filter(|(k, v)| {
                let Some(e) = schema::entry(k) else {
                    return false;
                };
                let here = shown(e, lookup(&now, k));
                let want = if v.is_null() {
                    shown(e, lookup(super::settings::defaults(), k))
                } else {
                    v.clone()
                };
                here != want
            })
            .count()
    };
    let mut out = Vec::new();
    for b in builtin_list().as_array().cloned().unwrap_or_default() {
        let id = b["id"].as_str().unwrap_or_default();
        let ch = target(home, id).unwrap_or_default();
        let mut b = b;
        b["settings"] = json!(ch.len());
        b["differs"] = json!(differs(&ch));
        out.push(b);
    }
    let mut mine = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir(home)) {
        for entry in rd.flatten().take(MAX_KEPT) {
            let p = entry.path();
            let (Some(stem), Some("json")) = (
                p.file_stem().and_then(|s| s.to_str()),
                p.extension().and_then(|s| s.to_str()),
            ) else {
                continue;
            };
            let Some(c) = read_custom(&p) else { continue };
            let ch: Vec<(String, Value)> = c.values.into_iter().collect();
            mine.push(json!({
                "id": stem, "label": c.label, "builtin": false,
                "blurb": "Your own: a snapshot of settings you changed.",
                "settings": ch.len(), "differs": differs(&ch),
            }));
        }
    }
    mine.sort_by(|a, b| a["label"].as_str().cmp(&b["label"].as_str()));
    out.extend(mine);
    json!({"ok": true, "profiles": out})
}

/// Whether a profile may set `key`: never a secret, a protected setting, a
/// port, a place on this computer or the panel's own look.
fn may_hold(key: &str) -> bool {
    schema::entry(key).is_some_and(|e| {
        !PLACES.contains(&e.key)
            && !e.confirm
            && !e.restart
            && !matches!(e.kind, Kind::Secret | Kind::Custom)
            && !e.key.starts_with("panel.")
    })
}

/// Keep the settings that differ from the defaults as a profile of the
/// user's own. What a profile may not hold (secrets, protected settings,
/// ports, places, the panel's own look) is left out; how many that was comes back.
pub fn save(home: &Path, cfg: &Config, label: &str) -> Value {
    save_of(home, cfg, label, |_| true)
}

/// [`save`] of the settings `wanted` names only (a host keeps the ones the user may change).
pub fn save_of(home: &Path, cfg: &Config, label: &str, wanted: impl Fn(&str) -> bool) -> Value {
    let label: String = label
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(60)
        .collect();
    let id = slug(&label);
    if id.is_empty() {
        return super::settings::fail("give the profile a name");
    }
    if is_builtin(&id) {
        return super::settings::fail("that name belongs to a profile that comes with the program");
    }
    let now = as_toml(cfg);
    let mut values = serde_json::Map::new();
    let mut left_out = 0usize;
    for e in schema::ENTRIES {
        let (v, d) = (
            shown(e, lookup(&now, e.key)),
            shown(e, lookup(super::settings::defaults(), e.key)),
        );
        if v == d {
            continue;
        }
        if may_hold(e.key) && wanted(e.key) {
            values.insert(e.key.into(), v);
        } else {
            left_out += 1;
        }
    }
    if values.is_empty() {
        return super::settings::fail(
            "nothing differs from the defaults, so there is nothing to keep",
        );
    }
    let Some(path) = custom_path(home, &id) else {
        return super::settings::fail("that name can't be kept");
    };
    // (Keeping one again under its name only replaces it.)
    if !path.exists() && kept_count(home) >= MAX_KEPT {
        return super::settings::fail("there are too many profiles of your own; delete one first");
    }
    if std::fs::create_dir_all(dir(home)).is_err() {
        return super::settings::fail("can't make the profiles folder");
    }
    let n = values.len();
    let text = json!({"label": label, "values": values}).to_string();
    match std::fs::write(&path, text) {
        Ok(()) => json!({"ok": true, "id": id, "settings": n, "left_out": left_out}),
        Err(e) => super::settings::fail(&format!("{}: {e}", path.display())),
    }
}

fn kept_count(home: &Path) -> usize {
    std::fs::read_dir(dir(home)).map_or(0, |rd| {
        rd.flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .count()
    })
}

pub fn delete(home: &Path, id: &str) -> Value {
    let Some(path) = custom_path(home, id) else {
        return super::settings::fail("that is not one of your profiles");
    };
    match std::fs::remove_file(&path) {
        Ok(()) => json!({"ok": true}),
        Err(e) => super::settings::fail(&e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_profiles_touch_only_settings_that_are_safe_and_valid() {
        for b in BUILTIN {
            let mut cfg = toml::Table::try_from(Config::default()).unwrap();
            for (key, json) in b.set {
                let e = schema::entry(key).unwrap_or_else(|| panic!("{key}: not a setting"));
                assert!(
                    !e.confirm && !e.restart,
                    "{}: a profile may not hold {key}",
                    b.id
                );
                assert!(!matches!(e.kind, Kind::Secret | Kind::Custom), "{key}");
                let v: Value = serde_json::from_str(json).unwrap();
                super::super::settings::coerce(e, &v)
                    .unwrap_or_else(|err| panic!("{}.{key}: {err}", b.id));
                let parts: Vec<_> = key.split('.').collect();
                let mut cur = &mut cfg;
                for p in &parts[..parts.len() - 1] {
                    cur = cur.get_mut(*p).unwrap().as_table_mut().unwrap();
                }
                cur.insert(
                    parts[parts.len() - 1].into(),
                    toml::Value::try_from(&v).unwrap(),
                );
            }
            let c: Config = cfg.try_into().unwrap_or_else(|e| panic!("{}: {e}", b.id));
            c.validate().unwrap_or_else(|e| panic!("{}: {e}", b.id));
        }
    }

    #[test]
    fn a_setting_that_names_a_place_is_never_held_by_a_profile() {
        for e in schema::ENTRIES {
            let names_a_place = e.key.ends_with("_file")
                || e.key.ends_with("_image")
                || e.key.ends_with("_path") && !matches!(e.kind, Kind::Choice(_))
                || [".font", ".dir"].iter().any(|s| e.key.ends_with(s));
            assert!(!names_a_place || !may_hold(e.key), "{}", e.key);
        }
        for key in PLACES {
            assert!(
                schema::entry(key).is_some(),
                "{key}: not a setting any more"
            );
        }
    }

    #[test]
    fn names_become_safe_ids() {
        assert_eq!(slug("  My Setup #2! "), "my-setup-2");
        assert_eq!(slug("../../etc"), "etc");
        assert_eq!(slug("!!!"), "");
        assert!(custom_path(Path::new("/x"), "../a").is_none());
        assert!(custom_path(Path::new("/x"), "a/b").is_none());
        assert!(custom_path(Path::new("/x"), "ok-1").is_some());
    }
}
