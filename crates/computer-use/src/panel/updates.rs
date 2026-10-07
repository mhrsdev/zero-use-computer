//! The panel's page for updates: what is known (when it last looked, what
//! waits, what could be gone back to), and the three things the user can
//! ask for: look now, put a waiting update in place, go back.

use std::path::Path;

use serde_json::{Value, json};

use super::settings::fail;
use crate::config::{self, Config};
use crate::update::{self, Found};

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Everything the updates card shows. `dir` is where updates are kept.
pub fn status(cfg: &Config, dir: &Path) -> Value {
    let u = &cfg.update;
    let every = update::interval_secs(u);
    let last = update::last_check(dir);
    let backoff = update::backoff_until(dir).saturating_sub(now());
    let pending = update::pending(dir);
    let next = if !u.enabled {
        Value::Null
    } else if last == 0 {
        json!(u.check_after_mins.saturating_mul(60))
    } else {
        json!(
            last.saturating_add(every)
                .saturating_sub(now())
                .max(backoff)
        )
    };
    // A waiting update the settings no longer take is shown as such; it
    // is forgotten when a server starts or it would be put in place.
    let wanted = pending.as_ref().is_none_or(|p| update::allowed(p, u));
    json!({
        "ok": true,
        "current": update::Version::current().to_string(),
        "enabled": u.enabled,
        "install": format!("{:?}", u.install).to_lowercase(),
        "when": update::when(u.install),
        "every_secs": every,
        "last_check_secs_ago": (last > 0).then(|| now().saturating_sub(last)),
        "next_in_secs": next,
        "rate_limited_for_secs": (backoff > 0).then_some(backoff),
        "channel": u.channel,
        "pin": u.pin,
        "skip_version": u.skip_version,
        "pending": pending.as_ref().map(|p| json!({
            "version": p.version,
            "since_secs": now().saturating_sub(p.downloaded),
            "wanted": wanted,
        })),
        "previous": update::previous(dir).map(|p| json!({"version": p.version})),
        "notes": update::release_notes(dir).map(|n| json!({"version": n.version, "text": n.notes, "page": n.page})),
        "can_update": update::asset_name().is_some(),
    })
}

/// Look now (nothing is put in place).
pub fn check(cfg: &Config, dir: &Path) -> Value {
    if update::asset_name().is_none() {
        return fail("there is no release of this program for this system");
    }
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join("last-check"), now().to_string());
    match update::check_in(&cfg.update, dir, true) {
        Ok(Found::UpToDate) => json!({
            "ok": true,
            "message": format!("{} is the latest under these settings.", update::Version::current()),
        }),
        Ok(Found::Waiting(p)) => json!({
            "ok": true,
            "message": format!("{} is downloaded and checked. It goes in {}.", p.version, update::when(cfg.update.install)),
        }),
        Err(e) => fail(&e.to_string()),
    }
}

fn confirm_text(text: &str) -> Value {
    json!({"ok": false, "error": "this needs the user's confirmation first", "confirm_text": text})
}

/// Put the waiting update in place of the program at `exe` now. The
/// servers already running keep the old one until they start again.
pub fn install(cfg: &Config, dir: &Path, exe: &Path, confirmed: bool) -> Value {
    let Some(p) = update::pending(dir) else {
        return fail("no update is waiting");
    };
    if !update::allowed(&p, &cfg.update) {
        update::discard(dir);
        return fail(&format!(
            "{} is no longer wanted by the settings (skipped, another version pinned, or a pre-release on the stable channel), so it was forgotten",
            p.version
        ));
    }
    if !confirmed {
        return confirm_text(&format!(
            "Put version {} in place of this program now? Your MCP clients use it the next time they start.",
            p.version
        ));
    }
    match update::install(&p, exe, dir) {
        Ok(()) => {
            json!({"ok": true, "message": format!("{} is in place. Start your MCP client again to use it.", p.version)})
        }
        Err(e) => fail(&e.to_string()),
    }
}

/// Go back to the version the last update replaced, and don't take that
/// update again.
pub fn rollback(path: Option<&Path>, dir: &Path, exe: &Path, confirmed: bool) -> Value {
    let Some(prev) = update::previous(dir) else {
        return fail("there is no earlier version kept to go back to");
    };
    if !confirmed {
        return confirm_text(&format!(
            "Go back to version {}? The version you have now is not taken again unless you clear update.skip_version.",
            prev.version
        ));
    }
    match update::rollback(exe, dir) {
        Ok((from, to)) => {
            let mut message = format!("{to} is back. Start your MCP client again to use it.");
            if let Some(p) = path {
                match config::edit_file(
                    p,
                    "update.skip_version",
                    config::Edit::SetText(from.to_string()),
                ) {
                    Ok(()) => message.push_str(&format!(" {from} will not be taken again.")),
                    Err(e) => {
                        message.push_str(&format!(" (Couldn't set update.skip_version: {e})"))
                    }
                }
            }
            json!({"ok": true, "message": message})
        }
        Err(e) => fail(&e.to_string()),
    }
}
