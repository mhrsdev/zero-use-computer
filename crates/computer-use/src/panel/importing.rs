//! Importing settings: what an import would do, shown before it does it
//! (each setting as it is now and as it would be, and the ones this
//! version can't take marked as such), and the copy of the settings kept
//! before an import, to go back to.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::schema::{self, Kind};
use super::settings::{self, as_toml, coerce, fail, lookup, shown};
use crate::config::{self, Config};

/// How one imported setting lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// A new value for a setting this version has.
    Change,
    /// The value it has already.
    Same,
    /// A new value for a protected setting: the user is asked first.
    Protected,
    /// Not a setting of this version (made by a newer one, or a typo).
    Unsupported,
    /// A setting of this version, with a value it can't take.
    Invalid,
}

impl Status {
    fn name(self) -> &'static str {
        match self {
            Status::Change => "change",
            Status::Same => "same",
            Status::Protected => "protected",
            Status::Unsupported => "unsupported",
            Status::Invalid => "invalid",
        }
    }
}

pub struct Row {
    pub key: String,
    pub status: Status,
    /// The value it has now, as the page shows it.
    pub before: Value,
    /// The value in the import.
    pub after: Value,
    pub note: Option<String>,
    pub value: Value,
}

pub struct Plan {
    pub rows: Vec<Row>,
    /// The version of the program that made the export, when it says.
    pub exported_by: Option<String>,
}

/// The line an export starts with.
pub const HEADER: &str = "# Zero Use Computer ";

/// The version a settings export says it was made by.
pub fn exported_by(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .take_while(|l| l.is_empty() || l.starts_with('#'))
        .find_map(|l| l.strip_prefix(HEADER.trim_end()))
        .and_then(|rest| rest.split_whitespace().next())
        .filter(|v| crate::update::Version::parse(v).is_some())
        .map(str::to_string)
}

/// Whether the settings file's values are the same: a whole number is the
/// same as itself with a fraction of nothing.
fn same_value(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) if a.is_number() && b.is_number() => x == y,
        _ => a == b,
    }
}

/// What importing `text` would do to the settings now in `cfg`.
pub fn plan(cfg: &Config, text: &str) -> Result<Plan, String> {
    if text.len() > 64 * 1024 {
        return Err("that is too long to be a list of settings".into());
    }
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let mut flat = Vec::new();
    super::flatten("", &table, &mut flat);
    if flat.is_empty() {
        return Err("there are no settings in it".into());
    }
    let exporter = exported_by(text);
    let newer = exporter.as_deref().and_then(crate::update::Version::parse)
        > Some(crate::update::Version::current());
    let now = as_toml(cfg);
    let mut rows = Vec::new();
    for (key, value) in flat {
        let mut row = Row {
            status: Status::Change,
            before: Value::Null,
            after: value.clone(),
            note: None,
            value: value.clone(),
            key: key.clone(),
        };
        match schema::entry(&key) {
            None => {
                row.status = Status::Unsupported;
                row.note = Some(match (&exporter, newer) {
                    (Some(v), true) => format!(
                        "This version ({}) has no such setting: the export is from {v}, a newer one",
                        crate::update::Version::current()
                    ),
                    _ => format!(
                        "This version ({}) has no such setting",
                        crate::update::Version::current()
                    ),
                });
            }
            Some(e) if e.kind == Kind::Custom => {
                row.status = Status::Invalid;
                row.note = Some("It is changed on its own page, not by an import".into());
            }
            Some(e) => {
                row.before = shown(e, lookup(&now, e.key));
                match coerce(e, &value) {
                    Err(why) => {
                        row.status = Status::Invalid;
                        row.note = Some(why);
                    }
                    Ok(None) => row.status = Status::Same,
                    Ok(Some(_)) => {
                        if e.kind == Kind::Secret {
                            // A secret is never shown, only that it is replaced.
                            row.after = json!("••••");
                        }
                        row.status = if e.kind != Kind::Secret && same_value(&row.before, &value) {
                            Status::Same
                        } else if e.confirm {
                            Status::Protected
                        } else {
                            Status::Change
                        };
                    }
                }
            }
        }
        rows.push(row);
    }
    Ok(Plan {
        rows,
        exported_by: exporter,
    })
}

impl Plan {
    pub fn json(&self) -> Value {
        let count = |s: Status| self.rows.iter().filter(|r| r.status == s).count();
        json!({
            "ok": true,
            "this_version": env!("CARGO_PKG_VERSION"),
            "exported_by": self.exported_by,
            "rows": self.rows.iter().map(|r| json!({
                "key": r.key,
                "group": schema::entry(&r.key).map(|e| e.group),
                "status": r.status.name(),
                "before": r.before,
                "after": r.after,
                "note": r.note,
            })).collect::<Vec<_>>(),
            "counts": {
                "change": count(Status::Change),
                "protected": count(Status::Protected),
                "same": count(Status::Same),
                "unsupported": count(Status::Unsupported),
                "invalid": count(Status::Invalid),
            },
        })
    }

