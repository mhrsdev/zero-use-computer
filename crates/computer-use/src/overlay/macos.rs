//! macOS overlay: borderless, transparent `NSWindow`s at screen-saver level
//! that ignore mouse events, join every Space, never take focus (the helper
//! is an accessory app with no Dock icon) and have `sharingType = none`, so
//! screen captures leave them out. The window server removes the windows if
//! the helper process dies.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use objc2::rc::{Retained, autoreleasepool};
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSEventMask,
    NSImage, NSImageScaling, NSImageView, NSScreen, NSScreenSaverWindowLevel, NSWindow,
    NSWindowCollectionBehavior, NSWindowSharingType, NSWindowStyleMask,
};
use objc2_foundation::{NSData, NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize};
use tiny_skia::Pixmap;

use super::helper::{Layer, Surface, SurfaceEvent};
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

/// The helper's input closed, or it was told to quit.
static INPUT_CLOSED: AtomicBool = AtomicBool::new(false);

/// How long the helper may outlive its input or its parent.
const WATCHDOG_GRACE: Duration = Duration::from_secs(3);

/// Tell the watchdog the helper's input is closed (or it was told to quit).
pub fn input_closed() {
    INPUT_CLOSED.store(true, Ordering::SeqCst);
}

/// End the process once its input has been closed, or `parent` has been
/// gone, for [`WATCHDOG_GRACE`]. The main loop normally fades out and exits
/// well before, but an AppKit call can block it, and an orphaned overlay
/// would stay on screen (and keep the stop key) for good.
pub fn start_watchdog(parent: Option<u32>) {
    let spawned = std::thread::Builder::new()
        .name("overlay-watchdog".into())
        .spawn(move || {
            let mut since: Option<Instant> = None;
            loop {
                std::thread::sleep(Duration::from_millis(250));
                let orphaned = INPUT_CLOSED.load(Ordering::SeqCst)
                    || parent.is_some_and(|p| !super::process_alive(p));
                if !orphaned {
                    since = None;
                } else if since.get_or_insert_with(Instant::now).elapsed() >= WATCHDOG_GRACE {
                    std::process::exit(0);
                }
            }
        });
    if let Err(e) = spawned {
        eprintln!("overlay: no watchdog ({e})");
    }
}

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

struct Win {
    window: Retained<NSWindow>,
    view: Retained<NSImageView>,
    /// Top-left corner, in top-left screen units.
    pos: (f64, f64),
    /// Size in points.
    size: (f64, f64),
    visible: bool,
}

/// The primary screen's geometry, as the overlay draws on it.
#[derive(Clone, Copy, PartialEq)]
struct Geometry {
    /// Height of the primary screen (Cocoa's y axis points up from its bottom).
    primary_h: f64,
    screen: Rect,
    /// Height of the menu bar (and notch) at the top of the primary screen.
    top_inset: f64,
    scale: f64,
}

impl Geometry {
    fn read(mtm: MainThreadMarker) -> Option<Self> {
        let screens = NSScreen::screens(mtm);
        let primary = screens.firstObject()?;
        let frame = primary.frame();
        let visible = primary.visibleFrame();
        let top_inset = ((frame.origin.y + frame.size.height)
            - (visible.origin.y + visible.size.height))
            .max(0.0);
        Some(Self {
            primary_h: frame.size.height,
            screen: Rect::new(0.0, 0.0, frame.size.width, frame.size.height),
            top_inset,
            scale: primary.backingScaleFactor().max(1.0),
        })
    }
}

/// How often the screen geometry is read again (displays added, removed,
/// rearranged or rescaled).
const GEOMETRY_EVERY: Duration = Duration::from_secs(1);

pub struct MacSurface {
    mtm: MainThreadMarker,
    app: Retained<NSApplication>,
    geo: Geometry,
    geo_at: Instant,
    layers: HashMap<Layer, Win>,
    hidden: bool,
    /// Current fade level, applied to every window.
    opacity: f32,
    /// The registered stop key, and whether the handler is installed.
    hotkey: Option<*mut c_void>,
    handler: bool,
}

