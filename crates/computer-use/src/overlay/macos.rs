//! macOS overlay: borderless, transparent `NSWindow`s at screen-saver level
//! that ignore mouse events, join every Space, never take focus (the helper
//! is an accessory app with no Dock icon) and have `sharingType = none`, so
//! screen captures leave them out. Confirmations use a native `NSAlert` run
//! as a modal *session* polled from the helper's loop (never `runModal`,
//! which would stall it), so the helper keeps following the engine and can
//! take the alert down again; the app that had the focus gets it back. The
//! window server removes the windows if the helper process dies.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication, NSApplicationActivationOptions,
    NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSEventMask, NSImage,
    NSImageScaling, NSImageView, NSModalResponseContinue, NSModalSession, NSRunningApplication,
    NSScreen, NSScreenSaverWindowLevel, NSWindow, NSWindowCollectionBehavior, NSWindowSharingType,
    NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{NSData, NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize, NSString};
use tiny_skia::Pixmap;

use super::helper::{Ask, Layer, Surface, SurfaceEvent};
use crate::keys::KeyCombo;
use crate::types::Rect;

// Carbon's hot key API: the system reports this one key combination to us,
// and nothing else (no Accessibility or Input Monitoring permission needed).
#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

type EventHandler = extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn InstallEventHandler(
        target: *mut c_void,
        handler: EventHandler,
        num_types: u32,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut *mut c_void,
    ) -> i32;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyID,
        target: *mut c_void,
        options: u32,
        out_ref: *mut *mut c_void,
    ) -> i32;
    fn UnregisterEventHotKey(hot_key: *mut c_void) -> i32;
    fn GetEventKind(event: *mut c_void) -> u32;
}

const K_EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const K_EVENT_HOT_KEY_PRESSED: u32 = 5;
const K_EVENT_HOT_KEY_RELEASED: u32 = 6;

/// Set by the hot key handler, read by `pump`.
static HOTKEY_HIT: AtomicBool = AtomicBool::new(false);
/// The hot key is held down (a repeat is not a new press).
static HOTKEY_DOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn on_hotkey(_next: *mut c_void, event: *mut c_void, _data: *mut c_void) -> i32 {
    // SAFETY: `event` is the event this handler was called for.
    match unsafe { GetEventKind(event) } {
        K_EVENT_HOT_KEY_RELEASED => HOTKEY_DOWN.store(false, Ordering::SeqCst),
        _ => {
            if !HOTKEY_DOWN.swap(true, Ordering::SeqCst) {
                HOTKEY_HIT.store(true, Ordering::SeqCst);
            }
        }
    }
    0 // noErr
}

/// A confirmation on screen: its alert, modal session and the app that
/// had the focus before.
struct Pending {
    id: u64,
    alert: Retained<NSAlert>,
    session: NSModalSession,
    previous: Option<Retained<NSRunningApplication>>,
}

struct Win {
    window: Retained<NSWindow>,
    view: Retained<NSImageView>,
    /// Size in points.
    size: (f64, f64),
    visible: bool,
}

pub struct MacSurface {
    mtm: MainThreadMarker,
    app: Retained<NSApplication>,
    /// Height of the primary screen (Cocoa's y axis points up from its bottom).
    primary_h: f64,
    screen: Rect,
    /// Height of the menu bar (and notch) at the top of the main screen.
    top_inset: f64,
    scale: f64,
    layers: HashMap<Layer, Win>,
    hidden: bool,
    /// Current fade level, applied to every window.
    opacity: f32,
    answers: Vec<(u64, bool)>,
    /// The registered stop key, and whether the handler is installed.
    hotkey: Option<*mut c_void>,
    handler: bool,
    pending: Option<Pending>,
}

impl MacSurface {
    pub fn open() -> Result<Self, String> {
        let mtm = MainThreadMarker::new().ok_or("the overlay must run on the main thread")?;
        let app = NSApplication::sharedApplication(mtm);
        // No Dock icon, never the active app unless asking the user.
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        app.finishLaunching();
        let screens = NSScreen::screens(mtm);
        let primary = screens.firstObject().ok_or("no screen")?;
        let frame = primary.frame();
        let visible = primary.visibleFrame();
        let top_inset = ((frame.origin.y + frame.size.height)
            - (visible.origin.y + visible.size.height))
            .max(0.0);
        let scale = NSScreen::mainScreen(mtm)
            .map(|s| s.backingScaleFactor())
            .unwrap_or(1.0)
            .max(1.0);
        Ok(Self {
            mtm,
            app,
            primary_h: frame.size.height,
            screen: Rect::new(0.0, 0.0, frame.size.width, frame.size.height),
            top_inset,
            scale,
            layers: HashMap::new(),
            hidden: false,
            opacity: 1.0,
            answers: Vec::new(),
            hotkey: None,
            handler: false,
            pending: None,
        })
    }

