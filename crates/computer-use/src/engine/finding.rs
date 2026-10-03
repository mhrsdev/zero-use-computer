//! Finding elements without reading the whole tree: `find_element` and `wait_for`.

use super::*;

impl<B: Backend> Engine<B> {
    // -- new tools ---------------------------------------------------------

    pub(super) fn find_element(&mut self, args: FindElementArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        let window = self.resolve_window(&app, args.window.as_deref(), false)?;
        self.observe(&app, &window, false)?;
        let role = args.role.map(|r| r.to_lowercase());
        let name = args.name.map(|n| crate::text::fold(&n));
        let text = args.text.map(|t| crate::text::fold(&t));
        let state = self.state(app.pid)?;
        let hits: Vec<&Node> = state
            .nodes
            .iter()
            .filter(|n| {
                role.as_deref().is_none_or(|r| n.role == r)
                    && name.as_deref().is_none_or(|q| {
                        n.name
                            .as_deref()
                            .is_some_and(|nm| crate::text::fold(nm).contains(q))
                    })
                    && text.as_deref().is_none_or(|q| node_text(n).contains(q))
                    && (!args.editable || n.states.editable)
            })
            .collect();
        let total = hits.len();
        let offset = args.offset.unwrap_or(0);
        let hits: Vec<&Node> = hits
            .into_iter()
            .skip(offset)
            .take(args.max_results.max(1))
            .collect();
        if hits.is_empty() {
            return Ok(ToolOutput::text(if total > 0 {
                format!(
                    "{total} element(s) in {} match; none after offset {offset}.",
                    app.name
                )
            } else {
                format!(
                    "No elements in {} match. Try get_app_state to see the whole tree.",
                    app.name
                )
            }));
        }
        let next = offset + hits.len();
        let mut out = if offset == 0 && next == total {
            format!("{} matching element(s) in {}:\n", hits.len(), app.name)
        } else {
            format!(
                "Matches {}–{next} of {total} in {}{}:\n",
                offset + 1,
                app.name,
                if next < total {
                    format!(" (offset={next} for the next ones)")
                } else {
                    String::new()
                }
            )
        };
        for n in hits {
            out.push_str(&format!("{} {}\n", n.index, n.line));
        }
        Ok(ToolOutput::text(out))
    }

    pub(super) fn wait_for(&mut self, args: WaitForArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        let role = args.role.clone().map(|r| r.to_lowercase());
        let name = args.name.clone().map(|n| crate::text::fold(&n));
        let text = args.text.clone().map(|t| crate::text::fold(&t));
        let timing = &self.store.config.timing;
        // At most two minutes: the server answers nothing else while it waits.
        let timeout_ms = args
            .timeout_ms
            .unwrap_or(timing.wait_timeout_ms)
            .clamp(1, 120_000);
        let poll = Duration::from_millis(
            args.poll_ms
                .unwrap_or(timing.wait_poll_ms)
                .clamp(20, 60_000),
        );
        let timeout = Duration::from_millis(timeout_ms);
        let start = (self.clock)();
        let deadline = start + timeout;
        // `until`: the decision model judges the window each time.
        let until = match args.until.as_deref().map(str::trim) {
            Some(q) if !q.is_empty() => Some((q.to_string(), self.decider()?)),
            _ => None,
        };
        let matchers = role.is_some() || name.is_some() || text.is_some();
        // The text the decision model last said no about.
        let mut asked = None;

        loop {
            if let Ok(window) = self.resolve_window(&app, args.window.as_deref(), true)
                && self.observe(&app, &window, true).is_ok()
            {
                let state = self.state(app.pid)?;
                let found = state
                    .nodes
                    .iter()
                    .find(|n| {
                        role.as_deref().is_none_or(|r| n.role == r)
                            && name.as_deref().is_none_or(|q| {
                                n.name
                                    .as_deref()
                                    .is_some_and(|nm| crate::text::fold(nm).contains(q))
                            })
                            && text.as_deref().is_none_or(|q| node_text(n).contains(q))
                            && state_matches(n, args.state)
                    })
                    .map(|n| (n.index, n.line.clone()));
                match (&until, found) {
                    (None, Some((index, line))) => {
                        return Ok(ToolOutput::text(format!(
                            "Found after waiting: {index} {line}"
                        )));
                    }
                    (Some((question, decider)), found) if found.is_some() || !matchers => {
                        let decider = decider.clone();
                        let window = args.window.clone();
                        if let Some(yes) = self.until_yes(
                            &decider,
                            question,
                            &args.app,
                            window.as_deref(),
                            &mut asked,
                        )? {
                            let waited = (self.clock)().saturating_duration_since(start);
                            let mut out = format!(
                                "Yes after {:.1} s ({yes:.2}): {question}",
                                waited.as_secs_f64()
                            );
                            if let Some((index, line)) = found.filter(|_| matchers) {
                                out.push_str(&format!("\nFound: {index} {line}"));
                            }
                            return Ok(ToolOutput::text(out));
                        }
                    }
                    _ => {}
                }
            }
            if self.halted() {
                return Err(self.stopped_error());
            }
            if (self.clock)() >= deadline {
                return Err(Error::ActionFailed(match &until {
                    Some((q, _)) => format!(
                        "timed out after {timeout_ms}ms: the decision model never answered yes to \"{q}\""
                    ),
                    None => format!(
                        "timed out after {}ms waiting for an element matching {}",
                        timeout_ms,
                        describe_matcher(&args)
                    ),
                }));
            }
            (self.sleep)(poll);
        }
    }
}

fn state_matches(n: &Node, want: crate::tools::ElementState) -> bool {
    use crate::tools::ElementState::*;
    match want {
        Present => true,
        Visible => !n.states.hidden,
        Enabled => n.states.enabled,
        Focused => n.states.focused,
        Checked => n.states.checked == Some(true),
    }
}

fn describe_matcher(args: &WaitForArgs) -> String {
    let mut parts = Vec::new();
    if let Some(r) = &args.role {
        parts.push(format!("role={r}"));
    }
    if let Some(n) = &args.name {
        parts.push(format!("name~\"{n}\""));
    }
    if let Some(t) = &args.text {
        parts.push(format!("text~\"{t}\""));
    }
    if parts.is_empty() {
        "any element".into()
    } else {
        parts.join(", ")
    }
}
