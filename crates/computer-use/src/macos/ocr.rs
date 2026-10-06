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

use super::ffi::nsstring_text;
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

/// The recognition languages for the user's `preferred` ones (BCP 47, as
/// `NSLocale.preferredLanguages` gives them), among those Vision
/// `supported`: each matched exactly, else by language (and script, so
/// "zh-TW" is "zh-Hant"), preferring the same region. Returns the chosen
/// ones, in order, and the preferred ones Vision can't read.
fn pick_languages(preferred: &[String], supported: &[String]) -> (Vec<String>, Vec<String>) {
    let parts = |tag: &str| -> Vec<String> {
        tag.split(['-', '_'])
            .map(|p| p.to_ascii_lowercase())
            .collect()
    };
    let mut chosen: Vec<String> = Vec::new();
    let mut dropped = Vec::new();
    for want in preferred {
        let w = parts(want);
        let Some(lang) = w.first().filter(|l| !l.is_empty()) else {
            continue;
        };
        let script = w[1..].iter().find(|p| p.len() == 4).cloned().or_else(|| {
            // Chinese without a script: traditional where it is written so.
            (lang == "zh").then(|| {
                let traditional = w[1..]
                    .iter()
                    .any(|p| matches!(p.as_str(), "tw" | "hk" | "mo"));
                if traditional { "hant" } else { "hans" }.to_string()
            })
        });
        let region = w[1..].iter().find(|p| p.len() != 4).cloned();
        let best = supported
            .iter()
            .filter(|s| {
                let s = parts(s);
                s.first() == Some(lang)
                    && script
                        .as_ref()
                        .is_none_or(|sc| !s[1..].iter().any(|p| p.len() == 4) || s.contains(sc))
            })
            .max_by_key(|s| {
                let s = parts(s);
                let same_script = script.as_ref().is_some_and(|sc| s.contains(sc));
                let same_region = region.as_ref().is_some_and(|r| s.contains(r));
                // max_by_key keeps the last of equals: reverse the index.
                (
                    s == w,
                    same_script,
                    same_region,
                    std::cmp::Reverse(supported.iter().position(|x| parts(x) == s)),
                )
            });
        match best {
            Some(s) if !chosen.contains(s) => chosen.push(s.clone()),
            Some(_) => {}
            None => dropped.push(want.clone()),
        }
    }
    (chosen, dropped)
}

/// An `NSArray<NSString>` as strings.
fn strings(array: &NSArray<NSString>) -> Vec<String> {
    (0..array.count())
        .map(|i| nsstring_text(&array.objectAtIndex(i)))
        .collect()
}

/// An `NSError`'s description.
///
/// # Safety
/// `error` is null or a live NSError.
unsafe fn describe(error: *mut AnyObject) -> Option<String> {
    // SAFETY: as the caller promises; `localizedDescription` is NSError's.
    let error = unsafe { error.as_ref() }?;
    let text: Retained<NSString> = unsafe { msg_send![error, localizedDescription] };
    Some(nsstring_text(&text))
}

/// The languages to recognize when none are configured: the user's
/// preferred ones that this request can read. Empty when none can (Vision
/// then reads English).
///
/// # Safety
/// `request` is a live `VNRecognizeTextRequest`.
unsafe fn auto_languages(request: &AnyObject) -> Vec<String> {
    let sel = objc2::sel!(supportedRecognitionLanguagesAndReturnError:);
    // SAFETY: NSLocale's and (macOS 12+, checked) Vision's documented
    // methods, with argument and return types as declared.
    unsafe {
        let responds: bool = msg_send![request, respondsToSelector: sel];
        if !responds {
            return Vec::new();
        }
        let Ok(locale) = class(c"NSLocale") else {
            return Vec::new();
        };
        let preferred: Retained<NSArray<NSString>> = msg_send![locale, preferredLanguages];
        let mut error: *mut AnyObject = std::ptr::null_mut();
        let supported: Option<Retained<NSArray<NSString>>> =
            msg_send![request, supportedRecognitionLanguagesAndReturnError: &mut error];
        let Some(supported) = supported else {
            log::warn!(
                "Vision did not list its languages ({}); reading English",
                describe(error).unwrap_or_default()
            );
            return Vec::new();
        };
        let (chosen, dropped) = pick_languages(&strings(&preferred), &strings(&supported));
        if !dropped.is_empty() {
            log::warn!(
                "text recognition can't read {} (Vision reads {})",
                dropped.join(", "),
                strings(&supported).join(", ")
            );
        }
        chosen
    }
}

pub fn recognize(cap: &Capture, languages: &[String]) -> Result<Vec<OcrLine>> {
    let (w, h) = (cap.width as usize, cap.height as usize);
    // CGImage::new panics on an image it can't make.
    let enough = w
        .checked_mul(h)
        .and_then(|n| n.checked_mul(4))
        .is_some_and(|n| cap.rgba.len() >= n);
    if w == 0 || h == 0 || !enough {
        return Err(Error::Platform(format!(
            "no image to read text from ({w}×{h} with {} bytes)",
            cap.rgba.len()
        )));
    }
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
    if !languages.is_empty() {
        return autoreleasepool(|_| read(cap, &image, languages, false));
    }
    // None configured: the user's own languages, not only English. Should
    // Vision refuse them, its defaults still read the image.
    match autoreleasepool(|_| read(cap, &image, &[], true)) {
        Err(Error::Platform(e)) => {
            log::warn!("{e}; reading again with Vision's default language");
            autoreleasepool(|_| read(cap, &image, &[], false))
        }
        done => done,
    }
}

