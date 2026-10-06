//! Reading a window: its tree, the text only its pixels have (OCR, blind areas), the rendered listing, and committing what the model was shown.

use super::*;

impl<B: Backend> Engine<B> {
    /// Snapshot `window` (or reuse a snapshot from within
    /// `cache.snapshot_ttl_ms` when no action ran since, unless `fresh`), work
    /// out which screen it shows, and number its elements. What the model is
    /// known to have seen is left alone (see [`Self::commit`]).
    /// Whether to read text off this window: a sparse tree (`ocr.mode`), or
    /// the agent asked.
    fn ocr_wanted(&self, raw: &[RawNode]) -> bool {
        use crate::config::OcrMode;
        let cfg = &self.store.config;
        if !cfg.screenshot.enabled {
            return false;
        }
        if self.ctx.force_ocr {
            return true;
        }
        match cfg.ocr.mode {
            OcrMode::Off => false,
            OcrMode::Always => true,
            OcrMode::Auto => {
                raw.iter()
                    .filter(|n| crate::roles::is_interactive(&n.role) || n.states.editable)
                    .count()
                    < cfg.ocr.sparse_threshold
            }
        }
    }

    /// The text on a window (OCR), reusing the last result when the picture
    /// hasn't changed (or, while settling, without looking again).
    fn read_screen_text(&mut self, app: &AppInfo, window: &WindowInfo) -> Vec<OcrLine> {
        let cached = self.states.get(&app.pid).and_then(|s| s.ocr_cache.clone());
        if self.ctx.ocr_reuse
            && let Some((_, lines)) = &cached
        {
            return lines.clone();
        }
        let cap = match self.capture_clean(|b| b.capture(app, window)) {
            Ok(c) => c,
            Err(e) => {
                log::debug!("no capture for OCR: {e}");
                return Vec::new();
            }
        };
        let cache = &self.store.config.cache;
        let sig = PixelSig::of(&cap, cache.pixel_grid);
        let lines = match cached {
            Some((old, lines)) if old.same_as(&sig, cache.pixel_tolerance) => lines,
            _ => {
                let mut lines = self.run_ocr(&cap);
                lines.retain(|l| !self.overlay_text(&l.text));
                lines
            }
        };
        self.states.entry(app.pid).or_default().ocr_cache = Some((sig, lines.clone()));
        self.ctx.last_capture = Some((app.pid, window.id, self.epoch, cap));
        lines
    }

    /// Whether text read off the screen is the on-screen indicator's own
    /// label (a picture can catch it where the platform can't leave it
    /// out): never the app's text.
    fn overlay_text(&self, text: &str) -> bool {
        let o = &self.store.config.overlay;
        if !o.enabled || self.overlay.is_none() {
            return false;
        }
        let read = crate::text::fold(text.trim().trim_end_matches(['…', '.']));
        if read.chars().count() < 4 {
            return false;
        }
        [
            &o.label_working,
            &o.label_thinking,
            &o.label_error,
            &o.label_done,
            &o.label_paused,
            &o.label_stopped,
        ]
        .iter()
        .any(|label| reads_as_start_of(&read, &crate::text::fold(label)))
    }

    /// Recognise the text in a capture with the configured engine.
    fn run_ocr(&mut self, cap: &Capture) -> Vec<OcrLine> {
        use crate::config::OcrEngineChoice;
        let cfg = self.store.config.ocr.clone();
        // As it is and enlarged, without grid lines: enlarging alone turns
        // big framed labels into one glyph.
        let tesseract = || {
            crate::ocr::tesseract_both(
                cap,
                &cfg.languages,
                &cfg.tesseract_path,
                crate::ocr::Layout::Sparse,
            )
        };
        let result = match cfg.engine {
            OcrEngineChoice::Native => self.backend.ocr(cap, &cfg.languages),
            OcrEngineChoice::Tesseract => tesseract(),
            OcrEngineChoice::Auto => match self.backend.ocr(cap, &cfg.languages) {
                Ok(lines) => Ok(lines),
                Err(e) => {
                    log::debug!("built-in OCR unavailable ({e}); trying Tesseract");
                    tesseract()
                }
            },
        };
        match result {
            Ok(lines) => {
                self.ocr_note = None;
                lines
            }
            Err(e) => {
                if self.ocr_note.is_none() {
                    log::warn!("text recognition unavailable: {e}");
                }
                self.ocr_note = Some(e.to_string());
                Vec::new()
            }
        }
    }

