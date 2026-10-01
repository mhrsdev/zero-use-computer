//! An in-memory backend: a tiny scriptable desktop. It lets the engine, the
//! tool contract and the MCP server be tested on any OS with no real UI.

use std::collections::HashMap;

use crate::backend::{Backend, Native};
use crate::error::{Error, Result};
use crate::keys::KeyCombo;
use crate::types::{
    ActionDesc, AppInfo, Capture, Display, ElementHandle, InputTarget, MouseButton, NodeStates,
    Notification, OcrLine, PermissionStatus, Point, RawNode, Rect, ScrollDirection,
    SnapshotOptions, WindowInfo, WindowOp,
};

/// A scriptable element.
#[derive(Debug, Clone)]
pub struct MockElement {
    pub handle: ElementHandle,
    pub parent: Option<ElementHandle>,
    pub role: String,
    pub name: Option<String>,
    pub value: Option<String>,
    pub bounds: Rect,
    pub actions: Vec<String>,
    pub states: NodeStates,
}

impl MockElement {
    pub fn new(handle: ElementHandle, role: &str, name: &str, bounds: Rect) -> Self {
        Self {
            handle,
            parent: None,
            role: role.into(),
            name: (!name.is_empty()).then(|| name.to_string()),
            value: None,
            bounds,
            actions: Vec::new(),
            states: NodeStates {
                enabled: true,
                ..Default::default()
            },
        }
    }

    pub fn child_of(mut self, parent: ElementHandle) -> Self {
        self.parent = Some(parent);
        self
    }

    pub fn with_actions(mut self, actions: &[&str]) -> Self {
        self.actions = actions.iter().map(|s| s.to_string()).collect();
        self
    }

    pub fn editable(mut self) -> Self {
        self.states.editable = true;
        self.states.value_settable = true;
        self
    }
}

#[derive(Debug, Clone)]
pub struct MockWindow {
    pub id: u64,
    pub title: String,
    pub bounds: Rect,
    /// Root element handle of this window.
    pub root: ElementHandle,
    pub focused: bool,
}

#[derive(Debug, Clone)]
pub struct MockApp {
    pub info: AppInfo,
    pub windows: Vec<MockWindow>,
    pub elements: Vec<MockElement>,
}

/// A recorded synthesized-input event, for assertions.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Action(ElementHandle, String),
    SetValue(ElementHandle, String),
    SelectText(ElementHandle, Option<String>, usize),
    Focus(ElementHandle),
    ScrollElement(ElementHandle, ScrollDirection, f64),
    Click(u32, Point, MouseButton, u8),
    Drag(u32, Point, Point),
    /// The pointer moved without a click (keys for what is under it).
    Hover(u32, Point),
    /// A `draw` gesture: button down, pointer moves, button up.
    PointerDown(u32, Point, MouseButton),
    PointerMove(u32, Point),
    PointerUp(u32, Point, MouseButton),
    ScrollWheel(u32, Point, i32, i32),
    Key(u32, String),
    Type(u32, String),
    Launch(String),
}

#[derive(Debug, Default)]
pub struct MockBackend {
    apps: Vec<MockApp>,
    pub events: Vec<Event>,
    /// Where the pointer is, when known (move_pointer reports it to be put
    /// back).
    pub pointer: Option<Point>,
    /// Elements whose native action support is disabled, to force fallbacks.
    pub no_native: std::collections::HashSet<ElementHandle>,
    /// Apps that appear (by name) only after launch_app is called.
    pub launchable: HashMap<String, MockApp>,
    /// Current clipboard contents.
    pub clipboard: String,
    /// Scripted navigation: pressing this element replaces the app (same pid)
    /// with the given state — a new page, a dialog, or the previous page.
    pub on_press: HashMap<ElementHandle, MockApp>,
    /// Brightness of captured pixels (tests change it to alter the picture).
    pub fill: u8,
    /// A screen area drawn in another brightness (a local change).
    pub patch: Option<(Rect, u8)>,
    /// What the built-in OCR "reads" (None = no built-in OCR), and how
    /// often it ran.
    pub ocr_text: Option<Vec<OcrLine>>,
    pub ocr_runs: usize,
    /// Notifications "received".
    pub notes: Vec<Notification>,
    /// Minimized windows, and window operations performed.
    pub minimized: std::collections::HashSet<u64>,
    pub window_ops: Vec<(u64, WindowOp)>,
    /// Backend calls, for cache assertions.
    pub snapshots: usize,
    pub captures: usize,
    pub window_lists: usize,
    /// Elements whose accessibility action fails.
    pub fail_actions: std::collections::HashSet<ElementHandle>,
    /// Elements that accept set_value but keep their old value.
    pub ignore_set_value: std::collections::HashSet<ElementHandle>,
    /// Elements whose focus() reports success without focusing.
    pub fake_focus: std::collections::HashSet<ElementHandle>,
    /// Windows refuse to come to the front (like Windows' focus-stealing
    /// prevention).
    pub refuse_focus: bool,
    /// A "select all" waiting for typed text to replace it.
    select_all: Option<ElementHandle>,
    /// Values to apply before successive snapshots (a UI still updating).
    pub snapshot_script: std::collections::VecDeque<(ElementHandle, String)>,
    /// What `user_idle` reports (None = unknown); `idle_script` values are
    /// reported first, one per call.
    pub idle: Option<std::time::Duration>,
    pub idle_script: std::collections::VecDeque<std::time::Duration>,
    next_handle: ElementHandle,
}

