//! The user's own picture as the agent's pointer (`overlay.cursor_style =
//! "custom"`, `overlay.cursor_image`): a PNG, or a GIF or APNG, still or
//! animated.
//!
//! The path only ever comes from the user's settings (the file, or the
//! settings panel), never from the agent. A picture is read with hard
//! limits and decoded once (then shared by every agent that shows it): a
//! file that is too big, too large, broken or has no frames is refused with
//! a reason, and the overlay keeps the default pointer. An animation keeps
//! its first [`MAX_FRAMES`] frames and [`MAX_LOOP_MS`] of them, and loops.
//! Nothing is fetched and no other program is run.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use image::{AnimationDecoder, ImageDecoder, ImageFormat, Limits};
use tiny_skia::Pixmap;

/// The largest file read.
pub const MAX_FILE: u64 = 4 * 1024 * 1024;
/// The largest picture, per side (px).
pub const MAX_SIDE: u32 = 512;
/// The most frames kept.
pub const MAX_FRAMES: usize = 150;
/// The longest animation kept (ms): later frames are left out.
pub const MAX_LOOP_MS: u64 = 5000;
/// The shortest a frame shows (ms): quicker ones are slowed to this.
pub const MIN_DELAY_MS: u64 = 20;
/// Frames are kept at most this big (their longer side), and all of them
/// together at most `STORE_PIXELS`: on screen a picture is 48 px at 100%.
const STORE_SIDE: u32 = 256;
const STORE_PIXELS: u64 = 6_000_000;
/// What a decoder may allocate at once.
const MAX_ALLOC: u64 = 64 * 1024 * 1024;
/// Pictures kept decoded at once (the agents' own and the custom one).
const MAX_CACHED: usize = 8;

/// One frame: the picture halved again and again (`levels[0]` is the
/// biggest kept), premultiplied, and when it shows in the loop.
pub struct Frame {
    pub levels: Vec<Pixmap>,
    pub start_ms: u64,
    pub delay_ms: u64,
}

/// A decoded picture.
pub struct Picture {
    pub frames: Vec<Frame>,
    /// Its size as made (px).
    pub width: u32,
    pub height: u32,
    /// One loop of the animation (0 for a still picture).
    pub loop_ms: u64,
    /// "png", "apng" or "gif".
    pub format: &'static str,
    /// Why some of it was left out (an animation too long), to say once.
    pub note: Option<String>,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Picture({} {}x{}, {} frames, {} ms)",
            self.format,
            self.width,
            self.height,
            self.frames.len(),
            self.loop_ms
        )
    }
}

impl Picture {
    pub fn animated(&self) -> bool {
        self.frames.len() > 1 && self.loop_ms > 0
    }

    /// The frame showing `ms` into the animation.
    pub fn frame_at(&self, ms: u64) -> usize {
        if !self.animated() {
            return 0;
        }
        let t = ms % self.loop_ms;
        self.frames
            .iter()
            .rposition(|f| f.start_ms <= t)
            .unwrap_or(0)
    }

    /// How long until the frame changes, `ms` into the animation (None:
    /// it never does).
    pub fn until_next(&self, ms: u64) -> Option<u64> {
        if !self.animated() {
            return None;
        }
        let t = ms % self.loop_ms;
        let f = &self.frames[self.frame_at(ms)];
        Some((f.start_ms + f.delay_ms).saturating_sub(t).max(1))
    }

    /// The level of `frame` to draw at `px` canvas px per picture px (as
    /// made), and the scale from that level's px to the picture's.
    pub fn level(&self, frame: usize, px: f32) -> (&Pixmap, f32) {
        let f = &self.frames[frame.min(self.frames.len() - 1)];
        // Picture px per px of each level.
        let unit = |l: &Pixmap| self.width.max(1) as f32 / l.width().max(1) as f32;
        let mut k = 0;
        while k + 1 < f.levels.len() && px * unit(&f.levels[k + 1]) <= 1.0 {
            k += 1;
        }
        (&f.levels[k], unit(&f.levels[k]))
    }
}

/// Read and decode the picture at `path` (see [`decode`]).
pub fn read(path: &Path) -> Result<Picture, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("can't read it: {e}"))?;
    if !meta.is_file() {
        return Err("it is not a file".into());
    }
    if meta.len() > MAX_FILE {
        return Err(format!(
            "the file is {:.1} MB; a pointer's picture may be 4 MB at most",
            meta.len() as f64 / 1_048_576.0
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("can't read it: {e}"))?;
    decode(&bytes)
}

/// The picture's kind by its first bytes: "png" (an APNG too) or "gif".
pub fn kind(bytes: &[u8]) -> Option<&'static str> {
    match image::guess_format(bytes) {
        Ok(ImageFormat::Png) => Some("png"),
        Ok(ImageFormat::Gif) => Some("gif"),
        _ => None,
    }
}

