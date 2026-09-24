//! A thin blocking wrapper over the AT-SPI2 D-Bus API.
//!
//! AT-SPI exposes the desktop accessibility tree on a dedicated bus. The
//! address of that bus is fetched from `org.a11y.Bus` on the session bus;
//! every accessible is addressed by `(bus_name, object_path)`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};

use crate::error::{Error, Result};

const A11Y_IFACE: &str = "org.a11y.atspi.Accessible";
const COMPONENT_IFACE: &str = "org.a11y.atspi.Component";
const ACTION_IFACE: &str = "org.a11y.atspi.Action";
const TEXT_IFACE: &str = "org.a11y.atspi.Text";
const EDITABLE_IFACE: &str = "org.a11y.atspi.EditableText";
const VALUE_IFACE: &str = "org.a11y.atspi.Value";
const PROPS_IFACE: &str = "org.freedesktop.DBus.Properties";
const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";

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

pub struct AtspiConnection {
    conn: Connection,
}

impl AtspiConnection {
    /// Connect to the accessibility bus (starting from the session bus).
    pub fn connect() -> Result<Self> {
        let session = Connection::session().map_err(bus_err)?;
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
        let addr: String = reply.body().deserialize().map_err(bus_err)?;
        let conn = zbus::blocking::connection::Builder::address(addr.as_str())
            .map_err(bus_err)?
            .build()
            .map_err(|e| Error::Platform(format!("cannot connect to the a11y bus: {e}")))?;
        Ok(Self { conn })
    }

    pub fn root(&self) -> ObjRef {
        ObjRef {
            bus: "org.a11y.atspi.Registry".into(),
            path: ROOT_PATH.into(),
        }
    }

    fn call<B, R>(&self, r: &ObjRef, iface: &str, method: &str, body: &B) -> Result<R>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
    {
        count();
        let path = ObjectPath::try_from(r.path.as_str()).map_err(bus_err)?;
        let reply = self
            .conn
            .call_method(Some(r.bus.as_str()), &path, Some(iface), method, body)
            .map_err(|e| Error::Platform(format!("{iface}.{method}: {e}")))?;
        reply.body().deserialize().map_err(bus_err)
    }

    pub fn children(&self, r: &ObjRef) -> Result<Vec<ObjRef>> {
        // GetChildren -> a(so). Present on modern AT-SPI.
        match self.call::<_, Vec<(String, OwnedObjectPath)>>(r, A11Y_IFACE, "GetChildren", &()) {
            Ok(list) => Ok(list
                .into_iter()
                .map(to_ref)
                .filter(|c| !c.is_null())
                .collect()),
            Err(_) => self.children_by_index(r),
        }
    }

    fn children_by_index(&self, r: &ObjRef) -> Result<Vec<ObjRef>> {
        let count = self.child_count(r).unwrap_or(0).clamp(0, 5000);
        let mut out = Vec::new();
        for i in 0..count {
            if let Ok((s, p)) =
                self.call::<_, (String, OwnedObjectPath)>(r, A11Y_IFACE, "GetChildAtIndex", &(i,))
            {
                let c = to_ref((s, p));
                if !c.is_null() {
                    out.push(c);
                }
            }
        }
        Ok(out)
    }

    fn child_count(&self, r: &ObjRef) -> Option<i32> {
        self.get_prop::<i32>(r, A11Y_IFACE, "ChildCount").ok()
    }

    fn get_prop<T>(&self, r: &ObjRef, iface: &str, name: &str) -> Result<T>
    where
        T: TryFrom<OwnedValue>,
    {
        let v: OwnedValue = self.call(r, PROPS_IFACE, "Get", &(iface, name))?;
        T::try_from(v).map_err(|_| Error::Platform(format!("unexpected type for {iface}.{name}")))
    }

    /// Read the common accessible properties in as few calls as possible.
    pub fn describe(&self, r: &ObjRef) -> Result<Accessible> {
        let mut a = Accessible::default();
        if let Ok(props) =
            self.call::<_, HashMap<String, OwnedValue>>(r, PROPS_IFACE, "GetAll", &(A11Y_IFACE,))
        {
            a.name = props.get("Name").and_then(owned_string).unwrap_or_default();
            a.description = props
                .get("Description")
                .and_then(owned_string)
                .unwrap_or_default();
            a.child_count = props
                .get("ChildCount")
                .and_then(|v| i32::try_from(v.clone()).ok())
                .unwrap_or(0);
        }
        a.role_name = self
            .call::<_, String>(r, A11Y_IFACE, "GetRoleName", &())
            .unwrap_or_default();
        if let Ok(v) = self.call::<_, Vec<u32>>(r, A11Y_IFACE, "GetState", &()) {
            a.states = States::from_pair(&v);
        }
        a.interfaces = self
            .call::<_, Vec<String>>(r, A11Y_IFACE, "GetInterfaces", &())
            .unwrap_or_default();
        Ok(a)
    }

