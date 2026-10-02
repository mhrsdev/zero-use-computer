//! Reading text off the screen for apps whose accessibility tree has
//! little or nothing in it (games, canvases, remote desktops, some custom
//! toolkits). Each OS's own OCR is used — Windows.Media.Ocr on Windows, the
//! Vision framework on macOS — and Tesseract (`tesseract` on PATH) wherever
//! there is none. Recognised lines become `ocr text` elements in the tree:
//! numbered like any other element and clickable by index (the engine
//! clicks at their position).

use std::io::Write as _;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::types::{ActionDesc, Capture, ElementHandle, NodeStates, OcrLine, RawNode, Rect};

/// Handles of elements read by OCR carry this bit (backends never make
/// such handles), so the engine knows to act on them by position.
pub const OCR_HANDLE: ElementHandle = 1 << 62;

/// Role of an element read by OCR.
pub const OCR_ROLE: &str = "ocr text";

pub fn is_ocr(handle: ElementHandle) -> bool {
    handle & OCR_HANDLE != 0
}

/// Text reduced to lowercase words (letters and digits), for comparing what
/// OCR read with what the accessibility tree says.
pub fn words(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Pixel rectangle of a capture → screen rectangle.
pub fn to_screen(cap: &Capture, x: f64, y: f64, w: f64, h: f64) -> Rect {
    let sx = cap.bounds.width / f64::from(cap.width.max(1));
    let sy = cap.bounds.height / f64::from(cap.height.max(1));
    Rect::new(cap.bounds.x + x * sx, cap.bounds.y + y * sy, w * sx, h * sy)
}

/// Tree elements for recognised lines, children of the window (node 0).
pub fn nodes(lines: &[OcrLine], min_confidence: f64, max: usize) -> Vec<RawNode> {
    lines
        .iter()
        .filter(|l| f64::from(l.confidence) >= min_confidence && !l.text.trim().is_empty())
        .take(max)
        .enumerate()
        .map(|(i, l)| RawNode {
            handle: OCR_HANDLE | i as u64,
            parent: Some(0),
            // Stable across small moves, so indices survive a re-read.
            key: Some(format!(
                "ocr:{}:{}:{}",
                l.text.trim(),
                (l.bounds.x / 24.0).round() as i64,
                (l.bounds.y / 24.0).round() as i64
            )),
            role: OCR_ROLE.into(),
            native_role: "ocr".into(),
            name: Some(l.text.trim().to_string()),
            bounds: Some(l.bounds),
            actions: Vec::<ActionDesc>::new(),
            states: NodeStates {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .collect()
}

/// Tesseract's language code for a common ISO 639-1 code.
fn tesseract_lang(code: &str) -> String {
    let full = code.trim().to_lowercase();
    // "en-US" / "en_US" → "en"; Tesseract's own codes ("chi_tra") pass through.
    let base = full.split(['-', '_']).next().unwrap_or("");
    if base.len() != 2 {
        return full;
    }
    match base {
        "en" => "eng",
        "fa" => "fas",
        "ar" => "ara",
        "de" => "deu",
        "fr" => "fra",
        "es" => "spa",
        "it" => "ita",
        "pt" => "por",
        "ru" => "rus",
        "tr" => "tur",
        "zh" => "chi_sim",
        "ja" => "jpn",
        "ko" => "kor",
        "nl" => "nld",
        "pl" => "pol",
        "uk" => "ukr",
        "hi" => "hin",
        "he" => "heb",
        other => return other.to_string(),
    }
    .to_string()
}

/// Run Tesseract on a capture. UI text is small, so the image is enlarged
/// first (Tesseract reads best at a larger x-height).
pub fn tesseract(cap: &Capture, languages: &[String], program: &str) -> Result<Vec<OcrLine>> {
    let scale: u32 = if cap.width.max(cap.height) <= 2400 {
        2
    } else {
        1
    };
    tesseract_at(cap, languages, program, scale)
}

/// Run Tesseract on a part of a window at both sizes and keep the best
/// reading of each place: enlarging helps small UI text but can make
/// Tesseract take a framed label for one big glyph (a canvas's boxes read
/// as "Ecce"), which it reads as it is.
pub fn tesseract_both(cap: &Capture, languages: &[String], program: &str) -> Result<Vec<OcrLine>> {
    let big = if cap.width.max(cap.height) <= 2400 {
        tesseract_at(cap, languages, program, 2)?
    } else {
        Vec::new()
    };
    let small = tesseract_at(cap, languages, program, 1)?;
    Ok(merge(small, big))
}

/// Lines from two readings of the same picture: where they overlap, the
/// one read with more confidence.
pub fn merge(a: Vec<OcrLine>, b: Vec<OcrLine>) -> Vec<OcrLine> {
    let overlap = |x: &Rect, y: &Rect| {
        let w = (x.x + x.width).min(y.x + y.width) - x.x.max(y.x);
        let h = (x.y + x.height).min(y.y + y.height) - x.y.max(y.y);
        if w <= 0.0 || h <= 0.0 {
            return false;
        }
        let smaller = (x.width * x.height).min(y.width * y.height).max(1e-9);
        w * h >= 0.5 * smaller
    };
    let mut out: Vec<OcrLine> = Vec::with_capacity(a.len() + b.len());
    for line in a.into_iter().chain(b) {
        match out.iter_mut().find(|o| overlap(&o.bounds, &line.bounds)) {
            Some(o) if line.confidence > o.confidence => *o = line,
            Some(_) => {}
            None => out.push(line),
        }
    }
    out
}

/// Whether a line read off a picture is likely text: mostly letters or
/// digits, few symbols, and not as tall as a shape (Tesseract reads
/// a canvas's circles and stars as big glyphs: "AH @®@").
pub fn plausible(line: &OcrLine) -> bool {
    const PUNCTUATION: &str = ".,:;!?'\"-–—/()&%#+$€£@";
    let chars: Vec<char> = line.text.chars().filter(|c| !c.is_whitespace()).collect();
    let alnum = chars.iter().filter(|c| c.is_alphanumeric()).count();
    let symbols = chars
        .iter()
        .filter(|c| !c.is_alphanumeric() && !PUNCTUATION.contains(**c))
        .count();
    alnum >= 2
        && alnum * 2 >= chars.len()
        && symbols * 3 <= chars.len()
        && line.bounds.height <= 80.0
}

/// Run Tesseract on a capture enlarged `scale` times.
pub fn tesseract_at(
    cap: &Capture,
    languages: &[String],
    program: &str,
    scale: u32,
) -> Result<Vec<OcrLine>> {
    let scale = scale.max(1);
    let img = image::RgbaImage::from_raw(cap.width, cap.height, cap.rgba.clone())
        .ok_or_else(|| Error::Internal("capture buffer size mismatch".into()))?;
    let img = if scale > 1 {
        image::imageops::resize(
            &img,
            cap.width * scale,
            cap.height * scale,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| Error::Internal(format!("encoding for OCR: {e}")))?;

    let langs: Vec<String> = languages.iter().map(|l| tesseract_lang(l)).collect();
    let mut cmd = Command::new(program);
    cmd.args(["stdin", "stdout", "--psm", "11"]);
    if !langs.is_empty() {
        cmd.args(["-l", &langs.join("+")]);
    }
    cmd.arg("tsv")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let child = cmd.spawn().map_err(|e| {
        Error::Unsupported(format!(
            "no OCR: `{program}` could not be started ({e}); install Tesseract or set ocr.tesseract_path"
        ))
    })?;
    let (status, stdout) = finish(child, png, TESSERACT_TIMEOUT)?;
    if !status.success() {
        return Err(Error::Platform(format!(
            "tesseract failed ({status}); are the language data files installed for {langs:?}?"
        )));
    }
    Ok(parse_tsv(
        &String::from_utf8_lossy(&stdout),
        cap,
        f64::from(scale),
    ))
}

/// Longest Tesseract may take on one picture.
const TESSERACT_TIMEOUT: Duration = Duration::from_secs(60);

/// Feed `input` to `child` and collect what it writes, within `limit`: a
/// stuck child is killed, and every child is waited for (no zombies).
fn finish(
    mut child: std::process::Child,
    input: Vec<u8>,
    limit: Duration,
) -> Result<(std::process::ExitStatus, Vec<u8>)> {
    let fail = |e: std::io::Error| Error::Platform(format!("tesseract: {e}"));
    // Writing and reading on threads of their own: a child that answers
    // before it has read everything can't block us.
    let writer = child.stdin.take().map(|mut stdin| {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        })
    });
    let reader = child.stdout.take().map(|mut stdout| {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = std::io::Read::read_to_end(&mut stdout, &mut out);
            out
        })
    });
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Platform(format!(
                    "tesseract took longer than {} s and was stopped",
                    limit.as_secs()
                )));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(fail(e));
            }
        }
    };
    if let Some(w) = writer {
        let _ = w.join();
    }
    let stdout = reader.and_then(|r| r.join().ok()).unwrap_or_default();
    Ok((status, stdout))
}

