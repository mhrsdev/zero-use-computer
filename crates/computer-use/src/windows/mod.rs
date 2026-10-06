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

use windows::Win32::Foundation::{
    CloseHandle, ERROR_INSUFFICIENT_BUFFER, HWND, LPARAM, RECT, RPC_E_TIMEOUT,
};
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
    EnumChildWindows, EnumWindows, GA_ROOT, GWL_EXSTYLE, GetAncestor, GetClassNameW,
    GetForegroundWindow, GetWindowLongW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsWindow, IsWindowVisible, WINDOW_EX_STYLE, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW,
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
    /// The element is fetched when first needed: listing windows makes no
    /// UI Automation call, so a hung app can't slow it down.
    window_handles: HashMap<ElementHandle, (u32, Option<IUIAutomationElement>)>,
    hwnds: HashMap<ElementHandle, isize>,
    next_handle: ElementHandle,
    /// Walk with a CacheRequest (one cross-process call per window).
    use_cache_request: bool,
    cache_request: Option<IUIAutomationCacheRequest>,
    /// Apps whose subtree cache request timed out (huge or hung trees), and
    /// when: walked node by node, within the snapshot limits, until
    /// [`UNCACHED_RETRY`] has passed.
    uncached_pids: HashMap<u32, Instant>,
    /// Whether each app runs elevated (`None`: its token can't be read).
    elevated: HashMap<u32, Option<bool>>,
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
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, PROCESS_PER_MONITOR_DPI_AWARE,
        SetProcessDpiAwareness, SetProcessDpiAwarenessContext,
    };
    // SAFETY: process-wide settings, made before any window or metric is read.
    unsafe {
        // Already set (a manifest, an earlier call) or a Windows 10 before
        // 1703, which has no per-monitor v2: per-monitor awareness (shcore,
        // Windows 8.1+), else at least system-DPI awareness. (Both refuse
        // harmlessly once awareness is set.)
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_err()
            && SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE).is_err()
        {
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
            // Lossy: a lone surrogate in an app's text isn't worth losing
            // the whole value over.
            let s = Some(String::from_utf16_lossy(p.as_wide()));
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

/// The element no longer exists (UIA_E_ELEMENTNOTAVAILABLE): its window
/// or control was closed.
fn is_gone(e: &windows::core::Error) -> bool {
    e.code().0 as u32 == UIA_E_ELEMENTNOTAVAILABLE
}

/// What one snapshot (or one action's lookups) may spend on cross-process
/// UI Automation calls: a deadline, and a stop at the first call the app
/// didn't answer in time, since every later call to a hung app would wait
/// out the timeout again.
struct Budget {
    deadline: Instant,
    timed_out: Cell<bool>,
    /// A call found its element gone.
    gone: Cell<bool>,
}

impl Budget {
    fn new(limit: Duration) -> Self {
        Self {
            deadline: Instant::now() + limit,
            timed_out: Cell::new(false),
            gone: Cell::new(false),
        }
    }

    fn gone(&self) -> bool {
        self.gone.get()
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
                } else if is_gone(&e) {
                    self.gone.set(true);
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
                elevated: HashMap::new(),
            })
        }
    }

    /// The name of the app an element belongs to, for messages.
    fn owner_name(&self, handle: ElementHandle) -> String {
        self.handles
            .get(&handle)
            .map(|(pid, _)| *pid)
            .or_else(|| self.window_handles.get(&handle).map(|(pid, _)| *pid))
            .and_then(|pid| process_image(pid).1)
            .unwrap_or_else(|| "the app".into())
    }

    /// Refuse to work on an app Windows shields from this process: one
    /// that runs as administrator while this server doesn't. Windows drops
    /// input sent to it without a word, and UI Automation sees only its
    /// frame. An app whose token can't be read is let through.
    fn check_elevation(&mut self, pid: u32, name: Option<&str>) -> Result<()> {
        let ours = self_elevated();
        if ours {
            return Ok(());
        }
        let theirs = *self
            .elevated
            .entry(pid)
            .or_insert_with(|| process_elevated(pid));
        if !shielded(ours, theirs) {
            return Ok(());
        }
        let name = name
            .map(str::to_string)
            .or_else(|| process_image(pid).1)
            .unwrap_or_else(|| format!("the app (pid {pid})"));
        Err(Error::ActionFailed(format!(
            "{name} runs as administrator and this server doesn't, so Windows blocks input to it and hides its controls; run the server as administrator to control it"
        )))
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
            .map(|(pid, _)| *pid)
            .chain(self.window_handles.values().map(|(pid, _)| *pid))
            .chain(self.uncached_pids.keys().copied())
            .chain(self.elevated.keys().copied())
            .filter(|pid| !listed.contains(pid))
            .collect();
        let exited: HashSet<u32> = known
            .into_iter()
            .filter(|pid| !crate::overlay::process_alive(*pid))
            .collect();
        self.uncached_pids
            .retain(|pid, since| !exited.contains(pid) && since.elapsed() < UNCACHED_RETRY);
        self.elevated.retain(|pid, _| !exited.contains(pid));
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

    /// The UI Automation element of a listed window, fetched (and kept
    /// until the next listing) when first needed. `None` for a handle that
    /// isn't a listed window's.
    fn window_element(
        &mut self,
        handle: ElementHandle,
    ) -> Option<windows::core::Result<IUIAutomationElement>> {
        let (_, el) = self.window_handles.get(&handle)?;
        if let Some(el) = el {
            return Some(Ok(el.clone()));
        }
        let hwnd = HWND(*self.hwnds.get(&handle)? as *mut _);
        // SAFETY: a COM call with a window handle (bounded by the UIA
        // connection timeout).
        let r = unsafe { self.automation.ElementFromHandle(hwnd) };
        if let (Ok(el), Some(entry)) = (&r, self.window_handles.get_mut(&handle)) {
            entry.1 = Some(el.clone());
        }
        Some(r)
    }

    /// The window `handle` (a listed window's) was closed.
    fn window_gone(&self, handle: ElementHandle) -> Error {
        Error::ActionFailed(format!(
            "{}'s window is gone (it was closed); call get_app_state again to see its windows now",
            self.owner_name(handle)
        ))
    }

    fn resolve(&mut self, handle: ElementHandle) -> Result<IUIAutomationElement> {
        if let Some((_, el)) = self.handles.get(&handle) {
            return Ok(el.clone());
        }
        match self.window_element(handle) {
            Some(Ok(el)) => Ok(el),
            Some(Err(e)) if is_timeout(&e) => Err(self.not_answering(handle)),
            Some(Err(e)) if is_gone(&e) || !self.window_exists(handle) => {
                Err(self.window_gone(handle))
            }
            Some(Err(e)) => Err(Error::ActionFailed(format!(
                "UI Automation can't reach this window ({e}); use coordinates from the screenshot instead"
            ))),
            None => Err(Error::Internal(format!("stale element handle {handle}"))),
        }
    }

    /// The listed window `handle` still exists.
    fn window_exists(&self, handle: ElementHandle) -> bool {
        self.hwnds
            .get(&handle)
            // SAFETY: a plain query; any value is safe to pass.
            .is_some_and(|h| unsafe { IsWindow(Some(HWND(*h as *mut _))) }.as_bool())
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
                .filter(|s| *s != ExpandCollapseState_LeafNode)
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
            actions.push(ActionDesc::new(
                default_action_name(&default, can_select),
                "DoDefaultAction",
            ));
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
            actions.push(ActionDesc::new(
                default_action_name(&default, has(UIA_IsSelectionItemPatternAvailablePropertyId)),
                "DoDefaultAction",
            ));
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

/// The name a LegacyIAccessible default action is offered under. Windows
/// names it in its own language ("Drücken", "Appuyer", "Click"); the
/// engine clicks an element through its "press" action, so that is its
/// name, whatever the language. Except on an element that can be selected
/// (a list or tree item), where the default action (often a double-click,
/// opening it) isn't what one click does: it keeps its own name there.
fn default_action_name(native: &str, selectable: bool) -> String {
    let name = native.trim().to_lowercase();
    let double_click = name.replace([' ', '-'], "") == "doubleclick";
    if selectable || double_click {
        name
    } else {
        "press".into()
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
            let pid = window_pid(hwnd);
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
        // A console program (cmd, powershell, python) gets a console of its
        // own, as from the Run box: started like an app, its input would be
        // nothing and it would quit at once, unseen.
        if let Some(exe) = find_program(query)
            && console_program(&exe)
        {
            shell_open_console(&exe)?;
            return Ok(Some(exe.display().to_string()));
        }
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
            if console_program(std::path::Path::new(&exe)) {
                shell_open_console(std::path::Path::new(&exe))?;
                return Ok(Some(exe));
            }
            crate::backend::spawn_detached(Command::new(&exe)).map_err(|e| {
                Error::ActionFailed(format!("could not launch `{query}` ({exe}): {e}"))
            })?;
            return Ok(Some(exe));
        }
        // An app's name in the Start Menu ("Google Chrome"): open its
        // shortcut, as clicking it would.
        match launch::pick(query, &start_menu_apps()) {
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
            if window_pid(hwnd) != app.pid {
                continue;
            }
            // From Win32, not UI Automation: nothing here waits on the app,
            // and a window whose element can't be had is still listed (its
            // element is fetched when a snapshot needs it). The frame the
            // user sees, without the invisible resize borders.
            let bounds = capture::visible_frame(hwnd)
                .or_else(|| {
                    let mut r = RECT::default();
                    // SAFETY: a read-only query into a local.
                    unsafe { GetWindowRect(hwnd, &mut r) }.ok().map(|()| r)
                })
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
            self.window_handles.insert(handle, (app.pid, None));
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
        self.check_elevation(app.pid, Some(&app.name))?;
        // Element handles of the app's previous views go; its window
        // handles (kept apart) stay valid.
        self.handles.retain(|_, (p, _)| *p != app.pid);
        let root = match self.window_element(window.handle) {
            Some(Ok(root)) => root,
            Some(Err(e)) if is_gone(&e) || !self.window_exists(window.handle) => {
                return Err(self.window_gone(window.handle));
            }
            // No tree, but the window is there: its screenshot (and the
            // text read off it) is all there is to go on.
            Some(Err(e)) => {
                log::warn!(
                    "UI Automation has no element for {}'s window ({e}); returning no tree",
                    app.name
                );
                return Ok(Vec::new());
            }
            None => {
                return Err(Error::Internal(format!(
                    "stale element handle {}",
                    window.handle
                )));
            }
        };
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
                Err(e) if is_gone(&e) => return Err(self.window_gone(window.handle)),
                Err(e) => log::warn!("UIA cache request failed ({e}); walking uncached"),
            }
        }
        self.walk(&budget, app.pid, &root, None, 0, opts, &mut out);
        // Nothing but a placeholder for a window that no longer exists.
        if budget.gone() && out.len() <= 1 {
            return Err(self.window_gone(window.handle));
        }
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
        // A lookup the app didn't answer: nothing was selected.
        let lookup = |e: windows::core::Error| {
            if is_timeout(&e) {
                self.not_answering(element)
            } else {
                Error::action(e)
            }
        };
        // SAFETY (below): COM calls on a live pattern and its ranges.
        let range = unsafe { text_pat.DocumentRange() }.map_err(lookup)?;
        match text {
            None => self.sent(element, unsafe { range.Select() })?,
            Some(needle) => {
                let bstr = BSTR::from(needle);
                // FindText fails (no range) when there is no match.
                let find = || {
                    unsafe { range.FindText(&bstr, false, false) }.map_err(|e| {
                        if is_timeout(&e) {
                            self.not_answering(element)
                        } else {
                            Error::ActionFailed(format!("`{needle}` not found"))
                        }
                    })
                };
                let mut found = find()?;
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
                    .map_err(lookup)?;
                    found = find()?;
                }
                self.sent(element, unsafe { found.Select() })?;
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
        target: &InputTarget,
        at: Point,
        button: MouseButton,
        count: u8,
    ) -> Result<()> {
        self.check_elevation(target.pid, None)?;
        input::click(at, button, count)
    }

    fn drag(&mut self, target: &InputTarget, from: Point, to: Point) -> Result<()> {
        self.check_elevation(target.pid, None)?;
        input::drag(from, to)
    }

    fn move_pointer(&mut self, target: &InputTarget, at: Point) -> Result<Option<Point>> {
        self.check_elevation(target.pid, None)?;
        input::move_pointer(at)
    }

    fn draw(
        &mut self,
        target: &InputTarget,
        strokes: &[Vec<Point>],
        button: MouseButton,
        pace: &mut dyn FnMut(f64) -> Result<()>,
    ) -> Result<()> {
        self.check_elevation(target.pid, None)?;
        input::draw(strokes, button, pace)
    }

    fn scroll_wheel(&mut self, target: &InputTarget, at: Point, dx: i32, dy: i32) -> Result<()> {
        self.check_elevation(target.pid, None)?;
        input::scroll(at, dx, dy)
    }

    fn press_key(&mut self, target: &InputTarget, combo: &KeyCombo) -> Result<()> {
        self.check_elevation(target.pid, None)?;
        input::press(combo)
    }

    fn type_text(&mut self, target: &InputTarget, text: &str) -> Result<()> {
        self.check_elevation(target.pid, None)?;
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

/// How long opening a Start Menu shortcut, a console program or a web
/// address may take before giving up waiting.
const SHELL_OPEN_WAIT: Duration = Duration::from_secs(30);

/// Open a file the backend found itself (a Start Menu shortcut) the way
/// Explorer does. Never called with anything the agent typed.
fn shell_open(file: &std::path::Path) -> Result<()> {
    let what = file.display().to_string();
    shell_run(file.as_os_str().to_owned(), &what).map_err(|e| match e {
        ShellError::Failed(e) => Error::ActionFailed(format!("could not open {what}: {e}")),
        ShellError::Other(e) => e,
    })
}

/// Start a console program (found by [`find_program`], never a command
/// line) with a console window of its own, as the Run box does. It shares
/// none of this process's handles: its input and output are its console.
fn shell_open_console(exe: &std::path::Path) -> Result<()> {
    let what = exe.display().to_string();
    shell_run(exe.as_os_str().to_owned(), &what).map_err(|e| match e {
        ShellError::Failed(e) => Error::ActionFailed(format!("could not start {what}: {e}")),
        ShellError::Other(e) => e,
    })
}

/// Open a web address (already checked to be http or https, one line) in
/// the user's default browser.
pub(crate) fn open_url(url: &str) -> Result<()> {
    shell_run(url.into(), url).map_err(|e| match e {
        ShellError::Failed(e) => Error::Platform(format!(
            "couldn't open the browser ({e}); open {url} yourself"
        )),
        ShellError::Other(e) => e,
    })
}

enum ShellError {
    /// ShellExecuteEx said no.
    Failed(windows::core::Error),
    /// It didn't finish in time, or couldn't be asked.
    Other(Error),
}

/// `ShellExecuteExW` "open" of `target`, waited for at most
/// [`SHELL_OPEN_WAIT`]: a hung shell extension, DDE conversation or
/// browser can't block the server. The shell extensions it may load can
/// need a single-threaded apartment, which the backend's thread isn't: it
/// runs on a short-lived STA thread of its own (left to finish on its own
/// after a timeout).
fn shell_run(target: std::ffi::OsString, what: &str) -> std::result::Result<(), ShellError> {
    use windows::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoUninitialize,
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("shell-open".into())
        .spawn(move || {
            // SAFETY: this new thread's own COM initialization, undone
            // before it ends.
            let init =
                unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
            let r = shell_execute(&target);
            if init.is_ok() {
                // SAFETY: balances the successful CoInitializeEx above.
                unsafe { CoUninitialize() };
            }
            let _ = tx.send(r);
        })
        .map_err(|e| {
            ShellError::Other(Error::Platform(format!(
                "could not start a thread to open {what}: {e}"
            )))
        })?;
    match rx.recv_timeout(SHELL_OPEN_WAIT) {
        Ok(r) => r.map_err(ShellError::Failed),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            Err(ShellError::Other(Error::ActionFailed(format!(
                "Windows didn't finish opening {what} within {}s (a shell extension or the program it starts may be hanging); it may still open, so check (list_apps) before trying again",
                SHELL_OPEN_WAIT.as_secs()
            ))))
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(ShellError::Other(
            Error::Platform(format!("opening {what} failed unexpectedly")),
        )),
    }
}

fn shell_execute(target: &std::ffi::OsStr) -> windows::core::Result<()> {
    use windows::Win32::UI::Shell::{
        SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, PCWSTR};
    let verb = HSTRING::from("open");
    let wfile = HSTRING::from(target);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        // No SEE_MASK_NO_CONSOLE: that makes a console program share this
        // process's console. Without it, one gets a console of its own (as
        // with CREATE_NEW_CONSOLE).
        fMask: SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(wfile.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: the strings outlive the call; no process handle is asked for
    // (no SEE_MASK_NOCLOSEPROCESS), so nothing is left to close.
    unsafe { ShellExecuteExW(&mut info) }
}

/// The program `Command::new(query)` would start, looked for the way it
/// looks: a path as given, else this program's folder, the system
/// folders, then `PATH`; `.exe` is added to a name without an extension.
fn find_program(query: &str) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    let name = if Path::new(query).extension().is_none() {
        format!("{query}.exe")
    } else {
        query.to_string()
    };
    if query.contains(['\\', '/', ':']) {
        let p = PathBuf::from(name);
        return p.is_file().then_some(p);
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(own) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        dirs.push(own);
    }
    if let Some(root) = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("windir")) {
        let root = PathBuf::from(root);
        dirs.push(root.join("System32"));
        dirs.push(root);
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.into_iter()
        .map(|d| d.join(&name))
        .find(|p| p.is_file())
}

/// The program is a console program (cmd, powershell, python): its image
/// says so. False when it can't be read (an app execution alias).
fn console_program(exe: &std::path::Path) -> bool {
    use std::io::Read;
    let mut head = Vec::with_capacity(4096);
    std::fs::File::open(exe)
        .and_then(|f| f.take(4096).read_to_end(&mut head))
        .is_ok()
        && pe_subsystem(&head) == Some(IMAGE_SUBSYSTEM_WINDOWS_CUI)
}

/// A PE image's subsystem for a character-mode (console) program.
const IMAGE_SUBSYSTEM_WINDOWS_CUI: u16 = 3;

/// The subsystem field of a PE image (2 = GUI, 3 = console), read from the
/// start of the file; `None` if it isn't a PE image.
fn pe_subsystem(head: &[u8]) -> Option<u16> {
    let u16_at = |at: usize| -> Option<u16> {
        Some(u16::from_le_bytes(
            head.get(at..at.checked_add(2)?)?.try_into().ok()?,
        ))
    };
    if head.get(..2)? != b"MZ" {
        return None;
    }
    let pe = u32::from_le_bytes(head.get(0x3c..0x40)?.try_into().ok()?) as usize;
    if head.get(pe..pe.checked_add(4)?)? != b"PE\0\0" {
        return None;
    }
    // The optional header follows the 20-byte file header; its magic says
    // PE32 or PE32+, and Subsystem is at offset 68 in both.
    let opt = pe.checked_add(24)?;
    if !matches!(u16_at(opt)?, 0x10b | 0x20b) {
        return None;
    }
    u16_at(opt.checked_add(68)?)
}

/// Start Menu shortcuts by the names people see: the file's, and, on a
/// Windows in another language, the name its folder's `desktop.ini` gives
/// it (`[LocalizedFileNames]`), which the Start Menu shows instead.
/// Uninstallers are left out, in a few languages besides English.
fn start_menu_apps() -> Vec<(String, String, std::path::PathBuf)> {
    let mut by_dir: HashMap<std::path::PathBuf, HashMap<String, String>> = HashMap::new();
    let mut out = Vec::new();
    for (name, key, path) in crate::launch::start_menu_shortcuts(&start_menu_dirs()) {
        if uninstaller(&name) {
            continue;
        }
        let shown = path.parent().zip(path.file_name()).and_then(|(dir, file)| {
            let names = by_dir
                .entry(dir.to_path_buf())
                .or_insert_with(|| read_localized_names(dir));
            names
                .get(&file.to_string_lossy().to_lowercase())
                .and_then(|v| display_name(v))
        });
        out.push((name.clone(), key.clone(), path.clone()));
        if let Some(shown) = shown
            && !uninstaller(&shown)
            && crate::text::fold(&shown) != crate::text::fold(&name)
        {
            // Its own key: the same app's file name doesn't hide it, and
            // the same shortcut in two folders is still one app.
            out.push((shown, format!("{key}\u{0}shown"), path));
        }
    }
    out
}

/// A folder's `desktop.ini` `[LocalizedFileNames]`, by lowercase file name.
fn read_localized_names(dir: &std::path::Path) -> HashMap<String, String> {
    let Ok(bytes) = std::fs::read(dir.join("desktop.ini")) else {
        return HashMap::new();
    };
    localized_file_names(&decode_ini(&bytes))
}

/// An INI file's text: UTF-16 with its byte-order mark (as Windows writes
/// `desktop.ini`), else UTF-8.
fn decode_ini(bytes: &[u8]) -> String {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => {
            let units: Vec<u16> = rest
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&c| u16::from_le_bytes(c))
                .collect();
            String::from_utf16_lossy(&units)
        }
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// The `[LocalizedFileNames]` section of an INI text, by lowercase file
/// name.
fn localized_file_names(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line.eq_ignore_ascii_case("[LocalizedFileNames]");
            continue;
        }
        if !inside || line.starts_with(';') {
            continue;
        }
        if let Some((file, value)) = line.split_once('=') {
            let (file, value) = (file.trim(), value.trim());
            if !file.is_empty() && !value.is_empty() {
                out.insert(file.to_lowercase(), value.to_string());
            }
        }
    }
    out
}

/// A `[LocalizedFileNames]` value as text: a resource reference
/// (`@%SystemRoot%\system32\shell32.dll,-22067`) is loaded from its file.
fn display_name(value: &str) -> Option<String> {
    if !value.starts_with('@') {
        return Some(value.to_string());
    }
    let source = windows::core::HSTRING::from(expand_env(value, |k| std::env::var(k).ok()));
    let mut buf = [0u16; 512];
    // SAFETY: the source string outlives the call; the result is written
    // into `buf`, at most its length, null-terminated.
    unsafe { windows::Win32::UI::Shell::SHLoadIndirectString(&source, &mut buf, None) }.ok()?;
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let s = String::from_utf16_lossy(&buf[..len]).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// `%NAME%` replaced by `lookup(NAME)`; unknown names are left as they are.
fn expand_env(s: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => match lookup(&after[..end]) {
                Some(v) => {
                    out.push_str(&v);
                    rest = &after[end + 1..];
                }
                None => {
                    out.push('%');
                    rest = after;
                }
            },
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A shortcut to an uninstaller, by its name: "Uninstall Zoom", "Zoom
/// deinstallieren", "Désinstaller Zoom" (a word that starts with one of
/// these, in a few languages).
fn uninstaller(name: &str) -> bool {
    const WORDS: [&str; 10] = [
        "uninstall",
        "deinstall",
        "désinstall",
        "desinstal",
        "disinstall",
        "odinstal",
        "avinstall",
        "afinstall",
        "деинсталл",
        "アンインストール",
    ];
    let lower = name.to_lowercase();
    WORDS.iter().any(|w| {
        lower.match_indices(w).any(|(at, _)| {
            lower[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric())
        })
    }) || lower.contains("卸载")
        || lower.contains("解除安裝")
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
    class_name(hwnd) == OVERLAY_CLASS
}

fn class_name(hwnd: HWND) -> String {
    let mut class = [0u16; 64];
    // SAFETY: a plain window query; writes at most `class.len()` units.
    let n = unsafe { GetClassNameW(hwnd, &mut class) };
    String::from_utf16_lossy(class.get(..n.max(0) as usize).unwrap_or_default())
}

/// The class of the frame ApplicationFrameHost draws around a store (UWP)
/// app, and of the app's own window inside it.
const UWP_FRAME_CLASS: &str = "ApplicationFrameWindow";
const UWP_CORE_CLASS: &str = "Windows.UI.Core.CoreWindow";

/// The process that created a window.
fn creator_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: a plain window query into a local.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

/// A store app's own window inside the frame ApplicationFrameHost draws
/// around it: the frame belongs to ApplicationFrameHost, the CoreWindow in
/// it to the app. `None` for any other window, and for a frame whose app
/// has left it (suspended or minimized, its CoreWindow is moved out).
pub(super) fn uwp_core_window(hwnd: HWND) -> Option<HWND> {
    if hwnd.0.is_null() || class_name(hwnd) != UWP_FRAME_CLASS {
        return None;
    }
    let mut children: Vec<HWND> = Vec::new();
    // SAFETY: the callback only pushes into `children`, which outlives
    // the call.
    unsafe {
        let _ = EnumChildWindows(
            Some(hwnd),
            Some(enum_proc),
            LPARAM(&mut children as *mut Vec<HWND> as isize),
        );
    }
    let at = core_window_index(
        creator_pid(hwnd),
        children.iter().map(|&c| (class_name(c), creator_pid(c))),
    )?;
    children.get(at).copied()
}

/// Which of a frame's child windows, as (class, pid), is the store app's:
/// a CoreWindow of a process other than the frame's host.
fn core_window_index(
    host: u32,
    children: impl IntoIterator<Item = (String, u32)>,
) -> Option<usize> {
    children
        .into_iter()
        .position(|(class, pid)| class == UWP_CORE_CLASS && pid != 0 && pid != host)
}

/// The process a top-level window belongs to, as the user sees it: for a
/// store app's frame, the app's, not ApplicationFrameHost's.
/// A minimized or suspended store app's CoreWindow leaves its frame, which
/// then reads as ApplicationFrameHost's: the app it was last seen holding
/// is remembered, while that app runs.
pub(super) fn window_pid(hwnd: HWND) -> u32 {
    static FRAME_APPS: std::sync::Mutex<Option<HashMap<isize, u32>>> = std::sync::Mutex::new(None);
    let key = hwnd.0 as isize;
    if let Some(core) = uwp_core_window(hwnd) {
        let pid = creator_pid(core);
        if let Ok(mut map) = FRAME_APPS.lock() {
            let map = map.get_or_insert_with(HashMap::new);
            if map.len() >= 256 {
                // SAFETY: IsWindow only checks a handle.
                map.retain(|h, _| unsafe { IsWindow(Some(HWND(*h as *mut _))) }.as_bool());
            }
            map.insert(key, pid);
        }
        return pid;
    }
    if !hwnd.0.is_null() && class_name(hwnd) == UWP_FRAME_CLASS {
        let remembered = FRAME_APPS.lock().ok().and_then(|mut map| {
            let map = map.as_mut()?;
            let pid = *map.get(&key)?;
            if process_running(pid) {
                Some(pid)
            } else {
                map.remove(&key);
                None
            }
        });
        if let Some(pid) = remembered {
            return pid;
        }
    }
    creator_pid(hwnd)
}

/// Whether process `pid` is still running.
fn process_running(pid: u32) -> bool {
    use windows::Win32::System::Threading::GetExitCodeProcess;
    const STILL_ACTIVE: u32 = 259;
    // SAFETY: a limited query handle, closed before returning.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code).is_ok();
        let _ = CloseHandle(handle);
        ok && code == STILL_ACTIVE
    }
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
    let pid = window_pid(hwnd);
    (pid != 0).then_some(pid)
}

fn process_image(pid: u32) -> (Option<String>, Option<String>) {
    // SAFETY: a limited query handle, closed before returning; the name is
    // written into `buf`, at most `size` units.
    let path = unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return (None, None);
        };
        let mut path = None;
        // Paths can be longer than MAX_PATH (up to 32767 units).
        for units in [1024usize, 32_768] {
            let mut buf = vec![0u16; units];
            let mut size = units as u32;
            match QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut size,
            ) {
                Ok(()) => {
                    path = Some(String::from_utf16_lossy(&buf[..(size as usize).min(units)]));
                    break;
                }
                Err(e) if e.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult() => continue,
                Err(_) => break,
            }
        }
        let _ = CloseHandle(handle);
        path
    };
    let Some(path) = path else {
        return (None, None);
    };
    let base = path
        .rsplit(['\\', '/'])
        .next()
        .map(|s| strip_exe(s).to_string());
    (Some(path), base)
}

