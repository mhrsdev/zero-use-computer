//! Turn a raw accessibility tree into what the model sees: a sparse, indexed
//! outline of the window, or a diff against the previous one.
//!
//! Pipeline: raw nodes → identity keys → visibility filter → drop
//! meaningless wrappers → cap size → assign element indices → render.

use std::collections::{HashMap, HashSet};

use crate::config::TreeConfig;
use crate::roles;
use crate::types::{ActionDesc, ElementHandle, NodeStates, RawNode, Rect};

/// An element that survived pruning.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Element index shown to the model.
    pub index: u32,
    /// Depth in the pruned tree (0 = root).
    pub depth: usize,
    /// Position of the nearest kept ancestor in the node list.
    pub parent: Option<usize>,
    /// Identity across snapshots (hash of the backend key or structural path).
    pub key: u64,
    /// Hash of the structural path (role + label + position among siblings),
    /// used to recognise the same screen even when backend identities change
    /// (e.g. a dialog that was closed and opened again).
    pub shape: u64,
    pub handle: ElementHandle,
    pub role: String,
    pub name: Option<String>,
    pub value: Option<String>,
    pub bounds: Option<Rect>,
    pub actions: Vec<ActionDesc>,
    pub states: NodeStates,
    /// Rendered description without index or indentation (diffed as-is).
    pub line: String,
}

impl Node {
    pub fn label(&self) -> String {
        match &self.name {
            Some(n) if !n.is_empty() => format!("{} \"{}\"", self.role, truncate(n, 60)),
            _ => self.role.clone(),
        }
    }

    pub fn has_action(&self, name: &str) -> Option<&ActionDesc> {
        self.actions
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name) || a.native.eq_ignore_ascii_case(name))
    }
}

#[derive(Debug, Default)]
pub struct Pruned {
    pub nodes: Vec<Node>,
    /// Elements dropped because of `max_nodes`.
    pub omitted: usize,
}

/// Stable 64-bit FNV-1a hash (identity keys, shapes and line fingerprints).
pub fn hash_str(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

const FNV_PRIME: u64 = 0x0100_0000_01b3;
const ROOT_SHAPE: u64 = 0x006d_6872_7364_6576;

/// Fold a value into a hash (FNV-1a over its bytes).
fn mix(mut h: u64, v: u64) -> u64 {
    for b in v.to_le_bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// Hash of one structural step: the role plus up to 40 characters of label.
fn step_hash(role: &str, label: Option<&str>) -> u64 {
    let mut h = ROOT_SHAPE;
    for b in role.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    if let Some(label) = label {
        h ^= u64::from(b':');
        h = h.wrapping_mul(FNV_PRIME);
        let end = label.char_indices().nth(40).map_or(label.len(), |(i, _)| i);
        for b in &label.as_bytes()[..end] {
            h ^= u64::from(*b);
            h = h.wrapping_mul(FNV_PRIME);
        }
    }
    h
}

/// Actions that don't make an element worth showing on their own.
fn is_incidental_action(name: &str) -> bool {
    matches!(name, "scroll_to_visible" | "show_menu" | "raise")
}

fn has_text(s: &Option<String>) -> bool {
    s.as_deref().is_some_and(|t| !t.trim().is_empty())
}

/// Collapse whitespace runs, escape quotes, and cap length.
pub fn clean_text(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max + 16));
    let mut prev_space = false;
    for c in s.trim().chars() {
        match c {
            // Invisible direction marks only reorder the display of mixed
            // left-to-right / right-to-left text; the model reads the text
            // in its stored order.
            c if crate::text::is_bidi_control(c) => {}
            '\n' => {
                out.push_str("\\n");
                prev_space = false;
            }
            '"' => {
                out.push_str("\\\"");
                prev_space = false;
            }
            c if c.is_whitespace() => {
                if !prev_space {
                    out.push(' ');
                }
                prev_space = true;
            }
            c if c.is_control() => {}
            c => {
                out.push(c);
                prev_space = false;
            }
        }
    }
    truncate(&out, max)
}

pub fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}… (+{} chars)", count - max)
}

fn render_line(n: &RawNode, cfg: &TreeConfig) -> String {
    let max_text = cfg.max_text_len;
    let mut line = n.role.clone();
    if let Some(name) = n.name.as_deref().filter(|t| !t.trim().is_empty()) {
        line.push_str(&format!(" \"{}\"", clean_text(name, max_text)));
    }
    if let Some(desc) = n.description.as_deref().filter(|t| !t.trim().is_empty())
        && n.name.as_deref() != Some(desc)
    {
        line.push_str(&format!(" desc=\"{}\"", clean_text(desc, max_text)));
    }
    let value = n.value.as_deref().filter(|v| !v.is_empty());
    if let Some(v) = value
        && n.name.as_deref() != Some(v)
    {
        line.push_str(&format!(" value=\"{}\"", clean_text(v, max_text)));
    }
    if value.is_none()
        && let Some(p) = n.placeholder.as_deref().filter(|t| !t.trim().is_empty())
    {
        line.push_str(&format!(" placeholder=\"{}\"", clean_text(p, max_text)));
    }

    let s = &n.states;
    let mut flags: Vec<&str> = Vec::new();
    if s.focused {
        flags.push("focused");
    }
    if !s.enabled {
        flags.push("disabled");
    }
    if s.selected {
        flags.push("selected");
    }
    match s.checked {
        Some(true) => flags.push("checked"),
        Some(false) => flags.push("unchecked"),
        None => {}
    }
    match s.expanded {
        Some(true) => flags.push("expanded"),
        Some(false) => flags.push("collapsed"),
        None => {}
    }
    let entry = cfg.compact && roles::is_text_entry(&n.role);
    if s.editable {
        // A text field is editable unless said otherwise.
        if !entry {
            flags.push("editable");
        }
    } else if s.value_settable {
        flags.push("settable");
    } else if entry {
        flags.push("read-only");
    }
    if cfg.show_states && !flags.is_empty() {
        line.push_str(&format!(" ({})", flags.join(", ")));
    }

    let interactive = roles::is_interactive(&n.role);
    let secondary: Vec<&str> = n
        .actions
        .iter()
        .map(|a| a.name.as_str())
        .filter(|a| *a != "press" && *a != "scroll_to_visible")
        .filter(|a| interactive || *a != "show_menu")
        .filter(|a| !(cfg.compact && roles::implied_action(&n.role, a, s.expanded.is_some())))
        .collect();
    if cfg.show_actions && !secondary.is_empty() {
        line.push_str(&format!(" actions=[{}]", secondary.join(", ")));
    }
    line
}

