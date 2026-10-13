//! The pointer's own picture and the overlay's sounds, from the page: a
//! picture the user chose is checked with the overlay's own reader and its
//! limits, kept in the server's folder (`cursors/custom.png` or `.gif`)
//! and set as the pointer; taken away again, the pointer it replaced comes
//! back. The sound test plays through the overlay's own player. Only the
//! page does this (the agent never uses the panel): no tool of the agent's
//! sets a picture or plays a sound.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde_json::{Value, json};

use super::settings::{apply, fail};
use crate::config::Config;
use crate::overlay::{picture, sound};

/// The largest request the upload route reads (a 4 MB picture as base64,
/// in JSON); every other route stays at `MAX_REQUEST`.
pub const MAX_UPLOAD: usize = 6 * 1024 * 1024;

fn folder(home: &Path) -> PathBuf {
    home.join("cursors")
}

/// The style the picture replaced, to put back.
fn before_file(home: &Path) -> PathBuf {
    folder(home).join("style-before")
}

/// Whether `p` is the picture the page keeps (not one of the user's own
/// files, which are never deleted here).
fn is_kept(home: &Path, p: &Path) -> bool {
    p.parent() == Some(folder(home).as_path())
        && p.file_stem().and_then(|s| s.to_str()) == Some("custom")
}

/// The custom pointer's picture as the settings name it, and how it reads.
pub fn info(cfg: &Config, home: &Path) -> Value {
    let o = &cfg.overlay;
    let path = o.cursor_image_path();
    let mut out = json!({
        "ok": true,
        "style": o.cursor_style,
        "image": o.cursor_image,
        "set": path.is_some(),
        "kept": path.as_deref().is_some_and(|p| is_kept(home, p)),
        "limits": {
            "file_mb": picture::MAX_FILE / (1024 * 1024),
            "side": picture::MAX_SIDE,
            "frames": picture::MAX_FRAMES,
            "seconds": picture::MAX_LOOP_MS / 1000,
        },
    });
    if let Some(p) = path {
        out["path"] = json!(p.display().to_string());
        match picture::read(&p) {
            Ok(pic) => {
                out["usable"] = json!(true);
                out["format"] = json!(pic.format);
                out["width"] = json!(pic.width);
                out["height"] = json!(pic.height);
                out["frames"] = json!(pic.frames.len());
                out["ms"] = json!(pic.loop_ms);
                if let Some(n) = &pic.note {
                    out["note"] = json!(n);
                }
            }
            Err(e) => {
                out["usable"] = json!(false);
                out["error"] = json!(e);
            }
        }
    }
    out
}

/// The picture to show on the page (`cursor/custom`): the settings' one,
/// when it reads as a picture.
pub fn image(cfg: &Config) -> Option<(Vec<u8>, &'static str)> {
    let p = cfg.overlay.cursor_image_path()?;
    if std::fs::metadata(&p).ok()?.len() > picture::MAX_FILE {
        return None;
    }
    let bytes = std::fs::read(&p).ok()?;
    let kind = match picture::kind(&bytes)? {
        "gif" => "image/gif",
        _ => "image/png",
    };
    picture::decode(&bytes).ok()?;
    Some((bytes, kind))
}

/// A picture from the page (`{name, data}`: base64): checked as the
/// overlay reads it, kept, and set as the pointer.
pub fn upload(path: &Path, home: &Path, cfg: &Config, body: &Value) -> Value {
    let Some(data) = body.get("data").and_then(Value::as_str) else {
        return fail("choose a picture");
    };
    // "data:image/png;base64,…" as a browser reads a file, or bare.
    let data = data.split_once(',').map_or(data, |(_, d)| d);
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data.trim()) else {
        return fail("that is not a picture");
    };
    let pic = match picture::decode(&bytes) {
        Ok(p) => p,
        // (Said as the overlay says it, so the page can put it in words.)
        Err(e) => return fail(&e),
    };
    let ext = if pic.format == "gif" { "gif" } else { "png" };
    let dir = folder(home);
    let file = dir.join(format!("custom.{ext}"));
    let part = dir.join(format!(".custom.{ext}.part"));
    let written = std::fs::create_dir_all(&dir)
        .and_then(|_| std::fs::write(&part, &bytes))
        .and_then(|_| std::fs::rename(&part, &file));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&part);
        return fail(&format!("can't keep the picture in {}: {e}", dir.display()));
    }
    // One kept picture at a time.
    for other in ["png", "gif"].iter().filter(|e| **e != ext) {
        let _ = std::fs::remove_file(dir.join(format!("custom.{other}")));
    }
    let style = cfg.overlay.cursor_style.trim();
    if style != "custom" {
        let _ = std::fs::write(before_file(home), style);
    }
    let mut reply = apply(
        path,
        &[
            (
                "overlay.cursor_image".into(),
                json!(file.display().to_string()),
            ),
            ("overlay.cursor_style".into(), json!("custom")),
        ],
        false,
    );
    if reply["ok"] == json!(true) {
        reply["frames"] = json!(pic.frames.len());
        if let Some(n) = &pic.note {
            reply["note"] = json!(n);
        }
    }
    reply
}

/// Take the picture away: the pointer before it comes back, and the kept
/// file goes (a file of the user's own is only no longer used).
pub fn remove(path: &Path, home: &Path, cfg: &Config) -> Value {
    let before = std::fs::read_to_string(before_file(home))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| crate::overlay::draw::CURSOR_STYLES.contains(&s.as_str()) && s != "custom")
        .unwrap_or_else(|| "random".into());
    let style = if cfg.overlay.cursor_style.trim() == "custom" {
        before
    } else {
        cfg.overlay.cursor_style.clone()
    };
    let reply = apply(
        path,
        &[
            ("overlay.cursor_image".into(), json!("")),
            ("overlay.cursor_style".into(), json!(style)),
        ],
        false,
    );
    if reply["ok"] == json!(true) {
        if let Some(p) = cfg.overlay.cursor_image_path().filter(|p| is_kept(home, p)) {
            let _ = std::fs::remove_file(p);
        }
        let _ = std::fs::remove_file(before_file(home));
    }
    reply
}

/// Play one of the overlay's sounds now, as the settings make it (the
/// page's test button).
pub fn test_sound(cfg: &Config, body: &Value) -> Value {
    let Some(event) = body
        .get("event")
        .and_then(Value::as_str)
        .and_then(sound::Event::named)
    else {
        return fail("say which sound");
    };
    let mut player = sound::Player::new();
    match player.test(event, &cfg.overlay) {
        Ok((name, file)) => json!({
            "ok": true,
            "player": name,
            "file": file.display().to_string(),
        }),
        Err(e) => fail(&e),
    }
}
