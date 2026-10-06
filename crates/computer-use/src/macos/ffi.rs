//! FFI to the macOS Accessibility (AX) API, plus the private
//! `_AXUIElementGetWindow` and the CoreGraphics functions used for background
//! input (`CGEventPostToPid`) and window capture (`CGWindowListCreateImage`).

#![allow(non_upper_case_globals, non_snake_case, dead_code)]

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use core_foundation::array::CFArray;
use core_foundation::base::{CFGetTypeID, CFType, CFTypeID, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringGetCharacters, CFStringGetLength, CFStringRef};
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use objc2_foundation::NSString;

pub type AXUIElementRef = CFTypeRef;
pub type AXValueRef = CFTypeRef;
pub type AXError = i32;
pub const kAXErrorSuccess: AXError = 0;
pub const kAXErrorIllegalArgument: AXError = -25201;
/// The element no longer exists (its window closed, the app quit).
pub const kAXErrorInvalidUIElement: AXError = -25202;
/// The app did not answer within the messaging timeout: it is busy, hung,
/// or (after an action) running a modal dialog the action opened.
pub const kAXErrorCannotComplete: AXError = -25204;
/// Accessibility access is off for this process.
pub const kAXErrorAPIDisabled: AXError = -25211;
/// The attribute has no value.
pub const kAXErrorNoValue: AXError = -25212;

/// `kAXErrorCannotComplete` that came back at once: the app didn't time
/// out (it may be launching, or this element can't answer through AX, as in
/// some views hosted by another process), so it says nothing about the next
/// call. Not a code macOS uses.
pub const kAXErrorCannotCompleteAtOnce: AXError = -25299;

/// Under this (ms), `kAXErrorCannotComplete` is no timeout. Set from the
/// messaging timeout by [`set_messaging_timeout`].
static AT_ONCE_MS: AtomicU64 = AtomicU64::new(250);

/// How soon a `kAXErrorCannotComplete` must come back to be no timeout: at
/// most 250 ms, and under half the messaging timeout `secs` (a short
/// timeout would otherwise read every real timeout as "at once").
pub fn at_once_threshold(secs: f32) -> Duration {
    // Half of it, in whole ms (NaN and negative read as 0).
    let half_ms = (secs.max(0.0) * 500.0) as u64;
    Duration::from_millis(half_ms.min(250))
}

/// Note the messaging timeout (s) in use, for telling timeouts apart.
pub fn set_messaging_timeout(secs: f32) {
    AT_ONCE_MS.store(
        at_once_threshold(secs).as_millis() as u64,
        Ordering::Relaxed,
    );
}

/// `err`, with a `kAXErrorCannotComplete` that came back after `elapsed`,
/// sooner than `threshold`, told apart ([`kAXErrorCannotCompleteAtOnce`]).
pub fn timed_after(err: AXError, elapsed: Duration, threshold: Duration) -> AXError {
    if err == kAXErrorCannotComplete && elapsed < threshold {
        kAXErrorCannotCompleteAtOnce
    } else {
        err
    }
}

/// [`timed_after`] for a call made at `started`.
fn timed(err: AXError, started: Instant) -> AXError {
    let threshold = Duration::from_millis(AT_ONCE_MS.load(Ordering::Relaxed));
    timed_after(err, started.elapsed(), threshold)
}

/// What an AX error means, for messages (`None` for success and codes
/// macOS doesn't document).
pub fn describe(err: AXError) -> Option<&'static str> {
    Some(match err {
        -25200 => "the accessibility request failed in the system",
        kAXErrorIllegalArgument => "the request was not valid for this element",
        kAXErrorInvalidUIElement => {
            "the element no longer exists (its window closed, or the app quit)"
        }
        -25203 => "the observer is no longer valid",
        kAXErrorCannotComplete => {
            "the app did not answer in time (it may be busy, hung, or showing a dialog)"
        }
        -25205 => "the element does not have this attribute",
        -25206 => "the element does not support this action",
        -25207 | -25209 | -25210 => "the app does not send this notification",
        -25208 => "the app does not implement this part of the accessibility API",
        kAXErrorAPIDisabled => "Accessibility access is turned off for this server",
        kAXErrorNoValue => "the attribute has no value",
        -25213 => "the element does not have this parameterized attribute",
        -25214 => "the value cannot be given precisely enough",
        kAXErrorCannotCompleteAtOnce => {
            "the app could not answer right now (it may still be starting up, or this part of it is drawn by another process)"
        }
        _ => return None,
    })
}

