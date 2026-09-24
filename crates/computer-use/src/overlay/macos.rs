//! macOS overlay: borderless, transparent `NSWindow`s at screen-saver level
//! that ignore mouse events, join every Space, never take focus (the helper
//! is an accessory app with no Dock icon) and have `sharingType = none`, so
//! screen captures leave them out. Confirmations use a native `NSAlert`. The
//! window server removes the windows if the helper process dies.

use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication, NSApplicationActivationPolicy,
    NSBackingStoreType, NSColor, NSEventMask, NSImage, NSImageScaling, NSImageView, NSScreen,
    NSScreenSaverWindowLevel, NSWindow, NSWindowCollectionBehavior, NSWindowSharingType,
    NSWindowStyleMask,
};
use objc2_foundation::{NSData, NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize, NSString};
use tiny_skia::Pixmap;

use super::helper::{Ask, Layer, Surface};
use crate::types::Rect;

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
    answers: Vec<(u64, bool)>,
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
            answers: Vec::new(),
        })
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

    fn confirm(&mut self, id: u64, ask: &Ask) {
        let alert = NSAlert::new(self.mtm);
        alert.setMessageText(&NSString::from_str(&ask.title));
        alert.setInformativeText(&NSString::from_str(&ask.message));
        alert.addButtonWithTitle(&NSString::from_str(&ask.allow));
        alert.addButtonWithTitle(&NSString::from_str(&ask.deny));
        alert.setAlertStyle(NSAlertStyle::Critical);
        // Bring the question in front of the user.
        #[allow(deprecated)]
        self.app.activateIgnoringOtherApps(true);
        let answer = alert.runModal() == NSAlertFirstButtonReturn;
        self.answers.push((id, answer));
    }

    fn pump(&mut self) -> Vec<(u64, bool)> {
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
        std::mem::take(&mut self.answers)
    }

    fn close(&mut self) {
        for (_, w) in self.layers.drain() {
            w.window.orderOut(None);
            w.window.close();
        }
    }
}
