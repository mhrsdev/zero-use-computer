//! `batch`, and what the model had seen before a batch or script ran.

use super::*;

impl<B: Backend> Engine<B> {
    pub(super) fn batch(&mut self, args: BatchArgs) -> Result<ToolOutput> {
        if args.steps.is_empty() {
            return Err(Error::InvalidArgs("batch needs at least one step".into()));
        }
        let total = args.steps.len();
        let mut report = String::new();
        let mut last_image = None;
        let mut any_error = false;
        let mut ran = 0;
        let mut stopped = String::new();
        // The app the steps acted on last: the report at the end is of it.
        let mut acted_on: Option<String> = None;
        // The report shows one line per step, so the trees the steps render
        // never reach the model: what it has seen of each app stays what it
        // saw before the batch (restored below).
        let seen_before = self.known_screens();
        for (i, step) in args.steps.iter().enumerate() {
            // Inject the default app when the step omits one.
            let mut step_args = match &step.arguments {
                serde_json::Value::Object(m) => m.clone(),
                serde_json::Value::Null => serde_json::Map::new(),
                other => {
                    report.push_str(&format!(
                        "{}. {} — bad arguments (expected an object, got {other})\n",
                        i + 1,
                        step.tool
                    ));
                    any_error = true;
                    break;
                }
            };
            if !step_args.contains_key("app")
                && let Some(app) = &args.app
            {
                step_args.insert("app".into(), serde_json::json!(app));
            }
            let step_app = step_args
                .get("app")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let expects = step_args
                .get("expect")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|e| !e.trim().is_empty());
            let more = i + 1 < total;
            // The window in front before the step, to notice one coming up.
            let window_before = match &step_app {
                Some(a) if more && !args.through_windows && !expects => self.front_window(a),
                _ => None,
            };
            let parsed = ToolCall::parse(&step.tool, serde_json::Value::Object(step_args));
            let acting = parsed.as_ref().ok().and_then(mutating_app);
            let before = self.ctx.pending_images.len();
            let shot_before = self.ctx.pending_screen_shot.take();
            let quiet = self.ctx.quiet_depth.replace(self.ctx.depth + 1);
            let result = parsed.and_then(|c| self.call(c));
            self.ctx.quiet_depth = quiet;
            ran += 1;
            self.report_progress(ran as f64, Some(total as f64), || {
                format!("step {ran} of {total}: {}", step.tool)
            });
            if acting.is_some() {
                acted_on = acting;
            }
            let imaged = result.as_ref().is_ok_and(|o| o.image.is_some());
            if !imaged {
                // No image from this step: an earlier step's stays the one
                // the batch returns.
                self.ctx.pending_screen_shot = shot_before;
            }
            match result {
                Ok(out) => {
                    let first = out.text.lines().next().unwrap_or("");
                    report.push_str(&format!("{}. {} — {first}\n", i + 1, step.tool));
                    if out.image.is_some() {
                        // Earlier images are replaced by this one.
                        self.ctx.pending_images.drain(..before);
                        last_image = out.image;
                    }
                    if out.is_error {
                        any_error = true;
                        if !args.continue_on_error {
                            break;
                        }
                        continue;
                    }
                    if !more {
                        continue;
                    }
                    // The steps after this one were planned on what it
                    // expected.
                    if expects && self.ctx.last_expect != Some(Outcome::Confirmed) {
                        stopped = format!(
                            "Stopped after step {}: what it expected wasn't confirmed; the {} step(s) after it were not run.",
                            i + 1,
                            total - i - 1
                        );
                        break;
                    }
                    if let (Some((id, _)), Some(a)) = (window_before, &step_app)
                        && let Some((now, title)) = self.front_window(a)
                        && now != id
                    {
                        stopped = format!(
                            "Stopped after step {}: window \"{title}\" came up, which it didn't expect; the {} step(s) after it were not run (they were meant for the window before; add expect dialog to a step that opens one, or through_windows=true).",
                            i + 1,
                            total - i - 1
                        );
                        break;
                    }
                }
                Err(e) => {
                    report.push_str(&format!("{}. {} — ERROR: {e}\n", i + 1, step.tool));
                    any_error = true;
                    if !args.continue_on_error || matches!(e, Error::Stopped(_) | Error::Cancelled)
                    {
                        break;
                    }
                }
            }
        }
        self.restore_known(seen_before);
        let head = if ran == total {
            format!("Ran {total} step(s):\n")
        } else {
            format!("Ran {ran} of {total} step(s):\n")
        };
        let mut text = format!("{head}{report}");
        if !stopped.is_empty() {
            text.push_str(&stopped);
            text.push('\n');
        }
        let mut out = ToolOutput {
            text,
            image: last_image,
            is_error: false,
        };
        // One report of what the steps changed, against what the model saw
        // before them.
        if self.store.config.tree.report_changes
            && let Some(app) = acted_on
        {
            let at = out.text.len();
            out = self.append_changes(&app, out);
            let tail = out.text.split_off(at);
            out.text
                .push_str(&tail.replacen("State after the action", "State after the steps", 1));
        }
        out.is_error = any_error;
        Ok(out)
    }

    /// The window an app shows in front now (its id and title).
    fn front_window(&mut self, query: &str) -> Option<(u64, String)> {
        let app = self.resolve_app(query).ok()?;
        let w = self.resolve_window(&app, None, false).ok()?;
        Some((w.id, w.title))
    }

    /// What the model has seen of each app, to put back after calls whose
    /// trees it never saw (batch steps, a script's tool calls).
    /// What the model has been shown so far, to put back with
    /// [`Self::not_delivered`] if the answer to the next call never reaches
    /// it (the client cancelled the call).
    pub fn shown(&self) -> Shown {
        Shown {
            hints: self.hints.0.borrow().clone(),
            apps: self
                .states
                .iter()
                .map(|(pid, st)| (*pid, (st.known.clone(), st.coord)))
                .collect(),
            screen_shot: self.screen_shot.clone(),
            partial: self.partial_report,
        }
    }

    /// The answer to the last call never reached the model: what it has
    /// seen is what it had seen before (`shown`), so the next look sends
    /// in full what that call would have shown, and no explanation counts
    /// as given.
    pub fn not_delivered(&mut self, shown: Shown) {
        *self.hints.0.borrow_mut() = shown.hints;
        self.partial_report = shown.partial;
        self.screen_shot = shown.screen_shot;
        for (pid, st) in self.states.iter_mut() {
            let (known, coord) = shown.apps.get(pid).cloned().unwrap_or_default();
            st.known = known;
            st.coord = coord;
        }
    }

    pub(super) fn known_screens(&self) -> SeenBefore {
        SeenBefore {
            known: self
                .states
                .iter()
                .map(|(pid, st)| (*pid, st.known.clone()))
                .collect(),
            memory: self.memory.views(),
            hints: self.hints.0.borrow().clone(),
        }
    }

    pub(super) fn restore_known(&mut self, seen: SeenBefore) {
        let SeenBefore {
            known: seen_before,
            memory,
            hints,
        } = seen;
        *self.hints.0.borrow_mut() = hints;
        self.memory.restore_views(memory);
        let mut refile = Vec::new();
        for (pid, st) in self.states.iter_mut() {
            let before = seen_before.get(pid).cloned().flatten();
            st.known = match (before, st.known.take()) {
                // Same screen: the model's tree is the one from before; the
                // screenshot state (only the returned image counts) is kept.
                (Some(b), Some(mut now)) if b.id == now.id => {
                    now.view = b.view;
                    Some(now)
                }
                // A screen reached meanwhile: its tree hasn't been shown, so
                // the next look sends all of it.
                (before, Some(mut now)) => {
                    refile.extend(before);
                    now.view = crate::screens::View::default();
                    Some(now)
                }
                (before, None) => before,
            };
        }
        let cache = self.store.config.cache.clone();
        if cache.enabled {
            for s in refile {
                self.memory.remember(
                    s,
                    cache.max_screens,
                    cache.max_memory_kb.saturating_mul(1024),
                );
            }
        }
    }
}
