//! A thin blocking wrapper over the AT-SPI2 D-Bus API.
//!
//! AT-SPI exposes the desktop accessibility tree on a dedicated bus. The
//! address of that bus is `$AT_SPI_BUS_ADDRESS`, else the X root window's
//! `AT_SPI_BUS`, else asked of `org.a11y.Bus` on the session bus; every
//! accessible is addressed by `(bus_name, object_path)`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};

use crate::error::{Error, Result};

const A11Y_IFACE: &str = "org.a11y.atspi.Accessible";
const COMPONENT_IFACE: &str = "org.a11y.atspi.Component";
const ACTION_IFACE: &str = "org.a11y.atspi.Action";
const TEXT_IFACE: &str = "org.a11y.atspi.Text";
const EDITABLE_IFACE: &str = "org.a11y.atspi.EditableText";
const VALUE_IFACE: &str = "org.a11y.atspi.Value";
const DOCUMENT_IFACE: &str = "org.a11y.atspi.Document";
const PROPS_IFACE: &str = "org.freedesktop.DBus.Properties";
const APP_IFACE: &str = "org.a11y.atspi.Application";
const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";
/// How long any one app may take to answer: a frozen app times out instead
/// of hanging every listing and snapshot.
const METHOD_TIMEOUT: Duration = Duration::from_secs(3);
/// A snapshot gives up after this long and returns the part of the tree it
/// has read (a slow app with a huge tree).
const WALK_DEADLINE: Duration = Duration::from_secs(8);
/// A container with more children than this is not asked for all of them at
/// once (the app would build an accessible for each: a 100 000-row table).
const HUGE_CHILD_COUNT: i32 = 2000;
/// How many children of such a container (or of one that manages its
/// descendants) are read, one by one.
const CHILD_CAP: i32 = 256;
/// AT-SPI's relation "labelled by" (AtspiRelationType).
const RELATION_LABELLED_BY: u32 = 2;

/// Number of D-Bus round trips made so far (diagnostics / benchmarking).
static IPC_CALLS: AtomicU64 = AtomicU64::new(0);

pub fn ipc_calls() -> u64 {
    IPC_CALLS.load(Ordering::Relaxed)
}

fn count() {
    IPC_CALLS.fetch_add(1, Ordering::Relaxed);
}

/// A reference to one accessible element.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjRef {
    pub bus: String,
    pub path: String,
}

impl ObjRef {
    fn is_null(&self) -> bool {
        self.bus.is_empty() || self.path.is_empty() || self.path.ends_with("/null")
    }
}

/// AT-SPI StateType bits we care about (index into the 64-bit state set).
pub mod state {
    pub const EDITABLE: u32 = 7;
    pub const ENABLED: u32 = 8;
    pub const EXPANDABLE: u32 = 9;
    pub const EXPANDED: u32 = 10;
    pub const FOCUSED: u32 = 12;
    pub const SELECTED: u32 = 23;
    pub const SENSITIVE: u32 = 24;
    pub const SHOWING: u32 = 25;
    pub const VISIBLE: u32 = 30;
    pub const MANAGES_DESCENDANTS: u32 = 31;
    pub const CHECKED: u32 = 4;
    pub const CHECKABLE: u32 = 41;
    pub const PRESSED: u32 = 20;
    pub const ACTIVE: u32 = 1;
}

/// A 64-bit AT-SPI state set.
#[derive(Debug, Clone, Copy, Default)]
pub struct States(pub u64);

impl States {
    fn from_pair(v: &[u32]) -> Self {
        let lo = v.first().copied().unwrap_or(0) as u64;
        let hi = v.get(1).copied().unwrap_or(0) as u64;
        States(lo | (hi << 32))
    }
    pub fn has(self, bit: u32) -> bool {
        self.0 & (1u64 << bit) != 0
    }
}

/// Element properties AT-SPI reports.
#[derive(Debug, Clone, Default)]
pub struct Accessible {
    pub name: String,
    pub description: String,
    pub role_name: String,
    pub child_count: i32,
    pub states: States,
    pub interfaces: Vec<String>,
}

impl Accessible {
    pub fn has_iface(&self, short: &str) -> bool {
        let full = format!("org.a11y.atspi.{short}");
        self.interfaces.iter().any(|i| i == &full || i == short)
    }
}

/// Why a call failed, as far as callers need to tell failures apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fail {
    /// No answer in time: the app is busy or frozen. A request that does
    /// something (DoAction) may still take effect.
    Timeout,
    /// The element or its app is gone (`UnknownObject`, `ServiceUnknown`).
    Gone,
    /// The connection to the accessibility bus broke (the bus restarted).
    Disconnected,
    /// Anything else: no such interface or method, an unexpected reply.
    Other,
}

impl Fail {
    fn of(e: &zbus::Error) -> Fail {
        let by_name = |name: &str| match name {
            "org.freedesktop.DBus.Error.NoReply"
            | "org.freedesktop.DBus.Error.Timeout"
            | "org.freedesktop.DBus.Error.TimedOut" => Fail::Timeout,
            "org.freedesktop.DBus.Error.UnknownObject"
            | "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner" => Fail::Gone,
            _ => Fail::Other,
        };
        match e {
            // zbus reports its own method timeout as a timed-out I/O error,
            // and a broken socket as any other I/O error.
            zbus::Error::InputOutput(io) if io.kind() == std::io::ErrorKind::TimedOut => {
                Fail::Timeout
            }
            zbus::Error::InputOutput(_) => Fail::Disconnected,
            zbus::Error::MethodError(name, _, _) => by_name(name.as_str()),
            zbus::Error::FDO(e) => match &**e {
                zbus::fdo::Error::ZBus(e) => Fail::of(e),
                e => by_name(zbus::DBusError::name(e).as_str()),
            },
            _ => Fail::Other,
        }
    }