/// Read the text in `image` (made of `cap`) in `languages`, or (`auto`) in
/// the user's preferred languages, telling them apart (macOS 13+). The
/// caller holds an autorelease pool.
fn read(cap: &Capture, image: &CGImage, languages: &[String], auto: bool) -> Result<Vec<OcrLine>> {
    let (w, h) = (cap.width as usize, cap.height as usize);
    // SAFETY: Vision's documented API, with argument and return types
    // matching its declarations (checked by objc2 in debug builds).
    unsafe {
        let request: Retained<AnyObject> = msg_send![class(c"VNRecognizeTextRequest")?, new];
        // VNRequestTextRecognitionLevelAccurate
        let _: () = msg_send![&*request, setRecognitionLevel: 0isize];
        let _: () = msg_send![&*request, setUsesLanguageCorrection: true];
        let languages = if auto {
            auto_languages(&request)
        } else {
            languages.to_vec()
        };
        if !languages.is_empty() {
            let langs: Vec<Retained<NSString>> =
                languages.iter().map(|l| NSString::from_str(l)).collect();
            let langs = NSArray::from_retained_slice(&langs);
            let _: () = msg_send![&*request, setRecognitionLanguages: &*langs];
        }
        // macOS 13+: also tell the language of each text apart.
        let detect = objc2::sel!(setAutomaticallyDetectsLanguage:);
        let can_detect: bool = msg_send![&*request, respondsToSelector: detect];
        if auto && can_detect {
            let _: () = msg_send![&*request, setAutomaticallyDetectsLanguage: true];
        }
        let options: Retained<AnyObject> = msg_send![class(c"NSDictionary")?, dictionary];
        let handler: Allocated<AnyObject> = msg_send![class(c"VNImageRequestHandler")?, alloc];
        let handler: Retained<AnyObject> = msg_send![
            handler,
            initWithCGImage: CGImageArg(image.as_ptr() as *const c_void),
            options: &*options
        ];
        let requests = NSArray::from_retained_slice(std::slice::from_ref(&request));
        let mut error: *mut AnyObject = std::ptr::null_mut();
        let ok: bool = msg_send![&*handler, performRequests: &*requests, error: &mut error];
        if !ok {
            let why = describe(error)
                .map(|d| format!(": {d}"))
                .unwrap_or_default();
            return Err(Error::Platform(format!(
                "Vision could not read the image{why}"
            )));
        }
        let results: Option<Retained<NSArray<AnyObject>>> = msg_send![&*request, results];
        let mut out = Vec::new();
        let Some(results) = results else {
            return Ok(out);
        };
        for i in 0..results.count() {
            let obs = results.objectAtIndex(i);
            let candidates: Retained<NSArray<AnyObject>> = msg_send![&*obs, topCandidates: 1usize];
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
                text: nsstring_text(&text),
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
}

#[cfg(test)]
mod tests {
    use super::pick_languages;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// What Vision's accurate level reads on macOS 13.
    fn vision() -> Vec<String> {
        v(&[
            "en-US", "fr-FR", "it-IT", "de-DE", "es-ES", "pt-BR", "zh-Hans", "zh-Hant", "yue-Hans",
            "yue-Hant", "ko-KR", "ja-JP", "ru-RU", "uk-UA",
        ])
    }

    #[test]
    fn short_codes_match_by_language() {
        let (chosen, dropped) = pick_languages(&v(&["de", "fr", "ja", "ko", "ru"]), &vision());
        assert_eq!(chosen, v(&["de-DE", "fr-FR", "ja-JP", "ko-KR", "ru-RU"]));
        assert!(dropped.is_empty());
    }

    #[test]
    fn chinese_matches_by_script() {
        let (chosen, _) = pick_languages(&v(&["zh-Hans-CN", "en-GB"]), &vision());
        assert_eq!(chosen, v(&["zh-Hans", "en-US"]));
        let (chosen, _) = pick_languages(&v(&["zh-TW"]), &vision());
        assert_eq!(chosen, v(&["zh-Hant"]));
        let (chosen, _) = pick_languages(&v(&["zh"]), &vision());
        assert_eq!(chosen, v(&["zh-Hans"]));
        let (chosen, _) = pick_languages(&v(&["zh-Hant-HK"]), &vision());
        assert_eq!(chosen, v(&["zh-Hant"]));
    }

    #[test]
    fn exact_and_regional_matches_win() {
        let supported = v(&["en-US", "en-GB", "pt-BR", "pt-PT"]);
        let (chosen, _) = pick_languages(&v(&["en-GB", "pt-PT", "en_US"]), &supported);
        assert_eq!(chosen, v(&["en-GB", "pt-PT", "en-US"]));
        // Without a region: the first supported one.
        let (chosen, _) = pick_languages(&v(&["pt"]), &supported);
        assert_eq!(chosen, v(&["pt-BR"]));
    }

    #[test]
    fn unsupported_languages_are_dropped() {
        let (chosen, dropped) = pick_languages(&v(&["fa-IR", "en", "he", "en-US"]), &vision());
        assert_eq!(chosen, v(&["en-US"]));
        assert_eq!(dropped, v(&["fa-IR", "he"]));
        let (chosen, dropped) = pick_languages(&[], &vision());
        assert!(chosen.is_empty() && dropped.is_empty());
    }
}
