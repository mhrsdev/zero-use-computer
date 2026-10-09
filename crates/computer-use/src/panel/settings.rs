//! Settings as the panel handles them: the page's description of each one
//! (sent once), the values (sent small: only what differs from the
//! defaults), and changes checked against each setting's type before they
//! are written.

use std::path::Path;
use std::sync::OnceLock;

use serde_json::{Value, json};

use super::profiles;
use super::schema::{self, ENTRIES, Entry, GROUPS, Kind};
use crate::config::{self, Config, Edit};

pub fn fail(error: &str) -> Value {
    json!({"ok": false, "error": error})
}

/// A change that needs the user's word first (the page asks, then repeats
/// it with `confirmed`).
pub fn needs_confirmation(keys: &[&str]) -> Value {
    json!({
        "ok": false,
        "error": "these settings need the user's confirmation first",
        "confirm": keys,
    })
}

pub fn lookup<'a>(root: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let mut v = root;
    for part in key.split('.') {
        v = v.get(part)?;
    }
    Some(v)
}

/// The settings as TOML values, to look keys up in.
pub fn as_toml(cfg: &Config) -> toml::Value {
    toml::Value::try_from(cfg).unwrap_or(toml::Value::Boolean(false))
}

/// The defaults, built once.
pub fn defaults() -> &'static toml::Value {
    static D: OnceLock<toml::Value> = OnceLock::new();
    D.get_or_init(|| as_toml(&Config::default()))
}

/// A setting as the page shows it: a secret only by its last characters,
/// and the decision model's (shown by its own card) not at all.
pub fn shown(e: &Entry, v: Option<&toml::Value>) -> Value {
    match (e.kind, v) {
        (Kind::Custom, _) => json!(""),
        (Kind::Secret, Some(toml::Value::String(s))) => json!(config::masked_key(s)),
        (Kind::Secret, _) => json!(""),
        (_, Some(v)) => serde_json::to_value(v).unwrap_or(Value::Null),
        (_, None) => json!(""),
    }
}

fn type_name(e: &Entry, def: Option<&toml::Value>) -> &'static str {
    match e.kind {
        Kind::Choice(_) => "choice",
        Kind::Range { .. } => "range",
        Kind::Color => "color",
        Kind::Hotkey => "hotkey",
        Kind::Secret => "secret",
        Kind::Custom => "custom",
        Kind::Number { .. } | Kind::Auto => match def {
            Some(toml::Value::Boolean(_)) => "bool",
            Some(toml::Value::Integer(_)) => "int",
            Some(toml::Value::Float(_)) => "float",
            Some(toml::Value::Array(_)) => "list",
            _ => "string",
        },
    }
}

fn entry_json(e: &Entry) -> Value {
    let def = lookup(defaults(), e.key);
    let mut j = json!({
        "key": e.key,
        "group": e.group,
        "help": e.help,
        "type": type_name(e, def),
        "default": shown(e, def),
        "restart": e.restart,
        "confirm": e.confirm,
        "advanced": e.advanced,
    });
    match e.kind {
        Kind::Choice(c) => j["choices"] = json!(c),
        Kind::Range {
            min,
            max,
            step,
            unit,
        } => {
            j["min"] = json!(min);
            j["max"] = json!(max);
            j["step"] = json!(step);
            j["unit"] = json!(unit);
        }
        Kind::Number { unit } => j["unit"] = json!(unit),
        _ => {}
    }
    j
}

/// What the page needs to know about every setting. It never changes while
/// the program runs, so it is made once and sent as it is.
pub fn schema_json() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    S.get_or_init(|| {
        let entries: Vec<Value> = ENTRIES.iter().map(entry_json).collect();
        json!({
            "ok": true,
            "version": env!("CARGO_PKG_VERSION"),
            "groups": GROUPS,
            "blurbs": schema::blurbs(),
            "entries": entries,
            "builtin_profiles": profiles::builtin_list(),
            "help": super::help::list(),
        })
        .to_string()
    })
}

/// The values that differ from the defaults (the page takes the rest from
/// the schema).
pub fn changed_values(cfg: &Config) -> Value {
    let now = as_toml(cfg);
    let mut out = serde_json::Map::new();
    for e in ENTRIES {
        let (v, d) = (
            shown(e, lookup(&now, e.key)),
            shown(e, lookup(defaults(), e.key)),
        );
        if v != d {
            out.insert(e.key.into(), v);
        }
    }
    Value::Object(out)
}

/// The values of `keys`, for the page to show after a change.
pub fn values_of(cfg: &Config, keys: &[&str]) -> Value {
    let now = as_toml(cfg);
    let mut out = serde_json::Map::new();
    for k in keys {
        if let Some(e) = schema::entry(k) {
            out.insert((*k).into(), shown(e, lookup(&now, k)));
        }
    }
    Value::Object(out)
}

