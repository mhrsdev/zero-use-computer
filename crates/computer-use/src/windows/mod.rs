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

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT, RPC_E_TIMEOUT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::System::Variant::{
    VARIANT, VariantClear, VariantToBoolean, VariantToInt32, VariantToInt32Array,
    VariantToStringAlloc,
};
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GA_ROOT, GWL_EXSTYLE, GetAncestor, GetClassNameW, GetForegroundWindow,
    GetWindowLongW, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible, WINDOW_EX_STYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
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
    /// Apps whose subtree cache request timed out (huge or hung trees), and
    /// when: walked node by node, within the snapshot limits, until
    /// [`UNCACHED_RETRY`] has passed.
    uncached_pids: HashMap<u32, Instant>,
}

/// Upper bound for one UI Automation cross-process transaction (a subtree
/// cache request of a huge tree, or a hung app): past it the call fails
/// instead of blocking.
const UIA_TRANSACTION_TIMEOUT_MS: u32 = 5_000;
const UIA_CONNECTION_TIMEOUT_MS: u32 = 3_000;
/// The longest one snapshot may take: past it, the tree read so far is
/// returned (a node-by-node walk makes some twenty cross-process calls per
/// element).
const SNAPSHOT_BUDGET: Duration = Duration::from_secs(10);
/// How long an app whose cache request timed out is walked node by node
/// before the (much faster) cache request is tried again.
const UNCACHED_RETRY: Duration = Duration::from_secs(60);
/// Time for the lookups an element action makes before it is sent.
const ACTION_BUDGET: Duration = Duration::from_secs(10);
/// Most UI Automation scroll steps (pages) one call makes.
const MAX_SCROLL_PAGES: i32 = 50;
/// The window class of this server's overlay (see `overlay/windows.rs`).
const OVERLAY_CLASS: &str = "ComputerUseOverlay";

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

/// A VARIANT received from UI Automation. The `windows` crate's `VARIANT`
/// has no destructor: what it holds (a BSTR, which for an edit control's
/// value is the whole document, or a SAFEARRAY of runtime ids) is freed
/// here, when the guard is dropped.
struct Var(VARIANT);

impl Var {
    fn new(v: windows::core::Result<VARIANT>) -> Option<Self> {
        v.ok().map(Self)
    }

    fn bool(&self) -> Option<bool> {
        // SAFETY: reads a VARIANT we own.
        unsafe { VariantToBoolean(&self.0) }
            .ok()
            .map(|b| b.as_bool())
    }

    fn i32(&self) -> Option<i32> {
        // SAFETY: as above.
        unsafe { VariantToInt32(&self.0) }.ok()
    }

    fn string(&self) -> Option<String> {
        // SAFETY: as above; the string it allocates is freed here.
        unsafe {
            let p = VariantToStringAlloc(&self.0).ok()?;
            if p.is_null() {
                return None;
            }
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const std::ffi::c_void));
            s
        }
    }
}

impl Drop for Var {
    fn drop(&mut self) {
        // SAFETY: the VARIANT was handed to us by UI Automation and is
        // cleared exactly once, here.
        let _ = unsafe { VariantClear(&mut self.0) };
    }
}

/// UI Automation gave up waiting for the app: UIA_E_TIMEOUT (its
/// transaction or connection timeout), or an RPC or Win32 timeout.
fn is_timeout(e: &windows::core::Error) -> bool {
    /// HRESULT_FROM_WIN32(ERROR_TIMEOUT).
    const WIN32_TIMEOUT: u32 = 0x8007_05B4;
    let code = e.code();
    matches!(code.0 as u32, UIA_E_TIMEOUT | WIN32_TIMEOUT) || code == RPC_E_TIMEOUT
}

/// What one snapshot (or one action's lookups) may spend on cross-process
/// UI Automation calls: a deadline, and a stop at the first call the app
/// didn't answer in time, since every later call to a hung app would wait
/// out the timeout again.
struct Budget {
    deadline: Instant,
    timed_out: Cell<bool>,
}