/// Decode a picture with the limits above: a PNG, an APNG or a GIF.
pub fn decode(bytes: &[u8]) -> Result<Picture, String> {
    if bytes.len() as u64 > MAX_FILE {
        return Err("the file is over 4 MB".into());
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    let too_big = |w: u32, h: u32| -> Result<(), String> {
        if w == 0 || h == 0 {
            return Err("the picture is empty".into());
        }
        if w > MAX_SIDE || h > MAX_SIDE {
            return Err(format!(
                "the picture is {w}×{h} px; it may be {MAX_SIDE} px a side at most"
            ));
        }
        Ok(())
    };
    let bad = |e: image::ImageError| format!("it can't be read as a picture ({e})");
    let (format, size, frames) = match kind(bytes) {
        Some("gif") => {
            let mut d = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).map_err(bad)?;
            let (w, h) = d.dimensions();
            too_big(w, h)?;
            d.set_limits(limits).map_err(bad)?;
            ("gif", (w, h), d.into_frames())
        }
        Some(_) => {
            // The size first, to say what is wrong; then read within limits.
            let (w, h) = image::codecs::png::PngDecoder::new(Cursor::new(bytes))
                .map_err(bad)?
                .dimensions();
            too_big(w, h)?;
            let d = image::codecs::png::PngDecoder::with_limits(Cursor::new(bytes), limits)
                .map_err(bad)?;
            if d.is_apng().map_err(bad)? {
                ("apng", (w, h), d.apng().map_err(bad)?.into_frames())
            } else {
                let img = image::DynamicImage::from_decoder(d).map_err(bad)?;
                let frame = image::Frame::new(img.to_rgba8());
                (
                    "png",
                    (w, h),
                    image::Frames::new(Box::new(std::iter::once(Ok(frame)))),
                )
            }
        }
        None => return Err("it is not a PNG or GIF picture".into()),
    };
    let mut kept: Vec<(Pixmap, u64)> = Vec::new();
    let mut at = 0u64;
    let mut note = None;
    for frame in frames {
        if kept.len() >= MAX_FRAMES || (at >= MAX_LOOP_MS && !kept.is_empty()) {
            note = Some(format!(
                "the animation is longer than this keeps: its first {} frames ({:.1} s) loop",
                kept.len(),
                at as f64 / 1000.0
            ));
            break;
        }
        let frame = match frame {
            Ok(f) => f,
            // A broken frame after good ones ends the animation there.
            Err(e) if !kept.is_empty() => {
                note = Some(format!(
                    "frame {} can't be read ({e}); the {} before it loop",
                    kept.len() + 1,
                    kept.len()
                ));
                break;
            }
            Err(e) => return Err(bad(e)),
        };
        let (n, d) = frame.delay().numer_denom_ms();
        let delay = if d == 0 {
            100
        } else {
            u64::from(n) / u64::from(d)
        }
        .max(MIN_DELAY_MS);
        let buf = frame.into_buffer();
        let mut pm = premultiplied(buf.width(), buf.height(), buf.into_raw())
            .ok_or("the picture is empty")?;
        while pm.width().max(pm.height()) > STORE_SIDE {
            match super::pointers::halve(&pm) {
                Some(h) => pm = h,
                None => break,
            }
        }
        kept.push((pm, delay));
        at += delay;
    }
    if kept.is_empty() {
        return Err("the picture has no frames".into());
    }
    // All of them together within the budget.
    while kept.len() > 1
        && kept
            .iter()
            .map(|(p, _)| u64::from(p.width()) * u64::from(p.height()))
            .sum::<u64>()
            > STORE_PIXELS
    {
        for (p, _) in &mut kept {
            if let Some(h) = super::pointers::halve(p) {
                *p = h;
            }
        }
    }
    let animated = kept.len() > 1;
    let mut start = 0;
    let frames = kept
        .into_iter()
        .map(|(pm, delay)| {
            let mut levels = vec![pm];
            while let Some(last) = levels.last() {
                if last.width() <= 24 || last.height() <= 24 {
                    break;
                }
                match super::pointers::halve(last) {
                    Some(h) => levels.push(h),
                    None => break,
                }
            }
            let f = Frame {
                levels,
                start_ms: start,
                delay_ms: delay,
            };
            start += delay;
            f
        })
        .collect();
    Ok(Picture {
        frames,
        width: size.0,
        height: size.1,
        loop_ms: if animated { start } else { 0 },
        format: if format == "apng" && !animated {
            "png"
        } else {
            format
        },
        note,
    })
}

