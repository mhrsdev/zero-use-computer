//! The `script` tool: running scripts and serving what they ask for, and
//! saved scripts as tools of their own. (computer-use, by mhrsdev)

use std::sync::atomic::Ordering;
use std::sync::mpsc::RecvTimeoutError;

use serde_json::{Value, json};

use super::*;
use crate::script::{self, Msg, Outcome, Request, Saved};

/// A picture one of a script's tool calls made, kept for `show`.
struct ScriptImage {
    image: EncodedImage,
    pending: Vec<PendingImage>,
    shot: Option<ScreenShot>,
}

/// Pictures a script can still show (older ones are let go).
const KEEP_IMAGES: usize = 16;

/// How long a script that ran out of time gets to end on its own.
const GRACE: Duration = Duration::from_secs(10);

impl<B: Backend> Engine<B> {
    /// The tools to offer: the built-in ones the settings allow, and saved
    /// scripts as tools of their own; with the tool manager on, the base
    /// tools, the categories found so far ("list_changed") and the
    /// manager's own tools.
    pub fn tool_definitions(&mut self) -> Vec<ToolDefinition> {
        use crate::config::ToolManager;
        let all = self.all_tool_definitions();
        let manager = self.store.config.tools.manager;
        if manager == ToolManager::Off {
            return all;
        }
        let mut shown: Vec<ToolDefinition> = all
            .into_iter()
            .filter(|d| {
                crate::tools::BASE_TOOLS.contains(&&*d.name)
                    || self
                        .active_tools
                        .contains(crate::tools::category_of(&d.name))
            })
            .collect();
        shown.extend(crate::tools::manager_definitions(
            manager == ToolManager::Dispatch,
        ));
        shown
    }

    /// Every tool the settings allow, the manager aside.
    pub(super) fn all_tool_definitions(&mut self) -> Vec<ToolDefinition> {
        let mut defs = crate::tools::definitions_from(&self.store.config);
        let cfg = &self.store.config;
        if !(cfg.script.saved_as_tools && cfg.tools.is_enabled("script")) {
            return defs;
        }
        for s in self.scripts.list() {
            if BUILTIN.contains(&s.name.as_str()) || !cfg.tools.is_enabled(&s.name) {
                continue;
            }
            defs.push(ToolDefinition {
                name: s.name.clone().into(),
                title: s.name.clone().into(),
                description: format!(
                    "{} (A saved script; script(show=\"{}\") shows it.)",
                    s.description, s.name
                )
                .into(),
                input_schema: s.input_schema(),
                annotations: json!({"title": s.name, "readOnlyHint": false, "destructiveHint": true, "openWorldHint": true}),
            });
        }
        defs
    }

    /// Whether `name` is a tool: a built-in one (even if switched off in
    /// the settings) or a saved script offered as a tool.
    pub fn has_tool(&mut self, name: &str) -> bool {
        BUILTIN.contains(&name)
            || self.saved_tool(name).is_some()
            || (crate::tools::MANAGER_TOOLS.contains(&name)
                && self.store.config.tools.manager != crate::config::ToolManager::Off)
    }

    /// The saved script a tool call names, when saved scripts are tools.
    pub(super) fn saved_tool(&mut self, name: &str) -> Option<Saved> {
        let cfg = &self.store.config;
        if BUILTIN.contains(&name)
            || crate::tools::MANAGER_TOOLS.contains(&name)
            || !cfg.script.saved_as_tools
            || !cfg.tools.is_enabled("script")
            || !cfg.tools.is_enabled(name)
            || script::valid_name(name).is_err()
        {
            return None;
        }
        self.scripts.get(name)
    }