/// Prune a raw pre-order tree. `viewport` is the window's screen rect;
/// elements entirely outside it are dropped (menus and other extra roots
/// are not clipped).
pub fn prune(raw: &[RawNode], viewport: Option<Rect>, cfg: &TreeConfig) -> Pruned {
    let n = raw.len();
    if n == 0 {
        return Pruned::default();
    }

    // Identity keys: backend keys where available, else the structural
    // shape: a hash chain of role + label + position among same-looking
    // siblings from the root down (no path strings are built).
    let mut keys: Vec<u64> = Vec::with_capacity(n);
    let mut shapes: Vec<u64> = Vec::with_capacity(n);
    let mut sibling_counts: HashMap<(Option<usize>, u64), u64> = HashMap::with_capacity(n);
    let mut root_order = 0usize;
    let mut root_of: Vec<usize> = vec![0; n];
    for (i, node) in raw.iter().enumerate() {
        let parent = node.parent.filter(|p| *p < i);
        // Roots (the window, the menu bar) are keyed by role alone so a title
        // change doesn't re-key the whole tree.
        let label = node
            .name
            .as_deref()
            .filter(|s| !s.is_empty() && parent.is_some());
        let step = step_hash(&node.role, label);
        let nth = sibling_counts
            .entry((parent, step))
            .and_modify(|c| *c += 1)
            .or_insert(0);
        let base = parent.map_or(ROOT_SHAPE, |p| shapes[p]);
        let shape = mix(mix(base, step), *nth);
        shapes.push(shape);
        keys.push(node.key.as_deref().map_or(shape, hash_str));
        root_of[i] = match parent {
            Some(p) => root_of[p],
            None => {
                root_order += 1;
                root_order - 1
            }
        };
    }

    // Visibility: hidden or off-viewport elements drop with their subtree.
    let mut visible = vec![true; n];
    for (i, node) in raw.iter().enumerate() {
        let parent = node.parent.filter(|p| *p < i);
        if let Some(p) = parent
            && !visible[p]
        {
            visible[i] = false;
            continue;
        }
        if parent.is_none() {
            continue;
        }
        if node.states.hidden {
            visible[i] = false;
            continue;
        }
        if let (Some(b), Some(vp), 0) = (node.bounds, viewport, root_of[i])
            && !b.is_empty()
            && !b.intersects(&vp)
        {
            visible[i] = false;
        }
    }

    // Candidates: elements carrying information or affordances.
    let mut candidate = vec![false; n];
    for (i, node) in raw.iter().enumerate() {
        if !visible[i] {
            continue;
        }
        let is_root = node.parent.is_none();
        let empty_bounds = node.bounds.is_some_and(|b| b.is_empty());
        let actionable = node.actions.iter().any(|a| !is_incidental_action(&a.name));
        let texty = has_text(&node.name) || has_text(&node.value) || has_text(&node.description);
        let s = &node.states;
        candidate[i] = is_root
            || (!empty_bounds
                && (actionable
                    || texty
                    || roles::is_interactive(&node.role)
                    || roles::is_structural(&node.role)
                    || s.editable
                    || s.value_settable
                    || s.focused));
    }

    // Drop text children that just repeat their parent's label
    // (a button containing a "text" with the same string).
    for (i, node) in raw.iter().enumerate() {
        if !candidate[i] || node.role != "text" || !node.actions.is_empty() {
            continue;
        }
        // Only earlier nodes are parents (a loop in a bad tree ends).
        let mut p = node.parent.filter(|p| *p < i);
        while let Some(pi) = p {
            if candidate[pi] {
                if raw[pi].name.is_some() && raw[pi].name == node.name {
                    candidate[i] = false;
                }
                break;
            }
            p = raw[pi].parent.filter(|pp| *pp < pi);
        }
    }

    // Structural containers that end up with no kept descendants and no
    // label of their own are noise. Walk children before parents.
    let mut kept_desc = vec![0usize; n];
    for i in (0..n).rev() {
        let node = &raw[i];
        let is_root = node.parent.is_none();
        if candidate[i]
            && !is_root
            && kept_desc[i] == 0
            && roles::is_structural(&node.role)
            && !roles::is_interactive(&node.role)
            && !has_text(&node.name)
            && !has_text(&node.value)
            && !node.states.focused
            && !node.actions.iter().any(|a| !is_incidental_action(&a.name))
        {
            candidate[i] = false;
        }
        if let Some(p) = node.parent.filter(|p| *p < i) {
            kept_desc[p] += kept_desc[i] + usize::from(candidate[i]);
        }
    }

    // Emit in pre-order with depth relative to kept ancestors.
    let mut out = Pruned::default();
    let mut kept_pos: Vec<Option<usize>> = vec![None; n];
    let mut seen_keys: HashSet<u64> = HashSet::new();
    for i in 0..n {
        if !candidate[i] {
            continue;
        }
        if out.nodes.len() >= cfg.max_nodes {
            out.omitted += 1;
            continue;
        }
        let node = &raw[i];
        let mut parent = None;
        let mut p = node.parent.filter(|p| *p < i);
        while let Some(pi) = p {
            if let Some(pos) = kept_pos[pi] {
                parent = Some(pos);
                break;
            }
            p = raw[pi].parent.filter(|pp| *pp < pi);
        }
        let depth = parent.map(|pp: usize| out.nodes[pp].depth + 1).unwrap_or(0);

        let mut key = keys[i];
        let mut dup = 1u64;
        while !seen_keys.insert(key) {
            key = mix(keys[i], dup);
            dup += 1;
        }

        kept_pos[i] = Some(out.nodes.len());
        out.nodes.push(Node {
            index: 0,
            depth,
            parent,
            key,
            shape: shapes[i],
            handle: node.handle,
            role: node.role.clone(),
            name: node.name.clone().filter(|s| !s.trim().is_empty()),
            value: node.value.clone(),
            bounds: node.bounds,
            actions: node.actions.clone(),
            states: node.states.clone(),
            line: render_line(node, cfg),
        });
    }
    out
}

/// Hands out element indices. Kept per app so diffs can reuse indices.
///
/// Numbers are never reused within an app: an index always means the same
/// element, so a stale index fails loudly instead of hitting something else.
#[derive(Debug, Default, Clone)]
pub struct IndexAllocator {
    by_key: HashMap<u64, u32>,
    next: u32,
}

impl IndexAllocator {
    /// Fresh numbering in document order.
    pub fn assign_fresh(&mut self, nodes: &mut [Node]) {
        self.by_key.clear();
        self.next = 0;
        self.assign_stable(nodes);
    }

    /// Forget which elements hold which numbers (a different window), but
    /// keep counting so old numbers are not handed out again.
    pub fn clear_keys(&mut self) {
        self.by_key.clear();
    }

    /// Give `key` a specific number (restoring a remembered screen).
    pub fn pin(&mut self, key: u64, index: u32) {
        self.by_key.insert(key, index);
        self.next = self.next.max(index.saturating_add(1));
    }