/// A value from the page as an edit of the settings file, checked against
/// the setting's type: `None` for nothing to change (an empty secret).
/// `null` puts the setting back to its default.
pub fn coerce(e: &Entry, v: &Value) -> std::result::Result<Option<Edit>, String> {
    let def = lookup(defaults(), e.key);
    if v.is_null() {
        return match e.kind {
            Kind::Secret => Err("a saved secret is replaced, not reset, here".into()),
            _ => Ok(Some(Edit::Unset)),
        };
    }
    let line = |s: &str| -> std::result::Result<String, String> {
        if s.chars().any(char::is_control) {
            return Err("must be one line of text".into());
        }
        Ok(s.to_string())
    };
    if let Kind::Secret = e.kind {
        let s = v.as_str().ok_or("must be text")?;
        if s.is_empty() {
            return Ok(None);
        }
        return Ok(Some(Edit::SetText(line(s)?)));
    }
    if let Kind::Choice(c) = e.kind {
        let s = v.as_str().ok_or("must be text")?;
        if !c.contains(&s) {
            return Err(format!("must be one of {}", c.join(", ")));
        }
        return Ok(Some(Edit::SetText(s.to_string())));
    }
    match def {
        Some(toml::Value::Boolean(_)) => v
            .as_bool()
            .map(|b| Some(Edit::Set(b.to_string())))
            .ok_or_else(|| "must be on or off".into()),
        Some(toml::Value::Integer(_)) => {
            let n = number(v)?;
            if n.fract() != 0.0 || !(0.0..=9.0e15).contains(&n) {
                return Err("must be a whole number, not negative".into());
            }
            range_check(e, n)?;
            Ok(Some(Edit::Set(format!("{}", n as i64))))
        }
        Some(toml::Value::Float(_)) => {
            let n = number(v)?;
            range_check(e, n)?;
            Ok(Some(Edit::Set(format!("{n:?}"))))
        }
        Some(toml::Value::Array(_)) => {
            let items = v.as_array().ok_or("must be a list")?;
            let mut out = Vec::new();
            for i in items {
                let s = i.as_str().ok_or("a list holds text")?;
                out.push(toml::Value::String(line(s.trim())?).to_string());
            }
            Ok(Some(Edit::Set(format!("[{}]", out.join(", ")))))
        }
        // Text, and settings that are unset by default (a path).
        _ => {
            let s = v.as_str().ok_or("must be text")?;
            let s = line(s.trim())?;
            if s.is_empty() && def.is_none() {
                return Ok(Some(Edit::Unset));
            }
            Ok(Some(Edit::SetText(s)))
        }
    }
}

fn number(v: &Value) -> std::result::Result<f64, String> {
    v.as_f64()
        .filter(|n| n.is_finite())
        .ok_or_else(|| "must be a number".into())
}

fn range_check(e: &Entry, n: f64) -> std::result::Result<(), String> {
    if let Kind::Range { min, max, .. } = e.kind
        && !(min..=max).contains(&n)
    {
        return Err(format!("must be between {min} and {max}"));
    }
    Ok(())
}

/// Several changes, all or none: `changes` are (key, value) with `null`
/// for "back to the default". Writes the settings file at `path`.
pub fn apply(path: &Path, changes: &[(String, Value)], confirmed: bool) -> Value {
    let mut edits: Vec<(&'static str, Edit)> = Vec::new();
    let mut protected: Vec<&'static str> = Vec::new();
    for (key, value) in changes {
        let Some(entry) = schema::entry(key) else {
            return fail(&format!("`{key}` is not a setting"));
        };
        if entry.kind == Kind::Custom {
            return fail(&format!("`{key}` is changed on its own page"));
        }
        match coerce(entry, value) {
            Ok(Some(edit)) => {
                if entry.confirm {
                    protected.push(entry.key);
                }
                edits.push((entry.key, edit));
            }
            Ok(None) => {}
            Err(e) => return fail(&format!("{key}: {e}")),
        }
    }
    if !protected.is_empty() && !confirmed {
        return needs_confirmation(&protected);
    }
    let keys: Vec<&str> = edits.iter().map(|(k, _)| *k).collect();
    if !edits.is_empty()
        && let Err(e) = config::edit_file_many(path, &edits)
    {
        return fail(&e.to_string());
    }
    let cfg = match config::ConfigStore::load(Some(path)) {
        Ok(s) => s.config,
        Err(e) => return fail(&e.to_string()),
    };
    let mut reply = json!({"ok": true, "values": values_of(&cfg, &keys)});
    if keys.iter().any(|k| k.starts_with("panel.")) {
        reply["theme"] = json!(cfg.panel.theme);
        reply["accent"] = json!(cfg.panel.accent);
        reply["language"] = json!(cfg.panel.language);
    }
    reply
}