    pub(super) fn observe(
        &mut self,
        app: &AppInfo,
        window: &WindowInfo,
        fresh: bool,
    ) -> Result<()> {
        let ttl = Duration::from_millis(self.store.config.cache.snapshot_ttl_ms);
        if !fresh
            && let Some(s) = self.states.get(&app.pid)
            && s.stamped
            && s.window_id == Some(window.id)
            && s.snap_epoch == self.epoch
            && s.snap_at
                .is_some_and(|t| (self.clock)().saturating_duration_since(t) < ttl)
        {
            return Ok(());
        }

        let tcfg = &self.store.config.tree;
        let opts = SnapshotOptions {
            max_nodes: tcfg.max_walk,
            max_depth: tcfg.max_depth,
        };
        let mut raw = self.backend.snapshot(app, window, &opts)?;
        let snap_at = (self.clock)();
        // Custom-drawn UI: add the text read off the window, or off the
        // areas of it the tree says nothing about.
        let mut ocr_lines = 0;
        let mut blind = Vec::new();
        let lines = if raw.is_empty() {
            Vec::new()
        } else if self.ocr_wanted(&raw) {
            self.read_screen_text(app, window)
        } else if self.blind_wanted() {
            let (areas, lines) = self.read_blind_areas(app, window, &raw);
            blind = areas;
            lines
        } else {
            Vec::new()
        };
        if !lines.is_empty() {
            let cfg = &self.store.config.ocr;
            // Shapes read as glyphs, and rulers' numbers, aren't text
            // (judged with the confidence each line was read with).
            let lines: Vec<OcrLine> = lines
                .into_iter()
                .filter(|l| crate::ocr::plausible(l) && !crate::ocr::ruler(l))
                .collect();
            let mut extra = crate::ocr::nodes(&lines, cfg.min_confidence, cfg.max_lines);
            // Leave out what the tree already says there.
            extra.retain(|o| {
                let text = crate::ocr::words(o.name.as_deref().unwrap_or(""));
                crate::ocr::enough_text(&text)
                    && !raw.iter().any(|n| {
                        // The window itself overlaps everything: not a match.
                        let Some((a, b)) = n.bounds.zip(o.bounds) else {
                            return false;
                        };
                        if n.parent.is_none() || !a.intersects(&b) {
                            return false;
                        }
                        // A line-sized element (not a big container) whose
                        // text the line merely adds glyphs to also counts.
                        let line_sized = a.height <= 3.0 * b.height.max(8.0);
                        [&n.name, &n.value].into_iter().flatten().any(|t| {
                            let t = crate::ocr::words(t);
                            t.contains(&text)
                                || (line_sized && t.chars().count() >= 3 && text.contains(&t))
                        })
                    })
            });
            ocr_lines = extra.len();
            raw.extend(extra);
        }
        // Private data never reaches the model (or the screen memory).
        let private = if crate::privacy::active(&self.store.config.privacy) {
            crate::privacy::scrub(&mut raw, &self.store.config.privacy)
        } else {
            Vec::new()
        };
        // OCR read the picture before it was blacked out: text read inside
        // a private area (a code in a field that shows no value) is left
        // out of the tree too.
        if !private.is_empty() {
            let before = raw.len();
            raw.retain(|n| {
                !crate::ocr::is_ocr(n.handle)
                    || !n
                        .bounds
                        .is_some_and(|b| private.iter().any(|p| p.intersects(&b)))
            });
            ocr_lines = ocr_lines.saturating_sub(before - raw.len());
        }
        let pruned = tree::prune(&raw, window.bounds, &self.store.config.tree);
        drop(raw);
        let mut nodes = pruned.nodes;

        let cache = &self.store.config.cache;
        let ratio = self.store.config.tree.diff_full_ratio;
        let mut st = self.states.remove(&app.pid).unwrap_or_default();
        let same_window = st.screen != 0 && st.window_id == Some(window.id);

        // Which screen is this? A small change keeps the current screen;
        // otherwise look for the most similar screen: the current one (a big
        // update of the same layout), the one the model knows, or one it saw
        // earlier.
        let small = same_window
            && !tree::big_change(tree::change_count(&st.nodes, &nodes), nodes.len(), ratio);
        let screen = if small {
            st.screen
        } else {
            let shapes: HashSet<u64> = nodes.iter().map(|n| n.shape).collect();
            let threshold = cache.match_threshold;
            let mut best: Option<(u32, f64)> = None;
            if same_window {
                let sim = shape_similarity(&st.nodes, &shapes);
                if sim >= threshold {
                    best = Some((st.screen, sim));
                }
            }
            if cache.enabled {
                if let Some(k) = st.known.as_ref().filter(|k| k.id != st.screen) {
                    let sim = k.view.similarity(&shapes);
                    if sim >= threshold && best.is_none_or(|b| sim > b.1) {
                        best = Some((k.id, sim));
                    }
                }
                if let Some(m) = self
                    .memory
                    .best_match(app.pid, window.id, &shapes, threshold)
                    && best.is_none_or(|b| m.1 > b.1)
                {
                    best = Some(m);
                }
            }
            match best {
                Some((id, _)) => id,
                None => self.memory.new_id(),
            }
        };

        // Numbering: a screen seen before gets its old indices back.
        if screen != st.screen {
            let view = match &st.known {
                Some(k) if k.id == screen => Some(&k.view),
                _ => self.memory.get(screen).map(|s| &s.view),
            };
            match view {
                Some(v) => v.restore_indices(&mut st.alloc, &nodes),
                None if !same_window => st.alloc.clear_keys(),
                None => {}
            }
        }
        st.alloc.assign_stable(&mut nodes);

        st.bounds = nodes
            .iter()
            .filter_map(|n| n.bounds.map(|b| (n.handle, b)))
            .collect();
        st.window_id = Some(window.id);
        st.nodes = nodes;
        st.omitted = pruned.omitted;
        st.screen = screen;
        st.snap_at = Some(snap_at);
        st.snap_epoch = self.epoch;
        st.stamped = true;
        st.private = private;
        st.ocr_lines = ocr_lines;
        st.blind = blind;
        self.states.insert(app.pid, st);
        Ok(())
    }