    /// Keep indices of elements seen before; new elements get new numbers.
    pub fn assign_stable(&mut self, nodes: &mut [Node]) {
        let live: HashSet<u64> = nodes.iter().map(|n| n.key).collect();
        self.by_key.retain(|k, _| live.contains(k));
        let mut used: HashSet<u32> = HashSet::with_capacity(nodes.len());
        for node in nodes.iter_mut() {
            let index = match self.by_key.get(&node.key) {
                Some(i) if !used.contains(i) => *i,
                _ => {
                    let i = self.next;
                    self.next += 1;
                    self.by_key.insert(node.key, i);
                    i
                }
            };
            used.insert(index);
            node.index = index;
        }
    }
}

pub fn render_full(nodes: &[Node], indent: usize) -> String {
    render_nodes(nodes, indent, None, &HashMap::new(), false)
}

fn push_line(out: &mut String, n: &Node, indent: usize) {
    pad(out, n.depth * indent);
    out.push_str(&format!("{} {}\n", n.index, n.line));
}

fn pad(out: &mut String, spaces: usize) {
    for _ in 0..spaces {
        out.push(' ');
    }
}

/// Where each element's subtree ends (exclusive) in a pre-order list.
fn subtree_ends(nodes: &[Node]) -> Vec<usize> {
    let mut end = vec![nodes.len(); nodes.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        while let Some(&t) = stack.last() {
            if nodes[t].depth >= n.depth {
                end[t] = i;
                stack.pop();
            } else {
                break;
            }
        }
        stack.push(i);
    }
    end
}

/// Render elements one a line, or, `compact`, look-alike siblings as
/// records. Elements marked `hidden` are left out with their subtrees; a
/// folded run says how many there are where it starts (`folded_at`).
fn render_nodes(
    nodes: &[Node],
    indent: usize,
    hidden: Option<&[bool]>,
    folded_at: &HashMap<usize, usize>,
    compact: bool,
) -> String {
    let none = vec![false; nodes.len()];
    let hidden = hidden.unwrap_or(&none);
    let end = subtree_ends(nodes);
    let runs = if compact {
        plan_runs(nodes, hidden, &end)
    } else {
        HashMap::new()
    };
    let mut out = String::new();
    let mut i = 0;
    while i < nodes.len() {
        let n = &nodes[i];
        if hidden[i] {
            if let Some(count) = folded_at.get(&i) {
                pad(&mut out, n.depth * indent);
                out.push_str(&format!(
                    "[… {count} more \"{}\" folded; find_element finds them]\n",
                    n.role
                ));
            }
            // Skip the folded element and everything inside it.
            i = end[i];
            continue;
        }
        if let Some(run) = runs.get(&i) {
            for line in &run.lines {
                pad(&mut out, n.depth * indent);
                out.push_str(line);
                out.push('\n');
            }
            i = run.end;
            continue;
        }
        push_line(&mut out, n, indent);
        i += 1;
    }
    out
}

/// Siblings drawn together: a header and their records.
struct Run {
    /// Lines, without indentation (they sit at the siblings' depth).
    lines: Vec<String>,
    /// Position after the run's last element.
    end: usize,
}

/// Fewest look-alike siblings drawn as records.
const MIN_RUN: usize = 3;
/// Most elements in one record.
const MAX_RECORD: usize = 6;
/// Records of single elements on one line.
const PER_LINE: usize = 6;

/// An element's line without its role (records name the roles once), and
/// without `common`, a suffix all its look-alikes share.
fn bare(n: &Node, common: Option<&str>) -> String {
    let mut rest = n.line.strip_prefix(n.role.as_str()).unwrap_or(&n.line);
    if let Some(c) = common {
        rest = rest.strip_suffix(c).unwrap_or(rest);
    }
    format!("{}{}", n.index, rest)
}

/// The ` actions=[…]` ending of an element's line, if any.
fn actions_suffix(line: &str) -> Option<&str> {
    let at = line.rfind(" actions=[")?;
    line.ends_with(']').then(|| &line[at..])
}

/// The suffix every element at the same place of each record shares.
fn shared_suffix<'a>(nodes: &'a [Node], members: &[usize]) -> Option<&'a str> {
    let first = actions_suffix(&nodes[*members.first()?].line)?;
    members
        .iter()
        .all(|&m| actions_suffix(&nodes[m].line) == Some(first))
        .then_some(first)
}

