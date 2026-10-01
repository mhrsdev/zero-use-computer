//! Windows backend: UI Automation for the tree and semantic actions,
//! `SendInput` for input, and `PrintWindow` for capture.
//!
//! Element-level actions (Invoke, Toggle, Value, scroll patterns, focus) go
//! through UI Automation and work without raising the window; coordinate
//! input via SendInput is the fallback and, as on Codex for Windows, operates
//! on the active desktop.

mod capture;
mod clipboard;
pub(crate) mod input;
mod notify;
mod ocr;
mod wm;

use std::collections::{HashMap, HashSet};
use std::process::Command;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::System::Variant::{
    VARIANT, VariantToBoolean, VariantToInt32, VariantToInt32Array, VariantToStringAlloc,
};
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GA_ROOT, GetAncestor, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsWindowVisible,
};
use windows::core::{BOOL, BSTR, Interface, PWSTR};

use crate::backend::{Backend, Native};
use crate::error::{Error, Result};
use crate::keys::KeyCombo;
use crate::roles;
use crate::types::*;

pub struct WindowsBackend {
    automation: IUIAutomation,
    walker: IUIAutomationTreeWalker,
    handles: HashMap<ElementHandle, (u32, IUIAutomationElement)>,
    /// Window handles from `list_windows`: (pid, window element), one per
    /// HWND for as long as the window is listed. Kept apart from `handles`
    /// so a snapshot doesn't invalidate a window list callers still reuse.
    window_handles: HashMap<ElementHandle, (u32, IUIAutomationElement)>,
    hwnds: HashMap<ElementHandle, isize>,
    next_handle: ElementHandle,
    /// Walk with a CacheRequest (one cross-process call per window).
    use_cache_request: bool,
    cache_request: Option<IUIAutomationCacheRequest>,
    /// Apps whose subtree cache request failed or timed out (huge or hung
    /// trees): walked node by node, within the snapshot limits, instead.
    uncached_pids: HashSet<u32>,
}

/// Upper bound for one UI Automation cross-process transaction (a subtree
/// cache request of a huge tree, or a hung app): past it the call fails
/// instead of blocking.
const UIA_TRANSACTION_TIMEOUT_MS: u32 = 5_000;
const UIA_CONNECTION_TIMEOUT_MS: u32 = 3_000;

/// Properties prefetched for every element by the cache request.
const CACHED_PROPS: [UIA_PROPERTY_ID; 22] = [
    UIA_RuntimeIdPropertyId,
    UIA_ControlTypePropertyId,
    UIA_NamePropertyId,
    UIA_AutomationIdPropertyId,
    UIA_ClassNamePropertyId,
    UIA_BoundingRectanglePropertyId,
    UIA_IsEnabledPropertyId,
    UIA_HasKeyboardFocusPropertyId,
    UIA_IsOffscreenPropertyId,
    UIA_IsInvokePatternAvailablePropertyId,
    UIA_IsTogglePatternAvailablePropertyId,
    UIA_IsValuePatternAvailablePropertyId,
    UIA_IsExpandCollapsePatternAvailablePropertyId,
    UIA_IsSelectionItemPatternAvailablePropertyId,
    UIA_IsLegacyIAccessiblePatternAvailablePropertyId,
    UIA_ValueValuePropertyId,
    UIA_ValueIsReadOnlyPropertyId,
    UIA_ToggleToggleStatePropertyId,
    UIA_ExpandCollapseExpandCollapseStatePropertyId,
    UIA_SelectionItemIsSelectedPropertyId,
    UIA_LegacyIAccessibleDefaultActionPropertyId,
    UIA_IsPasswordPropertyId,
];

/// Work in physical pixels, as UI Automation does. Without this, at 125%
/// or 150% display scaling Windows gives this process scaled window
/// rectangles, screen sizes and screen captures: screenshots come out
/// cropped or blurred and clicks land off target.
pub(crate) fn make_dpi_aware() {
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };
    // SAFETY: process-wide settings, made before any window or metric is read.
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_err() {
            // Already set (a manifest, an earlier call) or Windows before
            // 10 1703: at least system-DPI awareness.
            let _ = windows::Win32::UI::WindowsAndMessaging::SetProcessDPIAware();
        }
    }
}

/// UI Automation's runtime id, as an identity key: it stays the same for
/// as long as the element exists, so an element index can't move to a
/// look-alike element (the next row's "Delete") when one disappears.
fn runtime_key(v: windows::core::Result<VARIANT>) -> Option<String> {
    let v = v.ok()?;
    let mut ids = [0i32; 16];
    let mut n = 0u32;
    // SAFETY: reads an int array out of a VARIANT into a local buffer.
    unsafe { VariantToInt32Array(&v, &mut ids, &mut n) }.ok()?;
    let ids = ids.get(..n as usize).filter(|s| !s.is_empty())?;
    Some(format!(
        "uia:{}",
        ids.iter().map(i32::to_string).collect::<Vec<_>>().join(".")
    ))
}

