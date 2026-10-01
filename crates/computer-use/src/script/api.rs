//! The functions scripts call, and running one. Everything a script can
//! reach is registered here; Rhai itself has no access to the system.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use rhai::{
    Array, Dynamic, Engine, EvalAltResult, FLOAT, INT, Map, NativeCallContext, Position, Scope,
};
use serde_json::{Value, json};

use super::page::{self, PageState};
use super::{Env, Job, Msg, Outcome, Reply, Request, io};
use crate::cells::Cells;

type Res<T> = Result<T, Box<EvalAltResult>>;

/// Most printed text kept, and longest result value.
const MAX_OUTPUT: usize = 20_000;
const MAX_VALUE: usize = 8_000;
/// Saved scripts running inside each other.
const MAX_DEPTH: u32 = 8;

fn err(e: impl std::fmt::Display) -> Box<EvalAltResult> {
    e.to_string().into()
}

/// Ends the script (try/catch can't hold it): "stopped", "time" or "gone".
fn terminated(token: &str) -> Box<EvalAltResult> {
    EvalAltResult::ErrorTerminated(token.into(), Position::NONE).into()
}

/// The token a terminated script ended with.
fn terminated_token(e: &EvalAltResult) -> Option<String> {
    match e {
        EvalAltResult::ErrorTerminated(t, _) => Some(t.to_string()),
        EvalAltResult::ErrorInFunctionCall(_, _, inner, _)
        | EvalAltResult::ErrorInModule(_, inner, _) => terminated_token(inner),
        _ => None,
    }
}

/// A script's values as data. Whole numbers come out as integers, so they
/// fit arguments that need one (element_index).
pub(super) fn to_json(d: &Dynamic) -> Res<Value> {
    if d.is_unit() {
        return Ok(Value::Null);
    }
    if let Some(p) = d.clone().try_cast::<Page>() {
        return Ok(json!(p.ctx.pages.borrow()[p.id].name));
    }
    let v: Value = rhai::serde::from_dynamic(d)
        .map_err(|e| err(format!("can't turn a {} into data: {e}", d.type_name())))?;
    Ok(whole(v))
}

fn whole(v: Value) -> Value {
    match v {
        Value::Number(n) if n.is_f64() => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 9e15 => json!(f as i64),
            _ => Value::Number(n),
        },
        Value::Array(a) => Value::Array(a.into_iter().map(whole).collect()),
        Value::Object(m) => Value::Object(m.into_iter().map(|(k, v)| (k, whole(v))).collect()),
        other => other,
    }
}

pub(super) fn from_json(v: &Value) -> Res<Dynamic> {
    rhai::serde::to_dynamic(v)
}

fn map_json(m: Map) -> Res<serde_json::Map<String, Value>> {
    match to_json(&Dynamic::from_map(m))? {
        Value::Object(m) => Ok(m),
        _ => Ok(serde_json::Map::new()),
    }
}

/// A number, whole or not.
fn n(d: &Dynamic) -> Res<f64> {
    if let Ok(f) = d.as_float() {
        return Ok(f);
    }
    if let Ok(i) = d.as_int() {
        return Ok(i as f64);
    }
    Err(err(format!(
        "expected a number, got {} ({d})",
        d.type_name()
    )))
}

/// `[[x, y], ...]` or `[#{x, y}, ...]`.
fn points(a: &Array) -> Res<Vec<(f64, f64)>> {
    a.iter()
        .map(|p| {
            if let Some(pair) = p.read_lock::<Array>()
                && pair.len() == 2
            {
                return Ok((n(&pair[0])?, n(&pair[1])?));
            }
            if let Some(m) = p.read_lock::<Map>()
                && let (Some(x), Some(y)) = (m.get("x"), m.get("y"))
            {
                return Ok((n(x)?, n(y)?));
            }
            Err(err(format!("a point is [x, y] or #{{x, y}}, not {p}")))
        })
        .collect()
}

fn hex(rgb: [f64; 3]) -> String {
    let c = |v: f64| v.round().clamp(0.0, 255.0) as u8;
    format!("#{:02x}{:02x}{:02x}", c(rgb[0]), c(rgb[1]), c(rgb[2]))
}

fn parse_hex(s: &str) -> Res<[f64; 3]> {
    let t = s.trim().trim_start_matches('#');
    let full: String = if t.len() == 3 {
        t.chars().flat_map(|c| [c, c]).collect()
    } else {
        t.to_string()
    };
    if full.len() != 6 || !full.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(err(format!("\"{s}\" is not a colour like \"#1e88e5\"")));
    }
    let v = |i: usize| f64::from(u8::from_str_radix(&full[i..i + 2], 16).unwrap_or(0));
    Ok([v(0), v(2), v(4)])
}

