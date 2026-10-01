//! Screenshot scaling/encoding and the mapping between screenshot pixels
//! (what the model sees) and screen coordinates (what input APIs take).

use std::collections::HashMap;

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
    encode_min(capture, cfg, 64)
}

/// [`encode`] with the smallest `max_dimension` it will honour: a whole
/// image is never squeezed below 64 px, but a part must keep its whole
/// image's scale, however small that makes it.
fn encode_min(
    capture: Capture,
    cfg: &ScreenshotConfig,
    min_dimension: u32,
) -> Result<(EncodedImage, CoordMap)> {
    if capture.width == 0 || capture.height == 0 {
        return Err(Error::ActionFailed(
            "the screen capture is empty (0x0); nothing to show".into(),
        ));
    }
    let Capture {
        width,
        height,
        rgba,
        bounds,
    } = capture;
    let img = RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| Error::Internal("capture buffer size mismatch".into()))?;
    let (w, h) = fit(width, height, cfg.max_dimension.max(min_dimension));
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
    if w == 0
        || h == 0
        || !(r.x.is_finite() && r.y.is_finite() && r.width.is_finite() && r.height.is_finite())
    {
        return;
    }
    // Rectangles from accessibility trees can be absurdly large: only walk
    // the part of each edge that is inside the image.
    let clamp = |v: f64| v.clamp(-1.0e9, 1.0e9) as i64;
    let (x0, y0) = (clamp(r.x), clamp(r.y));
    let (x1, y1) = (clamp(r.x + r.width), clamp(r.y + r.height));
    for x in x0.max(0)..=x1.min(i64::from(w) - 1) {
        put(&mut cap.rgba, w, h, x, y0, rgb);
        put(&mut cap.rgba, w, h, x, y1, rgb);
    }
    for y in y0.max(0)..=y1.min(i64::from(h) - 1) {
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

/// Whether a capture is (almost) one colour: what some apps give a
/// background capture when they draw nothing for it. Over 99% of sampled
/// pixels equal the first one.
pub fn uniform(cap: &Capture) -> bool {
    let n = cap.rgba.len() / 4;
    if n == 0 {
        return true;
    }
    let step = (n / 4096).max(1);
    let first = &cap.rgba[..4];
    let (mut same, mut total) = (0usize, 0usize);
    for p in cap.rgba.as_chunks::<4>().0.iter().step_by(step) {
        total += 1;
        if p == first {
            same += 1;
        }
    }
    same * 100 > total * 99
}

/// Cut a pixel rectangle (x, y, width, height) out of a capture.
pub fn crop(cap: &Capture, px: (u32, u32, u32, u32)) -> Capture {
    if cap.width == 0 || cap.height == 0 {
        return cap.clone();
    }
    let x = px.0.min(cap.width - 1);
    let y = px.1.min(cap.height - 1);
    let w = px.2.max(1).min(cap.width - x);
    let h = px.3.max(1).min(cap.height - y);
    let stride = cap.width as usize * 4;
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for row in y..y + h {
        let start = row as usize * stride + x as usize * 4;
        rgba.extend_from_slice(&cap.rgba[start..start + w as usize * 4]);
    }
    let sx = cap.bounds.width / f64::from(cap.width.max(1));
    let sy = cap.bounds.height / f64::from(cap.height.max(1));
    Capture {
        width: w,
        height: h,
        rgba,
        bounds: Rect::new(
            cap.bounds.x + f64::from(x) * sx,
            cap.bounds.y + f64::from(y) * sy,
            f64::from(w) * sx,
            f64::from(h) * sy,
        ),
    }
}

/// Grow a pixel rectangle by `pad` on every side and to at least `min` per
/// side (centred on it), kept inside `width` x `height`.
pub fn widen(
    r: (u32, u32, u32, u32),
    pad: u32,
    min: u32,
    width: u32,
    height: u32,
) -> (u32, u32, u32, u32) {
    let grow = |start: u32, len: u32, limit: u32| -> (u32, u32) {
        let mut a = start.saturating_sub(pad);
        let mut b = (start + len + pad).min(limit);
        let want = min.min(limit);
        if b - a < want {
            let extra = want - (b - a);
            a = a.saturating_sub(extra / 2);
            b = (a + want).min(limit);
            a = b.saturating_sub(want);
        }
        (a, b - a)
    };
    let (x, w) = grow(r.0, r.2, width);
    let (y, h) = grow(r.1, r.3, height);
    (x, y, w, h)
}

/// Encode part of a capture at the scale an earlier full image of it was
/// sent at (`full`), so positions in the part line up with that image.
/// Returns the image and the part's offset in the full image's pixels.
pub fn encode_part(
    cap: &Capture,
    px: (u32, u32, u32, u32),
    full: &CoordMap,
    cfg: &ScreenshotConfig,
) -> Result<(EncodedImage, (u32, u32))> {
    let part = crop(cap, px);
    let scale = f64::from(full.width) / f64::from(cap.width.max(1));
    let longest = ((f64::from(part.width.max(part.height)) * scale).round() as u32).max(1);
    let cfg = ScreenshotConfig {
        max_dimension: longest,
        ..cfg.clone()
    };
    let (img, _) = encode_min(part, &cfg, 1)?;
    let offset = (
        (f64::from(px.0) * scale).round() as u32,
        (f64::from(px.1) * scale).round() as u32,
    );
    Ok((img, offset))
}

/// Black out screen-space rectangles of a capture (private data), before it
/// is encoded or fingerprinted. Returns how many areas were covered.
pub fn redact(cap: &mut Capture, rects: &[Rect], style: crate::config::RedactStyle) -> usize {
    if cap.bounds.width <= 0.0 || cap.bounds.height <= 0.0 {
        return 0;
    }
    let sx = f64::from(cap.width) / cap.bounds.width;
    let sy = f64::from(cap.height) / cap.bounds.height;
    let (w, h) = (i64::from(cap.width), i64::from(cap.height));
    let mut covered = 0;
    for r in rects {
        // A small margin so glyph edges and focus rings go too.
        let m = 2.0;
        let x0 = (((r.x - m - cap.bounds.x) * sx).floor() as i64).clamp(0, w);
        let y0 = (((r.y - m - cap.bounds.y) * sy).floor() as i64).clamp(0, h);
        let x1 = (((r.x + r.width + m - cap.bounds.x) * sx).ceil() as i64).clamp(0, w);
        let y1 = (((r.y + r.height + m - cap.bounds.y) * sy).ceil() as i64).clamp(0, h);
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        covered += 1;
        let stride = cap.width as usize * 4;
        match style {
            crate::config::RedactStyle::Fill => {
                for y in y0..y1 {
                    let row = y as usize * stride;
                    for x in x0..x1 {
                        let i = row + x as usize * 4;
                        cap.rgba[i..i + 3].copy_from_slice(&[128, 128, 128]);
                        cap.rgba[i + 3] = 255;
                    }
                }
            }
            crate::config::RedactStyle::Pixelate => {
                // Blocks as tall as the field, so no line of text survives.
                let block = (y1 - y0).clamp(8, 32);
                let mut by = y0;
                while by < y1 {
                    let ey = (by + block).min(y1);
                    let mut bx = x0;
                    while bx < x1 {
                        let ex = (bx + block).min(x1);
                        let mut sum = [0u64; 3];
                        let n = ((ey - by) * (ex - bx)).max(1) as u64;
                        for y in by..ey {
                            for x in bx..ex {
                                let i = y as usize * stride + x as usize * 4;
                                for (acc, v) in sum.iter_mut().zip(&cap.rgba[i..i + 3]) {
                                    *acc += u64::from(*v);
                                }
                            }
                        }
                        let avg = sum.map(|v| (v / n) as u8);
                        for y in by..ey {
                            for x in bx..ex {
                                let i = y as usize * stride + x as usize * 4;
                                cap.rgba[i..i + 3].copy_from_slice(&avg);
                            }
                        }
                        bx = ex;
                    }
                    by = ey;
                }
            }
        }
    }
    covered
}

/// How the coordinates written on a grid relate to a capture's pixels, on
/// one axis: `label = offset + scale * pixel`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Axis {
    pub offset: f64,
    pub scale: f64,
}

