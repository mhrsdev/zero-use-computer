//! FFI to the macOS Accessibility (AX) API, plus the private
//! `_AXUIElementGetWindow` and the CoreGraphics functions used for background
//! input (`CGEventPostToPid`) and window capture (`CGWindowListCreateImage`).

#![allow(non_upper_case_globals, non_snake_case, dead_code)]

use std::ffi::c_void;
use std::ptr;

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::geometry::{CGPoint, CGRect, CGSize};

pub type AXUIElementRef = CFTypeRef;
pub type AXValueRef = CFTypeRef;
pub type AXError = i32;
pub const kAXErrorSuccess: AXError = 0;

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
    pub fn CGMainDisplayID() -> u32;
    pub fn CGDisplayBounds(display: u32) -> CGRect;
}

// CGWindowListOption / CGWindowImageOption bits we use.
pub const kCGWindowListOptionOnScreenOnly: u32 = 1 << 0;
pub const kCGNullWindowID: u32 = 0;
pub const kCGWindowListOptionIncludingWindow: u32 = 1 << 3;
pub const kCGWindowImageBoundsIgnoreFraming: u32 = 1 << 0;
pub const kCGWindowImageBestResolution: u32 = 1 << 3;

/// Build a CFString for an AX attribute/action name.
pub fn cfstr(s: &str) -> CFString {
    CFString::new(s)
}

fn as_ref(s: &CFString) -> CFStringRef {
    s.as_concrete_TypeRef()
}

/// Copy an attribute as a generic CFType.
pub fn copy_attr(element: AXUIElementRef, attr: &str) -> Option<CFType> {
    let name = cfstr(attr);
    let mut out: CFTypeRef = ptr::null();
    let err = unsafe { AXUIElementCopyAttributeValue(element, as_ref(&name), &mut out) };
    if err != kAXErrorSuccess || out.is_null() {
        return None;
    }
    Some(unsafe { CFType::wrap_under_create_rule(out) })
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
    let Some(v) = copy_attr(element, attr) else {
        return Vec::new();
    };
    let Some(array) = v.downcast::<CFArray>() else {
        return Vec::new();
    };
    array
        .get_all_values()
        .into_iter()
        .filter_map(|raw| unsafe { AxRef::from_get(raw) })
        .collect()
}

pub fn copy_single_element(element: AXUIElementRef, attr: &str) -> Option<AxRef> {
    let v = copy_attr(element, attr)?;
    unsafe { AxRef::from_get(v.as_CFTypeRef()) }
}

pub fn action_names(element: AXUIElementRef) -> Vec<String> {
    let mut out: CFTypeRef = ptr::null();
    let err = unsafe { AXUIElementCopyActionNames(element, &mut out) };
    if err != kAXErrorSuccess || out.is_null() {
        return Vec::new();
    }
    let cf = unsafe { CFType::wrap_under_create_rule(out) };
    let Some(array) = cf.downcast::<CFArray>() else {
        return Vec::new();
    };
    array
        .get_all_values()
        .into_iter()
        .filter_map(|raw| {
            if raw.is_null() {
                None
            } else {
                Some(unsafe { CFString::wrap_under_get_rule(raw as CFStringRef) }.to_string())
            }
        })
        .collect()
}

pub fn perform_action(element: AXUIElementRef, action: &str) -> bool {
    let name = cfstr(action);
    unsafe { AXUIElementPerformAction(element, as_ref(&name)) == kAXErrorSuccess }
}

pub fn is_settable(element: AXUIElementRef, attr: &str) -> bool {
    let name = cfstr(attr);
    let mut settable: u8 = 0;
    unsafe { AXUIElementIsAttributeSettable(element, as_ref(&name), &mut settable) };
    settable != 0
}

pub fn set_string(element: AXUIElementRef, attr: &str, value: &str) -> bool {
    let name = cfstr(attr);
    let val = cfstr(value);
    unsafe {
        AXUIElementSetAttributeValue(element, as_ref(&name), val.as_CFTypeRef()) == kAXErrorSuccess
    }
}

pub fn set_bool(element: AXUIElementRef, attr: &str, value: bool) -> bool {
    let name = cfstr(attr);
    let b = if value {
        CFBoolean::true_value()
    } else {
        CFBoolean::false_value()
    };
    unsafe {
        AXUIElementSetAttributeValue(element, as_ref(&name), b.as_CFTypeRef()) == kAXErrorSuccess
    }
}

pub fn set_range(element: AXUIElementRef, attr: &str, location: isize, length: isize) -> bool {
    let range = CFRange { location, length };
    let value = unsafe { AXValueCreate(kAXValueCFRangeType, &range as *const _ as *const c_void) };
    if value.is_null() {
        return false;
    }
    let name = cfstr(attr);
    let ok =
        unsafe { AXUIElementSetAttributeValue(element, as_ref(&name), value) == kAXErrorSuccess };
    unsafe { core_foundation::base::CFRelease(value) };
    ok
}

pub fn copy_point(element: AXUIElementRef, attr: &str) -> Option<CGPoint> {
    let v = copy_attr(element, attr)?;
    let mut p = CGPoint { x: 0.0, y: 0.0 };
    let ok = unsafe {
        AXValueGetValue(
            v.as_CFTypeRef(),
            kAXValueCGPointType,
            &mut p as *mut _ as *mut c_void,
        )
    };
    (ok != 0).then_some(p)
}

pub fn copy_size(element: AXUIElementRef, attr: &str) -> Option<CGSize> {
    let v = copy_attr(element, attr)?;
    let mut s = CGSize {
        width: 0.0,
        height: 0.0,
    };
    let ok = unsafe {
        AXValueGetValue(
            v.as_CFTypeRef(),
            kAXValueCGSizeType,
            &mut s as *mut _ as *mut c_void,
        )
    };
    (ok != 0).then_some(s)
}

/// The CGWindowID behind an AX window element, via the private SPI.
pub fn window_id(element: AXUIElementRef) -> Option<u32> {
    let mut id: u32 = 0;
    let err = unsafe { _AXUIElementGetWindow(element, &mut id) };
    (err == kAXErrorSuccess && id != 0).then_some(id)
}

/// Convert a CFType (string, number or boolean) to a display string.
pub fn cftype_to_string(v: &CFType) -> Option<String> {
    if let Some(s) = v.downcast::<CFString>() {
        return Some(s.to_string());
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