impl MacSurface {
    pub fn open() -> Result<Self, String> {
        let mtm = MainThreadMarker::new().ok_or("the overlay must run on the main thread")?;
        autoreleasepool(|_| {
            let app = NSApplication::sharedApplication(mtm);
            // No Dock icon, never the active app.
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            app.finishLaunching();
            let geo = Geometry::read(mtm).ok_or("no screen")?;
            Ok(Self {
                mtm,
                app,
                geo,
                geo_at: Instant::now(),
                layers: HashMap::new(),
                hidden: false,
                opacity: 1.0,
                hotkey: None,
                handler: false,
            })
        })
    }

    /// Top-left screen units → Cocoa frame.
    fn frame(&self, x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect::new(
            NSPoint::new(x, self.geo.primary_h - y - h),
            NSSize::new(w, h),
        )
    }

    /// Read the screen geometry again; when the primary screen's height
    /// changed, put the windows back where they belong (Cocoa places them
    /// from the bottom of that screen).
    fn refresh_geometry(&mut self) {
        let Some(geo) = Geometry::read(self.mtm) else {
            return;
        };
        if geo == self.geo {
            return;
        }
        let moved = geo.primary_h != self.geo.primary_h;
        self.geo = geo;
        if moved {
            for w in self.layers.values() {
                let origin = NSPoint::new(w.pos.0, geo.primary_h - w.pos.1 - w.size.1);
                w.window.setFrameOrigin(origin);
            }
        }
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
            pos: (0.0, 0.0),
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
        self.geo.screen
    }

    fn top_inset(&self) -> f64 {
        self.geo.top_inset
    }

    fn render_scale(&self) -> f32 {
        self.geo.scale as f32
    }

    fn px_per_unit(&self) -> f32 {
        self.geo.scale as f32
    }

    // Every method that calls AppKit drains its own autorelease pool: the
    // helper's loop runs for hours, and nothing else would ever drain one.

    fn show(&mut self, layer: Layer, img: &Pixmap, x: f64, y: f64) {
        autoreleasepool(|_| {
            let size = (
                f64::from(img.width()) / self.geo.scale,
                f64::from(img.height()) / self.geo.scale,
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
                w.pos = (x, y);
                w.visible = true;
                if !self.hidden {
                    w.window.orderFrontRegardless();
                }
            }
        })
    }

    fn move_to(&mut self, layer: Layer, x: f64, y: f64) {
        let primary_h = self.geo.primary_h;
        if let Some(w) = self.layers.get_mut(&layer) {
            w.pos = (x, y);
            let origin = NSPoint::new(x, primary_h - y - w.size.1);
            autoreleasepool(|_| w.window.setFrameOrigin(origin));
        }
    }

    fn hide(&mut self, layer: Layer) {
        if let Some(w) = self.layers.get_mut(&layer) {
            w.visible = false;
            autoreleasepool(|_| w.window.orderOut(None));
        }
    }

    fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
        autoreleasepool(|_| {
            for w in self.layers.values().filter(|w| w.visible) {
                if hidden {
                    w.window.orderOut(None);
                } else {
                    w.window.orderFrontRegardless();
                }
            }
        })
    }

    fn set_opacity(&mut self, opacity: f32) -> bool {
        self.opacity = opacity;
        autoreleasepool(|_| {
            for w in self.layers.values() {
                w.window.setAlphaValue(f64::from(opacity));
            }
        });
        true
    }

    fn set_hotkey(&mut self, combo: Option<KeyCombo>) -> bool {
        if let Some(r) = self.hotkey.take() {
            // SAFETY: unregistering the hot key we registered.
            unsafe { UnregisterEventHotKey(r) };
        }
        // The old key's release (if it is held) never arrives now.
        HOTKEY_DOWN.store(false, Ordering::SeqCst);
        let Some(combo) = combo else {
            return false;
        };
        let Some((code, _)) = crate::macos::cg::keycode(combo.key) else {
            return false;
        };
        // SAFETY: Carbon calls on the main thread with valid arguments; the
        // handler is a plain function that only sets flags.
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
        autoreleasepool(|_| {
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
            if self.geo_at.elapsed() >= GEOMETRY_EVERY {
                self.geo_at = Instant::now();
                self.refresh_geometry();
            }
        });
        let mut events = Vec::new();
        if HOTKEY_HIT.swap(false, Ordering::SeqCst) {
            events.push(SurfaceEvent::Hotkey);
        }
        events
    }

    fn close(&mut self) {
        self.set_hotkey(None);
        autoreleasepool(|_| {
            for (_, w) in self.layers.drain() {
                w.window.orderOut(None);
                w.window.close();
            }
        })
    }
}
