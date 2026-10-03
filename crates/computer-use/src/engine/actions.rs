//! Acting on elements: click, secondary actions, values, text selection, scrolling, dragging, keys and typing, and the change report after each.

use super::*;

impl<B: Backend> Engine<B> {
    pub(super) fn click(&mut self, args: ClickArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
        let count = args.click_count.clamp(1, 3);
        let (x, y, snapped) = match &args.snap {
            None => (args.x, args.y, String::new()),
            Some(how) => {
                let (Some(x), Some(y)) = (args.x, args.y) else {
                    return Err(Error::InvalidArgs(
                        "snap moves an x/y point: give x and y".into(),
                    ));
                };
                let (nx, ny, note) =
                    self.snap_xy(&app, args.window.as_deref(), x, y, how, args.snap_radius)?;
                (Some(nx), Some(ny), note)
            }
        };
        // By name: the one element that has it.
        let mut named = String::new();
        let index = match args.element_index {
            None if x.is_none() && (args.name.is_some() || args.role.is_some()) => {
                let i = self.element_named(
                    &app,
                    args.window.as_deref(),
                    args.name.as_deref(),
                    args.role.as_deref(),
                )?;
                named = format!(" ({i})");
                Some(i)
            }
            i => i,
        };
        let anchor = self.anchor(&app, index, x, y, "click")?;
        // What things looked like, to tell whether the click did anything.
        let before = self.tree_fingerprint(app.pid);
        let what = format!("{}{named}", self.describe_anchor(&app, &anchor));
        let point = self.anchor_point(&app, &anchor).ok();

        // The agent cursor goes there first, so the user sees what is next.
        self.overlay_anchor(&app, &anchor, true);

        // A single left click on an element with a press action goes through
        // the accessibility API so it works in the background.
        let mut note = String::new();
        if let (Anchor::Element(h), MouseButton::Left, 1) = (&anchor, args.button, count) {
            let node = self.node_for_handle(&app, *h);
            // A table cell's press may not select its row (GTK): clicking it
            // again with the mouse only selects it, so that is safe.
            let cell = node.is_some_and(|n| crate::roles::is_cell(&n.role));
            if let Some(action) = node
                .and_then(|n| n.has_action("press"))
                .map(|a| a.native.clone())
            {
                match self.backend.perform_action(*h, &action) {
                    Ok(()) => {
                        self.settle_on(&app);
                        let unchanged = self.verified() && self.tree_fingerprint(app.pid) == before;
                        let v = &self.store.config.verify;
                        if unchanged
                            && v.retry
                            && (v.retry_on_no_change || cell)
                            && let Some(p) = point
                        {
                            // Nothing happened: click it with the mouse.
                            let target = self.input_target(&app)?;
                            self.backend.click(&target, p, MouseButton::Left, 1)?;
                            self.settle_on(&app);
                            let mut msg = format!(
                                "Pressed {what}; nothing changed, so clicked it with the mouse too."
                            );
                            if self.verified() && self.tree_fingerprint(app.pid) == before {
                                msg.push_str(NO_CHANGE_NOTE);
                            }
                            return Ok(ToolOutput::text(msg));
                        }
                        let mut msg = format!("Pressed {what}.");
                        if unchanged {
                            msg.push_str(NO_CHANGE_NOTE);
                        }
                        return Ok(ToolOutput::text(msg));
                    }
                    // Sent, but unanswered: clicking again could press it twice.
                    Err(e @ Error::Unanswered(_)) => {
                        self.settle_on(&app);
                        return Ok(ToolOutput::text(format!("Pressed {what}, but {e}")));
                    }
                    Err(e) if self.store.config.verify.retry && point.is_some() => {
                        log::info!("accessibility press failed ({e}); clicking instead");
                        note = format!(
                            " (its accessibility action failed: {e}; clicked it with the mouse instead)"
                        );
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        let point = match point {
            Some(p) => p,
            None => self.anchor_point(&app, &anchor)?,
        };
        let target = self.input_target(&app)?;
        self.backend.click(&target, point, args.button, count)?;
        self.settle_on(&app);
        let verb = match (args.button, count) {
            (MouseButton::Right, _) => "Right-clicked",
            (_, 2) => "Double-clicked",
            (_, 3) => "Triple-clicked",
            _ => "Clicked",
        };
        let mut msg = format!(
            "{verb} {what} at ({:.0}, {:.0}).{note}{snapped}",
            point.x, point.y
        );
        if self.verified() && self.tree_fingerprint(app.pid) == before {
            msg.push_str(NO_CHANGE_NOTE);
        }
        Ok(ToolOutput::text(msg))
    }

    pub(super) fn perform_secondary(&mut self, args: SecondaryActionArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
        let handle = self.element_by_index(&app, args.element_index)?;
        let node = self.node_by_index(&app, args.element_index)?.clone();
        ocr_can_only_be_clicked(handle, "perform_secondary_action")?;
        let action = node.has_action(&args.action).ok_or_else(|| {
            let available: Vec<&str> = node.actions.iter().map(|a| a.name.as_str()).collect();
            Error::InvalidArgs(format!(
                "{} has no action `{}`. Available: [{}]",
                node.label(),
                args.action,
                available.join(", ")
            ))
        })?;
        let native = action.native.clone();
        let before = self.tree_fingerprint(app.pid);
        self.overlay_point_element(&app, handle, true);
        match self.backend.perform_action(handle, &native) {
            Ok(()) => {}
            Err(e @ Error::Unanswered(_)) => {
                self.settle_on(&app);
                return Ok(ToolOutput::text(format!(
                    "Performed `{}` on {}, but {e}",
                    args.action,
                    node.label()
                )));
            }
            Err(e) => return Err(e),
        }
        self.settle_on(&app);
        let mut msg = format!("Performed `{}` on {}.", args.action, node.label());
        if self.verified() && self.tree_fingerprint(app.pid) == before {
            msg.push_str(NO_CHANGE_NOTE);
        }
        Ok(ToolOutput::text(msg))
    }

    pub(super) fn set_value(&mut self, args: SetValueArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
        let handle = self.element_by_index(&app, args.element_index)?;
        let node = self.node_by_index(&app, args.element_index)?.clone();
        ocr_can_only_be_clicked(handle, "set_value")?;
        self.overlay_point_element(&app, handle, true);
        // Text fields can be typed into when setting fails.
        let typable = node.states.editable && node.states.checked.is_none();
        let retry = self.store.config.verify.retry && typable;
        let mut how = String::new();
        if let Err(e) = self.backend.set_value(handle, &args.value) {
            // It may have been set: typing it as well could enter it twice,
            // into whatever the busy app shows next.
            if let Error::Unanswered(_) = e {
                self.settle_on(&app);
                return Ok(ToolOutput::text(format!(
                    "Sent the new value to {}, but {e}",
                    node.label()
                )));
            }
            if !retry {
                return Err(e);
            }
            log::info!("set_value failed ({e}); typing instead");
            self.retype(&app, handle, &node, &args.value)?;
            how = format!(" (setting it directly failed: {e}; typed it instead)");
        }
        self.settle_on(&app);
        let shown = tree::truncate(&args.value, 80);
        let mut msg = format!("Set {} to \"{shown}\".{how}", node.label());
        // The field shows it: the value is the one just sent, not news.
        if self.store.config.tree.compact
            && how.is_empty()
            && self.verified()
            && self.value_took(&app, args.element_index, &args.value) == Some(true)
        {
            msg = format!("Set {}; it shows the new value.", node.label());
        }
        if self.verified() && self.value_took(&app, args.element_index, &args.value) == Some(false)
        {
            let fresh = self.node_by_index(&app, args.element_index).ok().cloned();
            if retry
                && how.is_empty()
                && let Some(n) = fresh
            {
                // The value didn't take: type it into the field instead.
                self.retype(&app, n.handle, &n, &args.value)?;
                self.settle_on(&app);
                msg = format!(
                    "Set {} to \"{shown}\" (the value didn't take at first; typed it instead).",
                    node.label()
                );
            }
            if self.verified()
                && self.value_took(&app, args.element_index, &args.value) == Some(false)
            {
                let now = self
                    .node_by_index(&app, args.element_index)
                    .ok()
                    .and_then(|n| n.value.clone())
                    .unwrap_or_default();
                msg.push_str(&format!(
                    " Note: it now shows \"{}\", not the value that was set; check it before going on.",
                    tree::truncate(&now, 80)
                ));
            }
        }
        Ok(ToolOutput::text(msg))
    }

    pub(super) fn select_text(&mut self, args: SelectTextArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
        let handle = self.element_by_index(&app, args.element_index)?;
        let node = self.node_by_index(&app, args.element_index)?.clone();
        ocr_can_only_be_clicked(handle, "select_text")?;
        self.overlay_point_element(&app, handle, false);
        self.backend
            .select_text(handle, args.text.as_deref(), args.occurrence.max(1))?;
        self.settle_on(&app);
        let what = match &args.text {
            Some(t) => format!("\"{}\"", tree::truncate(t, 60)),
            None => "all text".into(),
        };
        Ok(ToolOutput::text(format!(
            "Selected {what} in {}.",
            node.label()
        )))
    }

    pub(super) fn scroll(&mut self, args: ScrollArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
        // A wheel turn per line: an amount in the millions would keep the
        // backend busy for hours.
        let pages = if args.amount.is_finite() && args.amount > 0.0 {
            args.amount.min(MAX_SCROLL_PAGES)
        } else {
            1.0
        };
        let anchor = self.anchor(&app, args.element_index, args.x, args.y, "scroll")?;
        let before = self.tree_fingerprint(app.pid);
        let what = self.describe_anchor(&app, &anchor);
        let point = self.anchor_point(&app, &anchor).ok();
        self.overlay_anchor(&app, &anchor, false);
        let (ux, uy) = args.direction.unit();
        // ~3 wheel lines per page.
        let lines = (pages * 3.0).round().max(1.0) as i32;

        if let Anchor::Element(h) = &anchor
            && !crate::ocr::is_ocr(*h)
            && let Native::Done(_) = self.backend.scroll_element(*h, args.direction, pages)?
        {
            self.settle_on(&app);
            let mut msg = format!("Scrolled {what} {:?} by {pages} page(s).", args.direction);
            if self.verified() && self.tree_fingerprint(app.pid) == before {
                // Scrolling again does no harm: try the mouse wheel.
                if self.store.config.verify.retry
                    && let Some(p) = point
                {
                    let target = self.input_target(&app)?;
                    self.backend
                        .scroll_wheel(&target, p, ux * lines, uy * lines)?;
                    self.settle_on(&app);
                    msg = format!(
                        "Scrolled {what} {:?} by {pages} page(s) (with the mouse wheel; the first try didn't move it).",
                        args.direction
                    );
                }
                if self.verified() && self.tree_fingerprint(app.pid) == before {
                    msg.push_str(" Nothing moved: it may already be at the end.");
                }
            }
            return Ok(ToolOutput::text(msg));
        }

        let point = match point {
            Some(p) => p,
            None => self.anchor_point(&app, &anchor)?,
        };
        let target = self.input_target(&app)?;
        self.backend
            .scroll_wheel(&target, point, ux * lines, uy * lines)?;
        self.settle_on(&app);
        let mut msg = format!(
            "Scrolled {:?} by {pages} page(s) at ({:.0}, {:.0}).",
            args.direction, point.x, point.y
        );
        if self.verified() && self.tree_fingerprint(app.pid) == before {
            msg.push_str(" Nothing moved: it may already be at the end, or not scrollable there.");
        }
        Ok(ToolOutput::text(msg))
    }

    pub(super) fn drag(&mut self, args: DragArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
        let (mut fx, mut fy, mut tx, mut ty) = (args.from_x, args.from_y, args.to_x, args.to_y);
        let mut snapped = String::new();
        if let Some(how) = &args.snap {
            let mut any = false;
            for (x, y, end) in [(&mut fx, &mut fy, "start"), (&mut tx, &mut ty, "end")] {
                if let (Some(px), Some(py)) = (*x, *y) {
                    let (nx, ny, note) =
                        self.snap_xy(&app, args.window.as_deref(), px, py, how, args.snap_radius)?;
                    (*x, *y) = (Some(nx), Some(ny));
                    snapped.push_str(&note.replace(" Snapped", &format!(" The {end} snapped")));
                    any = true;
                }
            }
            if !any {
                return Err(Error::InvalidArgs(
                    "snap moves x/y points: give from_x/from_y or to_x/to_y".into(),
                ));
            }
        }
        let from = self.anchor(&app, args.from_element_index, fx, fy, "drag source")?;
        let to = self.anchor(&app, args.to_element_index, tx, ty, "drag target")?;
        let (p0, p1) = (
            self.anchor_point(&app, &from)?,
            self.anchor_point(&app, &to)?,
        );
        let before = self.tree_fingerprint(app.pid);
        // At the start before the button goes down, then along with the drag.
        self.overlay_point(p0, true);
        self.overlay_glide(p1);
        let target = self.input_target(&app)?;
        self.backend.drag(&target, p0, p1)?;
        self.settle_on(&app);
        let mut msg = format!(
            "Dragged from ({:.0}, {:.0}) to ({:.0}, {:.0}).{snapped}",
            p0.x, p0.y, p1.x, p1.y
        );
        if self.verified() && self.tree_fingerprint(app.pid) == before {
            msg.push_str(NO_CHANGE_NOTE);
        }
        Ok(ToolOutput::text(msg))
    }

    /// Point the mouse at screenshot pixel (x, y) of `app`, for apps that
    /// send keys to what is under the pointer. Returns how to put the
    /// pointer back afterwards ([`Engine::unhover`]).
    fn hover(
        &mut self,
        app: &AppInfo,
        x: Option<f64>,
        y: Option<f64>,
    ) -> Result<Option<(InputTarget, Point)>> {
        if x.is_none() && y.is_none() {
            return Ok(None);
        }
        let anchor = self.anchor(app, None, x, y, "the pointer position")?;
        let at = self.anchor_point(app, &anchor)?;
        let target = self.input_target(app)?;
        self.overlay_point(at, false);
        let back = self.backend.move_pointer(&target, at)?;
        // Let the app see where the pointer is before the keys arrive.
        (self.sleep)(Duration::from_millis(40));
        Ok(back.map(|p| (target, p)))
    }

    fn unhover(&mut self, back: Option<(InputTarget, Point)>) {
        if let Some((target, p)) = back {
            let _ = self.backend.move_pointer(&target, p);
        }
    }

    fn press_combos(&mut self, app: &AppInfo, combos: &[KeyCombo]) -> Result<()> {
        let target = self.input_target(app)?;
        for combo in combos {
            if self.halted() {
                return Err(self.stopped_error());
            }
            self.backend.press_key(&target, combo)?;
            (self.sleep)(Duration::from_millis(self.store.config.timing.key_delay_ms));
        }
        Ok(())
    }

    pub(super) fn press_key(&mut self, args: PressKeyArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
        let combos = keys::parse_sequence(&args.key)?;
        if combos.len() > MAX_KEY_PRESSES {
            return Err(Error::InvalidArgs(format!(
                "`key` has {} presses; at most {MAX_KEY_PRESSES} per call (type text with type_text)",
                combos.len()
            )));
        }
        if let Some(i) = args.element_index {
            let h = self.element_by_index(&app, i)?;
            let node = self.node_by_index(&app, i)?.clone();
            self.overlay_point_element(&app, h, false);
            self.focus_element(&app, h, &node)?;
        } else if args.x.is_none() {
            self.overlay_point_focus(&app);
        }
        let back = self.hover(&app, args.x, args.y)?;
        let pressed = self.press_combos(&app, &combos);
        self.unhover(back);
        pressed?;
        self.settle_on(&app);
        let shown: Vec<String> = combos.iter().map(|c| c.to_string()).collect();
        Ok(ToolOutput::text(format!("Pressed {}.", shown.join(" "))))
    }

    pub(super) fn type_text(&mut self, args: TypeTextArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
        if args.text.is_empty() {
            return Err(Error::InvalidArgs("`text` must not be empty".into()));
        }
        let count = args.text.chars().count();
        if count > MAX_TYPED_CHARS {
            return Err(Error::InvalidArgs(format!(
                "`text` has {count} characters; at most {MAX_TYPED_CHARS} per call (paste long text: set_clipboard, then press_key ctrl+v / cmd+v)"
            )));
        }
        let mut field = None;
        if let Some(i) = args.element_index {
            let h = self.element_by_index(&app, i)?;
            let node = self.node_by_index(&app, i)?.clone();
            self.overlay_point_element(&app, h, true);
            self.focus_element(&app, h, &node)?;
            self.settle();
            field = Some((i, node));
        } else if args.x.is_none() {
            self.overlay_point_focus(&app);
        }
        let back = self.hover(&app, args.x, args.y)?;
        let typed = self.type_into_focus(&app, &args.text);
        self.unhover(back);
        typed?;
        self.settle_on(&app);
        let mut msg = format!("Typed {} character(s).", args.text.chars().count());

        // Typed into a field whose text we can read: did it land?
        if let Some((i, node)) = field
            && self.verified()
            && node.states.editable
            && node.value.is_some()
        {
            let unchanged = |e: &Self| {
                e.node_by_index(&app, i)
                    .ok()
                    .is_some_and(|n| n.value.is_some() && n.value == node.value)
            };
            // Never typed again automatically: many apps update what they
            // report a moment late, and typing twice would enter the text
            // twice (or send a message twice).
            if unchanged(self) {
                msg.push_str(TYPED_UNCONFIRMED_NOTE);
            }
        }
        Ok(ToolOutput::text(msg))
    }

    /// Type `text` into the app's focused element (newlines press Return).
    fn type_into_focus(&mut self, app: &AppInfo, text: &str) -> Result<()> {
        // Every path that types (set_value's fallback too) has the limit.
        let count = text.chars().count();
        if count > MAX_TYPED_CHARS {
            return Err(Error::InvalidArgs(format!(
                "{count} characters is too many to type; at most {MAX_TYPED_CHARS} (paste long text: set_clipboard, then press_key ctrl+v / cmd+v)"
            )));
        }
        let target = self.input_target(app)?;
        // A Windows line break is one Return, not two.
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        // Split on newlines so each becomes a Return press (works everywhere).
        let mut first = true;
        for segment in text.split('\n') {
            if !first {
                if self.halted() {
                    return Err(self.stopped_error());
                }
                self.backend.press_key(
                    &target,
                    &KeyCombo {
                        modifiers: keys::Modifiers::default(),
                        key: Key::Named(NamedKey::Return),
                    },
                )?;
            }
            // Long text goes in pieces, checking the stop key between them.
            let mut rest = segment;
            while !rest.is_empty() {
                let cut = rest
                    .char_indices()
                    .nth(TYPE_CHUNK)
                    .map_or(rest.len(), |(i, _)| i);
                let (piece, after) = rest.split_at(cut);
                if self.halted() {
                    return Err(self.stopped_error());
                }
                self.backend.type_text(&target, piece)?;
                rest = after;
            }
            first = false;
        }
        Ok(())
    }

    /// Give an element keyboard focus; a text field that won't take focus
    /// through accessibility is clicked instead (when retries are on).
    fn focus_element(&mut self, app: &AppInfo, handle: ElementHandle, node: &Node) -> Result<()> {
        let ocr = crate::ocr::is_ocr(handle);
        let focused = !ocr && matches!(self.backend.focus(handle), Ok(Native::Done(_)));
        // Text read off the screen is focused by clicking it.
        if !focused
            && (ocr || self.store.config.verify.retry && node.states.editable)
            && let Some(p) = node.bounds.filter(|b| !b.is_empty()).map(|b| b.center())
        {
            let target = self.input_target(app)?;
            self.backend.click(&target, p, MouseButton::Left, 1)?;
        }
        Ok(())
    }

    /// Replace a text field's contents by typing: focus it, select all, type.
    fn retype(
        &mut self,
        app: &AppInfo,
        handle: ElementHandle,
        node: &Node,
        text: &str,
    ) -> Result<()> {
        self.focus_element(app, handle, node)?;
        if self.backend.select_text(handle, None, 1).is_err() {
            let target = self.input_target(app)?;
            self.backend
                .press_key(&target, &keys::parse_combo("primary+a")?)?;
        }
        self.type_into_focus(app, text)
    }

    /// Whether element `index` now holds `want` (after a set_value). `None`
    /// when it can't be told (value not exposed, hidden for privacy, gone).
    fn value_took(&self, app: &AppInfo, index: u32, want: &str) -> Option<bool> {
        let n = self.node_by_index(app, index).ok()?;
        if crate::privacy::is_password(&n.role) {
            return None;
        }
        let truthy = |s: &str| {
            matches!(
                s.trim().to_lowercase().as_str(),
                "true" | "1" | "on" | "checked" | "yes"
            )
        };
        if let Some(c) = n.states.checked {
            return Some(c == truthy(want));
        }
        let got = n.value.as_deref()?;
        if let (Ok(a), Ok(b)) = (got.trim().parse::<f64>(), want.trim().parse::<f64>()) {
            return Some((a - b).abs() <= 1e-6_f64.max(b.abs() * 0.01));
        }
        let cfg = &self.store.config.privacy;
        // Compare the way the value is shown (card numbers are masked).
        let want = if cfg.redact_card_numbers {
            crate::privacy::mask_card_numbers(want).unwrap_or_else(|| want.to_string())
        } else {
            want.to_string()
        };
        if got.contains(crate::privacy::MASK) && !want.contains(crate::privacy::MASK) {
            return None;
        }
        Some(got.trim_end() == want.trim_end())
    }

    /// Re-inspect an app after a mutating action and append what changed
    /// (capped at `tree.report_changes_max_lines`): a diff, the new screen,
    /// or that the model is back on a screen it has seen.
    pub(super) fn append_changes(&mut self, app_query: &str, mut out: ToolOutput) -> ToolOutput {
        if out.is_error {
            return out;
        }
        let Ok(app) = self.resolve_app(app_query) else {
            return out;
        };
        // Settling already read the app back after the action (without
        // reading its text off the screen again).
        let ocr = self.states.get(&app.pid).is_some_and(|s| s.ocr_lines > 0);
        let fresh = self.settled != Some(self.epoch) || ocr;
        let Ok(window) = self.resolve_window(&app, None, fresh) else {
            return out;
        };
        if self.observe(&app, &window, fresh).is_err() {
            return out;
        }
        let Ok(r) = self.render(app.pid, false, None) else {
            return out;
        };
        if r.seen == Seen::Same && !r.full && r.changes == 0 {
            return out;
        }
        let title = match r.seen {
            Seen::Same if r.large_change => {
                "State after the action (large change, full tree):".to_string()
            }
            Seen::Same => "State after the action:".to_string(),
            Seen::New => format!(
                "State after the action: now on screen #{} (new), window \"{}\":",
                r.screen, window.title
            ),
            Seen::Revisit => format!(
                "State after the action: back on screen #{} (seen before), window \"{}\":",
                r.screen, window.title
            ),
        };
        // How much of it to report ([tree] report). The model then hasn't
        // seen all of it: the next get_app_state reports it.
        use crate::config::Report;
        let text = match self.store.config.tree.report {
            Report::Brief => {
                let count = if r.full {
                    format!(
                        "{} element(s)",
                        self.state(app.pid).map(|s| s.nodes.len()).unwrap_or(0)
                    )
                } else {
                    format!("{} change(s)", r.changes)
                };
                let how = self.explain("report-brief", "; get_app_state shows them.", ".");
                out.text.push_str(&format!("\n\n{title} {count}{how}"));
                return out;
            }
            Report::Relevant if !r.full => {
                let (text, others) = self.relevant_changes(app.pid, &r.text);
                if others == 0 {
                    r.text.clone()
                } else {
                    let max = self.store.config.tree.report_changes_max_lines.max(1);
                    let lines: Vec<&str> = text.lines().take(max).collect();
                    let how = self.explain("report-others", "; get_app_state shows them", "");
                    out.text.push_str(&format!(
                        "\n\n{title}\n{}\n[{others} other change(s) elsewhere{how}]",
                        lines.join("\n")
                    ));
                    return out;
                }
            }
            _ => r.text.clone(),
        };
        let max = self.store.config.tree.report_changes_max_lines.max(1);
        let lines: Vec<&str> = text.lines().collect();
        out.text.push_str("\n\n");
        out.text.push_str(&title);
        out.text.push('\n');
        if lines.len() > max {
            out.text.push_str(&lines[..max].join("\n"));
            let how = self.explain("report-more", "; call get_app_state for the rest", "");
            out.text
                .push_str(&format!("\n[+{} more lines{how}]", lines.len() - max));
            // The model hasn't seen all of it: the next get_app_state
            // reports against what it had seen before, and sends only the
            // rest if nothing changed (when the model reads this result
            // itself, not a script).
            if self.depth == 1 && !self.in_script {
                self.partial_report = Some((app.pid, r.screen, text_hash(&r.text), max));
            }
        } else {
            out.text.push_str(r.text.trim_end());
            self.commit(app.pid, &window);
        }
        out
    }

    /// The lines of a change report about what the action acted on: the
    /// changes in its container (its parent, or grandparent when the parent
    /// holds little), added elements, the focused element; and how many
    /// other changes there are.
    pub(super) fn relevant_changes(&self, pid: u32, text: &str) -> (String, usize) {
        let Ok(st) = self.state(pid) else {
            return (text.to_string(), 0);
        };
        let nodes = &st.nodes;
        let mut keep: HashSet<u32> = nodes
            .iter()
            .filter(|n| n.states.focused)
            .map(|n| n.index)
            .collect();
        if let Some((_, t)) = self.target.filter(|(p, _)| *p == pid)
            && let Some(pos) = nodes.iter().position(|n| n.index == t)
        {
            let size = |p: usize| {
                nodes[p + 1..]
                    .iter()
                    .take_while(|n| n.depth > nodes[p].depth)
                    .count()
            };
            let mut container = nodes[pos].parent.unwrap_or(pos);
            if size(container) < 4
                && let Some(g) = nodes[container].parent
            {
                container = g;
            }
            let end = container + 1 + size(container);
            keep.extend(nodes[container..end].iter().map(|n| n.index));
        }
        let mut out = String::new();
        let mut others = 0;
        for (i, line) in text.lines().enumerate() {
            // render()'s note on elements that keep changing: not an element.
            let restless = line.contains(" element(s) that keep changing on their own");
            let changed = line
                .strip_prefix("~ ")
                .filter(|_| !restless)
                .and_then(|l| l.split_whitespace().next())
                .and_then(|t| t.parse::<u32>().ok());
            let removed = line.starts_with("- ");
            let shown = i == 0
                || restless
                || line.starts_with("+ ")
                || line.starts_with("in ")
                || changed.is_some_and(|x| keep.contains(&x))
                || (changed.is_none() && !removed && !line.starts_with("~ "));
            if shown {
                out.push_str(line);
                out.push('\n');
            } else if let Some(rest) = line.strip_prefix("- ")
                && let Some((n, _)) = rest.split_once(" removed:")
            {
                others += n.parse::<usize>().unwrap_or(1);
            } else {
                others += 1;
            }
        }
        (out, others)
    }

    // -- describers --------------------------------------------------------

    fn node_for_handle(&self, app: &AppInfo, handle: ElementHandle) -> Option<&Node> {
        self.states
            .get(&app.pid)
            .and_then(|s| s.nodes.iter().find(|n| n.handle == handle))
    }

    pub(super) fn describe(&self, app: &AppInfo, handle: ElementHandle) -> String {
        self.node_for_handle(app, handle)
            .map(|n| n.label())
            .unwrap_or_else(|| "the element".into())
    }

    fn describe_anchor(&self, app: &AppInfo, anchor: &Anchor) -> String {
        match anchor {
            Anchor::Element(h) => self.describe(app, *h),
            Anchor::Point(_) => "the point".into(),
        }
    }
}

/// Elements read by OCR exist only as pictures: they can be clicked, not
/// set, selected or asked to do something.
fn ocr_can_only_be_clicked(handle: ElementHandle, tool: &str) -> Result<()> {
    if crate::ocr::is_ocr(handle) {
        return Err(Error::InvalidArgs(format!(
            "this element was read off the screen (OCR), so {tool} can't work on it; click it by element_index (and type after clicking) instead"
        )));
    }
    Ok(())
}

/// Appended when typed text doesn't show in the field (yet).
const TYPED_UNCONFIRMED_NOTE: &str = " Note: the field doesn't show the new text yet. Look (get_app_state) before typing again: typing again could enter the text twice.";

/// Appended when an action changed nothing that can be seen.
const NO_CHANGE_NOTE: &str = " Nothing on screen changed after it; check (get_app_state, screenshot=true) before repeating it.";
