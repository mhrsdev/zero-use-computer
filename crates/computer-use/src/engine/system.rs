//! Windows, notifications and the clipboard.

use super::*;

impl<B: Backend> Engine<B> {
    pub(super) fn window_tool(&mut self, args: WindowArgs) -> Result<ToolOutput> {
        use crate::tools::WindowAction as A;
        if args.action == A::Displays {
            let displays = self.backend.displays()?;
            let mut out = format!("{} display(s), in screen coordinates:\n", displays.len());
            for d in &displays {
                out.push_str(&format!(
                    "- display {}{}: {} (usable area {})\n",
                    d.index,
                    if d.primary { " (primary)" } else { "" },
                    show_rect(d.bounds),
                    show_rect(d.work_area)
                ));
            }
            match self.backend.desktops() {
                Some((n, cur)) => out.push_str(&format!(
                    "Virtual desktops: {n} (current: {cur}; numbered from 0).\n"
                )),
                None => out.push_str("Virtual desktops: not available here.\n"),
            }
            return Ok(ToolOutput::text(out));
        }
        let query = args
            .app
            .as_deref()
            .ok_or_else(|| Error::InvalidArgs("this window action needs `app`".into()))?;
        let app = self.resolve_app(query)?;
        if args.action == A::List {
            let windows = self.list_windows(&app, true)?;
            if windows.is_empty() {
                return Ok(ToolOutput::text(format!(
                    "{} has no open windows.",
                    app.name
                )));
            }
            let mut out = format!("{} window(s) of {}:\n", windows.len(), app.name);
            for w in &windows {
                out.push_str(&format!("- {}\n", describe_window(w)));
            }
            return Ok(ToolOutput::text(out));
        }

        let window = self.resolve_window(&app, args.window.as_deref(), true)?;
        let bounds = window.bounds;
        // The display to place it on: the one asked for, else its own.
        let displays = self.backend.displays().unwrap_or_default();
        let display =
            match args.display {
                Some(i) => Some(displays.iter().find(|d| d.index == i).cloned().ok_or_else(
                    || Error::InvalidArgs(format!("no display {i}; see window action=displays")),
                )?),
                None => bounds
                    .and_then(|b| {
                        displays
                            .iter()
                            .find(|d| d.bounds.contains(b.center()))
                            .or_else(|| displays.iter().find(|d| d.primary))
                            .or(displays.first())
                    })
                    .cloned(),
            };
        let area = || {
            display.as_ref().map(|d| d.work_area).ok_or_else(|| {
                Error::Unsupported("the display layout is not known on this platform".into())
            })
        };
        let size =
            || bounds.ok_or_else(|| Error::ActionFailed("the window's size is not known".into()));
        let (op, what) = match args.action {
            A::Displays | A::List => unreachable!("handled above"),
            A::Focus => (WindowOp::Focus, "Focused".to_string()),
            A::Maximize if args.display.is_some() => {
                (WindowOp::SetBounds(area()?), "Maximized".to_string())
            }
            A::Maximize => (WindowOp::Maximize, "Maximized".to_string()),
            A::Minimize => (WindowOp::Minimize, "Minimized".to_string()),
            A::Restore => (WindowOp::Restore, "Restored".to_string()),
            A::Fullscreen => (WindowOp::Fullscreen(true), "Made full screen".to_string()),
            A::ExitFullscreen => (WindowOp::Fullscreen(false), "Left full screen".to_string()),
            A::Close => (WindowOp::Close, "Asked to close".to_string()),
            A::Move => {
                let (x, y) = match (args.x, args.y) {
                    (Some(x), Some(y)) => (x, y),
                    _ => return Err(Error::InvalidArgs("move needs x and y".into())),
                };
                let b = size()?;
                let r = Rect::new(
                    x,
                    y,
                    args.width.unwrap_or(b.width),
                    args.height.unwrap_or(b.height),
                );
                (WindowOp::SetBounds(r), "Moved".to_string())
            }
            A::Resize => {
                let b = size()?;
                let (w, h) = match (args.width, args.height) {
                    (None, None) => {
                        return Err(Error::InvalidArgs(
                            "resize needs width and/or height".into(),
                        ));
                    }
                    (w, h) => (w.unwrap_or(b.width), h.unwrap_or(b.height)),
                };
                if !(w >= 1.0 && h >= 1.0) {
                    return Err(Error::InvalidArgs(
                        "width and height must be positive".into(),
                    ));
                }
                (
                    WindowOp::SetBounds(Rect::new(b.x, b.y, w, h)),
                    "Resized".to_string(),
                )
            }
            A::TileLeft | A::TileRight | A::TileTop | A::TileBottom => {
                let a = area()?;
                let (hw, hh) = (a.width / 2.0, a.height / 2.0);
                let r = match args.action {
                    A::TileLeft => Rect::new(a.x, a.y, hw, a.height),
                    A::TileRight => Rect::new(a.x + hw, a.y, a.width - hw, a.height),
                    A::TileTop => Rect::new(a.x, a.y, a.width, hh),
                    _ => Rect::new(a.x, a.y + hh, a.width, a.height - hh),
                };
                (WindowOp::SetBounds(r), "Tiled".to_string())
            }
            A::Center | A::MoveToDisplay => {
                let a = area()?;
                let b = size()?;
                let (w, h) = (b.width.min(a.width), b.height.min(a.height));
                let r = Rect::new(a.x + (a.width - w) / 2.0, a.y + (a.height - h) / 2.0, w, h);
                let what = match (args.action, &display) {
                    (A::MoveToDisplay, Some(d)) => format!("Moved to display {}", d.index),
                    _ => "Centered".to_string(),
                };
                (WindowOp::SetBounds(r), what)
            }
            A::MoveToDesktop => {
                let d = args
                    .desktop
                    .ok_or_else(|| Error::InvalidArgs("move_to_desktop needs desktop".into()))?;
                (WindowOp::ToDesktop(d), format!("Moved to desktop {d}"))
            }
        };
        if let WindowOp::SetBounds(r) = op
            && !(r.width >= 1.0 && r.height >= 1.0 && r.x.is_finite() && r.y.is_finite())
        {
            return Err(Error::InvalidArgs(format!(
                "invalid window area {}",
                show_rect(r)
            )));
        }
        if let (WindowOp::SetBounds(r), Some(_)) = (op, &bounds) {
            self.overlay_send(OverlayCmd::Target {
                rect: Some([r.x, r.y, r.width, r.height]),
            });
        }
        self.backend.window_op(&app, &window, &op)?;
        self.settle();

        // The window's elements and private areas are somewhere else now:
        // an element click right after must not land where it was.
        if op != WindowOp::Close {
            self.reobserve(&app);
        }
        // The window's screenshot no longer lines up: the next
        // get_app_state takes a fresh one.
        if let Some(st) = self.states.get_mut(&app.pid) {
            st.coord = None;
            if let Some(k) = st.known.as_mut() {
                k.shot = false;
                k.coord = None;
                k.pixels = None;
                k.shot_id = None;
            }
        }
        let now = self
            .list_windows(&app, true)
            .ok()
            .and_then(|ws| ws.into_iter().find(|w| w.id == window.id));
        // Where the agent put it is what it wants of the screen.
        if matches!(
            op,
            WindowOp::SetBounds(_)
                | WindowOp::Maximize
                | WindowOp::Restore
                | WindowOp::Fullscreen(_)
        ) && let Some(w) = &now
        {
            self.placed(w);
        }
        let mut msg = format!("{what} \"{}\" of {}.", window.title, app.name);
        match now {
            Some(w) => msg.push_str(&format!(" Now: {}.", describe_window(&w))),
            None if op == WindowOp::Close => msg.push_str(" It is closed."),
            None => {}
        }
        if let (WindowOp::SetBounds(want), Some(got)) = (op, self.window_bounds(&app, window.id)) {
            let off = (got.x - want.x).abs().max((got.y - want.y).abs());
            let size_off = (got.width - want.width)
                .abs()
                .max((got.height - want.height).abs());
            if self.store.config.verify.enabled && (off > 16.0 || size_off > 16.0) {
                msg.push_str(" (The app or window manager adjusted it; it may have a minimum size or be maximized.)");
            }
        }
        Ok(ToolOutput::text(msg))
    }

