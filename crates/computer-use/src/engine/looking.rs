//! Looking at apps: `list_apps`, `launch_app` and `get_app_state`, with the parts of a picture that changed.

use super::*;

/// [screenshot] smart: automatic pictures left out in a row before one is
/// sent anyway.
const SMART_MAX_LEFT_OUT: u8 = 3;

impl<B: Backend> Engine<B> {
    pub(super) fn list_apps(&mut self) -> Result<ToolOutput> {
        let apps = self.find_apps()?;
        self.set_structured(|| {
            let apps: Vec<serde_json::Value> = apps
                .iter()
                .map(|a| {
                    serde_json::json!({"name": a.name, "id": a.id, "pid": a.pid,
                        "frontmost": a.frontmost, "hidden": a.hidden})
                })
                .collect();
            serde_json::json!({ "apps": apps })
        });
        if apps.is_empty() {
            return Ok(ToolOutput::text("No GUI apps are currently running."));
        }
        let mut lines = vec![format!("{} running apps:", apps.len())];
        for a in &apps {
            let mut tags = Vec::new();
            if a.frontmost {
                tags.push("frontmost".to_string());
            }
            if a.hidden {
                tags.push("hidden".to_string());
            }
            let tags = if tags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", tags.join(", "))
            };
            lines.push(format!(
                "- {} (id: {}, pid: {}){}",
                a.name, a.id, a.pid, tags
            ));
        }
        // What this desktop doesn't allow, said once.
        if let Some(note) = self.backend.session_note()
            && self.hints.first("session-note")
        {
            lines.push(format!("\nNote: {note}"));
        }
        Ok(ToolOutput::text(lines.join("\n")))
    }

    pub(super) fn launch_app(&mut self, mut args: LaunchAppArgs) -> Result<ToolOutput> {
        // An app to open, not a command line: backends start exactly this
        // program with no arguments, and nothing that could read as an option.
        args.app = args.app.trim().to_string();
        if args.app.is_empty() || args.app.starts_with('-') || args.app.contains(char::is_control) {
            return Err(Error::InvalidArgs(
                "`app` must be an app name, bundle id or executable (no options or arguments)"
                    .into(),
            ));
        }
        // A web address: open it in the default browser.
        if crate::launch::is_url(&args.app) {
            crate::launch::open_url(&args.app)?;
            return Ok(ToolOutput::text(format!(
                "Opened \"{}\" in the default browser. Call list_apps, then get_app_state on the browser, to see it.",
                args.app
            )));
        }
        // Nothing a script wrote or downloaded: a program in the scripts'
        // folder would run whatever the web gave it (on Windows a .bat or
        // .exe runs as it is).
        if args.app.contains(['/', '\\']) {
            let real = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
            let path = match args.app.strip_prefix("~/").or(args.app.strip_prefix("~\\")) {
                Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
                None => std::path::PathBuf::from(&args.app),
            };
            let scripts = self.store.config.script.library();
            if real(&path).starts_with(real(&scripts)) {
                return Err(Error::ActionFailed(format!(
                    "{} is in the scripts' folder: programs there are not started (a script may have written it)",
                    args.app
                )));
            }
        }
        let before: HashSet<u32> = self.find_apps()?.iter().map(|a| a.pid).collect();
        let program = self.backend.launch_app(&args.app)?;

        let deadline = (self.clock)()
            + Duration::from_secs_f64(self.store.config.launch_timeout_secs.clamp(0.5, 3600.0));
        // The app shows up under the name asked for, or (when the name was
        // looked up in the OS's app list) under the program that was run.
        let mut names = vec![args.app.to_lowercase()];
        names.extend(program.as_deref().map(crate::launch::program_key));
        let matches = |a: &AppInfo| {
            a.match_keys()
                .iter()
                .any(|k| names.iter().any(|n| k.contains(n.as_str())))
        };
        loop {
            let apps = self.find_apps()?;
            // Prefer a newly-appeared app that matches the query.
            let found = apps
                .iter()
                .find(|a| !before.contains(&a.pid) && matches(a))
                .or_else(|| apps.iter().find(|a| matches(a)));
            if let Some(app) = found {
                let app = app.clone();
                let launched = format!("Launched {} (id: {}, pid: {}).", app.name, app.id, app.pid);
                // Its first state, saving the get_app_state that follows.
                if self.store.config.tools.launch_look
                    && let Some(state) = self.first_look(&app, deadline)
                {
                    return Ok(ToolOutput {
                        text: format!("{launched}\n\n{}", state.text),
                        ..state
                    });
                }
                return Ok(ToolOutput::text(format!(
                    "{launched} Call get_app_state to see it."
                )));
            }
            if self.halted() {
                return Err(self.stopped_error());
            }
            if (self.clock)() >= deadline {
                return Ok(ToolOutput::text(format!(
                    "Requested launch of `{}`. It hasn't shown a window yet; call list_apps or get_app_state shortly.",
                    args.app
                )));
            }
            (self.sleep)(Duration::from_millis(200));
        }
    }

    /// A just-launched app's state, once it shows a window (by `deadline`).
    fn first_look(&mut self, app: &AppInfo, deadline: Instant) -> Option<ToolOutput> {
        loop {
            if self.list_windows(app, true).is_ok_and(|w| !w.is_empty()) {
                // Give it a moment to fill the window.
                self.settle_on(app);
                return self
                    .get_app_state(GetAppStateArgs {
                        app: app.pid.to_string(),
                        ..Default::default()
                    })
                    .ok();
            }
            if self.halted() || (self.clock)() >= deadline {
                return None;
            }
            (self.sleep)(Duration::from_millis(200));
        }
    }

    pub(super) fn get_app_state(&mut self, mut args: GetAppStateArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        // The model's choice of screenshots for this app, kept.
        if let Some(p) = args.pictures {
            self.states.entry(app.pid).or_default().pictures = Some(p);
        }
        // [cache] rebase_after_tokens: much said since this app's tree was
        // last sent whole, so the model may have lost what a diff refers to.
        let threshold = self.store.config.cache.rebase_after_tokens;
        let rebase = threshold > 0
            && !args.disable_diff
            && args.within.is_none()
            && self.states.get(&app.pid).is_some_and(|s| {
                s.stamped
                    && s.known.is_some()
                    && self.sent_tokens.saturating_sub(s.full_at) >= threshold
            });
        if rebase {
            args.disable_diff = true;
            args.screenshot.get_or_insert(true);
        }
        let window = self.resolve_window(&app, args.window.as_deref(), false)?;
        // Other agents on the desktop: this one's window in its part.
        let window = self.arrange(&app, window);
        self.tell_doing(&app);
        self.ctx.force_ocr = args.ocr;
        let observed = self.observe(&app, &window, args.ocr);
        self.ctx.force_ocr = false;
        observed?;
        if let Some(index) = args.within {
            return self.part_of_tree(&app, &window, index, args.max_tokens);
        }
        if let Some(about) = args.about.as_deref().filter(|a| !a.trim().is_empty()) {
            return self.about_parts(&app, &window, about, args.max_tokens);
        }
        let mut r = self.render(app.pid, args.disable_diff, args.max_tokens)?;
        if self.ctx.depth == 1 {
            if r.full {
                let key = self.screen_key(app.pid);
                self.note.looks_full.push(key);
            } else {
                let key = self.screen_key(app.pid);
                self.note.looks_diff.push(key);
            }
        }
        if r.full {
            let sent = self.sent_tokens;
            if let Some(st) = self.states.get_mut(&app.pid) {
                st.full_at = sent;
            }
        }
        // The action before this one showed the start of this very tree:
        // only the rest is sent.
        if let Some((pid, screen, hash, shown)) = self.partial_report.take()
            && !args.disable_diff
            && self.ctx.depth == 1
            && pid == app.pid
            && screen == r.screen
            && hash == text_hash(&r.text)
        {
            let rest: Vec<&str> = r.text.lines().skip(shown).collect();
            let intro = self.explain(
                "partial-rest",
                "The first lines of this tree are as the action's result showed them (unchanged since); the rest:",
                "Rest of the tree (the start is as the action's result showed it):",
            );
            r.text = format!("{intro}\n{}\n", rest.join("\n"));
        }
        let size_changed = self.commit(app.pid, &window);

        let mut header = format!(
            "App: {} ({}, pid {}) · window \"{}\" (id {}",
            app.name, app.id, app.pid, window.title, window.id
        );
        if let Some(b) = window.bounds {
            header.push_str(&format!(
                ", {:.0}x{:.0} at {:.0},{:.0}",
                b.width, b.height, b.x, b.y
            ));
        }
        header.push(')');
        // The app and window as the last look gave them: named only.
        let compact = self.store.config.tree.compact;
        if let Some(st) = self.states.get_mut(&app.pid) {
            let same = st.header_seen.as_deref() == Some(header.as_str());
            st.header_seen = Some(header.clone());
            if compact && same {
                header = format!("App: {} · window \"{}\"", app.name, window.title);
            }
        }
        header.push_str(&format!(" · screen #{}", r.screen));
        match r.seen {
            Seen::New => header.push_str(" (new)"),
            Seen::Revisit => header.push_str(" (seen before)"),
            Seen::Same => {}
        }
        if rebase {
            header.push_str(" · sent whole again (much has been said since it last was)");
        }
        let head_len = header.len();
        let (ocr_lines, blind) = self
            .state(app.pid)
            .map(|s| (s.ocr_lines, s.blind.len()))
            .unwrap_or((0, 0));
        if blind > 0 && !args.ocr {
            // Said on a screen's first view (and the first time at all).
            if r.seen != Seen::Same || self.explain_first("blind") {
                header.push_str("\nPart of this window has no accessibility information (a canvas or a picture): ");
                if ocr_lines > 0 {
                    let how = self.explain(
                        "ocr-elements",
                        " (\"ocr text\" elements: click them by element_index; they can't be set or selected)",
                        " (\"ocr text\")",
                    );
                    header.push_str(&format!(
                        "{ocr_lines} line(s) of its text were read off the screen{how}; the screenshot shows the rest."
                    ));
                } else {
                    header.push_str("the screenshot shows it.");
                }
            }
        } else if ocr_lines > 0 {
            // What OCR elements are is said once.
            let how = self.explain(
                "ocr-elements",
                " (\"ocr text\" elements: click them by element_index; they can't be set or selected)",
                " (\"ocr text\")",
            );
            header.push_str(&if args.ocr {
                format!("\nRead {ocr_lines} more line(s) of text off the screen{how}.")
            } else {
                format!(
                    "\nThis window has little accessibility information, so {ocr_lines} line(s) of text were read off the screen{how}."
                )
            });
        }
        if let Some(note) = &self.ocr_note
            && !self.ocr_note_shown
            && (args.ocr || blind > 0 || r.interactive < self.store.config.ocr.sparse_threshold)
        {
            header.push_str(&format!("\n[Text recognition unavailable: {note}]"));
            self.ocr_note_shown = true;
        }

        // Decide whether this view needs pixels (screenshot.attach): a screen
        // the model has no picture of, a big change, a returning screen whose
        // window changed size, or custom-drawn UI with little in the tree.
        // [screenshot] adaptive: no automatic picture of a well-described
        // window in an app where the model hasn't used pixels.
        let (looks, pixel_uses) = self
            .states
            .get_mut(&app.pid)
            .map(|s| {
                s.looks = s.looks.saturating_add(1);
                (s.looks, s.pixel_uses)
            })
            .unwrap_or((1, 0));
        let sparse = r.interactive < self.store.config.ocr.sparse_threshold;
        self.apps_log.note(&app.name, |a| {
            a.looks += 1;
            a.little_tree += u64::from(sparse || blind > 0);
            a.text_read += u64::from(ocr_lines > 0);
        });
        let now = (self.clock)();
        self.apps_log.flush(Some(now));
        let attach = self
            .state(app.pid)
            .ok()
            .and_then(|s| s.pictures)
            .unwrap_or(self.store.config.screenshot.attach);
        let unneeded = self.store.config.screenshot.adaptive
            && attach == AttachMode::Auto
            && looks > 3
            && pixel_uses == 0
            && r.interactive >= self.store.config.screenshot.auto_sparse_threshold.max(3)
            && blind == 0
            && ocr_lines == 0;
        let shot = &self.store.config.screenshot;
        let allowed = shot.enabled && !self.store.config.text_only;
        let known = self
            .state(app.pid)?
            .known
            .as_ref()
            .ok_or(Error::Internal("no known screen".into()))?;
        let auto = match attach {
            AttachMode::Always => true,
            AttachMode::Never => false,
            AttachMode::Auto => {
                !known.shot
                    || r.large_change
                    || size_changed
                    || r.interactive < shot.auto_sparse_threshold
                    // Something changed, or the app came back to an
                    // earlier screen: look at the pixels rather than
                    // assume the model's picture still fits (an unchanged
                    // picture isn't sent again).
                    || r.changes > 0
                    || r.seen == Seen::Revisit
                    // What changes in an area the tree says nothing
                    // about only the pixels show.
                    || blind > 0
            }
        };
        let mut want = allowed && args.screenshot.unwrap_or(auto && !unneeded);
        // A picture held back only because the model hasn't needed any here.
        let withheld = allowed && args.screenshot.is_none() && auto && unneeded;
        let force = args.screenshot == Some(true);
        let mut image = None;
        let mut stale: Option<Changed> = None;
        // [screenshot] smart: an automatic picture whose only news is what
        // the tree reports changed (a number, a line read off the screen)
        // is left out, unless the model has to see what it did: an action
        // at x/y, an `expect` not met, or 3 left out in a row.
        let (pixel_action, expect_missed, left_out) = self
            .state(app.pid)
            .map(|s| (s.pixel_action, s.expect_missed, s.left_out))
            .unwrap_or_default();
        let candidate = want
            && args.screenshot.is_none()
            && attach == AttachMode::Auto
            && self.store.config.screenshot.smart
            && known.shot
            && r.seen == Seen::Same
            && !r.full
            && !r.large_change
            && !size_changed
            && r.changes > 0
            && !r.touched.is_empty()
            && !pixel_action
            && !expect_missed
            && left_out < SMART_MAX_LEFT_OUT;
        let mut captured_early = None;
        if candidate {
            // The picture just read for OCR, if any, is the screenshot.
            let cap = match self.ctx.last_capture.take() {
                Some((pid, wid, epoch, cap))
                    if pid == app.pid && wid == window.id && epoch == self.epoch =>
                {
                    Ok(cap)
                }
                _ => self.capture_clean(|b| b.capture(&app, &window)),
            };
            if let Ok(mut cap) = cap {
                // Compared with the last picture as it was sent: private
                // areas blacked out (covering them again later is no
                // change).
                self.redact_capture(&mut cap);
                if self.change_in_tree(app.pid, r.screen, &cap, &r.touched) {
                    want = false;
                    if let Some(st) = self.states.get_mut(&app.pid) {
                        st.left_out = st.left_out.saturating_add(1);
                    }
                    self.apps_log.note(&app.name, |a| a.pictures_left_out += 1);
                    header.push_str(self.explain(
                        "shot-in-tree",
                        "\nScreenshot: not sent: all that changed on screen is what the tree reports changed (screenshot=true sends one; pictures=\"always\" sends one every time in this app).",
                        "\nScreenshot: not sent (the change is in the tree).",
                    ));
                }
                captured_early = Some(cap);
            }
        }
        if want {
            // The picture just read for OCR, if any, is the screenshot.
            let reuse = match captured_early.take() {
                Some(cap) => Some(cap),
                None => match self.ctx.last_capture.take() {
                    Some((pid, wid, epoch, cap))
                        if pid == app.pid && wid == window.id && epoch == self.epoch =>
                    {
                        Some(cap)
                    }
                    _ => None,
                },
            };
            let captured = match reuse {
                Some(cap) => Ok(cap),
                None => self.capture_clean(|b| b.capture(&app, &window)),
            };
            match captured {
                Ok(mut cap) => {
                    if imaging::uniform(&cap) {
                        header.push_str(
                            "\n[The screenshot is one flat colour: the app may not draw while its window is in the background or minimized. Use the tree, or bring it forward (window action=focus) and look again.]",
                        );
                    }
                    let redacted = self.redact_capture(&mut cap);
                    if redacted > 0 {
                        header.push_str(&format!(
                            "\n[{redacted} private area(s) blacked out of the screenshot]"
                        ));
                    }
                    // Attached on its own to a window the tree already
                    // describes well: an overview is enough, and costs a
                    // fraction of the image tokens.
                    let shot_cfg = &self.store.config.screenshot;
                    let overview = args.screenshot.is_none()
                        && shot_cfg.overview_max_dimension > 0
                        && shot_cfg.overview_max_dimension < shot_cfg.max_dimension
                        && r.interactive >= shot_cfg.auto_sparse_threshold.max(1)
                        && self.state(app.pid).is_ok_and(|s| s.ocr_lines == 0);
                    match self.window_picture(app.pid, r.screen, cap, force, overview) {
                        Ok(Picture::Unchanged { base }) => {
                            self.saw_pixels(app.pid);
                            let at = base.map(|b| format!(" (#{b})")).unwrap_or_default();
                            header.push_str(&if self.explain_first("shot-unchanged") {
                                format!(
                                    "\nScreenshot: unchanged since you last saw it{at}, not re-sent (screenshot=true forces one)."
                                )
                            } else {
                                format!("\nScreenshot: unchanged{at}, not re-sent.")
                            });
                        }
                        Ok(Picture::Part {
                            img,
                            id,
                            base,
                            at,
                            changed,
                        }) => {
                            stale = changed;
                            self.saw_pixels(app.pid);
                            self.apps_log.note(&app.name, |a| a.pictures += 1);
                            if self.ctx.depth == 1 {
                                let key = self.screen_key(app.pid);
                                self.note.pictures_part.push(key);
                            }
                            let (w, h, (ox, oy)) = (img.width, img.height, at);
                            let (x1, y1) = (ox + w, oy + h);
                            header.push_str(&match base {
                                Some(b) if self.explain_first("shot-part") => format!(
                                    "\nScreenshot #{id}: only the part that changed, {w}x{h} px: the area x {ox}–{x1}, y {oy}–{y1} of screenshot #{b}, your earlier one of this screen (same scale; the rest is unchanged, and x/y coordinates still refer to that whole screenshot)."
                                ),
                                Some(b) => format!(
                                    "\nScreenshot #{id}: changed part only, the area x {ox}–{x1}, y {oy}–{y1} of #{b} (x/y still refer to #{b})."
                                ),
                                None => format!(
                                    "\nScreenshot #{id}: only the part that changed, {w}x{h} px: the area x {ox}–{x1}, y {oy}–{y1} of your earlier screenshot of this screen (same scale; the rest is unchanged, and x/y coordinates still refer to that whole screenshot)."
                                ),
                            });
                            image = Some(img);
                        }
                        Ok(Picture::Whole {
                            img,
                            id,
                            overview,
                            changed,
                        }) => {
                            stale = changed;
                            self.saw_pixels(app.pid);
                            self.apps_log.note(&app.name, |a| a.pictures += 1);
                            if self.ctx.depth == 1 {
                                let key = self.screen_key(app.pid);
                                self.note.pictures_whole.push(key);
                            }
                            header.push_str(&format!(
                                "\nScreenshot #{id}: {}x{} px.",
                                img.width, img.height
                            ));
                            if overview {
                                header.push_str(self.explain(
                                    "overview",
                                    " (An overview; pass screenshot=true for full detail, or screenshot(element_index) to zoom into one element.)",
                                    " (overview)",
                                ));
                            }
                            image = Some(img);
                        }
                        Err(e) => {
                            self.mark_shot(app.pid);
                            header.push_str(&format!("\n[screenshot encode failed: {e}]"));
                        }
                    }
                }
                Err(e) => {
                    self.mark_shot(app.pid);
                    header.push_str(&format!("\n[screenshot unavailable: {e}]"));
                }
            }
        } else if allowed && !(candidate && captured_early.is_some()) {
            header.push_str(if withheld && self.explain_first("shot-unneeded") {
                "\nScreenshot: not attached (you haven't needed pictures in this app; screenshot=true for one)."
            } else {
                self.explain(
                    "shot-none",
                    "\nScreenshot: not attached (pass screenshot=true for one).",
                    "\nScreenshot: not attached.",
                )
            });
        }

        // [screenshot] icon_sprite: no screenshot, but buttons without a
        // name: a strip of what they look like.
        if image.is_none()
            && allowed
            && self.store.config.screenshot.icon_sprite
            && let Some((img, listed)) = self.icon_strip(&app, &window)
        {
            header.push_str(&format!(
                "\nIcons of buttons without a name, numbered with their element_index: {listed}."
            ));
            image = Some(img);
        }
        // The pixels changed where the tree reports nothing (a toolkit that
        // doesn't tell accessibility what it redrew): say where.
        if let Some(c) = stale
            && r.seen == Seen::Same
            && !r.full
            && r.removed == 0
            && blind == 0
            && c.share >= 0.1
            && !r.touched.iter().any(|t| t.intersects(&c.screen))
        {
            let (x, y, w, h) = c.shot;
            header.push_str(&format!(
                "\n[The picture changed at x {x}–{}, y {y}–{} where the tree reports no change: the tree may be out of date there; trust the screenshot.]",
                x + w,
                y + h
            ));
        }
        // Nothing new at all: one line says so.
        let quiet = compact
            && r.seen == Seen::Same
            && r.changes == 0
            && !r.full
            && image.is_none()
            && header[head_len..]
                .lines()
                .filter(|l| !l.is_empty())
                .all(|l| {
                    l.starts_with("Screenshot: unchanged")
                        || l.starts_with("Screenshot: not attached")
                });
        if quiet {
            let mut text = format!(
                "{}: nothing changed since your last look (tree and screenshot)",
                &header[..head_len]
            );
            if !r.restless.is_empty() {
                text.push_str(&format!(
                    ", but for {} element(s) that keep changing on their own: {}",
                    r.restless.len(),
                    tree::ranges(&mut r.restless.clone())
                ));
            }
            text.push('.');
            return Ok(ToolOutput::text(text));
        }
        Ok(ToolOutput {
            text: format!("{header}\nTree:\n{}", r.text),
            image,
            is_error: false,
        })
    }

    /// The model got (or was told it has) a current picture of this app:
    /// what [screenshot] smart counted since starts again.
    pub(super) fn saw_pixels(&mut self, pid: u32) {
        if let Some(st) = self.states.get_mut(&pid) {
            st.pixel_action = false;
            st.expect_missed = false;
            st.left_out = 0;
        }
    }

    /// Whether every part of the window whose pixels changed since the
    /// model's picture of `screen` holds an element the tree reports added
    /// or changed (`touched`, those small enough to say what changed in
    /// them): then the tree already says what changed.
    /// False when it can't tell (no picture of this screen at this size).
    fn change_in_tree(&self, pid: u32, screen: u32, cap: &Capture, touched: &[Rect]) -> bool {
        let cache = &self.store.config.cache;
        let Some((known, coord)) = self
            .states
            .get(&pid)
            .and_then(|s| s.known.as_ref())
            .filter(|k| k.id == screen)
            .and_then(|k| Some((k.pixels.clone()?, k.coord?)))
        else {
            return false;
        };
        if coord.bounds != cap.bounds {
            return false;
        }
        let sig = PixelSig::of(cap, cache.pixel_grid);
        let Some(cells) = sig.changed_cells(&known, cache.pixel_tolerance) else {
            return false;
        };
        if cells.is_empty() {
            return false;
        }
        // A big element that changed (the window's title, a document)
        // doesn't say what changed inside it.
        let window_area = cap.bounds.width * cap.bounds.height;
        let small: Vec<Rect> = touched
            .iter()
            .filter(|t| t.width * t.height <= 0.25 * window_area)
            .map(|t| Rect::new(t.x - 4.0, t.y - 4.0, t.width + 8.0, t.height + 8.0))
            .collect();
        let sx = cap.bounds.width / f64::from(cap.width.max(1));
        let sy = cap.bounds.height / f64::from(cap.height.max(1));
        cells.iter().all(|&(x, y, w, h)| {
            let cell = Rect::new(
                cap.bounds.x + f64::from(x) * sx,
                cap.bounds.y + f64::from(y) * sy,
                f64::from(w) * sx,
                f64::from(h) * sy,
            );
            small.iter().any(|t| t.intersects(&cell))
        })
    }

    /// A strip of the buttons in the latest snapshot that have no name and
    /// haven't been shown yet, each numbered with its index.
    fn icon_strip(&mut self, app: &AppInfo, window: &WindowInfo) -> Option<(EncodedImage, String)> {
        let st = self.states.get(&app.pid)?;
        let icons: Vec<(u32, Rect, u64)> = st
            .nodes
            .iter()
            .filter(|n| {
                crate::roles::is_interactive(&n.role)
                    && n.name.is_none()
                    && !n.line.contains(" desc=")
                    && !st.icons_shown.contains(&n.key)
            })
            .filter_map(|n| {
                let b = n.bounds?;
                (b.width >= 4.0 && b.height >= 4.0 && b.width <= 96.0 && b.height <= 96.0)
                    .then_some((n.index, b, n.key))
            })
            .take(24)
            .collect();
        if icons.is_empty() {
            return None;
        }
        let reuse = match self.ctx.last_capture.take() {
            Some((pid, wid, epoch, cap))
                if pid == app.pid && wid == window.id && epoch == self.epoch =>
            {
                Some(cap)
            }
            _ => None,
        };
        let mut cap = match reuse {
            Some(c) => c,
            None => self.capture_clean(|b| b.capture(app, window)).ok()?,
        };
        self.redact_capture(&mut cap);
        let crops: Vec<(u32, Capture)> = icons
            .iter()
            .filter_map(|(i, b, _)| {
                crate::coverage::pixels_of(&cap, *b).map(|px| (*i, imaging::crop(&cap, px)))
            })
            .collect();
        if crops.is_empty() {
            return None;
        }
        let strip = imaging::strip(&crops, 28);
        let (img, _) = imaging::encode(strip, &self.store.config.screenshot).ok()?;
        let mut listed: Vec<u32> = crops.iter().map(|(i, _)| *i).collect();
        if let Some(st) = self.states.get_mut(&app.pid) {
            st.icons_shown.extend(icons.iter().map(|(_, _, k)| *k));
        }
        Some((img, tree::ranges(&mut listed)))
    }

    /// One element and what is in it (get_app_state within=index): a look
    /// that leaves what later diffs are against as it was.
    fn part_of_tree(
        &self,
        app: &AppInfo,
        window: &WindowInfo,
        index: u32,
        max_tokens: Option<usize>,
    ) -> Result<ToolOutput> {
        let st = self.state(app.pid)?;
        let pos = st
            .nodes
            .iter()
            .position(|n| n.index == index)
            .ok_or_else(|| {
                Error::InvalidArgs(format!(
                    "unknown element_index {index}: call get_app_state for the current indices"
                ))
            })?;
        let base = st.nodes[pos].depth;
        let end = pos
            + 1
            + st.nodes[pos + 1..]
                .iter()
                .take_while(|n| n.depth > base)
                .count();
        let part: Vec<Node> = st.nodes[pos..end]
            .iter()
            .map(|n| {
                let mut n = n.clone();
                n.depth -= base;
                n.parent = n.parent.filter(|p| *p >= pos).map(|p| p - pos);
                n
            })
            .collect();
        let tcfg = &self.store.config.tree;
        let mut budget = tree::Budget::from_config(tcfg);
        if let Some(tokens) = max_tokens {
            budget.tokens = tokens;
        }
        let text = tree::render_full_within(&part, tcfg.indent, budget);
        Ok(ToolOutput::text(format!(
            "App: {} · window \"{}\" · element {index} and what is in it ({} element(s)):\n{text}",
            app.name,
            window.title,
            part.len()
        )))
    }

    /// What the model gets of a window's pixels, given what it already has
    /// of `screen`: nothing when that picture is current, only the part
    /// that changed when it is small, else the whole window, which then is
    /// the picture x/y refer to. get_app_state and screenshot(app) share it.
    /// `force` (asked for explicitly): always the whole window.
    pub(super) fn window_picture(
        &mut self,
        pid: u32,
        screen: u32,
        cap: Capture,
        force: bool,
        overview: bool,
    ) -> Result<Picture> {
        let cache = self.store.config.cache.clone();
        let mut shot_cfg = self.store.config.screenshot.clone();
        let (known_pixels, known_coord, base) = self
            .states
            .get(&pid)
            .and_then(|s| s.known.as_ref())
            .filter(|k| k.id == screen)
            .map(|k| (k.pixels.clone(), k.coord, k.shot_id))
            .unwrap_or_default();
        // Pixel fingerprints tell unchanged pictures and changed parts apart.
        let fingerprint =
            cache.dedupe_screenshots || shot_cfg.scope == crate::config::ShotScope::Auto;
        let sig = fingerprint.then(|| PixelSig::of(&cap, cache.pixel_grid));
        let unchanged = !force
            && cache.dedupe_screenshots
            && known_coord.is_some_and(|c| c.bounds == cap.bounds)
            && matches!((&sig, &known_pixels), (Some(a), Some(b)) if a.same_as(b, cache.pixel_tolerance));
        // Where it changed, in screen and screenshot terms.
        let changed = match (&sig, &known_pixels, known_coord) {
            (Some(a), Some(b), Some(c)) if c.bounds == cap.bounds => a
                .changed_area(b, cache.pixel_tolerance)
                .map(|(x, y, w, h)| {
                    let sx = cap.bounds.width / f64::from(cap.width.max(1));
                    let sy = cap.bounds.height / f64::from(cap.height.max(1));
                    let screen = Rect::new(
                        cap.bounds.x + f64::from(x) * sx,
                        cap.bounds.y + f64::from(y) * sy,
                        f64::from(w) * sx,
                        f64::from(h) * sy,
                    );
                    let kx = f64::from(c.width) / f64::from(cap.width.max(1));
                    let ky = f64::from(c.height) / f64::from(cap.height.max(1));
                    let shot = (
                        (f64::from(x) * kx) as u32,
                        (f64::from(y) * ky) as u32,
                        (f64::from(w) * kx).ceil() as u32,
                        (f64::from(h) * ky).ceil() as u32,
                    );
                    let share = f64::from(w) * f64::from(h)
                        / (f64::from(cap.width) * f64::from(cap.height)).max(1.0);
                    Changed {
                        screen,
                        shot,
                        share,
                    }
                }),
            _ => None,
        };
        if unchanged {
            // The model already has this picture.
            if let Some(st) = self.states.get_mut(&pid) {
                st.coord = known_coord;
            }
            return Ok(Picture::Unchanged { base });
        }
        if !force
            && let Some(part) =
                self.changed_part(&cap, sig.as_ref(), known_pixels.as_ref(), known_coord)
            && let Some(full) = known_coord
            && let Ok((img, at)) = imaging::encode_part(&cap, part, &full, &shot_cfg)
        {
            // Only the part that changed, placed in the picture the model
            // already has.
            let id = self.next_shot();
            self.ctx.pending_images.push(PendingImage {
                pid,
                screen,
                coord: full,
                pixels: sig,
                id: base.unwrap_or(id),
            });
            return Ok(Picture::Part {
                img,
                id,
                base,
                at,
                changed,
            });
        }
        if overview {
            shot_cfg.max_dimension = shot_cfg.overview_max_dimension;
        }
        let (img, map) = imaging::encode(cap, &shot_cfg)?;
        let id = self.next_shot();
        self.ctx.pending_images.push(PendingImage {
            pid,
            screen,
            coord: map,
            pixels: sig,
            id,
        });
        Ok(Picture::Whole {
            img,
            id,
            overview,
            changed,
        })
    }

    /// With `screenshot.scope = "auto"`: the pixel area of a new capture worth
    /// sending, when the model already has a picture of this screen at this
    /// size and only a small part of it changed.
    pub(super) fn changed_part(
        &self,
        cap: &Capture,
        sig: Option<&PixelSig>,
        known: Option<&PixelSig>,
        coord: Option<CoordMap>,
    ) -> Option<(u32, u32, u32, u32)> {
        let cfg = &self.store.config.screenshot;
        if cfg.scope != crate::config::ShotScope::Auto {
            return None;
        }
        coord.filter(|c| c.bounds == cap.bounds)?;
        let area = sig?.changed_area(known?, self.store.config.cache.pixel_tolerance)?;
        let part = imaging::widen(
            area,
            cfg.region_padding,
            cfg.region_min_size,
            cap.width,
            cap.height,
        );
        let share = f64::from(part.2) * f64::from(part.3)
            / (f64::from(cap.width) * f64::from(cap.height)).max(1.0);
        (share <= cfg.region_max_ratio).then_some(part)
    }

    /// The next screenshot's number.
    pub(super) fn next_shot(&mut self) -> u32 {
        self.shots += 1;
        self.shots
    }

    /// A screenshot of the known screen was attempted and failed: don't
    /// retry it on every view.
    fn mark_shot(&mut self, pid: u32) {
        if let Some(k) = self.states.get_mut(&pid).and_then(|s| s.known.as_mut()) {
            k.shot = true;
        }
    }

    /// Actions act on the window of the latest `get_app_state` (their
    /// element indices and x/y belong to it). A `window` argument naming
    /// another window is an error, not silently ignored.
    pub(super) fn check_window(&mut self, app: &AppInfo, window: Option<&str>) -> Result<()> {
        let Some(w) = window.map(str::trim).filter(|w| !w.is_empty()) else {
            return Ok(());
        };
        let wanted = self.resolve_window(app, Some(w), false)?;
        match self.states.get(&app.pid).and_then(|s| s.window_id) {
            Some(id) if id != wanted.id => Err(Error::InvalidArgs(format!(
                "the latest get_app_state of {} shows another window; call get_app_state with window=\"{w}\" first, then act on it.",
                app.name
            ))),
            _ => Ok(()),
        }
    }
}