/// A file name without its `.exe`, in any case ("NOTEPAD.EXE").
fn strip_exe(file: &str) -> &str {
    match file.len().checked_sub(4) {
        Some(at) if file.is_char_boundary(at) && file[at..].eq_ignore_ascii_case(".exe") => {
            &file[..at]
        }
        _ => file,
    }
}

/// Whether this process runs elevated (as administrator).
fn self_elevated() -> bool {
    static ELEVATED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // SAFETY: the pseudo handle of this process needs no closing.
    *ELEVATED.get_or_init(|| {
        token_elevated(unsafe { windows::Win32::System::Threading::GetCurrentProcess() })
            .unwrap_or(false)
    })
}

/// Whether process `pid` runs elevated; `None` when that can't be read
/// (access denied, or it is gone).
fn process_elevated(pid: u32) -> Option<bool> {
    // SAFETY: a limited query handle, closed before returning.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let elevated = token_elevated(process);
        let _ = CloseHandle(process);
        elevated
    }
}

/// Whether a process's token is elevated (`TokenElevation`).
fn token_elevated(process: windows::Win32::Foundation::HANDLE) -> Option<bool> {
    use windows::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows::Win32::System::Threading::OpenProcessToken;
    // SAFETY: the token handle is closed before returning; the answer is
    // written into a local of exactly the size given.
    unsafe {
        let mut token = windows::Win32::Foundation::HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let read = GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        let _ = CloseHandle(token);
        read.ok()?;
        Some(elevation.TokenIsElevated != 0)
    }
}

