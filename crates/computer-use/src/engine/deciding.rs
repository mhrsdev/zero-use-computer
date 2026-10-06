//! The `decide` tool and wait_for's `until`: typed questions for the
//! decision model (see [`crate::decision`]), about text, many items at
//! once, or an app's window. (computer-use, by mhrsdev)

use serde_json::{Value, json};

use super::*;
use crate::decision::judge::Use;
use crate::decision::{self, Answer, Asked, Decider, Kind, Question};

/// Elements offered as the options of one question when picking one.
const PICK_CHUNK: usize = 64;
/// The most elements a pick looks through.
const PICK_MAX: usize = 1024;
/// The most items judged in one call.
pub(crate) const MAX_ITEMS: usize = 500;
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

/// The parts of a window as few requests as they fit in: each request's
/// state holds several parts, with one yes/no question each ("does part
/// 3 have to do with …?"). Each request, and which part each of its
/// questions is about.
#[allow(clippy::type_complexity)]
fn pack_parts(
    texts: &[String],
    about: &str,
    title: &str,
    app: &str,
    max_state: usize,
) -> (Vec<(String, Vec<Question>)>, Vec<Vec<(usize, String)>>) {
    let head = format!(
        "Parts of the window \"{title}\" of {app}, each with its elements (role, name, value):\n\n"
    );
    // Room left for the parts (a little kept back, as the state is cut
    // at max_state characters); with very little, a part a request.
    let room = max_state.saturating_sub(head.len() + 200).max(1);
    let mut jobs = Vec::new();
    let mut slots = Vec::new();
    let mut state = head.clone();
    let mut qs: Vec<Question> = Vec::new();
    let mut names: Vec<(usize, String)> = Vec::new();
    for (k, text) in texts.iter().enumerate() {
        let part = format!("[part {}]\n{}\n", k + 1, text.trim_end());
        let full = qs.len() >= decision::MAX_QUESTIONS
            || (!qs.is_empty() && state.len() - head.len() + part.len() > room);
        if full {
            jobs.push((
                std::mem::replace(&mut state, head.clone()),
                std::mem::take(&mut qs),
            ));
            slots.push(std::mem::take(&mut names));
        }
        state.push_str(&part);
        let name = format!("part{}", k + 1);
        qs.push(Question::yes_no(
            &name,
            &format!("Does part {} have to do with: {about}?", k + 1),
        ));
        names.push((k, name));
    }
    if !qs.is_empty() {
        jobs.push((state, qs));
        slots.push(names);
    }
    (jobs, slots)
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
        decision::not_set_up(self.settings_key_name().as_deref())
    }

    /// The settings file, when hot reload is off and the file changed
    /// since it was last read (an empty one is taken as half-written).
    fn changed_without_hot_reload(&self) -> Option<std::path::PathBuf> {
        let path = self.store.path.as_deref()?;
        (!self.store.config.hot_reload
            && file_mtime(path) != self.config_mtime
            && std::fs::metadata(path).is_ok_and(|m| m.len() > 0))
        .then(|| path.to_path_buf())
    }

    /// The decision model's settings as they are now. Without hot reload,
    /// settings saved on the page since the file was last read are read
    /// from it.
    pub(super) fn decision_settings(&self) -> crate::config::DecisionConfig {
        self.changed_without_hot_reload()
            .and_then(|p| ConfigStore::load(Some(&p)).ok())
            .map(|s| s.config.decision)
            .unwrap_or_else(|| self.store.config.decision.clone())
    }

    /// Without hot reload, take up the decision model saved on the page
    /// (only the file's `[decision]` settings), so the tool list,
    /// `decision.auto` and the model's record follow it. Before each call.
    pub(super) fn sync_decision_settings(&mut self) {
        let Some(path) = self.changed_without_hot_reload() else {
            return;
        };
        self.config_mtime = file_mtime(&path);
        let Ok(store) = ConfigStore::load(Some(&path)) else {
            return;
        };
        if store.config.decision != self.store.config.decision {
            // Another model: the old one's failures and answers aren't its.
            self.judge.forget();
            self.store.config.decision = store.config.decision;
        }
    }

    pub(super) fn decider(&self) -> Result<Decider> {
        match Decider::from_config(&self.decision_settings()) {
            Ok(Some(d)) => Ok(d.with_settings_key(self.settings_key_name())),
            Ok(None) => Err(Error::ActionFailed(self.not_set_up())),
            Err(e) => Err(Error::ActionFailed(format!(
                "the decision model's settings are incomplete: {e}"
            ))),
        }
    }

    /// The decision model for the server's own questions, when one may be
    /// asked now: set up, `decision.auto` on, and not resting after
    /// failing. `None`: the server judges by itself.
    pub(super) fn auto_decider(&self) -> Option<Decider> {
        if !self.store.config.decision.auto {
            return None;
        }
        let d = self.decider().ok()?;
        self.judge.ready((self.clock)()).then_some(d)
    }

    /// Questions through the decision layer (its cache, its time limits,
    /// its counts).
    pub(super) fn judged(
        &mut self,
        decider: &Decider,
        use_: Use,
        jobs: &[(String, Vec<Question>)],
    ) -> Vec<Asked> {
        let now = (self.clock)();
        let halted = self.halt_watch();
        let settings = self.store.config.decision.clone();
        self.judge
            .ask_batch(decider, &settings, use_, jobs, now, &halted)
    }

    /// One state's questions through the decision layer.
    pub(super) fn judged_one(
        &mut self,
        decider: &Decider,
        use_: Use,
        state: &str,
        questions: &[Question],
    ) -> Asked {
        self.judged(decider, use_, &[(state.to_string(), questions.to_vec())])
            .pop()
            .unwrap_or_else(|| Err(Error::Internal("no answer".into())))
    }

    /// The tools (of `cands`) that a `find_tools` query means, as the
    /// decision model reads it: the likely ones, most likely first, at
    /// most three. `None` without a model, or when it can't tell.
    pub(super) fn tools_by_model(
        &mut self,
        query: &str,
        cands: &[&crate::tools::ToolDefinition],
    ) -> Option<Vec<usize>> {
        if cands.len() < 2 || cands.len() > decision::MAX_OPTIONS {
            return None;
        }
        let decider = self.auto_decider()?;
        let q = Question {
            name: "tool".into(),
            kind: Kind::Choice,
            text: format!("Which tool is for this: {query}"),
            options: cands
                .iter()
                .map(|t| (t.name.to_string(), cut(&t.description, OPTION_CHARS)))
                .collect(),
        };
        let state = "The tools of a desktop-control server that an agent can ask for, each with what it does.";
        let (answers, _) = self.judged_one(&decider, Use::Auto, state, &[q]).ok()?;
        let Some((
            _,
            Answer::Choice {
                choice,
                probabilities,
                ..
            },
        )) = answers.first()
        else {
            return None;
        };
        let position = |name: &str| cands.iter().position(|t| t.name == name);
        let mut picked: Vec<usize> = probabilities
            .iter()
            .filter(|(_, p)| *p >= 0.15)
            .filter_map(|(o, _)| position(o))
            .take(3)
            .collect();
        if picked.is_empty() {
            picked.extend(position(choice));
        }
        (!picked.is_empty()).then_some(picked)
    }

    /// The stop key and the client's cancel, as the model's wait can watch
    /// them from other threads.
    fn halt_watch(&self) -> impl Fn() -> bool + Send + Sync + 'static + use<B> {
        let (stop, cancel) = (self.stop.clone(), self.cancel.clone());
        move || stop.load(Ordering::SeqCst) || cancel.load(Ordering::SeqCst)
    }

    /// The app's window as text, for the decision model: its elements with
    /// their indices (the same ones the model acts on).
    pub(super) fn window_text(
        &mut self,
        app: &str,
        window: Option<&str>,
        fresh: bool,
    ) -> Result<String> {
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
        self.sync_decision_settings();
        if let Some(setup) = args.setup.clone() {
            return self.decision_setup(setup);
        }
        let decider = self.decider()?;
        if let Some(what) = args.pick.as_deref() {
            let app = args.app.as_deref().ok_or_else(|| {
                Error::InvalidArgs("pick needs app: the app whose element to find".into())
            })?;
            return self.decide_pick(&decider, app, args.window.as_deref(), what, args.read);
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
            let jobs: Vec<(String, Vec<Question>)> =
                texts.into_iter().map(|t| (t, questions.clone())).collect();
            let results = self.judged(&decider, Use::Asked, &jobs);
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
        let (answers, took) = self
            .judged_one(&decider, Use::Asked, &state, &questions)
            .map_err(|e| {
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

    /// get_app_state(about): the parts of the window that have to do with
    /// `about`, the others folded to a line each. Which parts: the
    /// decision model's judgment when one is set up, else the words of
    /// `about` found in them. Like `within`, a look that doesn't change
    /// what later diffs are against.
    pub(super) fn about_parts(
        &mut self,
        app: &AppInfo,
        window: &WindowInfo,
        about: &str,
        max_tokens: Option<usize>,
    ) -> Result<ToolOutput> {
        let st = self.state(app.pid)?;
        let nodes = st.nodes.clone();
        let parts = window_parts(&nodes);
        let size = |p: usize| {
            1 + nodes[p + 1..]
                .iter()
                .take_while(|n| n.depth > nodes[p].depth)
                .count()
        };
        let texts: Vec<String> = parts
            .iter()
            .map(|&p| {
                let mut t = String::new();
                for n in &nodes[p..p + size(p)] {
                    t.push_str(&n.line);
                    t.push('\n');
                    if t.len() > 1500 {
                        break;
                    }
                }
                t
            })
            .collect();
        // The words of `about` found in a part: always there, free.
        let words: Vec<String> = crate::text::fold(about)
            .split_whitespace()
            .filter(|w| w.chars().count() >= 3)
            .map(str::to_string)
            .collect();
        let mut keep: Vec<bool> = parts
            .iter()
            .map(|&p| {
                // Names and values, not roles ("text" isn't every field).
                nodes[p..p + size(p)].iter().any(|n| {
                    let t = node_text(n);
                    words.iter().any(|w| t.contains(w.as_str()))
                })
            })
            .collect();
        let mut how = "words matched".to_string();
        // Better with the decision model: it knows "shipping address"
        // when the part says "Deliver to". All the parts go in one request
        // (as many as fit its state), not one each.
        if parts.len() > 1
            && let Some(decider) = self.auto_decider()
        {
            let max = self.store.config.decision.max_state_chars;
            let (jobs, slots) = pack_parts(&texts, about, &window.title, &app.name, max);
            let replies = self.judged(&decider, Use::Auto, &jobs);
            if self.halted() {
                return Err(self.stopped_error());
            }
            let mut answered = 0;
            for (reply, names) in replies.iter().zip(&slots) {
                let Ok((answers, _)) = reply else { continue };
                for (k, name) in names {
                    if let Some((_, Answer::YesNo { yes })) =
                        answers.iter().find(|(n, _)| n == name)
                    {
                        keep[*k] = *yes >= 0.3;
                        answered += 1;
                    }
                }
            }
            if answered > 0 {
                how = decider.label();
            }
        }
        // The focused element's part always stays.
        for (k, &p) in parts.iter().enumerate() {
            if nodes[p..p + size(p)].iter().any(|n| n.states.focused) {
                keep[k] = true;
            }
        }
        let mut note = String::new();
        if !keep.iter().any(|k| *k) {
            keep.iter_mut().for_each(|k| *k = true);
            note = " (no part matched, so all of them are here)".into();
        }
        // The kept nodes, with each left-out part folded into its first line.
        let mut hidden = vec![false; nodes.len()];
        let mut folded: HashMap<usize, usize> = HashMap::new();
        for (k, &p) in parts.iter().enumerate() {
            if !keep[k] {
                let n = size(p);
                hidden[p + 1..p + n].iter_mut().for_each(|h| *h = true);
                folded.insert(p, n - 1);
            }
        }
        let mut map: HashMap<usize, usize> = HashMap::new();
        let mut shown: Vec<Node> = Vec::new();
        for (pos, n) in nodes.iter().enumerate() {
            if hidden[pos] {
                continue;
            }
            map.insert(pos, shown.len());
            let mut n = n.clone();
            n.parent = n.parent.and_then(|p| map.get(&p).copied());
            if let Some(inside) = folded.get(&pos).filter(|i| **i > 0) {
                n.line.push_str(&format!(
                    " [{inside} inside, about something else: within={} shows them]",
                    n.index
                ));
            }
            shown.push(n);
        }
        let tcfg = &self.store.config.tree;
        let mut budget = tree::Budget::from_config(tcfg);
        if let Some(tokens) = max_tokens {
            budget.tokens = tokens;
        }
        let text = tree::render_full_within(&shown, tcfg.indent, budget);
        let kept = keep.iter().filter(|k| **k).count();
        Ok(ToolOutput::text(format!(
            "App: {} · window \"{}\" · the parts about \"{}\": {kept} of {} ({how}){note}:\n{text}",
            app.name,
            window.title,
            tree::truncate(about, 60),
            parts.len()
        )))
    }

    /// The element of an app's window that `what` describes.
    fn decide_pick(
        &mut self,
        decider: &Decider,
        app_query: &str,
        window: Option<&str>,
        what: &str,
        read: bool,
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
        let answer = if let [(only, _)] = cands.as_slice() {
            // Nothing to choose between (a choice needs two options).
            Some(Answer::Choice {
                choice: only.to_string(),
                confidence: None,
                probabilities: Vec::new(),
            })
        } else if parts.len() == 1 {
            let (mut a, _) = self.judged_one(
                decider,
                Use::Asked,
                &context,
                &[choice(parts[0], "element")],
            )?;
            a.pop().map(|(_, a)| a)
        } else {
            // A last part of one element is no choice: it goes on as it is.
            let qs: Vec<Question> = parts
                .iter()
                .enumerate()
                .filter(|(_, p)| p.len() > 1)
                .map(|(i, p)| choice(p, &format!("part{}", i + 1)))
                .collect();
            let (answers, _) = self.judged_one(decider, Use::Asked, &context, &qs)?;
            for (_, a) in &answers {
                if let Answer::Choice { choice, .. } = a
                    && let Some(c) = cands.iter().find(|(i, _)| i.to_string() == *choice)
                {
                    finalists.push(c.clone());
                }
            }
            if !finalists.is_empty() {
                finalists.extend(parts.iter().filter(|p| p.len() == 1).map(|p| p[0].clone()));
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
                    let (mut a, _) = self.judged_one(
                        decider,
                        Use::Asked,
                        &context,
                        &[choice(&finalists, "element")],
                    )?;
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
        // Extraction: the element's whole text, read by the server (the
        // model needn't read the window for it).
        if read
            && let Ok(index) = choice.parse::<u32>()
            && let Ok(n) = self.node_by_index(&app, index)
        {
            let text = if crate::privacy::is_password(&n.role) {
                "(masked: a password field)".to_string()
            } else {
                match (&n.name, &n.value) {
                    (_, Some(v)) if !v.trim().is_empty() => v.clone(),
                    (Some(name), _) => name.clone(),
                    _ => String::new(),
                }
            };
            out.push_str(&format!("\nIt reads: \"{}\"", tree::truncate(&text, 4000)));
        }
        Ok(ToolOutput::text(out))
    }

    /// `wait_for` with `until`: ask the decision model about the window
    /// until it says yes. `Ok(Some(answer))` once it does.
    /// The decision model's yes (its probability) to `question` about the
    /// app's window, or `None`. The window is the one just looked at
    /// (wait_for looks before asking). `asked` holds a hash of the text a
    /// "no" was last given about, and when: the same text is asked about
    /// again only after [`REASK`].
    pub(super) fn until_yes(
        &mut self,
        decider: &Decider,
        question: &str,
        app: &str,
        window: Option<&str>,
        asked: &mut Option<(u64, Instant)>,
    ) -> Result<Option<f64>> {
        /// How long a "no" about the same text stands.
        const REASK: Duration = Duration::from_secs(5);
        let text = self.window_text(app, window, false)?;
        let hash = super::text_hash(&text);
        let now = (self.clock)();
        if let Some((h, at)) = *asked
            && h == hash
            && now.saturating_duration_since(at) < REASK
        {
            return Ok(None);
        }
        let (answers, _) = self.judged_one(
            decider,
            Use::Asked,
            &text,
            &[Question::yes_no("answer", question)],
        )?;
        *asked = Some((hash, now));
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
                    // Never its address: it carries the page's secret, and
                    // the page is the user's alone.
                    let (_, browser) = decision::page::open(self.store.path.clone())?;
                    Ok(ToolOutput::text(match browser {
                        Ok(()) => format!(
                            "Opened the decision model's settings page in the user's browser. Ask them to choose the model, paste its API key and press Save there; it is used at once.{}",
                            key.map(|k| format!(" ({k} opens it too.)"))
                                .unwrap_or_default()
                        ),
                        // The command names this server's settings file:
                        // without it, the page would save to the default
                        // one, which this server may not read.
                        Err(e) => match &self.store.path {
                            Some(p) => format!(
                                "Couldn't open the browser ({e}). Ask the user to run `computer-use-mcp settings --no-browser --config \"{}\"` in a terminal on this computer: it shows the address of the page where they set up the decision model.",
                                p.display()
                            ),
                            None => format!(
                                "Couldn't open the browser ({e}). This server keeps its settings in memory, so no page can save them: the decision model can be given here with decide setup={{provider, base_url, model, api_key}}."
                            ),
                        },
                    }))
                }
                "test" => decision::page::try_decider(&self.decider()?, &self.halt_watch())
                    .map(ToolOutput::text)
                    .map_err(Error::ActionFailed),
                "remove" | "clear" | "delete" => {
                    if let Some(path) = self.store.path.clone() {
                        decision::page::remove_settings(&path)?;
                    }
                    self.store.config.decision = crate::config::DecisionConfig::default();
                    // Set here first, so the reload sees no change: the old
                    // model's failures and answers are forgotten now.
                    self.judge.forget();
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
        let d = &self.decision_settings();
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
                let auto = if d.auto {
                    "The server also asks it on its own where that saves a turn (decision.auto)."
                } else {
                    "The server doesn't ask it on its own (decision.auto = false)."
                };
                format!(
                    "Decision model: {} ({source}). {how} {auto}\n{}",
                    dec.label(),
                    self.judge.report((self.clock)())
                )
            }
            Ok(None) => format!("No decision model is set up. {}", self.not_set_up()),
            Err(e) => format!("The decision model's settings are incomplete: {e}. {how}"),
        }
    }

    /// Settings the user gave in the chat.
    fn decision_set(&mut self, m: &serde_json::Map<String, Value>) -> Result<ToolOutput> {
        // No `api_key_env` from the chat: it would let anything that can
        // write to the chat send one of the server's environment variables
        // to an address of its choosing. It stays a setting of the file.
        const KEYS: [&str; 4] = ["provider", "base_url", "model", "api_key"];
        if let Some(k) = m.keys().find(|k| !KEYS.contains(&k.as_str())) {
            return Err(Error::InvalidArgs(format!(
                "unknown setup field \"{k}\" (provider, base_url, model, api_key)"
            )));
        }
        let field = |k: &str| {
            m.get(k)
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
        };
        // What the file holds now: never write back settings the user
        // saved on the page since.
        let mut d = self.decision_settings();
        let (was_provider, was_url) = (d.provider.clone(), d.base_url.clone());
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
        } else if d.provider != was_provider || d.base_url != was_url {
            // The saved key belongs to the old model: never send it to
            // another address.
            d.api_key.clear();
            d.api_key_env.clear();
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
        // Set here first, so the reload sees no change: the old model's
        // failures and answers are forgotten now.
        self.judge.forget();
        self.reload_now();
        let decider = Decider::from_config(&d)
            .ok()
            .flatten()
            .map(|x| x.with_settings_key(self.settings_key_name()));
        let label = decider.as_ref().map(Decider::label).unwrap_or_default();
        let halted = self.halt_watch();
        let test = match decider
            .as_ref()
            .map(|d| decision::page::try_decider(d, &halted))
        {
            Some(Ok(t)) => format!("Test: {t}"),
            Some(Err(e)) => format!("But the test failed: {e}"),
            None => String::new(),
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
/// The parts of a window, as positions in its nodes: the root's
/// children, with a part that holds most of the window split into its own
/// children (up to three times).
fn window_parts(nodes: &[Node]) -> Vec<usize> {
    if nodes.is_empty() {
        return Vec::new();
    }
    let size = |p: usize| {
        1 + nodes[p + 1..]
            .iter()
            .take_while(|n| n.depth > nodes[p].depth)
            .count()
    };
    let children = |p: usize| -> Vec<usize> {
        (p + 1..p + size(p))
            .filter(|&c| nodes[c].parent == Some(p))
            .collect()
    };
    let mut parts = children(0);
    for _ in 0..3 {
        let big = parts
            .iter()
            .position(|&p| size(p) * 10 > nodes.len() * 7 && children(p).len() >= 2);
        let Some(k) = big else { break };
        let p = parts.remove(k);
        let kids = children(p);
        parts.splice(k..k, kids);
    }
    parts
}

fn items_report(
    decider: &Decider,
    questions: &[Question],
    results: &[decision::Asked],
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
                counts.sort_by_key(|c| std::cmp::Reverse(c.1));
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
        let mut e = engine_with(backend);
        e.store.config.decision = decision;
        e
    }

    fn engine_with(backend: MockBackend) -> Engine<MockBackend> {
        Engine::new(backend, ConfigStore::in_memory(Config::default()))
            .with_time(Instant::now, |_| {})
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
        let key = crate::overlay::helper::pretty_key(&Config::default().control.settings_hotkey);
        assert!(out.text.contains(&key), "{}", out.text);
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
    fn pick_can_read_what_it_finds() {
        let f = system_one(|_, q| {
            let crit = q["criteria"].as_object().unwrap();
            let (idx, _) = crit
                .iter()
                .find(|(_, line)| line.as_str().unwrap().contains("Document"))
                .unwrap();
            json!({"type": "choice", "choice": idx, "confidence": 0.9, "probabilities": {idx.clone(): 0.9}})
        });
        let mut e = engine(jev(&f.url));
        let out = e.call_tool(
            "decide",
            json!({"app": "TextEdit", "pick": "the document's text", "read": true}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("It reads: \"Hello\""), "{}", out.text);
    }

    /// An editor whose window holds `n` buttons and nothing else to pick.
    fn buttons(n: u32) -> Engine<MockBackend> {
        use crate::mock::MockElement;
        use crate::types::Rect;
        let mut app = MockBackend::text_editor(4242);
        app.elements = vec![MockElement::new(1, "window", "", app.windows[0].bounds)];
        for k in 0..n {
            let at = Rect::new(
                f64::from(k % 30) * 25.0,
                f64::from(k / 30) * 25.0,
                20.0,
                20.0,
            );
            app.elements.push(
                MockElement::new(2 + u64::from(k), "button", &format!("Button {k}"), at)
                    .child_of(1)
                    .with_actions(&["AXPress"]),
            );
        }
        let mut backend = MockBackend::new();
        backend.add_app(app);
        engine_with(backend)
    }

    #[test]
    fn a_pick_with_a_lone_element_left_over_still_works() {
        // Button 0 when it is among the options, else the first one.
        let f = system_one(|_, q| {
            let crit = q["criteria"].as_object().unwrap();
            let idx = crit
                .iter()
                .find(|(_, line)| line.as_str().unwrap().contains("\"Button 0\""))
                .or_else(|| crit.iter().next())
                .map(|(i, _)| i.clone())
                .unwrap();
            json!({"type": "choice", "choice": idx, "confidence": 0.9})
        });
        // 65: a part of 64 and a part of one, which goes to the final round.
        let mut e = buttons(65);
        e.store.config.decision = jev(&f.url);
        let out = e.call_tool(
            "decide",
            json!({"app": "TextEdit", "pick": "the first button"}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("\"Button 0\""), "{}", out.text);
        assert_eq!(f.seen.lock().unwrap().len(), 2, "two rounds");
        // One element in all: it, without asking.
        let mut e = buttons(1);
        e.store.config.decision = jev(&f.url);
        let out = e.call_tool("decide", json!({"app": "TextEdit", "pick": "the button"}));
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("\"Button 0\""), "{}", out.text);
        assert_eq!(f.seen.lock().unwrap().len(), 2, "not asked");
    }

    #[test]
    fn about_shows_the_parts_that_matter() {
        // Without a decision model: the words of `about`.
        let mut e = engine(DecisionConfig::default());
        let out = e.call_tool(
            "get_app_state",
            json!({"app": "TextEdit", "about": "bold text"}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text
                .contains("the parts about \"bold text\": 1 of 2 (words matched)"),
            "{}",
            out.text
        );
        assert!(out.text.contains("button \"Bold\""), "{}", out.text);
        // Nothing matches: all of it.
        let out = e.call_tool(
            "get_app_state",
            json!({"app": "TextEdit", "about": "zebra"}),
        );
        assert!(out.text.contains("no part matched"), "{}", out.text);
        // With one: its judgment, all the parts in one request (one
        // question each), and the same look again from the cache.
        let f = system_one(|state, q| {
            let n = q["instructions"]
                .as_str()
                .unwrap()
                .split_whitespace()
                .nth(2)
                .unwrap()
                .to_string();
            let part = state
                .split(&format!("[part {n}]"))
                .nth(1)
                .unwrap_or_default()
                .split("[part ")
                .next()
                .unwrap_or_default()
                .to_string();
            json!({"type": "noul", "noul": if part.contains("Bold") { 0.9 } else { 0.05 }})
        });
        let mut e = engine(jev(&f.url));
        let out = e.call_tool(
            "get_app_state",
            json!({"app": "TextEdit", "about": "formatting"}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains(": 1 of 2 ("), "{}", out.text);
        assert!(!out.text.contains("words matched"), "{}", out.text);
        assert!(out.text.contains("button \"Bold\""), "{}", out.text);
        assert_eq!(
            f.seen.lock().unwrap().len(),
            1,
            "one request for both parts"
        );
        let again = e.call_tool(
            "get_app_state",
            json!({"app": "TextEdit", "about": "formatting"}),
        );
        assert!(again.text.contains(": 1 of 2 ("), "{}", again.text);
        assert_eq!(
            f.seen.lock().unwrap().len(),
            1,
            "the second look from the cache"
        );
        // decision.auto off: the words, and no request.
        let mut cfg = e.store().config.clone();
        cfg.decision.auto = false;
        e.set_config(ConfigStore::in_memory(cfg));
        let out = e.call_tool(
            "get_app_state",
            json!({"app": "TextEdit", "about": "formatting"}),
        );
        assert!(out.text.contains("words matched"), "{}", out.text);
        assert_eq!(f.seen.lock().unwrap().len(), 1);
    }

    fn quick(mut e: Engine<MockBackend>) -> Engine<MockBackend> {
        let mut cfg = e.store().config.clone();
        cfg.timing.expect_wait_ms = 0;
        e.set_config(ConfigStore::in_memory(cfg));
        e
    }

    #[test]
    fn an_expected_text_in_other_words_is_confirmed_by_the_model() {
        // The document says "Hello"; the agent expected a greeting.
        let f = system_one(
            |state, _| json!({"type": "noul", "noul": if state.contains("Hello") { 0.93 } else { 0.02 }}),
        );
        let mut e = quick(engine(jev(&f.url)));
        let out = e.call_tool(
            "click",
            json!({"app": "TextEdit", "name": "Bold", "expect": "a greeting"}),
        );
        assert!(
            out.text
                .contains("Expected a greeting: confirmed (in other words, as the decision model reads it: 0.93)"),
            "{}",
            out.text
        );
        assert_eq!(f.seen.lock().unwrap().len(), 1);
        // Word for word: no question.
        let out = e.call_tool(
            "click",
            json!({"app": "TextEdit", "name": "Bold", "expect": "Hello"}),
        );
        assert!(
            out.text.contains("Expected Hello: confirmed ("),
            "{}",
            out.text
        );
        assert_eq!(f.seen.lock().unwrap().len(), 1);
        // A kind (a dialog) is never asked about.
        let out = e.call_tool(
            "click",
            json!({"app": "TextEdit", "name": "Bold", "expect": "dialog"}),
        );
        assert!(!out.text.contains("decision model"), "{}", out.text);
        assert_eq!(f.seen.lock().unwrap().len(), 1);
        // Without a model: not seen, as before.
        let mut plain = quick(engine(DecisionConfig::default()));
        let out = plain.call_tool(
            "click",
            json!({"app": "TextEdit", "name": "Bold", "expect": "a greeting"}),
        );
        assert!(
            out.text.contains("Expected a greeting: not seen"),
            "{}",
            out.text
        );
    }

    #[test]
    fn a_failing_model_is_left_alone_and_the_server_judges_by_itself() {
        let f = fake(|_| (500, json!({"error": {"message": "down"}}).to_string()));
        let mut e = engine(jev(&f.url));
        for _ in 0..3 {
            let out = e.call_tool(
                "get_app_state",
                json!({"app": "TextEdit", "about": "formatting"}),
            );
            assert!(!out.is_error, "{}", out.text);
            assert!(out.text.contains("words matched"), "{}", out.text);
        }
        assert_eq!(f.seen.lock().unwrap().len(), 3);
        // Three failures in a row: the server's own questions stop.
        let out = e.call_tool(
            "get_app_state",
            json!({"app": "TextEdit", "about": "formatting"}),
        );
        assert!(out.text.contains("words matched"), "{}", out.text);
        assert_eq!(f.seen.lock().unwrap().len(), 3, "not asked while it rests");
        let status = e.call_tool("decide", json!({"setup": "status"}));
        assert!(status.text.contains("3 failed"), "{}", status.text);
        assert!(status.text.contains("judges by itself"), "{}", status.text);
        // The agent's own question still goes, and its error is shown.
        let out = e.call_tool("decide", json!({"question": "Is it?", "state": "x"}));
        assert!(out.is_error, "{}", out.text);
        assert_eq!(f.seen.lock().unwrap().len(), 4);
    }

    #[test]
    fn find_tools_asks_the_model_when_no_tool_name_matches() {
        let f = system_one(|_, q| {
            assert_eq!(q["type"], "choice");
            json!({"type": "choice", "choice": "locate", "probabilities": {"locate": 0.81, "draw": 0.12}})
        });
        let mut e = engine(jev(&f.url));
        let out = e.call_tool(
            "find_tools",
            json!({"query": "pinpoint the exact spot of a tiny icon"}),
        );
        assert!(out.text.starts_with("locate: "), "{}", out.text);
        assert!(
            out.text.contains("Chosen by the decision model"),
            "{}",
            out.text
        );
        assert_eq!(f.seen.lock().unwrap().len(), 1);
        // A word in a tool's name is enough: no question.
        let out = e.call_tool("find_tools", json!({"query": "draw a star"}));
        assert!(out.text.starts_with("draw: "), "{}", out.text);
        assert!(!out.text.contains("decision model"), "{}", out.text);
        assert_eq!(f.seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn parts_are_packed_as_they_fit() {
        let texts: Vec<String> = (0..100).map(|i| format!("button \"B{i}\"")).collect();
        // Room for all: 64 questions at most a request.
        let (jobs, slots) = pack_parts(&texts, "x", "W", "App", 1_000_000);
        assert_eq!(jobs.len(), 2);
        assert_eq!((jobs[0].1.len(), jobs[1].1.len()), (64, 36));
        assert_eq!(slots[1][0], (64, "part65".to_string()));
        assert!(jobs[1].0.contains("[part 65]\nbutton \"B64\""));
        // Little room: every state stays within it.
        let (jobs, _) = pack_parts(&texts, "x", "W", "App", 400);
        assert!(jobs.len() > 2);
        assert!(
            jobs.iter().all(|(s, _)| s.chars().count() <= 400),
            "a state too long"
        );
        let all: usize = jobs.iter().map(|(_, q)| q.len()).sum();
        assert_eq!(all, 100);
    }

    #[test]
    fn wait_for_until_asks_until_yes() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = calls.clone();
        let f = system_one(move |_, _| {
            c.fetch_add(1, Ordering::SeqCst);
            json!({"type": "noul", "noul": 0.8})
        });
        let mut e = engine(jev(&f.url));
        let out = e.call_tool(
            "wait_for",
            json!({"app": "TextEdit", "until": "Has the document loaded?", "poll_ms": 20, "timeout_ms": 5000}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.starts_with("Yes after"), "{}", out.text);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn wait_for_until_asks_once_about_a_window_that_didnt_change() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = calls.clone();
        let f = system_one(move |_, _| {
            c.fetch_add(1, Ordering::SeqCst);
            json!({"type": "noul", "noul": 0.2})
        });
        let mut e = engine(jev(&f.url)).with_time(Instant::now, std::thread::sleep);
        let out = e.call_tool(
            "wait_for",
            json!({"app": "TextEdit", "until": "Has the document loaded?", "poll_ms": 20, "timeout_ms": 400}),
        );
        assert!(
            out.is_error && out.text.contains("never answered yes"),
            "{}",
            out.text
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
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
    fn setup_from_the_chat_never_sends_the_saved_key_elsewhere() {
        let good = system_one(|_, _| json!({"type": "noul", "noul": 1.0}));
        let mut e = engine(jev(&good.url));
        let other = system_one(|_, _| json!({"type": "noul", "noul": 1.0}));
        let _ = e.call_tool("decide", json!({"setup": {"base_url": other.url}}));
        let leaked = |f: &crate::decision::tests::Fake, secret: &str| {
            f.seen.lock().unwrap().iter().any(|(_, h, _)| {
                h.iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("authorization") && v.contains(secret))
            })
        };
        assert!(!leaked(&other, "sk-test-key-9876"));
        // No environment variable named from the chat.
        let out = e.call_tool(
            "decide",
            json!({"setup": {"provider": "jev", "base_url": other.url, "api_key_env": "HOME"}}),
        );
        assert!(
            out.is_error && out.text.contains("unknown setup field"),
            "{}",
            out.text
        );
    }

    #[test]
    fn a_model_set_from_the_chat_starts_with_a_clean_record() {
        let down = fake(|_| (500, json!({"error": {"message": "down"}}).to_string()));
        let mut e = engine(jev(&down.url));
        for _ in 0..3 {
            let _ = e.call_tool(
                "get_app_state",
                json!({"app": "TextEdit", "about": "formatting"}),
            );
        }
        let status = e.call_tool("decide", json!({"setup": "status"}));
        assert!(status.text.contains("judges by itself"), "{}", status.text);
        // The old model's failures aren't the new one's.
        let up = system_one(|_, _| json!({"type": "noul", "noul": 1.0}));
        let out = e.call_tool("decide", json!({"setup": {"base_url": up.url}}));
        assert!(!out.is_error, "{}", out.text);
        let asked = up.seen.lock().unwrap().len();
        // The server asks the new one on its own at once (no rest for the
        // old one's failures).
        let out = e.call_tool(
            "get_app_state",
            json!({"app": "TextEdit", "about": "formatting"}),
        );
        assert!(!out.text.contains("words matched"), "{}", out.text);
        assert_eq!(up.seen.lock().unwrap().len(), asked + 1);
    }

    #[test]
    fn without_hot_reload_what_the_page_saved_is_used_and_kept() {
        let dir = std::env::temp_dir().join(format!("cu-decide-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let file = |url: &str, key: &str| {
            format!(
                "hot_reload = false\n[decision]\nprovider = \"openai\"\nbase_url = \"{url}\"\nmodel = \"m1\"\napi_key = \"{key}\"\n"
            )
        };
        // Models that answer at once (the test after a change asks one).
        let old = fake(|_| (500, "{}".into()));
        let new = fake(|_| (500, "{}".into()));
        std::fs::write(&path, file(&old.url, "sk-old-1111")).unwrap();
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let mut e = Engine::new(backend, ConfigStore::load(Some(&path)).unwrap())
            .with_time(Instant::now, |_| {});
        // The user saves another model on the page.
        std::fs::write(&path, file(&new.url, "sk-new-2222")).unwrap();
        let status = e.call_tool("decide", json!({"setup": "status"}));
        assert!(
            status.text.contains("2222") && !status.text.contains("1111"),
            "{}",
            status.text
        );
        // Another model name from the chat keeps the rest the user saved.
        let out = e.call_tool("decide", json!({"setup": {"model": "m2"}}));
        assert!(out.text.contains("Decision model set"), "{}", out.text);
        let saved = ConfigStore::load(Some(&path)).unwrap().config.decision;
        assert_eq!(saved.base_url, new.url);
        assert_eq!(saved.api_key, "sk-new-2222");
        assert_eq!(saved.model, "m2");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn without_hot_reload_a_model_saved_on_the_page_is_taken_up_once() {
        let dir = std::env::temp_dir().join(format!("cu-decide-sync-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let file = |model: &str, auto: bool| {
            format!(
                "hot_reload = false\n[decision]\nprovider = \"openai\"\nbase_url = \"http://127.0.0.1:9/v1\"\nmodel = \"{model}\"\napi_key = \"k\"\nauto = {auto}\n"
            )
        };
        std::fs::write(&path, file("m1", true)).unwrap();
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let mut e = Engine::new(backend, ConfigStore::load(Some(&path)).unwrap())
            .with_time(Instant::now, |_| {});
        assert!(e.auto_decider().is_some());
        // The page saves another model that the server mustn't ask on its own.
        std::fs::write(&path, file("m2", false)).unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert!(e.changed_without_hot_reload().is_some());
        e.sync_decision_settings();
        assert_eq!(e.store.config.decision.model, "m2");
        assert!(e.auto_decider().is_none());
        // Read once: not again until the file changes.
        assert!(e.changed_without_hot_reload().is_none());
        let _ = std::fs::remove_dir_all(dir);
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
