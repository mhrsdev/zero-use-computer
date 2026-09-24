//! The engine: platform-agnostic implementation of the tool contract on top
//! of a [`Backend`]. It resolves apps/windows/elements, enforces approvals,
//! renders app state (tree + screenshot), maintains per-app element indices
//! and diffs, and maps screenshot coordinates to the screen.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::backend::{Backend, Native};
use crate::config::ConfigStore;
use crate::error::{Error, Result};
use crate::imaging::{self, CoordMap};
use crate::keys::{self, Key, KeyCombo, NamedKey};
use crate::policy::{self, Verdict};
use crate::tools::*;
use crate::tree::{self, IndexAllocator, Node};
use crate::types::*;

/// What the host must decide when an app needs approval.
#[derive(Debug, Clone)]
pub struct ApprovalRequest<'a> {
    pub app: &'a AppInfo,
    pub tool: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Allow for this call only.
    Once,
    /// Allow and persist to `always_allow`.
    Always,
    /// Allow for the rest of this session.
    Session,
    Deny,
}

/// Decides whether an app may be controlled when policy says to ask.
/// Implementations prompt the user (MCP elicitation, a desktop dialog, etc.).
pub trait Approver {
    fn request(&mut self, req: &ApprovalRequest<'_>) -> ApprovalDecision;
}

/// Approve nothing (used when no approver is wired up).
pub struct DenyApprover;
impl Approver for DenyApprover {
    fn request(&mut self, _req: &ApprovalRequest<'_>) -> ApprovalDecision {
        ApprovalDecision::Deny
    }
}

/// Approve everything (embedders that gate access themselves).
pub struct AllowApprover;
impl Approver for AllowApprover {
    fn request(&mut self, _req: &ApprovalRequest<'_>) -> ApprovalDecision {
        ApprovalDecision::Session
    }
}

/// Cached state for one app between tool calls.
#[derive(Default)]
struct AppState {
    window_id: Option<u64>,
    alloc: IndexAllocator,
    nodes: Vec<Node>,
    coord: Option<CoordMap>,
    /// Handle → screen bounds, for coordinate fallbacks.
    bounds: HashMap<ElementHandle, Rect>,
    stamped: bool,
}

pub struct Engine<B: Backend> {
    backend: B,
    store: ConfigStore,
    session_allowed: HashSet<String>,
    states: HashMap<u32, AppState>,
    clock: Box<dyn Fn() -> Instant + Send>,
    sleep: Box<dyn Fn(Duration) + Send>,
}

/// A resolved click/scroll/drag anchor.
enum Anchor {
    Element(ElementHandle),
    Point(Point),
}

impl<B: Backend> Engine<B> {
    pub fn new(backend: B, store: ConfigStore) -> Self {
        Self {
            backend,
            store,
            session_allowed: HashSet::new(),
            states: HashMap::new(),
            clock: Box::new(Instant::now),
            sleep: Box::new(std::thread::sleep),
        }
    }