impl WindowsBackend {
    pub fn new() -> Result<Self> {
        make_dpi_aware();
        unsafe {
            // Ignore RPC_E_CHANGED_MODE if COM is already initialized.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).map_err(|e| {
                    Error::Platform(format!("CoCreateInstance(CUIAutomation): {e}"))
                })?;
            // Bound every cross-process call (Windows 8+).
            if let Ok(a2) = automation.cast::<IUIAutomation2>() {
                let _ = a2.SetTransactionTimeout(UIA_TRANSACTION_TIMEOUT_MS);
                let _ = a2.SetConnectionTimeout(UIA_CONNECTION_TIMEOUT_MS);
            }
            let walker = automation
                .ControlViewWalker()
                .map_err(|e| Error::Platform(format!("ControlViewWalker: {e}")))?;
            Ok(Self {
                automation,
                walker,
                handles: HashMap::new(),
                window_handles: HashMap::new(),
                hwnds: HashMap::new(),
                next_handle: 1,
                use_cache_request: true,
                cache_request: None,
                uncached_pids: HashSet::new(),
            })
        }
    }

    fn handle_for(&mut self, pid: u32, el: IUIAutomationElement) -> ElementHandle {
        let h = self.next_handle;
        self.next_handle += 1;
        self.handles.insert(h, (pid, el));
        h
    }

    fn resolve(&self, handle: ElementHandle) -> Result<IUIAutomationElement> {
        self.handles
            .get(&handle)
            .or_else(|| self.window_handles.get(&handle))
            .map(|(_, el)| el.clone())
            .ok_or_else(|| Error::Internal(format!("stale element handle {handle}")))
    }

    fn top_level_windows(&self) -> Vec<HWND> {
        let mut hwnds: Vec<HWND> = Vec::new();
        unsafe {
            let _ = EnumWindows(
                Some(enum_proc),
                LPARAM(&mut hwnds as *mut Vec<HWND> as isize),
            );
        }
        hwnds
            .into_iter()
            .filter(|&h| unsafe { IsWindowVisible(h).as_bool() })
            .filter(|&h| unsafe { GetAncestor(h, GA_ROOT) } == h)
            .filter(|&h| unsafe { GetWindowTextLengthW(h) } > 0)
            .collect()
    }

    fn window_title(hwnd: HWND) -> String {
        unsafe {
            let len = GetWindowTextLengthW(hwnd);
            if len <= 0 {
                return String::new();
            }
            let mut buf = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(hwnd, &mut buf);
            String::from_utf16_lossy(&buf[..n as usize])
        }
    }

    fn build_node(
        &mut self,
        pid: u32,
        el: &IUIAutomationElement,
        parent: Option<usize>,
    ) -> RawNode {
        let control_type = unsafe { el.CurrentControlType() }.map(|c| c.0).unwrap_or(0);
        let password = unsafe { el.CurrentIsPassword() }.is_ok_and(|b| b.as_bool());
        let role = if password {
            "secure text field".to_string()
        } else {
            roles::from_uia(control_type)
        };
        let name = bstr(unsafe { el.CurrentName() });
        let automation_id = bstr(unsafe { el.CurrentAutomationId() });
        let class = bstr(unsafe { el.CurrentClassName() });

        let bounds = unsafe { el.CurrentBoundingRectangle() }
            .ok()
            .map(rect_to_bounds);
        let enabled = unsafe { el.CurrentIsEnabled() }
            .map(|b| b.as_bool())
            .unwrap_or(true);
        let focused = unsafe { el.CurrentHasKeyboardFocus() }
            .map(|b| b.as_bool())
            .unwrap_or(false);
        let offscreen = unsafe { el.CurrentIsOffscreen() }
            .map(|b| b.as_bool())
            .unwrap_or(false);

        // Value pattern → text value / editability.
        let value_pat = self.value_pattern(el);
        let (mut value, mut editable, mut value_settable) = (None, false, false);
        if let Some(v) = &value_pat {
            // Never read a password field's contents.
            value = (!password)
                .then(|| bstr(unsafe { v.CurrentValue() }))
                .flatten();
            let readonly = unsafe { v.CurrentIsReadOnly() }
                .map(|b| b.as_bool())
                .unwrap_or(true);
            value_settable = !readonly;
            editable = !readonly;
        }
        if matches!(
            role.as_str(),
            "text field" | "document" | "secure text field"
        ) {
            editable = editable || value_settable;
        }

        // Toggle → checked.
        let mut checked = None;
        if available(el, UIA_IsTogglePatternAvailablePropertyId)
            && let Some(t) = self.toggle_pattern(el)
        {
            checked = Some(
                unsafe { t.CurrentToggleState() }
                    .map(|s| s == ToggleState_On)
                    .unwrap_or(false),
            );
            value = None;
        }

        // Expand/collapse → expanded.
        let mut expanded = None;
        if available(el, UIA_IsExpandCollapsePatternAvailablePropertyId)
            && let Some(ec) = self.expand_pattern(el)
        {
            expanded = unsafe { ec.CurrentExpandCollapseState() }
                .ok()
                .map(|s| s == ExpandCollapseState_Expanded);
        }

        let selected = self
            .selection_item(el)
            .and_then(|s| unsafe { s.CurrentIsSelected() }.ok())
            .map(|b| b.as_bool())
            .unwrap_or(false);

        let mut actions = Vec::new();
        if available(el, UIA_IsInvokePatternAvailablePropertyId) {
            actions.push(ActionDesc::new("press", "Invoke"));
        }
        if available(el, UIA_IsTogglePatternAvailablePropertyId) {
            actions.push(ActionDesc::new("toggle", "Toggle"));
        }
        if available(el, UIA_IsExpandCollapsePatternAvailablePropertyId) {
            actions.push(ActionDesc::new("expand", "Expand"));
            actions.push(ActionDesc::new("collapse", "Collapse"));
        }
        if available(el, UIA_IsSelectionItemPatternAvailablePropertyId) {
            actions.push(ActionDesc::new("select", "Select"));
        }
        if let Some(leg) = self.legacy_pattern(el)
            && let Some(default) = bstr(unsafe { leg.CurrentDefaultAction() })
            && !default.is_empty()
            && !actions.iter().any(|a| a.name == "press")
        {
            actions.push(ActionDesc::new(default.to_lowercase(), "DoDefaultAction"));
        }

        let identifier = automation_id
            .filter(|s| !s.is_empty())
            .or(class.filter(|s| !s.is_empty()));

        let key = runtime_key(unsafe { el.GetCurrentPropertyValue(UIA_RuntimeIdPropertyId) });
        let handle = self.handle_for(pid, el.clone());
        RawNode {
            handle,
            parent,
            key,
            role,
            native_role: control_type.to_string(),
            name: name.filter(|s| !s.is_empty()),
            description: None,
            value: value.filter(|s| !s.is_empty()),
            placeholder: None,
            identifier,
            bounds,
            actions,
            states: NodeStates {
                enabled,
                focused,
                selected,
                checked,
                expanded,
                editable,
                value_settable,
                hidden: offscreen || bounds.is_some_and(|b| b.is_empty()),
            },
        }
    }

    fn walk(
        &mut self,
        pid: u32,
        el: &IUIAutomationElement,
        parent: Option<usize>,
        depth: usize,
        opts: &SnapshotOptions,
        out: &mut Vec<RawNode>,
    ) {
        if out.len() >= opts.max_nodes || depth > opts.max_depth {
            return;
        }
        let idx = out.len();
        let node = self.build_node(pid, el, parent);
        out.push(node);
        if depth >= opts.max_depth {
            return; // children would be cut anyway: don't enumerate them
        }
        // Bounded even if a provider reports a sibling cycle.
        let mut siblings = 0;
        let mut child = unsafe { self.walker.GetFirstChildElement(el) }.ok();
        while let Some(c) = child {
            if out.len() >= opts.max_nodes || siblings >= opts.max_nodes {
                break;
            }
            siblings += 1;
            self.walk(pid, &c, Some(idx), depth + 1, opts, out);
            child = unsafe { self.walker.GetNextSiblingElement(&c) }.ok();
        }
    }

    /// The subtree cache request (built once, reused for every snapshot).
    fn cache_request(&mut self) -> windows::core::Result<IUIAutomationCacheRequest> {
        if let Some(cr) = &self.cache_request {
            return Ok(cr.clone());
        }
        unsafe {
            let cr = self.automation.CreateCacheRequest()?;
            for prop in CACHED_PROPS {
                cr.AddProperty(prop)?;
            }
            cr.SetTreeScope(TreeScope_Subtree)?;
            cr.SetTreeFilter(&self.automation.ControlViewCondition()?)?;
            self.cache_request = Some(cr.clone());
            Ok(cr)
        }
    }

    /// Build a node from cached properties only (no cross-process calls).
    fn build_node_cached(
        &mut self,
        pid: u32,
        el: &IUIAutomationElement,
        parent: Option<usize>,
    ) -> RawNode {
        let control_type = unsafe { el.CachedControlType() }.map(|c| c.0).unwrap_or(0);
        let password = cached_bool(el, UIA_IsPasswordPropertyId).unwrap_or(false);
        let role = if password {
            "secure text field".to_string()
        } else {
            roles::from_uia(control_type)
        };
        let name = bstr(unsafe { el.CachedName() });
        let automation_id = bstr(unsafe { el.CachedAutomationId() });
        let class = bstr(unsafe { el.CachedClassName() });
        let bounds = unsafe { el.CachedBoundingRectangle() }
            .ok()
            .map(rect_to_bounds);
        let enabled = unsafe { el.CachedIsEnabled() }
            .map(|b| b.as_bool())
            .unwrap_or(true);
        let focused = unsafe { el.CachedHasKeyboardFocus() }
            .map(|b| b.as_bool())
            .unwrap_or(false);
        let offscreen = unsafe { el.CachedIsOffscreen() }
            .map(|b| b.as_bool())
            .unwrap_or(false);

        let has = |p: UIA_PROPERTY_ID| cached_bool(el, p).unwrap_or(false);
        let (mut value, mut editable, mut value_settable) = (None, false, false);
        if has(UIA_IsValuePatternAvailablePropertyId) {
            value = (!password)
                .then(|| cached_string(el, UIA_ValueValuePropertyId))
                .flatten();
            let readonly = cached_bool(el, UIA_ValueIsReadOnlyPropertyId).unwrap_or(true);
            value_settable = !readonly;
            editable = !readonly;
        }
        if matches!(
            role.as_str(),
            "text field" | "document" | "secure text field"
        ) {
            editable = editable || value_settable;
        }
        let mut checked = None;
        if has(UIA_IsTogglePatternAvailablePropertyId) {
            checked = Some(cached_i32(el, UIA_ToggleToggleStatePropertyId) == Some(1));
            value = None;
        }
        let expanded = has(UIA_IsExpandCollapsePatternAvailablePropertyId)
            .then(|| cached_i32(el, UIA_ExpandCollapseExpandCollapseStatePropertyId))
            .flatten()
            .filter(|s| *s != 3) // LeafNode
            .map(|s| s == 1);
        let selected = has(UIA_IsSelectionItemPatternAvailablePropertyId)
            && cached_bool(el, UIA_SelectionItemIsSelectedPropertyId).unwrap_or(false);

        let mut actions = Vec::new();
        if has(UIA_IsInvokePatternAvailablePropertyId) {
            actions.push(ActionDesc::new("press", "Invoke"));
        }
        if has(UIA_IsTogglePatternAvailablePropertyId) {
            actions.push(ActionDesc::new("toggle", "Toggle"));
        }
        if has(UIA_IsExpandCollapsePatternAvailablePropertyId) {
            actions.push(ActionDesc::new("expand", "Expand"));
            actions.push(ActionDesc::new("collapse", "Collapse"));
        }
        if has(UIA_IsSelectionItemPatternAvailablePropertyId) {
            actions.push(ActionDesc::new("select", "Select"));
        }
        if has(UIA_IsLegacyIAccessiblePatternAvailablePropertyId)
            && !actions.iter().any(|a| a.name == "press")
            && let Some(default) = cached_string(el, UIA_LegacyIAccessibleDefaultActionPropertyId)
        {
            actions.push(ActionDesc::new(default.to_lowercase(), "DoDefaultAction"));
        }

        let identifier = automation_id
            .filter(|s| !s.is_empty())
            .or(class.filter(|s| !s.is_empty()));
        let key = runtime_key(unsafe { el.GetCachedPropertyValue(UIA_RuntimeIdPropertyId) });
        let handle = self.handle_for(pid, el.clone());
        RawNode {
            handle,
            parent,
            key,
            role,
            native_role: control_type.to_string(),
            name: name.filter(|s| !s.is_empty()),
            description: None,
            value: value.filter(|s| !s.is_empty()),
            placeholder: None,
            identifier,
            bounds,
            actions,
            states: NodeStates {
                enabled,
                focused,
                selected,
                checked,
                expanded,
                editable,
                value_settable,
                hidden: offscreen || bounds.is_some_and(|b| b.is_empty()),
            },
        }
    }

    fn walk_cached(
        &mut self,
        pid: u32,
        el: &IUIAutomationElement,
        parent: Option<usize>,
        depth: usize,
        opts: &SnapshotOptions,
        out: &mut Vec<RawNode>,
    ) {
        if out.len() >= opts.max_nodes || depth > opts.max_depth {
            return;
        }
        let idx = out.len();
        let node = self.build_node_cached(pid, el, parent);
        out.push(node);
        if depth >= opts.max_depth {
            return;
        }
        let Ok(children) = (unsafe { el.GetCachedChildren() }) else {
            return;
        };
        let len = unsafe { children.Length() }.unwrap_or(0);
        for i in 0..len {
            if out.len() >= opts.max_nodes {
                break;
            }
            if let Ok(child) = unsafe { children.GetElement(i) } {
                self.walk_cached(pid, &child, Some(idx), depth + 1, opts, out);
            }
        }
    }

    // -- pattern getters ---------------------------------------------------

    fn get_pattern<T: Interface>(
        &self,
        el: &IUIAutomationElement,
        id: UIA_PATTERN_ID,
    ) -> Option<T> {
        unsafe { el.GetCurrentPattern(id).ok()?.cast::<T>().ok() }
    }
    fn value_pattern(&self, el: &IUIAutomationElement) -> Option<IUIAutomationValuePattern> {
        available(el, UIA_IsValuePatternAvailablePropertyId)
            .then(|| self.get_pattern(el, UIA_ValuePatternId))
            .flatten()
    }
    fn toggle_pattern(&self, el: &IUIAutomationElement) -> Option<IUIAutomationTogglePattern> {
        self.get_pattern(el, UIA_TogglePatternId)
    }
    fn expand_pattern(
        &self,
        el: &IUIAutomationElement,
    ) -> Option<IUIAutomationExpandCollapsePattern> {
        self.get_pattern(el, UIA_ExpandCollapsePatternId)
    }
    fn selection_item(
        &self,
        el: &IUIAutomationElement,
    ) -> Option<IUIAutomationSelectionItemPattern> {
        available(el, UIA_IsSelectionItemPatternAvailablePropertyId)
            .then(|| self.get_pattern(el, UIA_SelectionItemPatternId))
            .flatten()
    }
    fn legacy_pattern(
        &self,
        el: &IUIAutomationElement,
    ) -> Option<IUIAutomationLegacyIAccessiblePattern> {
        available(el, UIA_IsLegacyIAccessiblePatternAvailablePropertyId)
            .then(|| self.get_pattern(el, UIA_LegacyIAccessiblePatternId))
            .flatten()
    }
}