    /// The most telling of several failures of one element's queries.
    fn worst(fails: &[Fail]) -> Fail {
        [Fail::Disconnected, Fail::Timeout, Fail::Gone]
            .into_iter()
            .find(|f| fails.contains(f))
            .unwrap_or(Fail::Other)
    }
}

/// A failed AT-SPI call: what failed, and how.
#[derive(Debug, Clone)]
pub struct CallError {
    pub fail: Fail,
    msg: String,
}

impl CallError {
    fn new(what: &str, e: &zbus::Error) -> Self {
        let fail = Fail::of(e);
        let msg = match fail {
            Fail::Timeout => format!(
                "{what}: no answer within {} s (the app is busy or not responding)",
                METHOD_TIMEOUT.as_secs()
            ),
            Fail::Disconnected => format!("{what}: the accessibility bus connection broke ({e})"),
            _ => format!("{what}: {e}"),
        };
        Self { fail, msg }
    }

    fn other(e: impl std::fmt::Display) -> Self {
        Self {
            fail: Fail::Other,
            msg: format!("AT-SPI: {e}"),
        }
    }

    #[cfg(test)]
    pub fn of(fail: Fail) -> Self {
        Self {
            fail,
            msg: format!("{fail:?}"),
        }
    }
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl From<CallError> for Error {
    fn from(e: CallError) -> Self {
        Error::Platform(e.msg)
    }
}

type CallResult<T> = std::result::Result<T, CallError>;

pub struct AtspiConnection {
    conn: Connection,
    /// Set once a call found the connection broken: the owner reconnects.
    lost: AtomicBool,
    /// What an element keeps as long as it lives (its role and interfaces),
    /// so a tree read again (as it is several times after every action)
    /// doesn't ask for it again: only for apps whose elements' paths are
    /// never reused (see [`AtspiConnection::paths_unique`]).
    lasting: std::sync::Mutex<HashMap<ObjRef, Lasting>>,
    /// Per app (its bus name): whether a path, once gone, never names
    /// another element.
    unique_paths: std::sync::Mutex<HashMap<String, bool>>,
}

/// What doesn't change while an element lives.
#[derive(Clone)]
struct Lasting {
    role: String,
    interfaces: Vec<String>,
}

/// Elements whose role and interfaces are remembered, at most (the oldest
/// are forgotten all at once past it).
const LASTING_MAX: usize = 100_000;

/// Whether an app's toolkit names its elements from a counter, never
/// reusing a path for another element: GTK (3 through at-spi2-atk, and 4).
/// Others (Qt, Firefox) name them after their address in memory, which a
/// new element can be given once the old one is freed.
fn counts_paths(toolkit: &str) -> bool {
    toolkit.trim().eq_ignore_ascii_case("gtk")
}

/// A connection made on a thread of its own, given up after a while: the
/// method timeout covers calls, not connecting (the handshake and Hello),
/// and a bus daemon that hangs with its socket still there would hang the
/// server. While an earlier attempt is still unanswered, none is started.
fn connect_within(
    what: &'static str,
    make: impl FnOnce() -> Result<Connection> + Send + 'static,
) -> Result<Connection> {
    static UNANSWERED: AtomicBool = AtomicBool::new(false);
    const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    if UNANSWERED.load(Ordering::SeqCst) {
        return Err(Error::Platform(format!(
            "the {what} hasn't answered an earlier connection attempt"
        )));
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("a11y-connect".into())
        .spawn(move || {
            let r = make();
            // Only one attempt is ever left waiting: it clears the flag
            // when it ends, however late.
            if tx.send(r).is_err() {
                UNANSWERED.store(false, Ordering::SeqCst);
            }
        });
    if let Err(e) = spawned {
        return Err(Error::Platform(format!(
            "cannot connect to the {what}: {e}"
        )));
    }
    match rx.recv_timeout(CONNECT_TIMEOUT) {
        Ok(r) => r,
        Err(_) => {
            UNANSWERED.store(true, Ordering::SeqCst);
            drop(rx);
            Err(Error::Platform(format!(
                "the {what} didn't answer in {}s",
                CONNECT_TIMEOUT.as_secs()
            )))
        }
    }
}

/// The session bus, with a timeout on calls: a hung bus launcher must not
/// hang the server.
fn session() -> Result<Connection> {
    connect_within("session bus", || {
        zbus::blocking::connection::Builder::session()
            .map_err(bus_err)?
            .method_timeout(METHOD_TIMEOUT)
            .build()
            .map_err(bus_err)
    })
}

/// Where the accessibility bus is, in the order AT-SPI's own library looks:
/// `$AT_SPI_BUS_ADDRESS`, the X root window's `AT_SPI_BUS` property (set
/// by the bus launcher when there is an X server; asked only if needed),
/// then the bus launcher on the session bus.
fn bus_address(
    env: Option<String>,
    x_root: impl FnOnce() -> Option<String>,
    launcher: impl FnOnce() -> Result<String>,
) -> Result<String> {
    if let Some(a) = env.filter(|a| !a.trim().is_empty()) {
        return Ok(a);
    }
    if let Some(a) = x_root().filter(|a| !a.trim().is_empty()) {
        return Ok(a);
    }
    launcher()
}

/// Whether accessibility is switched on for the session
/// (`org.a11y.Status.IsEnabled`); `None` when there is no one to ask.
pub fn enabled() -> Option<bool> {
    status(&session().ok()?)
}

fn status(session: &Connection) -> Option<bool> {
    let reply = session
        .call_method(
            Some("org.a11y.Bus"),
            "/org/a11y/bus",
            Some(PROPS_IFACE),
            "Get",
            &("org.a11y.Status", "IsEnabled"),
        )
        .ok()?;
    let v: OwnedValue = reply.body().deserialize().ok()?;
    bool::try_from(v).ok()
}

/// Switch accessibility on for the session when it is off: it is off by
/// default outside GNOME (KDE, Xfce, sway, Hyprland), and Qt, Firefox and
/// Chromium then don't expose their elements. Apps started from then on
/// (and Qt ones that are running) do.
fn switch_on() {
    let Ok(session) = session() else {
        return;
    };
    if status(&session) != Some(false) {
        return;
    }
    let on = zbus::zvariant::Value::from(true);
    match session.call_method(
        Some("org.a11y.Bus"),
        "/org/a11y/bus",
        Some(PROPS_IFACE),
        "Set",
        &("org.a11y.Status", "IsEnabled", on),
    ) {
        Ok(_) => log::info!("switched accessibility on (org.a11y.Status.IsEnabled)"),
        Err(e) => log::warn!("cannot switch accessibility on: {e}"),
    }
}

impl AtspiConnection {
    /// The accessibility bus's address (see [`bus_address`]); `x_root`
    /// reads the X root window's `AT_SPI_BUS`.
    fn address(x_root: impl FnOnce() -> Option<String>) -> Result<String> {
        bus_address(std::env::var("AT_SPI_BUS_ADDRESS").ok(), x_root, || {
            Self::ask_launcher()
        })
    }

    /// The accessibility bus's address, as its launcher on the session bus
    /// says.
    fn ask_launcher() -> Result<String> {
        let session = session()?;
        let reply = session
            .call_method(
                Some("org.a11y.Bus"),
                "/org/a11y/bus",
                Some("org.a11y.Bus"),
                "GetAddress",
                &(),
            )
            .map_err(|e| {
                Error::Platform(format!(
                    "AT-SPI accessibility bus is not available ({e}). Is at-spi2 running and accessibility enabled?"
                ))
            })?;
        reply.body().deserialize().map_err(bus_err)
    }

    /// Connect to the accessibility bus (see [`bus_address`]; `x_root`
    /// reads the X root window's `AT_SPI_BUS`), switching accessibility on
    /// for the session if it is off.
    pub fn connect(x_root: impl FnOnce() -> Option<String>) -> Result<Self> {
        let addr = Self::address(x_root)?;
        switch_on();
        let conn = match Self::open(&addr) {
            Ok(c) => c,
            // An address from the environment or the X server can be
            // another machine's (ssh -X) or a session's that ended: the
            // launcher's, if it says another.
            Err(e) => match Self::ask_launcher() {
                Ok(other) if other != addr => Self::open(&other)?,
                _ => return Err(e),
            },
        };
        Ok(Self {
            conn,
            lost: AtomicBool::new(false),
            lasting: Default::default(),
            unique_paths: Default::default(),
        })
    }

    fn open(addr: &str) -> Result<Connection> {
        // The timeout covers every call on this connection, blocking and
        // async (`fetch_many`, `walk`, `pids_of`) alike; connecting has a
        // limit of its own.
        let addr = addr.to_string();
        connect_within("accessibility bus", move || {
            zbus::blocking::connection::Builder::address(addr.as_str())
                .map_err(bus_err)?
                .method_timeout(METHOD_TIMEOUT)
                .build()
                .map_err(|e| Error::Platform(format!("cannot connect to the a11y bus: {e}")))
        })
    }

    /// Whether the connection broke (the bus went away): reconnect.
    pub fn lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }

    fn note(&self, e: CallError) -> CallError {
        if e.fail == Fail::Disconnected {
            self.lost.store(true, Ordering::Relaxed);
        }
        e
    }

    pub fn root(&self) -> ObjRef {
        ObjRef {
            bus: "org.a11y.atspi.Registry".into(),
            path: ROOT_PATH.into(),
        }
    }

    async fn acall<B, R>(&self, r: &ObjRef, iface: &str, method: &str, body: &B) -> CallResult<R>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
    {
        count();
        let path = ObjectPath::try_from(r.path.as_str()).map_err(CallError::other)?;
        let reply = self
            .conn
            .inner()
            .call_method(Some(r.bus.as_str()), &path, Some(iface), method, body)
            .await
            .map_err(|e| self.note(CallError::new(&format!("{iface}.{method}"), &e)))?;
        reply.body().deserialize().map_err(CallError::other)
    }

    fn call<B, R>(&self, r: &ObjRef, iface: &str, method: &str, body: &B) -> CallResult<R>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
    {
        async_io::block_on(self.acall(r, iface, method, body))
    }

    pub fn children(&self, r: &ObjRef) -> CallResult<Vec<ObjRef>> {
        // GetChildren -> a(so). Present on modern AT-SPI.
        match self.call::<_, Vec<(String, OwnedObjectPath)>>(r, A11Y_IFACE, "GetChildren", &()) {
            Ok(list) => Ok(list
                .into_iter()
                .map(to_ref)
                .filter(|c| !c.is_null())
                .collect()),
            // Old AT-SPI without GetChildren; a busy, gone or disconnected
            // app would only fail the same way again, more slowly.
            Err(e) if e.fail == Fail::Other => self.children_by_index(r),
            Err(e) => Err(e),
        }
    }

    fn children_by_index(&self, r: &ObjRef) -> CallResult<Vec<ObjRef>> {
        let count = self.get_prop::<i32>(r, A11Y_IFACE, "ChildCount")?;
        let count = count.clamp(0, HUGE_CHILD_COUNT);
        Ok(async_io::block_on(self.children_at(r, count)).0)
    }

    /// Children `0..n` by index, concurrently, and the first timeout.
    async fn children_at(&self, r: &ObjRef, n: i32) -> (Vec<ObjRef>, Option<CallError>) {
        let got = futures_util::future::join_all((0..n).map(|i| async move {
            self.acall::<_, (String, OwnedObjectPath)>(r, A11Y_IFACE, "GetChildAtIndex", &(i,))
                .await
        }))
        .await;
        let mut out = Vec::new();
        let mut timeout = None;
        for g in got {
            match g {
                Ok(c) => {
                    let c = to_ref(c);
                    if !c.is_null() {
                        out.push(c);
                    }
                }
                Err(e) if e.fail == Fail::Timeout => timeout = Some(e),
                Err(_) => {}
            }
        }
        (out, timeout)
    }

    fn get_prop<T>(&self, r: &ObjRef, iface: &str, name: &str) -> CallResult<T>
    where
        T: TryFrom<OwnedValue>,
    {
        let v: OwnedValue = self.call(r, PROPS_IFACE, "Get", &(iface, name))?;
        T::try_from(v).map_err(|_| CallError::other(format!("unexpected type for {iface}.{name}")))
    }

    /// Read the common accessible properties (all queries at once); an
    /// error when the element answers none of them.
    pub fn describe(&self, r: &ObjRef) -> CallResult<Accessible> {
        let (nd, err) = async_io::block_on(self.fetch_props(r));
        match err {
            Some(e) => Err(e),
            None => Ok(nd.acc),
        }
    }

    /// (name, description, keybinding) for each action.
    pub fn actions(&self, r: &ObjRef) -> CallResult<Vec<(String, String, String)>> {
        self.call::<_, Vec<(String, String, String)>>(r, ACTION_IFACE, "GetActions", &())
    }

    /// Run an action. A timeout ([`Fail::Timeout`]) means it was sent but
    /// not answered: GTK runs a button's handler inside the call, so a
    /// handler that opens a modal dialog or takes long holds the reply,
    /// and the action may well have happened.
    pub fn do_action(&self, r: &ObjRef, index: i32) -> CallResult<bool> {
        self.call::<_, bool>(r, ACTION_IFACE, "DoAction", &(index,))
    }

    /// Index of the named action (case-insensitive, named as a snapshot
    /// names it), if the element has it. `#N` is the Nth action, for one
    /// with no name.
    pub fn action_index(&self, r: &ObjRef, name: &str) -> CallResult<Option<i32>> {
        let actions = match self.actions(r) {
            Ok(a) => a,
            // No Action interface at all: no such action.
            Err(e) if e.fail == Fail::Other => return Ok(None),
            Err(e) => return Err(e),
        };
        if let Some(i) = name.strip_prefix('#').and_then(|n| n.parse::<usize>().ok()) {
            return Ok((i < actions.len()).then_some(i as i32));
        }
        Ok(actions
            .into_iter()
            .position(|(n, d, _)| action_name(n, d).eq_ignore_ascii_case(name))
            .map(|i| i as i32))
    }

    /// The address of a web page (a browser's document), when it says:
    /// Firefox as "DocURL", Chromium as "URI".
    pub fn doc_url(&self, r: &ObjRef) -> Option<String> {
        ["DocURL", "URI"].into_iter().find_map(|attr| {
            self.call::<_, String>(r, DOCUMENT_IFACE, "GetAttributeValue", &(attr,))
                .ok()
                .map(|u| u.trim().to_string())
                .filter(|u| !u.is_empty())
        })
    }

    pub fn grab_focus(&self, r: &ObjRef) -> CallResult<bool> {
        self.call::<_, bool>(r, COMPONENT_IFACE, "GrabFocus", &())
    }

    pub fn set_text(&self, r: &ObjRef, text: &str) -> CallResult<bool> {
        self.call::<_, bool>(r, EDITABLE_IFACE, "SetTextContents", &(text,))
    }

    #[allow(dead_code)]
    pub fn insert_text(&self, r: &ObjRef, offset: i32, text: &str) -> CallResult<()> {
        let len = text.chars().count() as i32;
        self.call::<_, ()>(r, EDITABLE_IFACE, "InsertText", &(offset, text, len))
    }

    pub fn set_value(&self, r: &ObjRef, value: f64) -> CallResult<bool> {
        // CurrentValue is a read/write property of type double.
        let v = zbus::zvariant::Value::from(value);
        self.call::<_, ()>(r, PROPS_IFACE, "Set", &(VALUE_IFACE, "CurrentValue", v))
            .map(|_| true)
    }

    pub fn character_count(&self, r: &ObjRef) -> i32 {
        self.get_prop::<i32>(r, TEXT_IFACE, "CharacterCount")
            .unwrap_or(0)
    }

    pub fn get_text(&self, r: &ObjRef, start: i32, end: i32) -> CallResult<String> {
        self.call::<_, String>(r, TEXT_IFACE, "GetText", &(start, end))
    }

    pub fn set_selection(&self, r: &ObjRef, start: i32, end: i32) -> CallResult<bool> {
        // Selection index 0. Some toolkits require AddSelection first.
        if let Ok(true) = self.call::<_, bool>(r, TEXT_IFACE, "SetSelection", &(0i32, start, end)) {
            return Ok(true);
        }
        self.call::<_, bool>(r, TEXT_IFACE, "AddSelection", &(start, end))
    }

    pub fn set_caret(&self, r: &ObjRef, offset: i32) -> CallResult<bool> {
        self.call::<_, bool>(r, TEXT_IFACE, "SetCaretOffset", &(offset,))
    }
}

/// Everything a snapshot needs about one element.
#[derive(Debug, Default, Clone)]
pub struct NodeData {
    pub acc: Accessible,
    /// Screen-space (x, y, w, h), when the element has a Component.
    pub extents: Option<(i32, i32, i32, i32)>,
    /// Action names, in index order.
    pub actions: Vec<String>,
    /// Text content, for text elements.
    pub text: Option<String>,
    /// The text of the label it is "labelled by", for a control without a
    /// name of its own (GTK's mnemonic labels, `aria-labelledby`).
    pub label: Option<String>,
    pub children: Vec<ObjRef>,
    /// The element answered none of the basic queries (name, role, state),
    /// and why: gone, or its app is frozen (timed out).
    pub fail: Option<Fail>,
    /// Some query timed out: what is here may be incomplete.
    pub timed_out: bool,
    /// `acc.child_count` is what the element said (not a default).
    pub counted: bool,
}

/// An element returned by [`AtspiConnection::walk`], in pre-order.
pub struct Walked {
    pub r: ObjRef,
    pub data: NodeData,
    pub parent: Option<usize>,
}

/// An action's name as an app gives it (name, description). Some leave the
/// name empty, or put the key binding there (Firefox: ";;"): then the
/// description names it, else it is "action N" by its place — so every
/// action keeps its index, and none is shown as punctuation.
fn action_name(name: String, description: String) -> String {
    let word = |s: &str| s.chars().any(char::is_alphanumeric);
    if word(&name) && !name.contains(';') {
        return name;
    }
    if word(&description) && description.split_whitespace().count() <= 4 {
        return description;
    }
    String::new()
}

/// Whether an element's text content is worth reading (never passwords).
fn wants_text(acc: &Accessible) -> bool {
    if !acc.has_iface("Text") || acc.role_name == "password text" {
        return false;
    }
    acc.states.has(state::EDITABLE)
        || matches!(
            acc.role_name.as_str(),
            "entry" | "text" | "label" | "static" | "paragraph" | "heading"
        )
}

/// Controls that a separate label names, when they have no name of their
/// own (AT-SPI role names).
fn labelled(role: &str) -> bool {
    matches!(
        role,
        "text"
            | "entry"
            | "password text"
            | "spin button"
            | "combo box"
            | "slider"
            | "list"
            | "list box"
            | "table"
            | "tree table"
            | "tree"
            | "progress bar"
            | "date editor"
    )
}

/// One element waiting to be read by [`AtspiConnection::walk`].
struct Pending {
    r: ObjRef,
    depth: usize,
    /// Already read once, and timed out.
    retried: bool,
}

impl AtspiConnection {
    /// Fetch one element's properties, role, state, interfaces, extents and
    /// actions, all six queries in flight at once; with the error when it
    /// answered none of the basic ones.
    async fn fetch_props(&self, r: &ObjRef) -> (NodeData, Option<CallError>) {
        let unique = self.paths_unique(&r.bus).await;
        let known = unique
            .then(|| {
                self.lasting
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(r)
                    .cloned()
            })
            .flatten();
        let role = async {
            match &known {
                Some(k) => Ok(k.role.clone()),
                None => {
                    self.acall::<_, String>(r, A11Y_IFACE, "GetRoleName", &())
                        .await
                }
            }
        };
        let ifaces = async {
            match &known {
                Some(k) => Ok(k.interfaces.clone()),
                None => {
                    self.acall::<_, Vec<String>>(r, A11Y_IFACE, "GetInterfaces", &())
                        .await
                }
            }
        };
        let acts = async {
            // No Action interface: no actions to ask for.
            if known.as_ref().is_some_and(|k| {
                !k.interfaces
                    .iter()
                    .any(|i| i == ACTION_IFACE || i == "Action")
            }) {
                return Ok(Vec::new());
            }
            self.acall::<_, Vec<(String, String, String)>>(r, ACTION_IFACE, "GetActions", &())
                .await
        };
        let (props, role, state, ifaces, ext, acts) = futures_util::join!(
            self.acall::<_, HashMap<String, OwnedValue>>(r, PROPS_IFACE, "GetAll", &(A11Y_IFACE,)),
            role,
            self.acall::<_, Vec<u32>>(r, A11Y_IFACE, "GetState", &()),
            ifaces,
            self.acall::<_, (i32, i32, i32, i32)>(r, COMPONENT_IFACE, "GetExtents", &(0u32,)),
            acts,
        );
        if unique
            && known.is_none()
            && let (Ok(role), Ok(interfaces)) = (&role, &ifaces)
        {
            let mut lasting = self.lasting.lock().unwrap_or_else(|e| e.into_inner());
            if lasting.len() >= LASTING_MAX {
                lasting.clear();
            }
            lasting.insert(
                r.clone(),
                Lasting {
                    role: role.clone(),
                    interfaces: interfaces.clone(),
                },
            );
        }
        let fails: Vec<Fail> = [
            props.as_ref().err(),
            role.as_ref().err(),
            state.as_ref().err(),
            ifaces.as_ref().err(),
            ext.as_ref().err(),
            acts.as_ref().err(),
        ]
        .into_iter()
        .flatten()
        .map(|e| e.fail)
        .collect();
        let err = match (&props, &role, &state) {
            (Err(a), Err(b), Err(c)) => {
                let worst = Fail::worst(&[a.fail, b.fail, c.fail]);
                Some(
                    [a, b, c]
                        .into_iter()
                        .find(|e| e.fail == worst)
                        .unwrap_or(a)
                        .clone(),
                )
            }
            _ => None,
        };
        let mut acc = Accessible::default();
        let mut counted = false;
        if let Ok(props) = props {
            acc.name = props.get("Name").and_then(owned_string).unwrap_or_default();
            acc.description = props
                .get("Description")
                .and_then(owned_string)
                .unwrap_or_default();
            let count = props
                .get("ChildCount")
                .and_then(|v| i32::try_from(v.clone()).ok());
            counted = count.is_some();
            acc.child_count = count.unwrap_or(0);
        }
        acc.role_name = role.unwrap_or_default();
        if let Ok(v) = state {
            acc.states = States::from_pair(&v);
        }
        acc.interfaces = ifaces.unwrap_or_default();
        let nd = NodeData {
            acc,
            extents: ext.ok(),
            actions: acts
                .map(|a| a.into_iter().map(|(n, d, _)| action_name(n, d)).collect())
                .unwrap_or_default(),
            text: None,
            label: None,
            children: Vec::new(),
            fail: err.as_ref().map(|e| e.fail),
            timed_out: fails.contains(&Fail::Timeout),
            counted,
        };
        (nd, err)
    }

