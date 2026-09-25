//! Text recognition with macOS's Vision framework (`VNRecognizeTextRequest`,
//! accurate level, with language correction), called through the
//! Objective-C runtime.

use std::ffi::c_void;
use std::sync::Arc;

use core_graphics::base::{kCGImageAlphaPremultipliedLast, kCGRenderingIntentDefault};
use core_graphics::color_space::CGColorSpace;
use core_graphics::data_provider::CGDataProvider;
use core_graphics::image::CGImage;
use foreign_types::ForeignType;
use objc2::encode::{Encode, Encoding};
use objc2::msg_send;
use objc2::rc::{Allocated, Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::{NSArray, NSRect, NSString};

use crate::error::{Error, Result};
use crate::types::{Capture, OcrLine};

#[link(name = "Vision", kind = "framework")]
unsafe extern "C" {}

/// A `CGImageRef` argument, encoded as the method expects.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct CGImageArg(*const c_void);

// SAFETY: a pointer to the opaque CGImage struct, as Vision declares it.
unsafe impl Encode for CGImageArg {
    const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("CGImage", &[]));
}

fn class(name: &std::ffi::CStr) -> Result<&'static AnyClass> {
    AnyClass::get(name).ok_or_else(|| {
        Error::Unsupported("the Vision framework is not available (macOS 10.15+)".into())
    })
}

pub fn recognize(cap: &Capture, languages: &[String]) -> Result<Vec<OcrLine>> {
    let (w, h) = (cap.width as usize, cap.height as usize);
    let provider = CGDataProvider::from_buffer(Arc::new(cap.rgba.clone()));
    let image = CGImage::new(
        w,
        h,
        8,
        32,
        w * 4,
        &CGColorSpace::create_device_rgb(),
        kCGImageAlphaPremultipliedLast,
        &provider,
        false,
        kCGRenderingIntentDefault,
    );
    autoreleasepool(|_| {
        // SAFETY: Vision's documented API, with argument and return types
        // matching its declarations (checked by objc2 in debug builds).
        unsafe {
            let request: Retained<AnyObject> = msg_send![class(c"VNRecognizeTextRequest")?, new];
            // VNRequestTextRecognitionLevelAccurate
            let _: () = msg_send![&*request, setRecognitionLevel: 0isize];
            let _: () = msg_send![&*request, setUsesLanguageCorrection: true];
            if !languages.is_empty() {
                let langs: Vec<Retained<NSString>> =
                    languages.iter().map(|l| NSString::from_str(l)).collect();
                let langs = NSArray::from_retained_slice(&langs);
                let _: () = msg_send![&*request, setRecognitionLanguages: &*langs];
            }
            let options: Retained<AnyObject> = msg_send![class(c"NSDictionary")?, dictionary];
            let handler: Allocated<AnyObject> = msg_send![class(c"VNImageRequestHandler")?, alloc];
            let handler: Retained<AnyObject> = msg_send![
                handler,
                initWithCGImage: CGImageArg(image.as_ptr() as *const c_void),
                options: &*options
            ];
            let requests = NSArray::from_retained_slice(std::slice::from_ref(&request));
            let error: *mut *mut AnyObject = std::ptr::null_mut();
            let ok: bool = msg_send![&*handler, performRequests: &*requests, error: error];
            if !ok {
                return Err(Error::Platform("Vision could not read the image".into()));
            }
            let results: Option<Retained<NSArray<AnyObject>>> = msg_send![&*request, results];
            let mut out = Vec::new();
            let Some(results) = results else {
                return Ok(out);
            };
            for i in 0..results.count() {
                let obs = results.objectAtIndex(i);
                let candidates: Retained<NSArray<AnyObject>> =
                    msg_send![&*obs, topCandidates: 1usize];
                if candidates.count() == 0 {
                    continue;
                }
                let best = candidates.objectAtIndex(0);
                let text: Retained<NSString> = msg_send![&*best, string];
                let confidence: f32 = msg_send![&*best, confidence];
                // Normalised, with the origin at the bottom left.
                let b: NSRect = msg_send![&*obs, boundingBox];
                let (iw, ih) = (w as f64, h as f64);
                out.push(OcrLine {
                    text: text.to_string(),
                    bounds: crate::ocr::to_screen(
                        cap,
                        b.origin.x * iw,
                        (1.0 - b.origin.y - b.size.height) * ih,
                        b.size.width * iw,
                        b.size.height * ih,
                    ),
                    confidence,
                });
            }
            Ok(out)
        }
    })
}
