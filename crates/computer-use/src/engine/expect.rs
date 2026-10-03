//! `expect` on an action: waiting for what should follow and saying whether it was seen.

use super::*;

impl<B: Backend> Engine<B> {
    /// Wait (up to [timing] expect_wait_ms) for what an action was expected
    /// to bring, and say what was found: confirmed, not seen, or uncertain.
    pub(super) fn check_expect(
        &mut self,
        query: &str,
        what: &str,
        index: Option<u32>,
        before: Option<&Before>,
    ) -> (Outcome, String) {
        let timing = &self.store.config.timing;
        let wait = Duration::from_millis(timing.expect_wait_ms);
        let poll = Duration::from_millis(timing.settle_poll_ms.clamp(50, 1000) * 3);
        let start = (self.clock)();
        let mut fresh = self.settled != Some(self.epoch);
        let label = tree::truncate(what.trim().trim_matches('"'), 60);
        loop {
            let (mut outcome, mut detail, settled) =
                self.expect_now(query, what, index, before, fresh);
            let waited = (self.clock)().saturating_duration_since(start);
            if outcome == Outcome::Confirmed || settled || waited >= wait || self.halted() {
                // A text not there word for word may be there in other
                // words ("Saved" for "saved successfully"): the decision
                // model, when there is one, reads the window once. Without
                // one, "not seen" stands, and the agent looks.
                if outcome != Outcome::Confirmed
                    && !self.halted()
                    && let Some(yes) = self.expect_in_other_words(query, what)
                {
                    outcome = Outcome::Confirmed;
                    detail = format!("in other words, as the decision model reads it: {yes:.2}");
                }
                let detail = if detail.is_empty() {
                    String::new()
                } else {
                    format!(" ({detail})")
                };
                let note = match outcome {
                    Outcome::Confirmed => format!("Expected {label}: confirmed{detail}."),
                    Outcome::NotSeen => format!(
                        "Expected {label}: not seen{detail}{}. Look before doing it again.",
                        if waited.as_millis() >= 100 {
                            format!(" after {:.1} s", waited.as_secs_f64())
                        } else {
                            String::new()
                        }
                    ),
                    Outcome::Uncertain => format!(
                        "Expected {label}: uncertain{detail}. Don't repeat the action; look first."
                    ),
                };
                return (outcome, note);
            }
            (self.sleep)(poll);
            fresh = true;
        }
    }

    /// The probability (at least 0.7) that the window shows an expected
    /// text in other words, as the decision model reads it; `None` when it
    /// doesn't, when `what` isn't a text, or when no model may be asked.
    fn expect_in_other_words(&mut self, query: &str, what: &str) -> Option<f64> {
        use crate::decision::{Answer, Question, judge::Use};
        let text = expected_text(what)?;
        let decider = self.auto_decider()?;
        let window = self.window_text(query, None, false).ok()?;
        let q = Question::yes_no(
            "shows",
            &format!(
                "Does the window show this, in these words or in others that mean the same: \"{text}\"?"
            ),
        );
        let (answers, _) = self.judged_one(&decider, Use::Auto, &window, &[q]).ok()?;
        match answers.first() {
            Some((_, Answer::YesNo { yes })) if *yes >= 0.7 => Some(*yes),
            _ => None,
        }
    }