    /// Every screen, in top-left screen units.
    fn screens(&self) -> Vec<Rect> {
        let screens = NSScreen::screens(self.mtm);
        (0..screens.count())
            .map(|i| screens.objectAtIndex(i))
            .map(|s| {
                let f = s.frame();
                Rect::new(
                    f.origin.x,
                    self.primary_h - f.origin.y - f.size.height,
                    f.size.width,
                    f.size.height,
                )
            })
            .collect()
    }

    /// Close the confirmation (if any) and give the focus back.
    fn end_confirm(&mut self) {
        let Some(p) = self.pending.take() else {
            return;
        };
        // SAFETY: `session` was begun for this alert and not ended yet.
        unsafe { self.app.endModalSession(p.session) };
        p.alert.window().orderOut(None);
        // Hand the focus back to the app the user was in, unless they have
        // moved on to another one meanwhile.
        if self.app.isActive()
            && let Some(prev) = p.previous
        {
            prev.activateWithOptions(NSApplicationActivationOptions::empty());
        }
    }

    /// Top-left screen units → Cocoa frame.
    fn frame(&self, x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, self.primary_h - y - h), NSSize::new(w, h))
    }

    fn image(img: &Pixmap, size: (f64, f64)) -> Option<Retained<NSImage>> {
        let png = img.encode_png().ok()?;
        let data = NSData::with_bytes(&png);
        let image = NSImage::initWithData(NSImage::alloc(), &data)?;
        image.setSize(NSSize::new(size.0, size.1));
        Some(image)
    }

    fn create(&self, rect: NSRect, image: &NSImage) -> Win {
        // SAFETY: a borderless window created and configured on the main thread.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                self.mtm.alloc(),
                rect,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: we keep our own reference; closing must not free it.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(false);
        window.setIgnoresMouseEvents(true);
        window.setLevel(NSScreenSaverWindowLevel);
        // Left out of screenshots and screen recordings.
        window.setSharingType(NSWindowSharingType::None);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        window.setAlphaValue(f64::from(self.opacity));
        let view = NSImageView::imageViewWithImage(image, self.mtm);
        view.setImageScaling(NSImageScaling::ScaleAxesIndependently);
        window.setContentView(Some(&view));
        Win {
            window,
            view,
            size: (rect.size.width, rect.size.height),
            visible: false,
        }
    }
}

impl Surface for MacSurface {
    fn excluded_from_capture(&self) -> bool {
        true
    }

    fn screen(&self) -> Rect {
        self.screen
    }

    fn screen_at(&self, x: f64, y: f64) -> Rect {
        self.screens()
            .into_iter()
            .find(|r| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
            .unwrap_or(self.screen)
    }

    fn top_inset(&self) -> f64 {
        self.top_inset
    }

    fn render_scale(&self) -> f32 {
        self.scale as f32
    }

    fn px_per_unit(&self) -> f32 {
        self.scale as f32
    }

    fn show(&mut self, layer: Layer, img: &Pixmap, x: f64, y: f64) {
        let size = (
            f64::from(img.width()) / self.scale,
            f64::from(img.height()) / self.scale,
        );
        let Some(image) = Self::image(img, size) else {
            return;
        };
        let rect = self.frame(x, y, size.0, size.1);
        match self.layers.get_mut(&layer) {
            Some(w) => {
                w.window.setFrame_display(rect, true);
                w.view.setImage(Some(&image));
                w.size = size;
            }
            None => {
                let w = self.create(rect, &image);
                self.layers.insert(layer, w);
            }
        }
        if let Some(w) = self.layers.get_mut(&layer) {
            w.visible = true;
            if !self.hidden {
                w.window.orderFrontRegardless();
            }
        }
    }

    fn move_to(&mut self, layer: Layer, x: f64, y: f64) {
        if let Some(w) = self.layers.get(&layer) {
            let origin = NSPoint::new(x, self.primary_h - y - w.size.1);
            w.window.setFrameOrigin(origin);
        }
    }

    fn hide(&mut self, layer: Layer) {
        if let Some(w) = self.layers.get_mut(&layer) {
            w.visible = false;
            w.window.orderOut(None);
        }
    }

    fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
        for w in self.layers.values().filter(|w| w.visible) {
            if hidden {
                w.window.orderOut(None);
            } else {
                w.window.orderFrontRegardless();
            }
        }
    }

    fn set_opacity(&mut self, opacity: f32) -> bool {
        self.opacity = opacity;
        for w in self.layers.values() {
            w.window.setAlphaValue(f64::from(opacity));
        }
        true
    }

