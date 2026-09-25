//! Window management on macOS through the Accessibility API (a window's
//! AXPosition, AXSize, AXMinimized and AXFullScreen attributes, its close
//! button, AXRaise) and CoreGraphics / AppKit for the displays. macOS has no
//! public API to move windows between Spaces.

use core_graphics::display::CGDisplay;
use core_graphics::geometry::{CGPoint, CGSize};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSScreen};

use super::ffi;
use crate::error::{Error, Result};
use crate::types::{Display, Rect, WindowOp};

pub fn displays() -> Result<Vec<Display>> {
    let ids = CGDisplay::active_displays()
        .map_err(|e| Error::Platform(format!("could not list the displays ({e})")))?;
    let main = CGDisplay::main().id;
    let mut out: Vec<Display> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let b = CGDisplay::new(*id).bounds();
            let bounds = Rect::new(b.origin.x, b.origin.y, b.size.width, b.size.height);
            Display {
                index: i as u32,
                bounds,
                work_area: bounds,
                primary: *id == main,
            }
        })
        .collect();
    // The usable area (without the menu bar and Dock) is AppKit's
    // visibleFrame, in bottom-left coordinates; only on the main thread.
    if let Some(mtm) = MainThreadMarker::new() {
        let screens = NSScreen::screens(mtm);
        let primary_h = screens.firstObject().map(|s| s.frame().size.height);
        if let Some(h) = primary_h {
            for i in 0..screens.count() {
                let s = screens.objectAtIndex(i);
                let (f, v) = (s.frame(), s.visibleFrame());
                let top = h - (f.origin.y + f.size.height);
                if let Some(d) = out.iter_mut().find(|d| {
                    (d.bounds.x - f.origin.x).abs() < 1.0 && (d.bounds.y - top).abs() < 1.0
                }) {
                    d.work_area = Rect::new(
                        v.origin.x,
                        h - (v.origin.y + v.size.height),
                        v.size.width,
                        v.size.height,
                    );
                }
            }
        }
    }
    Ok(out)
}

fn fail(what: &str) -> Error {
    Error::ActionFailed(format!("the window did not accept {what}"))
}

pub fn apply(win: ffi::AXUIElementRef, pid: u32, op: &WindowOp) -> Result<()> {
    match *op {
        WindowOp::Focus => {
            if ffi::copy_bool(win, "AXMinimized") == Some(true) {
                ffi::set_bool(win, "AXMinimized", false);
            }
            ffi::perform_action(win, "AXRaise");
            ffi::set_bool(win, "AXMain", true);
            if let Some(app) =
                NSRunningApplication::runningApplicationWithProcessIdentifier(pid as libc::pid_t)
            {
                #[allow(deprecated)]
                app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
            }
        }
        WindowOp::SetBounds(r) => {
            if ffi::copy_bool(win, "AXFullScreen") == Some(true) {
                ffi::set_bool(win, "AXFullScreen", false);
            }
            let pos = CGPoint::new(r.x, r.y);
            let size = CGSize::new(r.width, r.height);
            // Move, size, then move again: a resize near a screen edge can
            // shift the window.
            let moved = ffi::set_point(win, "AXPosition", pos);
            let sized = ffi::set_size(win, "AXSize", size);
            ffi::set_point(win, "AXPosition", pos);
            if !moved && !sized {
                return Err(fail("a new position or size"));
            }
        }
        WindowOp::Maximize => {
            // A window's own "zoom" differs per app; fill its display instead.
            let center = match (
                ffi::copy_point(win, "AXPosition"),
                ffi::copy_size(win, "AXSize"),
            ) {
                (Some(p), Some(s)) => {
                    crate::types::Point::new(p.x + s.width / 2.0, p.y + s.height / 2.0)
                }
                _ => crate::types::Point::new(0.0, 0.0),
            };
            let ds = displays()?;
            let d = ds
                .iter()
                .find(|d| d.bounds.contains(center))
                .or_else(|| ds.iter().find(|d| d.primary))
                .ok_or_else(|| Error::Platform("no display".into()))?;
            return apply(win, pid, &WindowOp::SetBounds(d.work_area));
        }
        WindowOp::Minimize => {
            if !ffi::set_bool(win, "AXMinimized", true) {
                return Err(fail("minimizing"));
            }
        }
        WindowOp::Restore => {
            if ffi::copy_bool(win, "AXFullScreen") == Some(true) {
                ffi::set_bool(win, "AXFullScreen", false);
            }
            if ffi::copy_bool(win, "AXMinimized") == Some(true)
                && !ffi::set_bool(win, "AXMinimized", false)
            {
                return Err(fail("restoring"));
            }
        }
        WindowOp::Fullscreen(on) => {
            if !ffi::set_bool(win, "AXFullScreen", on) {
                return Err(fail("full screen"));
            }
        }
        WindowOp::Close => {
            let button = ffi::copy_single_element(win, "AXCloseButton")
                .ok_or_else(|| Error::ActionFailed("this window has no close button".into()))?;
            if !ffi::perform_action(button.as_ref(), "AXPress") {
                return Err(fail("closing"));
            }
        }
        WindowOp::ToDesktop(_) => {
            return Err(Error::Unsupported(
                "macOS has no public way to move windows between Spaces".into(),
            ));
        }
    }
    Ok(())
}