    /// Whether `bus`'s app never reuses an element's path (asked once per
    /// app: its toolkit's name).
    async fn paths_unique(&self, bus: &str) -> bool {
        if let Some(u) = self
            .unique_paths
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(bus)
        {
            return *u;
        }
        let root = ObjRef {
            bus: bus.to_string(),
            path: "/org/a11y/atspi/accessible/root".into(),
        };
        let toolkit: Option<String> = self
            .acall::<_, OwnedValue>(&root, PROPS_IFACE, "Get", &(APP_IFACE, "ToolkitName"))
            .await
            .ok()
            .and_then(|v| owned_string(&v));
        let unique = toolkit.as_deref().is_some_and(counts_paths);
        self.unique_paths
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(bus.to_string(), unique);
        unique
    }

    /// An element's children (when wanted), text content (when worth
    /// reading) and the label it is labelled by (when it has no name);
    /// whether any timed out.
    async fn fetch_rest(
        &self,
        r: &ObjRef,
        nd: &NodeData,
        kids: bool,
        text_end: i32,
    ) -> (Vec<ObjRef>, Option<String>, Option<String>, bool) {
        if nd.fail.is_some() {
            return (Vec::new(), None, None, false);
        }
        let children = async {
            if !kids {
                return (Vec::new(), false);
            }
            let n = nd.acc.child_count;
            // It said it has none: nothing to ask for (most elements).
            if nd.counted && n == 0 {
                return (Vec::new(), false);
            }
            if nd.acc.states.has(state::MANAGES_DESCENDANTS) || n > HUGE_CHILD_COUNT {
                // Its children are made on demand (a table or tree view):
                // only the first ones, never all of them at once.
                let (kids, timeout) = self.children_at(r, n.clamp(0, CHILD_CAP)).await;
                return (kids, timeout.is_some());
            }
            match self
                .acall::<_, Vec<(String, OwnedObjectPath)>>(r, A11Y_IFACE, "GetChildren", &())
                .await
            {
                Ok(k) => (
                    k.into_iter().map(to_ref).filter(|c| !c.is_null()).collect(),
                    false,
                ),
                Err(e) => (Vec::new(), e.fail == Fail::Timeout),
            }
        };
        let text = async {
            if !wants_text(&nd.acc) {
                return (None, false);
            }
            match self
                .acall::<_, String>(r, TEXT_IFACE, "GetText", &(0i32, text_end))
                .await
            {
                Ok(t) => (Some(t).filter(|s| !s.is_empty()), false),
                Err(e) => (None, e.fail == Fail::Timeout),
            }
        };
        let label = async {
            if !nd.acc.name.is_empty() || !labelled(&nd.acc.role_name) {
                return None;
            }
            let rels = self
                .acall::<_, Vec<(u32, Vec<(String, OwnedObjectPath)>)>>(
                    r,
                    A11Y_IFACE,
                    "GetRelationSet",
                    &(),
                )
                .await
                .ok()?;
            let target = rels
                .into_iter()
                .filter(|(kind, _)| *kind == RELATION_LABELLED_BY)
                .flat_map(|(_, targets)| targets)
                .map(to_ref)
                .find(|t| !t.is_null())?;
            let name: OwnedValue = self
                .acall(&target, PROPS_IFACE, "Get", &(A11Y_IFACE, "Name"))
                .await
                .ok()?;
            let name = owned_string(&name)?;
            // "Name:" names the field "Name".
            let name = name.trim().trim_end_matches(':').trim().to_string();
            (!name.is_empty()).then_some(name)
        };
        let ((children, t1), (text, t2), label) = futures_util::join!(children, text, label);
        (children, text, label, t1 || t2)
    }