    /// Whether what was expected shows now, and whether waiting longer
    /// could change the answer.
    fn expect_now(
        &mut self,
        query: &str,
        what: &str,
        index: Option<u32>,
        before: Option<&Before>,
        fresh: bool,
    ) -> (Outcome, String, bool) {
        use Outcome::*;
        let kind = crate::text::fold(what.trim().trim_matches('"'));
        let kind = kind.trim();
        let gone = matches!(
            kind,
            "gone" | "goes" | "closed" | "close" | "closes" | "disappear" | "disappears"
        );
        let Ok(app) = self.resolve_app(query) else {
            return if gone {
                (Confirmed, "the app closed".into(), true)
            } else {
                (Uncertain, "the app can't be found now".into(), true)
            };
        };
        let window = match self.resolve_window(&app, None, fresh) {
            Ok(w) => w,
            Err(_) if gone => return (Confirmed, "its window closed".into(), true),
            Err(_) => return (Uncertain, "the app shows no window".into(), false),
        };
        if self.observe(&app, &window, fresh).is_err() {
            return (Uncertain, "the app couldn't be read after it".into(), false);
        }
        let target = index.or_else(|| self.ctx.target.filter(|(p, _)| *p == app.pid).map(|t| t.1));
        let fingerprint = self.tree_fingerprint(app.pid);
        let Some(st) = self.states.get(&app.pid).filter(|s| s.stamped) else {
            return (Uncertain, "the app couldn't be read after it".into(), false);
        };
        let unseen = || {
            (
                Uncertain,
                "the app wasn't looked at before it, so there's nothing to compare".to_string(),
                true,
            )
        };
        match kind {
            "dialog" | "window" | "new window" | "popup" | "pop-up" | "sheet" | "alert" => {
                let Some(b) = before else { return unseen() };
                let opened = st.window_id != b.window
                    || st.seen_windows.iter().any(|w| !b.windows.contains(w));
                if opened {
                    (Confirmed, format!("window \"{}\"", window.title), true)
                } else {
                    (NotSeen, "no other window came up".into(), false)
                }
            }
            "menu" | "a menu" => {
                let Some(b) = before else { return unseen() };
                let menus = |nodes: &[Node]| {
                    nodes
                        .iter()
                        .filter(|n| n.role == "menu" || n.role == "menu item")
                        .filter(|n| !n.states.hidden)
                        .count()
                };
                if menus(&st.nodes) > menus(&b.nodes) || st.window_id != b.window {
                    (Confirmed, String::new(), true)
                } else {
                    (NotSeen, "no menu came up".into(), false)
                }
            }
            "change" | "changes" | "a change" | "anything" => {
                let Some(b) = before else { return unseen() };
                if fingerprint != b.fingerprint {
                    (Confirmed, String::new(), true)
                } else {
                    (NotSeen, "nothing changed".into(), false)
                }
            }
            "value" | "new value" | "value changes" | "checked" | "selected" => {
                let Some(b) = before else { return unseen() };
                let Some(t) = target else {
                    return (Uncertain, "no element_index to watch".into(), true);
                };
                let old = b.nodes.iter().find(|n| n.index == t);
                match (old, st.nodes.iter().find(|n| n.index == t)) {
                    (_, None) => (Uncertain, "the element is gone".into(), true),
                    (None, Some(_)) => unseen(),
                    (Some(o), Some(n))
                        if o.value != n.value
                            || o.states.checked != n.states.checked
                            || o.states.selected != n.states.selected =>
                    {
                        (Confirmed, String::new(), true)
                    }
                    _ => (NotSeen, "its value is as it was".into(), false),
                }
            }
            _ if gone => {
                let Some(b) = before else { return unseen() };
                if b.window.is_some_and(|w| !st.seen_windows.contains(&w)) {
                    return (Confirmed, "its window closed".into(), true);
                }
                match target {
                    Some(t) if !st.nodes.iter().any(|n| n.index == t) => {
                        (Confirmed, String::new(), true)
                    }
                    Some(_) => (NotSeen, "it is still there".into(), false),
                    None if st.window_id != b.window => {
                        (Confirmed, "another window is in front".into(), true)
                    }
                    None => (NotSeen, "the window is still there".into(), false),
                }
            }
            text => {
                // A text that should be on screen.
                let text = text
                    .strip_prefix("text ")
                    .unwrap_or(text)
                    .trim()
                    .trim_matches('"')
                    .to_string();
                let found = st
                    .nodes
                    .iter()
                    .filter(|n| !crate::privacy::is_password(&n.role))
                    .find(|n| node_text(n).contains(&text));
                match found {
                    Some(n) => (
                        Confirmed,
                        format!("{} {}", n.index, tree::truncate(&n.line, 80)),
                        true,
                    ),
                    None if !st.blind.is_empty() || st.ocr_lines > 0 => (
                        Uncertain,
                        "the tree doesn't show it, and part of the window is only a picture".into(),
                        false,
                    ),
                    None => (NotSeen, "no element shows it".into(), false),
                }
            }
        }
    }
}

/// The text an `expect` waits for, when it waits for a text rather than
/// for one of the kinds `expect_now` knows (a dialog, a menu, a change, a
/// value, gone).
fn expected_text(what: &str) -> Option<String> {
    const KINDS: &[&str] = &[
        "gone",
        "goes",
        "closed",
        "close",
        "closes",
        "disappear",
        "disappears",
        "dialog",
        "window",
        "new window",
        "popup",
        "pop-up",
        "sheet",
        "alert",
        "menu",
        "a menu",
        "change",
        "changes",
        "a change",
        "anything",
        "value",
        "new value",
        "value changes",
        "checked",
        "selected",
    ];
    let raw = what.trim().trim_matches('"').trim();
    let folded = crate::text::fold(raw);
    if KINDS.contains(&folded.trim()) {
        return None;
    }
    let text = match raw.get(..5) {
        Some(p) if p.eq_ignore_ascii_case("text ") => &raw[5..],
        _ => raw,
    };
    let text = text.trim().trim_matches('"').trim();
    (!text.is_empty()).then(|| text.to_string())
}
