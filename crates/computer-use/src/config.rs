//! User configuration (`~/.computer-use/config.toml`) and admin-managed
//! policy (`managed.toml`), mirroring how Codex keeps Computer Use approvals
//! in `$CODEX_HOME/config.toml` and lets workspace admins restrict apps.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const HOME_ENV: &str = "COMPUTER_USE_HOME";
pub const MANAGED_ENV: &str = "COMPUTER_USE_MANAGED_CONFIG";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// Ask the user the first time each app is used (MCP elicitation or the
    /// embedding agent's own prompt). Default.
    #[default]
    Prompt,
    /// Only apps in `always_allow` may be used; never prompt.
    Allowlist,
    /// Every app not blocked by policy is allowed without asking.
    AllowAll,
}

impl std::str::FromStr for ApprovalMode {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.replace('-', "_").as_str() {
            "prompt" => Ok(Self::Prompt),
            "allowlist" => Ok(Self::Allowlist),
            "allow_all" => Ok(Self::AllowAll),
            other => Err(Error::Config(format!(
                "unknown approval mode `{other}` (expected prompt, allowlist or allow-all)"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApprovalConfig {
    pub mode: ApprovalMode,
    /// Apps approved with "Always allow" (ids or names, case-insensitive).
    pub always_allow: Vec<String>,
    /// Apps the user never wants controlled.
    pub always_deny: Vec<String>,
    /// Apps that host agents (the agent must not operate itself).
    pub agent_apps: Vec<String>,
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            mode: ApprovalMode::Prompt,
            always_allow: Vec::new(),
            always_deny: Vec::new(),
            agent_apps: [
                "com.openai.chat",
                "com.openai.codex",
                "ChatGPT",
                "Codex",
                "com.anthropic.claudefordesktop",
                "Claude",
            ]
            .map(String::from)
            .to_vec(),
        }
    }
}

/// How a sensitive category (or on-screen action) is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SensitiveMode {
    /// Refuse, unless the specific app is in `approvals.always_allow`. Default.
    #[default]
    Block,
    /// Ask the user each time (via approval), even in allow-all mode.
    Ask,
    /// Treat like any other app (normal approval mode applies).
    Allow,
}

impl std::str::FromStr for SensitiveMode {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "block" | "deny" | "off" => Ok(Self::Block),
            "ask" | "prompt" => Ok(Self::Ask),
            "allow" | "on" => Ok(Self::Allow),
            other => Err(Error::Config(format!(
                "unknown sensitive mode `{other}` (expected block, ask or allow)"
            ))),
        }
    }
}

/// Per-category handling of apps that are sensitive to automate. Everything
/// defaults to `block`, but each category can be relaxed to `ask` or `allow`,
/// and any individual app can always be permitted by adding it to
/// `approvals.always_allow` (that per-app allowance overrides the category).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SensitiveConfig {
    /// Terminal emulators and shells (they can run arbitrary commands).
    pub terminals: SensitiveMode,
    /// Password managers and credential stores.
    pub credentials: SensitiveMode,
    /// OS authentication / consent / login prompts.
    pub security_prompts: SensitiveMode,
    /// Agent host apps (controlling the agent's own UI).
    pub agent_apps: SensitiveMode,
    /// The agent's own process (and its parent).
    pub own_process: SensitiveMode,
    /// Extra app patterns to treat as terminals (id/name, `*` suffix wildcard).
    pub extra_terminals: Vec<String>,
    /// Extra app patterns to treat as credential stores.
    pub extra_credentials: Vec<String>,
    /// Extra app patterns to treat as security prompts.
    pub extra_security_prompts: Vec<String>,
}

impl Default for SensitiveConfig {
    fn default() -> Self {
        Self {
            terminals: SensitiveMode::Block,
            credentials: SensitiveMode::Block,
            security_prompts: SensitiveMode::Block,
            agent_apps: SensitiveMode::Block,
            own_process: SensitiveMode::Block,
            extra_terminals: Vec::new(),
            extra_credentials: Vec::new(),
            extra_security_prompts: Vec::new(),
        }
    }
}

