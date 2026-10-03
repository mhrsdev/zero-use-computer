//! Window management on macOS through the Accessibility API (a window's
//! AXPosition, AXSize, AXMinimized and AXFullScreen attributes, its close
//! button, AXRaise, the app's AXFrontmost) and CoreGraphics / AppKit for the
//! displays. macOS has no public API to move windows between Spaces.

use std::time::{Duration, Instant};

use core_graphics::display::CGDisplay;
use core_graphics::geometry::{CGPoint, CGSize};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSScreen};

use super::ffi;
use crate::error::{Error, Result};
use crate::types::{AppInfo, Display, Rect, WindowOp};

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

/// The error for a window change that failed with `err`.
fn failed(err: ffi::AXError, app: &AppInfo, what: &str) -> Error {
    match err {
        ffi::kAXErrorCannotComplete => Error::Unanswered(app.name.clone()),
        ffi::kAXErrorAPIDisabled => Error::Permission(
            "Accessibility is turned off for this server: enable the app that runs it under System Settings ▸ Privacy & Security ▸ Accessibility".into(),
        ),
        _ => fail(what),
    }
}

/// Take the window out of full screen, and wait (up to 2 s) until it is:
/// the animation ignores a move or resize sent meanwhile.
fn leave_full_screen(win: ffi::AXUIElementRef, app: &AppInfo) -> Result<()> {
    if ffi::copy_bool(win, "AXFullScreen") != Some(true) {
        return Ok(());
    }
    ffi::set_bool(win, "AXFullScreen", false).map_err(|e| failed(e, app, "leaving full screen"))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while ffi::copy_bool(win, "AXFullScreen") == Some(true) {
        if Instant::now() >= deadline {
            return Err(Error::ActionFailed(
                "the window is still leaving full screen; try again in a moment".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// Bring the app forward. `activateWithOptions` alone is only a request
/// since macOS 14 (IgnoringOtherApps is ignored, and an app in the
/// background may be refused), so AXFrontmost is set too.
fn activate(app_el: ffi::AXUIElementRef, app: &AppInfo) -> Result<()> {
    let activated =
        NSRunningApplication::runningApplicationWithProcessIdentifier(app.pid as libc::pid_t)
            .is_some_and(|a| {
                #[allow(deprecated)]
                a.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps)
            });
    match ffi::set_bool(app_el, "AXFrontmost", true) {
        Ok(()) => Ok(()),
        Err(e) if ffi::is_fatal(e) => Err(failed(e, app, "")),
        Err(_) if activated => Ok(()),
        Err(_) => Err(Error::ActionFailed(format!(
            "{} could not be brought to the front",
            app.name
        ))),
    }
}

/// The window's frame, if it tells it.
fn frame(win: ffi::AXUIElementRef) -> Option<Rect> {
    let p = ffi::copy_point(win, "AXPosition")?;
    let s = ffi::copy_size(win, "AXSize")?;
    Some(Rect::new(p.x, p.y, s.width, s.height))
}

/// Within this (points), a window's edge hasn't moved.
const SAME: f64 = 1.0;

/// What of a move and resize to `want` didn't happen, by the window's
/// frame `before` and `after`: a part that was to change but didn't move
/// at all. A part that changed counts as done even when it isn't exactly
/// as asked (the system keeps windows below the menu bar, and some apps
/// size in steps or have a minimum size). `None` when all is done, or
/// the frames aren't known.
fn unapplied(before: Option<Rect>, after: Option<Rect>, want: Rect) -> Option<String> {
    let (b, a) = (before?, after?);
    let near = |x: f64, y: f64| (x - y).abs() <= SAME;
    let stuck = |from: (f64, f64), to: (f64, f64), wanted: (f64, f64)| {
        let asked = !(near(from.0, wanted.0) && near(from.1, wanted.1));
        asked && near(from.0, to.0) && near(from.1, to.1)
    };
    let not_moved = stuck((b.x, b.y), (a.x, a.y), (want.x, want.y));
    let not_sized = stuck(
        (b.width, b.height),
        (a.width, a.height),
        (want.width, want.height),
    );
    let what = match (not_moved, not_sized) {
        (false, false) => return None,
        (true, false) => "moved",
        (false, true) => "resized",
        (true, true) => "moved or resized",
    };
    Some(format!(
        "the window could not be {what}: it is still at {:.0},{:.0}, {:.0}×{:.0} (the app may not allow it)",
        a.x, a.y, a.width, a.height
    ))
}

pub fn apply(
    win: ffi::AXUIElementRef,
    app_el: ffi::AXUIElementRef,
    app: &AppInfo,
    op: &WindowOp,
) -> Result<()> {
    match *op {
        WindowOp::Focus => {
            if ffi::copy_bool(win, "AXMinimized") == Some(true) {
                ffi::set_bool(win, "AXMinimized", false)
                    .map_err(|e| failed(e, app, "restoring"))?;
            }
            // Raised and made main first: activating the app then brings
            // this window forward with it.
            if let Err(e) = ffi::perform_action(win, "AXRaise") {
                if ffi::is_fatal(e) {
                    return Err(failed(e, app, "raising"));
                }
                log::debug!("AXRaise failed ({e}); activating {} anyway", app.name);
            }
            // Not every window can be main (panels); raising is what counts.
            let _ = ffi::set_bool(win, "AXMain", true);
            activate(app_el, app)?;
        }
        WindowOp::SetBounds(r) => {
            leave_full_screen(win, app)?;
            let before = frame(win);
            let pos = CGPoint::new(r.x, r.y);
            let size = CGSize::new(r.width, r.height);
            // Move, size, then move again: a resize near a screen edge can
            // shift the window.
            let moved = ffi::set_point(win, "AXPosition", pos);
            let sized = ffi::set_size(win, "AXSize", size);
            let _ = ffi::set_point(win, "AXPosition", pos);
            if let (Err(e), Err(_)) = (moved, sized) {
                return Err(failed(e, app, "a new position or size"));
            }
            // One of the two may have been refused, or ignored. (Given a
            // moment: a window may animate there.)
            let deadline = Instant::now() + Duration::from_millis(300);
            let mut why = unapplied(before, frame(win), r);
            while why.is_some() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
                why = unapplied(before, frame(win), r);
            }
            if let Some(why) = why {
                return Err(Error::ActionFailed(why));
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
            return apply(win, app_el, app, &WindowOp::SetBounds(d.work_area));
        }
        WindowOp::Minimize => {
            ffi::set_bool(win, "AXMinimized", true).map_err(|e| failed(e, app, "minimizing"))?;
        }
        WindowOp::Restore => {
            leave_full_screen(win, app)?;
            if ffi::copy_bool(win, "AXMinimized") == Some(true) {
                ffi::set_bool(win, "AXMinimized", false)
                    .map_err(|e| failed(e, app, "restoring"))?;
            }
        }
        WindowOp::Fullscreen(on) => {
            ffi::set_bool(win, "AXFullScreen", on).map_err(|e| failed(e, app, "full screen"))?;
        }
        WindowOp::Close => {
            let button = ffi::copy_single_element(win, "AXCloseButton")
                .ok_or_else(|| Error::ActionFailed("this window has no close button".into()))?;
            // An app asking to save first answers once its dialog is shown
            // (or, if the dialog is app-modal, only once it closes).
            ffi::perform_action(button.as_ref(), "AXPress")
                .map_err(|e| failed(e, app, "closing"))?;
        }
        WindowOp::ToDesktop(_) => {
            return Err(Error::Unsupported(
                "macOS has no public way to move windows between Spaces".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_resize_is_reported() {
        let before = Rect::new(0.0, 25.0, 800.0, 600.0);
        let want = Rect::new(100.0, 100.0, 1000.0, 700.0);
        // Moved, but the size stayed.
        let after = Rect::new(100.0, 100.0, 800.0, 600.0);
        let msg = unapplied(Some(before), Some(after), want).unwrap();
        assert!(msg.contains("could not be resized"), "{msg}");
        // Resized, but not moved.
        let after = Rect::new(0.0, 25.0, 1000.0, 700.0);
        let msg = unapplied(Some(before), Some(after), want).unwrap();
        assert!(msg.contains("could not be moved"), "{msg}");
        // Neither.
        let msg = unapplied(Some(before), Some(before), want).unwrap();
        assert!(msg.contains("moved or resized"), "{msg}");
    }

    #[test]
    fn constrained_or_unchanged_parts_are_fine() {
        let before = Rect::new(0.0, 25.0, 800.0, 600.0);
        // Kept below the menu bar and sized in steps: both changed.
        let want = Rect::new(200.0, 0.0, 1000.0, 700.0);
        let after = Rect::new(200.0, 25.0, 994.0, 693.0);
        assert_eq!(unapplied(Some(before), Some(after), want), None);
        // Only a resize asked: the position never changing is fine.
        let want = Rect::new(0.0, 25.0, 900.0, 600.0);
        let after = Rect::new(0.0, 25.0, 900.0, 600.0);
        assert_eq!(unapplied(Some(before), Some(after), want), None);
        // Unknown frames: nothing to say.
        assert_eq!(unapplied(None, Some(after), want), None);
    }
}