impl Budget {
    fn new(limit: Duration) -> Self {
        Self {
            deadline: Instant::now() + limit,
            timed_out: Cell::new(false),
        }
    }

    /// Out of time, or the app stopped answering.
    fn over(&self) -> bool {
        self.timed_out.get() || Instant::now() >= self.deadline
    }

    fn timed_out(&self) -> bool {
        self.timed_out.get()
    }

    /// Make one call, unless the budget is spent; a timeout spends it.
    fn call<T>(&self, f: impl FnOnce() -> windows::core::Result<T>) -> Option<T> {
        if self.over() {
            return None;
        }
        match f() {
            Ok(v) => Some(v),
            Err(e) => {
                if is_timeout(&e) {
                    self.timed_out.set(true);
                }
                None
            }
        }
    }
}

/// UI Automation's runtime id, as an identity key: it stays the same for
/// as long as the element exists, so an element index can't move to a
/// look-alike element (the next row's "Delete") when one disappears.
fn runtime_key(v: Option<Var>) -> Option<String> {
    let v = v?;
    let mut ids = [0i32; 16];
    let mut n = 0u32;
    // SAFETY: reads an int array out of a VARIANT into a local buffer.
    unsafe { VariantToInt32Array(&v.0, &mut ids, &mut n) }.ok()?;
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
            // The multithreaded apartment, as Microsoft recommends for UI
            // Automation clients: this thread never pumps messages, which an
            // STA would need. (Nothing else on it needs an STA; opening a
            // Start Menu shortcut gets an STA thread of its own.) Ignore
            // RPC_E_CHANGED_MODE if COM is already initialized.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            // CUIAutomation8 (Windows 8+) is the object that implements
            // IUIAutomation2 and its timeouts; CUIAutomation doesn't.
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
                    .or_else(|e| {
                        log::info!("CUIAutomation8 unavailable ({e}); using CUIAutomation");
                        CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
                    })
                    .map_err(|e| {
                        Error::Platform(format!("CoCreateInstance(CUIAutomation): {e}"))
                    })?;
            // Bound every cross-process call, so a hung app can't block.
            match automation.cast::<IUIAutomation2>() {
                Ok(a2) => {
                    let t = a2.SetTransactionTimeout(UIA_TRANSACTION_TIMEOUT_MS);
                    let c = a2.SetConnectionTimeout(UIA_CONNECTION_TIMEOUT_MS);
                    if t.is_ok() && c.is_ok() {
                        log::info!(
                            "UI Automation timeouts set: {UIA_TRANSACTION_TIMEOUT_MS} ms per call, {UIA_CONNECTION_TIMEOUT_MS} ms to connect"
                        );
                    } else {
                        log::warn!(
                            "UI Automation timeouts not set (transaction: {t:?}, connection: {c:?}); a hung app may block calls"
                        );
                    }
                }
                Err(_) => log::warn!(
                    "UI Automation timeouts unavailable (no IUIAutomation2); a hung app may block calls"
                ),
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
                uncached_pids: HashMap::new(),
            })
        }
    }

    /// The name of the app an element belongs to, for messages.
    fn owner_name(&self, handle: ElementHandle) -> String {
        self.handles
            .get(&handle)
            .or_else(|| self.window_handles.get(&handle))
            .and_then(|(pid, _)| process_image(*pid).1)
            .unwrap_or_else(|| "the app".into())
    }

    /// The app didn't answer the lookups an action needs: nothing was sent.
    fn not_answering(&self, handle: ElementHandle) -> Error {
        Error::ActionFailed(format!(
            "{} didn't answer UI Automation in time, so nothing was sent; it may be busy or not responding. Look again (get_app_state) in a moment.",
            self.owner_name(handle)
        ))
    }

    /// The outcome of a sent action: a timeout means it was sent, but the
    /// app didn't answer (it may or may not have happened).
    fn sent(&self, handle: ElementHandle, r: windows::core::Result<()>) -> Result<()> {
        r.map_err(|e| {
            if is_timeout(&e) {
                Error::Unanswered(self.owner_name(handle))
            } else {
                Error::action(e)
            }
        })
    }

    /// Forget element and window handles of apps that are no longer
    /// running. `listed` are the pids known to be alive.
    fn prune_exited(&mut self, listed: &HashSet<u32>) {
        let known: HashSet<u32> = self
            .handles
            .values()
            .chain(self.window_handles.values())
            .map(|(pid, _)| *pid)
            .chain(self.uncached_pids.keys().copied())
            .filter(|pid| !listed.contains(pid))
            .collect();
        let exited: HashSet<u32> = known
            .into_iter()
            .filter(|pid| !crate::overlay::process_alive(*pid))
            .collect();
        self.uncached_pids
            .retain(|pid, since| !exited.contains(pid) && since.elapsed() < UNCACHED_RETRY);
        if exited.is_empty() {
            return;
        }
        self.handles.retain(|_, (pid, _)| !exited.contains(pid));
        let gone: Vec<ElementHandle> = self
            .window_handles
            .iter()
            .filter(|(_, (pid, _))| exited.contains(pid))
            .map(|(h, _)| *h)
            .collect();
        for h in gone {
            self.window_handles.remove(&h);
            self.hwnds.remove(&h);
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
            .filter(|&h| !ghost(h))
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

    /// Build a node with live (cross-process) calls, each within `b`.
    fn build_node(
        &mut self,
        b: &Budget,
        pid: u32,
        el: &IUIAutomationElement,
        parent: Option<usize>,
    ) -> RawNode {
        // SAFETY (every `unsafe` below): COM calls on a live element and
        // its patterns.
        let text = |r: Option<BSTR>| r.map(|s| s.to_string());
        let control_type = b
            .call(|| unsafe { el.CurrentControlType() })
            .map(|c| c.0)
            .unwrap_or(0);
        let password = b
            .call(|| unsafe { el.CurrentIsPassword() })
            .is_some_and(|x| x.as_bool());
        let role = if password {
            "secure text field".to_string()
        } else {
            roles::from_uia(control_type)
        };
        let name = text(b.call(|| unsafe { el.CurrentName() }));
        let automation_id = text(b.call(|| unsafe { el.CurrentAutomationId() }));
        let class = text(b.call(|| unsafe { el.CurrentClassName() }));

        let bounds = b
            .call(|| unsafe { el.CurrentBoundingRectangle() })
            .map(rect_to_bounds);
        let enabled = b
            .call(|| unsafe { el.CurrentIsEnabled() })
            .is_none_or(|x| x.as_bool());
        let focused = b
            .call(|| unsafe { el.CurrentHasKeyboardFocus() })
            .is_some_and(|x| x.as_bool());
        let offscreen = b
            .call(|| unsafe { el.CurrentIsOffscreen() })
            .is_some_and(|x| x.as_bool());

        // Value pattern → text value / editability.
        let value_pat = value_pattern(b, el);
        let (mut value, mut editable, mut value_settable) = (None, false, false);
        if let Some(v) = &value_pat {
            // Never read a password field's contents.
            value = (!password)
                .then(|| text(b.call(|| unsafe { v.CurrentValue() })))
                .flatten();
            let readonly = b
                .call(|| unsafe { v.CurrentIsReadOnly() })
                .is_none_or(|x| x.as_bool());
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
        let can_toggle = available(b, el, UIA_IsTogglePatternAvailablePropertyId);
        let mut checked = None;
        if can_toggle && let Some(t) = toggle_pattern(b, el) {
            checked = Some(
                b.call(|| unsafe { t.CurrentToggleState() })
                    .is_some_and(|s| s == ToggleState_On),
            );
            value = None;
        }

        // Expand/collapse → expanded.
        let can_expand = available(b, el, UIA_IsExpandCollapsePatternAvailablePropertyId);
        let mut expanded = None;
        if can_expand && let Some(ec) = expand_pattern(b, el) {
            expanded = b
                .call(|| unsafe { ec.CurrentExpandCollapseState() })
                .map(|s| s == ExpandCollapseState_Expanded);
        }

        let can_select = available(b, el, UIA_IsSelectionItemPatternAvailablePropertyId);
        let selected = can_select
            && pattern::<IUIAutomationSelectionItemPattern>(b, el, UIA_SelectionItemPatternId)
                .and_then(|s| b.call(|| unsafe { s.CurrentIsSelected() }))
                .is_some_and(|x| x.as_bool());

        let mut actions = Vec::new();
        if available(b, el, UIA_IsInvokePatternAvailablePropertyId) {
            actions.push(ActionDesc::new("press", "Invoke"));
        }
        if can_toggle {
            actions.push(ActionDesc::new("toggle", "Toggle"));
        }
        if can_expand {
            actions.push(ActionDesc::new("expand", "Expand"));
            actions.push(ActionDesc::new("collapse", "Collapse"));
        }
        if can_select {
            actions.push(ActionDesc::new("select", "Select"));
        }
        if !actions.iter().any(|a| a.name == "press")
            && let Some(leg) = legacy_pattern(b, el)
            && let Some(default) = text(b.call(|| unsafe { leg.CurrentDefaultAction() }))
            && !default.is_empty()
        {
            actions.push(ActionDesc::new(default.to_lowercase(), "DoDefaultAction"));
        }

        let identifier = automation_id
            .filter(|s| !s.is_empty())
            .or(class.filter(|s| !s.is_empty()));

        let key = runtime_key(
            b.call(|| unsafe { el.GetCurrentPropertyValue(UIA_RuntimeIdPropertyId) })
                .map(Var),
        );
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

    /// Walk node by node (cross-process calls) until the limits or the
    /// budget run out.
    #[allow(clippy::too_many_arguments)]
    fn walk(
        &mut self,
        b: &Budget,
        pid: u32,
        el: &IUIAutomationElement,
        parent: Option<usize>,
        depth: usize,
        opts: &SnapshotOptions,
        out: &mut Vec<RawNode>,
    ) {
        if out.len() >= opts.max_nodes || depth > opts.max_depth || b.over() {
            return;
        }
        let idx = out.len();
        let node = self.build_node(b, pid, el, parent);
        if b.over() && parent.is_some() {
            return; // cut short: leave it out rather than half-read
        }
        out.push(node);
        if depth >= opts.max_depth {
            return; // children would be cut anyway: don't enumerate them
        }
        // Bounded even if a provider reports a sibling cycle.
        let mut siblings = 0;
        // SAFETY: tree-walker calls on live elements.
        let mut child = b.call(|| unsafe { self.walker.GetFirstChildElement(el) });
        while let Some(c) = child {
            if out.len() >= opts.max_nodes || siblings >= opts.max_nodes || b.over() {
                break;
            }
            siblings += 1;
            self.walk(b, pid, &c, Some(idx), depth + 1, opts, out);
            // SAFETY: as above.
            child = b.call(|| unsafe { self.walker.GetNextSiblingElement(&c) });
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
        let key = runtime_key(Var::new(unsafe {
            el.GetCachedPropertyValue(UIA_RuntimeIdPropertyId)
        }));
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
}

// -- pattern getters (each call within a budget) ----------------------------

fn pattern<T: Interface>(b: &Budget, el: &IUIAutomationElement, id: UIA_PATTERN_ID) -> Option<T> {
    // SAFETY: a COM call on a live element.
    b.call(|| unsafe { el.GetCurrentPattern(id) })?
        .cast::<T>()
        .ok()
}
fn value_pattern(b: &Budget, el: &IUIAutomationElement) -> Option<IUIAutomationValuePattern> {
    available(b, el, UIA_IsValuePatternAvailablePropertyId)
        .then(|| pattern(b, el, UIA_ValuePatternId))
        .flatten()
}
fn toggle_pattern(b: &Budget, el: &IUIAutomationElement) -> Option<IUIAutomationTogglePattern> {
    pattern(b, el, UIA_TogglePatternId)
}
fn expand_pattern(
    b: &Budget,
    el: &IUIAutomationElement,
) -> Option<IUIAutomationExpandCollapsePattern> {
    pattern(b, el, UIA_ExpandCollapsePatternId)
}
fn selection_item(
    b: &Budget,
    el: &IUIAutomationElement,
) -> Option<IUIAutomationSelectionItemPattern> {
    available(b, el, UIA_IsSelectionItemPatternAvailablePropertyId)
        .then(|| pattern(b, el, UIA_SelectionItemPatternId))
        .flatten()
}
fn legacy_pattern(
    b: &Budget,
    el: &IUIAutomationElement,
) -> Option<IUIAutomationLegacyIAccessiblePattern> {
    available(b, el, UIA_IsLegacyIAccessiblePatternAvailablePropertyId)
        .then(|| pattern(b, el, UIA_LegacyIAccessiblePatternId))
        .flatten()
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
        // Handles of apps that have quit go (each holds a COM reference).
        self.prune_exited(&by_pid.keys().copied().collect());
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
        let budget = Budget::new(SNAPSHOT_BUDGET);
        let mut out = Vec::new();
        let try_cache = self
            .uncached_pids
            .get(&app.pid)
            .is_none_or(|since| since.elapsed() >= UNCACHED_RETRY);
        if self.use_cache_request && try_cache {
            // One cross-process call fetches the whole subtree's properties
            // (bounded by the UIA transaction timeout).
            let cached = self
                .cache_request()
                .and_then(|cr| unsafe { root.BuildUpdatedCache(&cr) });
            match cached {
                Ok(cached_root) => {
                    self.uncached_pids.remove(&app.pid);
                    self.walk_cached(app.pid, &cached_root, None, 0, opts, &mut out);
                    return Ok(out);
                }
                Err(e) if is_timeout(&e) => {
                    // Too big or too slow for one request: for a while,
                    // walk this app within the node/depth limits instead.
                    log::warn!(
                        "UIA cache request for {} timed out; walking it node by node for {}s",
                        app.name,
                        UNCACHED_RETRY.as_secs()
                    );
                    self.uncached_pids.insert(app.pid, Instant::now());
                }
                Err(e) => log::warn!("UIA cache request failed ({e}); walking uncached"),
            }
        }
        self.walk(&budget, app.pid, &root, None, 0, opts, &mut out);
        if budget.timed_out() {
            log::warn!(
                "{} stopped answering UI Automation; returning the {} elements read so far",
                app.name,
                out.len()
            );
        } else if budget.over() {
            log::warn!(
                "reading {}'s tree took over {}s; returning the {} elements read so far",
                app.name,
                SNAPSHOT_BUDGET.as_secs(),
                out.len()
            );
        }
        Ok(out)
    }

    fn configure(&mut self, cfg: &crate::config::Config) {
        self.use_cache_request = cfg.windows.use_cache_request;
        input::set_restore_pointer(cfg.restore_pointer);
    }

    fn capture(&mut self, app: &AppInfo, window: &WindowInfo) -> Result<Capture> {
        let hwnd = self
            .hwnds
            .get(&window.handle)
            .copied()
            .map(|h| HWND(h as *mut _))
            .ok_or_else(|| Error::Platform("no window handle to capture".into()))?;
        capture::capture_window(hwnd, &app.name)
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
        wm::apply(HWND(window.id as usize as *mut _), app, op)
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
        let b = Budget::new(ACTION_BUDGET);
        // SAFETY: COM calls on a live element and its patterns.
        let sent = unsafe {
            match native_action {
                "Invoke" => pattern::<IUIAutomationInvokePattern>(&b, &el, UIA_InvokePatternId)
                    .map(|p| p.Invoke()),
                "Toggle" => toggle_pattern(&b, &el).map(|p| p.Toggle()),
                "Expand" => expand_pattern(&b, &el).map(|p| p.Expand()),
                "Collapse" => expand_pattern(&b, &el).map(|p| p.Collapse()),
                "Select" => selection_item(&b, &el).map(|p| p.Select()),
                "DoDefaultAction" => legacy_pattern(&b, &el).map(|p| p.DoDefaultAction()),
                _ => None,
            }
        };
        match sent {
            Some(Ok(())) => Ok(()),
            // Sent, but the app didn't answer in time.
            Some(Err(e)) if is_timeout(&e) => Err(Error::Unanswered(self.owner_name(element))),
            Some(Err(e)) => Err(Error::ActionFailed(format!(
                "action `{native_action}` failed: {e}"
            ))),
            None if b.timed_out() => Err(self.not_answering(element)),
            None => Err(Error::ActionFailed(format!(
                "element no longer supports action `{native_action}`"
            ))),
        }
    }

    fn set_value(&mut self, element: ElementHandle, value: &str) -> Result<()> {
        let el = self.resolve(element)?;
        let b = Budget::new(ACTION_BUDGET);
        // Toggle controls: flip to the requested boolean.
        if let Some(t) = toggle_pattern(&b, &el) {
            let want = matches!(
                value.trim().to_lowercase().as_str(),
                "true" | "1" | "on" | "yes" | "checked"
            );
            // SAFETY: COM calls on a live pattern.
            let is = b
                .call(|| unsafe { t.CurrentToggleState() })
                .is_some_and(|s| s == ToggleState_On);
            if b.timed_out() {
                return Err(self.not_answering(element));
            }
            if is != want {
                // SAFETY: as above.
                self.sent(element, unsafe { t.Toggle() })?;
            }
            return Ok(());
        }
        if let Some(v) = value_pattern(&b, &el) {
            let bstr = BSTR::from(value);
            // SAFETY: as above.
            return self.sent(element, unsafe { v.SetValue(&bstr) });
        }
        if let Some(leg) = legacy_pattern(&b, &el) {
            let bstr = BSTR::from(value);
            // SAFETY: as above.
            return self.sent(element, unsafe { leg.SetValue(&bstr) });
        }
        if b.timed_out() {
            return Err(self.not_answering(element));
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
        let b = Budget::new(ACTION_BUDGET);
        let text_pat: IUIAutomationTextPattern =
            pattern(&b, &el, UIA_TextPatternId).ok_or_else(|| {
                if b.timed_out() {
                    self.not_answering(element)
                } else {
                    Error::ActionFailed("element has no selectable text".into())
                }
            })?;
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
        let b = Budget::new(ACTION_BUDGET);
        let Some(scroll) = pattern::<IUIAutomationScrollPattern>(&b, &el, UIA_ScrollPatternId)
        else {
            if b.timed_out() {
                return Err(self.not_answering(element));
            }
            return Ok(Native::Unsupported);
        };
        let none = ScrollAmount_NoAmount;
        let (h, v) = match direction {
            ScrollDirection::Down => (none, ScrollAmount_LargeIncrement),
            ScrollDirection::Up => (none, ScrollAmount_LargeDecrement),
            ScrollDirection::Right => (ScrollAmount_LargeIncrement, none),
            ScrollDirection::Left => (ScrollAmount_LargeDecrement, none),
        };
        // `as` saturates (and NaN gives 1): at most MAX_SCROLL_PAGES calls.
        let count = (pages.round().max(1.0) as i32).min(MAX_SCROLL_PAGES);
        for i in 0..count {
            // SAFETY: a COM call on a live pattern.
            if let Err(e) = unsafe { scroll.Scroll(h, v) } {
                if is_timeout(&e) {
                    return Err(Error::Unanswered(self.owner_name(element)));
                }
                // The first step failing leaves it to the mouse wheel; a
                // later one means it went as far as it goes.
                return Ok(if i == 0 {
                    Native::Unsupported
                } else {
                    Native::Done("scrolled".into())
                });
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

/// How long opening a Start Menu shortcut may take before giving up waiting.
const SHELL_OPEN_WAIT: Duration = Duration::from_secs(30);

/// Open a file the backend found itself (a Start Menu shortcut) the way
/// Explorer does. Never called with anything the agent typed.
///
/// The shell extensions this may load can need a single-threaded
/// apartment, which the backend's thread isn't: it runs on a short-lived
/// STA thread of its own.
fn shell_open(file: &std::path::Path) -> Result<()> {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoUninitialize};
    let path = file.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("shell-open".into())
        .spawn(move || {
            // SAFETY: this new thread's own COM initialization, undone
            // before it ends.
            let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            let r = shell_execute(&path);
            if init.is_ok() {
                // SAFETY: balances the successful CoInitializeEx above.
                unsafe { CoUninitialize() };
            }
            let _ = tx.send(r);
        })
        .map_err(|e| Error::Platform(format!("could not start a thread to open a file: {e}")))?;
    match rx.recv_timeout(SHELL_OPEN_WAIT) {
        Ok(r) => r,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(Error::ActionFailed(format!(
            "Windows didn't finish opening {} in time; it may still open (check list_apps)",
            file.display()
        ))),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(Error::Platform(format!(
            "opening {} failed unexpectedly",
            file.display()
        ))),
    }
}

fn shell_execute(file: &std::path::Path) -> Result<()> {
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

/// A window that is "visible" without being an app window the user sees:
/// cloaked by DWM (on another virtual desktop, a suspended store app, a
/// shell window kept for later; clicks there would land on the desktop in
/// view, so these are left out as Alt+Tab leaves them out), this server's
/// own overlay, or another untitled popup that is both a tool window and
/// never activated (overlays, toasts; titled ones, like a meeting's
/// floating controls, stay).
pub(super) fn ghost(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: DWM writes one DWORD into `cloaked`.
    let is_cloaked = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    }
    .is_ok()
        && cloaked != 0;
    if is_cloaked {
        return true;
    }
    // SAFETY: plain window queries.
    let ex = WINDOW_EX_STYLE(unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32);
    // SAFETY: plain window query.
    if ex.contains(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE)
        && unsafe { GetWindowTextLengthW(hwnd) } == 0
    {
        return true;
    }
    let mut class = [0u16; 64];
    // SAFETY: as above; writes at most `class.len()` units.
    let n = unsafe { GetClassNameW(hwnd, &mut class) };
    class
        .get(..n.max(0) as usize)
        .is_some_and(|c| String::from_utf16_lossy(c) == OVERLAY_CLASS)
}

/// Whether a pattern is available (live call within the budget).
fn available(b: &Budget, el: &IUIAutomationElement, prop: UIA_PROPERTY_ID) -> bool {
    // SAFETY: a COM call on a live element.
    b.call(|| unsafe { el.GetCurrentPropertyValue(prop) })
        .map(Var)
        .and_then(|v| v.bool())
        .unwrap_or(false)
}

fn cached(el: &IUIAutomationElement, prop: UIA_PROPERTY_ID) -> Option<Var> {
    // SAFETY: reads the element's cache (no cross-process call).
    Var::new(unsafe { el.GetCachedPropertyValue(prop) })
}

fn cached_bool(el: &IUIAutomationElement, prop: UIA_PROPERTY_ID) -> Option<bool> {
    cached(el, prop)?.bool()
}

fn cached_i32(el: &IUIAutomationElement, prop: UIA_PROPERTY_ID) -> Option<i32> {
    cached(el, prop)?.i32()
}

fn cached_string(el: &IUIAutomationElement, prop: UIA_PROPERTY_ID) -> Option<String> {
    cached(el, prop)?.string().filter(|s| !s.is_empty())
}

fn bstr(v: windows::core::Result<BSTR>) -> Option<String> {
    v.ok().map(|b| b.to_string())
}

fn rect_to_bounds(r: RECT) -> Rect {
    // In f64: a bogus rectangle from a provider can't overflow.
    Rect::new(
        f64::from(r.left),
        f64::from(r.top),
        f64::from(r.right) - f64::from(r.left),
        f64::from(r.bottom) - f64::from(r.top),
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