    /// Replace timing hooks (tests use instant clocks).
    pub fn with_time(
        mut self,
        clock: impl Fn() -> Instant + Send + 'static,
        sleep: impl Fn(Duration) + Send + 'static,
    ) -> Self {
        self.clock = Box::new(clock);
        self.sleep = Box::new(sleep);
        self
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn store(&self) -> &ConfigStore {
        &self.store
    }

    pub fn permissions(&mut self) -> Vec<PermissionStatus> {
        self.backend.permissions()
    }

    /// Pre-approve an app for the session (e.g. from a host allow-list).
    pub fn allow_for_session(&mut self, app_id: &str) {
        self.session_allowed.insert(app_id.to_lowercase());
    }

    // -- app / window / element resolution ---------------------------------

    fn find_apps(&mut self) -> Result<Vec<AppInfo>> {
        self.backend.list_apps()
    }

    fn resolve_app(&mut self, query: &str) -> Result<AppInfo> {
        let apps = self.find_apps()?;
        resolve_app_in(&apps, query)
    }

    /// Resolve an app and confirm it may be controlled.
    fn authorize(
        &mut self,
        query: &str,
        tool: &str,
        approver: &mut dyn Approver,
    ) -> Result<AppInfo> {
        let app = self.resolve_app(query)?;
        match policy::evaluate(&app, &self.store, &self.session_allowed) {
            Verdict::Allowed => Ok(app),
            Verdict::Blocked(reason) => Err(Error::Blocked(app.name, reason)),
            Verdict::NeedsApproval => {
                let decision = approver.request(&ApprovalRequest { app: &app, tool });
                match decision {
                    ApprovalDecision::Deny => Err(Error::Denied(app.name)),
                    ApprovalDecision::Once => Ok(app),
                    ApprovalDecision::Session => {
                        self.session_allowed.insert(app.id.to_lowercase());
                        Ok(app)
                    }
                    ApprovalDecision::Always => {
                        self.session_allowed.insert(app.id.to_lowercase());
                        if let Err(e) = self.store.add_always_allow(&app.id) {
                            log::warn!("could not persist approval for {}: {e}", app.id);
                        }
                        Ok(app)
                    }
                }
            }
        }
    }

    fn resolve_window(&mut self, app: &AppInfo, query: Option<&str>) -> Result<WindowInfo> {
        let mut windows = self.backend.list_windows(app)?;
        if windows.is_empty() {
            return Err(Error::NoWindows {
                app: app.name.clone(),
            });
        }
        if let Some(q) = query {
            let ql = q.to_lowercase();
            if let Some(w) = windows.iter().find(|w| w.id.to_string() == q) {
                return Ok(w.clone());
            }
            let matches: Vec<&WindowInfo> = windows
                .iter()
                .filter(|w| w.title.to_lowercase().contains(&ql))
                .collect();
            return match matches.as_slice() {
                [w] => Ok((*w).clone()),
                [] => Err(Error::WindowNotFound {
                    query: q.into(),
                    app: app.name.clone(),
                    available: window_list(&windows),
                }),
                many => {
                    // Prefer an exact title, else the focused one.
                    if let Some(w) = many.iter().find(|w| w.title.eq_ignore_ascii_case(q)) {
                        return Ok((*w).clone());
                    }
                    many.iter()
                        .find(|w| w.focused)
                        .map(|w| (*w).clone())
                        .ok_or_else(|| Error::WindowNotFound {
                            query: q.into(),
                            app: app.name.clone(),
                            available: window_list(&windows),
                        })
                }
            };
        }
        // No query: remembered window, else focused/main, else first.
        if let Some(id) = self.states.get(&app.pid).and_then(|s| s.window_id)
            && let Some(w) = windows.iter().find(|w| w.id == id && !w.minimized)
        {
            return Ok(w.clone());
        }
        windows.sort_by_key(|w| (!w.focused, !w.main, w.minimized));
        Ok(windows.into_iter().next().unwrap())
    }

    /// Refresh an app's state cache from a live snapshot.
    fn refresh(
        &mut self,
        app: &AppInfo,
        window: &WindowInfo,
        disable_diff: bool,
    ) -> Result<String> {
        let opts = SnapshotOptions {
            max_nodes: self.store.config.tree.max_walk,
            max_depth: self.store.config.tree.max_depth,
        };
        let raw = self.backend.snapshot(app, window, &opts)?;
        let pruned = tree::prune(&raw, window.bounds, &self.store.config.tree);

        let previous = self.states.remove(&app.pid);
        let same_window = previous.as_ref().and_then(|s| s.window_id) == Some(window.id);
        let mut alloc = previous
            .as_ref()
            .filter(|_| same_window)
            .map(|s| s.alloc.clone())
            .unwrap_or_default();
        let old_nodes = previous
            .as_ref()
            .filter(|_| same_window)
            .map(|s| s.nodes.clone())
            .unwrap_or_default();

        let mut nodes = pruned.nodes;
        let want_diff = self.store.config.tree.diff && !disable_diff && !old_nodes.is_empty();
        if want_diff {
            alloc.assign_stable(&mut nodes);
        } else {
            alloc.assign_fresh(&mut nodes);
        }

        let bounds: HashMap<ElementHandle, Rect> = nodes
            .iter()
            .filter_map(|n| n.bounds.map(|b| (n.handle, b)))
            .collect();

        let rendered = if want_diff {
            let d = tree::diff(&old_nodes, &nodes);
            // Fall back to the full tree when the diff isn't actually smaller.
            if d.len() * 3 >= nodes.len() {
                tree::render_full(&nodes)
            } else {
                tree::render_diff(&d, &nodes)
            }
        } else {
            tree::render_full(&nodes)
        };

        self.states.insert(
            app.pid,
            AppState {
                window_id: Some(window.id),
                alloc,
                nodes,
                coord: None,
                bounds,
                stamped: true,
            },
        );

        let mut out = rendered;
        if pruned.omitted > 0 {
            out.push_str(&format!(
                "\n[{} more elements not shown; narrow the window or act on what is visible]\n",
                pruned.omitted
            ));
        }
        Ok(out)
    }

    fn state(&self, pid: u32) -> Result<&AppState> {
        self.states
            .get(&pid)
            .filter(|s| s.stamped)
            .ok_or(Error::Internal("no cached state".into()))
    }

    /// Look up an element handle by index in the app's latest state.
    fn element_by_index(&self, app: &AppInfo, index: u32) -> Result<ElementHandle> {
        let state = self
            .states
            .get(&app.pid)
            .filter(|s| s.stamped)
            .ok_or_else(|| Error::NoState(app.name.clone()))?;
        state
            .nodes
            .iter()
            .find(|n| n.index == index)
            .map(|n| n.handle)
            .ok_or(Error::UnknownElement {
                app: app.name.clone(),
                index,
            })
    }

    fn node_by_index(&self, app: &AppInfo, index: u32) -> Result<&Node> {
        let state = self
            .state(app.pid)
            .map_err(|_| Error::NoState(app.name.clone()))?;
        state
            .nodes
            .iter()
            .find(|n| n.index == index)
            .ok_or(Error::UnknownElement {
                app: app.name.clone(),
                index,
            })
    }

    fn input_target(&self, app: &AppInfo) -> InputTarget {
        let (window_id, window_handle) = self
            .states
            .get(&app.pid)
            .and_then(|s| s.window_id)
            .map(|id| (Some(id), None))
            .unwrap_or((None, None));
        InputTarget {
            pid: app.pid,
            window_id,
            window_handle,
        }
    }

    /// Convert an element index / coordinate pair into a screen anchor.
    fn anchor(
        &self,
        app: &AppInfo,
        index: Option<u32>,
        x: Option<f64>,
        y: Option<f64>,
        what: &str,
    ) -> Result<Anchor> {
        if let Some(i) = index {
            return Ok(Anchor::Element(self.element_by_index(app, i)?));
        }
        match (x, y) {
            (Some(x), Some(y)) => {
                let map = self.state(app.pid).ok().and_then(|s| s.coord).ok_or_else(|| {
                    Error::InvalidArgs(format!(
                        "no current screenshot to map ({x}, {y}); call get_app_state first, or use element_index"
                    ))
                })?;
                Ok(Anchor::Point(map.to_screen(x, y)?))
            }
            _ => Err(Error::InvalidArgs(format!(
                "{what} needs either element_index or both x and y"
            ))),
        }
    }

    /// Screen point for an anchor (element center for element anchors).
    fn anchor_point(&self, app: &AppInfo, anchor: &Anchor) -> Result<Point> {
        match anchor {
            Anchor::Point(p) => Ok(*p),
            Anchor::Element(h) => self
                .state(app.pid)
                .ok()
                .and_then(|s| s.bounds.get(h))
                .filter(|b| !b.is_empty())
                .map(|b| b.center())
                .ok_or_else(|| {
                    Error::ActionFailed("this element has no on-screen position to click".into())
                }),
        }
    }

    fn settle(&self) {
        (self.sleep)(Duration::from_millis(40));
    }

    // -- tool dispatch -----------------------------------------------------

    /// Run one tool call.
    pub fn call(&mut self, call: ToolCall, approver: &mut dyn Approver) -> Result<ToolOutput> {
        match call {
            ToolCall::ListApps => self.list_apps(),
            ToolCall::LaunchApp(a) => self.launch_app(a, approver),
            ToolCall::GetAppState(a) => self.get_app_state(a, approver),
            ToolCall::Click(a) => self.click(a, approver),
            ToolCall::PerformSecondaryAction(a) => self.perform_secondary(a, approver),
            ToolCall::SetValue(a) => self.set_value(a, approver),
            ToolCall::SelectText(a) => self.select_text(a, approver),
            ToolCall::Scroll(a) => self.scroll(a, approver),
            ToolCall::Drag(a) => self.drag(a, approver),
            ToolCall::PressKey(a) => self.press_key(a, approver),
            ToolCall::TypeText(a) => self.type_text(a, approver),
        }
    }

    /// Parse and run a raw tool call, returning a tool result even on error.
    pub fn call_tool(
        &mut self,
        name: &str,
        args: serde_json::Value,
        approver: &mut dyn Approver,
    ) -> ToolOutput {
        match ToolCall::parse(name, args).and_then(|c| self.call(c, approver)) {
            Ok(out) => out,
            Err(e) => ToolOutput::error(&e),
        }
    }

    fn list_apps(&mut self) -> Result<ToolOutput> {
        let apps = self.find_apps()?;
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
            if let Some(t) = policy::tag(a, &self.store, &self.session_allowed) {
                tags.push(t);
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
        Ok(ToolOutput::text(lines.join("\n")))
    }

    fn launch_app(
        &mut self,
        args: LaunchAppArgs,
        approver: &mut dyn Approver,
    ) -> Result<ToolOutput> {
        // Enforce policy against a synthetic app record so a blocked category
        // (e.g. terminals) can't be launched-and-driven around the block.
        let probe = AppInfo {
            name: args.app.clone(),
            id: args.app.clone(),
            pid: 0,
            exe: Some(args.app.clone()),
            frontmost: false,
            hidden: false,
        };
        if let Verdict::Blocked(reason) =
            policy::evaluate(&probe, &self.store, &self.session_allowed)
        {
            return Err(Error::Blocked(args.app.clone(), reason));
        }

        let before: HashSet<u32> = self.find_apps()?.iter().map(|a| a.pid).collect();
        self.backend.launch_app(&args.app)?;

        let deadline = (self.clock)()
            + Duration::from_secs_f64(self.store.config.launch_timeout_secs.max(0.5));
        let ql = args.app.to_lowercase();
        loop {
            let apps = self.find_apps()?;
            // Prefer a newly-appeared app that matches the query.
            let found = apps
                .iter()
                .find(|a| {
                    !before.contains(&a.pid) && a.match_keys().iter().any(|k| k.contains(&ql))
                })
                .or_else(|| {
                    apps.iter()
                        .find(|a| a.match_keys().iter().any(|k| k.contains(&ql)))
                });
            if let Some(app) = found {
                let app = app.clone();
                // Authorize now so the model can act right away.
                let _ = self.authorize(&app.id, "launch_app", approver);
                return Ok(ToolOutput::text(format!(
                    "Launched {} (id: {}, pid: {}). Call get_app_state to see it.",
                    app.name, app.id, app.pid
                )));
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

    fn get_app_state(
        &mut self,
        args: GetAppStateArgs,
        approver: &mut dyn Approver,
    ) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "get_app_state", approver)?;
        let window = self.resolve_window(&app, args.window.as_deref())?;
        let tree_text = self.refresh(&app, &window, args.disable_diff)?;

        let mut header = format!(
            "App: {} (id: {}, pid: {})\nWindow: \"{}\" (id: {})",
            app.name, app.id, app.pid, window.title, window.id
        );
        if let Some(b) = window.bounds {
            header.push_str(&format!(
                " at ({:.0}, {:.0}) size {:.0}x{:.0}",
                b.x, b.y, b.width, b.height
            ));
        }

        let mut image = None;
        if self.store.config.screenshot.enabled {
            match self.backend.capture(&app, &window) {
                Ok(cap) => match imaging::encode(&cap, &self.store.config.screenshot) {
                    Ok((img, map)) => {
                        header.push_str(&format!(
                            "\nScreenshot: {}x{} px (x/y coordinates for click/scroll/drag are in this image's pixels).",
                            img.width, img.height
                        ));
                        if let Some(s) = self.states.get_mut(&app.pid) {
                            s.coord = Some(map);
                        }
                        image = Some(img);
                    }
                    Err(e) => header.push_str(&format!("\n[screenshot encode failed: {e}]")),
                },
                Err(e) => header.push_str(&format!("\n[screenshot unavailable: {e}]")),
            }
        }

        let text = format!("{header}\n\nAccessibility tree:\n{tree_text}");
        Ok(ToolOutput {
            text,
            image,
            is_error: false,
        })
    }

    fn click(&mut self, args: ClickArgs, approver: &mut dyn Approver) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "click", approver)?;
        let count = args.click_count.clamp(1, 3);
        let anchor = self.anchor(&app, args.element_index, args.x, args.y, "click")?;

        // A single left click on an element with a press action goes through
        // the accessibility API so it works in the background.
        if let (Anchor::Element(h), MouseButton::Left, 1) = (&anchor, args.button, count) {
            let node = self.node_for_handle(&app, *h);
            if let Some(action) = node
                .and_then(|n| n.has_action("press"))
                .map(|a| a.native.clone())
            {
                self.backend.perform_action(*h, &action)?;
                self.settle();
                return Ok(ToolOutput::text(format!(
                    "Pressed {}.",
                    self.describe(&app, *h)
                )));
            }
        }

        let point = self.anchor_point(&app, &anchor)?;
        let target = self.input_target(&app);
        self.backend.click(&target, point, args.button, count)?;
        self.settle();
        let verb = match (args.button, count) {
            (MouseButton::Right, _) => "Right-clicked",
            (_, 2) => "Double-clicked",
            (_, 3) => "Triple-clicked",
            _ => "Clicked",
        };
        Ok(ToolOutput::text(format!(
            "{verb} {} at ({:.0}, {:.0}).",
            self.describe_anchor(&app, &anchor),
            point.x,
            point.y
        )))
    }

    fn perform_secondary(
        &mut self,
        args: SecondaryActionArgs,
        approver: &mut dyn Approver,
    ) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "perform_secondary_action", approver)?;
        let handle = self.element_by_index(&app, args.element_index)?;
        let node = self.node_by_index(&app, args.element_index)?.clone();
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
        self.backend.perform_action(handle, &native)?;
        self.settle();
        Ok(ToolOutput::text(format!(
            "Performed `{}` on {}.",
            args.action,
            node.label()
        )))
    }

