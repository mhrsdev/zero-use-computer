//! Which apps may be controlled. Some rules are hard-coded (like Codex, which
//! can't automate terminals, itself, or OS security prompts); the rest come
//! from managed policy, user config and per-session approvals.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::config::{ApprovalMode, ConfigStore};
use crate::types::AppInfo;

/// Terminal emulators and shells. Controlling them would let a GUI agent run
/// arbitrary commands outside the host's sandbox.
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
];

/// OS authentication/consent prompts and credential stores.
const SECURITY: &[&str] = &[
    // macOS
    "com.apple.securityagent",
    "securityagent",
    "com.apple.localauthentication.uiagent",
    "coreautha",
    "com.apple.keychainaccess",
    "keychain access",
    "com.apple.passwords",
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
    "seahorse",
    "kwalletmanager5",
    // Password managers (all platforms)
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

/// Processes that must never be controlled: this server and its parent (the
/// agent that launched it).
pub fn protected_pids() -> Vec<u32> {
    #[allow(unused_mut)]
    let mut pids = vec![std::process::id()];
    #[cfg(unix)]
    pids.push(std::os::unix::process::parent_id());
    pids
}

/// Why an app can never be controlled, if it can't.
pub fn hard_block_reason(app: &AppInfo, store: &ConfigStore) -> Option<String> {
    let keys = app.match_keys();
    if protected_pids().contains(&app.pid) {
        return Some("this is the agent's own process".into());
    }
    if matches_any(TERMINALS, &keys) {
        return Some("terminal apps can't be automated with computer use".into());
    }
    if matches_any(SECURITY, &keys) {
        return Some(
            "authentication prompts and credential managers can't be automated; ask the user to do this step".into(),
        );
    }
    if matches_any(&store.config.approvals.agent_apps, &keys) {
        return Some("an agent host app can't operate itself".into());
    }
    if matches_any(&store.managed.denied_apps, &keys) {
        return Some("blocked by your administrator's managed policy".into());
    }
    if let Some(allowed) = &store.managed.allowed_apps
        && !matches_any(allowed, &keys)
    {
        return Some("not in your administrator's list of allowed apps".into());
    }
    if matches_any(&store.config.approvals.always_deny, &keys) {
        return Some("listed in approvals.always_deny in your config".into());
    }
    None
}

/// Decide whether `app` may be used right now.
pub fn evaluate(app: &AppInfo, store: &ConfigStore, session_allowed: &HashSet<String>) -> Verdict {
    if let Some(reason) = hard_block_reason(app, store) {
        return Verdict::Blocked(reason);
    }
    let keys = app.match_keys();
    if session_allowed.contains(&app.id.to_lowercase())
        || matches_any(&store.config.approvals.always_allow, &keys)
    {
        return Verdict::Allowed;
    }
    match store.approval_mode() {
        ApprovalMode::AllowAll => Verdict::Allowed,
        ApprovalMode::Prompt => Verdict::NeedsApproval,
        ApprovalMode::Allowlist => Verdict::Blocked(format!(
            "not approved. Ask the user to add \"{}\" to approvals.always_allow in {}",
            app.id,
            store
                .path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "the config".into())
        )),
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
    fn terminals_are_always_blocked() {
        let s = store(ApprovalMode::AllowAll);
        let none = HashSet::new();
        for a in [
            app("Terminal", "com.apple.Terminal", None),
            app(
                "Windows Terminal",
                "WindowsTerminal",
                Some(r"C:\x\WindowsTerminal.exe"),
            ),
            app("xterm", "xterm", Some("/usr/bin/xterm")),
        ] {
            assert!(
                matches!(evaluate(&a, &s, &none), Verdict::Blocked(_)),
                "{a:?}"
            );
        }
    }

    #[test]
    fn wildcard_and_security() {
        let s = store(ApprovalMode::AllowAll);
        let none = HashSet::new();
        assert!(matches!(
            evaluate(&app("pinentry-gtk-2", "pinentry-gtk-2", None), &s, &none),
            Verdict::Blocked(_)
        ));
        assert!(matches!(
            evaluate(
                &app("1Password 7", "com.agilebits.onepassword7", None),
                &s,
                &none
            ),
            Verdict::Blocked(_)
        ));
        assert_eq!(
            evaluate(&app("TextEdit", "com.apple.TextEdit", None), &s, &none),
            Verdict::Allowed
        );
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
    fn managed_policy_wins() {
        let te = app("TextEdit", "com.apple.TextEdit", None);
        let mut s = store(ApprovalMode::AllowAll);
        s.managed = ManagedConfig {
            allowed_apps: Some(vec!["com.apple.Safari".into()]),
            ..Default::default()
        };
        assert!(matches!(
            evaluate(&te, &s, &HashSet::new()),
            Verdict::Blocked(_)
        ));
        s.managed.allowed_apps = None;
        s.managed.denied_apps = vec!["TextEdit".into()];
        s.config.approvals.always_allow.push("TextEdit".into());
        assert!(matches!(
            evaluate(&te, &s, &HashSet::new()),
            Verdict::Blocked(_)
        ));
    }

    #[test]
    fn own_process_blocked() {
        let mut a = app("me", "me", None);
        a.pid = std::process::id();
        let s = store(ApprovalMode::AllowAll);
        assert!(matches!(
            evaluate(&a, &s, &HashSet::new()),
            Verdict::Blocked(_)
        ));
    }
}
