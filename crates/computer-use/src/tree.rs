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
    /// Identity across snapshots.
    pub key: String,
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

fn render_line(n: &RawNode, max_text: usize) -> String {
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
    if s.editable {
        flags.push("editable");
    } else if s.value_settable {
        flags.push("settable");
    }
    if !flags.is_empty() {
        line.push_str(&format!(" ({})", flags.join(", ")));
    }

    let interactive = roles::is_interactive(&n.role);
    let secondary: Vec<&str> = n
        .actions
        .iter()
        .map(|a| a.name.as_str())
        .filter(|a| *a != "press" && *a != "scroll_to_visible")
        .filter(|a| interactive || *a != "show_menu")
        .collect();
    if !secondary.is_empty() {
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

    // Identity keys: backend keys where available, else a structural path of
    // role + label + position among same-looking siblings.
    let mut keys: Vec<String> = Vec::with_capacity(n);
    let mut sibling_counts: HashMap<(Option<usize>, String), usize> = HashMap::new();
    let mut root_order = 0usize;
    let mut root_of: Vec<usize> = vec![0; n];
    for (i, node) in raw.iter().enumerate() {
        let parent = node.parent.filter(|p| *p < i);
        // Roots (the window, the menu bar) are keyed by role alone so a title
        // change doesn't re-key the whole tree.
        let step = match node
            .name
            .as_deref()
            .filter(|s| !s.is_empty() && parent.is_some())
        {
            Some(name) => format!("{}:{}", node.role, truncate(name, 40)),
            None => node.role.clone(),
        };
        let nth = sibling_counts
            .entry((parent, step.clone()))
            .and_modify(|c| *c += 1)
            .or_insert(0);
        let structural = match parent {
            Some(p) => format!("{}/{step}#{nth}", keys[p]),
            None => format!("{step}#{nth}"),
        };
        keys.push(node.key.clone().unwrap_or(structural));
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
        let mut p = node.parent;
        while let Some(pi) = p {
            if candidate[pi] {
                if raw[pi].name.is_some() && raw[pi].name == node.name {
                    candidate[i] = false;
                }
                break;
            }
            p = raw[pi].parent;
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
    let mut seen_keys: HashSet<String> = HashSet::new();
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

        let mut key = keys[i].clone();
        let mut dup = 1;
        while !seen_keys.insert(key.clone()) {
            key = format!("{}~{dup}", keys[i]);
            dup += 1;
        }

        kept_pos[i] = Some(out.nodes.len());
        out.nodes.push(Node {
            index: 0,
            depth,
            parent,
            key,
            handle: node.handle,
            role: node.role.clone(),
            name: node.name.clone().filter(|s| !s.trim().is_empty()),
            value: node.value.clone(),
            bounds: node.bounds,
            actions: node.actions.clone(),
            states: node.states.clone(),
            line: render_line(node, cfg.max_text_len),
        });
    }
    out
}

/// Hands out element indices. Kept per app so diffs can reuse indices.
#[derive(Debug, Default, Clone)]
pub struct IndexAllocator {
    by_key: HashMap<String, u32>,
    next: u32,
}

impl IndexAllocator {
    /// Fresh numbering in document order.
    pub fn assign_fresh(&mut self, nodes: &mut [Node]) {
        self.by_key.clear();
        self.next = 0;
        self.assign_stable(nodes);
    }

    /// Keep indices of elements seen before; new elements get new numbers.
    pub fn assign_stable(&mut self, nodes: &mut [Node]) {
        let live: HashSet<&str> = nodes.iter().map(|n| n.key.as_str()).collect();
        self.by_key.retain(|k, _| live.contains(k.as_str()));
        for node in nodes.iter_mut() {
            node.index = match self.by_key.get(&node.key) {
                Some(i) => *i,
                None => {
                    let i = self.next;
                    self.next += 1;
                    self.by_key.insert(node.key.clone(), i);
                    i
                }
            };
        }
    }
}

pub fn render_full(nodes: &[Node]) -> String {
    let mut out = String::new();
    for n in nodes {
        for _ in 0..n.depth {
            out.push_str("  ");
        }
        out.push_str(&format!("{} {}\n", n.index, n.line));
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
    let old_by_key: HashMap<&str, &Node> = old.iter().map(|n| (n.key.as_str(), n)).collect();
    let new_keys: HashSet<&str> = new.iter().map(|n| n.key.as_str()).collect();
    let mut d = Diff::default();
    for (pos, n) in new.iter().enumerate() {
        match old_by_key.get(n.key.as_str()) {
            None => d.added.push(pos),
            Some(o) if o.line != n.line => d.changed.push((pos, o.line.clone())),
            Some(_) => {}
        }
    }
    for o in old {
        if !new_keys.contains(o.key.as_str()) {
            d.removed.push((o.index, o.line.clone()));
        }
    }
    d
}

pub fn render_diff(d: &Diff, new: &[Node]) -> String {
    if d.is_empty() {
        return "No changes to the accessibility tree since the previous get_app_state.\n".into();
    }
    let mut out = String::from(
        "Changes since the previous get_app_state (+ added, ~ changed, - removed). Unchanged elements keep their indices.\n",
    );
    for &pos in &d.added {
        let n = &new[pos];
        let ctx = n
            .parent
            .map(|p| format!("  (in {} {})", new[p].index, new[p].label()))
            .unwrap_or_default();
        out.push_str(&format!("+ {} {}{}\n", n.index, n.line, ctx));
    }
    for (pos, old_line) in &d.changed {
        let n = &new[*pos];
        out.push_str(&format!("~ {} {}  (was: {})\n", n.index, n.line, old_line));
    }
    for (idx, old_line) in &d.removed {
        out.push_str(&format!("- {idx} {old_line}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::NodeStates;

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
        let text = render_full(&p.nodes);
        assert_eq!(
            text,
            "0 window \"Doc\"\n  1 button \"Save\"\n  2 text field value=\"hello\" (editable)\n"
        );
    }

    #[test]
    fn offscreen_elements_are_dropped() {
        let mut raw = sample();
        raw[2].bounds = Some(Rect::new(5000.0, 5000.0, 10.0, 10.0));
        let p = prune(&raw, Some(Rect::new(0.0, 0.0, 100.0, 100.0)), &cfg());
        assert!(!p.nodes.iter().any(|n| n.role == "button"));
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
