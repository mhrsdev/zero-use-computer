//! Which apps may be controlled.
//!
//! Some apps are *sensitive* to automate — terminals, password managers, OS
//! authentication prompts, and the agent's own UI. Each such category has a
//! configurable mode (`block` by default, or `ask` / `allow`), and any
//! individual app can always be permitted by adding it to
//! `approvals.always_allow`. Admin `managed.toml` policy still overrides
//! everything.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::config::{ApprovalMode, ConfigStore, SensitiveMode};
use crate::types::AppInfo;

/// Terminal emulators and shells (they can run arbitrary commands).
const TERMINALS: &[&str] = &[
    // macOS
    "com.apple.terminal",
    "com.googlecode.iterm2",
    "dev.warp.warp-stable",
    "dev.warp.warp",
    "io.alacritty",
    "org.alacritty",
    "net.kovidgoyal.kitty",
    "com.github.wez.wezterm",
    "co.zeit.hyper",
    "com.mitchellh.ghostty",
    "com.raphaelamorim.rio",
    "org.tabby",
    "terminal",
    "iterm2",
    "iterm",
    "warp",
    "ghostty",
    // Windows
    "cmd",
    "powershell",
    "powershell_ise",
    "pwsh",
    "windowsterminal",
    "windows terminal",
    "wt",
    "conhost",
    "openconsole",
    "mintty",
    "wezterm-gui",
    "command prompt",
    // Linux
    "gnome-terminal",
    "gnome-terminal-server",
    "konsole",
    "xterm",
    "uxterm",
    "xfce4-terminal",
    "tilix",
    "terminator",
    "alacritty",
    "kitty",
    "wezterm",
    "foot",
    "footclient",
    "urxvt",
    "rxvt",
    "lxterminal",
    "mate-terminal",
    "qterminal",
    "kgx",
    "org.gnome.console",
    "ptyxis",
    "st",
    "sakura",
    "guake",
    "yakuake",
    "terminology",
    "cool-retro-term",
    "blackbox",
    "hyper",
    "tabby",
    "x-terminal-emulator",
    // Shells and subsystems that run arbitrary commands (also reachable as
    // `launch_app("sh -c …")`).
    "sh",
    "bash",
    "zsh",
    "dash",
    "fish",
    "ksh",
    "csh",
    "tcsh",
    "wsl",
];

/// Password managers and credential stores.
const CREDENTIALS: &[&str] = &[
    "com.apple.keychainaccess",
    "keychain access",
    "com.apple.passwords",
    "seahorse",
    "kwalletmanager5",
    "1password*",
    "com.1password.*",
    "com.agilebits.*",
    "bitwarden",
    "com.bitwarden.*",
    "keepassxc",
    "org.keepassxc.*",
    "keepass",
    "dashlane",
    "lastpass",
];

/// OS authentication / consent / login prompts.
const SECURITY_PROMPTS: &[&str] = &[
    // macOS
    "com.apple.securityagent",
    "securityagent",
    "com.apple.localauthentication.uiagent",
    "coreautha",
    // Windows
    "consent",
    "credentialuibroker",
    "logonui",
    "lockapp",
    // Linux
    "polkit-gnome-authentication-agent-1",
    "polkit-kde-authentication-agent-1",
    "lxpolkit",
    "gcr-prompter",
    "pinentry*",
    "ssh-askpass*",
];

/// A category of app that is sensitive to automate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Terminal,
    Credentials,
    SecurityPrompt,
    AgentApp,
    OwnProcess,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::Terminal => "a terminal",
            Category::Credentials => "a password manager / credential store",
            Category::SecurityPrompt => "an OS security or login prompt",
            Category::AgentApp => "an agent host app",
            Category::OwnProcess => "the agent's own process",
        }
    }
    /// The `sensitive.<key>` this category is configured under.
    pub fn config_key(self) -> &'static str {
        match self {
            Category::Terminal => "terminals",
            Category::Credentials => "credentials",
            Category::SecurityPrompt => "security_prompts",
            Category::AgentApp => "agent_apps",
            Category::OwnProcess => "own_process",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    Allowed,
    NeedsApproval,
    Blocked(String),
}

fn matches(pattern: &str, keys: &[String]) -> bool {
    let p = pattern.to_lowercase();
    match p.strip_suffix('*') {
        Some(prefix) => keys.iter().any(|k| k.starts_with(prefix)),
        None => keys.contains(&p),
    }
}

fn matches_any<S: AsRef<str>>(patterns: &[S], keys: &[String]) -> bool {
    patterns.iter().any(|p| matches(p.as_ref(), keys))
}

/// Processes that must not be controlled by default: this server and its
/// parent (the agent that launched it).
pub fn protected_pids() -> Vec<u32> {
    #[allow(unused_mut)]
    let mut pids = vec![std::process::id()];
    #[cfg(unix)]
    pids.push(std::os::unix::process::parent_id());
    pids
}