impl Backend for WindowsBackend {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn permissions(&mut self) -> Vec<PermissionStatus> {
        vec![PermissionStatus {
            name: "UI Automation".into(),
            granted: true,
            detail: "available".into(),
        }]
    }

    fn list_apps(&mut self) -> Result<Vec<AppInfo>> {
        let mut by_pid: HashMap<u32, AppInfo> = HashMap::new();
        let foreground_pid = foreground_pid();
        for hwnd in self.top_level_windows() {
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            if pid == 0 {
                continue;
            }
            let (exe, base) = process_image(pid);
            let title = Self::window_title(hwnd);
            let entry = by_pid.entry(pid).or_insert_with(|| AppInfo {
                name: base.clone().unwrap_or_else(|| title.clone()),
                id: base.clone().unwrap_or_else(|| format!("pid {pid}")),
                pid,
                exe: exe.clone(),
                frontmost: Some(pid) == foreground_pid,
                hidden: false,
            });
            if entry.name.is_empty() {
                entry.name = title;
            }
        }
        Ok(by_pid.into_values().collect())
    }

    fn launch_app(&mut self, query: &str) -> Result<Option<String>> {
        use crate::launch::{self, Pick};
        // The whole query is the program: never split into arguments, so a
        // launch can't become a command line (`cmd /c …`).
        let err = match crate::backend::spawn_detached(Command::new(query)) {
            Ok(()) => return Ok(None),
            Err(e) => e,
        };
        if err.kind() != std::io::ErrorKind::NotFound || query.contains(['\\', '/', ':']) {
            return Err(Error::ActionFailed(format!(
                "could not launch `{query}`: {err}"
            )));
        }
        // A program registered under App Paths (`chrome`, `winword`), which
        // the Run box finds but PATH doesn't.
        if let Some(exe) = app_path(query) {
            crate::backend::spawn_detached(Command::new(&exe)).map_err(|e| {
                Error::ActionFailed(format!("could not launch `{query}` ({exe}): {e}"))
            })?;
            return Ok(Some(exe));
        }
        // An app's name in the Start Menu ("Google Chrome"): open its
        // shortcut, as clicking it would.
        match launch::pick(query, &launch::start_menu_shortcuts(&start_menu_dirs())) {
            Pick::One(lnk) => shell_open(&lnk).map(|()| None),
            other => Err(Error::ActionFailed(launch::not_found(
                query,
                &other,
                "the Start Menu",
            ))),
        }
    }