fn hsl(h: f64, s: f64, l: f64) -> [f64; 3] {
    let h = h.rem_euclid(360.0) / 360.0;
    let (s, l) = (s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
    if s == 0.0 {
        return [l * 255.0; 3];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let f = |t: f64| {
        let t = t.rem_euclid(1.0);
        255.0
            * if t < 1.0 / 6.0 {
                p + (q - p) * 6.0 * t
            } else if t < 0.5 {
                q
            } else if t < 2.0 / 3.0 {
                p + (q - p) * (2.0 / 3.0 - t) * 6.0
            } else {
                p
            }
    };
    [f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0)]
}

/// "2026-10-01 12:34:56" (UTC).
pub(super) fn utc_date(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// What a running script keeps: its link to the engine, its output, its
/// pages.
pub(super) struct Ctx {
    env: Env,
    to_engine: Sender<Msg>,
    from_engine: Receiver<Reply>,
    started: Instant,
    deadline: Instant,
    abort: Arc<AtomicBool>,
    output: RefCell<String>,
    cut: Cell<bool>,
    app: RefCell<Option<String>>,
    pages: RefCell<Vec<PageState>>,
    last_page: Cell<Option<usize>>,
    /// The picture the result shows: a page's (Some(Some(page))) or
    /// another (Some(None)).
    shown: Cell<Option<Option<usize>>>,
    calls: Cell<usize>,
    rng: Cell<u64>,
    depth: Cell<u32>,
    regexes: RefCell<HashMap<String, regex::Regex>>,
}

impl Ctx {
    fn check(&self) -> Res<()> {
        if self.env.stop.load(Ordering::Relaxed) {
            return Err(terminated("stopped"));
        }
        if self.abort.load(Ordering::Relaxed) || Instant::now() > self.deadline {
            return Err(terminated("time"));
        }
        Ok(())
    }

    fn ask(&self, r: Request) -> Res<Value> {
        self.check()?;
        self.to_engine
            .send(Msg::Ask(r))
            .map_err(|_| terminated("gone"))?;
        let reply = self.from_engine.recv().map_err(|_| terminated("gone"))?;
        self.check()?;
        reply.map_err(err)
    }

    /// Run a tool: `{ok, text, image}`.
    fn tool(&self, name: &str, args: Option<Map>) -> Res<Value> {
        let name = name.trim();
        if name == "script" {
            return Err(err(
                "a script can't call the script tool: run(name, args) runs a saved script",
            ));
        }
        let mut args = match args {
            Some(m) => Value::Object(map_json(m)?),
            None => json!({}),
        };
        if let (Value::Object(m), Some(app)) = (&mut args, self.app.borrow().as_ref())
            && !m.contains_key("app")
            && self.env.app_tools.iter().any(|t| t == name)
        {
            m.insert("app".into(), json!(app));
        }
        self.calls.set(self.calls.get() + 1);
        self.ask(Request::Tool {
            name: name.to_string(),
            args,
        })
    }

    fn print(&self, line: &str) {
        if self.cut.get() {
            return;
        }
        let mut out = self.output.borrow_mut();
        if out.len() + line.len() > MAX_OUTPUT {
            self.cut.set(true);
            return;
        }
        out.push_str(line);
        out.push('\n');
    }

    fn next(&self) -> u64 {
        // xorshift64*
        let mut x = self.rng.get();
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng.set(x);
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn unit(&self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn regex(&self, pattern: &str) -> Res<regex::Regex> {
        if let Some(r) = self.regexes.borrow().get(pattern) {
            return Ok(r.clone());
        }
        let r = regex::RegexBuilder::new(pattern)
            .size_limit(1 << 22)
            .build()
            .map_err(|e| err(format!("bad pattern: {e}")))?;
        let mut cache = self.regexes.borrow_mut();
        if cache.len() > 64 {
            cache.clear();
        }
        cache.insert(pattern.to_string(), r.clone());
        Ok(r)
    }

    fn wait(&self, ms: f64) -> Res<()> {
        let end = Instant::now() + Duration::from_secs_f64((ms.max(0.0) / 1000.0).min(3600.0));
        loop {
            self.check()?;
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(());
            }
            std::thread::sleep(left.min(Duration::from_millis(100)));
        }
    }

    // -- pages ------------------------------------------------------------

    fn page_open(self: &Rc<Self>, name: &str, size: Option<(f64, f64)>, opts: Map) -> Res<Page> {
        // Named as the design board names designs ("My Page" is "my-page").
        let name: String = name
            .trim()
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .chars()
            .take(40)
            .collect();
        if name.is_empty() {
            return Err(err("give the page a name"));
        }
        // What this script already put on a page of that name goes to the
        // board first, so opening it again sees it.
        let held = self.pages.borrow().iter().position(|p| p.name == name);
        if let Some(i) = held {
            self.flush(i)?;
        }
        let info = self.ask(Request::Design { name: name.clone() }).ok();
        let existing: Vec<String> = info
            .as_ref()
            .and_then(|i| i["layers"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|l| l["id"].as_str().map(str::to_string))
            .collect();
        let mut st = PageState {
            name: name.clone(),
            ..Default::default()
        };
        match size {
            None => {
                let info = info.ok_or_else(|| {
                    err(format!(
                        "no page or design called \"{name}\": page(\"{name}\", width, height) starts one"
                    ))
                })?;
                st.width = info["width"].as_f64().unwrap_or(1.0);
                st.height = info["height"].as_f64().unwrap_or(1.0);
                st.cell = info["cell"].as_f64();
                st.ids = existing;
                st.dirty = true;
            }
            Some((w, h)) => {
                if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
                    return Err(err("a page's width and height are above 0"));
                }
                let opts = map_json(opts)?;
                for k in opts.keys() {
                    if !["background", "cell", "margin"].contains(&k.as_str()) {
                        return Err(err(format!(
                            "page options are background, cell (the cells' size) and margin, not {k}"
                        )));
                    }
                }
                st.width = w;
                st.height = h;
                st.cell = opts
                    .get("cell")
                    .and_then(Value::as_f64)
                    .filter(|c| *c > 0.0);
                if let Some(c) = st.cell {
                    let most = (w / c).ceil().max((h / c).ceil());
                    if most > crate::design::MAX_CELLS {
                        return Err(err(format!(
                            "cells of {c} make {most} across the page; at most {}",
                            crate::design::MAX_CELLS
                        )));
                    }
                }
                st.start.insert("size".into(), json!([w, h]));
                st.start.insert(
                    "background".into(),
                    opts.get("background").cloned().unwrap_or(json!("#ffffff")),
                );
                st.start
                    .insert("cell_size".into(), json!(st.cell.unwrap_or(0.0)));
                if let Some(m) = opts.get("margin") {
                    st.start.insert("margin".into(), m.clone());
                }
                // A page starts empty, so running a script again draws the
                // same page.
                st.remove = existing;
                st.dirty = true;
            }
        }
        let mut pages = self.pages.borrow_mut();
        let id = match pages.iter().position(|p| p.name == name) {
            Some(i) => {
                pages[i] = st;
                i
            }
            None => {
                pages.push(st);
                pages.len() - 1
            }
        };
        self.last_page.set(Some(id));
        Ok(Page {
            id,
            ctx: self.clone(),
        })
    }

    /// Send what changed on a page to the design board.
    fn flush(&self, id: usize) -> Res<()> {
        let (name, args) = {
            let mut pages = self.pages.borrow_mut();
            let st = &mut pages[id];
            if !st.dirty {
                return Ok(());
            }
            (st.name.clone(), st.update())
        };
        let r = self.ask(Request::Tool {
            name: "design".into(),
            args,
        })?;
        let text = r["text"].as_str().unwrap_or_default().to_string();
        if r["ok"] != json!(true) {
            // The board took none of it: start again from what it has.
            let ids: Vec<String> = self
                .ask(Request::Design { name: name.clone() })
                .ok()
                .and_then(|i| i["layers"].as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|l| l["id"].as_str().map(str::to_string))
                .collect();
            self.pages.borrow_mut()[id].ids = ids;
            return Err(err(format!("the page \"{name}\" was not drawn: {text}")));
        }
        let mut pages = self.pages.borrow_mut();
        pages[id].image = r["image"].as_u64();
        pages[id].summary = text;
        Ok(())
    }

    fn show_page(&self, id: usize) -> Res<()> {
        self.flush(id)?;
        let image = self.pages.borrow()[id].image;
        if let Some(image) = image {
            self.ask(Request::Show { image })?;
        }
        self.shown.set(Some(Some(id)));
        Ok(())
    }

    /// At the end: every page drawn, and the last one touched shown unless
    /// the script showed another picture.
    fn finish(&self) -> Res<()> {
        let count = self.pages.borrow().len();
        for i in 0..count {
            self.flush(i)?;
        }
        match self.shown.get() {
            Some(None) => Ok(()),
            Some(Some(i)) => self.show_page(i),
            None => match self.last_page.get() {
                Some(i) => self.show_page(i),
                None => Ok(()),
            },
        }
    }
}

/// A page of graph paper (a design on the board), as scripts hold it.
#[derive(Clone)]
pub(super) struct Page {
    id: usize,
    ctx: Rc<Ctx>,
}

impl Page {
    fn with<T>(&self, f: impl FnOnce(&mut PageState) -> Result<T, String>) -> Res<T> {
        self.ctx.last_page.set(Some(self.id));
        f(&mut self.ctx.pages.borrow_mut()[self.id]).map_err(err)
    }

    fn add(&self, kind: &str, shape: Value, style: Option<Map>) -> Res<String> {
        let style = match style {
            Some(m) => map_json(m)?,
            None => serde_json::Map::new(),
        };
        let Value::Object(shape) = shape else {
            return Err(err("a layer is a map"));
        };
        self.with(|st| st.add(kind, shape, style))
    }

    fn cells(&self) -> Cells {
        self.ctx.pages.borrow()[self.id].cells()
    }

    fn cell(&self, name: &str) -> Res<Value> {
        let c = self.cells();
        let (col, row) = c.parse(name).map_err(err)?;
        Ok(page::cell_map(&c, col, row))
    }

    fn info(&self) -> Res<Value> {
        self.ctx.flush(self.id)?;
        let name = self.ctx.pages.borrow()[self.id].name.clone();
        self.ctx.ask(Request::Design { name })
    }
}

/// Cells over any canvas.
#[derive(Clone)]
pub(super) struct Grid(Cells);

/// The longest text a script makes (bytes).
const MAX_TEXT: usize = 32 * 1024 * 1024;

/// A Rhai engine with the limits and settings every script runs with, but
/// none of the functions (enough to check that a script parses).
pub(super) fn bare_engine() -> Engine {
    let mut engine = Engine::new();
    engine
        .set_strict_variables(true)
        .set_max_call_levels(64)
        .set_max_expr_depths(256, 128)
        .set_max_string_size(MAX_TEXT)
        .set_max_array_size(2_000_000)
        .set_max_map_size(500_000)
        .set_max_modules(64);
    // Never the process's stdout: it is the MCP connection.
    engine.on_print(|_| {});
    engine.on_debug(|_, _, _| {});
    engine
}

/// The variables every script starts with.
pub(super) fn base_scope(args: Value, data: Value) -> Scope<'static> {
    let mut s = Scope::new();
    s.push_constant("PI", std::f64::consts::PI);
    s.push_constant("TAU", std::f64::consts::TAU);
    let args = match args {
        Value::Object(_) => args,
        _ => json!({}),
    };
    s.push(
        "args",
        from_json(&args).unwrap_or_else(|_| Dynamic::from_map(Map::new())),
    );
    s.push("data", from_json(&data).unwrap_or(Dynamic::UNIT));
    s
}

/// Run a script to its end; how it went.
pub(super) fn run(
    job: Job,
    to_engine: Sender<Msg>,
    from_engine: Receiver<Reply>,
    abort: Arc<AtomicBool>,
    deadline: Instant,
) -> Outcome {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0x6d68_7273, |d| d.as_nanos() as u64)
        | 1;
    let ctx = Rc::new(Ctx {
        env: job.env.clone(),
        to_engine,
        from_engine,
        started: Instant::now(),
        deadline,
        abort,
        output: RefCell::default(),
        cut: Cell::new(false),
        app: RefCell::default(),
        pages: RefCell::default(),
        last_page: Cell::new(None),
        shown: Cell::new(None),
        calls: Cell::new(0),
        rng: Cell::new(seed),
        depth: Cell::new(0),
        regexes: RefCell::default(),
    });
    let engine = full_engine(&ctx);
    let mut scope = base_scope(job.args, job.data);
    let mut out = Outcome::default();
    let mut ended = false;
    match engine.compile_with_scope(&scope, &job.code) {
        Err(e) => out.error = Some(super::describe_parse(&e, &job.code)),
        Ok(mut ast) => {
            if let Some(s) = &job.source {
                ast.set_source(s.as_str());
            }
            match engine.eval_ast_with_scope::<Dynamic>(&mut scope, &ast) {
                Ok(v) => out.value = render(&v),
                Err(e) => match terminated_token(&e).as_deref() {
                    Some("stopped") => {
                        out.stopped = true;
                        ended = true;
                    }
                    Some("time") => {
                        out.error = Some(format!(
                            "the script ran out of time ({} s: the user's [script] max_seconds setting). Do less per run, or split the work",
                            job.env.max_seconds
                        ));
                        ended = true;
                    }
                    Some(_) => {
                        out.error = Some("the server ended the script".into());
                        ended = true;
                    }
                    None => {
                        out.error =
                            Some(super::describe_error(&e, &job.code, job.source.as_deref()))
                    }
                },
            }
        }
    }
    // What the script put on its pages is drawn even when it failed later:
    // the picture shows how far it got.
    if !ended && let Err(e) = ctx.finish() {
        match terminated_token(&e).as_deref() {
            Some("stopped") => out.stopped = true,
            Some(_) => {}
            None => {
                let e = e.to_string();
                out.error = Some(match out.error.take() {
                    Some(first) => format!("{first}\n{e}"),
                    None => e,
                });
            }
        }
    }
    out.output = ctx.output.take();
    if ctx.cut.get() {
        out.output.push_str("… (more output was left out)\n");
    }
    out.calls = ctx.calls.get();
    out
}

/// A value as the result shows it.
fn render(v: &Dynamic) -> Option<String> {
    if v.is_unit() {
        return None;
    }
    let text = if v.is_string() {
        v.to_string()
    } else if let Some(p) = v.clone().try_cast::<Page>() {
        page_text(&p)
    } else if let Some(g) = v.clone().try_cast::<Grid>() {
        g.0.describe()
    } else {
        to_json(v).map_or_else(|_| v.to_string(), |j| j.to_string())
    };
    Some(if text.len() > MAX_VALUE {
        let cut: String = text.chars().take(MAX_VALUE).collect();
        format!("{cut}… ({} characters in all)", text.chars().count())
    } else {
        text
    })
}

fn page_text(p: &Page) -> String {
    let pages = p.ctx.pages.borrow();
    let st = &pages[p.id];
    format!(
        "page \"{}\" {} x {}, {}",
        st.name,
        st.width,
        st.height,
        st.cells().describe()
    )
}

fn full_engine(ctx: &Rc<Ctx>) -> Engine {
    let mut engine = bare_engine();
    let c = ctx.clone();
    engine.on_print(move |s| c.print(s));
    let c = ctx.clone();
    engine.on_debug(move |s, _, pos| match pos.line() {
        Some(l) => c.print(&format!("[line {l}] {s}")),
        None => c.print(s),
    });
    let (stop, abort, deadline) = (ctx.env.stop.clone(), ctx.abort.clone(), ctx.deadline);
    engine.on_progress(move |ops| {
        if ops % 512 != 0 {
            return None;
        }
        if stop.load(Ordering::Relaxed) {
            return Some("stopped".into());
        }
        if abort.load(Ordering::Relaxed) || Instant::now() > deadline {
            return Some("time".into());
        }
        None
    });
    engine.set_module_resolver(
        rhai::module_resolvers::FileModuleResolver::new_with_path_and_extension(
            ctx.env.library.clone(),
            "rhai",
        ),
    );
    tools_api(&mut engine, ctx);
    page_api(&mut engine, ctx);
    data_api(&mut engine, ctx);
    maths_api(&mut engine, ctx);
    engine
}

// -- tools and the screen -----------------------------------------------------

fn tools_api(engine: &mut Engine, ctx: &Rc<Ctx>) {
    let text_of = |r: Value| -> Res<String> {
        let text = r["text"].as_str().unwrap_or_default().to_string();
        if r["ok"] == json!(true) {
            Ok(text)
        } else {
            Err(err(text))
        }
    };
    let c = ctx.clone();
    engine.register_fn("tool", move |name: &str| text_of(c.tool(name, None)?));
    let c = ctx.clone();
    engine.register_fn("tool", move |name: &str, args: Map| {
        text_of(c.tool(name, Some(args))?)
    });
    let c = ctx.clone();
    engine.register_fn("try_tool", move |name: &str| {
        from_json(&c.tool(name, None)?)
    });
    let c = ctx.clone();
    engine.register_fn("try_tool", move |name: &str, args: Map| {
        from_json(&c.tool(name, Some(args))?)
    });
    let c = ctx.clone();
    engine.register_fn("set_app", move |app: &str| {
        *c.app.borrow_mut() = Some(app.trim().to_string()).filter(|a| !a.is_empty());
    });

    let elements = |c: &Ctx, app: &str, filter: Map| -> Res<Dynamic> {
        let f = map_json(filter)?;
        let s = |k: &str| f.get(k).and_then(Value::as_str).map(str::to_string);
        for k in f.keys() {
            if !["role", "name", "text", "editable", "window", "max"].contains(&k.as_str()) {
                return Err(err(format!(
                    "elements filters by role, name, text, editable, window and max, not {k}"
                )));
            }
        }
        c.calls.set(c.calls.get() + 1);
        from_json(
            &c.ask(Request::Elements {
                app: app.to_string(),
                window: s("window"),
                role: s("role"),
                name: s("name"),
                text: s("text"),
                editable: f.get("editable").and_then(Value::as_bool).unwrap_or(false),
                max: f
                    .get("max")
                    .and_then(Value::as_u64)
                    .unwrap_or(500)
                    .clamp(1, 5000) as usize,
            })?,
        )
    };
    let c = ctx.clone();
    engine.register_fn("elements", move |app: &str| elements(&c, app, Map::new()));
    let c = ctx.clone();
    engine.register_fn("elements", move |app: &str, filter: Map| {
        elements(&c, app, filter)
    });

    let c = ctx.clone();
    engine.register_fn("colors", move |app: &str, pts: Array| {
        c.calls.set(c.calls.get() + 1);
        from_json(&c.ask(Request::Colors {
            app: app.to_string(),
            window: None,
            points: points(&pts)?,
        })?)
    });
    let c = ctx.clone();
    engine.register_fn(
        "color_at",
        move |app: &str, x: Dynamic, y: Dynamic| -> Res<Dynamic> {
            c.calls.set(c.calls.get() + 1);
            let r = c.ask(Request::Colors {
                app: app.to_string(),
                window: None,
                points: vec![(n(&x)?, n(&y)?)],
            })?;
            from_json(&r[0])
        },
    );

    let c = ctx.clone();
    engine.register_fn("show", move |r: Map| -> Res<()> {
        let id = r
            .get("image")
            .and_then(|v| v.as_int().ok())
            .ok_or_else(|| err("that result has no picture"))?;
        c.ask(Request::Show { image: id as u64 })?;
        c.shown.set(Some(None));
        Ok(())
    });
    let c = ctx.clone();
    engine.register_fn("show", move |id: INT| -> Res<()> {
        c.ask(Request::Show { image: id as u64 })?;
        c.shown.set(Some(None));
        Ok(())
    });
    let c = ctx.clone();
    engine.register_fn("show_image", move |path: &str| -> Res<()> {
        let p = io::resolve(&c.env, path, false).map_err(err)?;
        c.ask(Request::ShowFile { path: p })?;
        c.shown.set(Some(None));
        Ok(())
    });
    let c = ctx.clone();
    engine.register_fn("sleep", move |ms: Dynamic| c.wait(n(&ms)?));

    let c = ctx.clone();
    engine.register_fn("run", move |nc: NativeCallContext, name: &str| {
        run_saved(&c, &nc, name, Map::new())
    });
    let c = ctx.clone();
    engine.register_fn(
        "run",
        move |nc: NativeCallContext, name: &str, args: Map| run_saved(&c, &nc, name, args),
    );
}

/// Run a saved script inside this one; its last value.
fn run_saved(ctx: &Ctx, nc: &NativeCallContext, name: &str, args: Map) -> Res<Dynamic> {
    let name = name.trim().to_lowercase();
    super::valid_name(&name).map_err(err)?;
    if ctx.depth.get() >= MAX_DEPTH {
        return Err(err(format!(
            "scripts run inside each other at most {MAX_DEPTH} deep"
        )));
    }
    let path = ctx.env.library.join(format!("{name}.rhai"));
    let code = std::fs::read_to_string(&path)
        .map_err(|_| err(format!("no saved script called \"{name}\"")))?;
    let args = Value::Object(map_json(args)?);
    let mut scope = base_scope(args, Value::Null);
    let engine = nc.engine();
    let mut ast = engine
        .compile_with_scope(&scope, &code)
        .map_err(|e| err(format!("{name}: {}", super::describe_parse(&e, &code))))?;
    ast.set_source(name.as_str());
    ctx.depth.set(ctx.depth.get() + 1);
    let r = engine.eval_ast_with_scope::<Dynamic>(&mut scope, &ast);
    ctx.depth.set(ctx.depth.get() - 1);
    r.map_err(|e| {
        if terminated_token(&e).is_some() {
            e
        } else {
            err(format!(
                "in {name}: {}",
                super::describe_error(&e, &code, Some(&name))
            ))
        }
    })
}

// -- pages and cells -----------------------------------------------------------

fn page_api(engine: &mut Engine, ctx: &Rc<Ctx>) {
    engine.register_type_with_name::<Page>("Page");
    engine.register_type_with_name::<Grid>("Cells");

    let c = ctx.clone();
    engine.register_fn("page", move |name: &str| {
        c.page_open(name, None, Map::new())
    });
    let c = ctx.clone();
    engine.register_fn("page", move |name: &str, w: Dynamic, h: Dynamic| {
        c.page_open(name, Some((n(&w)?, n(&h)?)), Map::new())
    });
    let c = ctx.clone();
    engine.register_fn(
        "page",
        move |name: &str, w: Dynamic, h: Dynamic, opts: Map| {
            c.page_open(name, Some((n(&w)?, n(&h)?)), opts)
        },
    );

    // Shapes: the numbers they take, and the design layer they make.
    macro_rules! shape {
        ($name:literal, $kind:literal, |$($a:ident),*| $make:expr) => {
            engine.register_fn($name, |p: &mut Page, $($a: Dynamic),*| -> Res<String> {
                $(let $a = n(&$a)?;)*
                p.add($kind, $make, None)
            });
            engine.register_fn($name, |p: &mut Page, $($a: Dynamic,)* style: Map| -> Res<String> {
                $(let $a = n(&$a)?;)*
                p.add($kind, $make, Some(style))
            });
        };
    }
    shape!("rect", "rect", |x, y, w, h| json!({"rect": [x, y, w, h]}));
    shape!(
        "circle",
        "circle",
        |cx, cy, r| json!({"ellipse": [cx, cy, r, r]})
    );
    shape!(
        "ellipse",
        "ellipse",
        |cx, cy, rx, ry| json!({"ellipse": [cx, cy, rx, ry]})
    );
    shape!(
        "line",
        "line",
        |x1, y1, x2, y2| json!({"points": [[x1, y1], [x2, y2]]})
    );
    shape!(
        "regular",
        "polygon",
        |cx, cy, r, sides| json!({"polygon": [cx, cy, r, sides]})
    );
    shape!(
        "star",
        "star",
        |cx, cy, outer, inner, tips| json!({"star": [cx, cy, outer, inner, tips]})
    );
    shape!(
        "arc",
        "arc",
        |cx, cy, r, from, to| json!({"arc": [cx, cy, r, from, to]})
    );

    macro_rules! path {
        ($name:literal, $kind:literal, $key:literal, $closed:expr) => {
            engine.register_fn($name, |p: &mut Page, pts: Array| -> Res<String> {
                let pts: Vec<[f64; 2]> = points(&pts)?.into_iter().map(|(x, y)| [x, y]).collect();
                let mut shape = json!({ $key: pts });
                if $closed {
                    shape["closed"] = json!(true);
                }
                p.add($kind, shape, None)
            });
            engine.register_fn($name, |p: &mut Page, pts: Array, style: Map| -> Res<String> {
                let pts: Vec<[f64; 2]> = points(&pts)?.into_iter().map(|(x, y)| [x, y]).collect();
                let mut shape = json!({ $key: pts });
                if $closed {
                    shape["closed"] = json!(true);
                }
                p.add($kind, shape, Some(style))
            });
        };
    }
    path!("path", "path", "points", false);
    path!("polygon", "polygon", "points", true);
    path!("bezier", "bezier", "bezier", false);

    let curve = |p: &mut Page, x: &str, y: &str, t0: Dynamic, t1: Dynamic, style: Option<Map>| {
        let t = |d: &Dynamic| -> Res<Value> {
            if d.is_string() {
                Ok(json!(d.to_string()))
            } else {
                Ok(json!(n(d)?))
            }
        };
        p.add(
            "curve",
            json!({"x": x, "y": y, "t": [t(&t0)?, t(&t1)?]}),
            style,
        )
    };
    engine.register_fn(
        "curve",
        move |p: &mut Page, x: &str, y: &str, t0: Dynamic, t1: Dynamic| {
            curve(p, x, y, t0, t1, None)
        },
    );
    engine.register_fn(
        "curve",
        move |p: &mut Page, x: &str, y: &str, t0: Dynamic, t1: Dynamic, style: Map| {
            curve(p, x, y, t0, t1, Some(style))
        },
    );
    engine.register_fn(
        "text",
        |p: &mut Page, text: &str, x: Dynamic, y: Dynamic| {
            p.add("text", json!({"text": text, "at": [n(&x)?, n(&y)?]}), None)
        },
    );
    engine.register_fn(
        "text",
        |p: &mut Page, text: &str, x: Dynamic, y: Dynamic, style: Map| {
            p.add(
                "text",
                json!({"text": text, "at": [n(&x)?, n(&y)?]}),
                Some(style),
            )
        },
    );
    engine.register_fn("layer", |p: &mut Page, layer: Map| -> Res<String> {
        let mut m = map_json(layer)?;
        let kind = [
            "rect", "ellipse", "polygon", "star", "arc", "bezier", "points", "text",
        ]
        .into_iter()
        .find(|k| m.contains_key(*k))
        .unwrap_or(if m.contains_key("x") || m.contains_key("y") {
            "curve"
        } else {
            "layer"
        });
        let id = m.remove("id");
        let mut style = serde_json::Map::new();
        if let Some(id) = id {
            style.insert("id".into(), id);
        }
        p.with(|st| st.add(kind, m, style))
    });

    // Cells of the page.
    let fill_cell = |p: &mut Page, name: &str, color: &str, style: Option<Map>| -> Res<String> {
        let c = p.cells();
        let (col, row) = c.parse(name).map_err(err)?;
        let mut style = match style {
            Some(m) => map_json(m)?,
            None => serde_json::Map::new(),
        };
        style.insert("fill".into(), json!(color));
        style.entry("stroke").or_insert(json!("none"));
        let Value::Object(shape) = json!({"rect": page::rect_of(c.span(col, row))}) else {
            unreachable!("an object")
        };
        p.with(|st| st.add("cell", shape, style))
    };
    engine.register_fn("fill_cell", move |p: &mut Page, name: &str, color: &str| {
        fill_cell(p, name, color, None)
    });
    engine.register_fn(
        "fill_cell",
        move |p: &mut Page, name: &str, color: &str, style: Map| {
            fill_cell(p, name, color, Some(style))
        },
    );
    let text_in = |p: &mut Page, name: &str, text: &str, style: Option<Map>| -> Res<String> {
        let c = p.cells();
        let (col, row) = c.parse(name).map_err(err)?;
        let s = c.span(col, row);
        let mut style = match style {
            Some(m) => map_json(m)?,
            None => serde_json::Map::new(),
        };
        let size = style
            .get("size")
            .and_then(Value::as_f64)
            .unwrap_or(c.step * 0.45);
        style.insert("size".into(), json!(size));
        style.entry("align").or_insert(json!("center"));
        let at = [(s.x0 + s.x1) / 2.0, (s.y0 + s.y1) / 2.0 - size * 0.55];
        let Value::Object(shape) = json!({"text": text, "at": at}) else {
            unreachable!("an object")
        };
        p.with(|st| st.add("text", shape, style))
    };
    engine.register_fn("text_in", move |p: &mut Page, name: &str, text: &str| {
        text_in(p, name, text, None)
    });
    engine.register_fn(
        "text_in",
        move |p: &mut Page, name: &str, text: &str, style: Map| text_in(p, name, text, Some(style)),
    );
    engine.register_fn("cell", |p: &mut Page, name: &str| -> Res<Dynamic> {
        from_json(&p.cell(name)?)
    });
    engine.register_fn("cell", |p: &mut Page, col: INT, row: INT| -> Res<Dynamic> {
        let c = p.cells();
        let name = Cells::name(col.max(0) as usize, row.max(0) as usize);
        let (col, row) = c.parse(&name).map_err(err)?;
        from_json(&page::cell_map(&c, col, row))
    });
    engine.register_fn(
        "at",
        |p: &mut Page, x: Dynamic, y: Dynamic| -> Res<String> {
            Ok(page::cell_at(&p.cells(), n(&x)?, n(&y)?))
        },
    );
    engine.register_fn("cells", |p: &mut Page| Grid(p.cells()));
    engine.register_fn("change", |p: &mut Page, id: &str, spec: Map| -> Res<()> {
        let spec = map_json(spec)?;
        p.with(|st| st.change(id, spec))
    });
    engine.register_fn("remove", |p: &mut Page, id: &str| {
        p.with(|st| st.remove(id))
    });
    engine.register_fn("clear", |p: &mut Page| {
        p.with(|st| {
            st.clear();
            Ok(())
        })
    });
    engine.register_fn("show", |p: &mut Page| -> Res<String> {
        p.ctx.show_page(p.id)?;
        Ok(p.ctx.pages.borrow()[p.id].summary.clone())
    });
    engine.register_fn("export", |p: &mut Page, format: &str| -> Res<String> {
        p.ctx.flush(p.id)?;
        let name = p.ctx.pages.borrow()[p.id].name.clone();
        let r = p.ctx.ask(Request::Tool {
            name: "design".into(),
            args: json!({"name": name, "export": format.trim().to_lowercase()}),
        })?;
        let text = r["text"].as_str().unwrap_or_default();
        if r["ok"] != json!(true) {
            return Err(err(text));
        }
        text.lines()
            .find_map(|l| l.strip_prefix("Exported to "))
            .and_then(|l| l.rsplit_once(" KB)").map(|(a, _)| a))
            .and_then(|l| l.rsplit_once(" ("))
            .map(|(path, _)| path.to_string())
            .ok_or_else(|| err(format!("no file was written: {text}")))
    });
    engine.register_fn("steps", |p: &mut Page| from_json(&p.info()?["steps"]));
    engine.register_fn("layers", |p: &mut Page| from_json(&p.info()?["layers"]));
    engine.register_get("name", |p: &mut Page| {
        p.ctx.pages.borrow()[p.id].name.clone()
    });
    engine.register_get("width", |p: &mut Page| p.ctx.pages.borrow()[p.id].width);
    engine.register_get("height", |p: &mut Page| p.ctx.pages.borrow()[p.id].height);
    engine.register_fn("to_string", |p: &mut Page| page_text(p));
    engine.register_fn("to_debug", |p: &mut Page| page_text(p));

    // Cells for any canvas.
    engine.register_fn("cells", |w: Dynamic, h: Dynamic| -> Res<Grid> {
        let mut m = serde_json::Map::new();
        m.insert("size".into(), json!([n(&w)?, n(&h)?]));
        page::cells_from(&m).map(Grid).map_err(err)
    });
    engine.register_fn(
        "cells",
        |w: Dynamic, h: Dynamic, cell: Dynamic| -> Res<Grid> {
            let mut m = serde_json::Map::new();
            m.insert("size".into(), json!([n(&w)?, n(&h)?]));
            m.insert("cell".into(), json!(n(&cell)?));
            page::cells_from(&m).map(Grid).map_err(err)
        },
    );
    engine.register_fn("cells", |opts: Map| -> Res<Grid> {
        page::cells_from(&map_json(opts)?).map(Grid).map_err(err)
    });
    engine.register_fn("cell", |g: &mut Grid, name: &str| -> Res<Dynamic> {
        let (col, row) = g.0.parse(name).map_err(err)?;
        from_json(&page::cell_map(&g.0, col, row))
    });
    engine.register_fn("cell", |g: &mut Grid, col: INT, row: INT| -> Res<Dynamic> {
        let name = Cells::name(col.max(0) as usize, row.max(0) as usize);
        let (col, row) = g.0.parse(&name).map_err(err)?;
        from_json(&page::cell_map(&g.0, col, row))
    });
    engine.register_fn(
        "at",
        |g: &mut Grid, x: Dynamic, y: Dynamic| -> Res<String> {
            Ok(page::cell_at(&g.0, n(&x)?, n(&y)?))
        },
    );
    engine.register_fn("all", |g: &mut Grid| -> Res<Array> {
        let mut out = Array::new();
        for row in 0..g.0.rows {
            for col in 0..g.0.cols {
                out.push(from_json(&page::cell_map(&g.0, col, row))?);
            }
        }
        Ok(out)
    });
    engine.register_fn("name", |_: &mut Grid, col: INT, row: INT| {
        Cells::name(col.max(0) as usize, row.max(0) as usize)
    });
    engine.register_get("cols", |g: &mut Grid| g.0.cols as INT);
    engine.register_get("rows", |g: &mut Grid| g.0.rows as INT);
    engine.register_get("size", |g: &mut Grid| g.0.step);
    engine.register_get("left", |g: &mut Grid| g.0.left);
    engine.register_get("top", |g: &mut Grid| g.0.top);
    engine.register_fn("to_string", |g: &mut Grid| g.0.describe());
    engine.register_fn("to_debug", |g: &mut Grid| g.0.describe());
}

// -- data: files, the web, memory, text ----------------------------------------

fn data_api(engine: &mut Engine, ctx: &Rc<Ctx>) {
    let c = ctx.clone();
    engine.register_fn("read_text", move |path: &str| {
        io::read_text(&c.env, path).map_err(err)
    });
    let c = ctx.clone();
    engine.register_fn("read_json", move |path: &str| -> Res<Dynamic> {
        let text = io::read_text(&c.env, path).map_err(err)?;
        let v: Value =
            serde_json::from_str(&text).map_err(|e| err(format!("{path} is not JSON: {e}")))?;
        from_json(&v)
    });
    let csv = |text: &str, header: bool| -> Res<Dynamic> {
        let rows = io::parse_csv(text);
        if !header {
            return from_json(&json!(rows));
        }
        let mut it = rows.into_iter();
        let keys: Vec<String> = it
            .next()
            .unwrap_or_default()
            .into_iter()
            .map(|k| match k {
                Value::String(s) => s.trim().to_string(),
                other => other.to_string(),
            })
            .collect();
        let maps: Vec<Value> = it
            .map(|r| {
                Value::Object(
                    keys.iter()
                        .cloned()
                        .zip(r.into_iter().chain(std::iter::repeat(json!(""))))
                        .collect(),
                )
            })
            .collect();
        from_json(&Value::Array(maps))
    };
    let c = ctx.clone();
    engine.register_fn("read_csv", move |path: &str| {
        csv(&io::read_text(&c.env, path).map_err(err)?, false)
    });
    let c = ctx.clone();
    engine.register_fn("read_csv", move |path: &str, header: bool| {
        csv(&io::read_text(&c.env, path).map_err(err)?, header)
    });
    engine.register_fn("parse_csv", move |text: &str| csv(text, false));
    engine.register_fn("parse_csv", move |text: &str, header: bool| {
        csv(text, header)
    });
    engine.register_fn("to_csv", |rows: Array| -> Res<String> {
        let rows: Vec<Value> = rows.iter().map(to_json).collect::<Res<_>>()?;
        Ok(io::to_csv(&rows))
    });
    let written = |r: Result<std::path::PathBuf, String>| -> Res<String> {
        r.map(|p| p.display().to_string()).map_err(err)
    };
    let c = ctx.clone();
    engine.register_fn("write_text", move |path: &str, text: &str| {
        written(io::write_text(&c.env, path, text, false))
    });
    let c = ctx.clone();
    engine.register_fn("append_text", move |path: &str, text: &str| {
        written(io::write_text(&c.env, path, text, true))
    });
    let c = ctx.clone();
    engine.register_fn(
        "write_json",
        move |path: &str, value: Dynamic| -> Res<String> {
            let text = serde_json::to_string_pretty(&to_json(&value)?).map_err(err)?;
            written(io::write_text(&c.env, path, &text, false))
        },
    );
    let c = ctx.clone();
    engine.register_fn("write_csv", move |path: &str, rows: Array| -> Res<String> {
        let rows: Vec<Value> = rows.iter().map(to_json).collect::<Res<_>>()?;
        written(io::write_text(&c.env, path, &io::to_csv(&rows), false))
    });
    let c = ctx.clone();
    engine.register_fn("exists", move |path: &str| io::exists(&c.env, path));
    let c = ctx.clone();
    engine.register_fn("list_files", move || -> Res<Array> {
        Ok(io::list_files(&c.env, "")
            .map_err(err)?
            .into_iter()
            .map(Dynamic::from)
            .collect())
    });
    let c = ctx.clone();
    engine.register_fn("list_files", move |path: &str| -> Res<Array> {
        Ok(io::list_files(&c.env, path)
            .map_err(err)?
            .into_iter()
            .map(Dynamic::from)
            .collect())
    });
    let c = ctx.clone();
    engine.register_fn("workspace", move || c.env.workspace().display().to_string());

    // The web.
    let request = |opts: Map| -> Res<io::Fetch> {
        let o = map_json(opts)?;
        let mut req = io::Fetch::default();
        for (k, v) in o {
            match k.as_str() {
                "method" => req.method = v.as_str().map(str::to_string),
                "timeout" => req.timeout = v.as_f64(),
                "body" => {
                    req.body = Some(match v {
                        Value::String(s) => s,
                        other => {
                            req.headers
                                .push(("Content-Type".into(), "application/json".into()));
                            other.to_string()
                        }
                    })
                }
                "headers" => {
                    for (hk, hv) in v.as_object().cloned().unwrap_or_default() {
                        let hv = match hv {
                            Value::String(s) => s,
                            other => other.to_string(),
                        };
                        req.headers.push((hk, hv));
                    }
                }
                other => {
                    return Err(err(format!(
                        "fetch options are method, headers, body and timeout, not {other}"
                    )));
                }
            }
        }
        Ok(req)
    };
    let c = ctx.clone();
    engine.register_fn("fetch", move |url: &str| -> Res<String> {
        c.check()?;
        io::fetch(&c.env, url, &io::Fetch::default(), c.deadline).map_err(err)
    });
    let c = ctx.clone();
    engine.register_fn("fetch", move |url: &str, opts: Map| -> Res<String> {
        c.check()?;
        io::fetch(&c.env, url, &request(opts)?, c.deadline).map_err(err)
    });
    let json_of = |text: String| -> Res<Dynamic> {
        let v: Value = serde_json::from_str(&text).map_err(|e| {
            let start: String = text.chars().take(120).collect();
            err(format!("the answer is not JSON ({e}): {start}"))
        })?;
        from_json(&v)
    };
    let c = ctx.clone();
    engine.register_fn("fetch_json", move |url: &str| -> Res<Dynamic> {
        c.check()?;
        json_of(io::fetch(&c.env, url, &io::Fetch::default(), c.deadline).map_err(err)?)
    });
    let c = ctx.clone();
    engine.register_fn("fetch_json", move |url: &str, opts: Map| -> Res<Dynamic> {
        c.check()?;
        json_of(io::fetch(&c.env, url, &request(opts)?, c.deadline).map_err(err)?)
    });
    let c = ctx.clone();
    engine.register_fn("download", move |url: &str, path: &str| -> Res<String> {
        c.check()?;
        written(io::download(&c.env, url, path, c.deadline))
    });

    // Memory kept between runs.
    let c = ctx.clone();
    engine.register_fn("remember", move |key: &str, value: Dynamic| -> Res<()> {
        io::remember(&c.env, key, to_json(&value)?).map_err(err)
    });
    let c = ctx.clone();
    engine.register_fn("recall", move |key: &str| -> Res<Dynamic> {
        from_json(io::memory(&c.env).get(key).unwrap_or(&Value::Null))
    });
    let c = ctx.clone();
    engine.register_fn(
        "recall",
        move |key: &str, default: Dynamic| -> Res<Dynamic> {
            match io::memory(&c.env).get(key) {
                Some(v) => from_json(v),
                None => Ok(default),
            }
        },
    );
    let c = ctx.clone();
    engine.register_fn("forget", move |key: &str| {
        io::remember(&c.env, key, Value::Null).map_err(err)
    });
    let c = ctx.clone();
    engine.register_fn("memory", move || {
        from_json(&Value::Object(io::memory(&c.env)))
    });

    // JSON.
    engine.register_fn("parse_json", |text: &str| -> Res<Dynamic> {
        let v: Value = serde_json::from_str(text).map_err(|e| err(format!("not JSON: {e}")))?;
        from_json(&v)
    });
    engine.register_fn("to_json", |v: Dynamic| -> Res<String> {
        Ok(to_json(&v)?.to_string())
    });
    engine.register_fn("pretty_json", |v: Dynamic| -> Res<String> {
        serde_json::to_string_pretty(&to_json(&v)?).map_err(err)
    });

    // Text.
    let c = ctx.clone();
    engine.register_fn(
        "regex_test",
        move |text: &str, pattern: &str| -> Res<bool> { Ok(c.regex(pattern)?.is_match(text)) },
    );
    let c = ctx.clone();
    engine.register_fn(
        "regex_find",
        move |text: &str, pattern: &str| -> Res<Array> {
            Ok(c.regex(pattern)?
                .find_iter(text)
                .take(100_000)
                .map(|m| Dynamic::from(m.as_str().to_string()))
                .collect())
        },
    );
    let c = ctx.clone();
    engine.register_fn(
        "regex_groups",
        move |text: &str, pattern: &str| -> Res<Array> {
            Ok(c.regex(pattern)?
                .captures_iter(text)
                .take(100_000)
                .map(|caps| {
                    let groups: Array = caps
                        .iter()
                        .map(|g| Dynamic::from(g.map_or(String::new(), |g| g.as_str().to_string())))
                        .collect();
                    Dynamic::from_array(groups)
                })
                .collect())
        },
    );
    let c = ctx.clone();
    engine.register_fn(
        "regex_replace",
        move |text: &str, pattern: &str, with: &str| -> Res<String> {
            Ok(c.regex(pattern)?.replace_all(text, with).into_owned())
        },
    );
    let c = ctx.clone();
    engine.register_fn("numbers", move |text: &str| -> Res<Array> {
        let r = c.regex(r"-?\d+(?:\.\d+)?(?:[eE][-+]?\d+)?")?;
        Ok(r.find_iter(text)
            .filter_map(|m| {
                let s = m.as_str();
                if s.contains(['.', 'e', 'E']) {
                    s.parse::<FLOAT>().ok().map(Dynamic::from_float)
                } else {
                    s.parse::<INT>().ok().map(Dynamic::from_int)
                }
            })
            .collect())
    });
    // As in JavaScript, these give the new text (Rhai's change it in place
    // and give nothing).
    engine.register_fn("trim", |s: &str| s.trim().to_string());
    engine.register_fn(
        "replace",
        |s: &str, find: &str, with: &str| -> Res<String> {
            // Its size is known before it is made: a replace that would fill
            // the memory is refused, not attempted.
            let count = if find.is_empty() {
                s.chars().count() + 1
            } else {
                s.matches(find).count()
            };
            let size =
                (s.len() - count * find.len()).saturating_add(count.saturating_mul(with.len()));
            if size > MAX_TEXT {
                return Err(format!(
                    "replace would make a text of {} MB; texts are at most {} MB",
                    size / (1024 * 1024),
                    MAX_TEXT / (1024 * 1024)
                )
                .into());
            }
            Ok(s.replace(find, with))
        },
    );
    engine.register_fn("fixed", |x: Dynamic, digits: INT| -> Res<String> {
        Ok(format!("{:.*}", digits.clamp(0, 12) as usize, n(&x)?))
    });

    // Time.
    engine.register_fn("now", || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64())
    });
    engine.register_fn("date", || {
        utc_date(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64),
        )
    });
    engine.register_fn("date", |secs: Dynamic| -> Res<String> {
        Ok(utc_date(n(&secs)? as i64))
    });
    let c = ctx.clone();
    engine.register_fn("elapsed", move || c.started.elapsed().as_secs_f64());
}