    fn set_value(&mut self, args: SetValueArgs, approver: &mut dyn Approver) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "set_value", approver)?;
        let handle = self.element_by_index(&app, args.element_index)?;
        let node = self.node_by_index(&app, args.element_index)?.clone();
        self.backend.set_value(handle, &args.value)?;
        self.settle();
        Ok(ToolOutput::text(format!(
            "Set {} to \"{}\".",
            node.label(),
            tree::truncate(&args.value, 80)
        )))
    }

    fn select_text(
        &mut self,
        args: SelectTextArgs,
        approver: &mut dyn Approver,
    ) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "select_text", approver)?;
        let handle = self.element_by_index(&app, args.element_index)?;
        let node = self.node_by_index(&app, args.element_index)?.clone();
        self.backend
            .select_text(handle, args.text.as_deref(), args.occurrence.max(1))?;
        self.settle();
        let what = match &args.text {
            Some(t) => format!("\"{}\"", tree::truncate(t, 60)),
            None => "all text".into(),
        };
        Ok(ToolOutput::text(format!(
            "Selected {what} in {}.",
            node.label()
        )))
    }

    fn scroll(&mut self, args: ScrollArgs, approver: &mut dyn Approver) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "scroll", approver)?;
        let pages = if args.amount.is_finite() && args.amount > 0.0 {
            args.amount
        } else {
            1.0
        };
        let anchor = self.anchor(&app, args.element_index, args.x, args.y, "scroll")?;

        if let Anchor::Element(h) = &anchor
            && let Native::Done(_) = self.backend.scroll_element(*h, args.direction, pages)?
        {
            self.settle();
            return Ok(ToolOutput::text(format!(
                "Scrolled {} {:?} by {pages} page(s).",
                self.describe(&app, *h),
                args.direction
            )));
        }

        let point = self.anchor_point(&app, &anchor)?;
        let (ux, uy) = args.direction.unit();
        // ~3 wheel lines per page.
        let lines = (pages * 3.0).round().max(1.0) as i32;
        let target = self.input_target(&app);
        self.backend
            .scroll_wheel(&target, point, ux * lines, uy * lines)?;
        self.settle();
        Ok(ToolOutput::text(format!(
            "Scrolled {:?} by {pages} page(s) at ({:.0}, {:.0}).",
            args.direction, point.x, point.y
        )))
    }

    fn drag(&mut self, args: DragArgs, approver: &mut dyn Approver) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "drag", approver)?;
        let from = self.anchor(
            &app,
            args.from_element_index,
            args.from_x,
            args.from_y,
            "drag source",
        )?;
        let to = self.anchor(
            &app,
            args.to_element_index,
            args.to_x,
            args.to_y,
            "drag target",
        )?;
        let (p0, p1) = (
            self.anchor_point(&app, &from)?,
            self.anchor_point(&app, &to)?,
        );
        let target = self.input_target(&app);
        self.backend.drag(&target, p0, p1)?;
        self.settle();
        Ok(ToolOutput::text(format!(
            "Dragged from ({:.0}, {:.0}) to ({:.0}, {:.0}).",
            p0.x, p0.y, p1.x, p1.y
        )))
    }

    fn press_key(&mut self, args: PressKeyArgs, approver: &mut dyn Approver) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "press_key", approver)?;
        let combos = keys::parse_sequence(&args.key)?;
        if let Some(i) = args.element_index {
            let h = self.element_by_index(&app, i)?;
            let _ = self.backend.focus(h);
        }
        let target = self.input_target(&app);
        for combo in &combos {
            self.backend.press_key(&target, combo)?;
            (self.sleep)(Duration::from_millis(10));
        }
        self.settle();
        let shown: Vec<String> = combos.iter().map(|c| c.to_string()).collect();
        Ok(ToolOutput::text(format!("Pressed {}.", shown.join(" "))))
    }

    fn type_text(&mut self, args: TypeTextArgs, approver: &mut dyn Approver) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "type_text", approver)?;
        if args.text.is_empty() {
            return Err(Error::InvalidArgs("`text` must not be empty".into()));
        }
        if let Some(i) = args.element_index {
            let h = self.element_by_index(&app, i)?;
            let _ = self.backend.focus(h);
            self.settle();
        }
        let target = self.input_target(&app);
        // Split on newlines so each becomes a Return press (works everywhere).
        let mut first = true;
        for segment in args.text.split('\n') {
            if !first {
                self.backend.press_key(
                    &target,
                    &KeyCombo {
                        modifiers: keys::Modifiers::default(),
                        key: Key::Named(NamedKey::Return),
                    },
                )?;
            }
            if !segment.is_empty() {
                self.backend.type_text(&target, segment)?;
            }
            first = false;
        }
        self.settle();
        Ok(ToolOutput::text(format!(
            "Typed {} character(s).",
            args.text.chars().count()
        )))
    }

    // -- describers --------------------------------------------------------

    fn node_for_handle(&self, app: &AppInfo, handle: ElementHandle) -> Option<&Node> {
        self.states
            .get(&app.pid)
            .and_then(|s| s.nodes.iter().find(|n| n.handle == handle))
    }

    fn describe(&self, app: &AppInfo, handle: ElementHandle) -> String {
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

fn window_list(windows: &[WindowInfo]) -> String {
    windows
        .iter()
        .map(|w| format!("\"{}\" (id: {})", w.title, w.id))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve an app from a name / id / pid query against a snapshot of apps.
fn resolve_app_in(apps: &[AppInfo], query: &str) -> Result<AppInfo> {
    let q = query.trim();
    if q.is_empty() {
        return Err(Error::InvalidArgs("`app` must not be empty".into()));
    }
    if let Ok(pid) = q.parse::<u32>()
        && let Some(a) = apps.iter().find(|a| a.pid == pid)
    {
        return Ok(a.clone());
    }
    let ql = q.to_lowercase();
    // Exact id or name.
    let exact: Vec<&AppInfo> = apps
        .iter()
        .filter(|a| a.id.eq_ignore_ascii_case(q) || a.name.eq_ignore_ascii_case(q))
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0].clone());
    }
    if exact.len() > 1 {
        // Same name twice: prefer frontmost.
        if let Some(a) = exact.iter().find(|a| a.frontmost) {
            return Ok((*a).clone());
        }
        return Ok(exact[0].clone());
    }
    // Key match (exe stem, bundle id, etc.).
    let key: Vec<&AppInfo> = apps
        .iter()
        .filter(|a| a.match_keys().contains(&ql))
        .collect();
    if key.len() == 1 {
        return Ok(key[0].clone());
    }
    // Substring on name/id.
    let sub: Vec<&AppInfo> = apps
        .iter()
        .filter(|a| a.name.to_lowercase().contains(&ql) || a.id.to_lowercase().contains(&ql))
        .collect();
    match sub.as_slice() {
        [a] => Ok((*a).clone()),
        [] => Err(Error::AppNotFound(query.into())),
        many => {
            if let Some(a) = many.iter().find(|a| a.frontmost) {
                return Ok((*a).clone());
            }
            Err(Error::AmbiguousApp {
                query: query.into(),
                candidates: many
                    .iter()
                    .map(|a| format!("{} (pid {})", a.name, a.pid))
                    .collect::<Vec<_>>()
                    .join(", "),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApprovalMode, Config};
    use crate::mock::{Event, MockBackend};

    fn engine() -> Engine<MockBackend> {
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let mut cfg = Config::default();
        cfg.approvals.mode = ApprovalMode::AllowAll;
        let mut e = Engine::new(backend, ConfigStore::in_memory(cfg));
        // Deterministic, instant time.
        e = e.with_time(Instant::now, |_| {});
        e
    }

    fn allow() -> AllowApprover {
        AllowApprover
    }

    #[test]
    fn list_apps_and_state() {
        let mut e = engine();
        let out = e.call(ToolCall::ListApps, &mut allow()).unwrap();
        assert!(out.text.contains("TextEdit"));

        let out = e
            .call(
                ToolCall::GetAppState(GetAppStateArgs {
                    app: "TextEdit".into(),
                    ..Default::default()
                }),
                &mut allow(),
            )
            .unwrap();
        assert!(out.image.is_some());
        assert!(out.text.contains("button \"Bold\""));
        assert!(out.text.contains("text area \"Document\""));
        // Bold has only press (hidden); Style keeps show_menu as a listed action.
        assert!(out.text.contains("actions=[show_menu]"));
        assert!(!out.text.contains("actions=[press"));
    }

    #[test]
    fn click_uses_accessibility_then_falls_back_to_coords() {
        let mut e = engine();
        e.call(
            ToolCall::GetAppState(GetAppStateArgs {
                app: "TextEdit".into(),
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        // Find Bold's index.
        let bold = e
            .state(4242)
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.name.as_deref() == Some("Bold"))
            .unwrap()
            .index;
        e.call(
            ToolCall::Click(ClickArgs {
                app: "TextEdit".into(),
                element_index: Some(bold),
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        assert!(matches!(
            e.backend().events.last().unwrap(),
            Event::Action(3, a) if a == "AXPress"
        ));

        // Right click -> coordinate path.
        e.call(
            ToolCall::Click(ClickArgs {
                app: "TextEdit".into(),
                element_index: Some(bold),
                button: MouseButton::Right,
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        assert!(matches!(
            e.backend().events.last().unwrap(),
            Event::Click(4242, _, MouseButton::Right, 1)
        ));
    }

    #[test]
    fn coordinate_click_maps_through_screenshot() {
        let mut e = engine();
        e.call(
            ToolCall::GetAppState(GetAppStateArgs {
                app: "TextEdit".into(),
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        e.call(
            ToolCall::Click(ClickArgs {
                app: "TextEdit".into(),
                x: Some(400.0),
                y: Some(300.0),
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        // Window is 800x600; screenshot fits to 800x600 (<=1280) so 1:1.
        assert!(matches!(
            e.backend().events.last().unwrap(),
            Event::Click(4242, p, _, _) if (p.x-400.0).abs()<1.0 && (p.y-300.0).abs()<1.0
        ));
    }

    #[test]
    fn set_value_and_type_and_select() {
        let mut e = engine();
        e.call(
            ToolCall::GetAppState(GetAppStateArgs {
                app: "TextEdit".into(),
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        let doc = e
            .state(4242)
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.role == "text area")
            .unwrap()
            .index;
        e.call(
            ToolCall::SetValue(SetValueArgs {
                app: "TextEdit".into(),
                element_index: doc,
                value: "World".into(),
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        assert!(matches!(
            e.backend().events.last().unwrap(),
            Event::SetValue(5, v) if v == "World"
        ));
        e.call(
            ToolCall::SelectText(SelectTextArgs {
                app: "TextEdit".into(),
                element_index: doc,
                text: Some("or".into()),
                occurrence: 1,
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        assert!(matches!(
            e.backend().events.last().unwrap(),
            Event::SelectText(5, Some(t), 1) if t == "or"
        ));
    }

    #[test]
    fn press_key_sequences() {
        let mut e = engine();
        e.call(
            ToolCall::GetAppState(GetAppStateArgs {
                app: "TextEdit".into(),
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        e.call(
            ToolCall::PressKey(PressKeyArgs {
                app: "TextEdit".into(),
                key: "cmd+a Delete".into(),
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        let keys: Vec<_> = e
            .backend()
            .events
            .iter()
            .filter_map(|ev| match ev {
                Event::Key(_, k) => Some(k.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(keys, vec!["meta+a", "Delete"]);
    }

    #[test]
    fn unknown_index_is_a_tool_error() {
        let mut e = engine();
        e.call(
            ToolCall::GetAppState(GetAppStateArgs {
                app: "TextEdit".into(),
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": 999}),
            &mut allow(),
        );
        assert!(out.is_error);
        assert!(out.text.contains("Element indices are only valid"));
    }

    #[test]
    fn blocked_app_is_refused() {
        let mut backend = MockBackend::new();
        let mut term = MockBackend::text_editor(10);
        term.info.name = "iTerm2".into();
        term.info.id = "com.googlecode.iterm2".into();
        backend.add_app(term);
        let mut cfg = Config::default();
        cfg.approvals.mode = ApprovalMode::AllowAll;
        let mut e =
            Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
        let out = e.call_tool(
            "get_app_state",
            serde_json::json!({"app": "iTerm2"}),
            &mut allow(),
        );
        assert!(out.is_error);
        assert!(out.text.contains("terminal"));
    }

    #[test]
    fn approval_denied_blocks() {
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(7));
        let mut e = Engine::new(backend, ConfigStore::in_memory(Config::default()))
            .with_time(Instant::now, |_| {});
        let out = e.call_tool(
            "get_app_state",
            serde_json::json!({"app": "TextEdit"}),
            &mut DenyApprover,
        );
        assert!(out.is_error);
        assert!(out.text.contains("denied"));
    }

    #[test]
    fn launch_makes_app_available() {
        let mut backend = MockBackend::new();
        backend.add_launchable("notes", {
            let mut a = MockBackend::text_editor(55);
            a.info.name = "Notes".into();
            a.info.id = "com.apple.Notes".into();
            a.info.exe = Some("/System/Applications/Notes.app".into());
            a
        });
        let mut cfg = Config::default();
        cfg.approvals.mode = ApprovalMode::AllowAll;
        let mut e =
            Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
        let out = e
            .call(
                ToolCall::LaunchApp(LaunchAppArgs {
                    app: "Notes".into(),
                }),
                &mut allow(),
            )
            .unwrap();
        assert!(out.text.contains("Launched Notes"));
        let out = e.call_tool(
            "get_app_state",
            serde_json::json!({"app": "Notes"}),
            &mut allow(),
        );
        assert!(!out.is_error);
    }

    #[test]
    fn cannot_launch_a_terminal() {
        let mut e = engine();
        let out = e.call_tool(
            "launch_app",
            serde_json::json!({"app": "xterm"}),
            &mut allow(),
        );
        assert!(out.is_error);
        assert!(out.text.contains("terminal"));
    }

    #[test]
    fn resolve_by_pid_and_ambiguity() {
        let a = AppInfo {
            name: "Safari".into(),
            id: "com.apple.Safari".into(),
            pid: 1,
            exe: None,
            frontmost: false,
            hidden: false,
        };
        let b = AppInfo {
            name: "Safari Technology Preview".into(),
            id: "com.apple.SafariTechnologyPreview".into(),
            pid: 2,
            exe: None,
            frontmost: true,
            hidden: false,
        };
        let apps = vec![a.clone(), b.clone()];
        assert_eq!(resolve_app_in(&apps, "1").unwrap().pid, 1);
        assert_eq!(resolve_app_in(&apps, "com.apple.Safari").unwrap().pid, 1);
        // Exact name beats the frontmost tiebreak.
        assert_eq!(resolve_app_in(&apps, "Safari").unwrap().pid, 1);
        // "safari" (lowercased) is only a substring of both -> frontmost wins.
        assert_eq!(resolve_app_in(&apps, "afari").unwrap().pid, 2);
        assert!(resolve_app_in(&apps, "Firefox").is_err());
    }
}