/// The runs of look-alike siblings worth drawing as records, by the
/// position of their first element: a table's cells as rows, consecutive
/// siblings whose subtrees have the same roles in the same shape (list
/// items with their text, toolbar buttons) as records.
fn plan_runs(nodes: &[Node], hidden: &[bool], end: &[usize]) -> HashMap<usize, Run> {
    let mut kids: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        kids.entry(n.parent).or_default().push(i);
    }
    // A subtree's shape: the roles in it, by depth below its root.
    let shape = |k: usize| -> Option<Vec<(usize, &str)>> {
        if end[k] - k > MAX_RECORD || (k..end[k]).any(|p| hidden[p]) {
            return None;
        }
        Some(
            (k..end[k])
                .map(|p| (nodes[p].depth - nodes[k].depth, nodes[p].role.as_str()))
                .collect(),
        )
    };
    let mut runs = HashMap::new();
    for list in kids.values() {
        // Column headers name the columns of the cells beside them.
        let headers: Vec<usize> = list
            .iter()
            .copied()
            .filter(|&k| roles::is_column_header(&nodes[k].role) && !hidden[k])
            .collect();
        let mut j = 0;
        while j < list.len() {
            let k = list[j];
            // A table's cells, a row a line.
            if roles::is_cell(&nodes[k].role) && end[k] == k + 1 && !hidden[k] {
                let mut cells = vec![k];
                while j + cells.len() < list.len() {
                    let c = list[j + cells.len()];
                    if roles::is_cell(&nodes[c].role) && end[c] == c + 1 && !hidden[c] {
                        cells.push(c);
                    } else {
                        break;
                    }
                }
                let cols = if headers.len() >= 2 {
                    headers.len()
                } else {
                    // No headers: the cells in the first one's row.
                    let y0 = nodes[k].bounds.map(|b| b.y);
                    cells
                        .iter()
                        .take_while(|&&c| {
                            matches!((nodes[c].bounds.map(|b| b.y), y0), (Some(a), Some(b)) if (a - b).abs() < 2.0)
                        })
                        .count()
                };
                if cols >= 2 && cells.len() >= 2 * cols {
                    let common = shared_suffix(nodes, &cells);
                    // On the header's one line, its separators not in them.
                    let names: Vec<String> = headers
                        .iter()
                        .filter_map(|&h| {
                            nodes[h]
                                .name
                                .as_deref()
                                .map(|t| clean_text(t, 24).replace('|', "¦").replace(';', ","))
                        })
                        .collect();
                    let mut head = String::from("cells, a row a line");
                    if names.len() == cols {
                        head.push_str(&format!(" ({})", names.join(" | ")));
                    }
                    if let Some(c) = common {
                        head.push_str(&format!("; each{c}"));
                    }
                    head.push(':');
                    let mut lines = vec![head];
                    for row in cells.chunks(cols) {
                        lines.push(
                            row.iter()
                                .map(|&c| bare(&nodes[c], common))
                                .collect::<Vec<_>>()
                                .join(" | "),
                        );
                    }
                    let last = *cells.last().expect("cells");
                    runs.insert(
                        k,
                        Run {
                            lines,
                            end: end[last],
                        },
                    );
                    j += cells.len();
                    continue;
                }
            }
            // Look-alike siblings as records.
            let Some(first) = shape(k) else {
                j += 1;
                continue;
            };
            let mut members = vec![k];
            while j + members.len() < list.len()
                && shape(list[j + members.len()]).as_ref() == Some(&first)
            {
                members.push(list[j + members.len()]);
            }
            if members.len() < MIN_RUN {
                j += members.len();
                continue;
            }
            let slots = first.len();
            // What every element at the same place of each record shares,
            // for a role found once in a record (the header names it by role).
            let common: Vec<Option<&str>> = (0..slots)
                .map(|s| {
                    let once = first.iter().filter(|(_, r)| *r == first[s].1).count() == 1;
                    once.then(|| {
                        shared_suffix(nodes, &members.iter().map(|&m| m + s).collect::<Vec<_>>())
                    })
                    .flatten()
                })
                .collect();
            let roles: Vec<&str> = first.iter().map(|(_, r)| *r).collect();
            let mut head = format!("{} × {}", members.len(), roles.join(" › "));
            for (s, c) in common.iter().enumerate() {
                if let Some(c) = c {
                    head.push_str(&format!("; each {}{c}", roles[s]));
                }
            }
            head.push(':');
            let mut lines = vec![head];
            let records: Vec<String> = members
                .iter()
                .map(|&m| {
                    (0..slots)
                        .map(|s| bare(&nodes[m + s], common[s]))
                        .collect::<Vec<_>>()
                        .join(" › ")
                })
                .collect();
            if slots == 1 {
                for chunk in records.chunks(PER_LINE) {
                    lines.push(chunk.join(" · "));
                }
            } else {
                lines.extend(records);
            }
            let last = *members.last().expect("members");
            runs.insert(
                k,
                Run {
                    lines,
                    end: end[last],
                },
            );
            j += members.len();
        }
    }
    runs
}

/// Split on `sep` outside quoted text.
fn split_outside_quotes<'a>(text: &'a str, sep: &str) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if escaped {
            escaped = false;
        } else if c == b'\\' {
            escaped = true;
        } else if c == b'"' {
            quoted = !quoted;
        } else if !quoted && bytes[i..].starts_with(sep.as_bytes()) {
            parts.push(&text[start..i]);
            i += sep.len();
            start = i;
            continue;
        }
        i += 1;
    }
    parts.push(&text[start..]);
    parts
}

/// What a records header says.
struct Template {
    /// The roles of a record's elements.
    roles: Vec<String>,
    /// What each role's elements share (a suffix of their lines).
    common: HashMap<String, String>,
    /// A table's cells, a row a line.
    cells: bool,
}

/// A records header, or None if `line` isn't one.
fn records_header(line: &str) -> Option<Template> {
    let body = line.strip_suffix(':')?;
    let (lead, each) = match body.split_once("; each") {
        Some((l, e)) => (l, Some(e)),
        None => (body, None),
    };
    let (roles, cells) = if lead.starts_with("cells, a row a line") {
        (vec!["cell".to_string()], true)
    } else {
        let (count, roles) = lead.split_once(" × ")?;
        count.parse::<usize>().ok()?;
        (roles.split(" › ").map(str::to_string).collect(), false)
    };
    let mut common = HashMap::new();
    for part in each.into_iter().flat_map(|e| e.split("; each")) {
        let at = part.find(" actions=[")?;
        let role = part[..at].trim();
        let role = if role.is_empty() {
            roles[0].as_str()
        } else {
            role
        };
        common.insert(role.to_string(), part[at..].to_string());
    }
    Some(Template {
        roles,
        common,
        cells,
    })
}

/// Whether a line is a record (`12 "Name" · 13 …`), not an element's line
/// (`12 button "Name"`): after the index comes no role.
fn is_record_line(line: &str) -> bool {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return false;
    }
    let rest = &line[digits..];
    rest.is_empty()
        || [
            " \"",
            " (",
            " desc=",
            " value=",
            " placeholder=",
            " actions=",
            " ·",
            " |",
            " ›",
        ]
        .iter()
        .any(|p| rest.starts_with(p))
}

/// A tree rendered with records back to one element a line with its role,
/// for tools and tests that read trees line by line.
pub fn expand(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    let mut template: Option<(usize, Template)> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if let Some(h) = records_header(trimmed) {
            template = Some((indent, h));
            continue;
        }
        if let Some((at, t)) = &template
            && *at == indent
            && is_record_line(trimmed)
        {
            let (roles, common) = (&t.roles, &t.common);
            let records = if t.cells {
                split_outside_quotes(trimmed, " | ")
            } else {
                split_outside_quotes(trimmed, " · ")
            };
            for record in records {
                for (k, member) in split_outside_quotes(record, " › ").into_iter().enumerate() {
                    let role = &roles[k.min(roles.len() - 1)];
                    let digits = member.chars().take_while(char::is_ascii_digit).count();
                    let suffix = common.get(role).map(String::as_str).unwrap_or("");
                    out.push_str(&line[..indent]);
                    out.push_str(&format!(
                        "{} {role}{}{suffix}\n",
                        &member[..digits],
                        &member[digits..]
                    ));
                }
            }
            continue;
        }
        template = None;
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The index of the first element whose line (records expanded) contains
/// `needle`.
pub fn index_of(text: &str, needle: &str) -> Option<u32> {
    expand(text)
        .lines()
        .find(|l| l.contains(needle))
        .and_then(|l| {
            let l = l.trim_start();
            let l = l
                .strip_prefix("+ ")
                .or_else(|| l.strip_prefix("~ "))
                .unwrap_or(l);
            l.split_whitespace().next()?.parse().ok()
        })
}

/// Roles of the items that make long, repetitive lists (rows of a table,
/// files in a folder, messages in a mailbox): folded first.
const LIST_ROLES: &[&str] = &[
    "list item",
    "row",
    "tree item",
    "data item",
    "cell",
    "table cell",
    "menu item",
];

/// Items always kept at the end of a folded list.
const FOLD_TAIL: usize = 2;
/// A list is folded only if that hides at least this many items.
const FOLD_MIN_HIDDEN: usize = 6;

/// How a tree over its token budget is shortened.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    /// Estimated tokens (0 = no limit).
    pub tokens: usize,
    pub level: crate::config::Summarize,
    /// Items kept at the start of a folded list.
    pub keep: usize,
    /// Look-alike siblings as records ([tree] compact).
    pub compact: bool,
}