/// Confirmation guard for consequential on-screen actions (pressing a control
/// whose label looks like Send / Delete / Pay …). Independent of app approval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuardConfig {
    /// `ask` (default): require approval before such an action. `allow`: never
    /// confirm. `block`: refuse them outright.
    pub mode: SensitiveMode,
    /// Case-insensitive substrings of an element's label/role that trigger it.
    pub keywords: Vec<String>,
}

impl Default for GuardConfig {
    fn default() -> Self {
        Self {
            mode: SensitiveMode::Ask,
            keywords: [
                "send",
                "delete",
                "remove",
                "discard",
                "pay",
                "buy",
                "purchase",
                "checkout",
                "order",
                "transfer",
                "confirm",
                "publish",
                "post",
                "submit",
                "trash",
                "erase",
                "wipe",
                "shut down",
                "restart",
                "log out",
                "sign out",
                "uninstall",
                "format",
            ]
            .map(String::from)
            .to_vec(),
        }
    }
}

/// Optional JSONL audit log of every tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AuditConfig {
    pub enabled: bool,
    /// File to append to. Defaults to `<home>/audit.log` when enabled.
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenshotConfig {
    /// Attach a screenshot to get_app_state.
    pub enabled: bool,
    /// Longest edge of the image sent to the model, in pixels.
    pub max_dimension: u32,
    pub format: ImageFormat,
    pub jpeg_quality: u8,
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_dimension: 1280,
            format: ImageFormat::Png,
            jpeg_quality: 85,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TreeConfig {
    /// Maximum elements rendered in one get_app_state.
    pub max_nodes: usize,
    /// Maximum elements the backend walks before pruning.
    pub max_walk: usize,
    pub max_depth: usize,
    /// Longest text/value shown per element before truncation.
    pub max_text_len: usize,
    /// Return a diff instead of the full tree when little changed.
    pub diff: bool,
    /// After a mutating action, re-snapshot and append what changed.
    pub report_changes: bool,
}

impl Default for TreeConfig {
    fn default() -> Self {
        Self {
            max_nodes: 1200,
            max_walk: 6000,
            max_depth: 64,
            max_text_len: 200,
            diff: true,
            report_changes: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub approvals: ApprovalConfig,
    pub sensitive: SensitiveConfig,
    pub guard: GuardConfig,
    pub screenshot: ScreenshotConfig,
    pub tree: TreeConfig,
    pub audit: AuditConfig,
    /// Expose the clipboard tools (get_clipboard / set_clipboard).
    pub clipboard: bool,
    /// Never attach screenshots to any tool result (tree-only operation).
    pub text_only: bool,
    /// Seconds launch_app waits for the app to show a window.
    pub launch_timeout_secs: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            approvals: ApprovalConfig::default(),
            sensitive: SensitiveConfig::default(),
            guard: GuardConfig::default(),
            screenshot: ScreenshotConfig::default(),
            tree: TreeConfig::default(),
            audit: AuditConfig::default(),
            clipboard: true,
            text_only: false,
            launch_timeout_secs: 15.0,
        }
    }
}

/// Admin policy. Takes precedence over the user config.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ManagedConfig {
    /// Apps that can never be controlled.
    pub denied_apps: Vec<String>,
    /// When set, only these apps can be controlled.
    pub allowed_apps: Option<Vec<String>>,
    /// Forces the approval mode.
    pub approval_mode: Option<ApprovalMode>,
    /// Disables persisting "Always allow" decisions.
    pub disable_always_allow: bool,
}

pub fn home_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(HOME_ENV) {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".computer-use")
}

pub fn default_config_path() -> PathBuf {
    home_dir().join("config.toml")
}

pub fn managed_config_path() -> PathBuf {
    if let Some(p) = std::env::var_os(MANAGED_ENV) {
        return PathBuf::from(p);
    }
    if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/ComputerUse/managed.toml")
    } else if cfg!(windows) {
        let base = std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into());
        PathBuf::from(base).join("ComputerUse").join("managed.toml")
    } else {
        PathBuf::from("/etc/computer-use/managed.toml")
    }
}

/// Configuration plus where it lives, so approvals can be persisted.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    pub config: Config,
    pub managed: ManagedConfig,
    /// `None` keeps everything in memory (tests, embedders).
    pub path: Option<PathBuf>,
}