    pub(super) fn script(&mut self, args: ScriptArgs) -> Result<ToolOutput> {
        let asked = [
            args.code.is_some() && args.save.is_none(),
            args.run.is_some(),
            args.save.is_some(),
            args.list,
            args.show.is_some(),
            args.delete.is_some(),
            args.help,
        ];
        if asked.iter().filter(|a| **a).count() != 1 {
            return Err(Error::InvalidArgs(
                "script takes one of: code (run it now), run (a saved script), save (with code), list, show, delete or help"
                    .into(),
            ));
        }
        if args.help {
            return Ok(ToolOutput::text(script::HELP));
        }
        let dir = self.scripts.dir().display().to_string();
        if args.list {
            let as_tools = self.store.config.script.saved_as_tools;
            let list = self.scripts.list();
            if list.is_empty() {
                return Ok(ToolOutput::text(format!(
                    "No saved scripts yet (they go in {dir}). script(save=name, code, description, params) keeps one."
                )));
            }
            let lines: Vec<String> = list
                .iter()
                .map(|s| {
                    let params: Vec<String> = s.input_schema()["properties"]
                        .as_object()
                        .map(|m| m.keys().cloned().collect())
                        .unwrap_or_default();
                    format!(
                        "- {} ({}): {}",
                        s.name,
                        if params.is_empty() {
                            "no arguments".to_string()
                        } else {
                            params.join(", ")
                        },
                        s.description
                    )
                })
                .collect();
            return Ok(ToolOutput::text(format!(
                "{} saved script{} in {dir}{}:\n{}",
                lines.len(),
                if lines.len() == 1 { "" } else { "s" },
                if as_tools {
                    ", each also a tool of its own"
                } else {
                    ""
                },
                lines.join("\n")
            )));
        }
        let missing = |name: &str| {
            Error::InvalidArgs(format!(
                "no saved script called \"{name}\" (script(list=true) lists them)"
            ))
        };
        if let Some(name) = &args.show {
            let s = self.scripts.get(name).ok_or_else(|| missing(name))?;
            return Ok(ToolOutput::text(format!(
                "{} ({}):\n{}",
                s.name,
                s.path.display(),
                s.code
            )));
        }
        if let Some(name) = &args.delete {
            let path = self.scripts.delete(name).map_err(Error::InvalidArgs)?;
            return Ok(ToolOutput::text(format!(
                "Deleted the saved script \"{name}\" ({}).",
                path.display()
            )));
        }
        if let Some(name) = &args.save {
            let name = name.trim().to_lowercase();
            script::valid_name(&name).map_err(Error::InvalidArgs)?;
            if BUILTIN.contains(&name.as_str()) {
                return Err(Error::InvalidArgs(format!(
                    "\"{name}\" is a built-in tool: give the script another name"
                )));
            }
            let code = args
                .code
                .as_deref()
                .ok_or_else(|| Error::InvalidArgs("save needs code: the script to keep".into()))?;
            script::check(code).map_err(Error::InvalidArgs)?;
            let existed = self.scripts.get(&name).is_some();
            let path = self
                .scripts
                .save(
                    &name,
                    code,
                    args.description.as_deref().unwrap_or(""),
                    args.params.as_ref().unwrap_or(&Value::Null),
                )
                .map_err(Error::InvalidArgs)?;
            let cfg = &self.store.config;
            let tool = if cfg.script.saved_as_tools && cfg.tools.is_enabled(&name) {
                format!(", or as the tool \"{name}\" once the client has refreshed its tool list")
            } else {
                String::new()
            };
            return Ok(ToolOutput::text(format!(
                "{} the script \"{name}\" ({}). Run it with script(run=\"{name}\", args={{...}}){tool}.",
                if existed { "Replaced" } else { "Saved" },
                path.display()
            )));
        }
        let (code, source) = match &args.run {
            Some(name) => {
                let s = self.scripts.get(name).ok_or_else(|| missing(name))?;
                (s.code, Some(s.name))
            }
            None => (args.code.unwrap_or_default(), None),
        };
        let script_args = args.args.unwrap_or(Value::Null);
        if !matches!(script_args, Value::Object(_) | Value::Null) {
            return Err(Error::InvalidArgs(
                "args is a map of the script's arguments: {\"size\": 8}".into(),
            ));
        }
        self.run_script(code, source, script_args, args.data.unwrap_or(Value::Null))
    }

