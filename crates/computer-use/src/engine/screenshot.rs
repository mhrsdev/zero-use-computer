//! The `screenshot` tool: screen, region, window or element, with grids, cells, zoom, palettes and comparisons.

use super::*;

impl<B: Backend> Engine<B> {
    pub(super) fn screenshot(&mut self, args: ScreenshotArgs) -> Result<ToolOutput> {
        if self.store.config.text_only || !self.store.config.screenshot.enabled {
            return Err(Error::Blocked(
                "screenshot".into(),
                "screenshots are disabled (text_only / screenshot.enabled=false)".into(),
            ));
        }
        // x/y/width/height are a part of the screen: they make the mode
        // region, and are never silently ignored.
        let region = [args.x, args.y, args.width, args.height]
            .iter()
            .any(Option::is_some);
        let mode = match args.mode {
            Some(m) if region && m != ScreenshotMode::Region => {
                return Err(Error::InvalidArgs(format!(
                    "x, y, width and height give a part of the screen (mode=region); mode={} doesn't take them. For part of a window, use element_index (one element) or zoom=[x, y].",
                    mode_name(m)
                )));
            }
            Some(m) => m,
            None if region && args.app.is_some() => {
                return Err(Error::InvalidArgs(
                    "x, y, width and height are a part of the screen in screen coordinates (mode=region, without app). For part of a window, use element_index (one element) or zoom=[x, y].".into(),
                ));
            }
            None if region => ScreenshotMode::Region,
            None if args.app.is_some() => ScreenshotMode::Window,
            None => ScreenshotMode::Auto,
        };
        // An element to zoom into (screen rect).
        let mut zoom: Option<Rect> = None;
        // A plain picture of a window whose screen the model knows: (pid,
        // screen), so an unchanged picture isn't sent again.
        let mut known_window: Option<(u32, u32)> = None;
        // What grid labels and `pick` points are in: screen coordinates,
        // or the x/y actions use for the window.
        let mut space = LabelSpace::Screen;
        if args.canvas.is_some() && mode != ScreenshotMode::Window {
            return Err(Error::InvalidArgs(
                "canvas needs a window screenshot (app)".into(),
            ));
        }
        let (capture, marks, label) = match mode {
            ScreenshotMode::Auto | ScreenshotMode::Full => (
                self.capture_clean(|b| b.capture_screen(None))?,
                None,
                "full screen".to_string(),
            ),
            ScreenshotMode::Region => {
                let region = match (args.x, args.y, args.width, args.height) {
                    (Some(x), Some(y), Some(w), Some(h))
                        if [x, y, w, h].iter().all(|v| v.is_finite()) && w > 0.0 && h > 0.0 =>
                    {
                        self.clip_to_screens(Rect::new(x, y, w, h))?
                    }
                    _ => {
                        return Err(Error::InvalidArgs(
                            "region mode needs x, y, width and height".into(),
                        ));
                    }
                };
                let (x, y, w, h) = (region.x, region.y, region.width, region.height);
                (
                    self.capture_clean(|b| b.capture_screen(Some(Rect::new(x, y, w, h))))?,
                    None,
                    format!("region ({x:.0}, {y:.0}) {w:.0}x{h:.0}"),
                )
            }
            ScreenshotMode::Window => {
                let query = args
                    .app
                    .as_deref()
                    .ok_or_else(|| Error::InvalidArgs("window mode needs `app`".into()))?;
                let app = self.resolve_app(query)?;
                let window = self.resolve_window(&app, args.window.as_deref(), false)?;
                if crate::privacy::active(&self.store.config.privacy)
                    || args.annotate
                    || args.element_index.is_some()
                {
                    // A current tree: where private data is, the marks, the element.
                    self.observe(&app, &window, false)?;
                }
                let plain = !args.annotate
                    && args.element_index.is_none()
                    && args.grid.is_none()
                    && !args.palette
                    && args.pick.is_none()
                    && args.canvas.is_none()
                    && args.compare.is_none()
                    && !args.cells
                    && args.cell.is_none()
                    && args.zoom.is_none();
                let cache = &self.store.config.cache;
                if plain
                    && (cache.dedupe_screenshots
                        || self.store.config.screenshot.scope == crate::config::ShotScope::Auto)
                {
                    // Which screen this is, to compare with the picture the
                    // model has of it. (An app whose tree can't be read
                    // still gets its picture, in full.)
                    let read = crate::privacy::active(&self.store.config.privacy)
                        || self.observe(&app, &window, false).is_ok();
                    if read
                        && let Some(st) = self.states.get(&app.pid)
                        && st.window_id == Some(window.id)
                        && st.known.as_ref().is_some_and(|k| k.id == st.screen)
                    {
                        known_window = Some((app.pid, st.screen));
                    }
                }
                let mut label = format!("{} window \"{}\"", app.name, window.title);
                if let Some(i) = args.element_index {
                    let node = self.node_by_index(&app, i)?;
                    zoom = Some(node.bounds.filter(|b| !b.is_empty()).ok_or_else(|| {
                        Error::InvalidArgs(format!(
                            "element {i} ({}) has no on-screen area to show",
                            node.label()
                        ))
                    })?);
                    label = format!("{} in {}", node.label(), app.name);
                }
                // The picture this call's look just took, if any.
                let cap = match self.last_capture.take() {
                    Some((pid, wid, epoch, cap))
                        if pid == app.pid && wid == window.id && epoch == self.epoch =>
                    {
                        cap
                    }
                    _ => self.capture_clean(|b| b.capture(&app, &window))?,
                };
                space = match self.states.get(&app.pid) {
                    Some(st) if st.window_id == Some(window.id) => {
                        st.coord.map_or(LabelSpace::Image, LabelSpace::Map)
                    }
                    _ => LabelSpace::Image,
                };
                // A document's own units or a math range, as draw takes them.
                if let Some(c) = &args.canvas {
                    let element = if c.area.is_none() {
                        args.element_index
                    } else {
                        None
                    };
                    space = LabelSpace::Frame(self.draw_frame(&app, element, Some(c))?.0);
                }
                let marks = if args.annotate {
                    Some(
                        self.state(app.pid)?
                            .nodes
                            .iter()
                            .filter_map(|n| n.bounds.map(|b| (n.index, b)))
                            .collect::<Vec<_>>(),
                    )
                } else {
                    None
                };
                (cap, marks, label)
            }
        };

        let mut capture = capture;
        let redacted = self.redact_capture(&mut capture);
        let mut note = if redacted > 0 {
            format!(" [{redacted} private area(s) blacked out]")
        } else {
            String::new()
        };
        let cfg = self.store.config.screenshot.clone();
        let image = |img: EncodedImage, text: String| ToolOutput {
            text,
            image: Some(img),
            is_error: false,
        };

        // A loupe: magnified around a point, to aim.
        if let Some(z) = args.zoom {
            let LabelSpace::Map(map) = space else {
                return Err(Error::InvalidArgs(
                    "zoom is for a window screenshot (app) after get_app_state: its x/y are in that screenshot's pixels".into(),
                ));
            };
            let (x, y) = z.xy();
            let at = to_capture(&capture, &map, x, y)?;
            let per_x = to_capture(&capture, &map, x + 1.0, y)?.0 - at.0;
            let per_y = to_capture(&capture, &map, x, y + 1.0)?.1 - at.1;
            if at.0 < 0.0
                || at.1 < 0.0
                || at.0 >= f64::from(capture.width)
                || at.1 >= f64::from(capture.height)
            {
                return Err(Error::InvalidArgs(format!(
                    "({x}, {y}) is outside the window"
                )));
            }
            let per_screen = f64::from(capture.width) / capture.bounds.width.max(1e-9);
            let r = (args.radius.unwrap_or(12.0).clamp(3.0, 64.0) * per_screen)
                .round()
                .max(2.0) as usize;
            let k = (480 / (2 * r + 1)).clamp(2, 32);
            let (mut pic, origin) = crate::target::loupe(&capture, at.0, at.1, r, k)
                .ok_or_else(|| Error::InvalidArgs("nothing to magnify there".into()))?;
            let (ox, oy) = from_capture(&capture, &map, (origin.0 as f64, origin.1 as f64));
            let ax = imaging::Axis {
                offset: ox,
                scale: 1.0 / (k as f64 * per_x.abs().max(1e-9)),
            };
            let ay = imaging::Axis {
                offset: oy,
                scale: 1.0 / (k as f64 * per_y.abs().max(1e-9)),
            };
            let used = imaging::draw_grid(&mut pic, ax, ay, 0.0, 1.0, None);
            let under = imaging::color_at(&capture, at.0, at.1).unwrap_or_default();
            let (img, _) = imaging::encode(pic, &cfg)?;
            let text = if self.explain_first("loupe") {
                format!(
                    "Magnified around ({x}, {y}) in {label}: each square is one pixel of the screen picture ({:.2} of the x/y click takes); the crosshair is the point, on {under}. The grid is labelled in the x/y click takes, a line every {used}: read an exact point off it.{note}",
                    1.0 / per_x.abs().max(1e-9)
                )
            } else {
                format!(
                    "Magnified around ({x}, {y}) in {label}: a square a pixel ({:.2} of x/y); crosshair on {under}; grid in x/y, a line every {used}.{note}",
                    1.0 / per_x.abs().max(1e-9)
                )
            };
            return Ok(image(img, text));
        }

        // Zoomed in on one element, at up to full resolution.
        if let Some(b) = zoom {
            let sx = f64::from(capture.width) / capture.bounds.width.max(1.0);
            let sy = f64::from(capture.height) / capture.bounds.height.max(1.0);
            let px = (
                ((b.x - capture.bounds.x) * sx).max(0.0) as u32,
                ((b.y - capture.bounds.y) * sy).max(0.0) as u32,
                (b.width * sx).ceil() as u32,
                (b.height * sy).ceil() as u32,
            );
            let px = imaging::widen(px, 8, 0, capture.width, capture.height);
            capture = imaging::crop(&capture, px);
        }

        // Exact colours and coordinates, read before anything is drawn on it.
        let extras = args.grid.is_some() || args.palette || args.pick.is_some();
        let sig = matches!(mode, ScreenshotMode::Auto | ScreenshotMode::Full)
            .then(|| PixelSig::of(&capture, self.store.config.cache.pixel_grid));
        let (out_w, _) = imaging::fit(capture.width, capture.height, cfg.max_dimension.max(64));
        let (ax, ay) = space.axes(&capture, out_w);
        if args.palette {
            let colours: Vec<String> = imaging::palette(&capture, 8)
                .iter()
                .map(|(hex, share)| format!("{hex} {:.0}%", share * 100.0))
                .collect();
            note.push_str(&format!("\nMain colours: {}.", colours.join(", ")));
        }
        if let Some(points) = &args.pick {
            let read: Vec<String> = points
                .iter()
                .take(50)
                .map(|p| {
                    let (x, y) = p.xy();
                    match imaging::color_at(&capture, ax.pixel(x), ay.pixel(y)) {
                        Some(hex) => format!("({x}, {y}) {hex}"),
                        None => format!("({x}, {y}) is outside the image"),
                    }
                })
                .collect();
            note.push_str(&format!("\nColours: {}.", read.join("; ")));
        }
        if let Some(name) = &args.compare {
            let LabelSpace::Frame(f) = space else {
                return Err(Error::InvalidArgs(
                    "compare needs canvas: where the picture is on screen".into(),
                ));
            };
            note.push_str(
                &self
                    .compare_note(name, &f, &capture)
                    .map_err(Error::InvalidArgs)?,
            );
        }
        // Graph paper over a document: its cells, or one cell up close.
        let mut cells = None;
        if args.cells || args.cell.is_some() {
            let LabelSpace::Frame(f) = space else {
                return Err(Error::InvalidArgs(
                    "cells need canvas: where the document is on screen".into(),
                ));
            };
            let c = frame_cells(&f, None, args.cell_size)?;
            if let Some(name) = &args.cell {
                let (col, row) = c.parse(name).map_err(Error::InvalidArgs)?;
                let (pic, text) = cell_view(&capture, ax, ay, &c, col, row)?;
                let (img, _) = imaging::encode(pic, &cfg)?;
                return Ok(image(img, format!("{text}{note}")));
            }
            cells = Some((c, f));
        }
        if let Some(marks) = marks {
            imaging::annotate(&mut capture, &marks);
        }
        if let Some(step) = args.grid {
            let out_scale = f64::from(capture.width) / f64::from(out_w.max(1));
            // A document's grid covers just the document.
            let clip = match space {
                LabelSpace::Frame(f) => Some(capture_rect(&capture, f.screen_rect())),
                _ => None,
            };
            let used = imaging::draw_grid(&mut capture, ax, ay, step, out_scale, clip);
            note.push_str(&format!(
                "\nGrid: a line every {}, labelled in {}.",
                used,
                space.describe()
            ));
        }
        if let Some((c, f)) = cells {
            let out_scale = f64::from(capture.width) / f64::from(out_w.max(1));
            let clip = capture_rect(&capture, f.screen_rect());
            c.draw(&mut capture, ax, ay, out_scale, Some(clip));
            note.push_str(&format!(
                "\nCells over the document: {} (A1 top-left); cell=\"C4\" looks at one closely.",
                c.describe()
            ));
        }

        if zoom.is_some() {
            let (img, _) = imaging::encode(capture, &cfg)?;
            let how = self.explain(
                "zoomed",
                " It is its own picture: x/y for actions still refer to get_app_state's screenshot.",
                "",
            );
            let text = format!(
                "Screenshot of {label}, zoomed in: {}x{} px.{how}{note}",
                img.width, img.height
            );
            return Ok(image(img, text));
        }

        // The whole screen, or only what changed since the last one.
        if let Some(sig) = sig {
            let tolerance = self.store.config.cache.pixel_tolerance;
            let smart = mode == ScreenshotMode::Auto
                && cfg.scope == crate::config::ShotScope::Auto
                && !extras;
            if smart && let Some(last) = self.screen_shot.clone() {
                let (map, base) = (last.coord, last.id);
                if map.bounds == capture.bounds && sig.same_as(&last.pixels, tolerance) {
                    return Ok(ToolOutput::text(format!(
                        "The screen looks the same as in your last full-screen screenshot (#{base}); not re-sent (mode=full sends it anyway)."
                    )));
                }
                if let Some(part) =
                    self.changed_part(&capture, Some(&sig), Some(&last.pixels), Some(map))
                {
                    let (img, (ox, oy)) = imaging::encode_part(&capture, part, &map, &cfg)?;
                    let (w, h, x1, y1) = (img.width, img.height, ox + img.width, oy + img.height);
                    let id = self.next_shot();
                    let text = if self.explain_first("screen-part") {
                        format!(
                            "Screenshot #{id}: only the part of the screen that changed since your last full-screen screenshot (#{base}), {w}x{h} px: the area x {ox}–{x1}, y {oy}–{y1} of that screenshot (same scale; the rest is unchanged).{note}"
                        )
                    } else {
                        format!(
                            "Screenshot #{id}: changed part only, the area x {ox}–{x1}, y {oy}–{y1} of #{base} (x/y still refer to that whole screenshot).{note}"
                        )
                    };
                    self.pending_screen_shot = Some(ScreenShot {
                        pixels: sig,
                        coord: map,
                        id: base,
                    });
                    return Ok(image(img, text));
                }
            }
            let (img, map) = imaging::encode(capture, &cfg)?;
            let id = self.next_shot();
            self.pending_screen_shot = Some(ScreenShot {
                pixels: sig,
                coord: map,
                id,
            });
            let text = format!(
                "Screenshot #{id} of {label}: {}x{} px.{note}",
                img.width, img.height
            );
            return Ok(image(img, text));
        }

        // A window whose screen the model knows: as get_app_state sends it
        // (nothing if unchanged, else the part that changed or all of it),
        // and it becomes the picture x/y refer to. mode=window asks for all
        // of it.
        if let Some((pid, screen)) = known_window {
            let force = args.mode == Some(ScreenshotMode::Window);
            return Ok(
                match self.window_picture(pid, screen, capture, force, false)? {
                    Picture::Unchanged { base } => {
                        let at = base.map(|b| format!(" (#{b})")).unwrap_or_default();
                        ToolOutput::text(format!(
                            "Screenshot of {label}: unchanged since your last screenshot of it{at}; not re-sent (mode=\"window\" sends it anyway).{note}"
                        ))
                    }
                    Picture::Part {
                        img, id, base, at, ..
                    } => {
                        if self.depth == 1 {
                            self.note.pictures_part.push(pid);
                        }
                        let (ox, oy) = at;
                        let (x1, y1) = (ox + img.width, oy + img.height);
                        let of = base
                            .map(|b| format!("#{b}"))
                            .unwrap_or_else(|| "your earlier screenshot of it".into());
                        let text = format!(
                            "Screenshot #{id} of {label}: only the part that changed, {}x{} px: the area x {ox}–{x1}, y {oy}–{y1} of {of} (same scale; x/y still refer to that whole screenshot).{note}",
                            img.width, img.height
                        );
                        image(img, text)
                    }
                    Picture::Whole { img, id, .. } => {
                        if self.depth == 1 {
                            self.note.pictures_whole.push(pid);
                        }
                        let text = format!(
                            "Screenshot #{id} of {label}: {}x{} px.{note}",
                            img.width, img.height
                        );
                        image(img, text)
                    }
                },
            );
        }

        let (img, _map) = imaging::encode(capture, &cfg)?;
        let text = format!(
            "Screenshot of {label}: {}x{} px.{note}",
            img.width, img.height
        );
        Ok(image(img, text))
    }
}

