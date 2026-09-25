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
mod wm;

use std::collections::HashMap;
use std::process::Command;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::System::Variant::{VariantToBoolean, VariantToInt32, VariantToStringAlloc};
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GA_ROOT, GetAncestor, GetWindowTextLengthW, GetWindowTextW,
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
    hwnds: HashMap<ElementHandle, isize>,
    next_handle: ElementHandle,
    /// Walk with a CacheRequest (one cross-process call per window).
    use_cache_request: bool,
    cache_request: Option<IUIAutomationCacheRequest>,
}

/// Properties prefetched for every element by the cache request.
const CACHED_PROPS: [UIA_PROPERTY_ID; 21] = [
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

impl WindowsBackend {
    pub fn new() -> Result<Self> {
        unsafe {
            // Ignore RPC_E_CHANGED_MODE if COM is already initialized.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).map_err(|e| {
                    Error::Platform(format!("CoCreateInstance(CUIAutomation): {e}"))
                })?;
            let walker = automation
                .ControlViewWalker()
                .map_err(|e| Error::Platform(format!("ControlViewWalker: {e}")))?;
            Ok(Self {
                automation,
                walker,
                handles: HashMap::new(),
                hwnds: HashMap::new(),
                next_handle: 1,
                use_cache_request: true,
                cache_request: None,
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

        let handle = self.handle_for(pid, el.clone());
        RawNode {
            handle,
            parent,
            key: None,
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
        let mut child = unsafe { self.walker.GetFirstChildElement(el) }.ok();
        while let Some(c) = child {
            if out.len() >= opts.max_nodes {
                break;
            }
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
        let handle = self.handle_for(pid, el.clone());
        RawNode {
            handle,
            parent,
            key: None,
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

    fn launch_app(&mut self, query: &str) -> Result<()> {
        let mut parts = query.split_whitespace();
        let program = parts
            .next()
            .ok_or_else(|| Error::InvalidArgs("empty app".into()))?;
        let args: Vec<&str> = parts.collect();
        Command::new(program)
            .args(&args)
            .spawn()
            .map(|_| ())
            .map_err(|e| Error::ActionFailed(format!("could not launch `{program}`: {e}")))
    }

    fn list_windows(&mut self, app: &AppInfo) -> Result<Vec<WindowInfo>> {
        let mut out = Vec::new();
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
            let focused = unsafe { element.CurrentHasKeyboardFocus() }
                .map(|b| b.as_bool())
                .unwrap_or(false);
            let handle = self.handle_for(app.pid, element);
            self.hwnds.insert(handle, hwnd.0 as isize);
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
        Ok(out)
    }

    fn snapshot(
        &mut self,
        app: &AppInfo,
        window: &WindowInfo,
        opts: &SnapshotOptions,
    ) -> Result<Vec<RawNode>> {
        let root = self.resolve(window.handle)?;
        self.handles.retain(|_, (p, _)| *p != app.pid);
        let mut out = Vec::new();
        if self.use_cache_request {
            // One cross-process call fetches the whole subtree's properties.
            let cached = self
                .cache_request()
                .and_then(|cr| unsafe { root.BuildUpdatedCache(&cr) });
            match cached {
                Ok(cached_root) => {
                    self.walk_cached(app.pid, &cached_root, None, 0, opts, &mut out);
                    return Ok(out);
                }
                Err(e) => log::warn!("UIA cache request failed ({e}); walking uncached"),
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
        _occurrence: usize,
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
                let found = unsafe { range.FindText(&bstr, false, false) }
                    .map_err(|_| Error::ActionFailed(format!("`{needle}` not found")))?;
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
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
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