    fn list_windows(&mut self, app: &AppInfo) -> Result<Vec<WindowInfo>> {
        let foreground = unsafe { GetForegroundWindow() };
        let mut out = Vec::new();
        let mut listed: HashSet<ElementHandle> = HashSet::new();
        for hwnd in self.top_level_windows() {
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            if pid != app.pid {
                continue;
            }
            let element = match unsafe { self.automation.ElementFromHandle(hwnd) } {
                Ok(e) => e,
                Err(_) => continue,
            };
            let bounds = unsafe { element.CurrentBoundingRectangle() }
                .ok()
                .map(rect_to_bounds);
            // The element's own HasKeyboardFocus is false whenever one of
            // its children has the focus: compare with the foreground HWND.
            let focused = !hwnd.0.is_null() && hwnd == foreground;
            // The same handle for the same window, across listings.
            let known = self.hwnds.iter().find_map(|(h, w)| {
                (*w == hwnd.0 as isize
                    && self
                        .window_handles
                        .get(h)
                        .is_some_and(|(p, _)| *p == app.pid))
                .then_some(*h)
            });
            let handle = known.unwrap_or_else(|| {
                let h = self.next_handle;
                self.next_handle += 1;
                h
            });
            self.window_handles.insert(handle, (app.pid, element));
            self.hwnds.insert(handle, hwnd.0 as isize);
            listed.insert(handle);
            out.push(WindowInfo {
                id: hwnd.0 as u64,
                title: Self::window_title(hwnd),
                bounds,
                focused,
                main: focused,
                minimized: wm::minimized(hwnd),
                handle,
            });
        }
        // Windows of this app that are gone drop their handles.
        let gone: Vec<ElementHandle> = self
            .window_handles
            .iter()
            .filter(|(h, (p, _))| *p == app.pid && !listed.contains(h))
            .map(|(h, _)| *h)
            .collect();
        for h in gone {
            self.window_handles.remove(&h);
            self.hwnds.remove(&h);
        }
        Ok(out)
    }

