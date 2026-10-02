//! The `decide` tool and wait_for's `until`: typed questions for the
//! decision model (see [`crate::decision`]), about text, many items at
//! once, or an app's window. (computer-use, by mhrsdev)

use serde_json::{Value, json};

use super::*;
use crate::decision::{self, Answer, Decider, Kind, Question};

/// Elements offered as the options of one question when picking one.
const PICK_CHUNK: usize = 64;
/// The most elements a pick looks through.
const PICK_MAX: usize = 1024;
/// The most items judged in one call.
const MAX_ITEMS: usize = 500;
/// Longest description of an element offered to the model.
const OPTION_CHARS: usize = 200;

/// Text of a state given as JSON: a string as it is, anything else as JSON.
fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The questions a `decide` call asks.
fn questions_of(args: &DecideArgs) -> Result<Vec<Question>> {
    if let Some(qs) = &args.questions {
        if args.question.is_some() {
            return Err(Error::InvalidArgs(
                "give question (one) or questions (several), not both".into(),
            ));
        }
        return decision::questions_from(qs);
    }
    let Some(text) = args.question.as_deref() else {
        return Err(Error::InvalidArgs(
            "decide needs a question (with options for a choice, or scale for a score), or questions".into(),
        ));
    };
    let mut q = serde_json::Map::new();
    q.insert("question".into(), json!(text));
    match (&args.options, &args.scale) {
        (Some(_), Some(_)) => {
            return Err(Error::InvalidArgs(
                "give options (a choice) or scale (a score), not both".into(),
            ));
        }
        (Some(o), None) => {
            q.insert("type".into(), json!("choice"));
            q.insert("options".into(), o.clone());
        }
        (None, Some(s)) => {
            q.insert("type".into(), json!("score"));
            q.insert("scale".into(), s.clone());
        }
        (None, None) => {
            q.insert("type".into(), json!("yes_no"));
        }
    }
    Ok(vec![decision::question_from("answer", &Value::Object(q))?])
}

/// `a: yes (0.91) · b: billing (0.88)`, or just the answer for one
/// question named "answer".
fn answers_line(answers: &[(String, Answer)]) -> String {
    match answers {
        [(name, a)] if name == "answer" => a.brief(),
        _ => answers
            .iter()
            .map(|(n, a)| format!("{n}: {}", a.brief()))
            .collect::<Vec<_>>()
            .join(" · "),
    }
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    }
}

impl<B: Backend> Engine<B> {
    /// The settings key as the user knows it ("Ctrl+Alt+J"), if there is one.
    fn settings_key_name(&self) -> Option<String> {
        let k = self.store.config.control.settings_hotkey.trim();
        (!k.is_empty()).then(|| crate::overlay::helper::pretty_key(k))
    }

    /// What to tell the model when there is no decision model.
    fn not_set_up(&self) -> String {
        match self.settings_key_name() {
            Some(k) => decision::NOT_SET_UP.replace("Ctrl+Alt+J", &k),
            None => "no decision model is set up. decide setup=\"open\" opens a page in the user's browser where they add one (TypeSafe's Jev, or any OpenAI-compatible model) and its API key, which then never goes through the chat.".into(),
        }
    }

    pub(super) fn decider(&self) -> Result<Decider> {
        match Decider::from_config(&self.store.config.decision) {
            Ok(Some(d)) => Ok(d),
            Ok(None) => Err(Error::ActionFailed(self.not_set_up())),
            Err(e) => Err(Error::ActionFailed(format!(
                "the decision model's settings are incomplete: {e}"
            ))),
        }
    }

