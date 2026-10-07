//! The on-screen indicator: starting it, the stop and settings keys, and the cursor it glides to where the agent acts.

use super::*;

impl<B: Backend> Engine<B> {
    /// The running overlay helper, started on first use. Never fails: when
    /// it can't run, the engine simply works without it.
    fn overlay(&mut self) -> Option<&mut Overlay> {
        let keys = self.global_keys();
        let cfg = &self.store.config.overlay;
        // The helper also listens for the global keys, so it runs when any
        // is wanted (with the overlay off it draws nothing); so does the
        // hub, which also gives the turns.
        if !cfg.enabled && keys == crate::overlay::Keys::default() && !self.store.config.hub.enabled
        {
            self.overlay = None;
            return None;
        }
        if let Some(o) = self.overlay.as_ref().filter(|o| !o.alive()) {
            let was_hub = o.hub().is_some();
            self.overlay = None;
            // A connection that held a while: not one of a row of losses
            // (a long session would otherwise use them all up).
            let now = (self.clock)();
            if self
                .hub_since
                .is_some_and(|t| now.saturating_duration_since(t) >= HUB_STEADY)
            {
                self.hub_losses = 0;
            }
            if was_hub && self.hub_losses < MAX_HUB_LOSSES {
                // The hub went (killed, or ended between two agents): join
                // it again at once, so the stop key works again and this
                // agent keeps its number. Joining starts a new hub.
                self.hub_losses += 1;
                log::info!("the hub went away; joining it again");
            } else {
                // It died: try again a little later, a few times at most.
                self.overlay_failures += 1;
                self.overlay_error = Some("it stopped unexpectedly".into());
                self.overlay_retry_at = Some((self.clock)() + Duration::from_secs(10));
            }
        }
        if self.overlay.is_none() {
            let launcher = self.overlay_launcher.clone().or_else(|| {
                (!cfg.command.trim().is_empty())
                    .then(|| crate::overlay::find_helper(cfg))
                    .flatten()
            })?;
            if self.overlay_failures >= MAX_OVERLAY_FAILURES {
                if let Some(e) = &self.overlay_error
                    && !e.contains("gave up")
                {
                    self.overlay_error = Some(format!(
                        "{e}; gave up after {MAX_OVERLAY_FAILURES} tries, restart the server to try again"
                    ));
                }
                return None;
            }
            if self.overlay_retry_at.is_some_and(|t| (self.clock)() < t) {
                return None;
            }
            let on_settings = self.settings_handler();
            // The screen to share out, read when the hub is joined.
            let screen = if self.store.config.hub.enabled {
                self.work_area()
            } else {
                None
            };
            let cfg = &self.overlay_settings();
            // One hub for every agent on the desktop; an overlay of this
            // server's own when it can't be had.
            let hub = &self.store.config.hub;
            let joined = if hub.enabled {
                let opts = crate::overlay::hub::JoinOptions {
                    port: hub.port,
                    home: self
                        .hub_home
                        .clone()
                        .unwrap_or_else(crate::config::home_dir),
                    client: self.client.clone(),
                    want: self.hub_agent,
                    screen,
                };
                match Overlay::join_hub(
                    &launcher,
                    &opts,
                    cfg,
                    &keys,
                    self.stop.clone(),
                    Some(on_settings.clone()),
                ) {
                    Ok(o) => {
                        let agent = o.hub().map(|h| h.agent());
                        log::info!("joined the hub as agent {}", agent.unwrap_or(0));
                        self.hub_agent = agent;
                        self.hub_since = Some((self.clock)());
                        Some(o)
                    }
                    Err(e) => {
                        log::info!("no hub ({e}): this server's own overlay");
                        None
                    }
                }
            } else {
                None
            };
            let cfg = &self.store.config.overlay;
            let started = match joined {
                Some(o) => Ok(o),
                None => Overlay::spawn(&launcher, cfg, &keys, self.stop.clone(), Some(on_settings)),
            };
            match started {
                Ok(o) => {
                    self.overlay = Some(o);
                    self.overlay_error = None;
                }
                Err(e) => {
                    log::warn!("overlay unavailable: {e}");
                    self.overlay_failures += 1;
                    self.overlay_error = Some(e.to_string());
                    self.overlay_retry_at = Some((self.clock)() + Duration::from_secs(30));
                }
            }
        }
        self.overlay.as_mut()
    }

