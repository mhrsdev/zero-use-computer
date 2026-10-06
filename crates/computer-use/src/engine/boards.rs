//! The design board (`design`) and the 3D scene planner (`scene`).

use super::*;

impl<B: Backend> Engine<B> {
    /// A design on the design board, by name.
    pub(super) fn design_named(
        &self,
        name: &str,
    ) -> std::result::Result<&crate::design::Design, String> {
        let key = design_key(name);
        if let Some((_, d)) = self.designs.iter().rev().find(|(n, _)| *n == key) {
            return Ok(d);
        }
        let known: Vec<&str> = self.designs.iter().map(|(n, _)| n.as_str()).collect();
        Err(if known.is_empty() {
            format!("no design called \"{name}\": make it with the design tool first")
        } else {
            format!(
                "no design called \"{name}\" (designs: {})",
                known.join(", ")
            )
        })
    }

    /// The shapes of one step of a design, fitted into the drawing area
    /// with its proportions kept, and whether they are painted solid.
    pub(super) fn design_shapes(
        &mut self,
        s: &DrawStroke,
        name: &str,
        frame: &crate::draw::Frame,
        area: &str,
    ) -> std::result::Result<(Vec<crate::draw::Shape>, bool), String> {
        use crate::design::StepKind;
        if shape_given(s) {
            return Err(
                "give design on its own (with step, and fill for a solid step), not with a shape"
                    .into(),
            );
        }
        if s.rotate.is_some() || s.repeat.is_some() {
            return Err("a design step can't be turned or repeated: change the design".into());
        }
        if area == "the screenshot" {
            return Err("a design needs canvas or element_index: where it goes".into());
        }
        let d = self.design_named(name)?.clone();
        let steps = d.steps();
        let n = steps.len();
        let step = match s.step {
            Some(k) if k >= 1 && k as usize <= n => &steps[k as usize - 1],
            _ => return Err(format!("\"{name}\" has steps 1 to {n}: give step")),
        };
        if step.kind == StepKind::Text {
            let l = &d.layers[step.layers[0]];
            let (at, size) = l
                .text
                .as_ref()
                .map_or(((0.0, 0.0), 0.0), |t| (t.at, t.size));
            return Err(format!(
                "step {} is the text of {}: type it with the app's text tool at ({:.0}, {:.0}) in the design's units, size {:.0}",
                s.step.unwrap_or(0),
                l.id,
                at.0,
                at.1,
                size
            ));
        }
        // One scale for both sides, from the real size: rounding it first
        // (1.5 x 1 as 2 x 1) stretched the drawing.
        let r = frame.screen_rect();
        let scale = (r.width / d.width).min(r.height / d.height);
        let (w, h) = (d.width * scale, d.height * scale);
        let place =
            crate::types::Rect::new(r.x + (r.width - w) / 2.0, r.y + (r.height - h) / 2.0, w, h);
        let (sx, sy) = (scale, scale);
        let (xa, xb) = (frame.x0.min(frame.x1), frame.x0.max(frame.x1));
        let (ya, yb) = (frame.y0.min(frame.y1), frame.y0.max(frame.y1));
        let lines = d.step_lines(step)?;
        let shapes = lines
            .into_iter()
            .map(|(pts, closed)| {
                let pts = pts
                    .iter()
                    .map(|&(x, y)| {
                        let (fx, fy) =
                            frame.to_frame(Point::new(place.x + x * sx, place.y + y * sy));
                        (fx.clamp(xa, xb), fy.clamp(ya, yb))
                    })
                    .collect();
                crate::draw::Shape::points(pts, closed)
            })
            .collect();
        Ok((shapes, step.kind == StepKind::Solid))
    }

