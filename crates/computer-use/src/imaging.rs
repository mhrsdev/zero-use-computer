//! Screenshot scaling/encoding and the mapping between screenshot pixels
//! (what the model sees) and screen coordinates (what input APIs take).

use base64::Engine as _;
use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::{ExtendedColorType, ImageEncoder, RgbaImage, imageops::FilterType};

use crate::config::{ImageFormat, PngCompression, ResizeFilter, ScreenshotConfig};
use crate::error::{Error, Result};
use crate::types::{Capture, Point, Rect};

#[derive(Debug, Clone)]
pub struct EncodedImage {
    pub mime: &'static str,
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl EncodedImage {
    pub fn base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(&self.data)
    }
}

/// Maps screenshot pixel coordinates to screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoordMap {
    /// Screen rect covered by the screenshot.
    pub bounds: Rect,
    /// Size of the image the model received.
    pub width: u32,
    pub height: u32,
}

impl CoordMap {
    pub fn to_screen(&self, x: f64, y: f64) -> Result<Point> {
        if !(x.is_finite() && y.is_finite())
            || x < 0.0
            || y < 0.0
            || x > f64::from(self.width)
            || y > f64::from(self.height)
        {
            return Err(Error::InvalidArgs(format!(
                "({x}, {y}) is outside the {}x{} screenshot",
                self.width, self.height
            )));
        }
        Ok(Point::new(
            self.bounds.x + x * self.bounds.width / f64::from(self.width),
            self.bounds.y + y * self.bounds.height / f64::from(self.height),
        ))
    }

    pub fn to_image(&self, p: Point) -> (f64, f64) {
        (
            (p.x - self.bounds.x) * f64::from(self.width) / self.bounds.width,
            (p.y - self.bounds.y) * f64::from(self.height) / self.bounds.height,
        )
    }
}

/// Target size that fits `max` on the longest edge (never upscales).
pub fn fit(width: u32, height: u32, max: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= max || longest == 0 {
        return (width, height);
    }
    let scale = f64::from(max) / f64::from(longest);
    (
        ((f64::from(width) * scale).round() as u32).max(1),
        ((f64::from(height) * scale).round() as u32).max(1),
    )
}

/// Downscale and encode a capture. Takes the capture by value so the full-size
/// pixel buffer is reused (not copied) and freed as early as possible.
pub fn encode(capture: Capture, cfg: &ScreenshotConfig) -> Result<(EncodedImage, CoordMap)> {
    let Capture {
        width,
        height,
        rgba,
        bounds,
    } = capture;
    let img = RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| Error::Internal("capture buffer size mismatch".into()))?;
    let (w, h) = fit(width, height, cfg.max_dimension.max(64));
    let img = if (w, h) != (width, height) {
        match cfg.resize_filter {
            // Integer area averaging: several times faster than a filtered
            // resize and crisp enough for UI text when shrinking.
            ResizeFilter::Fast => image::imageops::thumbnail(&img, w, h),
            ResizeFilter::Smooth => image::imageops::resize(&img, w, h, FilterType::Triangle),
            ResizeFilter::Sharp => image::imageops::resize(&img, w, h, FilterType::Lanczos3),
        }
    } else {
        img
    };
    // Drop alpha in place (no second full-size buffer).
    let mut raw = img.into_raw();
    let pixels = (w * h) as usize;
    for i in 0..pixels {
        let (s, d) = (i * 4, i * 3);
        raw[d] = raw[s];
        raw[d + 1] = raw[s + 1];
        raw[d + 2] = raw[s + 2];
    }
    raw.truncate(pixels * 3);
    let rgb = image::RgbImage::from_raw(w, h, raw)
        .ok_or_else(|| Error::Internal("rgb buffer size mismatch".into()))?;

    let mut data = Vec::with_capacity((w * h) as usize / 2);
    let mime = match cfg.format {
        ImageFormat::Png => {
            let compression = match cfg.png_compression {
                PngCompression::Fast => CompressionType::Fast,
                PngCompression::Default => CompressionType::Default,
                PngCompression::Best => CompressionType::Best,
            };
            PngEncoder::new_with_quality(&mut data, compression, PngFilter::Adaptive)
                .write_image(rgb.as_raw(), w, h, ExtendedColorType::Rgb8)
                .map_err(|e| Error::Internal(format!("png encode: {e}")))?;
            "image/png"
        }
        ImageFormat::Jpeg => {
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
                &mut data,
                cfg.jpeg_quality.clamp(10, 100),
            );
            enc.encode_image(&rgb)
                .map_err(|e| Error::Internal(format!("jpeg encode: {e}")))?;
            "image/jpeg"
        }
    };
    Ok((
        EncodedImage {
            mime,
            data,
            width: w,
            height: h,
        },
        CoordMap {
            bounds,
            width: w,
            height: h,
        },
    ))
}

/// 3×5 pixel patterns for the digits 0–9 (row-major, top to bottom).
const DIGITS: [[u8; 15]; 10] = [
    [1, 1, 1, 1, 0, 1, 1, 0, 1, 1, 0, 1, 1, 1, 1], // 0
    [0, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 1, 1, 1], // 1
    [1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 0, 0, 1, 1, 1], // 2
    [1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1], // 3
    [1, 0, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 1], // 4
    [1, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 1], // 5
    [1, 1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 1, 1, 1, 1], // 6
    [1, 1, 1, 0, 0, 1, 0, 1, 0, 0, 1, 0, 0, 1, 0], // 7
    [1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1], // 8
    [1, 1, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1], // 9
];

