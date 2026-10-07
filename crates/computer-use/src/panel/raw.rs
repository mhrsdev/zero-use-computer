//! The settings file as text: shown with its secrets covered, checked as a
//! whole before it is written, and changed only where the user typed.

use std::path::Path;

use serde_json::{Value, json};

use super::schema::ENTRIES;
use super::settings::{as_toml, fail, lookup, needs_confirmation};
use crate::config::{self, Config};

/// What stands where a secret is. Left as it is, the saved one stays.
pub const KEPT: &str = "<kept: type a new value to replace it>";
const SECRETS: [&str; 2] = ["decision.api_key", "server.http_token"];

fn walk<'a>(doc: &'a toml_edit::DocumentMut, key: &str) -> Option<&'a toml_edit::Item> {
    let mut item = doc.as_item();
    for part in key.split('.') {
        item = item.as_table_like()?.get(part)?;
    }
    Some(item)
}

fn set_text(doc: &mut toml_edit::DocumentMut, key: &str, value: Option<&str>) {
    let parts: Vec<&str> = key.split('.').collect();
    let (last, tables) = parts.split_last().expect("a key");
    let mut table: &mut dyn toml_edit::TableLike = doc.as_table_mut();
    for t in tables {
        if value.is_none() && table.get(t).is_none() {
            return; // nothing to remove
        }
        let entry = table
            .entry(t)
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
        let Some(next) = entry.as_table_like_mut() else {
            return;
        };
        table = next;
    }
    match value {
        Some(v) => {
            table.insert(last, toml_edit::value(v));
        }
        None => {
            table.remove(last);
        }
    }
}

