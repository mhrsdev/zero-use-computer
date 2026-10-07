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

/// A fingerprint of the file as saved, so a save can tell whether someone
/// else changed it since it was shown.
fn fingerprint(text: &str) -> String {
    crate::update::sha256_hex(text.as_bytes())
}

/// The file's text with its secrets covered.
pub fn view(path: &Path) -> Value {
    let text = match saved_text(path) {
        Ok(t) => t,
        Err(e) => return fail(&e),
    };
    let hash = fingerprint(&text);
    let shown = path.display().to_string();
    let Ok(mut doc) = text.parse::<toml_edit::DocumentMut>() else {
        // A file with a mistake can't be read well enough to find its
        // secrets (an inline table, a string over several lines): one that
        // may hold one isn't shown at all.
        let low = text.to_ascii_lowercase();
        if low.contains("api_key") || low.contains("http_token") {
            return json!({
                "ok": false,
                "error": format!("The settings file has a mistake in it and may hold a secret, so it isn't shown here. Fix it in a text editor: {shown}"),
                "path": shown,
            });
        }
        return json!({"ok": true, "text": text, "path": shown, "broken": true, "hash": hash});
    };
    for k in SECRETS {
        if walk(&doc, k)
            .and_then(|i| i.as_str())
            .is_some_and(|s| !s.is_empty())
        {
            set_text(&mut doc, k, Some(KEPT));
        }
    }
    json!({"ok": true, "text": doc.to_string(), "path": shown, "hash": hash})
}

pub struct Checked {
    /// The text to write (the saved secrets put back).
    pub text: String,
    /// The settings that differ from what is saved now.
    pub changed: Vec<&'static str>,
    /// Of those, the ones that ask the user first.
    pub protected: Vec<&'static str>,
    /// The fingerprint of the file as it was read.
    pub was: String,
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
    let old_doc = old_text.parse::<toml_edit::DocumentMut>().ok();
    let mut kept_key = false;
    for k in SECRETS {
        let typed = walk(&doc, k).and_then(|i| i.as_str()).map(str::to_string);
        if typed.as_deref() == Some(KEPT) {
            // The saved file can't be read: there is nothing to put back,
            // and dropping the secret quietly would lose it.
            let Some(old_doc) = &old_doc else {
                return Err(format!(
                    "the saved file can't be read, so the kept `{k}` can't be put back: type it again, or fix the file in a text editor"
                ));
            };
            let saved = walk(old_doc, k)
                .and_then(|i| i.as_str())
                .map(str::to_string);
            set_text(&mut doc, k, saved.as_deref());
            kept_key |= k == "decision.api_key";
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
    // A key goes only to the address it was given for (as on the decision
    // model's page).
    if kept_key
        && (old.decision.provider.trim() != new.decision.provider.trim()
            || old.decision.base_url.trim() != new.decision.base_url.trim())
    {
        return Err("the decision model's kind or address changed: type its API key again (a saved key never goes to another address)".into());
    }
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
        was: fingerprint(&old_text),
    })
}