    fn run_script(
        &mut self,
        code: String,
        source: Option<String>,
        args: Value,
        data: Value,
    ) -> Result<ToolOutput> {
        let cfg = self.store.config.script.clone();
        let app_tools = crate::tools::definitions()
            .iter()
            .filter(|d| d.input_schema["properties"].get("app").is_some())
            .map(|d| d.name.to_string())
            .collect();
        let env = script::Env {
            files: cfg.files,
            web: cfg.web,
            library: cfg.library(),
            max_seconds: cfg.max_seconds.max(1),
            // Raised by the engine on the stop key or a cancel.
            stop: Arc::new(AtomicBool::new(self.halted())),
            app_tools,
            decision: self.store.config.decision.clone(),
        };
        let started = Instant::now();
        let running = script::start(script::Job {
            code,
            source: source.clone(),
            args,
            data,
            env,
        })
        .map_err(|e| Error::Internal(format!("can't start the script: {e}")))?;
        let halt = running.halt.clone();
        let seen_before = self.known_screens();
        self.in_script = true;
        let mut images: Vec<Option<ScriptImage>> = Vec::new();
        let mut shown: Option<usize> = None;
        let mut limit = running.deadline + GRACE;
        let outcome = loop {
            if self.halted() {
                halt.store(true, Ordering::SeqCst);
            }
            // Short waits, so the stop key or a cancel reaches the script
            // (and the curl it may be running) at once.
            let wait = limit
                .saturating_duration_since(Instant::now())
                .clamp(Duration::from_millis(20), Duration::from_millis(100));
            match running.rx.recv_timeout(wait) {
                Ok(Msg::Ask(req)) => {
                    let reply = self.script_request(req, &mut images, &mut shown);
                    // Raised before the reply, which the script reads first:
                    // a stop during its last tool call ends it as stopped.
                    if self.halted() {
                        halt.store(true, Ordering::SeqCst);
                    }
                    let _ = running.tx.send(reply);
                }
                Ok(Msg::Done(o)) => break o,
                Err(RecvTimeoutError::Timeout) if Instant::now() < limit => {}
                Err(RecvTimeoutError::Timeout) => {
                    if !running.abort.swap(true, Ordering::SeqCst) {
                        limit = Instant::now() + GRACE;
                        continue;
                    }
                    // It doesn't end: leave it behind (it stops at its next
                    // step, as it is told to).
                    break Outcome {
                        error: Some("the script ran out of time and didn't end".into()),
                        ..Default::default()
                    };
                }
                Err(RecvTimeoutError::Disconnected) => {
                    break Outcome {
                        error: Some("the script ended unexpectedly".into()),
                        ..Default::default()
                    };
                }
            }
        };
        self.in_script = false;
        self.restore_known(seen_before);
        // Only the picture the script shows reaches the model.
        self.pending_images.clear();
        self.pending_screen_shot = None;
        let image = shown
            .and_then(|i| images.get_mut(i).and_then(Option::take))
            .map(|img| {
                self.pending_images = img.pending;
                self.pending_screen_shot = img.shot;
                img.image
            });

        let secs = started.elapsed().as_secs_f64();
        let what = match &source {
            Some(name) => format!("The script \"{name}\""),
            None => "The script".to_string(),
        };
        let took = format!(
            "{secs:.1} s, {} tool call{}",
            outcome.calls,
            if outcome.calls == 1 { "" } else { "s" }
        );
        if outcome.stopped {
            let mut text = self.stopped_error().to_string();
            if !outcome.output.is_empty() {
                text.push_str(&format!(
                    "\nWhat the script printed before that:\n{}",
                    outcome.output.trim_end()
                ));
            }
            return Ok(ToolOutput {
                text,
                image: None,
                is_error: true,
            });
        }
        let mut text = match &outcome.error {
            Some(e) => format!("{what} failed ({took}): {e}"),
            None => format!("{what} ran ({took})."),
        };
        if !outcome.output.is_empty() {
            text.push_str(&format!(
                "\nOutput{}:\n{}",
                if outcome.error.is_some() {
                    " before that"
                } else {
                    ""
                },
                outcome.output.trim_end()
            ));
        }
        if let Some(v) = &outcome.value {
            text.push_str(&format!("\nResult: {v}"));
        }
        Ok(ToolOutput {
            text,
            image,
            is_error: outcome.error.is_some(),
        })
    }