    pub(super) fn scene(&mut self, args: SceneArgs) -> Result<ToolOutput> {
        use crate::scene::num;
        let key = design_key(&args.name);
        if key.is_empty() {
            return Err(Error::InvalidArgs("give the scene a name".into()));
        }
        let seen_key = format!("scene:{key}");
        let mut s = match self.scenes.iter().find(|(n, _)| *n == key) {
            Some((_, s)) => s.clone(),
            None => {
                self.drafts_seen.remove(&seen_key);
                Default::default()
            }
        };
        // All or nothing: the stored scene changes only if every part works.
        s.apply(&args).map_err(Error::InvalidArgs)?;
        self.scenes.retain(|(n, _)| *n != key);
        self.scenes.push((key.clone(), s.clone()));
        if self.scenes.len() > 8 {
            self.scenes.remove(0);
        }
        // Said once, then only what changed ([tree] compact); a call that
        // changes nothing is a look, and gets all of it.
        let looking = args.add.is_none()
            && args.change.is_none()
            && args.remove.is_none()
            && args.mirror.is_none()
            && args.repeat.is_none()
            && args.ground.is_none();
        let prev = self
            .drafts_seen
            .get(&seen_key)
            .filter(|_| self.store.config.tree.compact && !looking)
            .cloned();
        let items = s.items();
        let mut text = match s.bounds() {
            None => format!(
                "Scene \"{key}\": empty. Add objects: add=[{{\"id\", \"shape\", \"size\", \"at\"}}]."
            ),
            Some((lo, hi)) => {
                let head = format!(
                    "Scene \"{key}\": {} object{}, {} x {} x {} (x {} to {}, y {} to {}, z {} to {}), Z up{}.",
                    s.objects.len(),
                    if s.objects.len() == 1 { "" } else { "s" },
                    num(hi[0] - lo[0]),
                    num(hi[1] - lo[1]),
                    num(hi[2] - lo[2]),
                    num(lo[0]),
                    num(hi[0]),
                    num(lo[1]),
                    num(hi[1]),
                    num(lo[2]),
                    num(hi[2]),
                    if s.ground { ", the ground at z 0" } else { "" },
                );
                match &prev {
                    Some(p) => match items_diff(&p.items, &items) {
                        Some(diff) => format!("{head} Since the last answer: {diff}."),
                        None => format!("{head} Objects as before."),
                    },
                    None => format!("{head} {}.", s.listing()),
                }
            }
        };
        let mut checks_said = String::new();
        if !s.objects.is_empty() {
            let checks = s.checks();
            checks_said = checks.join("; ");
            if prev.as_ref().is_some_and(|p| p.checks == checks_said) {
                if !checks.is_empty() {
                    text.push_str("\nChecks as before.");
                }
            } else if checks.is_empty() {
                text.push_str(if s.ground {
                    "\nChecks: everything rests on the ground or on something; nothing runs into anything."
                } else {
                    "\nChecks: all parts are joined; nothing runs into anything."
                });
            } else {
                text.push_str(&format!("\nChecks: {}.", checks.join("; ")));
            }
            let build = self.explain(
                "scene-build",
                "\nTo build it: at is each object's centre, size its full extent on its own x, y, z before rotate turns it (degrees about x, then y, then z). In Blender: Add > Mesh > Cube (box), Cylinder, UV Sphere, Cone, Torus or Plane, then in the sidebar (N) > Item type Location = at, Rotation = rotate and Dimensions = size. Other apps take the same numbers (a box's corner is at minus half its size). Or export=\"obj\" and import the file (File > Import > Wavefront .obj).",
                "\nBuild: Location = at, Rotation = rotate, Dimensions = size; or export=\"obj\".",
            );
            text.push_str(build);
        }
        let view = args.view.unwrap_or_default();
        let look = args.look.unwrap_or([35.0, 25.0]);
        if look.iter().any(|v| !v.is_finite()) {
            return Err(Error::InvalidArgs("look is [turn, tilt] in degrees".into()));
        }
        let ids = args.ids.unwrap_or(true);
        let cfg = self.store.config.screenshot.clone();
        let picture = (!self.store.config.text_only && cfg.enabled
            || args.export == Some(SceneExport::Png))
        .then(|| s.render(view, look, ids));
        if let Some(format) = args.export {
            let written = match format {
                SceneExport::Obj => {
                    // The colours first: the model names their file.
                    let (_, mtl) = s.obj(&key, "");
                    let mtl_path = self
                        .exports
                        .write(&key, "mtl", mtl.as_bytes())
                        .map_err(Error::InvalidArgs)?;
                    let mtl_name = mtl_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let (obj, _) = s.obj(&key, &mtl_name);
                    let path = self
                        .exports
                        .write(&key, "obj", obj.as_bytes())
                        .map_err(Error::InvalidArgs)?;
                    (path, obj.len() + mtl.len(), " with its colours beside it")
                }
                SceneExport::Png => {
                    let pic = picture
                        .clone()
                        .ok_or_else(|| Error::InvalidArgs("nothing to write".into()))?;
                    let mut pm = tiny_skia::Pixmap::new(pic.width, pic.height)
                        .ok_or_else(|| Error::InvalidArgs("nothing to write".into()))?;
                    pm.data_mut().copy_from_slice(&pic.rgba);
                    let bytes = pm
                        .encode_png()
                        .map(imaging::signed_png)
                        .map_err(|e| Error::InvalidArgs(format!("could not write the PNG: {e}")))?;
                    let path = self
                        .exports
                        .write(&key, "png", &bytes)
                        .map_err(Error::InvalidArgs)?;
                    (path, bytes.len(), "")
                }
            };
            text.push_str(&format!(
                "\nExported to {}{} ({} KB). It is temporary: import it into the app now (it is deleted when the server stops).",
                written.0.display(),
                written.2,
                written.1.div_ceil(1024)
            ));
        }
        let mut seen = DraftSeen {
            head: String::new(),
            items,
            checks: checks_said,
            steps: String::new(),
            picture: prev.as_ref().map(|p| p.picture).unwrap_or(0),
            cells: String::new(),
            shot: None,
            marks: None,
        };
        if self.ctx.depth == 1 {
            if prev.is_none() {
                self.note.drafts_full.push(seen_key.clone());
            } else {
                self.note.drafts_diff.push(seen_key.clone());
            }
        }
        let Some(picture) = picture.filter(|_| !self.store.config.text_only && cfg.enabled) else {
            self.drafts_seen.insert(seen_key, seen);
            return Ok(ToolOutput::text(text));
        };
        // The very picture the model has: not sent again.
        let hash = picture_hash(&picture);
        if prev.as_ref().is_some_and(|p| p.picture == hash) {
            text.push_str("\nThe picture is as before.");
            self.drafts_seen.insert(seen_key, seen);
            return Ok(ToolOutput::text(text));
        }
        seen.picture = hash;
        if self.ctx.depth == 1 {
            self.note.drafts_picture.push(seen_key.clone());
        }
        self.drafts_seen.insert(seen_key, seen);
        if !s.objects.is_empty() {
            let long = format!(
                "\nThe picture: front (x right, z up), right (y right, z up) and top (x right, y up) to one scale, a grid line every {}; and a perspective view (look [{}, {}]) with shadows straight down onto the ground. view=\"front\" (or right, top, perspective) shows one bigger.",
                num(s.step()),
                num(look[0]),
                num(look[1])
            );
            let short = format!("\nGrid: a line every {}.", num(s.step()));
            text.push_str(self.explain("scene-picture", &long, &short));
        }
        let (img, _) = imaging::encode(picture, &cfg)?;
        Ok(ToolOutput {
            text,
            image: Some(img),
            is_error: false,
        })
    }