    /// (name, description, keybinding) for each action.
    pub fn actions(&self, r: &ObjRef) -> Vec<(String, String, String)> {
        self.call::<_, Vec<(String, String, String)>>(r, ACTION_IFACE, "GetActions", &())
            .unwrap_or_default()
    }

    pub fn do_action(&self, r: &ObjRef, index: i32) -> Result<bool> {
        self.call::<_, bool>(r, ACTION_IFACE, "DoAction", &(index,))
    }

    /// Index of the named action (case-insensitive), if the element has it.
    pub fn action_index(&self, r: &ObjRef, name: &str) -> Option<i32> {
        self.actions(r)
            .into_iter()
            .position(|(n, _, _)| n.eq_ignore_ascii_case(name))
            .map(|i| i as i32)
    }

    pub fn grab_focus(&self, r: &ObjRef) -> Result<bool> {
        self.call::<_, bool>(r, COMPONENT_IFACE, "GrabFocus", &())
    }

    pub fn set_text(&self, r: &ObjRef, text: &str) -> Result<bool> {
        self.call::<_, bool>(r, EDITABLE_IFACE, "SetTextContents", &(text,))
    }

    #[allow(dead_code)]
    pub fn insert_text(&self, r: &ObjRef, offset: i32, text: &str) -> Result<()> {
        let len = text.chars().count() as i32;
        self.call::<_, ()>(r, EDITABLE_IFACE, "InsertText", &(offset, text, len))
    }

    pub fn set_value(&self, r: &ObjRef, value: f64) -> Result<bool> {
        // CurrentValue is a read/write property of type double.
        let v = zbus::zvariant::Value::from(value);
        self.call::<_, ()>(r, PROPS_IFACE, "Set", &(VALUE_IFACE, "CurrentValue", v))
            .map(|_| true)
    }

    pub fn character_count(&self, r: &ObjRef) -> i32 {
        self.get_prop::<i32>(r, TEXT_IFACE, "CharacterCount")
            .unwrap_or(0)
    }

    pub fn get_text(&self, r: &ObjRef, start: i32, end: i32) -> Result<String> {
        self.call::<_, String>(r, TEXT_IFACE, "GetText", &(start, end))
    }

    pub fn set_selection(&self, r: &ObjRef, start: i32, end: i32) -> Result<bool> {
        // Selection index 0. Some toolkits require AddSelection first.
        if let Ok(b) = self.call::<_, bool>(r, TEXT_IFACE, "SetSelection", &(0i32, start, end)) {
            if b {
                return Ok(true);
            }
        }
        self.call::<_, bool>(r, TEXT_IFACE, "AddSelection", &(start, end))
    }

    pub fn set_caret(&self, r: &ObjRef, offset: i32) -> Result<bool> {
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
    pub children: Vec<ObjRef>,
}

/// An element returned by [`AtspiConnection::walk`], in pre-order.
pub struct Walked {
    pub r: ObjRef,
    pub data: NodeData,
    pub parent: Option<usize>,
}

async fn acall<B, R>(
    conn: &zbus::Connection,
    r: &ObjRef,
    iface: &str,
    method: &str,
    body: &B,
) -> Result<R>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
    R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
    count();
    let path = ObjectPath::try_from(r.path.as_str()).map_err(bus_err)?;
    let reply = conn
        .call_method(Some(r.bus.as_str()), &path, Some(iface), method, body)
        .await
        .map_err(|e| Error::Platform(format!("{iface}.{method}: {e}")))?;
    reply.body().deserialize().map_err(bus_err)
}

