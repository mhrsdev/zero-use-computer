//! Saving the decision model's settings and trying it. The page where the
//! user does this is a tab of the settings panel (see [`crate::panel`]); the
//! API key goes from there to the settings file, never through the chat.

use std::path::Path;

use super::{Decider, Question};
use crate::config::{self, DecisionConfig, Edit};
use crate::error::Result;

/// Save the decision model's settings (all or none).
pub fn save_settings(path: &Path, d: &DecisionConfig) -> Result<()> {
    let text = |v: &str| {
        if v.trim().is_empty() {
            Edit::Unset
        } else {
            Edit::SetText(v.trim().to_string())
        }
    };
    let mut edits = vec![
        ("decision.provider", text(&d.provider)),
        ("decision.base_url", text(&d.base_url)),
        ("decision.model", text(&d.model)),
        ("decision.api_key", text(&d.api_key)),
    ];
    if d.api_key_env.trim().is_empty() {
        edits.push(("decision.api_key_env", Edit::Unset));
    }
    config::edit_file_many(path, &edits)
}

/// Forget the decision model.
pub fn remove_settings(path: &Path) -> Result<()> {
    config::edit_file_many(
        path,
        &[
            ("decision.provider", Edit::Unset),
            ("decision.base_url", Edit::Unset),
            ("decision.model", Edit::Unset),
            ("decision.api_key", Edit::Unset),
            ("decision.api_key_env", Edit::Unset),
        ],
    )
}

/// Ask the model one easy question; what to show the user.
pub fn try_model(d: &DecisionConfig) -> std::result::Result<String, String> {
    let decider = Decider::from_config(d)
        .map_err(|e| e.to_string())?
        .ok_or("choose a kind of model first")?;
    try_decider(&decider, &|| false)
}

/// Ask this model one easy question; what to show the user. `halted`
/// stops the wait (the stop key, the client's cancel).
pub fn try_decider(
    decider: &Decider,
    halted: &(dyn Fn() -> bool + Sync),
) -> std::result::Result<String, String> {
    let q = [Question::yes_no("sky", "Is the sky in this text blue?")];
    let (answers, took) = decider
        .ask("The sky over the sea is clear and blue today.", &q, halted)
        .map_err(|e| e.to_string())?;
    let a = answers.first().map(|(_, a)| a.brief()).unwrap_or_default();
    Ok(format!(
        "{} answered in {} ms (a test question, answer {a}).",
        decider.label(),
        took.as_millis()
    ))
}