fn saved_text(path: &Path) -> Result<String, String> {
    match config::read_text(path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// The file's text with its secrets covered.
pub fn view(path: &Path) -> Value {
    let text = match saved_text(path) {
        Ok(t) => t,
        Err(e) => return fail(&e),
    };
    let Ok(mut doc) = text.parse::<toml_edit::DocumentMut>() else {
        // Shown as it is so the user can mend it; it holds no secret we can
        // find by name, but cover any line that names one.
        let covered: Vec<String> = text
            .lines()
            .map(|l| {
                let name = l.split('=').next().unwrap_or_default().trim();
                if SECRETS
                    .iter()
                    .any(|k| k.ends_with(name) && !name.is_empty())
                {
                    format!("{name} = \"{KEPT}\"")
                } else {
                    l.to_string()
                }
            })
            .collect();
        return json!({"ok": true, "text": covered.join("\n"), "path": path.display().to_string(), "broken": true});
    };
    for k in SECRETS {
        if walk(&doc, k)
            .and_then(|i| i.as_str())
            .is_some_and(|s| !s.is_empty())
        {
            set_text(&mut doc, k, Some(KEPT));
        }
    }
    json!({"ok": true, "text": doc.to_string(), "path": path.display().to_string()})
}

pub struct Checked {
    /// The text to write (the saved secrets put back).
    pub text: String,
    /// The settings that differ from what is saved now.
    pub changed: Vec<&'static str>,
    /// Of those, the ones that ask the user first.
    pub protected: Vec<&'static str>,
}

/// Whether `text` would be a good settings file, and what it would change.
pub fn check(path: &Path, text: &str) -> Result<Checked, String> {
    if text.len() > 256 * 1024 {
        return Err("the file is too long to be a settings file".into());
    }
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| e.to_string())?;
    let old_text = saved_text(path)?;
    let old_doc: toml_edit::DocumentMut = old_text.parse().unwrap_or_default();
    for k in SECRETS {
        let typed = walk(&doc, k).and_then(|i| i.as_str()).map(str::to_string);
        if typed.as_deref() == Some(KEPT) {
            let saved = walk(&old_doc, k)
                .and_then(|i| i.as_str())
                .map(str::to_string);
            set_text(&mut doc, k, saved.as_deref());
        }
    }
    let text = doc.to_string();
    let typos = config::unknown_keys(&text);
    if !typos.is_empty() {
        return Err(format!("not a setting: {}", typos.join(", ")));
    }
    let new: Config = toml::from_str(&text).map_err(|e| e.message().to_string())?;
    new.validate()?;
    let old: Config = toml::from_str(&old_text).unwrap_or_default();
    let (a, b) = (as_toml(&old), as_toml(&new));
    let mut changed = Vec::new();
    let mut protected = Vec::new();
    for e in ENTRIES {
        if lookup(&a, e.key) != lookup(&b, e.key) {
            changed.push(e.key);
            if e.confirm {
                protected.push(e.key);
            }
        }
    }
    Ok(Checked {
        text,
        changed,
        protected,
    })
}

/// Check the text, and with `save`, write it.
pub fn run(path: &Path, text: &str, confirmed: bool, save: bool) -> Value {
    let c = match check(path, text) {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    if !save {
        return json!({"ok": true, "changed": c.changed, "protected": c.protected});
    }
    if !c.protected.is_empty() && !confirmed {
        return needs_confirmation(&c.protected);
    }
    let private = text.contains(KEPT)
        || c.text.parse::<toml_edit::DocumentMut>().is_ok_and(|d| {
            SECRETS.iter().any(|k| {
                walk(&d, k)
                    .and_then(|i| i.as_str())
                    .is_some_and(|s| !s.trim().is_empty())
            })
        });
    if let Some(dir) = path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        return fail(&format!("{}: {e}", dir.display()));
    }
    match config::write_atomic(path, &c.text, private) {
        Ok(()) => json!({"ok": true, "changed": c.changed}),
        Err(e) => fail(&e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("cu-raw-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("config.toml")
    }

    #[test]
    fn secrets_are_covered_and_come_back_when_left_alone() {
        let p = temp("secret");
        std::fs::write(
            &p,
            "# mine\n[decision]\napi_key = \"sk-real-key\"\nprovider = \"jev\"\n\n[server]\nhttp_token = \"tok-123456789\"\n",
        )
        .unwrap();
        let v = view(&p);
        let shown = v["text"].as_str().unwrap();
        assert!(
            !shown.contains("sk-real-key") && !shown.contains("tok-123456789"),
            "{shown}"
        );
        assert!(shown.contains("# mine") && shown.contains(KEPT));
        // Saved as it was shown, with one real change.
        let edited = shown.replace("jev", "openai");
        let r = run(&p, &edited, false, true);
        assert_eq!(r["ok"], true, "{r}");
        let saved = std::fs::read_to_string(&p).unwrap();
        assert!(
            saved.contains("sk-real-key") && saved.contains("tok-123456789"),
            "{saved}"
        );
        assert!(saved.contains("openai") && saved.contains("# mine"));
    }

    #[test]
    fn a_bad_file_is_never_written() {
        let p = temp("bad");
        std::fs::write(&p, "clipboard = true\n").unwrap();
        for bad in [
            "clipboard = = true",
            "cliboard = true\n",
            "[screenshot]\nattach = \"sometimes\"\n",
            "[overlay]\ncursor_style = \"nope\"\n",
        ] {
            let r = run(&p, bad, true, true);
            assert_eq!(r["ok"], false, "{bad}: {r}");
        }
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "clipboard = true\n");
    }

    #[test]
    fn protected_changes_are_named_and_need_a_yes() {
        let p = temp("protected");
        std::fs::write(&p, "").unwrap();
        let text = "[control]\npause_on_user_input = false\n\n[tree]\nmax_nodes = 5\n";
        let r = run(&p, text, false, false);
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["protected"], json!(["control.pause_on_user_input"]));
        let r = run(&p, text, false, true);
        assert_eq!(r["ok"], false);
        assert_eq!(r["confirm"], json!(["control.pause_on_user_input"]));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "");
        assert_eq!(run(&p, text, true, true)["ok"], true);
        assert!(
            std::fs::read_to_string(&p)
                .unwrap()
                .contains("max_nodes = 5")
        );
    }

    #[test]
    fn a_missing_file_is_an_empty_one() {
        let p = temp("missing");
        assert_eq!(view(&p)["text"], "");
        assert_eq!(run(&p, "text_only = true\n", false, true)["ok"], true);
    }
}