impl MockBackend {
    pub fn new() -> Self {
        Self {
            next_handle: 1000,
            fill: 200,
            ..Default::default()
        }
    }

    pub fn add_app(&mut self, app: MockApp) {
        self.apps.push(app);
    }

    pub fn add_launchable(&mut self, name: &str, app: MockApp) {
        self.launchable.insert(name.to_lowercase(), app);
    }

    pub fn app_mut(&mut self, pid: u32) -> Option<&mut MockApp> {
        self.apps.iter_mut().find(|a| a.info.pid == pid)
    }

    fn element(&self, handle: ElementHandle) -> Result<&MockElement> {
        self.apps
            .iter()
            .flat_map(|a| &a.elements)
            .find(|e| e.handle == handle)
            .ok_or_else(|| Error::Internal(format!("mock: no element {handle}")))
    }

    fn element_mut(&mut self, handle: ElementHandle) -> Result<&mut MockElement> {
        self.apps
            .iter_mut()
            .flat_map(|a| &mut a.elements)
            .find(|e| e.handle == handle)
            .ok_or_else(|| Error::Internal(format!("mock: no element {handle}")))
    }

    /// Build a simple TextEdit-like app with a toolbar and a text field.
    pub fn text_editor(pid: u32) -> MockApp {
        let win = Rect::new(0.0, 0.0, 800.0, 600.0);
        let mut elements = vec![
            MockElement::new(1, "window", "Untitled", win),
            MockElement::new(2, "toolbar", "", Rect::new(0.0, 0.0, 800.0, 40.0)).child_of(1),
            MockElement::new(3, "button", "Bold", Rect::new(10.0, 8.0, 60.0, 24.0))
                .child_of(2)
                .with_actions(&["AXPress"]),
            MockElement::new(
                4,
                "pop up button",
                "Style",
                Rect::new(80.0, 8.0, 100.0, 24.0),
            )
            .child_of(2)
            .with_actions(&["AXPress", "AXShowMenu"]),
            {
                let mut e = MockElement::new(
                    5,
                    "text area",
                    "Document",
                    Rect::new(0.0, 40.0, 800.0, 560.0),
                )
                .child_of(1)
                .editable();
                e.value = Some("Hello".into());
                e
            },
        ];
        for e in &mut elements {
            e.states.enabled = true;
        }
        MockApp {
            info: AppInfo {
                name: "TextEdit".into(),
                id: "com.apple.TextEdit".into(),
                pid,
                exe: Some("/System/Applications/TextEdit.app".into()),
                frontmost: true,
                hidden: false,
            },
            windows: vec![MockWindow {
                id: 1,
                title: "Untitled".into(),
                bounds: win,
                root: 1,
                focused: true,
            }],
            elements,
        }
    }

    /// Give `element` keyboard focus (and take it from the rest of its app).
    fn set_focus(&mut self, element: ElementHandle) {
        for app in &mut self.apps {
            if app.elements.iter().any(|e| e.handle == element) {
                for e in &mut app.elements {
                    e.states.focused = e.handle == element;
                }
            }
        }
    }