    fn snapshot(
        &mut self,
        app: &AppInfo,
        window: &WindowInfo,
        opts: &SnapshotOptions,
    ) -> Result<Vec<RawNode>> {
        let root = self.resolve(window.handle)?;
        // Element handles of the app's previous views go; its window
        // handles (kept apart) stay valid.
        self.handles.retain(|_, (p, _)| *p != app.pid);
        let mut out = Vec::new();
        if self.use_cache_request && !self.uncached_pids.contains(&app.pid) {
            // One cross-process call fetches the whole subtree's properties
            // (bounded by the UIA transaction timeout).
            let cached = self
                .cache_request()
                .and_then(|cr| unsafe { root.BuildUpdatedCache(&cr) });
            match cached {
                Ok(cached_root) => {
                    self.walk_cached(app.pid, &cached_root, None, 0, opts, &mut out);
                    return Ok(out);
                }
                Err(e) => {
                    // Too big or too slow for one request: from now on walk
                    // this app within the node/depth limits instead.
                    log::warn!("UIA cache request failed ({e}); walking uncached");
                    self.uncached_pids.insert(app.pid);
                }
            }
        }
        self.walk(app.pid, &root, None, 0, opts, &mut out);
        Ok(out)
    }

    fn configure(&mut self, cfg: &crate::config::Config) {
        self.use_cache_request = cfg.windows.use_cache_request;
        input::set_restore_pointer(cfg.restore_pointer);
    }