    /// A window's current bounds, from the latest window list.
    fn window_bounds(&self, app: &AppInfo, id: u64) -> Option<Rect> {
        self.window_cache
            .get(&app.pid)
            .and_then(|(_, _, ws)| ws.iter().find(|w| w.id == id))
            .and_then(|w| w.bounds)
    }

    pub(super) fn get_notifications(&mut self, args: NotificationsArgs) -> Result<ToolOutput> {
        let cfg = self.store.config.notifications.clone();
        if !cfg.enabled {
            return Err(Error::Blocked(
                "notifications".into(),
                "reading notifications is off in settings ([notifications] enabled = false)".into(),
            ));
        }
        let all = self.backend.notifications()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut hidden = 0;
        let mut shown = Vec::new();
        for n in all {
            if let Some(q) = &args.app
                && !n.app.to_lowercase().contains(&q.to_lowercase())
            {
                continue;
            }
            let allowed_app =
                cfg.apps.is_empty() || cfg.apps.iter().any(|a| a.eq_ignore_ascii_case(&n.app));
            if !allowed_app {
                hidden += 1;
                continue;
            }
            shown.push(n);
        }
        let limit = args.limit.unwrap_or(10).max(1);
        let skip = shown.len().saturating_sub(limit);
        let privacy = &self.store.config.privacy;
        // `near`: the rest of the notification, which may be what says
        // the number is a code (title "Your PIN", body "4821").
        let clean = |s: &str, near: &str| -> String {
            let mut s = s.to_string();
            if privacy.redact_card_numbers
                && let Some(m) = crate::privacy::mask_card_numbers(&s)
            {
                s = m;
            }
            if cfg.mask_codes
                && let Some(m) = crate::privacy::mask_codes_near(&s, near)
            {
                s = m;
            }
            tree::truncate(&s.replace('\n', " / "), 300)
        };
        let mut out = if shown.is_empty() {
            "No recent notifications.".to_string()
        } else {
            format!(
                "{} recent notification(s), newest last:\n",
                shown.len() - skip
            )
        };
        for n in &shown[skip..] {
            let when = match n.time.map(|t| now.saturating_sub(t)) {
                Some(s) if s < 60 => "just now".to_string(),
                Some(s) if s < 3600 => format!("{} min ago", s / 60),
                Some(s) if s < 86_400 => format!("{} h ago", s / 3600),
                Some(s) => format!("{} d ago", s / 86_400),
                None => "on screen".to_string(),
            };
            let app = if n.app.is_empty() {
                "?"
            } else {
                n.app.as_str()
            };
            let body = clean(&n.body, &n.title);
            out.push_str(&format!("- [{when}] {app} — {}", clean(&n.title, &n.body)));
            if !body.is_empty() {
                out.push_str(&format!(": {body}"));
            }
            out.push('\n');
        }
        if hidden > 0 {
            out.push_str(&format!(
                "[{hidden} from apps not in notifications.apps not shown]\n"
            ));
        }
        Ok(ToolOutput::text(out))
    }

