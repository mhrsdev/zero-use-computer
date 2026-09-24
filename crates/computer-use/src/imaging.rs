//! Screenshot scaling/encoding and the mapping between screenshot pixels
//! (what the model sees) and screen coordinates (what input APIs take).

use base64::Engine as _;
use image::{DynamicImage, ImageFormat as ImgFormat, RgbaImage, imageops::FilterType};

use crate::config::{ImageFormat, ScreenshotConfig};
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

pub fn encode(capture: &Capture, cfg: &ScreenshotConfig) -> Result<(EncodedImage, CoordMap)> {
    let img = RgbaImage::from_raw(capture.width, capture.height, capture.rgba.clone())
        .ok_or_else(|| Error::Internal("capture buffer size mismatch".into()))?;
    let (w, h) = fit(capture.width, capture.height, cfg.max_dimension.max(64));
    let img = if (w, h) != (capture.width, capture.height) {
        image::imageops::resize(&img, w, h, FilterType::Triangle)
    } else {
        img
    };
    let rgb = DynamicImage::ImageRgba8(img).into_rgb8();

    let mut data = Vec::new();
    let mime = match cfg.format {
        ImageFormat::Png => {
            rgb.write_to(&mut std::io::Cursor::new(&mut data), ImgFormat::Png)
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
            bounds: capture.bounds,
            width: w,
            height: h,
        },
    ))
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
        let (img, map) = encode(&cap, &cfg).unwrap();
        assert_eq!((img.width, img.height), (100, 50));
        assert_eq!(&img.data[1..4], b"PNG");
        assert_eq!(map.to_screen(50.0, 25.0).unwrap(), Point::new(200.0, 100.0));

        let cfg = ScreenshotConfig {
            format: ImageFormat::Jpeg,
            ..cfg
        };
        let (img, _) = encode(&cap, &cfg).unwrap();
        assert_eq!(img.mime, "image/jpeg");
        assert_eq!(&img.data[..2], &[0xFF, 0xD8]);
    }
}