impl ConfigStore {
    pub fn in_memory(config: Config) -> Self {
        Self {
            config,
            managed: ManagedConfig::default(),
            path: None,
        }
    }

    /// Load the user config (missing file = defaults) and managed policy.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let path = path
            .map(Path::to_path_buf)
            .unwrap_or_else(default_config_path);
        let config = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|e| Error::Config(format!("{}: {e}", path.display())))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(e) => return Err(Error::Config(format!("{}: {e}", path.display()))),
        };
        let managed_path = managed_config_path();
        let managed = match std::fs::read_to_string(&managed_path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|e| Error::Config(format!("{}: {e}", managed_path.display())))?,
            Err(_) => ManagedConfig::default(),
        };
        Ok(Self {
            config,
            managed,
            path: Some(path),
        })
    }

    pub fn approval_mode(&self) -> ApprovalMode {
        self.managed
            .approval_mode
            .unwrap_or(self.config.approvals.mode)
    }

    /// Record an "Always allow" decision, in memory and on disk. The file is
    /// edited in place so user comments and formatting survive.
    pub fn add_always_allow(&mut self, app_id: &str) -> Result<()> {
        if self.managed.disable_always_allow {
            return Ok(());
        }
        let list = &mut self.config.approvals.always_allow;
        if list.iter().any(|a| a.eq_ignore_ascii_case(app_id)) {
            return Ok(());
        }
        list.push(app_id.to_string());
        let Some(path) = &self.path else {
            return Ok(());
        };

        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(Error::Config(format!("{}: {e}", path.display()))),
        };
        let mut doc: toml_edit::DocumentMut = text
            .parse()
            .map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
        let approvals = doc
            .entry("approvals")
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_mut()
            .ok_or_else(|| Error::Config("`approvals` must be a table".into()))?;
        let arr = approvals
            .entry("always_allow")
            .or_insert_with(|| toml_edit::value(toml_edit::Array::new()))
            .as_array_mut()
            .ok_or_else(|| Error::Config("`approvals.always_allow` must be an array".into()))?;
        arr.push(app_id);

        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| Error::Config(format!("{}: {e}", dir.display())))?;
        }
        std::fs::write(path, doc.to_string())
            .map_err(|e| Error::Config(format!("{}: {e}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_missing() {
        let dir = std::env::temp_dir().join(format!("cu-cfg-missing-{}", std::process::id()));
        let store = ConfigStore::load(Some(&dir.join("config.toml"))).unwrap();
        assert_eq!(store.config, Config::default());
        assert_eq!(store.approval_mode(), ApprovalMode::Prompt);
    }

    #[test]
    fn parses_partial_config() {
        let cfg: Config = toml::from_str(
            r#"
            [approvals]
            mode = "allowlist"
            always_allow = ["com.apple.TextEdit"]
            [screenshot]
            max_dimension = 1024
            "#,
        )
        .unwrap();
        assert_eq!(cfg.approvals.mode, ApprovalMode::Allowlist);
        assert_eq!(cfg.screenshot.max_dimension, 1024);
        assert!(cfg.screenshot.enabled);
        assert!(!cfg.approvals.agent_apps.is_empty());
    }

    #[test]
    fn always_allow_preserves_comments() {
        let dir = std::env::temp_dir().join(format!("cu-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "# my settings\n[screenshot]\nmax_dimension = 900\n").unwrap();
        let mut store = ConfigStore::load(Some(&path)).unwrap();
        store.add_always_allow("com.apple.TextEdit").unwrap();
        store.add_always_allow("com.apple.textedit").unwrap(); // duplicate, ignored
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my settings"));
        assert!(text.contains("always_allow = [\"com.apple.TextEdit\"]"));
        let reloaded = ConfigStore::load(Some(&path)).unwrap();
        assert_eq!(
            reloaded.config.approvals.always_allow,
            vec!["com.apple.TextEdit"]
        );
        assert_eq!(reloaded.config.screenshot.max_dimension, 900);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn approval_mode_from_str() {
        assert_eq!(
            "allow-all".parse::<ApprovalMode>().unwrap(),
            ApprovalMode::AllowAll
        );
        assert!("yolo".parse::<ApprovalMode>().is_err());
    }
}