    /// Do what a running script asks.
    fn script_request(
        &mut self,
        req: Request,
        images: &mut Vec<Option<ScriptImage>>,
        shown: &mut Option<usize>,
    ) -> script::Reply {
        let keep =
            |images: &mut Vec<Option<ScriptImage>>, img: ScriptImage, shown: Option<usize>| {
                images.push(Some(img));
                let alive: Vec<usize> = (0..images.len())
                    .filter(|i| images[*i].is_some() && Some(*i) != shown)
                    .collect();
                if alive.len() > KEEP_IMAGES {
                    images[alive[0]] = None;
                }
                images.len() - 1
            };
        match req {
            Request::Tool { name, args } => {
                let before = self.pending_images.len();
                let shot_before = self.pending_screen_shot.take();
                let result = match ToolCall::parse(&name, args) {
                    Ok(ToolCall::Script(_)) => Err(Error::InvalidArgs(
                        "a script can't call the script tool: run(name, args) runs a saved script"
                            .into(),
                    )),
                    Err(Error::UnknownTool(n)) if self.scripts.get(&n).is_some() => {
                        Err(Error::InvalidArgs(format!(
                            "\"{n}\" is a saved script: run(\"{n}\", #{{...}}) runs it"
                        )))
                    }
                    Ok(call) => self.call(call),
                    Err(e) => Err(e),
                };
                let from = before.min(self.pending_images.len());
                let pending: Vec<PendingImage> = self.pending_images.drain(from..).collect();
                let shot = std::mem::replace(&mut self.pending_screen_shot, shot_before);
                Ok(match result {
                    Ok(out) => {
                        let id = out.image.map(|image| {
                            keep(
                                images,
                                ScriptImage {
                                    image,
                                    pending,
                                    shot,
                                },
                                *shown,
                            )
                        });
                        json!({"ok": !out.is_error, "text": out.text, "image": id})
                    }
                    Err(e) => json!({"ok": false, "text": e.to_string(), "image": null}),
                })
            }
            Request::Elements {
                app,
                window,
                role,
                name,
                text,
                editable,
                max,
            } => self
                .script_elements(&app, window.as_deref(), role, name, text, editable, max)
                .map_err(|e| e.to_string()),
            Request::Colors {
                app,
                window,
                points,
            } => self
                .script_colors(&app, window.as_deref(), &points)
                .map_err(|e| e.to_string()),
            Request::Design { name } => self.design_info(&name),
            Request::Show { image } => match images.get(image as usize) {
                Some(Some(_)) => {
                    *shown = Some(image as usize);
                    Ok(Value::Null)
                }
                _ => Err(format!(
                    "picture {image} is gone: a script keeps its last {KEEP_IMAGES} pictures"
                )),
            },
            Request::ShowFile { path } => {
                let cfg = self.store.config.screenshot.clone();
                if self.store.config.text_only || !cfg.enabled {
                    return Err(
                        "pictures are off in the user's settings (text_only / screenshot.enabled)"
                            .into(),
                    );
                }
                let meta = std::fs::metadata(&path)
                    .map_err(|e| format!("can't read {}: {e}", path.display()))?;
                if !meta.is_file() {
                    return Err(format!("{} is not a regular file", path.display()));
                }
                if meta.len() > 64 * 1024 * 1024 {
                    return Err(format!("{} is over 64 MB", path.display()));
                }
                // A small file can still unpack to a huge picture.
                let (w, h) = image::image_dimensions(&path).map_err(|e| {
                    format!(
                        "{} isn't a picture I can read (PNG or JPEG): {e}",
                        path.display()
                    )
                })?;
                if u64::from(w) * u64::from(h) > 50_000_000 {
                    return Err(format!(
                        "{} is {w} x {h} pixels; show pictures up to 50 megapixels",
                        path.display()
                    ));
                }
                let img = image::open(&path)
                    .map_err(|e| {
                        format!(
                            "{} isn't a picture I can read (PNG or JPEG): {e}",
                            path.display()
                        )
                    })?
                    .to_rgba8();
                let (w, h) = img.dimensions();
                let cap = Capture {
                    width: w,
                    height: h,
                    rgba: img.into_raw(),
                    bounds: Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
                };
                let (image, _) = imaging::encode(cap, &cfg).map_err(|e| e.to_string())?;
                let id = keep(
                    images,
                    ScriptImage {
                        image,
                        pending: Vec::new(),
                        shot: None,
                    },
                    *shown,
                );
                *shown = Some(id);
                Ok(Value::Null)
            }
        }
    }