/// Words from Tesseract's TSV output, grouped into lines.
fn parse_tsv(tsv: &str, cap: &Capture, scale: f64) -> Vec<OcrLine> {
    struct Acc {
        words: Vec<String>,
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        conf: f64,
    }
    let mut lines: Vec<((u32, u32, u32), Acc)> = Vec::new();
    for row in tsv.lines().skip(1) {
        let f: Vec<&str> = row.split('\t').collect();
        if f.len() < 12 || f[0] != "5" {
            continue;
        }
        let num = |i: usize| f[i].trim().parse::<f64>().unwrap_or(-1.0);
        let text = f[11].trim();
        let conf = num(10);
        if text.is_empty() || conf < 0.0 {
            continue;
        }
        let key = (num(2) as u32, num(3) as u32, num(4) as u32);
        let (x, y, w, h) = (num(6), num(7), num(8), num(9));
        let acc = match lines.iter_mut().find(|(k, _)| *k == key) {
            Some((_, a)) => a,
            None => {
                lines.push((
                    key,
                    Acc {
                        words: Vec::new(),
                        x0: f64::MAX,
                        y0: f64::MAX,
                        x1: 0.0,
                        y1: 0.0,
                        conf: 0.0,
                    },
                ));
                &mut lines.last_mut().expect("just pushed").1
            }
        };
        acc.words.push(text.to_string());
        acc.x0 = acc.x0.min(x);
        acc.y0 = acc.y0.min(y);
        acc.x1 = acc.x1.max(x + w);
        acc.y1 = acc.y1.max(y + h);
        acc.conf += conf;
    }
    lines
        .into_iter()
        .map(|(_, a)| OcrLine {
            confidence: (a.conf / a.words.len() as f64 / 100.0) as f32,
            text: a.words.join(" "),
            bounds: to_screen(
                cap,
                a.x0 / scale,
                a.y0 / scale,
                (a.x1 - a.x0) / scale,
                (a.y1 - a.y0) / scale,
            ),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap() -> Capture {
        Capture {
            width: 200,
            height: 100,
            rgba: vec![255; 200 * 100 * 4],
            bounds: Rect::new(100.0, 50.0, 100.0, 50.0),
        }
    }

    #[test]
    fn shapes_read_as_glyphs_are_not_text() {
        let line = |text: &str, h: f64| OcrLine {
            text: text.into(),
            bounds: Rect::new(0.0, 0.0, 100.0, h),
            confidence: 0.8,
        };
        assert!(plausible(&line("Today's order number: 58213-QX", 22.0)));
        assert!(plausible(&line("DELTA", 13.0)));
        assert!(!plausible(&line("AH @®@", 30.0)), "mostly symbols");
        assert!(!plausible(&line("HeA", 114.0)), "as tall as a shape");
        assert!(!plausible(&line("~", 12.0)));
    }

    #[test]
    fn two_readings_keep_the_surer_line_of_each_place() {
        let line = |text: &str, x: f64, conf: f32| OcrLine {
            text: text.into(),
            bounds: Rect::new(x, 10.0, 60.0, 20.0),
            confidence: conf,
        };
        let small = vec![line("DELTA", 100.0, 0.9), line("ECHO", 300.0, 0.8)];
        let big = vec![line("Ecce", 95.0, 0.45), line("GOLF", 500.0, 0.7)];
        let merged = merge(small, big);
        let texts: Vec<&str> = merged.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["DELTA", "ECHO", "GOLF"]);
    }

    #[test]
    fn tsv_words_become_lines_in_screen_coordinates() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
            4\t1\t1\t1\t1\t0\t20\t40\t200\t30\t-1\t\n\
            5\t1\t1\t1\t1\t1\t20\t40\t80\t30\t96\tHello\n\
            5\t1\t1\t1\t1\t2\t120\t40\t100\t30\t90\tworld\n\
            5\t1\t2\t1\t1\t1\t20\t140\t60\t20\t40\tOK\n";
        // A 2x enlarged image of a Retina-like (2 px per point) capture.
        let lines = parse_tsv(tsv, &cap(), 2.0);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "Hello world");
        assert!((lines[0].confidence - 0.93).abs() < 0.01);
        assert_eq!(lines[0].bounds, Rect::new(105.0, 60.0, 50.0, 7.5));
        let nodes = nodes(&lines, 0.5, 10);
        assert_eq!(nodes.len(), 1, "low confidence dropped");
        assert!(is_ocr(nodes[0].handle));
        assert_eq!(nodes[0].role, OCR_ROLE);
    }

    #[test]
    fn language_codes() {
        assert_eq!(tesseract_lang("en-US"), "eng");
        assert_eq!(tesseract_lang("de"), "deu");
        assert_eq!(tesseract_lang("chi_tra"), "chi_tra");
    }

    /// Black text on white, drawn with a system font; `None` without fonts.
    fn sample(text: &str) -> Option<Capture> {
        use tiny_skia::{Color, FillRule, Paint, Pixmap, Transform};
        let fonts = crate::overlay::text::Fonts::load("");
        let t = crate::overlay::text::layout(&fonts, text, 36.0);
        let path = t.path?;
        let (w, h) = (t.width.ceil() as u32 + 80, t.height.ceil() as u32 + 60);
        let mut pm = Pixmap::new(w, h)?;
        pm.fill(Color::WHITE);
        let mut paint = Paint::default();
        paint.set_color(Color::BLACK);
        paint.anti_alias = true;
        pm.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::from_translate(40.0, 30.0),
            None,
        );
        Some(Capture {
            width: w,
            height: h,
            rgba: pm.data().to_vec(),
            bounds: Rect::new(1000.0, 500.0, f64::from(w), f64::from(h)),
        })
    }

    /// Reads real text with every engine this machine has: the OS's own
    /// (Windows, macOS) and Tesseract when installed.
    #[test]
    fn reads_rendered_text() {
        let Some(cap) = sample("Hello OCR world 2026") else {
            eprintln!("no font to draw with; skipped");
            return;
        };
        let mut engines: Vec<(&str, Result<Vec<OcrLine>>)> = Vec::new();
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            use crate::Backend as _;
            match crate::platform_backend() {
                Ok(mut b) => engines.push(("built-in", b.ocr(&cap, &[]))),
                Err(e) if std::env::var_os("CI").is_some() => panic!("no backend on CI: {e}"),
                Err(e) => eprintln!("no backend: {e}"),
            }
        }
        engines.push(("tesseract", tesseract(&cap, &[], "tesseract")));
        for (name, result) in engines {
            match result {
                Ok(lines) => {
                    let all: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
                    eprintln!("{name}: {all:?}");
                    let joined = all.join(" ").to_lowercase();
                    assert!(
                        joined.contains("hello") && joined.contains("world"),
                        "{name} read {all:?}"
                    );
                    // In screen coordinates, inside the capture.
                    let l = &lines[0];
                    assert!(
                        l.bounds.x >= 1000.0 && l.bounds.y >= 500.0,
                        "{name}: {:?}",
                        l.bounds
                    );
                }
                Err(e) => {
                    eprintln!("{name} OCR unavailable here: {e}");
                    // The OS's own OCR must work on the CI runners.
                    if name == "built-in" && std::env::var_os("CI").is_some() {
                        panic!("built-in OCR failed on CI: {e}");
                    }
                }
            }
        }
    }
}