/// An error after which no further call to the app does better for now:
/// it isn't answering (it timed out), or Accessibility access was revoked.
pub fn is_fatal(err: AXError) -> bool {
    err == kAXErrorCannotComplete || err == kAXErrorAPIDisabled
}

/// `Ok` for `kAXErrorSuccess`, else the error.
pub fn check(err: AXError) -> Result<(), AXError> {
    if err == kAXErrorSuccess {
        Ok(())
    } else {
        Err(err)
    }
}

// AXValueType tags.
pub const kAXValueCGPointType: u32 = 1;
pub const kAXValueCGSizeType: u32 = 2;
pub const kAXValueCGRectType: u32 = 3;
pub const kAXValueCFRangeType: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CFRange {
    pub location: isize,
    pub length: isize,
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    pub fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    pub fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    pub fn AXUIElementGetTypeID() -> CFTypeID;
    pub fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    pub fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    pub fn AXUIElementCopyActionNames(element: AXUIElementRef, names: *mut CFTypeRef) -> AXError;
    pub fn AXUIElementPerformAction(element: AXUIElementRef, action: CFStringRef) -> AXError;
    pub fn AXUIElementIsAttributeSettable(
        element: AXUIElementRef,
        attribute: CFStringRef,
        settable: *mut u8,
    ) -> AXError;
    pub fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout: f32) -> AXError;
    pub fn AXValueGetValue(value: AXValueRef, type_: u32, out: *mut c_void) -> u8;
    pub fn AXValueGetType(value: AXValueRef) -> u32;
    pub fn AXValueGetTypeID() -> CFTypeID;
    pub fn AXUIElementCopyMultipleAttributeValues(
        element: AXUIElementRef,
        attributes: CFTypeRef, // CFArrayRef of CFStringRef
        options: u32,
        values: *mut CFTypeRef, // CFArrayRef
    ) -> AXError;
    pub fn AXValueCreate(type_: u32, value: *const c_void) -> AXValueRef;
    pub fn AXIsProcessTrusted() -> u8;
    // Private but stable: the CGWindowID behind an AX window element.
    pub fn _AXUIElementGetWindow(element: AXUIElementRef, out: *mut u32) -> AXError;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub fn CGWindowListCreateImage(
        screen_bounds: CGRect,
        list_option: u32,
        window_id: u32,
        image_option: u32,
    ) -> *const c_void; // CGImageRef
    /// The `CGBitmapInfo` of a `CGImageRef`: alpha placement, byte order.
    pub fn CGImageGetBitmapInfo(image: *const c_void) -> u32;
    /// The image's data provider (+0), or null.
    pub fn CGImageGetDataProvider(image: *const c_void) -> *const c_void;
    /// A copy (+1 CFDataRef) of a data provider's bytes, or null.
    pub fn CGDataProviderCopyData(provider: *const c_void) -> CFTypeRef;
    /// A bitmap context (+1 CGContextRef) drawing into `data`, or null.
    pub fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: *const c_void,
        bitmap_info: u32,
    ) -> *mut c_void;
    pub fn CGMainDisplayID() -> u32;
    pub fn CGDisplayBounds(display: u32) -> CGRect;
    /// Seconds since the last input event of a type (only the time).
    pub fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
}

/// `kCGEventSourceStateHIDSystemState`: hardware input only (events posted
/// to an app with `CGEventPostToPid` don't count).
pub const kCGEventSourceStateHIDSystemState: i32 = 1;
/// `kCGAnyInputEventType`.
pub const kCGAnyInputEventType: u32 = !0;

// CGWindowListOption / CGWindowImageOption bits we use.
pub const kCGWindowListOptionOnScreenOnly: u32 = 1 << 0;
pub const kCGNullWindowID: u32 = 0;
pub const kCGWindowListOptionIncludingWindow: u32 = 1 << 3;
pub const kCGWindowListExcludeDesktopElements: u32 = 1 << 4;
pub const kCGWindowImageBoundsIgnoreFraming: u32 = 1 << 0;
pub const kCGWindowImageBestResolution: u32 = 1 << 3;
/// One pixel per point, whatever the displays' scale.
pub const kCGWindowImageNominalResolution: u32 = 1 << 4;