    /// Look for areas the tree says nothing about ([ocr] blind_regions):
    /// pixels are needed to tell them from empty background.
    fn blind_wanted(&self) -> bool {
        let cfg = &self.store.config;
        cfg.ocr.blind_regions && cfg.screenshot.enabled && !cfg.text_only
    }

    /// The areas of a window no informative element covers that show
    /// something, and the text read off them (unless OCR is off). An
    /// unchanged picture isn't read again, and while settling after an
    /// action the last reading is kept.
    fn read_blind_areas(
        &mut self,
        app: &AppInfo,
        window: &WindowInfo,
        raw: &[RawNode],
    ) -> (Vec<Rect>, Vec<OcrLine>) {
        let Some(bounds) = window.bounds else {
            return Default::default();
        };
        let candidates = crate::coverage::uncovered(raw, bounds);
        if candidates.is_empty() {
            return Default::default();
        }
        let cached = self
            .states
            .get(&app.pid)
            .and_then(|s| s.blind_cache.clone());
        if self.ctx.ocr_reuse
            && let Some((_, areas, lines)) = &cached
        {
            return (areas.clone(), lines.clone());
        }
        let cap = match self.capture_clean(|b| b.capture(app, window)) {
            Ok(c) => c,
            Err(e) => {
                log::debug!("no capture for blind areas: {e}");
                return Default::default();
            }
        };
        let cache = &self.store.config.cache;
        let sig = PixelSig::of(&cap, cache.pixel_grid);
        let (areas, lines) = match cached {
            Some((old, areas, lines)) if old.same_as(&sig, cache.pixel_tolerance) => (areas, lines),
            _ => {
                let areas: Vec<Rect> = candidates
                    .into_iter()
                    .filter(|a| crate::coverage::shows_something(&cap, a))
                    .map(|a| a.rect)
                    .collect();
                let mut lines = Vec::new();
                if self.store.config.ocr.mode != crate::config::OcrMode::Off {
                    let before = self
                        .states
                        .get_mut(&app.pid)
                        .map(|s| std::mem::take(&mut s.area_reads))
                        .unwrap_or_default();
                    let mut reads = Vec::new();
                    for r in &areas {
                        if let Some(px) = crate::coverage::pixels_of(&cap, *r) {
                            let crop = imaging::crop(&cap, px);
                            let hash = pixels_hash(&crop);
                            let kept = before
                                .iter()
                                .find(|(rect, h, _)| rect == r && *h == hash)
                                .map(|(_, _, read)| read.clone());
                            let read: Vec<OcrLine> = match kept {
                                Some(read) => read,
                                None => self
                                    .run_area_ocr(&crop)
                                    .into_iter()
                                    // Only what is in the area (an engine
                                    // may read around it).
                                    .filter(|l| {
                                        let b = l.bounds;
                                        r.contains(Point::new(
                                            b.x + b.width / 2.0,
                                            b.y + b.height / 2.0,
                                        )) && crate::ocr::plausible(l)
                                            && !self.overlay_text(&l.text)
                                    })
                                    .collect(),
                            };
                            lines.extend(read.iter().cloned());
                            reads.push((*r, hash, read));
                        }
                    }
                    self.states.entry(app.pid).or_default().area_reads = reads;
                }
                (areas, lines)
            }
        };
        self.states.entry(app.pid).or_default().blind_cache =
            Some((sig, areas.clone(), lines.clone()));
        self.ctx.last_capture = Some((app.pid, window.id, self.epoch, cap));
        (areas, lines)
    }

