//! What the panel reports about the running program: the overview, the
//! tools and what each costs, the apps that needed pixels, the audit log
//! and a preview of how the pointer moves.

use std::io::{Read, Seek, SeekFrom};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::config::{self, Config};
use crate::{apps_log, motion, tools, update};

fn ago(path: &Path) -> Option<u64> {
    let m = std::fs::metadata(path).ok()?.modified().ok()?;
    SystemTime::now()
        .duration_since(m)
        .ok()
        .map(|d| d.as_secs())
}

fn secs_since(t: u64) -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
        .saturating_sub(t)
}

/// The overview: what is set up, and what is running.
pub fn overview(path: Option<&Path>, cfg: &Config) -> Value {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let updates = update::updates_dir();
    let pending = update::pending(&updates);
    let hub_up = cfg.hub.enabled
        && TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], cfg.hub.port)),
            Duration::from_millis(150),
        )
        .is_ok();
    let key = |k: &str| {
        let k = k.trim();
        if k.is_empty() {
            Value::Null
        } else {
            json!(crate::overlay::helper::pretty_key(k))
        }
    };
    let d = &cfg.decision;
    let decision = if d.provider.trim().is_empty() {
        Value::Null
    } else {
        json!(format!(
            "{}{}",
            d.provider,
            if d.model.trim().is_empty() {
                String::new()
            } else {
                format!(" · {}", d.model.trim())
            }
        ))
    };
    json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "platform": crate::PLATFORM,
        "exe": exe,
        "settings_file": path.map(|p| p.display().to_string()),
        "settings_file_exists": path.is_some_and(Path::exists),
        "stop_key": key(&cfg.control.stop_hotkey),
        "settings_key": key(&cfg.control.settings_hotkey),
        "decision": decision,
        "tools": tools::definitions_from(cfg).len(),
        "hub": {"enabled": cfg.hub.enabled, "running": hub_up, "port": cfg.hub.port},
        "overlay": cfg.overlay.enabled,
        "update": {
            "enabled": cfg.update.enabled,
            "install": format!("{:?}", cfg.update.install).to_lowercase(),
            "last_check_secs_ago": ago(&updates.join("last-check")),
            "pending": pending.as_ref().map(|p| p.version.clone()),
            "pending_since_secs": pending.as_ref().map(|p| secs_since(p.downloaded)),
        },
        "audit": cfg.audit.enabled,
        "screenshots": cfg.screenshot.enabled && !cfg.text_only,
    })
}

/// Every tool, whether it is offered, and about what it costs on every
/// request (the project's estimate: four characters a token).
pub fn tool_list(cfg: &Config) -> Value {
    // All of them, in the description style in use.
    let open = tools::definitions_for(&config::ToolsConfig {
        disabled: Vec::new(),
        enabled: Vec::new(),
        ..cfg.tools.clone()
    });
    let offered: std::collections::HashSet<String> = tools::definitions_from(cfg)
        .into_iter()
        .map(|d| d.name.to_string())
        .collect();
    let mut list: Vec<Value> = open
        .into_iter()
        .map(|d| {
            let name = d.name.to_string();
            let chars = serde_json::to_string(&d).map_or(0, |s| s.chars().count());
            json!({
                "name": name,
                "tokens": chars.div_ceil(4),
                "disabled": cfg.tools.disabled.iter().any(|x| x.eq_ignore_ascii_case(&name)),
                "offered": offered.contains(&name),
                "title": d.title,
            })
        })
        .collect();
    list.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    json!({"ok": true, "tools": list, "manager": format!("{:?}", cfg.tools.manager).to_lowercase()})
}

/// The apps the tree served poorly, and how often pixels were needed.
pub fn apps() -> Value {
    let path = config::home_dir().join("apps.json");
    let list: Vec<Value> = apps_log::records(&path)
        .into_iter()
        .take(100)
        .map(|(name, r)| {
            json!({
                "app": name, "looks": r.looks, "little_tree": r.little_tree,
                "share_little": (r.share_little() * 100.0).round(),
                "text_read": r.text_read, "pictures": r.pictures,
                "pictures_left_out": r.pictures_left_out,
            })
        })
        .collect();
    json!({"ok": true, "apps": list, "file": path.display().to_string()})
}

/// The end of the audit log: the last `lines` lines, newest last.
pub fn audit(cfg: &Config, lines: usize) -> Value {
    let path: PathBuf = cfg
        .audit
        .path
        .as_deref()
        .and_then(config::settings_path)
        .unwrap_or_else(|| config::home_dir().join("audit.log"));
    let Ok(mut f) = std::fs::File::open(&path) else {
        return json!({"ok": true, "enabled": cfg.audit.enabled, "file": path.display().to_string(), "lines": [], "exists": false});
    };
    let len = f.metadata().map_or(0, |m| m.len());
    // Only the end: the log may be large.
    let take = len.min(256 * 1024);
    let mut buf = Vec::with_capacity(take as usize);
    let _ = f.seek(SeekFrom::Start(len - take));
    let _ = f.take(take).read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let mut all: Vec<&str> = text.lines().collect();
    if len > take && !all.is_empty() {
        all.remove(0); // cut in the middle
    }
    // Only the log's own records (one JSON object with a tool, each): the
    // path may name any file, and nothing else in it is shown.
    let before = all.len();
    all.retain(|l| {
        serde_json::from_str::<Value>(l).is_ok_and(|v| v.get("tool").is_some_and(Value::is_string))
    });
    let other = before - all.len();
    let from = all.len().saturating_sub(lines.clamp(1, 500));
    let lines: Vec<&str> = all[from..].iter().map(|l| l.trim_end()).collect();
    json!({"ok": true, "enabled": cfg.audit.enabled, "file": path.display().to_string(), "lines": lines, "exists": true, "size": len, "other_lines": other})
}