fn put(buf: &mut [u8], w: u32, h: u32, x: i64, y: i64, rgb: [u8; 3]) {
    if x < 0 || y < 0 || x >= i64::from(w) || y >= i64::from(h) {
        return;
    }
    let i = ((y as u32 * w + x as u32) * 4) as usize;
    buf[i] = rgb[0];
    buf[i + 1] = rgb[1];
    buf[i + 2] = rgb[2];
    buf[i + 3] = 255;
}

fn draw_rect_outline(cap: &mut Capture, r: Rect, rgb: [u8; 3]) {
    let (w, h) = (cap.width, cap.height);
    let x0 = r.x as i64;
    let y0 = r.y as i64;
    let x1 = (r.x + r.width) as i64;
    let y1 = (r.y + r.height) as i64;
    for x in x0..=x1 {
        put(&mut cap.rgba, w, h, x, y0, rgb);
        put(&mut cap.rgba, w, h, x, y1, rgb);
    }
    for y in y0..=y1 {
        put(&mut cap.rgba, w, h, x0, y, rgb);
        put(&mut cap.rgba, w, h, x1, y, rgb);
    }
}

fn draw_digit(cap: &mut Capture, digit: usize, x: i64, y: i64, scale: i64, rgb: [u8; 3]) {
    let pat = &DIGITS[digit % 10];
    for row in 0..5i64 {
        for col in 0..3i64 {
            if pat[(row * 3 + col) as usize] == 1 {
                for dy in 0..scale {
                    for dx in 0..scale {
                        put(
                            &mut cap.rgba,
                            cap.width,
                            cap.height,
                            x + col * scale + dx,
                            y + row * scale + dy,
                            rgb,
                        );
                    }
                }
            }
        }
    }
}

/// Draw each element's index over the capture (set-of-marks). `marks` are
/// (index, screen-space bounds); they are mapped into the capture's pixels.
pub fn annotate(cap: &mut Capture, marks: &[(u32, Rect)]) {
    let box_rgb = [255, 40, 40];
    let text_rgb = [255, 255, 0];
    let bg_rgb = [0, 0, 0];
    let scale = 2i64;
    let (box_w, ox, oy) = (cap.bounds.width, cap.bounds.x, cap.bounds.y);
    if box_w <= 0.0 {
        return;
    }
    let sx = f64::from(cap.width) / cap.bounds.width;
    let sy = f64::from(cap.height) / cap.bounds.height;
    for (index, b) in marks {
        // Skip the window root and anything with no real size.
        if b.width < 4.0 || b.height < 4.0 {
            continue;
        }
        let r = Rect::new(
            (b.x - ox) * sx,
            (b.y - oy) * sy,
            b.width * sx,
            b.height * sy,
        );
        draw_rect_outline(cap, r, box_rgb);
        // Label background + digits at the element's top-left.
        let digits: Vec<usize> = index
            .to_string()
            .chars()
            .map(|c| c as usize - '0' as usize)
            .collect();
        let label_w = digits.len() as i64 * (3 * scale + 1) + 2;
        let label_h = 5 * scale + 2;
        let lx = r.x as i64;
        let ly = (r.y as i64 - label_h).max(0);
        for yy in 0..label_h {
            for xx in 0..label_w {
                put(
                    &mut cap.rgba,
                    cap.width,
                    cap.height,
                    lx + xx,
                    ly + yy,
                    bg_rgb,
                );
            }
        }
        let mut cx = lx + 1;
        for d in digits {
            draw_digit(cap, d, cx, ly + 1, scale, text_rgb);
            cx += 3 * scale + 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_aspect() {
        assert_eq!(fit(2560, 1600, 1280), (1280, 800));
        assert_eq!(fit(800, 600, 1280), (800, 600));
        assert_eq!(fit(1000, 3000, 1500), (500, 1500));
    }

    #[test]
    fn coord_map_round_trip() {
        // A retina-style capture: 1600x1200 pixels covering 800x600 points at (100, 50).
        let map = CoordMap {
            bounds: Rect::new(100.0, 50.0, 800.0, 600.0),
            width: 1280,
            height: 960,
        };
        let p = map.to_screen(640.0, 480.0).unwrap();
        assert_eq!(p, Point::new(500.0, 350.0));
        assert_eq!(map.to_image(p), (640.0, 480.0));
        assert!(map.to_screen(2000.0, 1.0).is_err());
        assert!(map.to_screen(f64::NAN, 1.0).is_err());
    }

    #[test]
    fn encodes_and_downscales() {
        let cap = Capture {
            width: 400,
            height: 200,
            rgba: vec![128; 400 * 200 * 4],
            bounds: Rect::new(0.0, 0.0, 400.0, 200.0),
        };
        let cfg = ScreenshotConfig {
            max_dimension: 100,
            ..Default::default()
        };
        let (img, map) = encode(cap.clone(), &cfg).unwrap();
        assert_eq!((img.width, img.height), (100, 50));
        assert_eq!(&img.data[1..4], b"PNG");
        assert_eq!(map.to_screen(50.0, 25.0).unwrap(), Point::new(200.0, 100.0));

        let cfg = ScreenshotConfig {
            format: ImageFormat::Jpeg,
            ..cfg
        };
        let (img, _) = encode(cap, &cfg).unwrap();
        assert_eq!(img.mime, "image/jpeg");
        assert_eq!(&img.data[..2], &[0xFF, 0xD8]);
    }
}
