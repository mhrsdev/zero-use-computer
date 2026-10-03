//! Drawing with the mouse, tracing pictures into paint steps, and exact aiming (`locate`, `snap`).

use super::*;

impl<B: Backend> Engine<B> {
    /// The coordinates a drawing (or a canvas-labelled screenshot) uses:
    /// the latest screenshot's pixels, an element's box (fractions), or a
    /// document on screen in its own units or as a math range.
    pub(super) fn draw_frame(
        &self,
        app: &AppInfo,
        element_index: Option<u32>,
        canvas: Option<&DrawCanvas>,
    ) -> Result<(crate::draw::Frame, &'static str)> {
        use crate::draw::Frame;
        let map = self.state(app.pid).ok().and_then(|s| s.coord);
        let no_map = || {
            Error::InvalidArgs(
                "no current screenshot to draw on: call get_app_state first, or pass element_index"
                    .into(),
            )
        };
        // The element's box (screen coordinates).
        let element = match element_index {
            Some(i) => {
                let h = self.handle_of(app, i)?;
                let b = self
                    .state(app.pid)
                    .ok()
                    .and_then(|s| s.bounds.get(&h))
                    .copied()
                    .filter(|b| !b.is_empty())
                    .ok_or_else(|| {
                        Error::ActionFailed("this element has no on-screen box to draw in".into())
                    })?;
                Some(b)
            }
            None => None,
        };
        let Some(c) = canvas else {
            return Ok(match element {
                Some(b) => (Frame::fractions(b), "the element"),
                None => {
                    let map = map.ok_or_else(no_map)?;
                    (
                        Frame::pixels(map.bounds, map.width, map.height),
                        "the screenshot",
                    )
                }
            });
        };
        let b = match (c.area, element) {
            (Some(_), Some(_)) => {
                return Err(Error::InvalidArgs(
                    "give canvas.box or element_index, not both".into(),
                ));
            }
            (Some([l, t, r, btm]), None) => {
                let map = map.ok_or_else(no_map)?;
                let (a, z) = (map.to_screen(l, t)?, map.to_screen(r, btm)?);
                let b = Rect::new(a.x, a.y, z.x - a.x, z.y - a.y);
                if b.is_empty() {
                    return Err(Error::InvalidArgs(
                        "canvas.box is [left, top, right, bottom], right of left and below top"
                            .into(),
                    ));
                }
                b
            }
            (None, Some(b)) => b,
            (None, None) => {
                return Err(Error::InvalidArgs(
                    "canvas needs box (where the document is in the screenshot) or element_index"
                        .into(),
                ));
            }
        };
        match (c.size, c.range) {
            (Some(_), Some(_)) => Err(Error::InvalidArgs(
                "canvas takes size (document units) or range (math), not both".into(),
            )),
            (Some([w, h]), None) => {
                if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
                    return Err(Error::InvalidArgs(
                        "canvas.size is [width, height], both positive".into(),
                    ));
                }
                Ok((Frame::units(b, w, h), "the document"))
            }
            (None, Some([x0, x1, y0, y1])) => {
                if ![x0, x1, y0, y1].iter().all(|v| v.is_finite()) || x1 <= x0 || y1 <= y0 {
                    return Err(Error::InvalidArgs(
                        "canvas.range is [x min, x max, y min, y max], each max above its min"
                            .into(),
                    ));
                }
                Ok((Frame::range(b, x0, x1, y0, y1), "the plot"))
            }
            (None, None) => Err(Error::InvalidArgs(
                "canvas needs size [width, height] or range [x min, x max, y min, y max]".into(),
            )),
        }
    }

    pub(super) fn draw(&mut self, args: DrawArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
        if args.strokes.is_empty() || args.strokes.len() > DRAW_MAX_STROKES {
            return Err(Error::InvalidArgs(format!(
                "`strokes` needs 1 to {DRAW_MAX_STROKES} strokes"
            )));
        }
        let (frame, area) = self.draw_frame(&app, args.element_index, args.canvas.as_ref())?;
        // A cell size the model chose is checked before anything is drawn.
        frame_cells(&frame, None, args.cell_size)?;
        let mut shapes: Vec<(String, crate::draw::Shape)> = Vec::new();
        // Per shape: painted solid with a brush this wide on screen, and
        // how far past its edge (traces go about one of their pixels past,
        // so no gaps show between neighbours).
        let mut fills: Vec<Option<(f64, f64)>> = Vec::new();
        let unit = {
            let (sx, sy) = frame.scale();
            sx.abs().min(sy.abs())
        };
        for (i, s) in args.strokes.iter().enumerate() {
            let bad = |e: String| Error::InvalidArgs(format!("stroke {}: {e}", i + 1));
            // Shapes, and whether they must be painted solid (a trace or
            // a design's solid step), may not be (a design's line step), or
            // either.
            let (made, solid) = match (&s.trace, &s.design) {
                (Some(_), Some(_)) => return Err(bad("give trace or design, not both".into())),
                (Some(name), None) => (
                    self.trace_shapes(s, name, &frame, area).map_err(bad)?,
                    Some(true),
                ),
                (None, Some(name)) => {
                    let (shapes, solid) = self.design_shapes(s, name, &frame, area).map_err(bad)?;
                    (shapes, Some(solid))
                }
                (None, None) => (draw_shapes(s, &frame).map_err(bad)?, None),
            };
            let what = if s.trace.is_some() {
                "a trace step"
            } else {
                "this design step"
            };
            let fill = match (s.fill, solid) {
                (_, Some(false)) => None,
                (None, Some(true)) => {
                    return Err(bad(format!(
                        "{what} is painted solid: give fill, the brush width"
                    )));
                }
                (None, None) => None,
                (Some(w), _) if w.is_finite() && w > 0.0 => {
                    if w * unit < 2.0 {
                        return Err(bad(format!(
                            "fill {w} is under 2 pixels on screen; use a wider brush"
                        )));
                    }
                    let bleed = match &s.trace {
                        Some(name) => self.trace_pixel(name, &frame),
                        None => 0.0,
                    };
                    Some((w * unit, bleed))
                }
                (Some(_), _) => {
                    return Err(bad("fill is the brush width, a positive number".into()));
                }
            };
            let copies = made.len();
            for (k, shape) in made.into_iter().enumerate() {
                let name = if copies == 1 {
                    format!("stroke {}", i + 1)
                } else {
                    format!("stroke {} (part {})", i + 1, k + 1)
                };
                shapes.push((name, shape));
                fills.push(fill);
            }
            if shapes.len() > DRAW_MAX_STROKES {
                return Err(Error::InvalidArgs(format!(
                    "more than {DRAW_MAX_STROKES} strokes once repeated; draw it in parts"
                )));
            }
        }
        let plan = crate::draw::plan_labelled(&shapes, &frame, DRAW_STEP, DRAW_MAX_POINTS)
            .map_err(Error::InvalidArgs)?;
        let names: Vec<String> = shapes.iter().map(|s| s.0.clone()).collect();
        // Solid shapes narrower than the brush come out bigger.
        let thin: Vec<f64> = plan
            .strokes
            .iter()
            .zip(&plan.shape)
            .filter_map(|(s, &k)| {
                let (w, _) = fills.get(k).copied().flatten()?;
                let width = crate::draw::shape_width(s);
                (width < w).then_some(width)
            })
            .collect();
        let plan = crate::draw::fill_plan(plan, &fills, &names, DRAW_STEP, DRAW_MAX_POINTS)
            .map_err(|e| {
                if e.contains("pointer positions") {
                    Error::InvalidArgs(format!("{e}, or paint with a wider brush (fill)"))
                } else {
                    Error::InvalidArgs(e)
                }
            })?;
        let speed = args
            .speed
            .filter(|s| s.is_finite())
            .unwrap_or(DRAW_SPEED)
            .clamp(50.0, 5000.0);
        let secs = plan.length / speed;
        if secs > DRAW_MAX_SECS {
            return Err(Error::InvalidArgs(format!(
                "this drawing would take about {secs:.0} s at {speed:.0} pixels per second; raise speed or draw it in parts"
            )));
        }
        let (Some(first), Some(last)) = (
            plan.strokes.first().and_then(|s| s.first()).copied(),
            plan.strokes.last().and_then(|s| s.last()).copied(),
        ) else {
            return Err(Error::InvalidArgs("nothing to draw".into()));
        };
        let mut summary = self.draw_summary(&frame, area, &plan, args.cell_size);
        if let Some(w) = fills.iter().flatten().map(|f| f.0).min_by(f64::total_cmp) {
            summary.push_str(&format!(
                " Solid shapes are painted for a brush {w:.0} px wide on screen; a smaller brush leaves stripes."
            ));
        }
        if let Some(narrowest) = thin.iter().copied().min_by(f64::total_cmp) {
            summary.push_str(&format!(
                " {} of the solid shapes are narrower than the brush and come out bigger (the narrowest is about {:.0} wide): for fine detail use a smaller brush.",
                thin.len(),
                narrowest / unit.max(1e-9)
            ));
        }
        if args.preview {
            let cap = self.window_capture(&app, args.window.as_deref())?;
            let fill_note = self.fill_report(&app, cap.clone(), &plan, &shapes, &fills, true);
            return self.draw_preview(
                cap,
                &frame,
                &plan,
                args.cell_size,
                format!("{summary}{fill_note}"),
            );
        }

        self.overlay_point(first, true);
        let target = self.input_target(&app)?;
        // Paced to `speed`, and stoppable between any two moves: the stop
        // key ends the drawing (the backend lets go of the button).
        let (stop, cancel) = (self.stop.clone(), self.cancel.clone());
        let stop_name = self.stop_control_name();
        let sleep = &self.sleep;
        let mut owed = 0.0f64;
        // The agent cursor follows the pen: the backends report each move's
        // length, so how far along the strokes the pen is gives where.
        let overlay = self.overlay.as_ref();
        let along = PathPosition::new(&plan.strokes);
        let (mut travelled, mut shown_at) = (0.0f64, Instant::now());
        let mut pace = |d: f64| -> Result<()> {
            if stop.load(Ordering::SeqCst) {
                return Err(Error::Stopped(stop_name.clone()));
            }
            if cancel.load(Ordering::SeqCst) {
                return Err(Error::Cancelled);
            }
            travelled += d;
            if let Some(o) = overlay
                && shown_at.elapsed() >= Duration::from_millis(60)
                && let Some(p) = along.at(travelled)
            {
                shown_at = Instant::now();
                o.send(&OverlayCmd::Pointer {
                    x: p.x,
                    y: p.y,
                    click: false,
                    id: None,
                });
            }
            owed += d / speed;
            if owed >= 0.004 {
                sleep(Duration::from_secs_f64(owed.min(0.25)));
                owed = 0.0;
            }
            Ok(())
        };
        let drawn = self
            .backend
            .draw(&target, &plan.strokes, args.button, &mut pace);
        self.last_input = Some((self.clock)());
        self.overlay_glide(last);
        drawn?;
        self.settle_on(&app);

        let cap = self
            .window_capture(&app, args.window.as_deref())
            .ok()
            .flatten();
        let fill_note = self.fill_report(&app, cap, &plan, &shapes, &fills, false);
        let mut msg = format!("Drew {summary}{fill_note}");
        msg.push_str(self.explain(
            "draw-check",
            " Drawing changes pixels, which the accessibility tree doesn't show: check the result with a screenshot (screenshot: true).",
            " Check it with a screenshot.",
        ));
        Ok(ToolOutput::text(msg))
    }

    /// "3 strokes (…) on the plot: x … to …, y … to …." plus what fell
    /// outside.
    fn draw_summary(
        &self,
        frame: &crate::draw::Frame,
        area: &str,
        plan: &crate::draw::Plan,
        cell_size: Option<f64>,
    ) -> String {
        // Where it goes, in the coordinates the model used.
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in plan.strokes.iter().flatten() {
            let (x, y) = frame.to_frame(*p);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        let digits = match area {
            "the element" | "the plot" => 2,
            _ => 0,
        };
        let n = plan.strokes.len();
        let mut msg = format!(
            "{n} stroke{} ({} pointer positions, {:.0} px of line) on {area}: x {x0:.digits$} to {x1:.digits$}, y {y0:.digits$} to {y1:.digits$}.",
            if n == 1 { "" } else { "s" },
            plan.points(),
            plan.length,
        );
        if let Some(t) = plan_span(frame, plan) {
            let (cells, _) = drawing_cells(frame, Some(t), cell_size);
            msg.push_str(&format!(
                " It covers cells {} (cells of {}, A1 top-left).",
                cells.covering(t),
                imaging::grid_label(cells.step, cells.step)
            ));
        }
        if plan.skipped > 0 {
            msg.push_str(&format!(
                " {} sample(s) of a curve were outside {area} and were not drawn.",
                plan.skipped
            ));
        }
        // Circles come out as ellipses when a unit of x and of y differ on
        // screen: say so, with the numbers to fix it.
        let (sx, sy) = frame.scale();
        let (ux, uy) = (sx.abs(), sy.abs());
        if area != "the screenshot" && ux > 0.0 && uy > 0.0 && (ux / uy - 1.0).abs() > 0.02 {
            msg.push_str(&format!(
                " On screen 1 unit of x is {ux:.1} px and 1 unit of y is {uy:.1} px, so circles look like ellipses; for equal units make the canvas's width/height match its box's."
            ));
        }
        msg
    }

    /// A picture traced with trace_image, by name.
    fn trace_named(&self, name: &str) -> std::result::Result<&crate::paint::Trace, String> {
        if let Some((_, t)) = self
            .traces
            .iter()
            .rev()
            .find(|(n, _)| n.eq_ignore_ascii_case(name.trim()))
        {
            return Ok(t);
        }
        let known: Vec<&str> = self.traces.iter().map(|(n, _)| n.as_str()).collect();
        Err(if known.is_empty() {
            format!("no picture called \"{name}\": trace it with trace_image first")
        } else {
            format!(
                "no picture called \"{name}\" (traced: {})",
                known.join(", ")
            )
        })
    }

    /// How big one of a traced picture's working pixels is on screen,
    /// fitted into `frame` (how far apart its neighbouring shapes' edges
    /// may be).
    fn trace_pixel(&self, name: &str, frame: &crate::draw::Frame) -> f64 {
        self.trace_named(name).map_or(0.0, |t| {
            let place = crate::paint::fit_in(frame.screen_rect(), t.width, t.height);
            place.width / f64::from(t.target.width.max(1))
        })
    }

    /// The shapes of one step of a traced picture, fitted into the
    /// drawing area with its proportions kept.
    fn trace_shapes(
        &self,
        s: &DrawStroke,
        name: &str,
        frame: &crate::draw::Frame,
        area: &str,
    ) -> std::result::Result<Vec<crate::draw::Shape>, String> {
        if shape_given(s) {
            return Err("give trace on its own (with step and fill), not with a shape".into());
        }
        if s.rotate.is_some() || s.repeat.is_some() {
            return Err("a trace can't be turned or repeated".into());
        }
        if area == "the screenshot" {
            return Err("a trace needs canvas or element_index: where the picture goes".into());
        }
        let t = self.trace_named(name)?;
        let n = t.steps.len();
        let step = match s.step {
            Some(k) if k >= 1 && k as usize <= n => k as usize,
            _ => return Err(format!("\"{name}\" has steps 1 to {n}: give step")),
        };
        let place = crate::paint::fit_in(frame.screen_rect(), t.width, t.height);
        let (xa, xb) = (frame.x0.min(frame.x1), frame.x0.max(frame.x1));
        let (ya, yb) = (frame.y0.min(frame.y1), frame.y0.max(frame.y1));
        Ok(t.steps[step - 1]
            .regions
            .iter()
            .map(|r| {
                let pts = r
                    .iter()
                    .map(|&(u, v)| {
                        let (x, y) = frame.to_frame(Point::new(
                            place.x + u * place.width,
                            place.y + v * place.height,
                        ));
                        (x.clamp(xa, xb), y.clamp(ya, yb))
                    })
                    .collect();
                crate::draw::Shape::points(pts, true)
            })
            .collect())
    }

    pub(super) fn trace_image(&mut self, args: TraceImageArgs) -> Result<ToolOutput> {
        use crate::tools::TraceDetail;
        let colors = args.colors.unwrap_or(8);
        if !(2..=16).contains(&colors) {
            return Err(Error::InvalidArgs("colors is 2 to 16".into()));
        }
        let (src, source, default_name) = match (&args.path, &args.app) {
            (Some(_), Some(_)) => {
                return Err(Error::InvalidArgs(
                    "give path (an image file) or app (what its window shows), not both".into(),
                ));
            }
            (None, None) => {
                return Err(Error::InvalidArgs(
                    "give path (an image file) or app (trace what its window shows)".into(),
                ));
            }
            (Some(path), None) => {
                let p = std::path::Path::new(path.trim());
                let meta = std::fs::metadata(p)
                    .map_err(|e| Error::InvalidArgs(format!("can't read {path}: {e}")))?;
                if !meta.is_file() {
                    return Err(Error::InvalidArgs(format!("{path} is not a regular file")));
                }
                let size = meta.len();
                if size > 64 * 1024 * 1024 {
                    return Err(Error::InvalidArgs(format!(
                        "{path} is {} MB; give an image under 64 MB",
                        size / (1024 * 1024)
                    )));
                }
                // A small file can still unpack to a huge picture.
                let (w, h) = image::image_dimensions(p).map_err(|e| {
                    Error::InvalidArgs(format!(
                        "{path} isn't a picture I can read (PNG or JPEG): {e}"
                    ))
                })?;
                if u64::from(w) * u64::from(h) > 50_000_000 {
                    return Err(Error::InvalidArgs(format!(
                        "{path} is {w} x {h} pixels; trace pictures up to 50 megapixels"
                    )));
                }
                let img = image::open(p)
                    .map_err(|e| {
                        Error::InvalidArgs(format!(
                            "{path} isn't a picture I can read (PNG or JPEG): {e}"
                        ))
                    })?
                    .to_rgba8();
                let (w, h) = img.dimensions();
                let file = p
                    .file_name()
                    .map_or_else(|| path.clone(), |f| f.to_string_lossy().into_owned());
                let stem = p
                    .file_stem()
                    .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
                (
                    Capture {
                        width: w,
                        height: h,
                        rgba: img.into_raw(),
                        bounds: Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
                    },
                    file,
                    stem,
                )
            }
            (None, Some(query)) => {
                if self.store.config.text_only || !self.store.config.screenshot.enabled {
                    return Err(Error::Blocked(
                        "trace_image".into(),
                        "screenshots are disabled (text_only / screenshot.enabled=false)".into(),
                    ));
                }
                let app = self.resolve_app(query)?;
                let window = self.resolve_window(&app, args.window.as_deref(), false)?;
                let part = match (args.area, args.element_index) {
                    (Some(_), Some(_)) => {
                        return Err(Error::InvalidArgs(
                            "give box or element_index, not both".into(),
                        ));
                    }
                    (Some([l, t, r, b]), None) => {
                        let map = self.state(app.pid).ok().and_then(|s| s.coord).ok_or_else(|| {
                            Error::InvalidArgs(
                                "box is in screenshot pixels: call get_app_state (or screenshot) first"
                                    .into(),
                            )
                        })?;
                        let (a, z) = (map.to_screen(l, t)?, map.to_screen(r, b)?);
                        let rect = Rect::new(a.x, a.y, z.x - a.x, z.y - a.y);
                        if rect.is_empty() {
                            return Err(Error::InvalidArgs(
                                "box is [left, top, right, bottom], right of left and below top"
                                    .into(),
                            ));
                        }
                        Some(rect)
                    }
                    (None, Some(i)) => {
                        self.observe(&app, &window, false)?;
                        let node = self.node_by_index(&app, i)?;
                        Some(node.bounds.filter(|b| !b.is_empty()).ok_or_else(|| {
                            Error::InvalidArgs(format!(
                                "element {i} ({}) has no on-screen area",
                                node.label()
                            ))
                        })?)
                    }
                    (None, None) => None,
                };
                let mut cap = self.capture_clean(|b| b.capture(&app, &window))?;
                self.redact_capture(&mut cap);
                if let Some(r) = part {
                    let sx = f64::from(cap.width) / cap.bounds.width.max(1e-9);
                    let sy = f64::from(cap.height) / cap.bounds.height.max(1e-9);
                    let x0 = ((r.x - cap.bounds.x) * sx).max(0.0);
                    let y0 = ((r.y - cap.bounds.y) * sy).max(0.0);
                    let x1 = ((r.x + r.width - cap.bounds.x) * sx).min(f64::from(cap.width));
                    let y1 = ((r.y + r.height - cap.bounds.y) * sy).min(f64::from(cap.height));
                    if x1 - x0 < 4.0 || y1 - y0 < 4.0 {
                        return Err(Error::InvalidArgs(
                            "that part of the window is (almost) off the picture".into(),
                        ));
                    }
                    cap = imaging::crop(
                        &cap,
                        (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32),
                    );
                }
                (
                    cap,
                    format!("{} window \"{}\"", app.name, window.title),
                    app.name.clone(),
                )
            }
        };
        let detail = match args.detail.unwrap_or_default() {
            TraceDetail::Low => crate::paint::Detail::LOW,
            TraceDetail::Medium => crate::paint::Detail::MEDIUM,
            TraceDetail::High => crate::paint::Detail::HIGH,
        };
        let t = crate::paint::trace(&src, colors as usize, detail);
        // A short name the model can repeat.
        let name: String = args
            .name
            .clone()
            .unwrap_or(default_name)
            .trim()
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .chars()
            .take(40)
            .collect();
        let name = if name.is_empty() {
            "picture".to_string()
        } else {
            name
        };
        let regions: usize = t.steps.iter().map(|s| s.regions.len()).sum();
        let mut text = format!(
            "Traced \"{name}\" ({source}, {} x {} px) as {} steps of flat colour, {regions} shapes in all; the picture shows the result. Paint the steps in order, back to front: for each, set the app's colour to its hex, then draw.",
            t.width,
            t.height,
            t.steps.len()
        );
        for (k, s) in t.steps.iter().enumerate() {
            text.push_str(&format!(
                "\n{}. {}: {} shape{}, {:.0}% of the picture",
                k + 1,
                crate::paint::hex(s.color),
                s.regions.len(),
                if s.regions.len() == 1 { "" } else { "s" },
                s.share * 100.0
            ));
            if k == 0 && s.share > 0.9 {
                text.push_str(" (all of it: on an empty canvas one bucket click does this step)");
            }
        }
        // How to paint and check it is said once; then only the call.
        if self.explain_first("trace-paint") {
            text.push_str(&format!(
                "\nEach step: draw(app, canvas=<the document's box and size>, strokes=[{{\"trace\": \"{name}\", \"step\": n, \"fill\": <the app's brush size>}}]). The picture keeps its proportions ({} x {}): give the canvas the same, or it is centred with margins. Afterwards screenshot(app, canvas=..., compare=\"{name}\") shows where the canvas still differs.",
                t.width, t.height
            ));
        } else {
            text.push_str(&format!(
                "\nEach step: draw strokes=[{{\"trace\": \"{name}\", \"step\": n, \"fill\": ...}}] on a {} x {} canvas; check with compare=\"{name}\".",
                t.width, t.height
            ));
        }
        let cfg = self.store.config.screenshot.clone();
        let (pw, ph) = imaging::fit(t.width, t.height, 512);
        let picture = crate::paint::render(&t, pw, ph);
        self.traces.retain(|(n, _)| n != &name);
        self.traces.push((name, t));
        if self.traces.len() > 8 {
            self.traces.remove(0);
        }
        if self.store.config.text_only || !cfg.enabled {
            return Ok(ToolOutput::text(text));
        }
        let (img, _) = imaging::encode(picture, &cfg)?;
        Ok(ToolOutput {
            text,
            image: Some(img),
            is_error: false,
        })
    }

    /// How the document in `cap` compares with a traced picture: how many
    /// cells of an 8 x 8 grid look alike, and the most different ones.
    pub(super) fn compare_note(
        &self,
        name: &str,
        frame: &crate::draw::Frame,
        cap: &Capture,
    ) -> std::result::Result<String, String> {
        const CELLS: usize = 8;
        /// ΔE under this looks alike.
        const ALIKE: f64 = 12.0;
        let t = self.trace_named(name)?;
        let place = crate::paint::fit_in(frame.screen_rect(), t.width, t.height);
        let (sx, sy) = (
            f64::from(cap.width) / cap.bounds.width.max(1e-9),
            f64::from(cap.height) / cap.bounds.height.max(1e-9),
        );
        let area = Rect::new(
            (place.x - cap.bounds.x) * sx,
            (place.y - cap.bounds.y) * sy,
            place.width * sx,
            place.height * sy,
        );
        let mut cells = crate::paint::compare(t, cap, area, CELLS);
        if cells.is_empty() {
            return Err("the canvas is not in this picture".into());
        }
        let alike = cells.iter().filter(|c| c.delta < ALIKE).count();
        let mean = cells.iter().map(|c| c.delta).sum::<f64>() / cells.len() as f64;
        let mut msg = format!(
            "\nCompared with \"{name}\": {alike} of {} cells look alike (mean colour difference {mean:.0}; under {ALIKE:.0} looks alike).",
            cells.len()
        );
        cells.sort_by(|a, b| b.delta.total_cmp(&a.delta));
        let worst: Vec<String> = cells
            .iter()
            .filter(|c| c.delta >= ALIKE)
            .take(4)
            .map(|c| {
                let at = |i: usize, j: usize| {
                    frame.to_frame(Point::new(
                        place.x + place.width * i as f64 / CELLS as f64,
                        place.y + place.height * j as f64 / CELLS as f64,
                    ))
                };
                let (a, b) = (at(c.col, c.row), at(c.col + 1, c.row + 1));
                format!(
                    "x {:.0} to {:.0}, y {:.0} to {:.0} should be {} but is {}",
                    a.0.min(b.0),
                    a.0.max(b.0),
                    a.1.min(b.1),
                    a.1.max(b.1),
                    crate::paint::hex(c.want),
                    crate::paint::hex(c.got)
                )
            })
            .collect();
        if !worst.is_empty() {
            msg.push_str(&format!(" Most different: {}.", worst.join("; ")));
        }
        Ok(msg)
    }

    /// A fresh, clean picture of the window to aim with, and the map
    /// between the x/y actions take (the latest get_app_state screenshot)
    /// and the screen.
    pub(super) fn aim_capture(
        &mut self,
        app: &AppInfo,
        window: Option<&str>,
    ) -> Result<(Capture, CoordMap)> {
        if self.store.config.text_only || !self.store.config.screenshot.enabled {
            return Err(Error::Blocked(
                "pixel targeting".into(),
                "it reads the screen, and screenshots are disabled (text_only / screenshot.enabled=false)"
                    .into(),
            ));
        }
        let map = self
            .state(app.pid)
            .ok()
            .and_then(|s| s.coord)
            .ok_or_else(|| {
                Error::InvalidArgs(
                    "x/y are in the pixels of get_app_state's screenshot: call get_app_state first"
                        .into(),
                )
            })?;
        let window = self.resolve_window(app, window, false)?;
        let mut cap = self.capture_clean(|b| b.capture(app, &window))?;
        self.redact_capture(&mut cap);
        Ok((cap, map))
    }

    /// The x/y point snapped to `how` ("corner", "edge", "center", a
    /// colour) within `radius` screenshot pixels, and a note saying how
    /// far it moved.
    pub(super) fn snap_xy(
        &mut self,
        app: &AppInfo,
        window: Option<&str>,
        x: f64,
        y: f64,
        how: &str,
        radius: Option<f64>,
    ) -> Result<(f64, f64, String)> {
        use crate::target::{Feature, snap};
        let feature = Feature::parse(how).map_err(Error::InvalidArgs)?;
        let (cap, map) = self.aim_capture(app, window)?;
        let r = radius.unwrap_or(10.0).clamp(1.0, 100.0);
        let at = to_capture(&cap, &map, x, y)?;
        let per = to_capture(&cap, &map, x + 1.0, y)?.0 - at.0;
        let found = snap(&cap, at, r * per.abs().max(1e-9), feature).ok_or_else(|| {
            Error::InvalidArgs(format!(
                "no {} within {r} pixels of ({x}, {y}), so nothing was done. Look with screenshot(app, zoom=[{x}, {y}]), or leave out snap",
                feature.name()
            ))
        })?;
        let (nx, ny) = from_capture(&cap, &map, found);
        Ok((
            nx,
            ny,
            format!(
                " Snapped to the {} at ({nx:.1}, {ny:.1}), {}.",
                feature.name(),
                moved(nx - x, ny - y)
            ),
        ))
    }

    pub(super) fn locate(&mut self, args: LocateArgs) -> Result<ToolOutput> {
        use crate::target::{Feature, colour_blobs, look_alikes, snap};
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
        let asked = [
            args.color.is_some(),
            args.like.is_some(),
            args.near.is_some(),
        ];
        if asked.iter().filter(|a| **a).count() != 1 {
            return Err(Error::InvalidArgs(
                "give one of color (areas of a colour), like (look-alikes of a box) or near with feature (a corner, edge or centre next to a point)".into(),
            ));
        }
        let (cap, map) = self.aim_capture(&app, args.window.as_deref())?;
        let whole = Rect::new(0.0, 0.0, f64::from(cap.width), f64::from(cap.height));
        let to_rect = |b: [f64; 4]| -> Result<Rect> {
            let [l, t, r, bt] = b;
            let a = to_capture(&cap, &map, l, t)?;
            let z = to_capture(&cap, &map, r, bt)?;
            let rect = Rect::new(
                a.0.min(z.0),
                a.1.min(z.1),
                (z.0 - a.0).abs(),
                (z.1 - a.1).abs(),
            );
            if rect.is_empty() {
                return Err(Error::InvalidArgs(
                    "a box is [left, top, right, bottom], right of left and below top".into(),
                ));
            }
            Ok(rect)
        };
        let area = match args.area {
            Some(b) => to_rect(b)?,
            None => whole,
        };
        let back = |p: (f64, f64)| from_capture(&cap, &map, p);
        let span = |r: Rect| {
            let (a, z) = (back((r.x, r.y)), back((r.x + r.width, r.y + r.height)));
            format!("box {:.0},{:.0} to {:.0},{:.0}", a.0, a.1, z.0, z.1)
        };
        // Results: (text, screen box to mark).
        let mut marks: Vec<Rect> = Vec::new();
        let screen_rect = |r: Rect| {
            let b = cap.bounds;
            let (sx, sy) = (
                f64::from(cap.width) / b.width.max(1e-9),
                f64::from(cap.height) / b.height.max(1e-9),
            );
            Rect::new(b.x + r.x / sx, b.y + r.y / sy, r.width / sx, r.height / sy)
        };
        let text = if let Some(c) = &args.color {
            let colour = crate::design::parse_colour(c)
                .map_err(Error::InvalidArgs)?
                .ok_or_else(|| Error::InvalidArgs("color must be a colour, not none".into()))?;
            let tol = args.tolerance.unwrap_or(16.0).clamp(0.0, 255.0) as i32;
            let blobs = colour_blobs(&cap, area, colour, tol);
            let shown: Vec<String> = blobs
                .iter()
                .take(10)
                .enumerate()
                .map(|(k, b)| {
                    marks.push(screen_rect(b.bbox));
                    let (x, y) = back(b.center);
                    format!("{} at ({x:.1}, {y:.1}), {}", k + 1, span(b.bbox))
                })
                .collect();
            if shown.is_empty() {
                format!(
                    "No {} (within {tol} per channel) here.",
                    crate::design::hex(colour)
                )
            } else {
                format!(
                    "{}{} area{} of {} (within {tol}), biggest first: {}{}.",
                    // Only the biggest are kept of a picture full of specks.
                    if blobs.len() >= crate::target::MAX_BLOBS {
                        "At least "
                    } else {
                        ""
                    },
                    blobs.len(),
                    if blobs.len() == 1 { "" } else { "s" },
                    crate::design::hex(colour),
                    shown.join("; "),
                    if blobs.len() > 10 {
                        "; and smaller ones"
                    } else {
                        ""
                    }
                )
            }
        } else if let Some(b) = args.like {
            let template = to_rect(b)?;
            let found = look_alikes(&cap, template, area, 0.85).map_err(Error::InvalidArgs)?;
            let shown: Vec<String> = found
                .iter()
                .enumerate()
                .map(|(k, m)| {
                    marks.push(screen_rect(m.bbox));
                    let (x, y) = back((
                        m.bbox.x + m.bbox.width / 2.0,
                        m.bbox.y + m.bbox.height / 2.0,
                    ));
                    let same =
                        (m.bbox.x - template.x).abs() < 2.0 && (m.bbox.y - template.y).abs() < 2.0;
                    format!(
                        "{} at ({x:.1}, {y:.1}), match {:.0}%{}",
                        k + 1,
                        m.score * 100.0,
                        if same { " (the one you gave)" } else { "" }
                    )
                })
                .collect();
            if shown.is_empty() {
                "Nothing here looks like that box.".to_string()
            } else {
                format!(
                    "{} place{} look like it, best first (centres): {}.",
                    found.len(),
                    if found.len() == 1 { "" } else { "s" },
                    shown.join("; ")
                )
            }
        } else {
            let p = args.near.map(|p| p.xy()).unwrap_or_default();
            let feature = Feature::parse(args.feature.as_deref().unwrap_or("center"))
                .map_err(Error::InvalidArgs)?;
            let r = args.radius.unwrap_or(12.0).clamp(1.0, 100.0);
            let at = to_capture(&cap, &map, p.0, p.1)?;
            let per = to_capture(&cap, &map, p.0 + 1.0, p.1)?.0 - at.0;
            match snap(&cap, at, r * per.abs().max(1e-9), feature) {
                Some(q) => {
                    let (x, y) = back(q);
                    let mark = 4.0 * per.abs().max(1.0);
                    marks.push(screen_rect(Rect::new(
                        q.0 - mark,
                        q.1 - mark,
                        2.0 * mark,
                        2.0 * mark,
                    )));
                    format!(
                        "The {} near ({}, {}): ({x:.1}, {y:.1}), {}.",
                        feature.name(),
                        p.0,
                        p.1,
                        moved(x - p.0, y - p.1)
                    )
                }
                None => format!(
                    "No {} within {r} pixels of ({}, {}).",
                    feature.name(),
                    p.0,
                    p.1
                ),
            }
        };
        let text = format!("{text} Coordinates are the x/y click takes.");
        let cfg = self.store.config.screenshot.clone();
        if marks.is_empty() || !args.picture.unwrap_or(cfg.locate_picture) {
            return Ok(ToolOutput::text(text));
        }
        // The window with each place found numbered, to check before acting.
        let mut shown = cap;
        let numbered: Vec<(u32, Rect)> = marks
            .into_iter()
            .enumerate()
            .map(|(k, r)| (k as u32 + 1, r))
            .collect();
        imaging::annotate(&mut shown, &numbered);
        let (img, _) = imaging::encode(shown, &cfg)?;
        Ok(ToolOutput {
            text,
            image: Some(img),
            is_error: false,
        })
    }

    /// A clean picture of the window (private areas blacked out), or
    /// `None` when screenshots are off.
    fn window_capture(&mut self, app: &AppInfo, window: Option<&str>) -> Result<Option<Capture>> {
        if self.store.config.text_only || !self.store.config.screenshot.enabled {
            return Ok(None);
        }
        let window = self.resolve_window(app, window, false)?;
        let mut cap = self.capture_clean(|b| b.capture(app, &window))?;
        self.redact_capture(&mut cap);
        Ok(Some(cap))
    }

    /// Where a bucket click (or magic wand) fills each closed outline of
    /// the drawing, in the x/y click takes: read off the pixels of `cap`
    /// (with the strokes added first when they are only `planned`), so it
    /// knows when other lines cut a shape into pieces or a gap lets a
    /// fill run out. Without a picture, from the shapes alone.
    fn fill_report(
        &self,
        app: &AppInfo,
        cap: Option<Capture>,
        plan: &crate::draw::Plan,
        shapes: &[(String, crate::draw::Shape)],
        fills: &[Option<(f64, f64)>],
        planned: bool,
    ) -> String {
        let solid = |i: usize| {
            plan.shape
                .get(i)
                .and_then(|&k| fills.get(k))
                .is_some_and(Option::is_some)
        };
        let targets = crate::draw::fill_targets(&plan.strokes, 20, |i| !solid(i));
        let Some(map) = self.state(app.pid).ok().and_then(|s| s.coord) else {
            return String::new();
        };
        if targets.is_empty() {
            return String::new();
        }
        let name = |i: usize| {
            plan.shape
                .get(i)
                .and_then(|&k| shapes.get(k))
                .map_or("a stroke", |(n, _)| n.as_str())
        };
        let mut cap = cap;
        let (to_cap, to_screen) = match &cap {
            Some(c) => {
                let (b, sx, sy) = (
                    c.bounds,
                    f64::from(c.width) / c.bounds.width.max(1e-9),
                    f64::from(c.height) / c.bounds.height.max(1e-9),
                );
                (
                    Some(move |p: Point| Point::new((p.x - b.x) * sx, (p.y - b.y) * sy)),
                    Some(move |p: Point| Point::new(b.x + p.x / sx, b.y + p.y / sy)),
                )
            }
            None => (None, None),
        };
        // Not drawn yet: put the strokes on the picture, as paint.
        if planned && let (Some(c), Some(to_cap)) = (cap.as_mut(), to_cap) {
            let sx = f64::from(c.width) / c.bounds.width.max(1e-9);
            for (i, stroke) in plan.strokes.iter().enumerate() {
                let width = plan
                    .shape
                    .get(i)
                    .and_then(|&k| fills.get(k).copied().flatten())
                    .map_or(2.0, |(w, _)| w * sx);
                let pts: Vec<(f64, f64)> = stroke
                    .iter()
                    .map(|p| {
                        let q = to_cap(*p);
                        (q.x, q.y)
                    })
                    .collect();
                imaging::draw_path(c, &pts, [0, 0, 0], width.round().max(2.0) as i64);
            }
        }
        let show = |p: Point| {
            let (x, y) = map.to_image(p);
            format!("({x:.0}, {y:.0})")
        };
        let mut clicks: Vec<String> = Vec::new();
        let mut leaks: Vec<String> = Vec::new();
        for t in targets {
            let checked = match (&cap, to_cap, to_screen) {
                (Some(c), Some(to_cap), Some(_)) => {
                    let outline: Vec<Point> =
                        plan.strokes[t.stroke].iter().map(|p| to_cap(*p)).collect();
                    let holes: Vec<Vec<Point>> = t
                        .holes
                        .iter()
                        .map(|&h| plan.strokes[h].iter().map(|p| to_cap(*p)).collect())
                        .collect();
                    let hole_refs: Vec<&[Point]> = holes.iter().map(Vec::as_slice).collect();
                    crate::paint::fill_check(c, &outline, &hole_refs, to_cap(t.point))
                }
                _ => None,
            };
            let back = |p: Point| to_screen.map_or(p, |f| f(p));
            match checked {
                Some(f) if f.leak.is_some() => {
                    let at = back(f.leak.unwrap_or(t.point));
                    leaks.push(format!(
                        "{} would leak out through a gap near {}: close the outline first",
                        name(t.stroke),
                        show(at)
                    ));
                }
                Some(f) if f.clicks.len() > 1 => {
                    let n = f.clicks.len();
                    let at: Vec<String> = f.clicks.iter().take(5).map(|&p| show(back(p))).collect();
                    let more = if n > 5 {
                        format!(" and {} smaller", n - 5)
                    } else {
                        String::new()
                    };
                    clicks.push(format!(
                        "{}, cut into {n} pieces by other lines, at {}{more}",
                        name(t.stroke),
                        at.join(", ")
                    ));
                }
                _ => clicks.push(format!("{} at {}", name(t.stroke), show(t.point))),
            }
        }
        let mut msg = String::new();
        if !clicks.is_empty() {
            msg.push_str(&format!(
                " To fill a closed outline (bucket or magic wand), click {}.",
                clicks.join("; ")
            ));
        }
        for l in leaks {
            let mut l = l;
            if let Some(first) = l.get(..1) {
                l = first.to_uppercase() + &l[1..];
            }
            msg.push_str(&format!(" {l}."));
        }
        msg
    }

    /// The strokes in red over a screenshot of the window, with a grid in
    /// the coordinates the call used; nothing is drawn.
    fn draw_preview(
        &mut self,
        cap: Option<Capture>,
        frame: &crate::draw::Frame,
        plan: &crate::draw::Plan,
        cell_size: Option<f64>,
        summary: String,
    ) -> Result<ToolOutput> {
        let legend = self.explain(
            "draw-preview",
            " Red: the strokes (green: where each starts); the blue cells (A1 top-left, their lines labelled) are in the coordinates you gave. Call draw again without preview to draw them.",
            "",
        );
        let text = format!("Preview only, nothing was drawn: {summary}{legend}");
        let Some(mut cap) = cap else {
            return Ok(ToolOutput::text(text));
        };
        let cfg = self.store.config.screenshot.clone();
        let (out_w, _) = imaging::fit(cap.width, cap.height, cfg.max_dimension.max(64));
        let out_scale = f64::from(cap.width) / f64::from(out_w.max(1));
        let (bounds, cw, ch) = (cap.bounds, f64::from(cap.width), f64::from(cap.height));
        let to_cap = |p: Point| {
            (
                (p.x - bounds.x) * cw / bounds.width.max(1e-9),
                (p.y - bounds.y) * ch / bounds.height.max(1e-9),
            )
        };
        let r = frame.screen_rect();
        let (cx0, cy0) = to_cap(Point::new(r.x, r.y));
        let (cx1, cy1) = to_cap(Point::new(r.x + r.width, r.y + r.height));
        let (ax, ay) = LabelSpace::Frame(*frame).axes(&cap, out_w);
        // Graph paper sized to the drawing: over it (and a cell round it)
        // when it is small in the area, else over the whole area.
        let (cells, sized) = drawing_cells(frame, plan_span(frame, plan), cell_size);
        let mut clip = Rect::new(cx0, cy0, cx1 - cx0, cy1 - cy0);
        if let (true, Some(t)) = (sized, plan_span(frame, plan)) {
            let e = cells.step;
            let a = to_cap(frame.to_screen(t.x0 - e, t.y0 - e));
            let b = to_cap(frame.to_screen(t.x1 + e, t.y1 + e));
            let (x0, x1) = (a.0.min(b.0).max(cx0), a.0.max(b.0).min(cx1));
            let (y0, y1) = (a.1.min(b.1).max(cy0), a.1.max(b.1).min(cy1));
            clip = Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0));
        }
        cells.draw(&mut cap, ax, ay, out_scale, Some(clip));
        let width = (2.0 * out_scale).round().max(2.0) as i64;
        for stroke in &plan.strokes {
            let pts: Vec<(f64, f64)> = stroke.iter().map(|p| to_cap(*p)).collect();
            imaging::draw_path(&mut cap, &pts, [255, 30, 60], width);
            if let Some(&start) = pts.first() {
                imaging::draw_path(&mut cap, &[start], [0, 200, 70], width * 3);
            }
        }
        let (img, _) = imaging::encode(cap, &cfg)?;
        Ok(ToolOutput {
            text,
            image: Some(img),
            is_error: false,
        })
    }
}