    /// Read a batch of elements: first their properties, then (knowing
    /// which are text and which are huge containers) children and text.
    async fn fetch_batch(&self, refs: &[(ObjRef, bool)], text_end: i32) -> Vec<NodeData> {
        // Each element's children and text are asked for as soon as its
        // own properties are in, not after the whole batch's.
        futures_util::future::join_all(refs.iter().map(|(r, kids)| async move {
            let (mut nd, _) = self.fetch_props(r).await;
            let (children, text, label, timed_out) = self.fetch_rest(r, &nd, *kids, text_end).await;
            nd.children = children;
            nd.text = text;
            nd.label = label;
            nd.timed_out |= timed_out;
            nd
        }))
        .await
    }

    /// Walk the subtree under `root` breadth-first, `batch` elements at a time
    /// with all their queries pipelined, then return it in pre-order.
    ///
    /// Elements that time out are read again once, in smaller batches (a
    /// slow app answers a few queries in time, not hundreds); only elements
    /// that are gone are left out. When nothing answers at all (the app is
    /// frozen), or after [`WALK_DEADLINE`], the part read so far is
    /// returned.
    pub fn walk(
        &self,
        root: &ObjRef,
        max_nodes: usize,
        max_depth: usize,
        batch: usize,
        text_max: usize,
    ) -> Vec<Walked> {
        use futures_util::future::{Either, select};
        use std::collections::{HashSet, VecDeque};

        let deadline = Instant::now() + WALK_DEADLINE;
        let mut batch = batch.max(1);
        let text_end = i32::try_from(text_max.max(1)).unwrap_or(i32::MAX);
        let mut data: HashMap<ObjRef, NodeData> = HashMap::new();
        let mut cut_short = None;

        async_io::block_on(async {
            let mut queue: VecDeque<Pending> = VecDeque::new();
            let mut seen: HashSet<ObjRef> = HashSet::new();
            queue.push_back(Pending {
                r: root.clone(),
                depth: 0,
                retried: false,
            });
            seen.insert(root.clone());
            while !queue.is_empty() && data.len() < max_nodes {
                let n = queue.len().min(batch).min(max_nodes - data.len());
                let chunk: Vec<Pending> = queue.drain(..n).collect();
                let refs: Vec<(ObjRef, bool)> = chunk
                    .iter()
                    .map(|p| (p.r.clone(), p.depth < max_depth))
                    .collect();
                let fetch = Box::pin(self.fetch_batch(&refs, text_end));
                let timer = Box::pin(async_io::Timer::at(deadline));
                let nodes = match select(fetch, timer).await {
                    Either::Left((nodes, _)) => nodes,
                    Either::Right(_) => {
                        cut_short = Some("it took too long");
                        break;
                    }
                };
                // Not a slow app but a frozen one (or one whose modal dialog
                // holds its accessibility): asking again would only wait again.
                if nodes.iter().all(|n| n.fail == Some(Fail::Timeout)) {
                    cut_short = Some("the app is not responding");
                    break;
                }
                let mut again = Vec::new();
                for (p, nd) in chunk.into_iter().zip(nodes) {
                    if nd.timed_out && !p.retried {
                        again.push(Pending { retried: true, ..p });
                        continue;
                    }
                    if nd.fail.is_some() {
                        continue; // gone, or still no answer at all
                    }
                    if p.depth < max_depth {
                        for c in &nd.children {
                            if seen.insert(c.clone()) {
                                queue.push_back(Pending {
                                    r: c.clone(),
                                    depth: p.depth + 1,
                                    retried: false,
                                });
                            }
                        }
                    }
                    data.insert(p.r, nd);
                }
                if self.lost() {
                    cut_short = Some("the accessibility bus connection broke");
                    break;
                }
                if !again.is_empty() {
                    // The app can't keep up: ask it for less at a time.
                    batch = (batch / 4).max(1);
                    for p in again.into_iter().rev() {
                        queue.push_front(p);
                    }
                }
            }
        });
        if let Some(why) = cut_short {
            log::warn!(
                "read only part of the accessibility tree ({} elements): {why}",
                data.len()
            );
        }

        // Re-assemble in pre-order (document order).
        let mut out = Vec::with_capacity(data.len());
        let mut stack: Vec<(ObjRef, Option<usize>)> = vec![(root.clone(), None)];
        while let Some((r, parent)) = stack.pop() {
            let Some(nd) = data.remove(&r) else {
                continue; // not fetched (limit reached) or already emitted
            };
            let idx = out.len();
            for c in nd.children.iter().rev() {
                stack.push((c.clone(), Some(idx)));
            }
            out.push(Walked {
                r,
                data: nd,
                parent,
            });
        }
        out
    }