    pub(super) fn design(&mut self, args: DesignArgs) -> Result<ToolOutput> {
        use crate::design::{Design, Extras, StepKind, hex};
        let key = design_key(&args.name);
        if key.is_empty() {
            return Err(Error::InvalidArgs("give the design a name".into()));
        }
        let seen_key = format!("design:{key}");
        let mut d = match self.designs.iter().position(|(n, _)| *n == key) {
            Some(i) => self.designs[i].1.clone(),
            None => {
                self.drafts_seen.remove(&seen_key);
                let [w, h] = args.size.ok_or_else(|| {
                    Error::InvalidArgs(format!(
                        "no design called \"{key}\" yet: give size [width, height] to start one"
                    ))
                })?;
                Design::new(w, h)
            }
        };
        // All or nothing: the stored design changes only if every part works.
        d.apply(&args, &mut self.fonts)
            .map_err(Error::InvalidArgs)?;
        self.designs.retain(|(n, _)| *n != key);
        self.designs.push((key.clone(), d.clone()));
        if self.designs.len() > 8 {
            self.designs.remove(0);
        }
        // Said once, then only what changed ([tree] compact); a call that
        // changes nothing is a look, and gets all of it.
        let looking = args.size.is_none()
            && args.background.is_none()
            && args.margin.is_none()
            && args.cell_size.is_none()
            && args.add.is_none()
            && args.change.is_none()
            && args.remove.is_none()
            && args.mirror.is_none()
            && args.align.is_none()
            && args.distribute.is_none()
            && args.order.is_none()
            // Writing a file shows nothing new.
            && args.export.is_none();
        let stored = self.drafts_seen.get(&seen_key).cloned();
        let prev = stored
            .clone()
            .filter(|_| self.store.config.tree.compact && !looking);
        let head = format!(
            "Design \"{key}\": {} x {}, background {}, margin {}.",
            d.width,
            d.height,
            hex(d.background),
            d.margin,
        );
        let items = d.items(&mut self.fonts);
        let count = format!(
            "{} layer{}",
            d.layers.len(),
            if d.layers.len() == 1 { "" } else { "s" }
        );
        let mut text = match &prev {
            Some(p) => {
                let head = if p.head == head {
                    format!("Design \"{key}\":")
                } else {
                    head.clone()
                };
                match items_diff(&p.items, &items) {
                    Some(diff) => format!("{head} {count}; since the last answer: {diff}."),
                    None => format!("{head} {count}, as before."),
                }
            }
            None => format!(
                "{head} {count}, back to front: {}.",
                if d.layers.is_empty() {
                    "none yet".to_string()
                } else {
                    d.listing(&mut self.fonts)
                }
            ),
        };
        let checks = d.checks(&mut self.fonts);
        let checks_said = checks.join("; ");
        if prev.as_ref().is_some_and(|p| p.checks == checks_said) {
            if !checks.is_empty() {
                text.push_str("\nChecks as before.");
            }
        } else if checks.is_empty() {
            if !d.layers.is_empty() {
                text.push_str("\nChecks: nothing off.");
            }
        } else {
            text.push_str(&format!("\nChecks: {}.", checks.join("; ")));
        }
        let steps = d.steps();
        let show = args.show.clone().unwrap_or_default();
        let list_steps = show.steps
            || self.store.config.tools.design_steps == crate::config::DesignSteps::Always;
        let mut steps_said = prev.as_ref().map(|p| p.steps.clone()).unwrap_or_default();
        if !steps.is_empty() && !list_steps {
            text.push_str(&format!(
                "\n{} step(s) to paint it (show steps=true lists them).",
                steps.len()
            ));
        } else if !steps.is_empty() {
            let list: Vec<String> = steps
                .iter()
                .enumerate()
                .map(|(k, st)| {
                    let ids: Vec<&str> =
                        st.layers.iter().map(|&i| d.layers[i].id.as_str()).collect();
                    let how = match st.kind {
                        StepKind::Solid => "solid".to_string(),
                        StepKind::Outline(w) => format!("lines {w}"),
                        StepKind::Text => "text".to_string(),
                    };
                    format!("{} {} {how} ({})", k + 1, hex(st.color), ids.join(", "))
                })
                .collect();
            // How to paint it is said once; then only the steps.
            let how = self.explain(
                "design-paint",
                " Each: set the colour, then draw(strokes=[{\"design\": <name>, \"step\": n, \"fill\": <brush size>}], canvas=...); a lines step uses a brush that wide; type text steps with the text tool. Or export=\"svg\" / \"png\" and import the file.",
                "",
            );
            let said = format!(
                "background {} first, then steps {}.",
                hex(d.background),
                list.join("; ")
            );
            if prev.as_ref().is_some_and(|p| p.steps == said) {
                text.push_str("\nThe steps to paint it are as before.");
            } else {
                text.push_str(&format!("\nTo paint it in an app: {said}{how}"));
            }
            steps_said = said;
        }
        let mut seen = DraftSeen {
            head,
            items,
            checks: checks_said,
            steps: steps_said,
            // What the model has of the picture holds through a look or a
            // zoom into a cell (only `prev` is set aside for those).
            picture: stored.as_ref().map(|p| p.picture).unwrap_or(0),
            cells: stored.as_ref().map(|p| p.cells.clone()).unwrap_or_default(),
            shot: stored.as_ref().and_then(|p| p.shot.clone()),
            marks: stored.as_ref().and_then(|p| p.marks),
        };
        if self.ctx.depth == 1 {
            if prev.is_none() {
                self.note.drafts_full.push(seen_key.clone());
            } else {
                self.note.drafts_diff.push(seen_key.clone());
            }
        }
        if let Some(format) = args.export {
            let (bytes, ext) = match format {
                ExportFormat::Png => (d.png(&mut self.fonts).map_err(Error::InvalidArgs)?, "png"),
                ExportFormat::Svg => (
                    d.svg(&mut self.fonts)
                        .map_err(Error::InvalidArgs)?
                        .into_bytes(),
                    "svg",
                ),
            };
            let path = self
                .exports
                .write(&key, ext, &bytes)
                .map_err(Error::InvalidArgs)?;
            text.push_str(&format!(
                "\nExported to {} ({} KB). It is temporary: import it into the app now (it is deleted when the server stops).",
                path.display(),
                bytes.len().div_ceil(1024)
            ));
        }
        let cfg = self.store.config.screenshot.clone();
        if self.store.config.text_only || !cfg.enabled {
            self.drafts_seen.insert(seen_key, seen);
            return Ok(ToolOutput::text(text));
        }
        // One cell, magnified: its picture and what is in it.
        if let Some(cell) = &show.cell {
            self.drafts_seen.insert(seen_key, seen);
            let (picture, about) = d
                .render_cell(cell, &mut self.fonts)
                .map_err(Error::InvalidArgs)?;
            text.push_str(&format!("\n{about}"));
            let (img, _) = imaging::encode(picture, &cfg)?;
            return Ok(ToolOutput {
                text,
                image: Some(img),
                is_error: false,
            });
        }
        let extras = Extras {
            grid: show.grid,
            ids: show.ids,
            guides: show.guides,
            cells: show.cells.unwrap_or(true),
        };
        let cells = extras.cells;
        let picture = d
            .render(cfg.max_dimension.clamp(256, 1024), extras, &mut self.fonts)
            .map_err(Error::InvalidArgs)?;
        // The very picture the model has: not sent again.
        let hash = picture_hash(&picture);
        if prev.as_ref().is_some_and(|p| p.picture == hash) {
            text.push_str("\nThe picture is as before.");
            self.drafts_seen.insert(seen_key, seen);
            return Ok(ToolOutput::text(text));
        }
        // Only the part that changed, when the model has this design's
        // picture at this size and with the same marks, and the change is
        // a small part of it: a colour, a move, a few layers.
        let part = prev
            .as_ref()
            .filter(|_| cfg.scope == crate::config::ShotScope::Auto)
            .filter(|p| p.marks == Some(extras))
            .and_then(|p| p.shot.as_deref())
            .and_then(|old| imaging::diff_box(old, &picture, 2))
            .map(|area| imaging::widen(area, 16, 96, picture.width, picture.height))
            .filter(|&(_, _, w, h)| {
                f64::from(w) * f64::from(h)
                    <= 0.5 * f64::from(picture.width) * f64::from(picture.height)
            });
        if let Some((px, py, pw, ph)) = part {
            let scale = f64::from(picture.width) / d.width.max(1e-9);
            let u = |v: u32| f64::from(v) / scale;
            let span = crate::cells::Span {
                x0: u(px),
                x1: u(px + pw),
                y0: u(py),
                y1: u(py + ph),
            };
            let at = format!(
                "x {:.0}–{:.0}, y {:.0}–{:.0} ({})",
                span.x0,
                span.x1,
                span.y0,
                span.y1,
                d.cells().covering(span)
            );
            let long = format!(
                "\nThe picture changed only at {at}: that part is shown, {pw}×{ph} px at the same scale as your last picture of it; the rest is as before."
            );
            let short = format!("\nOnly the part that changed: {at}.");
            let note = self.explain("design-part", &long, &short).to_string();
            text.push_str(&note);
            seen.picture = hash;
            seen.shot = Some(std::sync::Arc::new(picture.clone()));
            self.drafts_seen.insert(seen_key, seen);
            let (img, _) = imaging::encode(imaging::crop(&picture, (px, py, pw, ph)), &cfg)?;
            return Ok(ToolOutput {
                text,
                image: Some(img),
                is_error: false,
            });
        }
        // The cells are as they were: said with the first picture only.
        let described = d.cells().describe();
        if cells && prev.as_ref().is_none_or(|p| p.cells != described) {
            text.push_str(&format!(
                "\nOn the picture, {described} (A1 top-left); show {{\"cell\": \"C4\"}} looks at one closely."
            ));
            seen.cells = described;
        }
        seen.picture = hash;
        seen.marks = Some(extras);
        if self.ctx.depth == 1 {
            self.note.drafts_picture.push(seen_key.clone());
        }
        let (img, _) = imaging::encode(picture.clone(), &cfg)?;
        seen.shot = Some(std::sync::Arc::new(picture));
        self.drafts_seen.insert(seen_key, seen);
        Ok(ToolOutput {
            text,
            image: Some(img),
            is_error: false,
        })
    }
}