/// A `draw` stroke as the drawing module takes it: one shape, or several
/// (axes and ticks, repeated copies).
pub(crate) fn draw_shapes(
    s: &DrawStroke,
    frame: &crate::draw::Frame,
) -> std::result::Result<Vec<crate::draw::Shape>, String> {
    use crate::draw::{self, Affine, Expr, Shape};
    let curve = s.x.is_some() || s.y.is_some();
    let kinds = [
        ("points", s.points.is_some()),
        ("x/y", curve),
        ("rect", s.rect.is_some()),
        ("ellipse", s.ellipse.is_some()),
        ("polygon", s.polygon.is_some()),
        ("star", s.star.is_some()),
        ("arc", s.arc.is_some()),
        ("bezier", s.bezier.is_some()),
        ("axes", s.axes.is_some()),
    ];
    let given: Vec<&str> = kinds.iter().filter(|k| k.1).map(|k| k.0).collect();
    match given.len() {
        0 => {
            return Err(
                "give a shape (rect, ellipse, polygon, star, arc, bezier), points, axes, or x and/or y expressions in t"
                    .into(),
            );
        }
        1 => {}
        _ => {
            return Err(format!(
                "give one kind of stroke, not {}",
                given.join(" and ")
            ));
        }
    }
    let finite = |v: &[f64], what: &str| -> std::result::Result<(), String> {
        if v.iter().all(|x| x.is_finite()) {
            Ok(())
        } else {
            Err(format!("{what} has a number that isn't finite"))
        }
    };
    let whole = |v: f64, what: &str, lo: u32, hi: u32| -> std::result::Result<u32, String> {
        if v.fract() == 0.0 && v >= f64::from(lo) && v <= f64::from(hi) {
            Ok(v as u32)
        } else {
            Err(format!("{what} must be a whole number from {lo} to {hi}"))
        }
    };
    let circle = |cx: f64, cy: f64, rx: f64, ry: f64, t0: f64, t1: f64| {
        Ok::<Shape, String>(Shape::Curve {
            x: Expr::parse(&format!("({cx:?}) + ({rx:?})*cos(t)"))?,
            y: Expr::parse(&format!("({cy:?}) + ({ry:?})*sin(t)"))?,
            t0,
            t1,
            steps: None,
            transform: Affine::IDENTITY,
        })
    };
    let base: Shape = if let Some(r) = &s.rect {
        if !(r.len() == 4 || r.len() == 5) {
            return Err(
                "rect is [x, y, width, height] or [x, y, width, height, corner radius]".into(),
            );
        }
        finite(r, "rect")?;
        let (x, y, w, h) = (r[0], r[1], r[2], r[3]);
        let radius = r.get(4).copied().unwrap_or(0.0);
        if w <= 0.0 || h <= 0.0 || radius < 0.0 {
            return Err("rect needs a positive width and height (and radius 0 or more)".into());
        }
        Shape::points(draw::rect(x, y, w, h, radius), true)
    } else if let Some(e) = s.ellipse {
        let [cx, cy, rx, ry] = e;
        finite(&e, "ellipse")?;
        if rx <= 0.0 || ry <= 0.0 {
            return Err(
                "ellipse is [center x, center y, radius x, radius y] with positive radii".into(),
            );
        }
        // Drawn from its rightmost point, all the way round.
        circle(cx, cy, rx, ry, 0.0, std::f64::consts::TAU)?
    } else if let Some(p) = s.polygon {
        let [cx, cy, r, n] = p;
        finite(&p, "polygon")?;
        if r <= 0.0 {
            return Err("polygon is [center x, center y, radius, corners], radius positive".into());
        }
        let n = whole(n, "polygon corners", 3, 1000)?;
        Shape::points(draw::polygon(cx, cy, r, n, frame.y_up()), true)
    } else if let Some(p) = s.star {
        let [cx, cy, outer, inner, n] = p;
        finite(&p, "star")?;
        if outer <= 0.0 || inner <= 0.0 {
            return Err(
                "star is [center x, center y, outer radius, inner radius, points], radii positive"
                    .into(),
            );
        }
        let n = whole(n, "star points", 2, 500)?;
        Shape::points(draw::star(cx, cy, outer, inner, n, frame.y_up()), true)
    } else if let Some(a) = s.arc {
        let [cx, cy, r, from, to] = a;
        finite(&a, "arc")?;
        if r <= 0.0 {
            return Err(
                "arc is [center x, center y, radius, from degrees, to degrees], radius positive"
                    .into(),
            );
        }
        circle(cx, cy, r, r, from.to_radians(), to.to_radians())?
    } else if let Some(b) = &s.bezier {
        let pts: Vec<(f64, f64)> = b.iter().map(|p| p.xy()).collect();
        finite(
            &pts.iter().flat_map(|p| [p.0, p.1]).collect::<Vec<_>>(),
            "bezier",
        )?;
        Shape::Points {
            points: draw::bezier(&pts)?,
            closed: s.closed,
            smooth: false,
            transform: Affine::IDENTITY,
        }
    } else if let Some(a) = s.axes {
        if !frame.y_up() {
            return Err("axes need canvas.range (math coordinates)".into());
        }
        if s.rotate.is_some() || s.repeat.is_some() {
            return Err("axes can't be turned or repeated".into());
        }
        finite(&a, "axes")?;
        return draw::axes(frame, a[0].max(0.0), a[1].max(0.0), 10.0);
    } else if let Some(points) = &s.points {
        Shape::Points {
            points: points.iter().map(|p| p.xy()).collect(),
            closed: s.closed,
            smooth: s.smooth,
            transform: Affine::IDENTITY,
        }
    } else {
        // A curve: x and y in t, or a plot of y in x (x in y) over the
        // whole area unless t says otherwise.
        let (x, y, default_t) = match (&s.x, &s.y) {
            (Some(x), Some(y)) => (
                Expr::parse(x).map_err(|e| format!("x: {e}"))?,
                Expr::parse(y).map_err(|e| format!("y: {e}"))?,
                (0.0, 1.0),
            ),
            (None, Some(y)) => (
                Expr::parse("t")?,
                Expr::parse_in(y, Some("x")).map_err(|e| format!("y: {e}"))?,
                (frame.x0, frame.x1),
            ),
            (Some(x), None) => (
                Expr::parse_in(x, Some("y")).map_err(|e| format!("x: {e}"))?,
                Expr::parse("t")?,
                (frame.y0, frame.y1),
            ),
            (None, None) => unreachable!("one of x and y is given"),
        };
        let bound = |v: &DrawNumber, which: &str| -> std::result::Result<f64, String> {
            match v {
                DrawNumber::Num(n) => Ok(*n),
                DrawNumber::Expr(e) => Expr::parse(e)
                    .map(|e| e.eval(0.0))
                    .map_err(|err| format!("t {which}: {err}")),
            }
        };
        let (t0, t1) = match &s.t {
            Some([a, b]) => (bound(a, "from")?, bound(b, "to")?),
            None => default_t,
        };
        if let Some(n) = s.steps
            && !(1..=10_000).contains(&n)
        {
            return Err("steps must be 1 to 10000".into());
        }
        Shape::Curve {
            x,
            y,
            t0,
            t1,
            steps: s.steps,
            transform: Affine::IDENTITY,
        }
    };
    let center = base.center().unwrap_or((0.0, 0.0));
    let mut shape = base;
    if let Some(deg) = s.rotate {
        let about = s.about.map(|p| p.xy()).unwrap_or(center);
        finite(&[deg, about.0, about.1], "rotate")?;
        shape = shape.then(frame.rotation(about, deg));
    }
    let Some(r) = &s.repeat else {
        return Ok(vec![shape]);
    };
    if r.count == 0 || r.count > 500 {
        return Err("repeat.count must be 1 to 500".into());
    }
    let [dx, dy] = r.offset.unwrap_or([0.0, 0.0]);
    let turn = r.rotate.unwrap_or(0.0);
    let about = r.about.map(|p| p.xy()).unwrap_or(center);
    finite(&[dx, dy, turn, about.0, about.1], "repeat")?;
    Ok((0..r.count)
        .map(|k| {
            let k = f64::from(k);
            shape
                .then(Affine::translate(k * dx, k * dy))
                .then(frame.rotation(about, k * turn))
        })
        .collect())
}