/// Windows (UIPI) shields the target from this process: it runs elevated
/// and this process doesn't. Unknown (`None`) never counts.
fn shielded(ours: bool, theirs: Option<bool>) -> bool {
    !ours && theirs == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_actions_are_pressed_in_any_language() {
        assert_eq!(default_action_name("Drücken", false), "press");
        assert_eq!(default_action_name("Appuyer", false), "press");
        assert_eq!(default_action_name("Click", false), "press");
        assert_eq!(default_action_name("Press", false), "press");
        // A double-click, or an item one click only selects, keeps its name.
        assert_eq!(default_action_name("Double Click", false), "double click");
        assert_eq!(default_action_name("Doppelklicken", true), "doppelklicken");
        assert_eq!(default_action_name("Press", true), "press");
    }

    #[test]
    fn exe_is_stripped_in_any_case() {
        assert_eq!(strip_exe("notepad.exe"), "notepad");
        assert_eq!(strip_exe("NOTEPAD.EXE"), "NOTEPAD");
        assert_eq!(strip_exe("Code.Exe"), "Code");
        assert_eq!(strip_exe("python3.11"), "python3.11");
        assert_eq!(strip_exe("exe"), "exe");
        assert_eq!(strip_exe("ä.exe"), "ä");
        assert_eq!(strip_exe("ää"), "ää");
    }

    #[test]
    fn a_store_apps_frame_belongs_to_the_app() {
        let host = 100;
        let children = |list: &[(&str, u32)]| -> Vec<(String, u32)> {
            list.iter().map(|(c, p)| (c.to_string(), *p)).collect()
        };
        // The host's own windows come first; the app's CoreWindow is found.
        assert_eq!(
            core_window_index(
                host,
                children(&[
                    ("ApplicationFrameTitleBarWindow", host),
                    ("ApplicationFrameInputSinkWindow", host),
                    (UWP_CORE_CLASS, 200),
                ])
            ),
            Some(2)
        );
        // Only the host's windows (the app left the frame): none.
        assert_eq!(
            core_window_index(host, children(&[(UWP_CORE_CLASS, host)])),
            None
        );
        assert_eq!(
            core_window_index(host, children(&[(UWP_CORE_CLASS, 0)])),
            None
        );
    }

    #[test]
    fn only_an_elevated_app_shields_itself_from_a_plain_server() {
        assert!(shielded(false, Some(true)));
        assert!(!shielded(false, Some(false)));
        // Its token couldn't be read: no error.
        assert!(!shielded(false, None));
        // An elevated server reaches everything.
        assert!(!shielded(true, Some(true)));
    }

    /// The first bytes of a PE image with `subsystem`, as a linker writes.
    fn pe_head(magic: u16, subsystem: u16) -> Vec<u8> {
        let mut h = vec![0u8; 0x200];
        h[..2].copy_from_slice(b"MZ");
        h[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        h[0x80..0x84].copy_from_slice(b"PE\0\0");
        h[0x98..0x9a].copy_from_slice(&magic.to_le_bytes());
        h[0x98 + 68..0x98 + 70].copy_from_slice(&subsystem.to_le_bytes());
        h
    }

    #[test]
    fn console_programs_are_told_from_apps() {
        assert_eq!(
            pe_subsystem(&pe_head(0x20b, 3)),
            Some(IMAGE_SUBSYSTEM_WINDOWS_CUI)
        );
        assert_eq!(pe_subsystem(&pe_head(0x10b, 2)), Some(2));
        // Not a PE image, or cut short.
        assert_eq!(pe_subsystem(b"@echo off\r\n"), None);
        assert_eq!(pe_subsystem(&pe_head(0x123, 3)), None);
        assert_eq!(pe_subsystem(&pe_head(0x20b, 3)[..0xa0]), None);
        let mut far = pe_head(0x20b, 3);
        far[0x3c..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(pe_subsystem(&far), None);
    }

    #[test]
    fn localized_start_menu_names_are_read_from_desktop_ini() {
        let ini = "\u{feff}[.ShellClassInfo]\r\nLocalizedResourceName=@%SystemRoot%\\system32\\shell32.dll,-21787\r\n[LocalizedFileNames]\r\nNotepad.lnk=@%SystemRoot%\\system32\\notepad.exe,-9469\r\n; a comment\r\nRechner.lnk = Taschenrechner\r\n";
        let names = localized_file_names(ini.trim_start_matches('\u{feff}'));
        assert_eq!(
            names.get("notepad.lnk").map(String::as_str),
            Some("@%SystemRoot%\\system32\\notepad.exe,-9469")
        );
        assert_eq!(
            names.get("rechner.lnk").map(String::as_str),
            Some("Taschenrechner")
        );
        assert_eq!(names.len(), 2);
        // Windows writes desktop.ini as UTF-16 with a byte-order mark.
        let mut utf16 = vec![0xFF, 0xFE];
        for u in "[LocalizedFileNames]\r\nMail.lnk=Courrier\r\n".encode_utf16() {
            utf16.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(
            localized_file_names(&decode_ini(&utf16))
                .get("mail.lnk")
                .map(String::as_str),
            Some("Courrier")
        );
        // A plain name needs no lookup.
        assert_eq!(display_name("Courrier").as_deref(), Some("Courrier"));
    }

    #[test]
    fn environment_variables_are_expanded() {
        let env =
            |k: &str| (k.eq_ignore_ascii_case("SystemRoot")).then(|| "C:\\Windows".to_string());
        assert_eq!(
            expand_env("@%SystemRoot%\\system32\\notepad.exe,-9469", env),
            "@C:\\Windows\\system32\\notepad.exe,-9469"
        );
        assert_eq!(expand_env("100% %NOPE% 50%", env), "100% %NOPE% 50%");
        assert_eq!(expand_env("%%", env), "%%");
        assert_eq!(expand_env("plain", env), "plain");
    }

    #[test]
    fn uninstallers_are_recognised_in_several_languages() {
        for name in [
            "Uninstall Zoom",
            "Zoom deinstallieren",
            "Désinstaller Zoom",
            "Desinstalar Zoom",
            "Disinstalla Zoom",
            "Odinstaluj Zoom",
            "Zoom アンインストール",
            "卸载 Zoom",
        ] {
            assert!(uninstaller(name), "{name}");
        }
        for name in [
            "Zoom",
            "Google Chrome",
            "Installer Helper",
            "Reinstall Tool",
        ] {
            assert!(!uninstaller(name), "{name}");
        }
    }
}