    /// Read a part of a window. Tesseract reads it both enlarged and as it
    /// is: a part can hold big painted labels as well as small text.
    fn run_area_ocr(&mut self, cap: &Capture) -> Vec<OcrLine> {
        use crate::config::OcrEngineChoice;
        let cfg = self.store.config.ocr.clone();
        let layout = crate::ocr::Layout::for_height(cap.bounds.height);
        let tesseract =
            || crate::ocr::tesseract_both(cap, &cfg.languages, &cfg.tesseract_path, layout);
        let result = match cfg.engine {
            OcrEngineChoice::Native => self.backend.ocr(cap, &cfg.languages),
            OcrEngineChoice::Tesseract => tesseract(),
            OcrEngineChoice::Auto => self
                .backend
                .ocr(cap, &cfg.languages)
                .or_else(|_| tesseract()),
        };
        match result {
            Ok(lines) => lines,
            Err(e) => {
                if self.ocr_note.is_none() {
                    log::warn!("text recognition unavailable: {e}");
                }
                self.ocr_note = Some(e.to_string());
                Vec::new()
            }
        }
    }

    /// Render the latest snapshot against what the model has seen of that
    /// screen: a diff, the full tree, or a note that nothing changed.
    pub(super) fn render(
        &self,
        pid: u32,
        full: bool,
        max_tokens: Option<usize>,
    ) -> Result<Refreshed> {
        let st = self.state(pid)?;
        let tcfg = &self.store.config.tree;
        let (seen, base) = match &st.known {
            Some(k) if k.id == st.screen => (Seen::Same, Some(&k.view)),
            _ => match self.memory.get(st.screen) {
                Some(s) if self.store.config.cache.enabled => (Seen::Revisit, Some(&s.view)),
                _ => (Seen::New, None),
            },
        };
        let nodes = &st.nodes;
        let mut out = Refreshed {
            seen,
            screen: st.screen,
            text: String::new(),
            full: false,
            large_change: false,
            changes: 0,
            interactive: nodes
                .iter()
                .filter(|n| crate::roles::is_interactive(&n.role) || n.states.editable)
                .count(),
            touched: Vec::new(),
            removed: 0,
            restless: Vec::new(),
        };
        let mut budget = tree::Budget::from_config(tcfg);
        if let Some(tokens) = max_tokens {
            budget.tokens = tokens;
        }
        match base {
            None => {
                out.text = tree::render_full_within(nodes, tcfg.indent, budget);
                out.full = true;
            }
            Some(view) => {
                let mut d = view.diff(nodes);
                // Elements that keep changing on their own: one line.
                let mut restless: Vec<u32> = Vec::new();
                if tcfg.quiet_volatile && seen == Seen::Same {
                    d.changed.retain(|(pos, _)| {
                        let n = &nodes[*pos];
                        let busy = st.volatile.get(&n.key).is_some_and(|c| *c >= 2);
                        if busy {
                            restless.push(n.index);
                        }
                        !busy
                    });
                }
                out.changes = d.len();
                out.removed = d.removed.len();
                out.touched = d
                    .added
                    .iter()
                    .chain(d.changed.iter().map(|(p, _)| p))
                    .filter_map(|&p| nodes[p].bounds)
                    .collect();
                let large = tree::big_change(out.changes, nodes.len(), tcfg.diff_full_ratio);
                if full || !tcfg.diff || large {
                    out.text = tree::render_full_within(nodes, tcfg.indent, budget);
                    out.full = true;
                    out.large_change = large;
                } else if seen == Seen::Same {
                    out.text = if d.is_empty() {
                        tree::render_diff(&d, nodes)
                    } else {
                        // In full once, the legend once more, then a word.
                        let intro = if self.explain_first("diff") {
                            tree::DIFF_INTRO
                        } else {
                            self.explain(
                                "diff-legend",
                                "Changes (+ added, ~ changed, - removed):",
                                "Changes:",
                            )
                        };
                        tree::render_diff_with(&d, nodes, intro, tcfg.compact)
                    };
                    if !restless.is_empty() {
                        out.text.push_str(&format!(
                            "~ {} element(s) that keep changing on their own left out: {}\n",
                            restless.len(),
                            tree::ranges(&mut restless.clone())
                        ));
                    }
                    out.restless = restless;
                } else if d.is_empty() {
                    out.text = if self.explain_first("revisit") {
                        format!(
                            "Identical to when you last saw screen #{}; element indices are as they were then.\n",
                            st.screen
                        )
                    } else {
                        format!("Identical to screen #{} as you saw it.\n", st.screen)
                    };
                } else {
                    let intro = if self.explain_first("revisit") {
                        format!(
                            "Changes since you last saw screen #{} (+ added, ~ changed, - removed). Other elements are as they were then, with the same indices.",
                            st.screen
                        )
                    } else {
                        format!("Changes since you saw screen #{} (+/~/-):", st.screen)
                    };
                    out.text = tree::render_diff_with(&d, nodes, &intro, tcfg.compact);
                }
                // A diff gets the same budget as a whole tree.
                if !out.full
                    && budget.active()
                    && budget.level == crate::config::Summarize::Normal
                    && crate::text::estimate_tokens(&out.text) > budget.tokens
                {
                    out.text = tree::cut_to_budget(&out.text, budget.tokens);
                }
            }
        }
        if out.full && st.omitted > 0 {
            out.text
                .push_str(&format!("[{} more elements not shown]\n", st.omitted));
        }
        Ok(out)
    }