    fn confirm(&mut self, id: u64, ask: &Ask) {
        self.end_confirm();
        let alert = NSAlert::new(self.mtm);
        alert.setMessageText(&NSString::from_str(&ask.title));
        alert.setInformativeText(&NSString::from_str(&ask.message));
        alert.addButtonWithTitle(&NSString::from_str(&ask.allow));
        alert.addButtonWithTitle(&NSString::from_str(&ask.deny));
        alert.setAlertStyle(NSAlertStyle::Critical);
        alert.layout();
        let window = alert.window();
        // Above the overlay, on every Space, and out of screenshots.
        window.setLevel(NSScreenSaverWindowLevel + 1);
        window.setSharingType(NSWindowSharingType::None);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        let me = std::process::id() as libc::pid_t;
        let previous = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .filter(|a| a.processIdentifier() != me);
        // Bring the question in front of the user (the focus goes back
        // once it is answered or withdrawn).
        #[allow(deprecated)]
        self.app.activateIgnoringOtherApps(true);
        // A modal session, run a step at a time from `pump`: the helper
        // keeps reading the engine, animating and watching its parent.
        let session = self.app.beginModalSessionForWindow(&window);
        if session.is_null() {
            self.answers.push((id, false));
            return;
        }
        window.makeKeyAndOrderFront(None);
        self.pending = Some(Pending {
            id,
            alert,
            session,
            previous,
        });
    }

    fn dismiss(&mut self) {
        self.end_confirm();
    }

    fn set_hotkey(&mut self, combo: Option<KeyCombo>) -> bool {
        if let Some(r) = self.hotkey.take() {
            // SAFETY: unregistering the hot key we registered.
            unsafe { UnregisterEventHotKey(r) };
        }
        let Some(combo) = combo else {
            return false;
        };
        let Some((code, _)) = crate::macos::cg::keycode(combo.key) else {
            return false;
        };
        // SAFETY: Carbon calls on the main thread with valid arguments; the
        // handler is a plain function that only sets a flag.
        unsafe {
            let target = GetApplicationEventTarget();
            if !self.handler {
                let specs =
                    [K_EVENT_HOT_KEY_PRESSED, K_EVENT_HOT_KEY_RELEASED].map(|kind| EventTypeSpec {
                        event_class: K_EVENT_CLASS_KEYBOARD,
                        event_kind: kind,
                    });
                let mut r = std::ptr::null_mut();
                self.handler = InstallEventHandler(
                    target,
                    on_hotkey,
                    specs.len() as u32,
                    specs.as_ptr(),
                    std::ptr::null_mut(),
                    &mut r,
                ) == 0;
            }
            let m = combo.modifiers;
            let mut mods = 0u32;
            for (on, bit) in [
                (m.meta, 0x0100u32), // cmdKey
                (m.shift, 0x0200),   // shiftKey
                (m.alt, 0x0800),     // optionKey
                (m.ctrl, 0x1000),    // controlKey
            ] {
                if on {
                    mods |= bit;
                }
            }
            let id = EventHotKeyID {
                signature: u32::from_be_bytes(*b"ZSTP"),
                id: 1,
            };
            let mut r = std::ptr::null_mut();
            if self.handler
                && RegisterEventHotKey(u32::from(code), mods, id, target, 0, &mut r) == 0
            {
                self.hotkey = Some(r);
            }
        }
        self.hotkey.is_some()
    }

    fn pump(&mut self) -> Vec<SurfaceEvent> {
        if let Some(p) = &self.pending {
            // SAFETY: a session begun in `confirm` and not ended yet.
            let r = unsafe { self.app.runModalSession(p.session) };
            if r != NSModalResponseContinue {
                let (id, ok) = (p.id, r == NSAlertFirstButtonReturn);
                self.end_confirm();
                self.answers.push((id, ok));
            }
        }
        let past = NSDate::distantPast();
        // SAFETY: reading a constant Foundation string.
        let mode = unsafe { NSDefaultRunLoopMode };
        while let Some(event) = self.app.nextEventMatchingMask_untilDate_inMode_dequeue(
            NSEventMask::Any,
            Some(&past),
            mode,
            true,
        ) {
            self.app.sendEvent(&event);
        }
        let mut events: Vec<SurfaceEvent> = std::mem::take(&mut self.answers)
            .into_iter()
            .map(|(id, ok)| SurfaceEvent::Answer(id, ok))
            .collect();
        if HOTKEY_HIT.swap(false, Ordering::SeqCst) {
            events.push(SurfaceEvent::Hotkey);
        }
        events
    }

    fn close(&mut self) {
        self.end_confirm();
        self.set_hotkey(None);
        for (_, w) in self.layers.drain() {
            w.window.orderOut(None);
            w.window.close();
        }
    }
}