    /// The changes to make, leaving out the settings already as they are
    /// and, when `skip_unsupported`, the ones this version can't take
    /// (`Err` with the first of them when it is not).
    pub fn changes(&self, skip_unsupported: bool) -> Result<Vec<(String, Value)>, String> {
        let mut out = Vec::new();
        for r in &self.rows {
            match r.status {
                Status::Change | Status::Protected => out.push((r.key.clone(), r.value.clone())),
                Status::Same => {}
                Status::Unsupported | Status::Invalid if skip_unsupported => {}
                Status::Unsupported => return Err(format!("`{}` is not a setting", r.key)),
                Status::Invalid => {
                    return Err(format!("{}: {}", r.key, r.note.as_deref().unwrap_or("")));
                }
            }
        }
        Ok(out)
    }

    /// The settings that were left out, with why.
    pub fn skipped(&self) -> Vec<Value> {
        self.rows
            .iter()
            .filter(|r| matches!(r.status, Status::Unsupported | Status::Invalid))
            .map(|r| json!({"key": r.key, "note": r.note}))
            .collect()
    }
}

/// Where the settings from before the last import are kept.
pub fn backup_path(home: &Path) -> PathBuf {
    home.join("settings-before-import.toml")
}

fn sibling(config: &Path, suffix: &str) -> PathBuf {
    let name = config.file_name().map_or_else(
        || "config.toml".into(),
        |n| n.to_string_lossy().into_owned(),
    );
    config.with_file_name(format!("{name}.{suffix}"))
}

/// Copy `from` to `to` for the owner alone, into a new file moved over
/// `to` at the end (a file never half written).
fn copy_private(from: &Path, to: &Path) -> std::io::Result<()> {
    let staged = sibling(to, "tmp");
    let _ = std::fs::remove_file(&staged);
    std::fs::copy(from, &staged)?;
    config::owner_only(&staged);
    std::fs::rename(&staged, to).inspect_err(|_| {
        let _ = std::fs::remove_file(&staged);
    })
}

/// Keep the settings file as it is (an empty file when there is none yet)
/// as the ones to go back to; it takes the place of an older copy only when
/// `commit` is called.
pub struct Backup {
    staged: PathBuf,
    kept: PathBuf,
}

impl Backup {
    pub fn take(config: &Path, home: &Path) -> Option<Backup> {
        let kept = backup_path(home);
        let staged = sibling(&kept, "new");
        let _ = std::fs::create_dir_all(home);
        let done = if config.is_file() {
            std::fs::copy(config, &staged).map(|_| ())
        } else {
            std::fs::write(&staged, "")
        };
        match done {
            Ok(()) => {
                config::owner_only(&staged);
                Some(Backup { staged, kept })
            }
            Err(e) => {
                log::warn!("settings: couldn't keep a copy before the import: {e}");
                let _ = std::fs::remove_file(&staged);
                None
            }
        }
    }

    pub fn commit(self) {
        if std::fs::rename(&self.staged, &self.kept).is_err() {
            let _ = std::fs::remove_file(&self.staged);
        }
    }

    pub fn discard(self) {
        let _ = std::fs::remove_file(&self.staged);
    }
}

/// How long ago the settings from before the last import were kept.
pub fn backup_age_secs(home: &Path) -> Option<u64> {
    let modified = std::fs::metadata(backup_path(home)).ok()?.modified().ok()?;
    Some(
        std::time::SystemTime::now()
            .duration_since(modified)
            .map_or(0, |d| d.as_secs()),
    )
}

/// Put the settings from before the last import back. What they replace
/// is kept in their place, so going back can itself be undone.
pub fn undo(config: &Path, home: &Path, confirmed: bool) -> Value {
    let kept = backup_path(home);
    if !kept.is_file() {
        return fail("no import has been made yet, so there is nothing to go back to");
    }
    if !confirmed {
        return json!({
            "ok": false,
            "error": "this needs the user's confirmation first",
            "confirm_text": "Put the settings back as they were before the last import? The ones you have now are kept, so this can be undone too.",
        });
    }
    let Some(now) = Backup::take(config, home) else {
        return fail("couldn't keep the settings you have now, so nothing was changed");
    };
    match copy_private(&kept, config) {
        Ok(()) => {
            now.commit();
            let cfg = config::ConfigStore::load(Some(config)).map(|s| s.config);
            match cfg {
                Ok(c) => json!({
                    "ok": true,
                    "message": "The settings from before the import are back.",
                    "values": settings::changed_values(&c),
                }),
                Err(e) => fail(&e.to_string()),
            }
        }
        Err(e) => {
            now.discard();
            fail(&format!("{}: {e}", config.display()))
        }
    }
}