impl Budget {
    pub fn from_config(cfg: &TreeConfig) -> Self {
        Self {
            tokens: cfg.max_tokens,
            level: cfg.summarize,
            keep: cfg.fold_keep,
            compact: cfg.compact,
        }
    }

    /// Whether anything is ever shortened.
    pub fn active(&self) -> bool {
        self.tokens > 0 && self.level != crate::config::Summarize::Off
    }
}

/// The whole tree within its budget. When it doesn't fit, long runs of
/// look-alike siblings are folded to their first and last few (list items
/// first; with `Normal`, then any role), with a line saying how many are
/// hidden; with `Normal`, what still doesn't fit is cut. Folded and cut
/// elements keep their indices: `find_element` finds them.
pub fn render_full_within(nodes: &[Node], indent: usize, budget: Budget) -> String {
    use crate::config::Summarize;
    let full = render_nodes(nodes, indent, None, &HashMap::new(), budget.compact);
    if !budget.active() || crate::text::estimate_tokens(&full) <= budget.tokens {
        return full;
    }
    let lists = render_folded(nodes, indent, budget.keep, budget.compact, |role| {
        LIST_ROLES.contains(&role)
    });
    if budget.level == Summarize::Light || crate::text::estimate_tokens(&lists) <= budget.tokens {
        return lists;
    }
    let any = render_folded(nodes, indent, budget.keep, budget.compact, |_| true);
    if crate::text::estimate_tokens(&any) <= budget.tokens {
        return any;
    }
    cut_to_budget(&any, budget.tokens)
}

/// Render with long sibling runs of the roles `fold` accepts folded,
/// keeping the first `keep` and the last `FOLD_TAIL` of each.
fn render_folded(
    nodes: &[Node],
    indent: usize,
    keep: usize,
    compact: bool,
    fold: impl Fn(&str) -> bool,
) -> String {
    let mut groups: HashMap<(Option<usize>, &str), Vec<usize>> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        if fold(&n.role) {
            groups
                .entry((n.parent, n.role.as_str()))
                .or_default()
                .push(i);
        }
    }
    // What the user is working on is never folded away: the focused or
    // selected element, and every element that contains one.
    let mut current = vec![false; nodes.len()];
    for (i, n) in nodes.iter().enumerate().rev() {
        current[i] |= n.states.focused || n.states.selected;
        if current[i]
            && let Some(p) = n.parent
        {
            current[p] = true;
        }
    }
    // First position of each folded run -> how many are folded there.
    let mut folded_at: HashMap<usize, usize> = HashMap::new();
    let mut hidden = vec![false; nodes.len()];
    // A table's cells fold by whole rows.
    let mut cols: HashMap<Option<usize>, usize> = HashMap::new();
    for n in nodes.iter().filter(|n| roles::is_column_header(&n.role)) {
        *cols.entry(n.parent).or_default() += 1;
    }
    for ((parent, role), members) in &groups {
        let per = if roles::is_cell(role) {
            cols.get(parent).copied().unwrap_or(1).max(1)
        } else {
            1
        };
        let (head, tail) = (keep * per, FOLD_TAIL * per);
        if members.len() < head + tail + FOLD_MIN_HIDDEN * per {
            continue;
        }
        let mut run: Option<usize> = None;
        for &i in &members[head..members.len() - tail] {
            if current[i] {
                run = None;
                continue;
            }
            hidden[i] = true;
            match run {
                Some(start) => *folded_at.entry(start).or_default() += 1,
                None => {
                    folded_at.insert(i, 1);
                    run = Some(i);
                }
            }
        }
    }
    render_nodes(nodes, indent, Some(&hidden), &folded_at, compact)
}

