//! Label text: system font lookup, shaping (rustybuzz) and bidirectional
//! ordering, so labels in right-to-left scripts (Arabic script, Hebrew) and
//! mixed-direction text render correctly.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use rustybuzz::ttf_parser::{GlyphId, OutlineBuilder};
use rustybuzz::{Direction, Face, UnicodeBuffer};
use tiny_skia::{Path, PathBuilder};

/// A font file mapped into memory: its pages are shared with the system's
/// file cache and only those in use stay resident (Arial Unicode alone is
/// 22 MB), and each file is mapped once per process.
type FontData = Arc<memmap2::Mmap>;

fn open_font(path: &str) -> Option<FontData> {
    static OPEN: OnceLock<Mutex<HashMap<String, Option<FontData>>>> = OnceLock::new();
    let mut open = OPEN
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    open.entry(path.to_string())
        .or_insert_with(|| {
            let file = std::fs::File::open(path).ok()?;
            // SAFETY: a font file isn't changed while programs use it; the
            // map is only read, as every program that maps fonts does.
            let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
            Face::from_slice(&map, 0)?;
            Some(Arc::new(map))
        })
        .clone()
}

/// Loaded font files, tried in order for each run of text.
#[derive(Default)]
pub struct Fonts {
    faces: Vec<(FontData, u32)>,
}

impl Fonts {
    /// `custom` (a font file path) first, then the platform's usual fonts.
    pub fn load(custom: &str) -> Self {
        let mut paths: Vec<String> = Vec::new();
        if !custom.trim().is_empty() {
            paths.push(custom.trim().to_string());
        }
        #[cfg(target_os = "linux")]
        // A font for right-to-left scripts first, then the usual one.
        for query in ["sans:lang=ar", "sans:lang=he", "sans"] {
            if let Ok(out) = std::process::Command::new("fc-match")
                .args(["-f", "%{file}", query])
                .output()
                && out.status.success()
            {
                paths.push(String::from_utf8_lossy(&out.stdout).trim().to_string());
            }
        }
        paths.extend(CANDIDATES.iter().map(|s| s.to_string()));

        let mut faces = Vec::new();
        for p in paths {
            if faces.len() >= 4 {
                break;
            }
            let Some(data) = open_font(&p) else {
                continue;
            };
            if !faces.iter().any(|(d, _)| Arc::ptr_eq(d, &data)) {
                faces.push((data, 0));
            }
        }
        Self { faces }
    }

    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }
}

#[cfg(target_os = "linux")]
const CANDIDATES: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansArabic-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansHebrew-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
];
#[cfg(target_os = "macos")]
const CANDIDATES: &[&str] = &[
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/Library/Fonts/Arial Unicode.ttf",
    "/System/Library/Fonts/Supplemental/Tahoma.ttf",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "/System/Library/Fonts/Geneva.ttf",
];
#[cfg(target_os = "windows")]
const CANDIDATES: &[&str] = &[
    "C:\\Windows\\Fonts\\segoeui.ttf",
    "C:\\Windows\\Fonts\\tahoma.ttf",
    "C:\\Windows\\Fonts\\arial.ttf",
];
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
const CANDIDATES: &[&str] = &[];

/// A laid-out line of text as one fillable path.
pub struct TextPath {
    pub path: Option<Path>,
    pub width: f32,
    /// Distance from the top of the line box to the baseline.
    pub ascent: f32,
    /// Height of the line box.
    pub height: f32,
    /// The text reads right to left (align it right).
    pub rtl: bool,
}

struct Builder {
    pb: PathBuilder,
    scale: f32,
    x: f32,
    y: f32,
}

impl OutlineBuilder for Builder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.pb
            .move_to(self.x + x * self.scale, self.y - y * self.scale);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.pb
            .line_to(self.x + x * self.scale, self.y - y * self.scale);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.pb.quad_to(
            self.x + x1 * self.scale,
            self.y - y1 * self.scale,
            self.x + x * self.scale,
            self.y - y * self.scale,
        );
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.pb.cubic_to(
            self.x + x1 * self.scale,
            self.y - y1 * self.scale,
            self.x + x2 * self.scale,
            self.y - y2 * self.scale,
            self.x + x * self.scale,
            self.y - y * self.scale,
        );
    }
    fn close(&mut self) {
        self.pb.close();
    }
}