    /// The model has now been shown the latest snapshot: it becomes the known
    /// screen. Leaving a screen files it in the screen memory; returning to
    /// one takes it back out, with its screenshot's coordinate map. Returns
    /// whether a returning screen's window changed size (its old screenshot
    /// no longer lines up).
    pub(super) fn commit(&mut self, pid: u32, window: &WindowInfo) -> bool {
        let Some(mut st) = self.states.remove(&pid) else {
            return false;
        };
        let cache = self.store.config.cache.clone();
        let view = View::from_nodes(&st.nodes);
        let target_key = self
            .ctx
            .target
            .filter(|(p, _)| *p == pid)
            .and_then(|(_, i)| st.nodes.iter().find(|n| n.index == i))
            .map(|n| n.key);
        let quiet_volatile = self.store.config.tree.quiet_volatile;
        let size = window.bounds.map(|b| (b.width, b.height));
        let origin = window.bounds.map(|b| (b.x, b.y));
        let mut size_changed = false;
        match st.known.as_mut() {
            Some(k) if k.id == st.screen => {
                // Which elements changed since the model's last look, and
                // how many looks in a row they did (the acted-on element's
                // change isn't its own doing).
                // Changes after the model's own actions are theirs, not
                // the element's: only looks with no action between count.
                let acted = st.volatile_inputs != self.inputs;
                st.volatile_inputs = self.inputs;
                if quiet_volatile {
                    let d = k.view.diff(&st.nodes);
                    // The focused element changes with the model's own keys.
                    let changed: HashSet<u64> = d
                        .changed
                        .iter()
                        .map(|(p, _)| &st.nodes[*p])
                        .filter(|n| !n.states.focused)
                        .map(|n| n.key)
                        .collect();
                    st.volatile.retain(|key, _| changed.contains(key));
                    for key in changed {
                        if acted {
                            // Kept as it was: neither restless nor calm.
                        } else if Some(key) != target_key {
                            let c = st.volatile.entry(key).or_default();
                            *c = c.saturating_add(1);
                        } else {
                            st.volatile.remove(&key);
                        }
                    }
                }
                // The same screen in a window that moved or changed size: the
                // model's screenshot still names the same places only if the
                // size is the same, shifted by the move. (The tree's text
                // doesn't change, so nothing else would notice.)
                match (k.size, size) {
                    (Some(a), Some(b)) if (a.0 - b.0).abs() < 1.0 && (a.1 - b.1).abs() < 1.0 => {
                        if let (Some(o0), Some(o1)) = (k.origin, origin) {
                            let (dx, dy) = (o1.0 - o0.0, o1.1 - o0.1);
                            if dx != 0.0 || dy != 0.0 {
                                for c in [k.coord.as_mut(), st.coord.as_mut()].into_iter().flatten()
                                {
                                    c.bounds.x += dx;
                                    c.bounds.y += dy;
                                }
                            }
                        }
                    }
                    (Some(_), Some(_)) => {
                        k.coord = None;
                        k.pixels = None;
                        k.shot = false;
                        k.shot_id = None;
                        st.coord = None;
                        size_changed = true;
                    }
                    _ => {}
                }
                k.view = view;
                k.window = window.id;
                k.size = size;
                k.origin = origin;
            }
            _ => {
                // Where the window was when the model last saw it (another
                // window's place says nothing about this one).
                let before = st
                    .known
                    .as_ref()
                    .filter(|k| k.window == window.id)
                    .map(|k| (k.origin, k.size));
                let mut next = self
                    .memory
                    .take(st.screen)
                    .unwrap_or_else(|| Screen::new(st.screen, pid, window.id, View::default()));
                if let Some(old) = st.known.take()
                    && cache.enabled
                {
                    self.memory.remember(
                        old,
                        cache.max_screens,
                        cache.max_memory_kb.saturating_mul(1024),
                    );
                }
                // The old screenshot still lines up if the window kept its
                // size; follow the window if it moved.
                if next.shot {
                    match (next.size, size) {
                        (Some(a), Some(b))
                            if (a.0 - b.0).abs() < 1.0 && (a.1 - b.1).abs() < 1.0 =>
                        {
                            if let (Some(o0), Some(o1), Some(c)) =
                                (next.origin, origin, next.coord.as_mut())
                            {
                                c.bounds.x += o1.0 - o0.0;
                                c.bounds.y += o1.1 - o0.1;
                            }
                        }
                        _ => {
                            next.coord = None;
                            next.pixels = None;
                            next.shot = false;
                            next.shot_id = None;
                            size_changed = true;
                        }
                    }
                }
                next.view = view;
                next.window = window.id;
                next.size = size;
                next.origin = origin;
                if let Some(c) = next.coord {
                    st.coord = Some(c);
                } else if let Some(c) = st.coord.as_mut() {
                    // No picture of this screen yet: x/y still refer to the
                    // last one the model saw, which follows the window if it
                    // moved and is no use if it changed size.
                    match (before, origin, size) {
                        (Some((Some(o0), Some(s0))), Some(o1), Some(s1))
                            if (s0.0 - s1.0).abs() < 1.0 && (s0.1 - s1.1).abs() < 1.0 =>
                        {
                            c.bounds.x += o1.0 - o0.0;
                            c.bounds.y += o1.1 - o0.1;
                        }
                        (Some((_, Some(_))), _, Some(_)) => {
                            st.coord = None;
                            size_changed = true;
                        }
                        _ => {}
                    }
                }
                st.known = Some(next);
            }
        }
        self.states.insert(pid, st);
        size_changed
    }