/// Whether this process may capture other apps' windows (the Screen
/// Recording permission), by `CGPreflightScreenCaptureAccess` (macOS
/// 10.15+, looked up at run time so older systems still start). `None`
/// before 10.15, which has no such permission. Without it, captures come
/// back with the wallpaper and menu bar only.
pub fn screen_capture_allowed() -> Option<bool> {
    type Preflight = unsafe extern "C" fn() -> bool;
    static PREFLIGHT: std::sync::OnceLock<Option<Preflight>> = std::sync::OnceLock::new();
    let preflight = (*PREFLIGHT.get_or_init(|| {
        // SAFETY: looking up an exported CoreGraphics function by name.
        let sym = unsafe {
            libc::dlsym(
                libc::RTLD_DEFAULT,
                c"CGPreflightScreenCaptureAccess".as_ptr(),
            )
        };
        // SAFETY: the symbol is `bool CGPreflightScreenCaptureAccess(void)`.
        (!sym.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, Preflight>(sym) })
    }))?;
    // SAFETY: a plain query, without side effects (no prompt).
    Some(unsafe { preflight() })
}

/// Build a CFString for an AX attribute/action name.
pub fn cfstr(s: &str) -> CFString {
    CFString::new(s)
}

fn as_ref(s: &CFString) -> CFStringRef {
    s.as_concrete_TypeRef()
}

/// Copy an attribute as a generic CFType, or the AX error that stopped it
/// (`kAXErrorNoValue` when the call succeeded without a value).
pub fn try_copy_attr(element: AXUIElementRef, attr: &str) -> Result<CFType, AXError> {
    let name = cfstr(attr);
    let mut out: CFTypeRef = ptr::null();
    let started = Instant::now();
    let err = unsafe { AXUIElementCopyAttributeValue(element, as_ref(&name), &mut out) };
    // SAFETY: a +1 reference from a Copy call, released on drop.
    let value = (!out.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(out) });
    check(timed(err, started))?;
    value.ok_or(kAXErrorNoValue)
}

/// Copy an attribute, where only a fatal error ([`is_fatal`]) is an error:
/// a missing attribute is `Ok(None)`.
pub fn read_attr(element: AXUIElementRef, attr: &str) -> Result<Option<CFType>, AXError> {
    match try_copy_attr(element, attr) {
        Ok(v) => Ok(Some(v)),
        Err(e) if is_fatal(e) => Err(e),
        Err(_) => Ok(None),
    }
}

/// Copy an attribute as a generic CFType.
pub fn copy_attr(element: AXUIElementRef, attr: &str) -> Option<CFType> {
    try_copy_attr(element, attr).ok()
}

pub fn copy_string(element: AXUIElementRef, attr: &str) -> Option<String> {
    copy_attr(element, attr).and_then(|v| cftype_to_string(&v))
}

pub fn copy_bool(element: AXUIElementRef, attr: &str) -> Option<bool> {
    let v = copy_attr(element, attr)?;
    v.downcast::<CFBoolean>()
        .map(|b| b == CFBoolean::true_value())
}

/// Copy an AX child list attribute (e.g. AXChildren, AXWindows), each element
/// wrapped as an owned [`AxRef`].
pub fn copy_elements(element: AXUIElementRef, attr: &str) -> Vec<AxRef> {
    value_to_elements(&copy_attr(element, attr))
}

pub fn copy_single_element(element: AXUIElementRef, attr: &str) -> Option<AxRef> {
    let v = copy_attr(element, attr)?;
    // SAFETY: `v` is a live CF object; we take our own retain.
    unsafe { AxRef::element_from_get(v.as_CFTypeRef()) }
}

/// The element's action names, or the AX error that stopped the call.
pub fn action_names(element: AXUIElementRef) -> Result<Vec<String>, AXError> {
    let mut out: CFTypeRef = ptr::null();
    let started = Instant::now();
    let err = unsafe { AXUIElementCopyActionNames(element, &mut out) };
    // SAFETY: a +1 reference from a Copy call, released on drop.
    let cf = (!out.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(out) });
    check(timed(err, started))?;
    let Some(array) = cf.and_then(|cf| cf.downcast::<CFArray>()) else {
        return Ok(Vec::new());
    };
    Ok(array
        .get_all_values()
        .into_iter()
        .filter(|raw| !raw.is_null())
        // SAFETY: a non-null item of the array, retained while wrapped.
        .filter_map(|raw| unsafe { CFType::wrap_under_get_rule(raw) }.downcast::<CFString>())
        .map(|s| cfstring_text(&s))
        .collect())
}