/// Fetch one element's properties, state, interfaces, extents, actions and
/// children — all seven queries in flight at once.
async fn fetch_node(conn: &zbus::Connection, r: &ObjRef) -> NodeData {
    let (props, role, state, ifaces, ext, acts, kids) = futures_util::join!(
        acall::<_, HashMap<String, OwnedValue>>(conn, r, PROPS_IFACE, "GetAll", &(A11Y_IFACE,)),
        acall::<_, String>(conn, r, A11Y_IFACE, "GetRoleName", &()),
        acall::<_, Vec<u32>>(conn, r, A11Y_IFACE, "GetState", &()),
        acall::<_, Vec<String>>(conn, r, A11Y_IFACE, "GetInterfaces", &()),
        acall::<_, (i32, i32, i32, i32)>(conn, r, COMPONENT_IFACE, "GetExtents", &(0u32,)),
        acall::<_, Vec<(String, String, String)>>(conn, r, ACTION_IFACE, "GetActions", &()),
        acall::<_, Vec<(String, OwnedObjectPath)>>(conn, r, A11Y_IFACE, "GetChildren", &()),
    );
    let mut acc = Accessible::default();
    if let Ok(props) = props {
        acc.name = props.get("Name").and_then(owned_string).unwrap_or_default();
        acc.description = props
            .get("Description")
            .and_then(owned_string)
            .unwrap_or_default();
        acc.child_count = props
            .get("ChildCount")
            .and_then(|v| i32::try_from(v.clone()).ok())
            .unwrap_or(0);
    }
    acc.role_name = role.unwrap_or_default();
    if let Ok(v) = state {
        acc.states = States::from_pair(&v);
    }
    acc.interfaces = ifaces.unwrap_or_default();
    NodeData {
        acc,
        extents: ext.ok(),
        actions: acts
            .map(|a| a.into_iter().map(|(n, _, _)| n).collect())
            .unwrap_or_default(),
        text: None,
        children: kids
            .map(|k| k.into_iter().map(to_ref).filter(|c| !c.is_null()).collect())
            .unwrap_or_default(),
    }
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

impl AtspiConnection {
    /// Walk the subtree under `root` breadth-first, `batch` elements at a time
    /// with all their queries pipelined, then return it in pre-order.
    pub fn walk(
        &self,
        root: &ObjRef,
        max_nodes: usize,
        max_depth: usize,
        batch: usize,
        text_max: usize,
    ) -> Vec<Walked> {
        use std::collections::{HashSet, VecDeque};

        let conn = self.conn.inner();
        let batch = batch.max(1);
        let text_end = i32::try_from(text_max.max(1)).unwrap_or(i32::MAX);
        let mut data: HashMap<ObjRef, NodeData> = HashMap::new();

        async_io::block_on(async {
            let mut queue: VecDeque<(ObjRef, usize)> = VecDeque::new();
            let mut seen: HashSet<ObjRef> = HashSet::new();
            queue.push_back((root.clone(), 0));
            seen.insert(root.clone());
            while !queue.is_empty() && data.len() < max_nodes {
                let n = queue.len().min(batch).min(max_nodes - data.len());
                let chunk: Vec<(ObjRef, usize)> = queue.drain(..n).collect();
                let mut nodes =
                    futures_util::future::join_all(chunk.iter().map(|(r, _)| fetch_node(conn, r)))
                        .await;

                // Second, smaller round: text content where it matters.
                let need: Vec<usize> = (0..nodes.len())
                    .filter(|&i| wants_text(&nodes[i].acc))
                    .collect();
                let text_args = (0i32, text_end);
                let texts = futures_util::future::join_all(need.iter().map(|&i| {
                    acall::<_, String>(conn, &chunk[i].0, TEXT_IFACE, "GetText", &text_args)
                }))
                .await;
                for (i, t) in need.into_iter().zip(texts) {
                    nodes[i].text = t.ok().filter(|s| !s.is_empty());
                }

                for ((r, depth), nd) in chunk.into_iter().zip(nodes) {
                    if depth < max_depth {
                        for c in &nd.children {
                            if seen.insert(c.clone()) {
                                queue.push_back((c.clone(), depth + 1));
                            }
                        }
                    }
                    data.insert(r, nd);
                }
            }
        });

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

    /// Fetch several elements concurrently (no text content).
    pub fn fetch_many(&self, refs: &[ObjRef]) -> Vec<NodeData> {
        let conn = self.conn.inner();
        async_io::block_on(futures_util::future::join_all(
            refs.iter().map(|r| fetch_node(conn, r)),
        ))
    }

    /// Unix pids behind several accessibles' bus connections, concurrently.
    pub fn pids_of(&self, refs: &[ObjRef]) -> Vec<Option<u32>> {
        let conn = self.conn.inner();
        async_io::block_on(futures_util::future::join_all(refs.iter().map(
            |r| async move {
                count();
                let reply = conn
                    .call_method(
                        Some("org.freedesktop.DBus"),
                        "/org/freedesktop/DBus",
                        Some("org.freedesktop.DBus"),
                        "GetConnectionUnixProcessID",
                        &(r.bus.as_str(),),
                    )
                    .await
                    .ok()?;
                reply.body().deserialize::<u32>().ok()
            },
        )))
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
