//! An in-memory backend: a tiny scriptable desktop. It lets the engine, the
//! tool contract and the MCP server be tested on any OS with no real UI.

use std::collections::HashMap;

use crate::backend::{Backend, Native};
use crate::error::{Error, Result};
use crate::keys::KeyCombo;
use crate::types::{
    ActionDesc, AppInfo, Capture, ElementHandle, InputTarget, MouseButton, NodeStates,
    PermissionStatus, Point, RawNode, Rect, ScrollDirection, SnapshotOptions, WindowInfo,
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
    ScrollWheel(u32, Point, i32, i32),
    Key(u32, String),
    Type(u32, String),
    Launch(String),
}

#[derive(Debug, Default)]
pub struct MockBackend {
    apps: Vec<MockApp>,
    pub events: Vec<Event>,
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
    /// Backend calls, for cache assertions.
    pub snapshots: usize,
    pub captures: usize,
    pub window_lists: usize,
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

    fn launch_app(&mut self, query: &str) -> Result<()> {
        self.events.push(Event::Launch(query.into()));
        if let Some(app) = self.launchable.remove(&query.to_lowercase()) {
            self.apps.push(app);
            Ok(())
        } else if self
            .apps
            .iter()
            .any(|a| a.info.match_keys().contains(&query.to_lowercase()))
        {
            Ok(())
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
                minimized: false,
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
        Ok(fake_capture(b, self.fill))
    }

    fn capture_screen(&mut self, region: Option<Rect>) -> Result<Capture> {
        self.captures += 1;
        Ok(fake_capture(
            region.unwrap_or(Rect::new(0.0, 0.0, 1280.0, 800.0)),
            self.fill,
        ))
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
        let e = self.element_mut(element)?;
        e.value = Some(value.into());
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
        Ok(())
    }

    fn drag(&mut self, target: &InputTarget, from: Point, to: Point) -> Result<()> {
        self.events.push(Event::Drag(target.pid, from, to));
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
        if let Some(app) = self.app_mut(target.pid)
            && let Some(field) = app
                .elements
                .iter_mut()
                .find(|e| e.states.editable && e.states.focused)
        {
            let v = field.value.get_or_insert_with(String::new);
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