/// Classify an app into a sensitive category, if any.
pub fn classify(app: &AppInfo, store: &ConfigStore) -> Option<Category> {
    let keys = app.match_keys();
    let s = &store.config.sensitive;
    if protected_pids().contains(&app.pid) {
        return Some(Category::OwnProcess);
    }
    if matches_any(&store.config.approvals.agent_apps, &keys) {
        return Some(Category::AgentApp);
    }
    if matches_any(TERMINALS, &keys) || matches_any(&s.extra_terminals, &keys) {
        return Some(Category::Terminal);
    }
    if matches_any(CREDENTIALS, &keys) || matches_any(&s.extra_credentials, &keys) {
        return Some(Category::Credentials);
    }
    if matches_any(SECURITY_PROMPTS, &keys) || matches_any(&s.extra_security_prompts, &keys) {
        return Some(Category::SecurityPrompt);
    }
    None
}

fn category_mode(cat: Category, store: &ConfigStore) -> SensitiveMode {
    let s = &store.config.sensitive;
    match cat {
        Category::Terminal => s.terminals,
        Category::Credentials => s.credentials,
        Category::SecurityPrompt => s.security_prompts,
        Category::AgentApp => s.agent_apps,
        Category::OwnProcess => s.own_process,
    }
}

fn config_path(store: &ConfigStore) -> String {
    store
        .path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "your config".into())
}

/// Decide whether `app` may be used right now.
pub fn evaluate(app: &AppInfo, store: &ConfigStore, session_allowed: &HashSet<String>) -> Verdict {
    let keys = app.match_keys();

    // 1. Admin managed policy — cannot be overridden by the user.
    if matches_any(&store.managed.denied_apps, &keys) {
        return Verdict::Blocked("blocked by your administrator's managed policy".into());
    }
    if let Some(allowed) = &store.managed.allowed_apps
        && !matches_any(allowed, &keys)
    {
        return Verdict::Blocked("not in your administrator's list of allowed apps".into());
    }

    // 2. The user's own explicit deny.
    if matches_any(&store.config.approvals.always_deny, &keys) {
        return Verdict::Blocked("listed in approvals.always_deny in your config".into());
    }

    // A specific app the user has allowed overrides sensitive-category rules.
    let explicitly_allowed = session_allowed.contains(&app.id.to_lowercase())
        || matches_any(&store.config.approvals.always_allow, &keys);

    // 3. Sensitive categories.
    if let Some(cat) = classify(app, store) {
        match category_mode(cat, store) {
            SensitiveMode::Allow => {} // fall through to the normal approval flow
            SensitiveMode::Ask => {
                return if explicitly_allowed {
                    Verdict::Allowed
                } else {
                    Verdict::NeedsApproval
                };
            }
            SensitiveMode::Block => {
                if explicitly_allowed {
                    return Verdict::Allowed;
                }
                return Verdict::Blocked(format!(
                    "{app} is {what}, which is blocked by default. To allow it, add \"{id}\" to approvals.always_allow, or set sensitive.{key} = \"ask\" (or \"allow\") in {path}.",
                    app = app.name,
                    what = cat.label(),
                    id = app.id,
                    key = cat.config_key(),
                    path = config_path(store),
                ));
            }
        }
    }

    // 4. Normal approval flow.
    if explicitly_allowed {
        return Verdict::Allowed;
    }
    match store.approval_mode() {
        ApprovalMode::AllowAll => Verdict::Allowed,
        ApprovalMode::Prompt => Verdict::NeedsApproval,
        ApprovalMode::Allowlist => Verdict::Blocked(format!(
            "not approved. Add \"{}\" to approvals.always_allow in {}",
            app.id,
            config_path(store),
        )),
    }
}

