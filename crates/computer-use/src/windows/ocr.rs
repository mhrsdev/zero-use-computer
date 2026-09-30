//! Text recognition with Windows' own OCR (`Windows.Media.Ocr`), which
//! reads the languages the user has installed. It runs on a thread of its
//! own (a multithreaded apartment) so the WinRT call never waits on the UI
//! Automation thread.

use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::HSTRING;

use crate::error::{Error, Result};
use crate::types::{Capture, OcrLine};

fn wine(e: windows::core::Error) -> Error {
    Error::Platform(format!("Windows OCR: {e}"))
}

/// A recognised line: text and (x, y, width, height) in image pixels.
type PixelLine = (String, [f64; 4]);

fn recognize_pixels(
    bgra: Vec<u8>,
    width: u32,
    height: u32,
    languages: &[String],
) -> Result<Vec<PixelLine>> {
    // SAFETY: joins (or creates) this thread's multithreaded apartment.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let engine = match languages.first() {
        Some(tag) => OcrEngine::TryCreateFromLanguage(
            &Language::CreateLanguage(&HSTRING::from(tag.as_str())).map_err(wine)?,
        ),
        None => OcrEngine::TryCreateFromUserProfileLanguages(),
    }
    .map_err(|e| {
        Error::Unsupported(format!(
            "Windows OCR has no language to read with ({e}); add an OCR language in Settings > Time & language"
        ))
    })?;
    let buffer = CryptographicBuffer::CreateFromByteArray(&bgra).map_err(wine)?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        width as i32,
        height as i32,
    )
    .map_err(wine)?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(wine)?
        .join()
        .map_err(wine)?;
    let lines = result.Lines().map_err(wine)?;
    let mut out = Vec::new();
    for i in 0..lines.Size().map_err(wine)? {
        let line = lines.GetAt(i).map_err(wine)?;
        let text = line.Text().map_err(wine)?.to_string_lossy();
        let words = line.Words().map_err(wine)?;
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, 0.0f64, 0.0f64);
        for j in 0..words.Size().map_err(wine)? {
            let r = words.GetAt(j).map_err(wine)?.BoundingRect().map_err(wine)?;
            x0 = x0.min(f64::from(r.X));
            y0 = y0.min(f64::from(r.Y));
            x1 = x1.max(f64::from(r.X + r.Width));
            y1 = y1.max(f64::from(r.Y + r.Height));
        }
        if x1 > x0 && !text.trim().is_empty() {
            out.push((text, [x0, y0, x1 - x0, y1 - y0]));
        }
    }
    Ok(out)
}

pub fn recognize(cap: &Capture, languages: &[String]) -> Result<Vec<OcrLine>> {
    // The engine takes images up to a size; shrink larger ones.
    let max = OcrEngine::MaxImageDimension().unwrap_or(2600).max(64);
    let longest = cap.width.max(cap.height).max(1);
    let (w, h, rgba) = if longest > max {
        let (w, h) = crate::imaging::fit(cap.width, cap.height, max);
        let img = image::RgbaImage::from_raw(cap.width, cap.height, cap.rgba.clone())
            .ok_or_else(|| Error::Internal("capture buffer size mismatch".into()))?;
        let small = image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle);
        (w, h, small.into_raw())
    } else {
        (cap.width, cap.height, cap.rgba.clone())
    };
    let mut bgra = rgba;
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
        px[3] = 255;
    }
    let langs = languages.to_vec();
    let lines = std::thread::Builder::new()
        .name("ocr".into())
        .spawn(move || recognize_pixels(bgra, w, h, &langs))
        .map_err(|e| Error::Platform(format!("OCR thread: {e}")))?
        .join()
        .map_err(|_| Error::Platform("the OCR thread failed".into()))??;
    // Back to capture pixels, then to the screen.
    let (fx, fy) = (
        f64::from(cap.width) / f64::from(w),
        f64::from(cap.height) / f64::from(h),
    );
    Ok(lines
        .into_iter()
        .map(|(text, [x, y, lw, lh])| OcrLine {
            text,
            bounds: crate::ocr::to_screen(cap, x * fx, y * fy, lw * fx, lh * fy),
            // Windows OCR doesn't report confidence.
            confidence: 1.0,
        })
        .collect())
}