/// Cut rendered lines at about `budget` tokens, saying how many are left.
pub fn cut_to_budget(text: &str, budget: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::new();
    let mut used = 0;
    for (i, line) in lines.iter().enumerate() {
        let cost = crate::text::estimate_tokens(line) + 1;
        if used + cost > budget {
            out.push_str(&format!(
                "[… {} more lines not shown (token budget); find_element searches all of them]\n",
                lines.len() - i
            ));
            return out;
        }
        used += cost;
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[derive(Debug, Default, PartialEq)]
pub struct Diff {
    /// Positions in the new node list.
    pub added: Vec<usize>,
    /// (position in new list, old line).
    pub changed: Vec<(usize, String)>,
    /// (old index, old line).
    pub removed: Vec<(u32, String)>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }

    pub fn len(&self) -> usize {
        self.added.len() + self.changed.len() + self.removed.len()
    }
}

pub fn diff(old: &[Node], new: &[Node]) -> Diff {
    let old_by_key: HashMap<u64, &Node> = old.iter().map(|n| (n.key, n)).collect();
    let new_keys: HashSet<u64> = new.iter().map(|n| n.key).collect();
    let mut d = Diff::default();
    for (pos, n) in new.iter().enumerate() {
        match old_by_key.get(&n.key) {
            None => d.added.push(pos),
            Some(o) if o.line != n.line => d.changed.push((pos, o.line.clone())),
            Some(_) => {}
        }
    }
    for o in old {
        if !new_keys.contains(&o.key) {
            d.removed.push((o.index, o.line.clone()));
        }
    }
    d
}

/// Number of elements added, changed or removed between two snapshots,
/// without building the diff.
/// Whether `changes` to a tree of `len` elements make a big change (a new
/// screen, the whole tree sent): `ratio` of them, and more than one line
/// replaced, so a small window (a painted app's few lines read off the
/// screen) isn't new each time one of its lines changes.
pub fn big_change(changes: usize, len: usize, ratio: f64) -> bool {
    changes as f64 >= (ratio * len.max(1) as f64).max(3.0)
}

pub fn change_count(old: &[Node], new: &[Node]) -> usize {
    let old_by_key: HashMap<u64, &str> = old.iter().map(|n| (n.key, n.line.as_str())).collect();
    let mut matched = 0usize;
    let mut changes = 0usize;
    for n in new {
        match old_by_key.get(&n.key) {
            None => changes += 1,
            Some(line) => {
                matched += 1;
                if *line != n.line {
                    changes += 1;
                }
            }
        }
    }
    changes + old.len().saturating_sub(matched)
}

/// Intro line of a diff against the previous get_app_state.
pub const DIFF_INTRO: &str = "Changes since the previous get_app_state (+ added, ~ changed, - removed; a changed line ends with what it was, … standing for the start it kept). Unchanged elements keep their indices.";

pub fn render_diff(d: &Diff, new: &[Node]) -> String {
    if d.is_empty() {
        return "No changes to the accessibility tree since the previous get_app_state.\n".into();
    }
    render_diff_with(d, new, DIFF_INTRO, false)
}

/// More removed elements than this are given as ranges of indices
/// (`compact`): the model has seen their lines.
const REMOVED_LISTED: usize = 5;

/// Render a diff under a custom intro line. `compact`: elements added
/// together under one parent are listed under it once, and many removed
/// elements are given as ranges of their indices.
pub fn render_diff_with(d: &Diff, new: &[Node], intro: &str, compact: bool) -> String {
    let mut out = String::with_capacity(intro.len() + 64 * d.len());
    out.push_str(intro);
    out.push('\n');
    let added: HashSet<usize> = d.added.iter().copied().collect();
    let mut i = 0;
    while i < d.added.len() {
        let n = &new[d.added[i]];
        // An added element inside another added one needs no context.
        let parent = n.parent.filter(|p| !added.contains(p));
        let together = if compact {
            d.added[i..]
                .iter()
                .take_while(|&&pos| {
                    new[pos].parent == n.parent
                        || new[pos].parent.is_some_and(|p| added.contains(&p))
                })
                .count()
        } else {
            1
        };
        match parent {
            Some(p) if together >= 2 => {
                out.push_str(&format!("in {} {}:\n", new[p].index, new[p].label()));
                for &pos in &d.added[i..i + together] {
                    out.push_str(&format!("+ {} {}\n", new[pos].index, new[pos].line));
                }
                i += together;
            }
            Some(p) => {
                out.push_str(&format!(
                    "+ {} {}  (in {} {})\n",
                    n.index,
                    n.line,
                    new[p].index,
                    new[p].label()
                ));
                i += 1;
            }
            None => {
                out.push_str(&format!("+ {} {}\n", n.index, n.line));
                i += 1;
            }
        }
    }
    for (pos, old_line) in &d.changed {
        let n = &new[*pos];
        out.push_str(&format!(
            "~ {} {}  (was: {})\n",
            n.index,
            n.line,
            was(old_line, &n.line)
        ));
    }
    if compact && d.removed.len() > REMOVED_LISTED {
        let mut idx: Vec<u32> = d.removed.iter().map(|(i, _)| *i).collect();
        out.push_str(&format!("- {} removed: {}\n", idx.len(), ranges(&mut idx)));
    } else {
        for (idx, old_line) in &d.removed {
            out.push_str(&format!("- {idx} {old_line}\n"));
        }
    }
    out
}

/// Indices as ranges: `12–35, 40`.
pub fn ranges(idx: &mut [u32]) -> String {
    idx.sort_unstable();
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < idx.len() {
        let start = idx[i];
        let mut j = i;
        while j + 1 < idx.len() && idx[j + 1] == idx[j] + 1 {
            j += 1;
        }
        parts.push(if j > i {
            format!("{start}–{}", idx[j])
        } else {
            start.to_string()
        });
        i = j + 1;
    }
    parts.join(", ")
}

/// What a changed line was, without the start it shares with what it is
/// now (`…` stands for that): `button "Bold" (checked)` that was
/// `button "Bold" (unchecked)` shows `…(unchecked)`.
fn was(old: &str, new: &str) -> String {
    let same = old
        .char_indices()
        .zip(new.chars())
        .find(|((_, a), b)| a != b)
        .map_or(old.len().min(new.len()), |((i, _), _)| i);
    // Cut after a space, quote, bracket or `=`, never inside a word.
    let cut = old[..same]
        .rfind([' ', '"', '(', '[', '='])
        .map_or(0, |i| i + 1);
    if cut < 8 || cut >= old.len() {
        return old.to_string();
    }
    format!("…{}", &old[cut..])
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::types::NodeStates;

    #[test]
    fn one_line_replaced_in_a_small_window_is_not_a_big_change() {
        // A painted app's six lines: one replaced (removed + added).
        assert!(!big_change(2, 6, 0.33));
        assert!(big_change(3, 6, 0.33));
        // A large tree: the ratio as before.
        assert!(!big_change(32, 100, 0.33));
        assert!(big_change(33, 100, 0.33));
    }

    #[test]
    fn a_changed_line_shows_only_what_was_different() {
        assert_eq!(
            was("button \"Bold\" (unchecked)", "button \"Bold\" (checked)"),
            "…unchecked)"
        );
        assert_eq!(
            was(
                "text field \"To\" (editable)",
                "text field \"To\" value=\"bob\" (focused, editable)"
            ),
            "…(editable)"
        );
        assert_eq!(
            was(
                "text area value=\"hello world\"",
                "text area value=\"hello there\""
            ),
            "…world\""
        );
        // Little in common: all of it.
        assert_eq!(was("link \"A\"", "button \"B\""), "link \"A\"");
        // The old line is the start of the new one: all of it.
        assert_eq!(
            was("button \"Bold\"", "button \"Bold\" (checked)"),
            "button \"Bold\""
        );
    }

    fn node(parent: Option<usize>, role: &str, name: &str) -> RawNode {
        RawNode {
            handle: 0,
            parent,
            key: None,
            role: role.into(),
            native_role: role.into(),
            name: (!name.is_empty()).then(|| name.to_string()),
            bounds: Some(Rect::new(0.0, 0.0, 10.0, 10.0)),
            states: NodeStates {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn cfg() -> TreeConfig {
        TreeConfig::default()
    }

    fn sample() -> Vec<RawNode> {
        let mut v = vec![
            node(None, "window", "Doc"),       // 0
            node(Some(0), "group", ""),        // 1 wrapper, dropped
            node(Some(1), "button", "Save"),   // 2
            node(Some(2), "text", "Save"),     // 3 duplicate label, dropped
            node(Some(1), "text field", ""),   // 4
            node(Some(0), "list", ""),         // 5 empty list, dropped
            node(Some(0), "button", "Hidden"), // 6 hidden
        ];
        v[4].states.editable = true;
        v[4].value = Some("hello".into());
        v[6].states.hidden = true;
        v[2].actions = vec![ActionDesc::new("press", "AXPress")];
        v
    }

    #[test]
    fn prunes_wrappers_duplicates_and_hidden() {
        let mut p = prune(&sample(), None, &cfg());
        IndexAllocator::default().assign_fresh(&mut p.nodes);
        let text = render_full(&p.nodes, 2);
        // A text field is editable unless said otherwise (compact).
        assert_eq!(
            text,
            "0 window \"Doc\"\n  1 button \"Save\"\n  2 text field value=\"hello\"\n"
        );
        let wordy = TreeConfig {
            compact: false,
            ..cfg()
        };
        let mut p = prune(&sample(), None, &wordy);
        IndexAllocator::default().assign_fresh(&mut p.nodes);
        assert!(render_full(&p.nodes, 2).contains("value=\"hello\" (editable)"));
        let mut raw = sample();
        raw[4].states.editable = false;
        let mut p = prune(&raw, None, &cfg());
        IndexAllocator::default().assign_fresh(&mut p.nodes);
        assert!(render_full(&p.nodes, 2).contains("(read-only)"));
    }

    fn compact_budget() -> Budget {
        Budget {
            tokens: 0,
            level: crate::config::Summarize::Normal,
            keep: 5,
            compact: true,
        }
    }

    #[test]
    fn look_alike_siblings_are_records_and_cells_rows() {
        let mut raw = vec![
            node(None, "window", "Shop"),
            node(Some(0), "toolbar", "Tools"),
        ];
        for name in [
            "Select", "Pen", "Pencil", "Brush", "Eraser", "Line", "Arrow",
        ] {
            raw.push(node(Some(1), "button", name));
        }
        let list = raw.len();
        raw.push(node(Some(0), "list", "Sections"));
        for name in ["General", "Profile", "Privacy"] {
            let item = raw.len();
            raw.push(node(Some(list), "list item", ""));
            raw.push(node(Some(item), "text", name));
        }
        raw[list + 1].states.selected = true;
        let table = raw.len();
        raw.push(node(Some(0), "table", ""));
        for h in ["SKU", "Price"] {
            raw.push(node(Some(table), "table column header", h));
        }
        for r in 0..3 {
            for (c, v) in [format!("K-{r}"), format!("{r}.00")].iter().enumerate() {
                let mut cell = node(Some(table), "cell", v);
                cell.bounds = Some(Rect::new(
                    c as f64 * 50.0,
                    20.0 + r as f64 * 20.0,
                    50.0,
                    20.0,
                ));
                cell.actions = vec![
                    ActionDesc::new("press", "activate"),
                    ActionDesc::new("edit", "edit"),
                ];
                raw.push(cell);
            }
        }
        let mut p = prune(&raw, None, &cfg());
        IndexAllocator::default().assign_fresh(&mut p.nodes);
        let text = render_full_within(&p.nodes, 1, compact_budget());
        // Buttons: the role once, six to a line.
        assert!(text.contains(" 7 × button:\n"), "{text}");
        assert!(text.contains("2 \"Select\" · 3 \"Pen\""), "{text}");
        // List items with their text: one record a line.
        assert!(text.contains("3 × list item › text:"), "{text}");
        assert!(text.contains("(selected) › "), "{text}");
        // Cells: a row a line under the column names, the shared actions once.
        assert!(
            text.contains("cells, a row a line (SKU | Price); each actions=[edit]:"),
            "{text}"
        );
        assert!(text.contains("\"K-1\" | "), "{text}");
        assert!(!text.contains("cell \"K-1\""), "{text}");
        // Every index is still there, and the elements read back.
        for n in &p.nodes {
            assert!(
                index_of(&text, &format!("\"{}\"", n.name.as_deref().unwrap_or("§"))).is_some()
                    || n.name.is_none(),
                "{} missing: {text}",
                n.index
            );
        }
        let expanded = expand(&text);
        assert!(expanded.contains("button \"Pen\""), "{expanded}");
        assert!(expanded.contains("cell \"K-1\""), "{expanded}");
        assert!(expanded.contains("text \"Profile\""), "{expanded}");
        // Shorter, losing nothing.
        let plain = render_full(&p.nodes, 1);
        assert!(
            text.len() < plain.len(),
            "{} vs {}",
            text.len(),
            plain.len()
        );
        // Off: one element a line, as before.
        let off = render_full_within(
            &p.nodes,
            1,
            Budget {
                compact: false,
                ..compact_budget()
            },
        );
        assert_eq!(off, plain);
    }

    #[test]
    fn a_role_twice_in_a_record_keeps_its_own_actions() {
        let mut raw = vec![node(None, "window", "Mail"), node(Some(0), "list", "Inbox")];
        for name in ["One", "Two", "Three"] {
            let item = raw.len();
            raw.push(node(Some(1), "list item", ""));
            raw.push(node(Some(item), "text", name));
            let mut flag = node(Some(item), "text", &format!("{name} flag"));
            flag.actions = vec![ActionDesc::new("edit", "edit")];
            raw.push(flag);
        }
        let mut p = prune(&raw, None, &cfg());
        IndexAllocator::default().assign_fresh(&mut p.nodes);
        let text = render_full_within(&p.nodes, 1, compact_budget());
        assert!(text.contains("3 × list item › text › text:"), "{text}");
        let expanded = expand(&text);
        assert!(expanded.contains("text \"One\"\n"), "{expanded}");
        assert!(
            expanded.contains("text \"One flag\" actions=[edit]"),
            "{expanded}"
        );
    }

    #[test]
    fn column_names_stay_on_the_header_line() {
        let mut raw = vec![node(None, "window", "Shop"), node(Some(0), "table", "")];
        for h in ["SKU\nid", "Price | net"] {
            raw.push(node(Some(1), "table column header", h));
        }
        for r in 0..3 {
            for (c, v) in [format!("K-{r}"), format!("{r}.00")].iter().enumerate() {
                let mut cell = node(Some(1), "cell", v);
                cell.bounds = Some(Rect::new(
                    c as f64 * 50.0,
                    20.0 + r as f64 * 20.0,
                    50.0,
                    20.0,
                ));
                raw.push(cell);
            }
        }
        let mut p = prune(&raw, None, &cfg());
        IndexAllocator::default().assign_fresh(&mut p.nodes);
        let text = render_full_within(&p.nodes, 1, compact_budget());
        assert!(
            text.contains("cells, a row a line (SKU\\nid | Price ¦ net):"),
            "{text}"
        );
        assert!(expand(&text).contains("cell \"K-1\""), "{text}");
    }

    #[test]
    fn diffs_group_what_was_added_and_range_what_was_removed() {
        let mut raw = vec![node(None, "window", "Mail"), node(Some(0), "list", "Inbox")];
        for i in 0..10 {
            raw.push(node(Some(1), "list item", &format!("Message {i}")));
        }
        let mut alloc = IndexAllocator::default();
        let mut a = prune(&raw, None, &cfg()).nodes;
        alloc.assign_fresh(&mut a);
        let mut raw2 = vec![node(None, "window", "Mail"), node(Some(0), "list", "Inbox")];
        for i in 0..3 {
            raw2.push(node(Some(1), "list item", &format!("New {i}")));
        }
        let mut b = prune(&raw2, None, &cfg()).nodes;
        alloc.assign_stable(&mut b);
        let d = diff(&a, &b);
        let text = render_diff_with(&d, &b, DIFF_INTRO, true);
        assert!(text.contains("in 1 list \"Inbox\":\n+ "), "{text}");
        assert!(!text.contains("(in 1 list"), "{text}");
        assert!(text.contains("- 10 removed: 2–11"), "{text}");
        let wordy = render_diff_with(&d, &b, DIFF_INTRO, false);
        assert!(wordy.contains("(in 1 list \"Inbox\")") && wordy.contains("- 2 list item"));
    }

    #[test]
    fn offscreen_elements_are_dropped() {
        let mut raw = sample();
        raw[2].bounds = Some(Rect::new(5000.0, 5000.0, 10.0, 10.0));
        let p = prune(&raw, Some(Rect::new(0.0, 0.0, 100.0, 100.0)), &cfg());
        assert!(!p.nodes.iter().any(|n| n.role == "button"));
    }

    #[test]
    fn long_lists_fold_to_fit_the_token_budget() {
        let mut raw = vec![node(None, "window", "Mail"), node(Some(0), "list", "Inbox")];
        for i in 0..200 {
            raw.push(node(Some(1), "list item", &format!("Message {i}")));
        }
        raw.push(node(Some(0), "button", "Send"));
        let mut p = prune(&raw, None, &cfg());
        IndexAllocator::default().assign_fresh(&mut p.nodes);
        let full = render_full(&p.nodes, 1);
        use crate::config::Summarize;
        let b = |tokens, level| Budget {
            tokens,
            level,
            keep: 5,
            compact: false,
        };
        // No limit, plenty of room, or summarizing off: nothing changes.
        assert_eq!(
            render_full_within(&p.nodes, 1, b(0, Summarize::Normal)),
            full
        );
        assert_eq!(
            render_full_within(&p.nodes, 1, b(100_000, Summarize::Normal)),
            full
        );
        assert_eq!(render_full_within(&p.nodes, 1, b(10, Summarize::Off)), full);
        // Tight: the list is folded to its first and last items, the rest of
        // the window is kept.
        let folded = render_full_within(&p.nodes, 1, b(300, Summarize::Normal));
        assert!(crate::text::estimate_tokens(&folded) <= 300, "{folded}");
        assert!(folded.contains("Message 0") && folded.contains("Message 199"));
        assert!(!folded.contains("Message 100"));
        assert!(
            folded.contains("[… 193 more \"list item\" folded"),
            "{folded}"
        );
        assert!(folded.contains("button \"Send\""));
        // The selected message, deep in the list, is never folded away.
        let mut raw2 = raw.clone();
        raw2[2 + 100].states.selected = true;
        let mut p2 = prune(&raw2, None, &cfg());
        IndexAllocator::default().assign_fresh(&mut p2.nodes);
        let folded = render_full_within(&p2.nodes, 1, b(300, Summarize::Normal));
        assert!(folded.contains("Message 100"), "{folded}");
        assert!(
            folded.contains("[… 95 more \"list item\" folded"),
            "{folded}"
        );
        assert!(
            folded.contains("[… 97 more \"list item\" folded"),
            "{folded}"
        );
        // Keep more of each list.
        let more = render_full_within(
            &p.nodes,
            1,
            Budget {
                keep: 20,
                ..b(300, Summarize::Normal)
            },
        );
        assert!(
            more.contains("Message 19") && !more.contains("Message 20\""),
            "{more}"
        );
        // Tighter still: cut, saying so.
        let cut = render_full_within(&p.nodes, 1, b(40, Summarize::Normal));
        assert!(cut.contains("more lines not shown"), "{cut}");
        assert!(crate::text::estimate_tokens(&cut) <= 80, "{cut}");
        // Light: lists folded, nothing cut.
        let light = render_full_within(&p.nodes, 1, b(40, Summarize::Light));
        assert!(
            !light.contains("not shown") && light.contains("folded"),
            "{light}"
        );
        assert!(light.contains("button \"Send\""));
    }

    #[test]
    fn max_nodes_caps_output() {
        let mut raw = vec![node(None, "window", "W")];
        for i in 0..50 {
            raw.push(node(Some(0), "button", &format!("b{i}")));
        }
        let c = TreeConfig {
            max_nodes: 10,
            ..cfg()
        };
        let p = prune(&raw, None, &c);
        assert_eq!(p.nodes.len(), 10);
        assert_eq!(p.omitted, 41);
    }

    #[test]
    fn stable_indices_and_diff() {
        let mut alloc = IndexAllocator::default();
        let mut a = prune(&sample(), None, &cfg()).nodes;
        alloc.assign_fresh(&mut a);

        // Insert a new button before Save, change the field, remove nothing.
        let mut raw = sample();
        raw[4].value = Some("hello world".into());
        raw.insert(2, node(Some(1), "button", "New"));
        // Fix parents after insertion.
        for n in raw.iter_mut().skip(3) {
            if let Some(p) = n.parent.as_mut()
                && *p >= 2
            {
                *p += 1;
            }
        }
        let mut b = prune(&raw, None, &cfg()).nodes;
        alloc.assign_stable(&mut b);
        let save = b
            .iter()
            .find(|n| n.name.as_deref() == Some("Save"))
            .unwrap();
        assert_eq!(save.index, 1, "unchanged element keeps its index");
        let new = b.iter().find(|n| n.name.as_deref() == Some("New")).unwrap();
        assert_eq!(new.index, 3, "new element gets a fresh index");

        let d = diff(&a, &b);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.changed.len(), 1);
        assert!(d.removed.is_empty());
        let text = render_diff(&d, &b);
        assert!(
            text.contains("+ 3 button \"New\"  (in 0 window \"Doc\")"),
            "{text}"
        );
        assert!(
            text.contains("~ 2 text field value=\"hello world\""),
            "{text}"
        );
    }

    #[test]
    fn removed_elements_listed() {
        let mut alloc = IndexAllocator::default();
        let mut a = prune(&sample(), None, &cfg()).nodes;
        alloc.assign_fresh(&mut a);
        let mut raw = sample();
        raw[2].states.hidden = true;
        let mut b = prune(&raw, None, &cfg()).nodes;
        alloc.assign_stable(&mut b);
        let d = diff(&a, &b);
        assert_eq!(d.removed, vec![(1, "button \"Save\"".to_string())]);
        assert!(diff(&b, &b).is_empty());
    }

    #[test]
    fn text_cleanup() {
        assert_eq!(clean_text("  a\n\"b\"   c ", 100), "a\\n\\\"b\\\" c");
        assert_eq!(truncate("abcdef", 3), "abc… (+3 chars)");
    }
}
