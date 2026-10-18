//! The settings as a host (Zero) asks for them: the panel's own data and
//! checks, answered as requests on the server's own stdio instead of as a
//! page on a port. There is no listener, no token and no page: the host
//! draws the controls itself, from the description of each setting.
//!
//! Two layers hold the settings: `user.toml`, which only `set` and `reset`
//! write, and `managed.toml`, which only the host writes and which wins
//! over it. The host also says in `managed.toml` which settings are the
//! user's (`[host] user_keys`); any other setting is the host's, and a
//! request to change it is refused with `managedByHost`, so a host that is
//! wrong about what it asks for is still stopped here. A host that said
//! nothing has made no setting the user's.
//!
//! Every refusal is a fixed word (`code`), never an English sentence: the
//! host has its own words and acts on the code.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::lang;
use super::schema::{self, GROUPS, Kind};
use super::settings::{self, Rejection};
use crate::config::{self, ConfigStore, Edit};

/// The shape of what `schema` answers. A change that a host reading the old
/// shape would misread is a new number.
pub const SCHEMA_VERSION: u32 = 1;

/// Every request of this API starts with this.
pub const METHOD_PREFIX: &str = "computer_use/settings/";

/// A request that was refused: a fixed word, the setting it is about (when
/// there is one) and what the host may need to act on it.
#[derive(Debug, Clone, PartialEq)]
pub struct HostError {
    pub code: &'static str,
    pub key: Option<String>,
    pub detail: Value,
}

impl HostError {
    fn new(code: &'static str) -> Self {
        Self {
            code,
            key: None,
            detail: Value::Null,
        }
    }

    fn on(code: &'static str, key: &str) -> Self {
        Self {
            code,
            key: Some(key.to_string()),
            detail: Value::Null,
        }
    }

    fn with(mut self, detail: Value) -> Self {
        self.detail = detail;
        self
    }

    /// The `data` of the JSON-RPC error.
    pub fn data(&self) -> Value {
        let mut data = json!({ "code": self.code });
        if let Some(key) = &self.key {
            data["key"] = json!(key);
        }
        if !self.detail.is_null() {
            data["detail"] = self.detail.clone();
        }
        data
    }
}

/// The settings of one server folder, as the host sees them.
#[derive(Debug, Clone)]
pub struct Host {
    user: PathBuf,
    managed: PathBuf,
}

impl Host {
    /// The two layers in the server's folder.
    pub fn in_home(home: &Path) -> Self {
        Self {
            user: home.join("user.toml"),
            managed: home.join("managed.toml"),
        }
    }

    pub fn user_path(&self) -> &Path {
        &self.user
    }

    pub fn managed_path(&self) -> &Path {
        &self.managed
    }

    /// The settings with both layers read: what the server uses.
    pub fn load(&self) -> crate::error::Result<ConfigStore> {
        ConfigStore::load_layers(&self.user, &self.managed)
    }

    /// The settings the user may change: the ones the host named, that exist,
    /// that the host's layer does not set itself.
    pub fn writable(&self) -> Vec<String> {
        let set_by_host = managed_keys(&self.managed);
        config::host_user_keys(&self.managed)
            .into_iter()
            .filter(|k| schema::entry(k).is_some() && !set_by_host.contains(k))
            .collect()
    }

    /// Answers one request, or `None` when it is not one of this API's.
    pub fn handle(&self, method: &str, params: &Value) -> Option<Result<Value, HostError>> {
        let name = method.strip_prefix(METHOD_PREFIX)?;
        Some(match name {
            "schema" => Ok(self.schema()),
            "values" => self.values(),
            "set" => self.set(params, false),
            "reset" => self.set(params, true),
            "texts" => texts(params),
            _ => return None,
        })
    }

    fn schema(&self) -> Value {
        json!({
            "schemaVersion": SCHEMA_VERSION,
            "version": env!("CARGO_PKG_VERSION"),
            "groups": GROUPS,
            "blurbs": schema::blurbs(),
            "entries": settings::entries_json(),
            "languages": languages(),
        })
    }

    fn values(&self) -> Result<Value, HostError> {
        let store = self
            .load()
            .map_err(|_| HostError::new("settingsUnreadable"))?;
        Ok(json!({
            "values": settings::changed_values(&store.config),
            "writable": self.writable(),
        }))
    }