    /// A design's size, cells, layers and paint steps, for a script.
    fn design_info(&mut self, name: &str) -> std::result::Result<Value, String> {
        use crate::design::{StepKind, hex};
        let d = self.design_named(name)?.clone();
        let layers: Vec<Value> = d
            .layers
            .iter()
            .map(|l| {
                let b = d.bbox(l, &mut self.fonts);
                json!({"id": l.id, "box": b.map(|r| [r.x, r.y, r.width, r.height])})
            })
            .collect();
        let steps: Vec<Value> = d
            .steps()
            .iter()
            .enumerate()
            .map(|(k, st)| {
                let (kind, width) = match st.kind {
                    StepKind::Solid => ("solid", None),
                    StepKind::Outline(w) => ("lines", Some(w)),
                    StepKind::Text => ("text", None),
                };
                let ids: Vec<&str> = st.layers.iter().map(|&i| d.layers[i].id.as_str()).collect();
                json!({"step": k + 1, "color": hex(st.color), "kind": kind, "width": width, "layers": ids})
            })
            .collect();
        Ok(json!({
            "name": design_key(name),
            "width": d.width,
            "height": d.height,
            "background": hex(d.background),
            "cell": d.cell,
            "cells": d.cells().describe(),
            "layers": layers,
            "steps": steps,
        }))
    }

    /// The elements of a window that match, as data: what find_element
    /// lists, with their states and boxes (in the x/y click takes).
    #[allow(clippy::too_many_arguments)]
    fn script_elements(
        &mut self,
        app: &str,
        window: Option<&str>,
        role: Option<String>,
        name: Option<String>,
        text: Option<String>,
        editable: bool,
        max: usize,
    ) -> Result<Value> {
        let app = self.resolve_app(app)?;
        let win = self.resolve_window(&app, window, false)?;
        self.observe(&app, &win, false)?;
        let role = role.map(|r| r.to_lowercase());
        let name = name.map(|n| crate::text::fold(&n));
        let text = text.map(|t| crate::text::fold(&t));
        let state = self.state(app.pid)?;
        let coord = state.coord;
        let list = state
            .nodes
            .iter()
            .filter(|n| {
                role.as_deref().is_none_or(|r| n.role == r)
                    && name.as_deref().is_none_or(|q| {
                        n.name
                            .as_deref()
                            .is_some_and(|nm| crate::text::fold(nm).contains(q))
                    })
                    && text.as_deref().is_none_or(|q| node_text(n).contains(q))
                    && (!editable || n.states.editable)
            })
            .take(max)
            .map(|n| {
                let s = &n.states;
                let mut m = json!({
                    "index": n.index,
                    "role": n.role,
                    "name": n.name,
                    "value": n.value,
                    "enabled": s.enabled,
                    "focused": s.focused,
                    "selected": s.selected,
                    "checked": s.checked,
                    "expanded": s.expanded,
                    "editable": s.editable,
                    "actions": n.actions.iter().map(|a| a.name.clone()).collect::<Vec<_>>(),
                });
                if let (Some(b), Some(map)) = (n.bounds, coord) {
                    let (x0, y0) = map.to_image(Point::new(b.x, b.y));
                    let (x1, y1) = map.to_image(Point::new(b.x + b.width, b.y + b.height));
                    let r = |v: f64| (v * 10.0).round() / 10.0;
                    m["x"] = json!(r(x0));
                    m["y"] = json!(r(y0));
                    m["w"] = json!(r(x1 - x0));
                    m["h"] = json!(r(y1 - y0));
                    m["cx"] = json!(r((x0 + x1) / 2.0));
                    m["cy"] = json!(r((y0 + y1) / 2.0));
                }
                m
            })
            .collect();
        Ok(Value::Array(list))
    }

    /// The colours at points of a window (x/y as click takes them); null
    /// where a point is off the window.
    fn script_colors(
        &mut self,
        app: &str,
        window: Option<&str>,
        points: &[(f64, f64)],
    ) -> Result<Value> {
        let app = self.resolve_app(app)?;
        let (cap, map) = self.aim_capture(&app, window)?;
        Ok(Value::Array(
            points
                .iter()
                .take(100_000)
                .map(|&(x, y)| {
                    to_capture(&cap, &map, x, y)
                        .ok()
                        .and_then(|(px, py)| imaging::color_at(&cap, px, py))
                        .map_or(Value::Null, Value::String)
                })
                .collect(),
        ))
    }
}