/// Sample paths of the pointer's gliding in a style (the real routes the
/// pointer takes, drawn from the program's own path maker).
pub fn path_preview(style: &str, feel: motion::Feel, real: bool) -> Value {
    let from = (30.0, 190.0);
    let to = (450.0, 50.0);
    let mut rng = motion::Rng::new();
    let styles: Vec<motion::Style> = match motion::Style::named(style) {
        Some(s) => vec![s; 3],
        None => (0..3)
            .map(|_| motion::Style::pick("mixed", &mut rng))
            .collect(),
    };
    let paths: Vec<Value> = styles
        .into_iter()
        .map(|s| {
            // The real mouse jumps to the last stretch of a long reach and
            // travels only that; the overlay's pointer glides the whole way.
            let pts = if real {
                motion::travel_near_feel(from, to, motion::Kind::Reach, s, &mut rng, feel)
            } else {
                motion::travel_feel(from, to, motion::Kind::Reach, s, &mut rng, feel)
            };
            let ms = pts.len() as f64 * motion::STEP.as_secs_f64() * 1000.0;
            let thin: Vec<[f64; 2]> = pts
                .iter()
                .enumerate()
                .filter(|(i, _)| i % 2 == 0 || *i + 1 == pts.len())
                .map(|(_, p)| [(p.0 * 10.0).round() / 10.0, (p.1 * 10.0).round() / 10.0])
                .collect();
            json!({"style": s.name(), "ms": ms.round(), "points": thin})
        })
        .collect();
    json!({"ok": true, "from": [from.0, from.1], "to": [to.0, to.1], "paths": paths, "jumps": real})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_start_near_and_end_on_the_target() {
        for style in ["hand", "sine", "arc", "spring", "spiral", "mixed"] {
            let v = path_preview(style, motion::Feel::default(), false);
            assert_eq!(v["paths"].as_array().unwrap().len(), 3);
            for p in v["paths"].as_array().unwrap() {
                let pts = p["points"].as_array().unwrap();
                assert!(pts.len() > 5, "{style}: {}", pts.len());
                let last = pts.last().unwrap();
                assert_eq!(
                    (last[0].as_f64().unwrap(), last[1].as_f64().unwrap()),
                    (450.0, 50.0)
                );
            }
        }
    }

    #[test]
    fn the_preview_shows_the_real_mouses_feel() {
        let avg = |feel: motion::Feel| {
            let v = path_preview("hand", feel, false);
            v["paths"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["ms"].as_f64().unwrap())
                .sum::<f64>()
                / 3.0
        };
        let normal: f64 = (0..30).map(|_| avg(motion::Feel::default())).sum::<f64>() / 30.0;
        let fast: f64 = (0..30)
            .map(|_| {
                avg(motion::Feel {
                    speed: 2.0,
                    ..Default::default()
                })
            })
            .sum::<f64>()
            / 30.0;
        assert!(fast < normal * 0.65, "{fast} against {normal}");
    }

    #[test]
    fn the_audit_log_is_read_from_its_end() {
        let dir = std::env::temp_dir().join(format!("cu-audit-view-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("audit.log");
        let body: String = (0..1000)
            .map(|i| format!("{{\"tool\":\"click\",\"n\":{i}}}\n"))
            .collect();
        std::fs::write(&log, body).unwrap();
        let mut cfg = Config::default();
        cfg.audit.enabled = true;
        cfg.audit.path = Some(log);
        let v = audit(&cfg, 3);
        let lines: Vec<&str> = v["lines"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(
            lines,
            [
                "{\"tool\":\"click\",\"n\":997}",
                "{\"tool\":\"click\",\"n\":998}",
                "{\"tool\":\"click\",\"n\":999}"
            ]
        );
        // A path that names another file shows nothing of it.
        let other = dir.join("config.toml");
        std::fs::write(&other, "[decision]\napi_key = \"sk-AUDIT-LEAK-1234\"\n").unwrap();
        cfg.audit.path = Some(other);
        let v = audit(&cfg, 50);
        assert!(!v.to_string().contains("sk-AUDIT"), "{v}");
        assert_eq!(v["other_lines"], 2);
        cfg.audit.path = Some(dir.join("nothing.log"));
        assert_eq!(audit(&cfg, 3)["exists"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_tool_list_names_every_tool_with_a_cost() {
        let cfg = Config::default();
        let v = tool_list(&cfg);
        let list = v["tools"].as_array().unwrap();
        assert!(list.len() >= 20, "{}", list.len());
        assert!(list.iter().all(|t| t["tokens"].as_u64().unwrap() > 10));
        let mut cfg = Config::default();
        cfg.tools.disabled = vec!["drag".into()];
        let v = tool_list(&cfg);
        let drag = v["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "drag")
            .unwrap();
        assert_eq!(drag["disabled"], true);
    }
}
