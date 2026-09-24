#![allow(dead_code)]
//! TEMPORARY stub — replaced by the real backend.
use crate::backend::{Backend, Native};
use crate::error::{Error, Result};
use crate::keys::KeyCombo;
use crate::types::*;

pub struct WindowsStubBackend;
impl WindowsStubBackend {
    pub fn new() -> Result<Self> { Err(Error::Unsupported("backend not built yet".into())) }
}
impl Backend for WindowsStubBackend {
    fn name(&self) -> &'static str { "stub" }
    fn permissions(&mut self) -> Vec<PermissionStatus> { vec![] }
    fn list_apps(&mut self) -> Result<Vec<AppInfo>> { Err(Error::Unsupported("stub".into())) }
    fn launch_app(&mut self, _q: &str) -> Result<()> { Err(Error::Unsupported("stub".into())) }
    fn list_windows(&mut self, _a: &AppInfo) -> Result<Vec<WindowInfo>> { Err(Error::Unsupported("stub".into())) }
    fn snapshot(&mut self, _a: &AppInfo, _w: &WindowInfo, _o: &SnapshotOptions) -> Result<Vec<RawNode>> { Err(Error::Unsupported("stub".into())) }
    fn capture(&mut self, _a: &AppInfo, _w: &WindowInfo) -> Result<Capture> { Err(Error::Unsupported("stub".into())) }
    fn perform_action(&mut self, _e: ElementHandle, _n: &str) -> Result<()> { Err(Error::Unsupported("stub".into())) }
    fn set_value(&mut self, _e: ElementHandle, _v: &str) -> Result<()> { Err(Error::Unsupported("stub".into())) }
    fn select_text(&mut self, _e: ElementHandle, _t: Option<&str>, _o: usize) -> Result<()> { Err(Error::Unsupported("stub".into())) }
    fn focus(&mut self, _e: ElementHandle) -> Result<Native> { Err(Error::Unsupported("stub".into())) }
    fn scroll_element(&mut self, _e: ElementHandle, _d: ScrollDirection, _p: f64) -> Result<Native> { Err(Error::Unsupported("stub".into())) }
    fn click(&mut self, _t: &InputTarget, _at: Point, _b: MouseButton, _c: u8) -> Result<()> { Err(Error::Unsupported("stub".into())) }
    fn drag(&mut self, _t: &InputTarget, _f: Point, _to: Point) -> Result<()> { Err(Error::Unsupported("stub".into())) }
    fn scroll_wheel(&mut self, _t: &InputTarget, _at: Point, _dx: i32, _dy: i32) -> Result<()> { Err(Error::Unsupported("stub".into())) }
    fn press_key(&mut self, _t: &InputTarget, _c: &KeyCombo) -> Result<()> { Err(Error::Unsupported("stub".into())) }
    fn type_text(&mut self, _t: &InputTarget, _txt: &str) -> Result<()> { Err(Error::Unsupported("stub".into())) }
}

pub type WindowsBackend = WindowsStubBackend;