/// A short reason an app is blocked or needs approval, for `list_apps` tags.
pub fn tag(
    app: &AppInfo,
    store: &ConfigStore,
    session_allowed: &HashSet<String>,
) -> Option<String> {
    match evaluate(app, store, session_allowed) {
        Verdict::Allowed => None,
        Verdict::NeedsApproval => Some("needs approval".into()),
        Verdict::Blocked(_) => {
            let cat = classify(app, store).map(|c| c.label());
            Some(match cat {
                Some(what) => format!("blocked ({what})"),
                None => "blocked".into(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ManagedConfig};

    fn app(name: &str, id: &str, exe: Option<&str>) -> AppInfo {
        AppInfo {
            name: name.into(),
            id: id.into(),
            pid: 999_999,
            exe: exe.map(String::from),
            frontmost: false,
            hidden: false,
        }
    }

    fn store(mode: ApprovalMode) -> ConfigStore {
        let mut cfg = Config::default();
        cfg.approvals.mode = mode;
        ConfigStore::in_memory(cfg)
    }

    #[test]
    fn terminals_blocked_by_default_but_configurable() {
        let none = HashSet::new();
        let mut s = store(ApprovalMode::AllowAll);
        let term = app("iTerm2", "com.googlecode.iterm2", None);
        assert!(matches!(evaluate(&term, &s, &none), Verdict::Blocked(_)));

        // Relax the whole category to "ask".
        s.config.sensitive.terminals = SensitiveMode::Ask;
        assert_eq!(evaluate(&term, &s, &none), Verdict::NeedsApproval);

        // Or allow the category outright (normal approval then applies).
        s.config.sensitive.terminals = SensitiveMode::Allow;
        assert_eq!(evaluate(&term, &s, &none), Verdict::Allowed);
    }

    #[test]
    fn per_app_allow_overrides_category() {
        let none = HashSet::new();
        let mut s = store(ApprovalMode::AllowAll);
        // Category stays blocked, but this one terminal is explicitly allowed.
        s.config
            .approvals
            .always_allow
            .push("com.googlecode.iterm2".into());
        let term = app("iTerm2", "com.googlecode.iterm2", None);
        assert_eq!(evaluate(&term, &s, &none), Verdict::Allowed);
        // A different terminal is still blocked.
        let other = app("xterm", "xterm", Some("/usr/bin/xterm"));
        assert!(matches!(evaluate(&other, &s, &none), Verdict::Blocked(_)));
    }

    #[test]
    fn credentials_and_security_are_separate_categories() {
        let none = HashSet::new();
        let mut s = store(ApprovalMode::AllowAll);
        let pw = app("1Password", "com.agilebits.onepassword7", None);
        let auth = app("pinentry-gtk-2", "pinentry-gtk-2", None);
        assert_eq!(classify(&pw, &s), Some(Category::Credentials));
        assert_eq!(classify(&auth, &s), Some(Category::SecurityPrompt));
        // Relaxing terminals doesn't unblock credentials.
        s.config.sensitive.terminals = SensitiveMode::Allow;
        assert!(matches!(evaluate(&pw, &s, &none), Verdict::Blocked(_)));
        s.config.sensitive.credentials = SensitiveMode::Allow;
        assert_eq!(evaluate(&pw, &s, &none), Verdict::Allowed);
    }

    #[test]
    fn extra_patterns_extend_categories() {
        let none = HashSet::new();
        let mut s = store(ApprovalMode::AllowAll);
        s.config.sensitive.extra_terminals.push("myshell".into());
        let app = app("MyShell", "myshell", None);
        assert_eq!(classify(&app, &s), Some(Category::Terminal));
        assert!(matches!(evaluate(&app, &s, &none), Verdict::Blocked(_)));
    }

    #[test]
    fn normal_apps_unaffected() {
        let none = HashSet::new();
        let s = store(ApprovalMode::AllowAll);
        let te = app("TextEdit", "com.apple.TextEdit", None);
        assert_eq!(classify(&te, &s), None);
        assert_eq!(evaluate(&te, &s, &none), Verdict::Allowed);
    }

    #[test]
    fn prompt_allowlist_and_session() {
        let te = app("TextEdit", "com.apple.TextEdit", None);
        let mut session = HashSet::new();
        let s = store(ApprovalMode::Prompt);
        assert_eq!(evaluate(&te, &s, &session), Verdict::NeedsApproval);
        session.insert("com.apple.textedit".to_string());
        assert_eq!(evaluate(&te, &s, &session), Verdict::Allowed);

        let mut s = store(ApprovalMode::Allowlist);
        assert!(matches!(
            evaluate(&te, &s, &HashSet::new()),
            Verdict::Blocked(_)
        ));
        s.config.approvals.always_allow.push("textedit".into());
        assert_eq!(evaluate(&te, &s, &HashSet::new()), Verdict::Allowed);
    }

    #[test]
    fn managed_policy_wins_over_user_allow() {
        let te = app("TextEdit", "com.apple.TextEdit", None);
        let mut s = store(ApprovalMode::AllowAll);
        s.managed = ManagedConfig {
            denied_apps: vec!["TextEdit".into()],
            ..Default::default()
        };
        s.config.approvals.always_allow.push("TextEdit".into());
        assert!(matches!(
            evaluate(&te, &s, &HashSet::new()),
            Verdict::Blocked(_)
        ));
    }

    #[test]
    fn own_process_blocked_by_default() {
        let mut a = app("me", "me", None);
        a.pid = std::process::id();
        let mut s = store(ApprovalMode::AllowAll);
        assert_eq!(classify(&a, &s), Some(Category::OwnProcess));
        assert!(matches!(
            evaluate(&a, &s, &HashSet::new()),
            Verdict::Blocked(_)
        ));
        // The user can even allow this if they really want to.
        s.config.sensitive.own_process = SensitiveMode::Allow;
        assert_eq!(evaluate(&a, &s, &HashSet::new()), Verdict::Allowed);
    }
}