    /// Fetch several elements' properties concurrently (no children or text).
    pub fn fetch_many(&self, refs: &[ObjRef]) -> Vec<NodeData> {
        async_io::block_on(futures_util::future::join_all(
            refs.iter().map(|r| self.fetch_props(r)),
        ))
        .into_iter()
        .map(|(nd, _)| nd)
        .collect()
    }

    /// Several elements' children, concurrently.
    pub fn children_many(&self, refs: &[ObjRef]) -> Vec<CallResult<Vec<ObjRef>>> {
        async_io::block_on(futures_util::future::join_all(refs.iter().map(
            |r| async move {
                self.acall::<_, Vec<(String, OwnedObjectPath)>>(r, A11Y_IFACE, "GetChildren", &())
                    .await
                    .map(|k| k.into_iter().map(to_ref).filter(|c| !c.is_null()).collect())
            },
        )))
    }

    /// For each of several windows, its name if it is the active one (has
    /// the focus): only states are read, then the active ones' names.
    pub fn active_names(&self, refs: &[ObjRef]) -> Vec<Option<String>> {
        async_io::block_on(futures_util::future::join_all(refs.iter().map(
            |r| async move {
                let st = self
                    .acall::<_, Vec<u32>>(r, A11Y_IFACE, "GetState", &())
                    .await
                    .ok()?;
                if !States::from_pair(&st).has(state::ACTIVE) {
                    return None;
                }
                let name: OwnedValue = self
                    .acall(r, PROPS_IFACE, "Get", &(A11Y_IFACE, "Name"))
                    .await
                    .ok()?;
                owned_string(&name)
            },
        )))
    }