    pub(super) fn get_clipboard(&mut self) -> Result<ToolOutput> {
        if !self.store.config.clipboard {
            return Err(Error::Blocked(
                "clipboard".into(),
                "clipboard access is disabled in config".into(),
            ));
        }
        let text = self.backend.clipboard_get()?;
        self.set_structured(|| {
            let count = text.chars().count();
            serde_json::json!({
                "text": text.chars().take(MAX_CLIPBOARD_CHARS).collect::<String>(),
                "characters": count,
                "truncated": count > MAX_CLIPBOARD_CHARS,
            })
        });
        if text.is_empty() {
            return Ok(ToolOutput::text("The clipboard is empty."));
        }
        // Someone may have copied a whole book.
        let count = text.chars().count();
        Ok(ToolOutput::text(if count > MAX_CLIPBOARD_CHARS {
            let head: String = text.chars().take(MAX_CLIPBOARD_CHARS).collect();
            format!("Clipboard (first {MAX_CLIPBOARD_CHARS} of {count} characters):\n{head}")
        } else {
            format!("Clipboard:\n{text}")
        }))
    }

    pub(super) fn set_clipboard(&mut self, args: SetClipboardArgs) -> Result<ToolOutput> {
        if !self.store.config.clipboard {
            return Err(Error::Blocked(
                "clipboard".into(),
                "clipboard access is disabled in config".into(),
            ));
        }
        self.backend.clipboard_set(&args.text)?;
        Ok(ToolOutput::text(format!(
            "Copied {} character(s) to the clipboard.",
            args.text.chars().count()
        )))
    }
}

fn describe_window(w: &WindowInfo) -> String {
    let mut s = format!("\"{}\" (id {})", w.title, w.id);
    if let Some(b) = w.bounds {
        s.push_str(&format!(", {}", show_rect(b)));
    }
    let mut flags = Vec::new();
    if w.focused {
        flags.push("focused");
    }
    if w.minimized {
        flags.push("minimized");
    }
    if !flags.is_empty() {
        s.push_str(&format!(" [{}]", flags.join(", ")));
    }
    s
}