    /// Draw the patch (if any) into a capture.
    fn paint(&self, mut cap: Capture) -> Capture {
        if let Some((r, v)) = self.patch {
            for y in 0..cap.height {
                for x in 0..cap.width {
                    let p = Point::new(cap.bounds.x + f64::from(x), cap.bounds.y + f64::from(y));
                    if r.contains(p) {
                        let i = ((y * cap.width + x) * 4) as usize;
                        cap.rgba[i..i + 3].fill(v);
                    }
                }
            }
        }
        cap
    }

    pub fn alloc(&mut self) -> ElementHandle {
        self.next_handle += 1;
        self.next_handle
    }
}

impl Backend for MockBackend {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn permissions(&mut self) -> Vec<PermissionStatus> {
        vec![PermissionStatus {
            name: "mock".into(),
            granted: true,
            detail: "always granted".into(),
        }]
    }

    fn list_apps(&mut self) -> Result<Vec<AppInfo>> {
        Ok(self.apps.iter().map(|a| a.info.clone()).collect())
    }

    fn launch_app(&mut self, query: &str) -> Result<Option<String>> {
        self.events.push(Event::Launch(query.into()));
        if let Some(app) = self.launchable.remove(&query.to_lowercase()) {
            // Like a catalog lookup: report the program when it differs.
            let program = app.info.exe.clone().filter(|_| {
                !app.info
                    .match_keys()
                    .iter()
                    .any(|k| k.contains(&query.to_lowercase()))
            });
            self.apps.push(app);
            Ok(program)
        } else if self
            .apps
            .iter()
            .any(|a| a.info.match_keys().contains(&query.to_lowercase()))
        {
            Ok(None)
        } else {
            Err(Error::AppNotFound(query.into()))
        }
    }

    fn list_windows(&mut self, app: &AppInfo) -> Result<Vec<WindowInfo>> {
        self.window_lists += 1;
        let a = self
            .apps
            .iter()
            .find(|a| a.info.pid == app.pid)
            .ok_or_else(|| Error::AppNotFound(app.name.clone()))?;
        Ok(a.windows
            .iter()
            .map(|w| WindowInfo {
                id: w.id,
                title: w.title.clone(),
                bounds: Some(w.bounds),
                focused: w.focused,
                main: w.focused,
                minimized: self.minimized.contains(&w.id),
                handle: w.root,
            })
            .collect())
    }

    fn snapshot(
        &mut self,
        app: &AppInfo,
        window: &WindowInfo,
        _opts: &SnapshotOptions,
    ) -> Result<Vec<RawNode>> {
        self.snapshots += 1;
        if let Some((h, v)) = self.snapshot_script.pop_front()
            && let Ok(e) = self.element_mut(h)
        {
            e.value = Some(v);
        }
        let a = self
            .apps
            .iter()
            .find(|a| a.info.pid == app.pid)
            .ok_or_else(|| Error::AppNotFound(app.name.clone()))?;
        // Collect the subtree rooted at the window, pre-order.
        let mut nodes = Vec::new();
        let mut handle_to_idx: HashMap<ElementHandle, usize> = HashMap::new();
        fn walk(
            root: ElementHandle,
            parent_idx: Option<usize>,
            elements: &[MockElement],
            nodes: &mut Vec<RawNode>,
            map: &mut HashMap<ElementHandle, usize>,
        ) {
            let Some(e) = elements.iter().find(|e| e.handle == root) else {
                return;
            };
            let idx = nodes.len();
            map.insert(e.handle, idx);
            nodes.push(RawNode {
                handle: e.handle,
                parent: parent_idx,
                key: Some(format!("h{}", e.handle)),
                role: e.role.clone(),
                native_role: e.role.clone(),
                name: e.name.clone(),
                value: e.value.clone(),
                bounds: Some(e.bounds),
                actions: e
                    .actions
                    .iter()
                    .map(|a| ActionDesc::new(crate::roles::ax_action(a), a.clone()))
                    .collect(),
                states: e.states.clone(),
                ..Default::default()
            });
            let children: Vec<ElementHandle> = elements
                .iter()
                .filter(|c| c.parent == Some(root))
                .map(|c| c.handle)
                .collect();
            for c in children {
                walk(c, Some(idx), elements, nodes, map);
            }
        }
        walk(
            window.handle,
            None,
            &a.elements,
            &mut nodes,
            &mut handle_to_idx,
        );
        Ok(nodes)
    }