    fn capture(&mut self, _app: &AppInfo, window: &WindowInfo) -> Result<Capture> {
        let hwnd = self
            .hwnds
            .get(&window.handle)
            .copied()
            .map(|h| HWND(h as *mut _))
            .ok_or_else(|| Error::Platform("no window handle to capture".into()))?;
        capture::capture_window(hwnd)
    }

    fn capture_screen(&mut self, region: Option<Rect>) -> Result<Capture> {
        capture::capture_screen(region)
    }

    fn displays(&mut self) -> Result<Vec<Display>> {
        wm::displays()
    }

    fn ocr(&mut self, cap: &Capture, languages: &[String]) -> Result<Vec<OcrLine>> {
        ocr::recognize(cap, languages)
    }

    fn notifications(&mut self) -> Result<Vec<Notification>> {
        notify::recent()
    }

    fn window_op(&mut self, app: &AppInfo, window: &WindowInfo, op: &WindowOp) -> Result<()> {
        wm::apply(HWND(window.id as usize as *mut _), app.pid, op)
    }

    fn user_idle(&mut self) -> Option<std::time::Duration> {
        use windows::Win32::System::SystemInformation::GetTickCount;
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
        let mut info = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        // SAFETY: fills in the tick count of the last input event (only
        // its time; nothing about what it was).
        let ok = unsafe { GetLastInputInfo(&mut info) }.as_bool();
        // Tick counts wrap every ~49 days; the difference still works.
        let now = unsafe { GetTickCount() };
        ok.then(|| std::time::Duration::from_millis(u64::from(now.wrapping_sub(info.dwTime))))
    }

    fn clipboard_get(&mut self) -> Result<String> {
        clipboard::get()
    }

    fn clipboard_set(&mut self, text: &str) -> Result<()> {
        clipboard::set(text)
    }

    fn perform_action(&mut self, element: ElementHandle, native_action: &str) -> Result<()> {
        let el = self.resolve(element)?;
        let ok = unsafe {
            match native_action {
                "Invoke" => self
                    .get_pattern::<IUIAutomationInvokePattern>(&el, UIA_InvokePatternId)
                    .map(|p| p.Invoke().is_ok()),
                "Toggle" => self.toggle_pattern(&el).map(|p| p.Toggle().is_ok()),
                "Expand" => self.expand_pattern(&el).map(|p| p.Expand().is_ok()),
                "Collapse" => self.expand_pattern(&el).map(|p| p.Collapse().is_ok()),
                "Select" => self.selection_item(&el).map(|p| p.Select().is_ok()),
                "DoDefaultAction" => self
                    .legacy_pattern(&el)
                    .map(|p| p.DoDefaultAction().is_ok()),
                _ => None,
            }
        };
        match ok {
            Some(true) => Ok(()),
            Some(false) => Err(Error::ActionFailed(format!(
                "action `{native_action}` failed"
            ))),
            None => Err(Error::ActionFailed(format!(
                "element no longer supports action `{native_action}`"
            ))),
        }
    }