/// For a mutating tool, the app to re-inspect afterwards (change reporting).
/// What changed between two listings of a design's layers or a scene's
/// objects (by id, in order): None when nothing did.
fn items_diff(old: &[(String, String)], new: &[(String, String)]) -> Option<String> {
    let before: HashMap<&str, &str> = old.iter().map(|(i, l)| (i.as_str(), l.as_str())).collect();
    let now: HashSet<&str> = new.iter().map(|(i, _)| i.as_str()).collect();
    let changed: Vec<&str> = new
        .iter()
        .filter(|(i, l)| before.get(i.as_str()).is_some_and(|o| o != l))
        .map(|(_, l)| l.as_str())
        .collect();
    let added: Vec<&str> = new
        .iter()
        .filter(|(i, _)| !before.contains_key(i.as_str()))
        .map(|(_, l)| l.as_str())
        .collect();
    let removed: Vec<&str> = old
        .iter()
        .filter(|(i, _)| !now.contains(i.as_str()))
        .map(|(i, _)| i.as_str())
        .collect();
    let mut parts = Vec::new();
    if !changed.is_empty() {
        parts.push(format!("changed {}", changed.join("; ")));
    }
    if !added.is_empty() {
        parts.push(format!("added {}", added.join("; ")));
    }
    if !removed.is_empty() {
        parts.push(format!("removed {}", removed.join(", ")));
    }
    // The order, when it isn't the old one with the new ones on top.
    let expected: Vec<&str> = old
        .iter()
        .map(|(i, _)| i.as_str())
        .filter(|i| now.contains(i))
        .chain(
            new.iter()
                .map(|(i, _)| i.as_str())
                .filter(|i| !before.contains_key(i)),
        )
        .collect();
    let order: Vec<&str> = new.iter().map(|(i, _)| i.as_str()).collect();
    if order != expected {
        parts.push(format!("back to front now {}", order.join(", ")));
    }
    (!parts.is_empty()).then(|| parts.join(". "))
}

/// A picture's identity, to tell whether it is the one sent last.
fn picture_hash(c: &Capture) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (c.width, c.height).hash(&mut h);
    c.rgba.hash(&mut h);
    h.finish()
}