/// Perform an AX action. `kAXErrorCannotComplete` here means the action
/// was sent but the app didn't answer in time: it may well have happened
/// (a press that opened a modal dialog answers only once it closes).
pub fn perform_action(element: AXUIElementRef, action: &str) -> Result<(), AXError> {
    let name = cfstr(action);
    check(unsafe { AXUIElementPerformAction(element, as_ref(&name)) })
}

/// Whether the attribute can be set, or the AX error that stopped the call.
pub fn try_is_settable(element: AXUIElementRef, attr: &str) -> Result<bool, AXError> {
    let name = cfstr(attr);
    let mut settable: u8 = 0;
    let started = Instant::now();
    let err = unsafe { AXUIElementIsAttributeSettable(element, as_ref(&name), &mut settable) };
    check(timed(err, started))?;
    Ok(settable != 0)
}

pub fn is_settable(element: AXUIElementRef, attr: &str) -> bool {
    try_is_settable(element, attr).unwrap_or(false)
}

fn set_cf(element: AXUIElementRef, attr: &str, value: CFTypeRef) -> Result<(), AXError> {
    let name = cfstr(attr);
    check(unsafe { AXUIElementSetAttributeValue(element, as_ref(&name), value) })
}

pub fn set_string(element: AXUIElementRef, attr: &str, value: &str) -> Result<(), AXError> {
    let val = cfstr(value);
    set_cf(element, attr, val.as_CFTypeRef())
}

pub fn set_bool(element: AXUIElementRef, attr: &str, value: bool) -> Result<(), AXError> {
    let b = if value {
        CFBoolean::true_value()
    } else {
        CFBoolean::false_value()
    };
    set_cf(element, attr, b.as_CFTypeRef())
}

/// Set an attribute to an AXValue of type `kind` holding `v`.
fn set_value_of<T>(element: AXUIElementRef, attr: &str, kind: u32, v: &T) -> Result<(), AXError> {
    let value = unsafe { AXValueCreate(kind, v as *const T as *const c_void) };
    if value.is_null() {
        return Err(kAXErrorIllegalArgument);
    }
    let r = set_cf(element, attr, value);
    unsafe { core_foundation::base::CFRelease(value) };
    r
}

pub fn set_range(
    element: AXUIElementRef,
    attr: &str,
    location: isize,
    length: isize,
) -> Result<(), AXError> {
    let range = CFRange { location, length };
    set_value_of(element, attr, kAXValueCFRangeType, &range)
}

pub fn set_point(element: AXUIElementRef, attr: &str, p: CGPoint) -> Result<(), AXError> {
    set_value_of(element, attr, kAXValueCGPointType, &p)
}

pub fn set_size(element: AXUIElementRef, attr: &str, s: CGSize) -> Result<(), AXError> {
    set_value_of(element, attr, kAXValueCGSizeType, &s)
}

pub fn copy_point(element: AXUIElementRef, attr: &str) -> Option<CGPoint> {
    value_to_point(&copy_attr(element, attr))
}

pub fn copy_size(element: AXUIElementRef, attr: &str) -> Option<CGSize> {
    value_to_size(&copy_attr(element, attr))
}

/// The struct inside an AXValue, if `v` is an AXValue of type `kind` (whose
/// payload is a `T`).
fn ax_value<T: Copy>(v: &CFType, kind: u32, mut out: T) -> Option<T> {
    let raw = v.as_CFTypeRef();
    // SAFETY: AXValueGetType/GetValue are only called on an AXValue, and
    // only copy into a `T` when its type tag is the one `T` stands for.
    unsafe {
        if CFGetTypeID(raw) != AXValueGetTypeID() || AXValueGetType(raw) != kind {
            return None;
        }
        (AXValueGetValue(raw, kind, &mut out as *mut T as *mut c_void) != 0).then_some(out)
    }
}

/// AXValueType tag used for per-attribute errors in multi-attribute results.
pub const kAXValueAXErrorType: u32 = 5;