    fn set_value(&mut self, element: ElementHandle, value: &str) -> Result<()> {
        let el = self.resolve(element)?;
        // Toggle controls: flip to the requested boolean.
        if let Some(t) = self.toggle_pattern(&el) {
            let want = matches!(
                value.trim().to_lowercase().as_str(),
                "true" | "1" | "on" | "yes" | "checked"
            );
            let is = unsafe { t.CurrentToggleState() }
                .map(|s| s == ToggleState_On)
                .unwrap_or(false);
            if is != want {
                unsafe { t.Toggle() }.map_err(Error::action)?;
            }
            return Ok(());
        }
        if let Some(v) = self.value_pattern(&el) {
            let bstr = BSTR::from(value);
            unsafe { v.SetValue(&bstr) }.map_err(Error::action)?;
            return Ok(());
        }
        if let Some(leg) = self.legacy_pattern(&el) {
            let bstr = BSTR::from(value);
            unsafe { leg.SetValue(&bstr) }.map_err(Error::action)?;
            return Ok(());
        }
        Err(Error::ActionFailed(
            "this element does not accept a value directly".into(),
        ))
    }

    fn select_text(
        &mut self,
        element: ElementHandle,
        text: Option<&str>,
        occurrence: usize,
    ) -> Result<()> {
        let el = self.resolve(element)?;
        let text_pat: IUIAutomationTextPattern = self
            .get_pattern(&el, UIA_TextPatternId)
            .ok_or_else(|| Error::ActionFailed("element has no selectable text".into()))?;
        let range = unsafe { text_pat.DocumentRange() }.map_err(Error::action)?;
        match text {
            None => unsafe { range.Select() }.map_err(Error::action)?,
            Some(needle) => {
                let bstr = BSTR::from(needle);
                let not_found = || Error::ActionFailed(format!("`{needle}` not found"));
                let mut found =
                    unsafe { range.FindText(&bstr, false, false) }.map_err(|_| not_found())?;
                // FindText returns the first match in the range: move the
                // range's start past each hit to reach the nth.
                for _ in 1..occurrence.max(1) {
                    unsafe {
                        range.MoveEndpointByRange(
                            TextPatternRangeEndpoint_Start,
                            &found,
                            TextPatternRangeEndpoint_End,
                        )
                    }
                    .map_err(Error::action)?;
                    found =
                        unsafe { range.FindText(&bstr, false, false) }.map_err(|_| not_found())?;
                }
                unsafe { found.Select() }.map_err(Error::action)?;
            }
        }
        Ok(())
    }

    fn focus(&mut self, element: ElementHandle) -> Result<Native> {
        let el = self.resolve(element)?;
        match unsafe { el.SetFocus() } {
            Ok(_) => Ok(Native::Done("focused".into())),
            Err(_) => Ok(Native::Unsupported),
        }
    }

    fn scroll_element(
        &mut self,
        element: ElementHandle,
        direction: ScrollDirection,
        pages: f64,
    ) -> Result<Native> {
        let el = self.resolve(element)?;
        let Some(scroll) = self.get_pattern::<IUIAutomationScrollPattern>(&el, UIA_ScrollPatternId)
        else {
            return Ok(Native::Unsupported);
        };
        let none = ScrollAmount_NoAmount;
        let (h, v) = match direction {
            ScrollDirection::Down => (none, ScrollAmount_LargeIncrement),
            ScrollDirection::Up => (none, ScrollAmount_LargeDecrement),
            ScrollDirection::Right => (ScrollAmount_LargeIncrement, none),
            ScrollDirection::Left => (ScrollAmount_LargeDecrement, none),
        };
        let count = pages.round().max(1.0) as i32;
        for _ in 0..count {
            if unsafe { scroll.Scroll(h, v) }.is_err() {
                return Ok(Native::Unsupported);
            }
        }
        Ok(Native::Done("scrolled".into()))
    }

    fn click(
        &mut self,
        _target: &InputTarget,
        at: Point,
        button: MouseButton,
        count: u8,
    ) -> Result<()> {
        input::click(at, button, count)
    }

    fn drag(&mut self, _target: &InputTarget, from: Point, to: Point) -> Result<()> {
        input::drag(from, to)
    }

    fn move_pointer(&mut self, _target: &InputTarget, at: Point) -> Result<Option<Point>> {
        input::move_pointer(at)
    }

    fn draw(
        &mut self,
        _target: &InputTarget,
        strokes: &[Vec<Point>],
        button: MouseButton,
        pace: &mut dyn FnMut(f64) -> Result<()>,
    ) -> Result<()> {
        input::draw(strokes, button, pace)
    }