    /// Unix pids behind several accessibles' bus connections, concurrently.
    pub fn pids_of(&self, refs: &[ObjRef]) -> Vec<Option<u32>> {
        let bus = ObjRef {
            bus: "org.freedesktop.DBus".into(),
            path: "/org/freedesktop/DBus".into(),
        };
        async_io::block_on(futures_util::future::join_all(refs.iter().map(|r| {
            let bus = &bus;
            async move {
                self.acall::<_, u32>(
                    bus,
                    "org.freedesktop.DBus",
                    "GetConnectionUnixProcessID",
                    &(r.bus.as_str(),),
                )
                .await
                .ok()
            }
        })))
    }
}

fn to_ref((bus, path): (String, OwnedObjectPath)) -> ObjRef {
    ObjRef {
        bus,
        path: path.as_str().to_string(),
    }
}

fn owned_string(v: &OwnedValue) -> Option<String> {
    String::try_from(v.clone()).ok()
}

fn bus_err(e: impl std::fmt::Display) -> Error {
    Error::Platform(format!("AT-SPI: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn actions_without_a_name_take_their_description_or_none() {
        assert_eq!(action_name("jump".into(), String::new()), "jump");
        // Firefox puts a key binding where the name goes.
        assert_eq!(action_name(";;".into(), "Jump".into()), "Jump");
        assert_eq!(action_name(String::new(), "click".into()), "click");
        assert_eq!(action_name(";;".into(), String::new()), "");
        // A description that is a sentence names nothing.
        assert_eq!(
            action_name(String::new(), "Activates the link the cursor is on".into()),
            ""
        );
    }

    #[test]
    fn timeouts_and_gone_elements_are_told_apart() {
        let io = |k| zbus::Error::InputOutput(Arc::new(std::io::Error::new(k, "x")));
        assert_eq!(Fail::of(&io(std::io::ErrorKind::TimedOut)), Fail::Timeout);
        assert_eq!(
            Fail::of(&io(std::io::ErrorKind::BrokenPipe)),
            Fail::Disconnected
        );
        let fdo = |e| zbus::Error::FDO(Box::new(e));
        assert_eq!(
            Fail::of(&fdo(zbus::fdo::Error::UnknownObject("x".into()))),
            Fail::Gone
        );
        assert_eq!(
            Fail::of(&fdo(zbus::fdo::Error::ServiceUnknown("x".into()))),
            Fail::Gone
        );
        assert_eq!(
            Fail::of(&fdo(zbus::fdo::Error::NoReply("x".into()))),
            Fail::Timeout
        );
        assert_eq!(
            Fail::of(&fdo(zbus::fdo::Error::UnknownMethod("x".into()))),
            Fail::Other
        );
        assert_eq!(Fail::of(&zbus::Error::InvalidReply), Fail::Other);
        let e = CallError::new("Action.DoAction", &io(std::io::ErrorKind::TimedOut));
        assert!(e.to_string().contains("no answer within 3 s"), "{e}");
    }

    /// Needs the accessibility bus (run inside the live test's session:
    /// `cargo test -p computer-use --lib -- --ignored atspi`).
    #[test]
    #[ignore]
    fn an_action_held_by_the_app_times_out_and_a_gone_app_is_gone() {
        let a11y = AtspiConnection::connect(|| None).expect("the accessibility bus");
        // An app that takes the call and doesn't answer (a button whose
        // handler opened a modal dialog before the reply).
        let app = zbus::blocking::connection::Builder::address(
            AtspiConnection::address(|| None).unwrap().as_str(),
        )
        .unwrap()
        .build()
        .unwrap();
        let r = ObjRef {
            bus: app.unique_name().unwrap().to_string(),
            path: "/org/a11y/atspi/accessible/1".into(),
        };
        let t = Instant::now();
        let e = a11y.do_action(&r, 0).unwrap_err();
        assert_eq!(e.fail, Fail::Timeout, "{e}");
        assert!(t.elapsed() >= METHOD_TIMEOUT, "{:?}", t.elapsed());
        assert!(!a11y.lost());
        drop(app);
        let e = a11y.do_action(&r, 0).unwrap_err();
        assert_eq!(e.fail, Fail::Gone, "{e}");
        assert!(!a11y.lost());
    }

    #[test]
    fn the_bus_address_is_looked_for_in_order() {
        let fail = || -> Result<String> { Err(Error::Platform("no session bus".into())) };
        // The environment first: neither X nor the session bus is asked.
        assert_eq!(
            bus_address(
                Some("unix:path=/a".into()),
                || unreachable!(),
                || unreachable!()
            )
            .unwrap(),
            "unix:path=/a"
        );
        // Then the X root window's property (ssh -X, containers).
        assert_eq!(
            bus_address(None, || Some("unix:path=/x".into()), || unreachable!()).unwrap(),
            "unix:path=/x"
        );
        assert_eq!(
            bus_address(Some(" ".into()), || Some("unix:path=/x".into()), fail).unwrap(),
            "unix:path=/x"
        );
        // Then the bus launcher.
        assert_eq!(
            bus_address(None, || None, || Ok("unix:path=/s".into())).unwrap(),
            "unix:path=/s"
        );
        assert!(bus_address(None, || Some(String::new()), fail).is_err());
    }

    #[test]
    fn the_most_telling_failure_wins() {
        assert_eq!(
            Fail::worst(&[Fail::Gone, Fail::Timeout, Fail::Other]),
            Fail::Timeout
        );
        assert_eq!(Fail::worst(&[Fail::Gone, Fail::Other]), Fail::Gone);
        assert_eq!(Fail::worst(&[Fail::Other]), Fail::Other);
        assert_eq!(
            Fail::worst(&[Fail::Timeout, Fail::Disconnected]),
            Fail::Disconnected
        );
    }
}