/// A screen rectangle in a capture's pixels.
fn capture_rect(cap: &Capture, r: Rect) -> Rect {
    let sx = f64::from(cap.width) / cap.bounds.width.max(1e-9);
    let sy = f64::from(cap.height) / cap.bounds.height.max(1e-9);
    Rect::new(
        (r.x - cap.bounds.x) * sx,
        (r.y - cap.bounds.y) * sy,
        r.width * sx,
        r.height * sy,
    )
}

/// One cell of a document up close: the cell and a little around it,
/// magnified, with a fine grid in the document's units and the cell's edges
/// in blue, and what it says about it.
fn cell_view(
    cap: &Capture,
    ax: imaging::Axis,
    ay: imaging::Axis,
    cells: &crate::cells::Cells,
    col: usize,
    row: usize,
) -> Result<(Capture, String)> {
    let sp = cells.span(col, row);
    let pad = cells.step * 0.15;
    let px = |a: imaging::Axis, v0: f64, v1: f64| {
        let (p0, p1) = (a.pixel(v0), a.pixel(v1));
        (p0.min(p1), p0.max(p1))
    };
    let (x0, x1) = px(ax, sp.x0 - pad, sp.x1 + pad);
    let (y0, y1) = px(ay, sp.y0 - pad, sp.y1 + pad);
    let (w, h) = (f64::from(cap.width), f64::from(cap.height));
    let (x0, y0) = (x0.floor().clamp(0.0, w), y0.floor().clamp(0.0, h));
    let (x1, y1) = (x1.ceil().clamp(0.0, w), y1.ceil().clamp(0.0, h));
    if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
        return Err(Error::InvalidArgs(format!(
            "cell {} is not on the screen",
            crate::cells::Cells::name(col, row)
        )));
    }
    let part = imaging::crop(
        cap,
        (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32),
    );
    // Its colours, from the cell alone.
    let (cx0, cx1) = px(ax, sp.x0, sp.x1);
    let (cy0, cy1) = px(ay, sp.y0, sp.y1);
    let inner = imaging::crop(
        cap,
        (
            cx0.max(0.0) as u32,
            cy0.max(0.0) as u32,
            (cx1 - cx0).max(1.0) as u32,
            (cy1 - cy0).max(1.0) as u32,
        ),
    );
    let k = (512 / part.width.max(part.height).max(1)).clamp(1, 16);
    let mut pic = imaging::magnify(&part, k);
    let kf = f64::from(k);
    let (bx, by) = (ax.label(x0), ay.label(y0));
    let (fx, fy) = (
        imaging::Axis {
            offset: bx,
            scale: ax.scale / kf,
        },
        imaging::Axis {
            offset: by,
            scale: ay.scale / kf,
        },
    );
    let fine = imaging::nice_step(cells.step, 10.0);
    let used = imaging::draw_grid(&mut pic, fx, fy, fine, 1.0, None);
    let (ex0, ex1) = px(fx, sp.x0, sp.x1);
    let (ey0, ey1) = px(fy, sp.y0, sp.y1);
    for t in 0..2 {
        for x in ex0 as i64..=ex1 as i64 {
            imaging::blend(&mut pic, x, ey0 as i64 + t, [40, 100, 210], 0.9);
            imaging::blend(&mut pic, x, ey1 as i64 - t, [40, 100, 210], 0.9);
        }
        for y in ey0 as i64..=ey1 as i64 {
            imaging::blend(&mut pic, ex0 as i64 + t, y, [40, 100, 210], 0.9);
            imaging::blend(&mut pic, ex1 as i64 - t, y, [40, 100, 210], 0.9);
        }
    }
    let colours: Vec<String> = imaging::palette(&inner, 5)
        .iter()
        .map(|(hex, share)| format!("{hex} {:.0}%", share * 100.0))
        .collect();
    let n = |v: f64| imaging::grid_label(v, fine);
    let text = format!(
        "Cell {} of the document: x {} to {}, y {} to {} (shown {k} times bigger, a grid line every {}, in the document's units; its edges in blue). Main colours: {}.",
        crate::cells::Cells::name(col, row),
        n(sp.x0),
        n(sp.x1),
        n(sp.y0),
        n(sp.y1),
        n(used),
        colours.join(", ")
    );
    Ok((pic, text))
}

fn mode_name(m: ScreenshotMode) -> &'static str {
    match m {
        ScreenshotMode::Auto => "auto",
        ScreenshotMode::Full => "full",
        ScreenshotMode::Region => "region",
        ScreenshotMode::Window => "window",
    }
}