    fn capture(&mut self, _app: &AppInfo, window: &WindowInfo) -> Result<Capture> {
        self.captures += 1;
        let b = window.bounds.unwrap_or(Rect::new(0.0, 0.0, 100.0, 100.0));
        Ok(self.paint(fake_capture(b, self.fill)))
    }

    fn capture_screen(&mut self, region: Option<Rect>) -> Result<Capture> {
        self.captures += 1;
        Ok(self.paint(fake_capture(
            region.unwrap_or(Rect::new(0.0, 0.0, 1280.0, 800.0)),
            self.fill,
        )))
    }

    fn ocr(&mut self, _cap: &Capture, _languages: &[String]) -> Result<Vec<OcrLine>> {
        match &self.ocr_text {
            Some(lines) => {
                self.ocr_runs += 1;
                Ok(lines.clone())
            }
            None => Err(Error::Unsupported("mock: no OCR".into())),
        }
    }

    fn notifications(&mut self) -> Result<Vec<Notification>> {
        Ok(self.notes.clone())
    }

    fn displays(&mut self) -> Result<Vec<Display>> {
        Ok(vec![
            Display {
                index: 0,
                bounds: Rect::new(0.0, 0.0, 1280.0, 800.0),
                work_area: Rect::new(0.0, 0.0, 1280.0, 760.0),
                primary: true,
            },
            Display {
                index: 1,
                bounds: Rect::new(1280.0, 0.0, 1920.0, 1080.0),
                work_area: Rect::new(1280.0, 0.0, 1920.0, 1040.0),
                primary: false,
            },
        ])
    }

    fn window_op(&mut self, app: &AppInfo, window: &WindowInfo, op: &WindowOp) -> Result<()> {
        self.window_ops.push((window.id, *op));
        let a = self
            .app_mut(app.pid)
            .ok_or_else(|| Error::AppNotFound(app.name.clone()))?;
        let i = a
            .windows
            .iter()
            .position(|w| w.id == window.id)
            .ok_or_else(|| Error::ActionFailed("mock: no such window".into()))?;
        match op {
            WindowOp::SetBounds(r) => {
                // Like real apps: a minimum size.
                let r = Rect::new(r.x, r.y, r.width.max(200.0), r.height.max(150.0));
                a.windows[i].bounds = r;
                let root = a.windows[i].root;
                if let Some(e) = a.elements.iter_mut().find(|e| e.handle == root) {
                    e.bounds = r;
                }
            }
            WindowOp::Close => {
                a.windows.remove(i);
            }
            WindowOp::Minimize => {
                self.minimized.insert(window.id);
            }
            WindowOp::Restore => {
                self.minimized.remove(&window.id);
            }
            WindowOp::Focus => {
                self.minimized.remove(&window.id);
                if !self.refuse_focus {
                    let pid = app.pid;
                    for other in &mut self.apps {
                        other.info.frontmost = other.info.pid == pid;
                    }
                }
            }
            WindowOp::ToDesktop(_) => {
                return Err(Error::Unsupported("mock: no virtual desktops".into()));
            }
            _ => {}
        }
        Ok(())
    }

    fn user_idle(&mut self) -> Option<std::time::Duration> {
        self.idle_script.pop_front().or(self.idle)
    }

    fn clipboard_get(&mut self) -> Result<String> {
        Ok(self.clipboard.clone())
    }

    fn clipboard_set(&mut self, text: &str) -> Result<()> {
        self.clipboard = text.to_string();
        Ok(())
    }

    fn perform_action(&mut self, element: ElementHandle, native_action: &str) -> Result<()> {
        self.element(element)?;
        if self.fail_actions.contains(&element) {
            return Err(Error::ActionFailed("mock: action failed".into()));
        }
        self.events
            .push(Event::Action(element, native_action.into()));
        if let Some(next) = self.on_press.get(&element).cloned()
            && let Some(app) = self.app_mut(next.info.pid)
        {
            *app = next;
        }
        Ok(())
    }

    fn set_value(&mut self, element: ElementHandle, value: &str) -> Result<()> {
        let ignore = self.ignore_set_value.contains(&element);
        let e = self.element_mut(element)?;
        if !ignore {
            e.value = Some(value.into());
        }
        self.events.push(Event::SetValue(element, value.into()));
        Ok(())
    }