/// Format characters that shape or order text but draw nothing.
fn invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}')
        || crate::text::is_bidi_control(c)
}

/// Lay out one line of `text` at `px` pixels per em, in visual order.
pub fn layout(fonts: &Fonts, text: &str, px: f32) -> TextPath {
    let parsed: Vec<Face> = fonts
        .faces
        .iter()
        .filter_map(|(d, i)| Face::from_slice(d, *i))
        .collect();
    let Some(first) = parsed.first() else {
        return TextPath {
            path: None,
            width: 0.0,
            ascent: px * 0.8,
            height: px * 1.2,
            rtl: false,
        };
    };
    let scale0 = px / f32::from(first.units_per_em().max(1) as u16);
    let ascent = f32::from(first.ascender()) * scale0;
    let height = (f32::from(first.ascender()) - f32::from(first.descender())) * scale0;

    let mut pb = Builder {
        pb: PathBuilder::new(),
        scale: 1.0,
        x: 0.0,
        y: ascent,
    };
    let mut pen = 0.0f32;
    let line = text.replace(['\n', '\r'], " ");
    let bidi = unicode_bidi::BidiInfo::new(&line, None);
    let rtl = bidi.paragraphs.first().is_some_and(|p| p.level.is_rtl());
    for para in &bidi.paragraphs {
        let (levels, runs) = bidi.visual_runs(para, para.range.clone());
        for run in runs {
            let slice = &line[run.clone()];
            let rtl = levels[run.start].is_rtl();
            // The first font that has every visible character of the run.
            // Zero-width joiners/non-joiners and direction marks have no
            // glyph of their own in many fonts; they don't count.
            let face = parsed
                .iter()
                .find(|f| {
                    slice
                        .chars()
                        .filter(|c| !c.is_whitespace() && !c.is_control() && !invisible(*c))
                        .all(|c| f.glyph_index(c).is_some())
                })
                .unwrap_or(first);
            let scale = px / f32::from(face.units_per_em().max(1) as u16);
            let mut buf = UnicodeBuffer::new();
            buf.push_str(slice);
            buf.set_direction(if rtl {
                Direction::RightToLeft
            } else {
                Direction::LeftToRight
            });
            let shaped = rustybuzz::shape(face, &[], buf);
            for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
                pb.scale = scale;
                pb.x = pen + pos.x_offset as f32 * scale;
                pb.y = ascent - pos.y_offset as f32 * scale;
                face.outline_glyph(GlyphId(info.glyph_id as u16), &mut pb);
                pen += pos.x_advance as f32 * scale;
            }
        }
    }
    TextPath {
        path: pb.pb.finish(),
        width: pen.max(0.0),
        ascent,
        height,
        rtl,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lays_out_left_to_right_and_right_to_left_text() {
        let fonts = Fonts::load("");
        if fonts.is_empty() {
            return; // no system fonts in this environment
        }
        let en = layout(&fonts, "Zero is using the computer", 14.0);
        assert!(en.width > 100.0 && en.path.is_some());
        // Arabic-script letters (joined by shaping), a zero-width
        // non-joiner and a direction mark: still one right-to-left line.
        let arabic: String =
            "\u{0628}\u{0628}\u{200C}\u{0628}\u{0628} \u{0628}\u{0628}\u{200F}".into();
        let rtl = layout(&fonts, &arabic, 14.0);
        assert!(rtl.rtl && !en.rtl);
        assert!(rtl.width > 10.0, "{}", rtl.width);
        let he = layout(&fonts, "\u{05E9}\u{05DC}\u{05D5}\u{05DD}", 14.0);
        assert!(he.rtl);
        assert!(layout(&fonts, "", 14.0).width == 0.0);
    }
}