/// Straight RGBA to a (premultiplied) pixmap.
fn premultiplied(w: u32, h: u32, mut rgba: Vec<u8>) -> Option<Pixmap> {
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = u16::from(px[3]);
        for c in &mut px[..3] {
            *c = ((u16::from(*c) * a + 127) / 255) as u8;
        }
    }
    Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(w, h)?)
}

/// What a lookup found.
#[derive(Clone)]
pub enum Lookup {
    Ready(Arc<Picture>),
    /// Being read (in the background): ask again.
    Loading,
    /// It can't be used, and why.
    Failed(String),
}

/// What a file was when it was read: a changed file is read again.
type Stamp = (u64, Option<SystemTime>);

#[derive(Clone)]
enum Slot {
    Loading,
    Done(std::result::Result<Arc<Picture>, String>),
}

/// Each picture's file as read, what came of it, and when it was last asked
/// for (the oldest goes first when the cache is full).
type Cache = HashMap<PathBuf, (Stamp, Slot, u64)>;

fn cache() -> &'static Mutex<Cache> {
    static CACHE: std::sync::OnceLock<Mutex<Cache>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn stamp(path: &Path) -> Stamp {
    std::fs::metadata(path).map_or((0, None), |m| (m.len(), m.modified().ok()))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The picture at `path`, without waiting: read once in the background
/// (and again when the file changes), then shared.
pub fn get(path: &Path) -> Lookup {
    lookup(path, false)
}

/// The picture at `path`, read now if it isn't yet.
pub fn load_now(path: &Path) -> std::result::Result<Arc<Picture>, String> {
    match lookup(path, true) {
        Lookup::Ready(p) => Ok(p),
        Lookup::Failed(e) => Err(e),
        // Another thread reads it: wait for that, briefly.
        Lookup::Loading => {
            for _ in 0..400 {
                std::thread::sleep(std::time::Duration::from_millis(10));
                match lookup(path, false) {
                    Lookup::Ready(p) => return Ok(p),
                    Lookup::Failed(e) => return Err(e),
                    Lookup::Loading => {}
                }
            }
            Err("it took too long to read".into())
        }
    }
}

fn lookup(path: &Path, now: bool) -> Lookup {
    static USE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tick = USE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let st = stamp(path);
    {
        let mut c = lock(cache());
        if let Some((s, slot, used)) = c.get_mut(path)
            && *s == st
        {
            *used = tick;
            return match slot {
                Slot::Loading => Lookup::Loading,
                Slot::Done(Ok(p)) => Lookup::Ready(p.clone()),
                Slot::Done(Err(e)) => Lookup::Failed(e.clone()),
            };
        }
        if c.len() >= MAX_CACHED {
            let oldest = c
                .iter()
                .filter(|(_, (_, s, _))| !matches!(s, Slot::Loading))
                .min_by_key(|(_, (_, _, u))| *u)
                .map(|(p, _)| p.clone());
            if let Some(p) = oldest {
                c.remove(&p);
            }
        }
        c.insert(path.to_path_buf(), (st, Slot::Loading, tick));
    }
    let path = path.to_path_buf();
    let work = move || {
        let r = read(&path).map(Arc::new);
        let mut c = lock(cache());
        if let Some((s, slot, _)) = c.get_mut(&path)
            && *s == st
        {
            *slot = Slot::Done(r.clone());
        }
        r
    };
    if now {
        return match work() {
            Ok(p) => Lookup::Ready(p),
            Err(e) => Lookup::Failed(e),
        };
    }
    let spawned = std::thread::Builder::new()
        .name("pointer-picture".into())
        .spawn(move || {
            let _ = work();
        });
    if spawned.is_err() {
        return Lookup::Failed("can't read it now".into());
    }
    Lookup::Loading
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A GIF of `frames` frames of `w`×`h`, each `delay` hundredths of a
    /// second, a frame its own colour.
    pub(crate) fn gif(w: u32, h: u32, frames: usize, delay: u32) -> Vec<u8> {
        use image::codecs::gif::{GifEncoder, Repeat};
        let mut out = Vec::new();
        {
            let mut e = GifEncoder::new_with_speed(&mut out, 30);
            e.set_repeat(Repeat::Infinite).unwrap();
            for i in 0..frames {
                // A clear column at the left, the rest coloured by the frame.
                let img = image::RgbaImage::from_fn(w, h, |x, _| {
                    let c = (i * 40 % 256) as u8;
                    image::Rgba([c, 200, 255 - c, if x > 1 { 255 } else { 0 }])
                });
                let d = image::Delay::from_numer_denom_ms(delay * 10, 1);
                e.encode_frame(image::Frame::from_parts(img, 0, 0, d))
                    .unwrap();
            }
        }
        out
    }

    pub(crate) fn png(w: u32, h: u32) -> Vec<u8> {
        let mut pm = Pixmap::new(w, h).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(30, 140, 230, 255));
        pm.encode_png().unwrap()
    }

    #[test]
    fn a_still_png_and_an_animated_gif_are_read_and_loop() {
        let p = decode(&png(40, 20)).unwrap();
        assert_eq!((p.width, p.height, p.frames.len()), (40, 20, 1));
        assert!(!p.animated());
        assert_eq!(p.until_next(123), None);
        let g = decode(&gif(16, 16, 3, 10)).unwrap();
        assert_eq!(g.frames.len(), 3);
        assert_eq!(g.loop_ms, 300);
        assert_eq!(g.format, "gif");
        assert_eq!((g.frame_at(0), g.frame_at(150), g.frame_at(250)), (0, 1, 2));
        // It loops.
        assert_eq!(g.frame_at(310), 0);
        assert_eq!(g.until_next(150), Some(50));
        // Each frame its own picture.
        let px = |f: usize| g.frames[f].levels[0].pixel(8, 8).unwrap();
        assert_ne!(px(0), px(1));
        // Too quick frames are slowed to 20 ms.
        let quick = decode(&gif(8, 8, 2, 0)).unwrap();
        assert!(quick.frames.iter().all(|f| f.delay_ms >= MIN_DELAY_MS));
    }

    #[test]
    fn long_animations_are_cut_short_not_refused() {
        // 200 frames of 50 ms = 10 s: the first 5 s (100 frames) stay.
        let g = decode(&gif(8, 8, 200, 5)).unwrap();
        assert_eq!(g.frames.len(), 100);
        assert_eq!(g.loop_ms, MAX_LOOP_MS);
        assert!(g.note.as_deref().unwrap().contains("first 100 frames"));
        // 180 quick frames: 150 stay.
        let g = decode(&gif(8, 8, 180, 2)).unwrap();
        assert_eq!(g.frames.len(), MAX_FRAMES);
        assert!(g.note.is_some());
    }

    #[test]
    fn bad_pictures_are_refused_with_a_reason() {
        // Too large, broken, not a picture, empty.
        let e = decode(&png(600, 20)).unwrap_err();
        assert!(e.contains("512"), "{e}");
        let e = decode(&gif(600, 10, 1, 10)).unwrap_err();
        assert!(e.contains("512"), "{e}");
        let mut broken = png(40, 40);
        broken.truncate(60);
        assert!(decode(&broken).is_err());
        assert!(decode(b"GIF89a\x10\x00\x10\x00").is_err());
        assert!(
            decode(b"hello, world")
                .unwrap_err()
                .contains("not a PNG or GIF")
        );
        assert!(decode(&[]).is_err());
        // A GIF with no frames at all: a header, a screen and the end.
        let none = b"GIF89a\x08\x00\x08\x00\x00\x00\x00\x3b";
        let e = decode(none).unwrap_err();
        assert!(
            e.contains("no frames") || e.contains("can't be read"),
            "{e}"
        );
        // Over 4 MB.
        assert!(
            decode(&vec![0u8; MAX_FILE as usize + 1])
                .unwrap_err()
                .contains("4 MB")
        );
        let dir = std::env::temp_dir().join(format!("cu-picture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("big.png");
        std::fs::write(&big, vec![0u8; MAX_FILE as usize + 10]).unwrap();
        assert!(read(&big).unwrap_err().contains("4 MB"));
        assert!(
            read(&dir.join("missing.png"))
                .unwrap_err()
                .contains("can't read")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_picture_is_read_once_and_again_when_it_changes() {
        let dir = std::env::temp_dir().join(format!("cu-picture-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("p.png");
        std::fs::write(&file, png(10, 10)).unwrap();
        let a = load_now(&file).unwrap();
        let b = load_now(&file).unwrap();
        assert!(Arc::ptr_eq(&a, &b), "decoded once");
        assert!(matches!(get(&file), Lookup::Ready(_)));
        std::fs::write(&file, png(12, 10)).unwrap();
        let c = load_now(&file).unwrap();
        assert_eq!(c.width, 12);
        // In the background: loading, then ready.
        let other = dir.join("q.gif");
        std::fs::write(&other, gif(8, 8, 2, 10)).unwrap();
        let mut ready = false;
        for _ in 0..200 {
            if let Lookup::Ready(p) = get(&other) {
                assert_eq!(p.frames.len(), 2);
                ready = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(ready);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