    /// The primary screen's work area (without task bars and docks).
    fn work_area(&mut self) -> Option<crate::types::Rect> {
        let displays = self.backend.displays().ok()?;
        displays
            .iter()
            .find(|d| d.primary)
            .or(displays.first())
            .map(|d| d.work_area)
    }

    pub(super) fn overlay_send(&mut self, cmd: OverlayCmd) {
        if let Some(o) = self.overlay() {
            o.send(&cmd);
        }
    }

    /// The overlay settings for this agent: its own pointer when
    /// `agent_cursors` names its client or its tag.
    pub(super) fn overlay_settings(&self) -> crate::config::OverlayConfig {
        let o = &self.store.config.overlay;
        o.for_agent(&[&self.client, &o.cursor_tag])
    }

    pub(super) fn overlay_reconfigure(&mut self) {
        let cfg = self.overlay_settings();
        let keys = self.global_keys();
        // As in `overlay`: the hub needs the connection too (dropping it
        // would give up this agent's turn and number).
        if !cfg.enabled && keys == crate::overlay::Keys::default() && !self.store.config.hub.enabled
        {
            self.overlay = None;
        } else if let Some(o) = &self.overlay {
            o.configure(&cfg, &keys);
        } else if keys != crate::overlay::Keys::default() {
            // Listen for the keys from now on, not only once work starts.
            self.overlay();
        }
    }

    /// The global keys the helper listens for.
    fn global_keys(&self) -> crate::overlay::Keys {
        let c = &self.store.config.control;
        crate::overlay::Keys {
            stop: c.stop_hotkey.trim().to_string(),
            settings: c.settings_hotkey.trim().to_string(),
        }
    }

    /// What the settings key does: open the settings panel
    /// (from the helper's reader thread, whatever the engine is doing).
    fn settings_handler(&self) -> crate::overlay::OnSettings {
        let path = self.store.path.clone();
        Arc::new(move || {
            if let Err(e) = crate::panel::open(path.clone(), "") {
                log::warn!("couldn't open the settings page: {e}");
            }
        })
    }