impl Axis {
    pub fn label(&self, pixel: f64) -> f64 {
        self.offset + self.scale * pixel
    }

    pub fn pixel(&self, label: f64) -> f64 {
        (label - self.offset) / self.scale
    }
}

/// Glyphs for grid labels besides the digits: a minus sign.
const MINUS: [u8; 15] = [0, 0, 0, 0, 0, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0];

fn draw_glyph(cap: &mut Capture, pat: &[u8; 15], x: i64, y: i64, scale: i64, rgb: [u8; 3]) {
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

/// `text` (digits and `-`) in a dark box with its top-left at (x, y).
fn draw_label(cap: &mut Capture, text: &str, x: i64, y: i64, scale: i64) {
    let w = text.chars().count() as i64 * (3 * scale + scale) + scale;
    let h = 5 * scale + 2 * scale;
    for yy in 0..h {
        for xx in 0..w {
            put(
                &mut cap.rgba,
                cap.width,
                cap.height,
                x + xx,
                y + yy,
                [0, 0, 0],
            );
        }
    }
    let mut cx = x + scale;
    for c in text.chars() {
        let pat = match c.to_digit(10) {
            Some(d) => &DIGITS[d as usize],
            None => &MINUS,
        };
        draw_glyph(cap, pat, cx, y + scale, scale, [255, 255, 0]);
        cx += 4 * scale;
    }
}

/// Mix `rgb` into the pixel at (x, y): 45% of the colour, 55% of what was there.
fn tint(buf: &mut [u8], w: u32, h: u32, x: i64, y: i64, rgb: [u8; 3]) {
    if x < 0 || y < 0 || x >= i64::from(w) || y >= i64::from(h) {
        return;
    }
    let i = ((y as u32 * w + x as u32) * 4) as usize;
    for (k, c) in rgb.iter().enumerate() {
        buf[i + k] = ((u16::from(buf[i + k]) * 55 + u16::from(*c) * 45) / 100) as u8;
    }
}

/// Draw a labelled coordinate grid over `cap`: a line every `step` label
/// units on each axis, its value written at the top (x) or left (y).
/// `out_scale` is how many capture pixels make one pixel of the image that
/// will be sent, so lines and labels stay readable after it is shrunk.
/// Returns the step used (larger than asked when lines would crowd).
pub fn draw_grid(cap: &mut Capture, ax: Axis, ay: Axis, step: f64, out_scale: f64) -> f64 {
    if cap.width == 0
        || cap.height == 0
        || step.is_nan()
        || step <= 0.0
        || ax.scale == 0.0
        || ay.scale == 0.0
    {
        return step;
    }
    let out_scale = if out_scale.is_finite() {
        out_scale.max(1.0)
    } else {
        1.0
    };
    let line = out_scale.round().max(1.0) as i64;
    // Digits 9x15 sent pixels: readable for any model.
    let glyph = (3.0 * out_scale).round().max(1.0) as i64;
    // At least 48 sent pixels between lines (room for a label), at most
    // 200 lines per axis.
    let mut step = step;
    let min_gap = |a: &Axis| 48.0 * out_scale * a.scale.abs();
    while step < min_gap(&ax).max(min_gap(&ay)) {
        step *= 2.0;
    }
    let (w, h) = (f64::from(cap.width), f64::from(cap.height));
    let lines = |a: &Axis, len: f64| -> Vec<(f64, i64)> {
        let (l0, l1) = (a.label(0.0), a.label(len));
        let (lo, hi) = (l0.min(l1), l0.max(l1));
        let mut out = Vec::new();
        let mut k = (lo / step).ceil();
        while k * step <= hi && out.len() < 200 {
            let v = k * step;
            out.push((v, a.pixel(v).round() as i64));
            k += 1.0;
        }
        out
    };
    let magenta = [255, 0, 255];
    let (xs, ys) = (lines(&ax, w), lines(&ay, h));
    for &(_, px) in &xs {
        for dx in 0..line {
            for y in 0..i64::from(cap.height) {
                tint(&mut cap.rgba, cap.width, cap.height, px + dx, y, magenta);
            }
        }
    }
    for &(_, py) in &ys {
        for dy in 0..line {
            for x in 0..i64::from(cap.width) {
                tint(&mut cap.rgba, cap.width, cap.height, x, py + dy, magenta);
            }
        }
    }
    let text = |v: f64| format!("{}", v.round() as i64);
    for &(v, px) in &xs {
        draw_label(cap, &text(v), px + line + 1, 0, glyph);
    }
    for &(v, py) in &ys {
        draw_label(cap, &text(v), 0, py + line + 1, glyph);
    }
    step
}

/// The colour at a capture pixel, as `#RRGGBB`.
pub fn color_at(cap: &Capture, x: f64, y: f64) -> Option<String> {
    if !(x.is_finite() && y.is_finite()) || x < 0.0 || y < 0.0 {
        return None;
    }
    let (x, y) = (x.floor() as u32, y.floor() as u32);
    if x >= cap.width || y >= cap.height {
        return None;
    }
    let i = ((y * cap.width + x) * 4) as usize;
    let p = cap.rgba.get(i..i + 3)?;
    Some(format!("#{:02X}{:02X}{:02X}", p[0], p[1], p[2]))
}

/// The main colours of a capture, most common first: `#RRGGBB` and the
/// share of the image (0 to 1). Similar shades count as one colour.
pub fn palette(cap: &Capture, max: usize) -> Vec<(String, f64)> {
    let n = cap.width as usize * cap.height as usize;
    if n == 0 || cap.rgba.len() < n * 4 {
        return Vec::new();
    }
    // Count 5-bit-per-channel bins over at most ~250k sampled pixels.
    let stride = (n / 250_000).max(1);
    let mut bins: HashMap<u16, (u64, [u64; 3])> = HashMap::new();
    let mut sampled = 0u64;
    for i in (0..n).step_by(stride) {
        let p = &cap.rgba[i * 4..i * 4 + 3];
        let key = (u16::from(p[0] >> 3) << 10) | (u16::from(p[1] >> 3) << 5) | u16::from(p[2] >> 3);
        let e = bins.entry(key).or_insert((0, [0; 3]));
        e.0 += 1;
        for (sum, v) in e.1.iter_mut().zip(p) {
            *sum += u64::from(*v);
        }
        sampled += 1;
    }
    // Most common first; ties in bin order, so the result is the same
    // every time.
    let mut bins: Vec<(u16, (u64, [u64; 3]))> = bins.into_iter().collect();
    bins.sort_by_key(|(key, (count, _))| (std::cmp::Reverse(*count), *key));
    let bins = bins.into_iter().map(|(_, b)| b);
    // Merge each bin into the first colour close to it (most common first).
    // Closer than this counts as one shade (5-bit bins are 8 apart); an
    // off-white and white stay apart.
    const NEAR: f64 = 14.0;
    let mut colours: Vec<(u64, [u64; 3])> = Vec::new();
    let avg = |c: &(u64, [u64; 3])| c.1.map(|v| v as f64 / c.0 as f64);
    for b in bins {
        let mb = avg(&b);
        let near = colours.iter_mut().find(|c| {
            let mc = avg(c);
            (0..3).map(|k| (mc[k] - mb[k]).powi(2)).sum::<f64>().sqrt() < NEAR
        });
        match near {
            Some(c) => {
                c.0 += b.0;
                for k in 0..3 {
                    c.1[k] += b.1[k];
                }
            }
            None => colours.push(b),
        }
    }
    colours.sort_by_key(|c| std::cmp::Reverse(c.0));
    colours
        .iter()
        .map(|c| {
            let m = avg(c);
            (
                format!(
                    "#{:02X}{:02X}{:02X}",
                    m[0].round() as u8,
                    m[1].round() as u8,
                    m[2].round() as u8
                ),
                c.0 as f64 / sampled as f64,
            )
        })
        .filter(|(_, share)| *share >= 0.005)
        .take(max)
        .collect()
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

    fn solid(w: u32, h: u32, rgb: [u8; 3]) -> Capture {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        Capture {
            width: w,
            height: h,
            rgba,
            bounds: Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
        }
    }

    #[test]
    fn grids_mark_their_lines_and_label_them() {
        let mut cap = solid(400, 300, [255, 255, 255]);
        let axis = Axis {
            offset: 0.0,
            scale: 1.0,
        };
        let step = draw_grid(&mut cap, axis, axis, 100.0, 1.0);
        assert_eq!(step, 100.0);
        // A line at x = 100 (tinted), none at x = 150.
        assert_ne!(color_at(&cap, 100.0, 150.0).unwrap(), "#FFFFFF");
        assert_eq!(color_at(&cap, 150.0, 150.0).unwrap(), "#FFFFFF");
        // Its label sits at the top, on a dark box.
        assert_eq!(color_at(&cap, 102.0, 0.0).unwrap(), "#000000");
        // Labels follow the axis: screen coordinates of an offset capture.
        let mut cap = solid(200, 100, [255, 255, 255]);
        let shifted = Axis {
            offset: 1000.0,
            scale: 2.0,
        };
        draw_grid(&mut cap, shifted, axis, 100.0, 1.0);
        // 1100 is at pixel 50.
        assert_ne!(color_at(&cap, 50.0, 60.0).unwrap(), "#FFFFFF");
        // Too dense a grid is thinned out.
        let mut cap = solid(400, 300, [255, 255, 255]);
        assert_eq!(draw_grid(&mut cap, axis, axis, 5.0, 2.0), 160.0);
    }

    #[test]
    fn palettes_and_picks_read_exact_colours() {
        let mut cap = solid(100, 100, [30, 136, 229]);
        for y in 0..100u32 {
            for x in 0..25u32 {
                let i = ((y * 100 + x) * 4) as usize;
                cap.rgba[i..i + 3].copy_from_slice(&[250, 250, 250]);
            }
            // An off-white strip: a colour of its own, not white.
            for x in 25..35u32 {
                let i = ((y * 100 + x) * 4) as usize;
                cap.rgba[i..i + 3].copy_from_slice(&[244, 241, 234]);
            }
        }
        let p = palette(&cap, 8);
        assert_eq!(p.len(), 3, "{p:?}");
        assert_eq!(p[0].0, "#1E88E5");
        assert!((p[0].1 - 0.65).abs() < 0.01);
        assert_eq!(p[1].0, "#FAFAFA");
        assert_eq!(p[2].0, "#F4F1EA");
        assert_eq!(color_at(&cap, 10.0, 10.0).as_deref(), Some("#FAFAFA"));
        assert_eq!(color_at(&cap, 99.5, 99.5).as_deref(), Some("#1E88E5"));
        assert_eq!(color_at(&cap, 100.0, 5.0), None);
        assert_eq!(color_at(&cap, -1.0, 5.0), None);
    }

    #[test]
    fn empty_captures_and_huge_outlines_do_not_panic_or_hang() {
        let empty = Capture {
            width: 0,
            height: 0,
            rgba: Vec::new(),
            bounds: Rect::new(0.0, 0.0, 10.0, 10.0),
        };
        assert_eq!(crop(&empty, (0, 0, 5, 5)).width, 0);
        assert!(encode(empty, &ScreenshotConfig::default()).is_err());

        let mut cap = Capture {
            width: 20,
            height: 10,
            rgba: vec![0; 20 * 10 * 4],
            bounds: Rect::new(0.0, 0.0, 20.0, 10.0),
        };
        let t = std::time::Instant::now();
        draw_rect_outline(
            &mut cap,
            Rect::new(-2.0e9, -2.0e9, 4.0e9, 4.0e9),
            [255, 0, 0],
        );
        draw_rect_outline(
            &mut cap,
            Rect::new(2.0, 2.0, f64::INFINITY, 5.0),
            [255, 0, 0],
        );
        draw_rect_outline(&mut cap, Rect::new(2.0, 2.0, f64::NAN, 5.0), [255, 0, 0]);
        assert!(t.elapsed() < std::time::Duration::from_secs(1));
        // A small rectangle is still drawn.
        draw_rect_outline(&mut cap, Rect::new(2.0, 2.0, 5.0, 4.0), [0, 255, 0]);
        let at = (2 * 20 + 3) * 4;
        assert_eq!(&cap.rgba[at..at + 3], &[0, 255, 0]);
    }

    #[test]
    fn a_small_changed_part_keeps_the_whole_images_scale() {
        let cap = Capture {
            width: 4000,
            height: 2000,
            rgba: vec![128; 4000 * 2000 * 4],
            bounds: Rect::new(0.0, 0.0, 4000.0, 2000.0),
        };
        let cfg = ScreenshotConfig {
            max_dimension: 1000,
            ..ScreenshotConfig::default()
        };
        let full = encode(cap.clone(), &cfg).unwrap().1;
        assert_eq!((full.width, full.height), (1000, 500));
        // 100x40 screen pixels are 25x10 in the whole image's scale.
        let (img, offset) = encode_part(&cap, (400, 200, 100, 40), &full, &cfg).unwrap();
        assert_eq!((img.width, img.height), (25, 10));
        assert_eq!(offset, (100, 50));
    }

    #[test]
    fn flat_captures_are_spotted() {
        let cap = |rgba: Vec<u8>| Capture {
            width: 2,
            height: 2,
            rgba,
            bounds: Rect::new(0.0, 0.0, 2.0, 2.0),
        };
        assert!(uniform(&cap([90; 16].to_vec())));
        let mut mixed = [90u8; 16].to_vec();
        mixed[4] = 10;
        assert!(!uniform(&cap(mixed)));
    }

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

    #[test]
    fn redaction_covers_only_the_private_area() {
        use crate::config::RedactStyle;
        // A 2x capture (Retina-like) of a 100x50 area at (10, 10).
        let mut cap = Capture {
            width: 200,
            height: 100,
            rgba: vec![0; 200 * 100 * 4],
            bounds: Rect::new(10.0, 10.0, 100.0, 50.0),
        };
        let field = Rect::new(30.0, 20.0, 20.0, 10.0);
        assert_eq!(redact(&mut cap, &[field], RedactStyle::Fill), 1);
        let px = |c: &Capture, x: usize, y: usize| c.rgba[(y * 200 + x) * 4];
        assert_eq!(px(&cap, 50, 30), 128, "inside the field");
        assert_eq!(px(&cap, 150, 80), 0, "elsewhere untouched");
        // Off-image areas are ignored.
        assert_eq!(
            redact(
                &mut cap,
                &[Rect::new(500.0, 500.0, 5.0, 5.0)],
                RedactStyle::Fill
            ),
            0
        );
        let mut cap2 = Capture {
            rgba: (0..200 * 100 * 4).map(|i| (i % 251) as u8).collect(),
            ..cap
        };
        assert_eq!(redact(&mut cap2, &[field], RedactStyle::Pixelate), 1);
    }
}