/// Check the text, and with `save`, write it. `seen` is the fingerprint of
/// the file as the user was shown it: a file changed since (another tab, a
/// command) isn't written over.
pub fn run(path: &Path, text: &str, confirmed: bool, save: bool, seen: Option<&str>) -> Value {
    // Read, checked and written as one, with every other edit kept out.
    let _one = config::EDIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let c = match check(path, text) {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    if !save {
        return json!({"ok": true, "changed": c.changed, "protected": c.protected});
    }
    if seen.is_some_and(|h| h != c.was) {
        return fail(
            "the settings file changed since it was shown here (another tab, a command, a profile): press Revert to see it as it is now, then make your change again",
        );
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
        Ok(()) => json!({"ok": true, "changed": c.changed, "hash": fingerprint(&c.text)}),
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
        let edited = shown.replace("provider = \"jev\"", "provider = \"jev\"\nmodel = \"m9\"");
        let r = run(&p, &edited, false, true, None);
        assert_eq!(r["ok"], true, "{r}");
        let saved = std::fs::read_to_string(&p).unwrap();
        assert!(
            saved.contains("sk-real-key") && saved.contains("tok-123456789"),
            "{saved}"
        );
        assert!(saved.contains("m9") && saved.contains("# mine"));
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
            let r = run(&p, bad, true, true, None);
            assert_eq!(r["ok"], false, "{bad}: {r}");
        }
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "clipboard = true\n");
    }

    #[test]
    fn protected_changes_are_named_and_need_a_yes() {
        let p = temp("protected");
        std::fs::write(&p, "").unwrap();
        let text = "[control]\npause_on_user_input = false\n\n[tree]\nmax_nodes = 5\n";
        let r = run(&p, text, false, false, None);
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["protected"], json!(["control.pause_on_user_input"]));
        let r = run(&p, text, false, true, None);
        assert_eq!(r["ok"], false);
        assert_eq!(r["confirm"], json!(["control.pause_on_user_input"]));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "");
        assert_eq!(run(&p, text, true, true, None)["ok"], true);
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
        assert_eq!(run(&p, "text_only = true\n", false, true, None)["ok"], true);
    }

    #[test]
    fn a_broken_file_never_shows_a_secret_nor_loses_one() {
        let p = temp("broken");
        let inline = "decision = { api_key = \"sk-LEAKED-123456\" }\ntree = = 5\n";
        std::fs::write(&p, inline).unwrap();
        let v = view(&p);
        assert_eq!(v["ok"], false);
        assert!(!v.to_string().contains("sk-LEAKED"), "{v}");
        // Without a secret in it, it is shown so it can be mended.
        std::fs::write(&p, "tree = = 5\n").unwrap();
        assert_eq!(view(&p)["broken"], true);
        // A placeholder the broken file can't fill: refused, file untouched.
        std::fs::write(
            &p,
            "[decision]\napi_key = \"sk-keep\"\n[tree]\nmax_nodes = = 5\n",
        )
        .unwrap();
        let fixed = format!("[decision]\napi_key = \"{KEPT}\"\n[tree]\nmax_nodes = 5\n");
        let r = run(&p, &fixed, true, true, None);
        assert_eq!(r["ok"], false, "{r}");
        assert!(std::fs::read_to_string(&p).unwrap().contains("sk-keep"));
    }

    #[test]
    fn a_kept_key_never_follows_a_new_address() {
        let p = temp("address");
        std::fs::write(&p, "[decision]\nprovider = \"openai\"\nbase_url = \"https://api.openai.com/v1\"\nmodel = \"m\"\napi_key = \"sk-mine-0001\"\n").unwrap();
        let shown = view(&p)["text"].as_str().unwrap().to_string();
        let moved = shown.replace("https://api.openai.com/v1", "https://evil.example/v1");
        let r = run(&p, &moved, true, true, None);
        assert_eq!(r["ok"], false, "{r}");
        assert!(
            r["error"]
                .as_str()
                .unwrap()
                .contains("type its API key again")
        );
        // Typed again: fine.
        let typed = moved.replace(KEPT, "sk-other-0002");
        assert_eq!(run(&p, &typed, true, true, None)["ok"], true);
        // Same address, another model: the kept key stays.
        let shown = view(&p)["text"].as_str().unwrap().to_string();
        assert_eq!(
            run(&p, &shown.replace("\"m\"", "\"m2\""), true, true, None)["ok"],
            true
        );
        assert!(
            std::fs::read_to_string(&p)
                .unwrap()
                .contains("sk-other-0002")
        );
    }

    #[test]
    fn a_file_changed_since_it_was_shown_is_not_written_over() {
        let p = temp("changed");
        std::fs::write(&p, "clipboard = false\n").unwrap();
        let v = view(&p);
        let hash = v["hash"].as_str().unwrap().to_string();
        // Someone else changes it meanwhile.
        crate::config::edit_file(&p, "text_only", crate::config::Edit::Set("true".into())).unwrap();
        let r = run(&p, "clipboard = true\n", true, true, Some(&hash));
        assert_eq!(r["ok"], false, "{r}");
        assert!(std::fs::read_to_string(&p).unwrap().contains("text_only"));
        // With what is there now, it goes through.
        let now = view(&p)["hash"].as_str().unwrap().to_string();
        let r = run(&p, "clipboard = true\n", true, true, Some(&now));
        assert_eq!(r["ok"], true, "{r}");
    }
}