// -- maths, random numbers, colours ---------------------------------------------

/// A maths function of one number.
type Maths = fn(f64) -> f64;

fn maths_api(engine: &mut Engine, ctx: &Rc<Ctx>) {
    // Rhai's maths takes decimals only: sin(1) works as sin(1.0).
    let unary: [(&str, Maths); 17] = [
        ("sin", f64::sin),
        ("cos", f64::cos),
        ("tan", f64::tan),
        ("asin", f64::asin),
        ("acos", f64::acos),
        ("atan", f64::atan),
        ("sinh", f64::sinh),
        ("cosh", f64::cosh),
        ("tanh", f64::tanh),
        ("sqrt", f64::sqrt),
        ("exp", f64::exp),
        ("ln", f64::ln),
        ("log", f64::log10),
        ("floor", f64::floor),
        ("ceiling", f64::ceil),
        ("round", f64::round),
        ("cbrt", f64::cbrt),
    ];
    for (name, f) in unary {
        engine.register_fn(name, move |x: INT| f(x as f64));
    }
    engine.register_fn("hypot", |a: Dynamic, b: Dynamic| -> Res<FLOAT> {
        Ok(n(&a)?.hypot(n(&b)?))
    });
    engine.register_fn("atan", |y: Dynamic, x: Dynamic| -> Res<FLOAT> {
        Ok(n(&y)?.atan2(n(&x)?))
    });
    engine.register_fn(
        "clamp",
        |x: Dynamic, lo: Dynamic, hi: Dynamic| -> Res<FLOAT> {
            let (lo, hi) = (n(&lo)?, n(&hi)?);
            Ok(n(&x)?.max(lo.min(hi)).min(hi.max(lo)))
        },
    );
    engine.register_fn("lerp", |a: Dynamic, b: Dynamic, t: Dynamic| -> Res<FLOAT> {
        let (a, b) = (n(&a)?, n(&b)?);
        Ok(a + (b - a) * n(&t)?)
    });
    engine.register_fn(
        "dist",
        |x1: Dynamic, y1: Dynamic, x2: Dynamic, y2: Dynamic| -> Res<FLOAT> {
            Ok((n(&x2)? - n(&x1)?).hypot(n(&y2)? - n(&y1)?))
        },
    );
    engine.register_fn("deg", |r: Dynamic| -> Res<FLOAT> {
        Ok(n(&r)?.to_degrees())
    });
    engine.register_fn("rad", |d: Dynamic| -> Res<FLOAT> {
        Ok(n(&d)?.to_radians())
    });
    engine.register_fn("round", |x: Dynamic, digits: INT| -> Res<FLOAT> {
        let p = 10f64.powi(digits.clamp(-12, 12) as i32);
        Ok((n(&x)? * p).round() / p)
    });

    let c = ctx.clone();
    engine.register_fn("random", move || c.unit());
    let c = ctx.clone();
    engine.register_fn("random", move |a: Dynamic, b: Dynamic| -> Res<Dynamic> {
        if let (Ok(a), Ok(b)) = (a.as_int(), b.as_int()) {
            let (lo, hi) = (a.min(b), a.max(b));
            let span = (hi - lo) as u64 + 1;
            return Ok(Dynamic::from_int(lo + (c.next() % span.max(1)) as INT));
        }
        let (a, b) = (n(&a)?, n(&b)?);
        Ok(Dynamic::from_float(a + (b - a) * c.unit()))
    });
    let c = ctx.clone();
    engine.register_fn("seed", move |s: INT| {
        // splitmix64, so nearby seeds give unrelated numbers.
        let mut z = (s as u64).wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        c.rng.set((z ^ (z >> 31)) | 1);
    });
    let c = ctx.clone();
    engine.register_fn("shuffle", move |mut a: Array| {
        for i in (1..a.len()).rev() {
            let j = (c.next() % (i as u64 + 1)) as usize;
            a.swap(i, j);
        }
        a
    });
    let c = ctx.clone();
    engine.register_fn("choice", move |a: Array| -> Res<Dynamic> {
        if a.is_empty() {
            return Err(err("choice of an empty array"));
        }
        Ok(a[(c.next() % a.len() as u64) as usize].clone())
    });

    engine.register_fn("rgb", |r: Dynamic, g: Dynamic, b: Dynamic| -> Res<String> {
        Ok(hex([n(&r)?, n(&g)?, n(&b)?]))
    });
    engine.register_fn("hsl", |h: Dynamic, s: Dynamic, l: Dynamic| -> Res<String> {
        Ok(hex(hsl(n(&h)?, n(&s)?, n(&l)?)))
    });
    engine.register_fn("color_rgb", |c: &str| -> Res<Array> {
        Ok(parse_hex(c)?
            .iter()
            .map(|v| Dynamic::from_int(*v as INT))
            .collect())
    });
    engine.register_fn("mix", |a: &str, b: &str, t: Dynamic| -> Res<String> {
        let (a, b, t) = (parse_hex(a)?, parse_hex(b)?, n(&t)?.clamp(0.0, 1.0));
        Ok(hex([0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)))
    });
}