    /// The stop key and the client's cancel, as the model's wait can watch
    /// them from other threads.
    fn halt_watch(&self) -> impl Fn() -> bool + Send + Sync + 'static + use<B> {
        let (stop, cancel) = (self.stop.clone(), self.cancel.clone());
        move || stop.load(Ordering::SeqCst) || cancel.load(Ordering::SeqCst)
    }

    /// The app's window as text, for the decision model: its elements with
    /// their indices (the same ones the model acts on).
    fn window_text(&mut self, app: &str, window: Option<&str>, fresh: bool) -> Result<String> {
        let app = self.resolve_app(app)?;
        let window = self.resolve_window(&app, window, fresh)?;
        self.observe(&app, &window, fresh)?;
        let state = self.state(app.pid)?;
        let mut text = format!("{} · window \"{}\"\n", app.name, window.title);
        for n in &state.nodes {
            text.push_str(&" ".repeat(n.depth.min(24)));
            text.push_str(&format!("[{}] {}\n", n.index, n.line));
        }
        Ok(text)
    }

    pub(super) fn decide(&mut self, args: DecideArgs) -> Result<ToolOutput> {
        if let Some(setup) = args.setup.clone() {
            return self.decision_setup(setup);
        }
        let decider = self.decider()?;
        let halted = self.halt_watch();
        if let Some(what) = args.pick.as_deref() {
            let app = args.app.as_deref().ok_or_else(|| {
                Error::InvalidArgs("pick needs app: the app whose element to find".into())
            })?;
            return self.decide_pick(&decider, app, args.window.as_deref(), what);
        }
        let questions = questions_of(&args)?;
        let mut state = args.state.as_ref().map(text_of).unwrap_or_default();

        if let Some(items) = &args.items {
            if args.app.is_some() {
                return Err(Error::InvalidArgs(
                    "items are judged on their own: leave out app".into(),
                ));
            }
            if items.is_empty() || items.len() > MAX_ITEMS {
                return Err(Error::InvalidArgs(format!(
                    "items must hold 1 to {MAX_ITEMS} things to judge"
                )));
            }
            // A state with items is what they have in common.
            let texts: Vec<String> = items
                .iter()
                .map(|i| {
                    if state.trim().is_empty() {
                        text_of(i)
                    } else {
                        format!("{state}\n\n{}", text_of(i))
                    }
                })
                .collect();
            let start = Instant::now();
            let results = decider.ask_each(&texts, &questions, &halted);
            if self.halted() {
                return Err(self.stopped_error());
            }
            return Ok(ToolOutput::text(items_report(
                &decider,
                &questions,
                &results,
                start.elapsed(),
            )));
        }

        if let Some(app) = args.app.as_deref() {
            let window = self.window_text(app, args.window.as_deref(), false)?;
            if !state.trim().is_empty() {
                state.push_str("\n\n");
            }
            state.push_str(&window);
        }
        if state.trim().is_empty() {
            return Err(Error::InvalidArgs(
                "decide needs something to judge: state (text or JSON), items, or app (its window)"
                    .into(),
            ));
        }
        let (answers, took) = decider.ask(&state, &questions, &halted).map_err(|e| {
            if self.halted() {
                self.stopped_error()
            } else {
                e
            }
        })?;
        Ok(ToolOutput::text(format!(
            "Decision ({}, {} ms): {}",
            decider.label(),
            took.as_millis(),
            answers_line(&answers)
        )))
    }

    /// The element of an app's window that `what` describes.
    fn decide_pick(
        &mut self,
        decider: &Decider,
        app_query: &str,
        window: Option<&str>,
        what: &str,
    ) -> Result<ToolOutput> {
        let app = self.resolve_app(app_query)?;
        let win = self.resolve_window(&app, window, false)?;
        self.observe(&app, &win, false)?;
        let state = self.state(app.pid)?;
        // What can be told apart: elements with a name, a value or an action.
        let cands: Vec<(u32, String)> = state
            .nodes
            .iter()
            .filter(|n| {
                !n.states.hidden
                    && (crate::roles::is_interactive(&n.role)
                        || !n.actions.is_empty()
                        || n.name.as_deref().is_some_and(|s| !s.trim().is_empty())
                        || n.value.as_deref().is_some_and(|s| !s.trim().is_empty()))
            })
            .take(PICK_MAX)
            .map(|n| (n.index, cut(&n.line, OPTION_CHARS)))
            .collect();
        if cands.is_empty() {
            return Err(Error::ActionFailed(format!(
                "the window \"{}\" of {} has no elements to pick from; call get_app_state",
                win.title, app.name
            )));
        }
        let context = format!(
            "The window \"{}\" of {}. Each option is one of its elements: its number, then its role, name, value and state.",
            win.title, app.name
        );
        let ask = format!("Which element is this: {what}");
        let halted = self.halt_watch();
        let choice = |cands: &[(u32, String)], name: &str| Question {
            name: name.into(),
            kind: Kind::Choice,
            text: ask.clone(),
            options: cands
                .iter()
                .map(|(i, line)| (i.to_string(), line.clone()))
                .collect(),
        };
        let start = Instant::now();
        // Up to 64 at a time; with more, the best of each part go on to a
        // final round.
        let parts: Vec<&[(u32, String)]> = cands.chunks(PICK_CHUNK).collect();
        let mut finalists: Vec<(u32, String)> = Vec::new();
        let answer = if parts.len() == 1 {
            let (mut a, _) = decider.ask(&context, &[choice(parts[0], "element")], &halted)?;
            a.pop().map(|(_, a)| a)
        } else {
            let qs: Vec<Question> = parts
                .iter()
                .enumerate()
                .map(|(i, p)| choice(p, &format!("part{}", i + 1)))
                .collect();
            let (answers, _) = decider.ask(&context, &qs, &halted)?;
            for (_, a) in &answers {
                if let Answer::Choice { choice, .. } = a
                    && let Some(c) = cands.iter().find(|(i, _)| i.to_string() == *choice)
                {
                    finalists.push(c.clone());
                }
            }
            finalists.dedup();
            match finalists.len() {
                0 => None,
                1 => Some(Answer::Choice {
                    choice: finalists[0].0.to_string(),
                    confidence: None,
                    probabilities: Vec::new(),
                }),
                _ => {
                    let (mut a, _) =
                        decider.ask(&context, &[choice(&finalists, "element")], &halted)?;
                    a.pop().map(|(_, a)| a)
                }
            }
        };
        let Some(Answer::Choice {
            choice,
            confidence,
            probabilities,
        }) = answer
        else {
            return Err(Error::ActionFailed(
                "the decision model picked no element".into(),
            ));
        };
        let line = |i: &str| {
            cands
                .iter()
                .find(|(n, _)| n.to_string() == i)
                .map(|(_, l)| l.clone())
                .unwrap_or_default()
        };
        let p = probabilities
            .iter()
            .find(|(o, _)| *o == choice)
            .map(|(_, p)| *p)
            .or(confidence);
        let mut out = format!(
            "Element {choice}: {}{} ({}, {} ms)",
            line(&choice),
            p.map(|p| format!(" — {p:.2}")).unwrap_or_default(),
            decider.label(),
            start.elapsed().as_millis()
        );
        let others: Vec<String> = probabilities
            .iter()
            .filter(|(o, p)| *o != choice && *p >= 0.1)
            .take(2)
            .map(|(o, p)| format!("{o}: {} — {p:.2}", line(o)))
            .collect();
        if !others.is_empty() {
            out.push_str(&format!("\nAlso possible: {}", others.join("; ")));
        }
        if p.is_some_and(|p| p < 0.4) {
            out.push_str("\nThe model is unsure: check the element before acting on it.");
        }
        Ok(ToolOutput::text(out))
    }

    /// `wait_for` with `until`: ask the decision model about the window
    /// until it says yes. `Ok(Some(answer))` once it does.
    pub(super) fn until_yes(
        &mut self,
        decider: &Decider,
        question: &str,
        app: &str,
        window: Option<&str>,
    ) -> Result<Option<f64>> {
        let text = self.window_text(app, window, true)?;
        let halted = self.halt_watch();
        let (answers, _) = decider.ask(&text, &[Question::yes_no("answer", question)], &halted)?;
        Ok(match answers.first() {
            Some((_, Answer::YesNo { yes })) if *yes >= 0.5 => Some(*yes),
            _ => None,
        })
    }

    /// `decide setup=…`: see, open, test, remove or set the decision model.
    fn decision_setup(&mut self, setup: Value) -> Result<ToolOutput> {
        let key = self.settings_key_name();
        match setup {
            Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                "" | "status" => Ok(ToolOutput::text(self.decision_status())),
                "open" | "page" | "settings" => {
                    let (url, browser) = decision::page::open(self.store.path.clone())?;
                    Ok(ToolOutput::text(match browser {
                        Ok(()) => format!(
                            "Opened the decision model's settings page in the user's browser ({url}). Ask them to choose the model, paste its API key and press Save there; it is used at once.{}",
                            key.map(|k| format!(" ({k} opens it too.)"))
                                .unwrap_or_default()
                        ),
                        Err(e) => format!(
                            "Couldn't open the browser ({e}). Ask the user to open {url} (on this computer) to set up the decision model."
                        ),
                    }))
                }
                "test" => {
                    let d = self.store.config.decision.clone();
                    if d.provider.trim().is_empty() {
                        return Err(Error::ActionFailed(self.not_set_up()));
                    }
                    decision::page::try_model(&d)
                        .map(ToolOutput::text)
                        .map_err(Error::ActionFailed)
                }
                "remove" | "clear" | "delete" => {
                    match self.store.path.clone() {
                        Some(path) => decision::page::remove_settings(&path)?,
                        None => {}
                    }
                    self.store.config.decision = crate::config::DecisionConfig::default();
                    self.reload_now();
                    Ok(ToolOutput::text("Removed the decision model and its key."))
                }
                other => Err(Error::InvalidArgs(format!(
                    "setup is \"status\", \"open\", \"test\", \"remove\", or the settings {{provider, base_url, model, api_key}} (got \"{other}\")"
                ))),
            },
            Value::Object(m) => self.decision_set(&m),
            other => Err(Error::InvalidArgs(format!(
                "setup is \"status\", \"open\", \"test\", \"remove\", or the settings {{provider, base_url, model, api_key}} (got {other})"
            ))),
        }
    }

    fn decision_status(&self) -> String {
        let d = &self.store.config.decision;
        let key = self.settings_key_name();
        let how = match &key {
            Some(k) => format!("The user changes it on the page {k} opens."),
            None => "decide setup=\"open\" opens its settings page.".into(),
        };
        match Decider::from_config(d) {
            Ok(Some(dec)) => {
                let source = if !d.api_key.trim().is_empty() {
                    format!("key {}", crate::config::masked_key(&d.api_key))
                } else if !d.api_key_env.trim().is_empty() {
                    format!("key from ${}", d.api_key_env.trim())
                } else {
                    "no key".into()
                };
                format!("Decision model: {} ({source}). {how}", dec.label())
            }
            Ok(None) => format!("No decision model is set up. {}", self.not_set_up()),
            Err(e) => format!("The decision model's settings are incomplete: {e}. {how}"),
        }
    }

    /// Settings the user gave in the chat.
    fn decision_set(&mut self, m: &serde_json::Map<String, Value>) -> Result<ToolOutput> {
        const KEYS: [&str; 5] = ["provider", "base_url", "model", "api_key", "api_key_env"];
        if let Some(k) = m.keys().find(|k| !KEYS.contains(&k.as_str())) {
            return Err(Error::InvalidArgs(format!(
                "unknown setup field \"{k}\" (provider, base_url, model, api_key, api_key_env)"
            )));
        }
        let field = |k: &str| {
            m.get(k)
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
        };
        let mut d = self.store.config.decision.clone();
        if let Some(p) = field("provider") {
            let p = decision::Provider::parse(&p).ok_or_else(|| {
                Error::InvalidArgs(format!(
                    "provider is \"jev\" (TypeSafe's System One API, or a server speaking it) or \"openai\" (any OpenAI-compatible chat API), not \"{p}\""
                ))
            })?;
            if p.id() != d.provider {
                d.base_url.clear();
                d.model.clear();
            }
            d.provider = p.id().into();
        }
        if d.provider.is_empty() {
            return Err(Error::InvalidArgs(
                "say which kind of model: provider \"jev\" or \"openai\"".into(),
            ));
        }
        if let Some(u) = field("base_url") {
            d.base_url = u;
        }
        if let Some(v) = field("model") {
            d.model = v;
        }
        if let Some(k) = field("api_key") {
            d.api_key = k;
            d.api_key_env.clear();
        }
        if let Some(e) = field("api_key_env") {
            d.api_key_env = e;
            d.api_key.clear();
        }
        let mut whole = self.store.config.clone();
        whole.decision = d.clone();
        whole.validate().map_err(Error::InvalidArgs)?;
        Decider::from_config(&d).map_err(|e| Error::InvalidArgs(e.to_string()))?;
        let saved = match self.store.path.clone() {
            Some(path) => {
                decision::page::save_settings(&path, &d)?;
                format!("saved in {}", path.display())
            }
            None => "kept for this session".into(),
        };
        self.store.config.decision = d.clone();
        self.reload_now();
        let label = Decider::from_config(&d)
            .ok()
            .flatten()
            .map(|x| x.label())
            .unwrap_or_default();
        let test = match decision::page::try_model(&d) {
            Ok(t) => format!("Test: {t}"),
            Err(e) => format!("But the test failed: {e}"),
        };
        Ok(ToolOutput::text(format!(
            "Decision model set: {label} (key {}; {saved}). {test}{}",
            if d.api_key.is_empty() {
                "none".to_string()
            } else {
                crate::config::masked_key(&d.api_key)
            },
            match self.settings_key_name() {
                Some(k) => format!(
                    "\nKeys typed in the chat stay in its history: next time the user can set it on the page {k} opens instead."
                ),
                None => String::new(),
            }
        )))
    }

    /// Read the settings file again now (after the decision model's
    /// settings were written).
    fn reload_now(&mut self) {
        let Some(path) = self.store.path.clone() else {
            return;
        };
        match ConfigStore::load(Some(&path)) {
            Ok(store) => {
                self.set_config(store);
                self.config_mtime = file_mtime(&path);
            }
            Err(e) => log::warn!("couldn't read the settings again: {e}"),
        }
    }
}