/// The box a drawing covers, in its frame's units.
fn plan_span(frame: &crate::draw::Frame, plan: &crate::draw::Plan) -> Option<crate::cells::Span> {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in plan.strokes.iter().flatten() {
        let (x, y) = frame.to_frame(*p);
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    (x0 <= x1).then_some(crate::cells::Span { x0, x1, y0, y1 })
}

/// Cells for a drawing on `frame`: sized to the drawing when it is small
/// in the area (and then `true`), else to the whole area.
fn drawing_cells(
    frame: &crate::draw::Frame,
    target: Option<crate::cells::Span>,
    cell_size: Option<f64>,
) -> (crate::cells::Cells, bool) {
    // The size the model chose (checked before drawing) is kept as is.
    if let Ok(c) = frame_cells(frame, None, cell_size)
        && cell_size.is_some()
    {
        return (c, false);
    }
    let (w, h) = ((frame.x1 - frame.x0).abs(), (frame.y1 - frame.y0).abs());
    let small = target.filter(|t| {
        let (tw, th) = (t.x1 - t.x0, t.y1 - t.y0);
        tw.max(th) > 0.0 && tw < w * 0.35 && th < h * 0.35
    });
    (
        crate::cells::Cells::new(frame.x0, frame.x1, frame.y0, frame.y1, frame.y_up(), small),
        small.is_some(),
    )
}

/// Where along a drawing's strokes the pen is, by how far it has moved
/// along them (the jumps between strokes aren't moves).
pub(super) struct PathPosition {
    /// Every point, with how far along the strokes it is.
    points: Vec<(f64, Point)>,
}

impl PathPosition {
    pub(super) fn new(strokes: &[Vec<Point>]) -> Self {
        let mut points = Vec::new();
        let mut far = 0.0;
        for stroke in strokes {
            let mut last: Option<Point> = None;
            for &p in stroke {
                if let Some(l) = last {
                    far += (p.x - l.x).hypot(p.y - l.y);
                }
                points.push((far, p));
                last = Some(p);
            }
        }
        Self { points }
    }

    /// The point the pen has reached after moving `far`.
    pub(super) fn at(&self, far: f64) -> Option<Point> {
        let i = self.points.partition_point(|(d, _)| *d < far);
        self.points
            .get(i)
            .or_else(|| self.points.last())
            .map(|(_, p)| *p)
    }
}