    fn scroll_wheel(&mut self, _target: &InputTarget, at: Point, dx: i32, dy: i32) -> Result<()> {
        input::scroll(at, dx, dy)
    }

    fn press_key(&mut self, _target: &InputTarget, combo: &KeyCombo) -> Result<()> {
        input::press(combo)
    }

    fn type_text(&mut self, _target: &InputTarget, text: &str) -> Result<()> {
        input::type_text(text)
    }
}

/// The Start Menu's program folders: the user's own first, then all users'.
fn start_menu_dirs() -> Vec<std::path::PathBuf> {
    ["APPDATA", "ProgramData"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|base| std::path::PathBuf::from(base).join(r"Microsoft\Windows\Start Menu\Programs"))
        .collect()
}

/// The program registered for `name` under `App Paths` (per user, then for
/// the machine), if that file exists.
fn app_path(name: &str) -> Option<String> {
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
    };
    use windows::core::HSTRING;
    let file = if name.to_lowercase().ends_with(".exe") {
        name.to_string()
    } else {
        format!("{name}.exe")
    };
    let key = HSTRING::from(format!(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{file}"
    ));
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let mut buf = vec![0u16; 1024];
        let mut bytes = (buf.len() * 2) as u32;
        // SAFETY: `buf` holds `bytes` bytes; the default value is read
        // (null value name), and REG_EXPAND_SZ comes back expanded.
        let status = unsafe {
            RegGetValueW(
                root,
                &key,
                windows::core::PCWSTR::null(),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut bytes),
            )
        };
        if status.is_err() {
            continue;
        }
        let len = (bytes as usize / 2).min(buf.len());
        let value = String::from_utf16_lossy(&buf[..len]);
        let path = value
            .trim_end_matches('\0')
            .trim()
            .trim_matches('"')
            .to_string();
        if !path.is_empty() && std::path::Path::new(&path).is_file() {
            return Some(path);
        }
    }
    None
}

/// Open a file the backend found itself (a Start Menu shortcut) the way
/// Explorer does. Never called with anything the agent typed.
fn shell_open(file: &std::path::Path) -> Result<()> {
    use windows::Win32::UI::Shell::{
        SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, PCWSTR};
    let verb = HSTRING::from("open");
    let wfile = HSTRING::from(file.as_os_str());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(wfile.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: the strings outlive the call; no process handle is asked for
    // (no SEE_MASK_NOCLOSEPROCESS), so nothing is left to close.
    unsafe { ShellExecuteExW(&mut info) }
        .map_err(|e| Error::ActionFailed(format!("could not open {}: {e}", file.display())))
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let vec = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    vec.push(hwnd);
    BOOL(1)
}

fn available(el: &IUIAutomationElement, prop: UIA_PROPERTY_ID) -> bool {
    unsafe {
        el.GetCurrentPropertyValue(prop)
            .ok()
            .and_then(|v| VariantToBoolean(&v).ok())
            .map(|b| b.as_bool())
            .unwrap_or(false)
    }
}

fn cached_bool(el: &IUIAutomationElement, prop: UIA_PROPERTY_ID) -> Option<bool> {
    unsafe {
        let v = el.GetCachedPropertyValue(prop).ok()?;
        VariantToBoolean(&v).ok().map(|b| b.as_bool())
    }
}

fn cached_i32(el: &IUIAutomationElement, prop: UIA_PROPERTY_ID) -> Option<i32> {
    unsafe {
        let v = el.GetCachedPropertyValue(prop).ok()?;
        VariantToInt32(&v).ok()
    }
}

fn cached_string(el: &IUIAutomationElement, prop: UIA_PROPERTY_ID) -> Option<String> {
    unsafe {
        let v = el.GetCachedPropertyValue(prop).ok()?;
        let p = VariantToStringAlloc(&v).ok()?;
        if p.is_null() {
            return None;
        }
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const std::ffi::c_void));
        s.filter(|s| !s.is_empty())
    }
}

fn bstr(v: windows::core::Result<BSTR>) -> Option<String> {
    v.ok().map(|b| b.to_string())
}

fn rect_to_bounds(r: RECT) -> Rect {
    Rect::new(
        f64::from(r.left),
        f64::from(r.top),
        f64::from(r.right - r.left),
        f64::from(r.bottom - r.top),
    )
}

fn foreground_pid() -> Option<u32> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (pid != 0).then_some(pid)
}

fn process_image(pid: u32) -> (Option<String>, Option<String>) {
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return (None, None);
        };
        let mut buf = vec![0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        if ok.is_err() {
            return (None, None);
        }
        let path = String::from_utf16_lossy(&buf[..size as usize]);
        let base = path
            .rsplit(['\\', '/'])
            .next()
            .map(|s| s.trim_end_matches(".exe").to_string());
        (Some(path), base)
    }
}