/// The answers for each item, and a summary.
fn items_report(
    decider: &Decider,
    questions: &[Question],
    results: &[Result<(Vec<(String, Answer)>, Duration)>],
    took: Duration,
) -> String {
    let mut out = format!(
        "Decisions ({}, {} items, {} ms):\n",
        decider.label(),
        results.len(),
        took.as_millis()
    );
    let mut failed = 0;
    for (i, r) in results.iter().enumerate() {
        match r {
            Ok((answers, _)) => out.push_str(&format!("{}. {}\n", i + 1, answers_line(answers))),
            Err(e) => {
                failed += 1;
                out.push_str(&format!("{}. failed: {e}\n", i + 1));
            }
        }
    }
    // One question: a summary over the items.
    if let [q] = questions {
        let answers: Vec<(usize, &Answer)> = results
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                r.as_ref()
                    .ok()
                    .and_then(|(a, _)| a.first())
                    .map(|(_, a)| (i + 1, a))
            })
            .collect();
        match q.kind {
            Kind::YesNo => {
                let yes: Vec<String> = answers
                    .iter()
                    .filter(|(_, a)| a.is_yes())
                    .map(|(i, _)| i.to_string())
                    .collect();
                out.push_str(&format!(
                    "Yes for {} of {}{}",
                    yes.len(),
                    answers.len(),
                    if yes.is_empty() || yes.len() == answers.len() {
                        ".".to_string()
                    } else {
                        format!(": {}.", yes.join(", "))
                    }
                ));
            }
            Kind::Choice => {
                let mut counts: Vec<(String, usize)> = Vec::new();
                for (_, a) in &answers {
                    if let Answer::Choice { choice, .. } = a {
                        match counts.iter_mut().find(|(c, _)| c == choice) {
                            Some((_, n)) => *n += 1,
                            None => counts.push((choice.clone(), 1)),
                        }
                    }
                }
                counts.sort_by(|a, b| b.1.cmp(&a.1));
                let parts: Vec<String> = counts.iter().map(|(c, n)| format!("{c} {n}")).collect();
                out.push_str(&format!("Counts: {}.", parts.join(", ")));
            }
            Kind::Score => {
                let mut scored: Vec<(usize, f64)> = answers
                    .iter()
                    .filter_map(|(i, a)| match a {
                        Answer::Score { score, .. } => Some((*i, *score)),
                        _ => None,
                    })
                    .collect();
                if !scored.is_empty() {
                    let mean = scored.iter().map(|(_, s)| s).sum::<f64>() / scored.len() as f64;
                    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
                    let top: Vec<String> = scored
                        .iter()
                        .take(3)
                        .map(|(i, s)| format!("{i} ({s:.2})"))
                        .collect();
                    out.push_str(&format!("Mean {mean:.2}; highest: {}.", top.join(", ")));
                }
            }
        }
    }
    if failed > 0 {
        out.push_str(&format!("\n{failed} item(s) failed."));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::config::{Config, DecisionConfig};
    use crate::decision::tests::fake;
    use crate::mock::MockBackend;

    fn engine(decision: DecisionConfig) -> Engine<MockBackend> {
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let cfg = Config {
            decision,
            ..Config::default()
        };
        Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {})
    }

    fn jev(url: &str) -> DecisionConfig {
        DecisionConfig {
            provider: "jev".into(),
            base_url: url.into(),
            api_key: "sk-test-key-9876".into(),
            ..DecisionConfig::default()
        }
    }

    /// A System One server that answers each question by `f(state,
    /// question)`.
    fn system_one(
        f: impl Fn(&str, &serde_json::Map<String, Value>) -> Value + Send + Sync + 'static,
    ) -> crate::decision::tests::Fake {
        fake(move |body| {
            let v: Value = serde_json::from_str(body).unwrap();
            let state = v["state"].as_str().unwrap_or_default().to_string();
            let mut answers = serde_json::Map::new();
            for (name, q) in v["questions"].as_object().unwrap() {
                answers.insert(name.clone(), f(&state, q.as_object().unwrap()));
            }
            (200, json!({"answers": answers}).to_string())
        })
    }

    #[test]
    fn without_a_model_the_user_is_sent_to_the_settings_key() {
        let mut e = engine(DecisionConfig::default());
        let out = e.call_tool("decide", json!({"question": "Is it?", "state": "x"}));
        assert!(out.is_error);
        assert!(out.text.contains("Ctrl+Alt+J"), "{}", out.text);
        let status = e.call_tool("decide", json!({"setup": "status"}));
        assert!(status.text.contains("No decision model"), "{}", status.text);
    }

    #[test]
    fn a_question_about_text_and_about_a_window() {
        let f = system_one(|state, q| {
            let yes = if q["instructions"].as_str().unwrap().contains("bold") {
                // The window's elements arrive as text, with their indices.
                if state.contains("button \"Bold\"") && state.contains("TextEdit") {
                    0.97
                } else {
                    0.0
                }
            } else if state.contains("blue") {
                0.9
            } else {
                0.1
            };
            json!({"type": "noul", "noul": yes})
        });
        let mut e = engine(jev(&f.url));
        let out = e.call_tool(
            "decide",
            json!({"question": "Is the sky blue?", "state": "a blue sky"}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("yes (0.90)"), "{}", out.text);
        let out = e.call_tool(
            "decide",
            json!({"question": "Is there a bold button?", "app": "TextEdit"}),
        );
        assert!(out.text.contains("yes (0.97)"), "{}", out.text);
        assert!(!out.text.contains("sk-test"), "the key leaked");
    }

    #[test]
    fn items_are_judged_each_and_summed_up() {
        let f = system_one(|state, q| match q["type"].as_str().unwrap() {
            "score" => {
                json!({"type": "score", "score": if state.contains("great") { 2.0 } else { 0.0 }, "confidence": 0.9})
            }
            _ => json!({"type": "noul", "noul": if state.contains("great") { 0.95 } else { 0.05 }}),
        });
        let mut e = engine(jev(&f.url));
        let out = e.call_tool(
            "decide",
            json!({"question": "Is the review positive?", "items": ["great sound", "broke in a week", "great value"]}),
        );
        assert!(
            out.text.contains("1. yes (0.95)") && out.text.contains("2. no (0.05)"),
            "{}",
            out.text
        );
        assert!(out.text.contains("Yes for 2 of 3: 1, 3."), "{}", out.text);
        let out = e.call_tool(
            "decide",
            json!({"question": "How happy?", "scale": ["unhappy", "fine", "happy"], "items": ["great", "bad"]}),
        );
        assert!(
            out.text.contains("2.00 → happy") && out.text.contains("highest: 1 (2.00)"),
            "{}",
            out.text
        );
    }

    #[test]
    fn pick_finds_the_element_a_description_means() {
        let f = system_one(|_, q| {
            // The options are element indices, described by their lines.
            let crit = q["criteria"].as_object().unwrap();
            let (idx, _) = crit
                .iter()
                .find(|(_, line)| line.as_str().unwrap().contains("Bold"))
                .unwrap();
            json!({"type": "choice", "choice": idx, "confidence": 0.9, "probabilities": {idx.clone(): 0.92}})
        });
        let mut e = engine(jev(&f.url));
        let out = e.call_tool(
            "decide",
            json!({"app": "TextEdit", "pick": "the button that makes text thick"}),
        );
        assert!(!out.is_error, "{}", out.text);
        let index: u32 = out.text["Element ".len()..]
            .split(':')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        // The index is one the model can act on at once.
        let click = e.call_tool("click", json!({"app": "TextEdit", "element_index": index}));
        assert!(!click.is_error, "{}", click.text);
        assert!(out.text.contains("Bold"), "{}", out.text);
    }

    #[test]
    fn wait_for_until_asks_until_yes() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = calls.clone();
        let f = system_one(move |_, _| {
            let n = c.fetch_add(1, Ordering::SeqCst);
            json!({"type": "noul", "noul": if n >= 2 { 0.8 } else { 0.2 }})
        });
        let mut e = engine(jev(&f.url));
        let out = e.call_tool(
            "wait_for",
            json!({"app": "TextEdit", "until": "Has the document loaded?", "poll_ms": 20, "timeout_ms": 5000}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.starts_with("Yes after"), "{}", out.text);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn setup_from_the_chat_never_echoes_the_key() {
        let f = system_one(|_, _| json!({"type": "noul", "noul": 1.0}));
        let mut e = engine(DecisionConfig::default());
        let out = e.call_tool(
            "decide",
            json!({"setup": {"provider": "jev", "base_url": f.url, "api_key": "sk-chat-secret-4321"}}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.contains("••••4321") && !out.text.contains("sk-chat"),
            "{}",
            out.text
        );
        assert!(out.text.contains("answered in"), "{}", out.text);
        let status = e.call_tool("decide", json!({"setup": "status"}));
        assert!(
            status.text.contains("jev-latest") && !status.text.contains("sk-chat"),
            "{}",
            status.text
        );
        let bad = e.call_tool("decide", json!({"setup": {"provider": "skynet"}}));
        assert!(bad.is_error);
        let removed = e.call_tool("decide", json!({"setup": "remove"}));
        assert!(!removed.is_error, "{}", removed.text);
        assert!(
            e.call_tool("decide", json!({"question": "?", "state": "x"}))
                .is_error
        );
    }

    #[test]
    fn scripts_ask_choose_and_score() {
        let f = system_one(|state, q| match q["type"].as_str().unwrap() {
            "noul" => {
                json!({"type": "noul", "noul": if state.contains("good") { 0.9 } else { 0.1 }})
            }
            "choice" => {
                json!({"type": "choice", "choice": "bug", "probabilities": {"bug": 0.8, "idea": 0.2}})
            }
            _ => json!({"type": "score", "score": 1.0}),
        });
        let mut e = engine(jev(&f.url));
        let out = e.call_tool(
            "script",
            json!({"code": r#"
                let p = ask("good stuff", "Is it good?");
                let c = choose("it crashes", "What is it?", ["bug", "idea"]);
                let s = score("meh", "How good?", ["bad", "ok", "great"]);
                let all = decide_each(["good", "bad"], #{ok: #{question: "good?"}});
                print(`${p > 0.5} ${c} ${s} ${all[0].ok.answer} ${all[1].ok.answer}`);
            "#}),
        );
        assert!(out.text.contains("true bug 1.0 true false"), "{}", out.text);
    }
}