/// Fetch several attributes in one IPC round trip. Each slot is `None` when
/// the attribute is missing or errored. Returns the AX error if the call
/// itself fails, so callers can fall back to per-attribute reads (unless
/// it [`is_fatal`]: then they would only wait as long again for each).
pub fn copy_attrs(element: AXUIElementRef, names: &[&str]) -> Result<Vec<Option<CFType>>, AXError> {
    let cf_names: Vec<CFString> = names.iter().map(|n| cfstr(n)).collect();
    let array = CFArray::from_CFTypes(&cf_names);
    let mut out: CFTypeRef = ptr::null();
    let started = Instant::now();
    let err = unsafe {
        AXUIElementCopyMultipleAttributeValues(element, array.as_CFTypeRef(), 0, &mut out)
    };
    // SAFETY: a +1 reference from a Copy call, released on drop.
    let values = (!out.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(out) });
    check(timed(err, started))?;
    let values = values
        .and_then(|v| v.downcast::<CFArray>())
        .ok_or(kAXErrorNoValue)?;
    let ax_value_type = unsafe { AXValueGetTypeID() };
    let mut result = Vec::with_capacity(names.len());
    for raw in values.get_all_values() {
        if raw.is_null() {
            result.push(None);
            continue;
        }
        let v = unsafe { CFType::wrap_under_get_rule(raw) };
        let is_error =
            v.type_of() == ax_value_type && unsafe { AXValueGetType(raw) } == kAXValueAXErrorType;
        result.push((!is_error).then_some(v));
    }
    if result.len() == names.len() {
        Ok(result)
    } else {
        Err(kAXErrorNoValue)
    }
}

pub fn value_to_string(v: &Option<CFType>) -> Option<String> {
    v.as_ref().and_then(cftype_to_string)
}

pub fn value_to_bool(v: &Option<CFType>) -> Option<bool> {
    v.as_ref()?
        .downcast::<CFBoolean>()
        .map(|b| b == CFBoolean::true_value())
}

pub fn value_to_point(v: &Option<CFType>) -> Option<CGPoint> {
    ax_value(v.as_ref()?, kAXValueCGPointType, CGPoint { x: 0.0, y: 0.0 })
}

pub fn value_to_size(v: &Option<CFType>) -> Option<CGSize> {
    let zero = CGSize {
        width: 0.0,
        height: 0.0,
    };
    ax_value(v.as_ref()?, kAXValueCGSizeType, zero)
}

pub fn value_to_elements(v: &Option<CFType>) -> Vec<AxRef> {
    let Some(array) = v.as_ref().and_then(|v| v.downcast::<CFArray>()) else {
        return Vec::new();
    };
    array
        .get_all_values()
        .into_iter()
        // SAFETY: items of a live array; each kept one is retained.
        .filter_map(|raw| unsafe { AxRef::element_from_get(raw) })
        .collect()
}

/// The CGWindowID behind an AX window element, via the private SPI.
pub fn window_id(element: AXUIElementRef) -> Option<u32> {
    let mut id: u32 = 0;
    let err = unsafe { _AXUIElementGetWindow(element, &mut id) };
    (err == kAXErrorSuccess && id != 0).then_some(id)
}

/// A CFString's text, read as UTF-16. Other apps can put a lone surrogate in
/// a title or a value; it becomes U+FFFD here, where `CFString`'s `Display`
/// panics (it converts to strict UTF-8 and asserts every character made it).
pub fn cfstring_text(s: &CFString) -> String {
    utf16_text(s.as_concrete_TypeRef())
}

/// An NSString's text, read as UTF-16 like [`cfstring_text`]: its `Display`
/// goes through `UTF8String`, which is NULL for a lone surrogate (and objc2
/// then makes a slice from that NULL).
pub fn nsstring_text(s: &NSString) -> String {
    // NSString is toll-free bridged with CFString.
    utf16_text((s as *const NSString).cast())
}

fn utf16_text(s: CFStringRef) -> String {
    // SAFETY: `s` is a live CFString (the callers borrow it), and the buffer
    // holds the `len` UTF-16 units asked for.
    let len = unsafe { CFStringGetLength(s) };
    let mut units = vec![0u16; usize::try_from(len).unwrap_or(0)];
    if !units.is_empty() {
        let range = core_foundation::base::CFRange {
            location: 0,
            length: len,
        };
        unsafe { CFStringGetCharacters(s, range, units.as_mut_ptr()) };
    }
    String::from_utf16_lossy(&units)
}

/// Convert a CFType (string, number or boolean) to a display string.
pub fn cftype_to_string(v: &CFType) -> Option<String> {
    if let Some(s) = v.downcast::<CFString>() {
        return Some(cfstring_text(&s));
    }
    if let Some(n) = v.downcast::<CFNumber>() {
        if let Some(i) = n.to_i64() {
            return Some(i.to_string());
        }
        if let Some(f) = n.to_f64() {
            return Some(f.to_string());
        }
    }
    if let Some(b) = v.downcast::<CFBoolean>() {
        return Some(if b == CFBoolean::true_value() {
            "true".into()
        } else {
            "false".into()
        });
    }
    None
}