    fn select_text(
        &mut self,
        element: ElementHandle,
        text: Option<&str>,
        occurrence: usize,
    ) -> Result<()> {
        self.element(element)?;
        if text.is_none() {
            self.select_all = Some(element);
        }
        self.events.push(Event::SelectText(
            element,
            text.map(String::from),
            occurrence,
        ));
        Ok(())
    }

    fn focus(&mut self, element: ElementHandle) -> Result<Native> {
        self.element(element)?;
        self.events.push(Event::Focus(element));
        if !self.fake_focus.contains(&element) {
            self.set_focus(element);
        }
        Ok(Native::Done("focused".into()))
    }

    fn scroll_element(
        &mut self,
        element: ElementHandle,
        direction: ScrollDirection,
        pages: f64,
    ) -> Result<Native> {
        self.element(element)?;
        if self.no_native.contains(&element) {
            return Ok(Native::Unsupported);
        }
        self.events
            .push(Event::ScrollElement(element, direction, pages));
        Ok(Native::Done("scrolled".into()))
    }

    fn click(
        &mut self,
        target: &InputTarget,
        at: Point,
        button: MouseButton,
        count: u8,
    ) -> Result<()> {
        self.events
            .push(Event::Click(target.pid, at, button, count));
        // Clicking a text field focuses it.
        let hit = self.app_mut(target.pid).and_then(|app| {
            app.elements
                .iter()
                .filter(|e| e.states.editable && e.bounds.contains(at))
                .min_by(|a, b| {
                    (a.bounds.width * a.bounds.height)
                        .total_cmp(&(b.bounds.width * b.bounds.height))
                })
                .map(|e| e.handle)
        });
        if let Some(h) = hit {
            self.set_focus(h);
        }
        Ok(())
    }

    fn drag(&mut self, target: &InputTarget, from: Point, to: Point) -> Result<()> {
        self.events.push(Event::Drag(target.pid, from, to));
        Ok(())
    }

    fn move_pointer(&mut self, target: &InputTarget, at: Point) -> Result<Option<Point>> {
        self.events.push(Event::Hover(target.pid, at));
        Ok(self.pointer.replace(at))
    }

    fn draw(
        &mut self,
        target: &InputTarget,
        strokes: &[Vec<Point>],
        button: MouseButton,
        pace: &mut dyn FnMut(f64) -> Result<()>,
    ) -> Result<()> {
        let pid = target.pid;
        for stroke in strokes {
            let Some(&first) = stroke.first() else {
                continue;
            };
            self.events.push(Event::PointerDown(pid, first, button));
            let mut last = first;
            for &p in &stroke[1..] {
                if let Err(e) = pace((p.x - last.x).hypot(p.y - last.y)) {
                    self.events.push(Event::PointerUp(pid, last, button));
                    return Err(e);
                }
                self.events.push(Event::PointerMove(pid, p));
                last = p;
            }
            self.events.push(Event::PointerUp(pid, last, button));
        }
        Ok(())
    }

    fn scroll_wheel(&mut self, target: &InputTarget, at: Point, dx: i32, dy: i32) -> Result<()> {
        self.events.push(Event::ScrollWheel(target.pid, at, dx, dy));
        Ok(())
    }

    fn press_key(&mut self, target: &InputTarget, combo: &KeyCombo) -> Result<()> {
        self.events.push(Event::Key(target.pid, combo.to_string()));
        Ok(())
    }

    fn type_text(&mut self, target: &InputTarget, text: &str) -> Result<()> {
        let replace = self.select_all.take();
        if let Some(app) = self.app_mut(target.pid)
            && let Some(field) = app
                .elements
                .iter_mut()
                .find(|e| e.states.editable && e.states.focused)
        {
            let v = field.value.get_or_insert_with(String::new);
            if replace == Some(field.handle) {
                v.clear();
            }
            v.push_str(text);
        }
        self.events.push(Event::Type(target.pid, text.into()));
        Ok(())
    }
}

fn fake_capture(b: Rect, fill: u8) -> Capture {
    let (w, h) = (b.width.max(1.0) as u32, b.height.max(1.0) as u32);
    Capture {
        width: w,
        height: h,
        rgba: vec![fill; (w * h * 4) as usize],
        bounds: b,
    }
}
