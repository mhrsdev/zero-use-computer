//! Saved scripts: `<dir>/<name>.rhai`, plain text the user can read and
//! edit. The first lines say what the script does and what it takes:
//!
//! ```text
//! // description: Paint a chessboard on the page
//! // params: {"size": {"type": "number", "description": "page width"}}
//! ```

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{Value, json};

/// Longest saved script.
const MAX_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    pub name: String,
    pub description: String,
    /// Its arguments: JSON-schema properties.
    pub params: Value,
    /// The whole file (the header lines are comments).
    pub code: String,
    pub path: PathBuf,
}

impl Saved {
    /// Its arguments as a tool's input schema.
    pub fn input_schema(&self) -> Value {
        let mut schema = match &self.params {
            Value::Object(m) if m.get("type") == Some(&json!("object")) => self.params.clone(),
            Value::Object(m) => json!({"type": "object", "properties": m}),
            _ => json!({"type": "object", "properties": {}}),
        };
        if schema.get("properties").is_none() {
            schema["properties"] = json!({});
        }
        schema
    }
}

/// The saved scripts in one folder, read again when its files change.
#[derive(Debug, Default)]
pub struct Library {
    dir: PathBuf,
    saved: Vec<Saved>,
    /// (file name, modified, size) of every file when last read.
    stamp: Option<Vec<(String, Option<SystemTime>, u64)>>,
    /// Goes up whenever the saved scripts change (for caches of the tool
    /// list).
    generation: u64,
}

/// A saved script's name: a tool name too, so lowercase letters, digits,
/// `_` and `-`, starting with a letter.
pub fn valid_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 48
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "\"{name}\" can't be a script name: use lowercase letters, digits, _ and - (starting with a letter, at most 48)"
        ))
    }
}

impl Library {
    pub fn new(dir: PathBuf) -> Self {
        Library {
            dir,
            saved: Vec::new(),
            stamp: None,
            generation: 0,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Use another folder (the settings changed).
    pub fn set_dir(&mut self, dir: PathBuf) {
        if dir != self.dir {
            let generation = self.generation + 1;
            *self = Library::new(dir);
            self.generation = generation;
        }
    }

    /// A number that changes whenever the saved scripts do.
    pub fn generation(&mut self) -> u64 {
        self.refresh();
        self.generation
    }

    /// Every saved script, by name.
    pub fn list(&mut self) -> &[Saved] {
        self.refresh();
        &self.saved
    }

    pub fn get(&mut self, name: &str) -> Option<Saved> {
        let name = name.trim().to_lowercase();
        self.list().iter().find(|s| s.name == name).cloned()
    }

    /// Save (or replace) a script. Returns its file.
    pub fn save(
        &mut self,
        name: &str,
        code: &str,
        description: &str,
        params: &Value,
    ) -> Result<PathBuf, String> {
        valid_name(name)?;
        if !matches!(params, Value::Object(_) | Value::Null) {
            return Err(
                "params are the script's arguments as JSON-schema properties: {\"size\": {\"type\": \"number\"}}"
                    .into(),
            );
        }
        let description = description.split_whitespace().collect::<Vec<_>>().join(" ");
        if description.is_empty() {
            return Err(
                "give a description: what the script does (it is the tool's description)".into(),
            );
        }
        let mut text = format!("// description: {description}\n");
        if let Value::Object(m) = params
            && !m.is_empty()
        {
            text.push_str(&format!("// params: {params}\n"));
        }
        text.push_str(strip_header(code).trim_start_matches('\n'));
        if !text.ends_with('\n') {
            text.push('\n');
        }
        if text.len() as u64 > MAX_BYTES {
            return Err(format!("a saved script is at most {} KB", MAX_BYTES / 1024));
        }
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("can't make {}: {e}", self.dir.display()))?;
        let path = self.dir.join(format!("{name}.rhai"));
        let tmp = self.dir.join(format!(".{name}.rhai.tmp"));
        std::fs::write(&tmp, &text).map_err(|e| format!("can't write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("can't write {}: {e}", path.display())
        })?;
        self.stamp = None;
        Ok(path)
    }

    pub fn delete(&mut self, name: &str) -> Result<PathBuf, String> {
        let saved = self
            .get(name)
            .ok_or_else(|| format!("no saved script called \"{name}\""))?;
        std::fs::remove_file(&saved.path)
            .map_err(|e| format!("can't delete {}: {e}", saved.path.display()))?;
        self.stamp = None;
        Ok(saved.path)
    }

    /// Read the folder again if any script in it changed.
    fn refresh(&mut self) {
        let mut stamp = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if !name.ends_with(".rhai") || name.starts_with('.') {
                    continue;
                }
                let meta = e.metadata().ok();
                stamp.push((
                    name,
                    meta.as_ref().and_then(|m| m.modified().ok()),
                    meta.map_or(0, |m| m.len()),
                ));
            }
        }
        stamp.sort();
        if self.stamp.as_ref() == Some(&stamp) {
            return;
        }
        self.generation += 1;
        self.saved = stamp
            .iter()
            .filter(|(_, _, len)| *len <= MAX_BYTES)
            .filter_map(|(file, _, _)| {
                let name = file.strip_suffix(".rhai")?.to_string();
                valid_name(&name).ok()?;
                let path = self.dir.join(file);
                let text = std::fs::read_to_string(&path).ok()?;
                Some(parse(name, text, path))
            })
            .collect();
        self.stamp = Some(stamp);
    }
}

/// A saved script from its file.
fn parse(name: String, code: String, path: PathBuf) -> Saved {
    let mut description = String::new();
    let mut params = Value::Null;
    for line in code
        .lines()
        .take_while(|l| l.trim_start().starts_with("//"))
    {
        let body = line.trim_start().trim_start_matches('/').trim();
        if let Some(d) = body.strip_prefix("description:") {
            description = d.trim().to_string();
        } else if let Some(p) = body.strip_prefix("params:") {
            params = serde_json::from_str(p.trim()).unwrap_or(Value::Null);
        }
    }
    if description.is_empty() {
        description = format!("The saved script {name}.");
    }
    Saved {
        name,
        description,
        params,
        code,
        path,
    }
}

/// The code without a description/params header (it is written anew).
fn strip_header(code: &str) -> &str {
    let mut rest = code;
    loop {
        let line_end = rest.find('\n').map_or(rest.len(), |i| i + 1);
        let body = rest[..line_end].trim_start().trim_start_matches('/').trim();
        let header = rest[..line_end].trim_start().starts_with("//")
            && (body.starts_with("description:") || body.starts_with("params:"));
        if !header || line_end == 0 {
            return rest;
        }
        rest = &rest[line_end..];
    }
}