    /// Whether the settings key works: `None` when there is none, or the
    /// helper hasn't said yet.
    pub fn settings_key_ok(&mut self, timeout: Duration) -> Option<bool> {
        let key = self.store.config.control.settings_hotkey.trim().to_string();
        if key.is_empty() {
            return None;
        }
        let deadline = Instant::now() + timeout;
        loop {
            if self.overlay.is_none() {
                self.overlay();
            }
            match &self.overlay {
                Some(o) if !o.alive() => return Some(false),
                Some(o) => {
                    if let Some(ok) = o.settings_key_ok(&key) {
                        return Some(ok);
                    }
                }
                None => return Some(false),
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Start listening for the user's stop key (and get the overlay ready)
    /// before the first tool call. Hosts call this once at start-up.
    pub fn arm(&mut self) {
        self.overlay();
    }

    /// Why the emergency stop key isn't working, if it is configured and
    /// known not to work: the helper that listens for it couldn't start or
    /// stopped, or the system refused the key combination.
    pub fn stop_key_problem(&mut self) -> Option<String> {
        let key = self.store.config.control.stop_hotkey.trim().to_string();
        if key.is_empty() {
            return None; // the user chose to have none
        }
        if self.overlay.is_none() {
            // Try to start it (cheap if it is already known not to start).
            self.overlay();
        }
        match &self.overlay {
            Some(o) if !o.alive() => Some("the helper that listens for it stopped".into()),
            Some(o) if o.hotkey_ok(&key) == Some(false) => Some(
                "the system refused it (another program may use this key, or there is no display the helper can use); set control.stop_hotkey to another combination".into(),
            ),
            Some(_) => None,
            None => self
                .overlay_error
                .as_ref()
                .map(|e| format!("the helper that listens for it could not start ({e})")),
        }
    }

    /// Wait up to `timeout` for the stop key to be confirmed working
    /// (`computer-use-mcp doctor`).
    pub fn check_stop_key(&mut self, timeout: Duration) -> std::result::Result<(), String> {
        let key = self.store.config.control.stop_hotkey.trim().to_string();
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(problem) = self.stop_key_problem() {
                return Err(problem);
            }
            if self.overlay.as_ref().and_then(|o| o.hotkey_ok(&key)) == Some(true) {
                return Ok(());
            }
            if self.overlay.is_none() && self.overlay_error.is_none() {
                return Err("no overlay helper is set up to listen for it".into());
            }
            if Instant::now() >= deadline {
                return Err("the helper didn't confirm it in time".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Point the agent cursor at an element (its centre), optionally
    /// clicking, and wait until it is there. An element with no box of its
    /// own (accessibility trees have many) is shown at the nearest one
    /// around it that has one, else at the window: the cursor always goes
    /// where the work is, never stays behind.
    pub(super) fn overlay_point_element(
        &mut self,
        app: &AppInfo,
        handle: ElementHandle,
        click: bool,
    ) {
        if self.overlay.is_none() {
            return;
        }
        let Ok(st) = self.state(app.pid) else { return };
        let boxed = |b: &&Rect| !b.is_empty();
        let mut at = st.bounds.get(&handle).filter(boxed).copied();
        if at.is_none() {
            let mut node = st.nodes.iter().position(|n| n.handle == handle);
            while let Some(i) = node {
                if let Some(b) = st.nodes[i].bounds.as_ref().filter(boxed) {
                    at = Some(*b);
                    break;
                }
                node = st.nodes[i].parent;
            }
        }
        // Else the window, as the last screenshot of it covered it.
        let at = at.or_else(|| st.coord.map(|c| c.bounds).filter(|b| !b.is_empty()));
        if let Some(b) = at {
            self.overlay_point(b.center(), click);
        }
    }

    /// Point the agent cursor at a screen point (clicking there when
    /// `click`) and wait until it is shown there, so the action that
    /// follows happens where the user sees it.
    pub(super) fn overlay_point(&mut self, p: Point, click: bool) {
        let glide = Duration::from_millis(self.store.config.overlay.move_ms);
        if let Some(o) = self.overlay.as_mut() {
            o.point(p.x, p.y, click, glide);
        }
    }

    /// Start the agent cursor gliding to a point without waiting: it moves
    /// along with an action under way (a drag).
    pub(super) fn overlay_glide(&mut self, p: Point) {
        if self.overlay.is_some() {
            self.overlay_send(OverlayCmd::Pointer {
                x: p.x,
                y: p.y,
                click: false,
                id: None,
            });
        }
    }

    /// The glide just started is a drag: the cursor draws its line.
    pub(super) fn overlay_dragging(&mut self) {
        if self.overlay.is_some() {
            self.overlay_send(OverlayCmd::Dragging);
        }
    }

    /// Show a key combination being pressed by the agent cursor.
    pub(super) fn overlay_keys(&mut self, combo: &KeyCombo) {
        if self.overlay.is_some() && self.store.config.overlay.show_keys {
            self.overlay_send(OverlayCmd::Keys {
                keys: combo.to_string(),
            });
        }
    }

    /// Show text being typed by the agent cursor: dots for a password.
    pub(super) fn overlay_typed(&mut self, text: &str, secret: bool) {
        if self.overlay.is_none() || !self.store.config.overlay.show_keys {
            return;
        }
        let text = if secret {
            "•".repeat(text.chars().count().min(12))
        } else {
            text.to_string()
        };
        self.overlay_send(OverlayCmd::Typed { text });
    }

    /// Show the way the agent scrolls (wheel clicks; +: right, down).
    pub(super) fn overlay_scroll(&mut self, dx: i32, dy: i32) {
        if self.overlay.is_some() && self.store.config.overlay.cursor_motion {
            self.overlay_send(OverlayCmd::Scroll { dx, dy });
        }
    }

    /// Whether the focused element (as last read) is a password field.
    pub(super) fn focused_is_password(&self, app: &AppInfo) -> bool {
        self.state(app.pid).ok().is_some_and(|st| {
            st.nodes
                .iter()
                .rev()
                .find(|n| n.states.focused)
                .is_some_and(|n| crate::privacy::is_password(&n.role))
        })
    }

    /// Point the agent cursor at the element keys go to (the focused one),
    /// when an action types without naming one.
    pub(super) fn overlay_point_focus(&mut self, app: &AppInfo) {
        let focused = self.state(app.pid).ok().and_then(|st| {
            st.nodes
                .iter()
                .rev()
                .find(|n| n.states.focused)
                .map(|n| n.handle)
        });
        if let Some(h) = focused {
            self.overlay_point_element(app, h, false);
        }
    }

    /// Point the agent cursor at a click/scroll anchor.
    pub(super) fn overlay_anchor(&mut self, app: &AppInfo, anchor: &Anchor, click: bool) {
        match anchor {
            Anchor::Element(h) => self.overlay_point_element(app, *h, click),
            Anchor::Point(p) => self.overlay_point(*p, click),
        }
    }
}
