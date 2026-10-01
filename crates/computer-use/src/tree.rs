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
    if s.editable {
        flags.push("editable");
    } else if s.value_settable {
        flags.push("settable");
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
    let mut out = String::new();
    for n in nodes {
        push_line(&mut out, n, indent);
    }
    out
}

fn push_line(out: &mut String, n: &Node, indent: usize) {
    for _ in 0..n.depth * indent {
        out.push(' ');
    }
    out.push_str(&format!("{} {}\n", n.index, n.line));
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
}

impl Budget {
    pub fn from_config(cfg: &TreeConfig) -> Self {
        Self {
            tokens: cfg.max_tokens,
            level: cfg.summarize,
            keep: cfg.fold_keep,
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
    let full = render_full(nodes, indent);
    if !budget.active() || crate::text::estimate_tokens(&full) <= budget.tokens {
        return full;
    }
    let lists = render_folded(nodes, indent, budget.keep, |role| {
        LIST_ROLES.contains(&role)
    });
    if budget.level == Summarize::Light || crate::text::estimate_tokens(&lists) <= budget.tokens {
        return lists;
    }
    let any = render_folded(nodes, indent, budget.keep, |_| true);
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
    for members in groups
        .values()
        .filter(|m| m.len() >= keep + FOLD_TAIL + FOLD_MIN_HIDDEN)
    {
        let mut run: Option<usize> = None;
        for &i in &members[keep..members.len() - FOLD_TAIL] {
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
    let mut out = String::new();
    let mut i = 0;
    while i < nodes.len() {
        let n = &nodes[i];
        if !hidden[i] {
            push_line(&mut out, n, indent);
            i += 1;
            continue;
        }
        if let Some(count) = folded_at.get(&i) {
            for _ in 0..n.depth * indent {
                out.push(' ');
            }
            out.push_str(&format!(
                "[… {count} more \"{}\" folded; find_element finds them]\n",
                n.role
            ));
        }
        // Skip the folded element and everything inside it.
        i += 1;
        while i < nodes.len() && nodes[i].depth > n.depth {
            i += 1;
        }
    }
    out
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
    render_diff_with(d, new, DIFF_INTRO)
}

/// Render a diff under a custom intro line.
pub fn render_diff_with(d: &Diff, new: &[Node], intro: &str) -> String {
    let mut out = String::with_capacity(intro.len() + 64 * d.len());
    out.push_str(intro);
    out.push('\n');
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
        out.push_str(&format!(
            "~ {} {}  (was: {})\n",
            n.index,
            n.line,
            was(old_line, &n.line)
        ));
    }
    for (idx, old_line) in &d.removed {
        out.push_str(&format!("- {idx} {old_line}\n"));
    }
    out
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