    /// Record that the images handed out by the finished top-level call
    /// reached the model: their screens now have that screenshot.
    pub(super) fn commit_images(&mut self) {
        if let Some(shot) = self.ctx.pending_screen_shot.take() {
            self.screen_shot = Some(shot);
        }
        for p in std::mem::take(&mut self.ctx.pending_images) {
            let Some(st) = self.states.get_mut(&p.pid) else {
                continue;
            };
            st.coord = Some(p.coord);
            let screen = match st.known.as_mut() {
                Some(k) if k.id == p.screen => Some(k),
                _ => self.memory.get_mut(p.screen),
            };
            if let Some(s) = screen {
                s.coord = Some(p.coord);
                s.pixels = p.pixels;
                s.shot = true;
                s.shot_id = Some(p.id);
            }
        }
    }
}

/// A hash of a rendered tree, to tell whether it is the same text.
/// Whether `read` (text read off the screen) is the start of `label`, with
/// at most one misread character in six ("zero is thir" for "zero is
/// thinking").
pub(super) fn reads_as_start_of(read: &str, label: &str) -> bool {
    let a: Vec<char> = read.chars().collect();
    let b: Vec<char> = label.chars().take(a.len()).collect();
    if b.len() < a.len() {
        return false;
    }
    // Levenshtein distance between the two.
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb))
                .min(prev[j + 1] + 1)
                .min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()] <= (a.len() / 6).max(usize::from(a.len() >= 6))
}

/// A hash of a picture's exact pixels (and size).
fn pixels_hash(cap: &Capture) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (cap.width, cap.height).hash(&mut h);
    cap.rgba.hash(&mut h);
    h.finish()
}

/// Jaccard similarity between the shapes of `nodes` and `shapes`.
fn shape_similarity(nodes: &[Node], shapes: &HashSet<u64>) -> f64 {
    let mine: HashSet<u64> = nodes.iter().map(|n| n.shape).collect();
    if mine.is_empty() && shapes.is_empty() {
        return 1.0;
    }
    let common = mine.intersection(shapes).count();
    common as f64 / (mine.len() + shapes.len() - common).max(1) as f64
}