    /// `set {key, value, confirmed?}`; `reset {key}` is `set` with `null`.
    fn set(&self, params: &Value, reset: bool) -> Result<Value, HostError> {
        let key = params
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| HostError::new("badParams"))?;
        let entry = schema::entry(key).ok_or_else(|| HostError::on("unknownKey", key))?;
        if !self.writable().iter().any(|k| k == key) {
            return Err(HostError::on("managedByHost", key));
        }
        if matches!(entry.kind, Kind::Custom | Kind::Secret) {
            return Err(HostError::on("notEditable", key));
        }
        let value = if reset {
            Value::Null
        } else {
            params
                .get("value")
                .cloned()
                .ok_or_else(|| HostError::on("badParams", key))?
        };
        let edit: Edit = match settings::coerce(entry, &value) {
            Ok(Some(edit)) => edit,
            // Nothing to change (only a secret is ever empty-means-keep).
            Ok(None) => Edit::Unset,
            Err(why) => return Err(rejected(key, &why)),
        };
        let confirmed = params.get("confirmed").and_then(Value::as_bool) == Some(true);
        if entry.confirm && !confirmed {
            return Err(HostError::on("needsConfirmation", key));
        }
        config::edit_file_many(&self.user, &[(entry.key, edit)])
            .map_err(|_| HostError::on("writeFailed", key))?;
        let store = self
            .load()
            .map_err(|_| HostError::on("settingsUnreadable", key))?;
        let shown = settings::values_of(&store.config, &[entry.key]);
        Ok(json!({
            "key": entry.key,
            "value": shown.get(entry.key).cloned().unwrap_or(Value::Null),
            "restart": entry.restart,
        }))
    }
}

fn rejected(key: &str, why: &Rejection) -> HostError {
    let error = HostError::on(why.code(), key);
    match why {
        Rejection::NotAChoice(choices) => error.with(json!({ "choices": choices })),
        Rejection::OutOfRange { min, max } => error.with(json!({ "min": min, "max": max })),
        _ => error,
    }
}

/// The settings the host's layer sets itself, as dotted keys.
fn managed_keys(managed: &Path) -> Vec<String> {
    let Ok(text) = config::read_text(managed) else {
        return Vec::new();
    };
    let Ok(mut table) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    table.remove(config::HOST_TABLE);
    let mut keys = Vec::new();
    config::collect_keys("", &table, &mut keys);
    keys
}

fn languages() -> Value {
    Value::Array(
        lang::LANGUAGES
            .iter()
            .map(|(code, name, rtl)| json!({ "code": code, "name": name, "rtl": rtl }))
            .collect(),
    )
}

/// `texts {language}`: the label and the help of every setting, the names of the
/// groups and a line about each, in one language. A text a language does not
/// have is in English; the host sees that from the answer (`translated`).
fn texts(params: &Value) -> Result<Value, HostError> {
    let code = params
        .get("language")
        .and_then(Value::as_str)
        .unwrap_or("en");
    let Some(&(code, _, rtl)) = lang::LANGUAGES.iter().find(|l| l.0 == code) else {
        return Err(HostError::new("unknownLanguage"));
    };
    let table: Value = lang::translations(code)
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or(Value::Null);
    let ui =
        |english: &str| -> String { table["ui"][english].as_str().unwrap_or(english).to_string() };
    let mut settings = serde_json::Map::new();
    for entry in schema::ENTRIES {
        let own = &table["settings"][entry.key];
        let label = own[0]
            .as_str()
            .map_or_else(|| settings::english_label(entry.key), str::to_string);
        let help = own[1].as_str().unwrap_or(entry.help).to_string();
        settings.insert(entry.key.to_string(), json!([label, help]));
    }
    let groups: serde_json::Map<String, Value> = GROUPS
        .iter()
        .map(|g| ((*g).to_string(), json!(ui(g))))
        .collect();
    let blurbs: serde_json::Map<String, Value> = schema::blurbs()
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(group, text)| (group.clone(), json!(ui(text.as_str().unwrap_or("")))))
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({
        "language": code,
        "rtl": rtl,
        "settings": settings,
        "groups": groups,
        "blurbs": blurbs,
        "languages": languages(),
    }))
}