/// An owned, reference-counted AX element handle.
pub struct AxRef(AXUIElementRef);

impl AxRef {
    /// Wrap a +1 reference (from a Create/Copy call).
    pub unsafe fn from_create(ptr: AXUIElementRef) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(AxRef(ptr))
        }
    }
    /// Wrap a borrowed (+0) reference, taking our own retain.
    pub unsafe fn from_get(ptr: AXUIElementRef) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(AxRef(unsafe { core_foundation::base::CFRetain(ptr) }))
        }
    }
    /// Like [`AxRef::from_get`], but only for a CF object that is an
    /// AXUIElement (an attribute's value may be of any type).
    pub unsafe fn element_from_get(ptr: CFTypeRef) -> Option<Self> {
        // SAFETY: `ptr` is a live CF object (null is checked first).
        if ptr.is_null() || unsafe { CFGetTypeID(ptr) != AXUIElementGetTypeID() } {
            return None;
        }
        unsafe { Self::from_get(ptr) }
    }
    pub fn as_ref(&self) -> AXUIElementRef {
        self.0
    }
}

impl Clone for AxRef {
    fn clone(&self) -> Self {
        AxRef(unsafe { core_foundation::base::CFRetain(self.0) })
    }
}

impl Drop for AxRef {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { core_foundation::base::CFRelease(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_once_is_under_half_the_timeout() {
        assert_eq!(at_once_threshold(5.0), Duration::from_millis(250));
        assert_eq!(at_once_threshold(0.5), Duration::from_millis(250));
        assert_eq!(at_once_threshold(0.2), Duration::from_millis(100));
        assert_eq!(at_once_threshold(0.0), Duration::ZERO);
    }

    #[test]
    fn a_real_timeout_is_not_read_as_at_once() {
        // With a 0.2 s timeout, a failure after 180 ms is that timeout.
        let threshold = at_once_threshold(0.2);
        let late = Duration::from_millis(180);
        assert_eq!(
            timed_after(kAXErrorCannotComplete, late, threshold),
            kAXErrorCannotComplete
        );
        let early = Duration::from_millis(5);
        assert_eq!(
            timed_after(kAXErrorCannotComplete, early, threshold),
            kAXErrorCannotCompleteAtOnce
        );
        // Other errors are left alone.
        assert_eq!(
            timed_after(kAXErrorInvalidUIElement, early, threshold),
            kAXErrorInvalidUIElement
        );
    }

    #[test]
    fn common_errors_are_described() {
        for e in [
            kAXErrorInvalidUIElement,
            kAXErrorCannotComplete,
            kAXErrorCannotCompleteAtOnce,
            kAXErrorAPIDisabled,
            -25205,
            -25208,
        ] {
            assert!(describe(e).is_some_and(|d| !d.is_empty()), "{e}");
        }
        assert_eq!(describe(kAXErrorSuccess), None);
        assert_eq!(describe(-1), None);
    }

    /// A CFString of these UTF-16 units, as another app could hand over.
    fn utf16_cfstring(units: &[u16]) -> CFString {
        // SAFETY: the buffer holds `units.len()` units; +1, owned by the wrapper.
        unsafe {
            CFString::wrap_under_create_rule(core_foundation::string::CFStringCreateWithCharacters(
                core_foundation::base::kCFAllocatorDefault,
                units.as_ptr(),
                units.len() as isize,
            ))
        }
    }

    #[test]
    fn a_lone_surrogate_is_read_as_a_replacement_character() {
        let s = utf16_cfstring(&[0x61, 0xD800, 0x62]);
        assert_eq!(cfstring_text(&s), "a\u{FFFD}b");
        // SAFETY: CFString and NSString are toll-free bridged.
        let ns = unsafe { &*(s.as_concrete_TypeRef() as *const NSString) };
        assert_eq!(nsstring_text(ns), "a\u{FFFD}b");
    }

    #[test]
    fn ordinary_text_is_read_whole() {
        let s = CFString::new("héllo 👋");
        assert_eq!(cfstring_text(&s), "héllo 👋");
        assert_eq!(cfstring_text(&CFString::new("")), "");
    }
}
