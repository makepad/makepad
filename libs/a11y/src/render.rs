//! Token-lean text renderings for agents and people: the tree, one line per node; semantic diffs of two
//! snapshots; what a screen reader would announce; and lint findings.
//!
//! Node line: `#1a2 button "Save" ="value" focusable,default 10,20 80x24 [press,focus]`. Fields that are
//! empty are left out; bounds are window-local layout points, rounded.

use crate::mirror::Tree;
use crate::query::Query;
use crate::*;
use std::fmt::Write;

fn quote(s: &str, max: usize) -> String {
    let mut t: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        t.push('…');
    }
    format!("\"{}\"", t.replace('\n', "⏎").replace('"', "\\\""))
}

/// One node as one line (no indentation, no children).
pub fn node_line(n: &Node) -> String {
    let mut s = format!("#{} {}", n.id.short(), n.role.short());
    if !n.name.is_empty() {
        let _ = write!(s, " {}", quote(&n.name, 60));
    }
    let value = if !n.value.is_empty() { Some(n.value.as_str()) } else { n.text.as_ref().map(|t| t.content.as_str()).filter(|c| !c.is_empty() && *c != n.name) };
    if let Some(v) = value {
        let _ = write!(s, " ={}", quote(v, 40));
    }
    if let Some(r) = n.range {
        let _ = write!(s, " {}/{}..{}", fmt_num(r.now), fmt_num(r.min), fmt_num(r.max));
    }
    if n.toggle != Toggle::None {
        let _ = write!(s, " {}", n.toggle.short());
    }
    if !n.states.is_empty() {
        let _ = write!(s, " {}", n.states.list());
    }
    if n.set_size > 0 {
        let _ = write!(s, " {}/{}", n.pos_in_set, n.set_size);
    }
    if n.level > 0 {
        let _ = write!(s, " L{}", n.level);
    }
    if let Some(t) = &n.text {
        if t.selection.0 != t.selection.1 || n.states.contains(States::FOCUSED) {
            let _ = write!(s, " sel={}..{}", t.selection.0, t.selection.1);
        }
    }
    let b = n.bounds;
    if !b.is_empty() {
        let _ = write!(s, " {},{} {}x{}", b.x.round(), b.y.round(), b.w.round(), b.h.round());
    }
    if !n.actions.is_empty() {
        let _ = write!(s, " [{}]", n.actions.list());
    }
    if !n.custom_actions.is_empty() {
        let _ = write!(s, " +[{}]", n.custom_actions.join(","));
    }
    s
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e12 {
        format!("{}", v as i64)
    } else {
        format!("{:.3}", v).trim_end_matches('0').to_string()
    }
}

#[derive(Clone, Debug, Default)]
pub struct TreeOptions {
    /// Show only matching nodes (with their ancestors as context).
    pub query: Option<Query>,
    /// Start at this node instead of the root.
    pub node: Option<NodeId>,
    /// Levels below the start node (None = all).
    pub depth: Option<usize>,
    /// Stop after this many lines (the footer says how many were left out).
    pub max_lines: Option<usize>,
}

/// The tree as indented lines with a one-line footer.
pub fn tree_text(tree: &Tree, opt: &TreeOptions) -> String {
    let start = opt.node.unwrap_or(tree.root);
    if tree.node(start).is_none() {
        return if tree.nodes.is_empty() { "empty tree (nothing drawn yet, or a11y just turned on)\n".into() } else { format!("no node #{}\n", start.short()) };
    }
    // which nodes to print: everything under `start`, or the matches plus their ancestors
    let show: Option<std::collections::HashSet<NodeId>> = opt.query.as_ref().filter(|q| !q.is_empty()).map(|q| {
        let mut set = std::collections::HashSet::new();
        for n in q.find(tree) {
            set.insert(n.id);
            set.extend(tree.ancestors(n.id));
        }
        set
    });
    let matched = opt.query.as_ref().filter(|q| !q.is_empty()).map(|q| q.find(tree).len());
    let mut out = String::new();
    let (mut lines, mut total, mut omitted) = (0usize, 0usize, 0usize);
    let mut stack = vec![(start, 0usize)];
    while let Some((id, depth)) = stack.pop() {
        let Some(n) = tree.node(id) else { continue };
        total += 1;
        if let Some(s) = &show {
            if !s.contains(&id) {
                continue;
            }
        }
        if opt.depth.map(|d| depth > d).unwrap_or(false) {
            omitted += 1;
            continue;
        }
        if opt.max_lines.map(|m| lines >= m).unwrap_or(false) {
            omitted += 1;
        } else {
            for _ in 0..depth {
                out.push_str("  ");
            }
            out.push_str(&node_line(n));
            if tree.focus == id && !n.states.contains(States::FOCUSED) {
                out.push_str(" <focus");
            }
            out.push('\n');
            lines += 1;
        }
        for c in n.children.iter().rev() {
            stack.push((*c, depth + 1));
        }
    }
    let _ = write!(out, "-- {lines} shown of {total} nodes");
    if let Some(m) = matched {
        let _ = write!(out, ", {m} matched");
    }
    if omitted > 0 {
        let _ = write!(out, ", {omitted} omitted (depth/max)");
    }
    if !tree.focus.is_none() {
        let _ = write!(out, ", focus #{}", tree.focus.short());
    }
    out.push('\n');
    out
}

/// A semantic diff from `a` to `b`: `+ line` added, `- line` removed, `~ #id field: old -> new`.
pub fn diff_text(a: &Tree, b: &Tree) -> String {
    let mut out = String::new();
    let mut order: Vec<NodeId> = Vec::new();
    b.walk(|n, _| order.push(n.id));
    let mut removed: Vec<NodeId> = Vec::new();
    a.walk(|n, _| {
        if b.node(n.id).is_none() {
            removed.push(n.id)
        }
    });
    let (mut add, mut del, mut chg) = (0, 0, 0);
    for id in &removed {
        let _ = writeln!(out, "- {}", node_line(a.node(*id).unwrap()));
        del += 1;
    }
    for id in order {
        let nb = b.node(id).unwrap();
        match a.node(id) {
            None => {
                let _ = writeln!(out, "+ {}", node_line(nb));
                add += 1;
            }
            Some(na) if na != nb => {
                let mut fields = Vec::new();
                let mut f = |name: &str, x: String, y: String| {
                    if x != y {
                        fields.push(format!("{name}: {x} -> {y}"))
                    }
                };
                f("role", na.role.short().into(), nb.role.short().into());
                f("name", quote(&na.name, 40), quote(&nb.name, 40));
                f("value", quote(&na.value, 40), quote(&nb.value, 40));
                f("states", na.states.list(), nb.states.list());
                f("toggle", na.toggle.short().into(), nb.toggle.short().into());
                f("range", format!("{:?}", na.range.map(|r| r.now)), format!("{:?}", nb.range.map(|r| r.now)));
                f("actions", na.actions.list(), nb.actions.list());
                f("text", quote(na.text.as_ref().map(|t| t.content.as_str()).unwrap_or(""), 40), quote(nb.text.as_ref().map(|t| t.content.as_str()).unwrap_or(""), 40));
                f("selection", format!("{:?}", na.text.as_ref().map(|t| t.selection)), format!("{:?}", nb.text.as_ref().map(|t| t.selection)));
                f("children", na.children.len().to_string(), nb.children.len().to_string());
                f("description", quote(&na.description, 40), quote(&nb.description, 40));
                let moved = (na.bounds.x - nb.bounds.x).abs() >= 0.5 || (na.bounds.y - nb.bounds.y).abs() >= 0.5;
                let sized = (na.bounds.w - nb.bounds.w).abs() >= 0.5 || (na.bounds.h - nb.bounds.h).abs() >= 0.5;
                if fields.is_empty() && !moved && !sized {
                    continue; // sub-point jitter or fields this rendering does not show
                }
                if moved || sized {
                    fields.push(format!(
                        "bounds: {},{} {}x{} -> {},{} {}x{}",
                        na.bounds.x.round(), na.bounds.y.round(), na.bounds.w.round(), na.bounds.h.round(),
                        nb.bounds.x.round(), nb.bounds.y.round(), nb.bounds.w.round(), nb.bounds.h.round()
                    ));
                }
                let label = if nb.name.is_empty() { String::new() } else { format!(" {}", quote(&nb.name, 30)) };
                let _ = writeln!(out, "~ #{} {}{}: {}", id.short(), nb.role.short(), label, fields.join("; "));
                chg += 1;
            }
            _ => {}
        }
    }
    if a.focus != b.focus {
        let _ = writeln!(out, "~ focus: #{} -> #{}", a.focus.short(), b.focus.short());
    }
    let _ = writeln!(out, "-- {add} added, {del} removed, {chg} changed");
    out
}

/// What a screen reader would say when landing on `n`: name, role, value and the states that matter.
pub fn speak_node(tree: &Tree, n: &Node) -> String {
    let mut parts: Vec<String> = Vec::new();
    let name = if !n.name.is_empty() {
        n.name.clone()
    } else {
        // a labelled_by relation names it
        n.relations.labelled_by.iter().filter_map(|id| tree.node(*id)).map(|l| l.name.clone()).collect::<Vec<_>>().join(" ")
    };
    if !name.is_empty() {
        parts.push(name);
    }
    let role = if n.role_description.is_empty() { n.role.short().replace('_', " ") } else { n.role_description.clone() };
    if n.role != Role::Generic && n.role != Role::StaticText {
        parts.push(role);
    }
    if !n.value.is_empty() && !n.states.contains(States::PROTECTED) {
        parts.push(n.value.clone());
    } else if let Some(r) = n.range {
        parts.push(fmt_num(r.now));
    }
    match n.toggle {
        Toggle::On => parts.push(if n.role == Role::Switch { "on".into() } else { "checked".into() }),
        Toggle::Off => parts.push(if n.role == Role::Switch { "off".into() } else { "not checked".into() }),
        Toggle::Mixed => parts.push("mixed".into()),
        Toggle::None => {}
    }
    for (flag, word) in [
        (States::SELECTED, "selected"),
        (States::EXPANDED, "expanded"),
        (States::COLLAPSED, "collapsed"),
        (States::DISABLED, "dimmed"),
        (States::REQUIRED, "required"),
        (States::INVALID, "invalid"),
        (States::BUSY, "busy"),
    ] {
        if n.states.contains(flag) {
            parts.push(word.into());
        }
    }
    if n.set_size > 0 {
        parts.push(format!("{} of {}", n.pos_in_set, n.set_size));
    }
    if !n.description.is_empty() {
        parts.push(n.description.clone());
    }
    parts.join(", ")
}

/// The announcement stream since sequence `since`: `seq  what it says`.
pub fn speak_since(tree: &Tree, since: u64) -> String {
    let (list, omitted) = tree.notifications_since(since);
    let mut out = String::new();
    if omitted > 0 {
        let _ = writeln!(out, "({omitted} older notifications no longer kept)");
    }
    for (seq, n) in list {
        let line = match n {
            Notification::FocusChanged(id) => tree.node(*id).map(|n| format!("focus: {}", speak_node(tree, n))),
            Notification::ValueChanged(id) => tree.node(*id).map(|n| format!("value: {}", speak_node(tree, n))),
            Notification::NameChanged(id) => tree.node(*id).map(|n| format!("name: {}", speak_node(tree, n))),
            Notification::StatesChanged { node, old, new } => tree.node(*node).map(|n| {
                let on = States(new.0 & !old.0).list();
                let off = States(old.0 & !new.0).list();
                format!("state #{}: +{on} -{off}", n.id.short())
            }),
            Notification::LiveRegionChanged(id) => tree.node(*id).map(|n| format!("live: {}", speak_node(tree, n))),
            Notification::Announce { text, live } => Some(format!("announce({live:?}): {text}")),
            Notification::TextChanged { node, .. } => Some(format!("text changed #{}", node.short())),
            Notification::CaretMoved(id) => Some(format!("caret #{}", id.short())),
            Notification::ChildrenChanged(_) => None,
        };
        if let Some(l) = line {
            let _ = writeln!(out, "{seq} {l}");
        }
    }
    let _ = writeln!(out, "-- next since={}", tree.notify_seq);
    out
}

/// Accessibility problems, one per line: `#id role: problem`.
pub fn lint_text(tree: &Tree) -> String {
    let mut out = String::new();
    let mut count = 0;
    let mut issue = |n: &Node, what: &str| {
        let _ = writeln!(out, "#{} {}: {what}", n.id.short(), n.role.short());
        count += 1;
    };
    tree.walk(|n, _| {
        if n.states.contains(States::HIDDEN) {
            return;
        }
        let named = !n.name.is_empty() || !n.relations.labelled_by.is_empty();
        if n.role.is_interactive() && !named && !n.role.is_text_input() {
            issue(n, "interactive without a name");
        }
        if n.role.is_text_input() && !named && n.placeholder.is_empty() {
            issue(n, "text input without a name or label");
        }
        if n.states.contains(States::FOCUSABLE) && n.bounds.is_empty() {
            issue(n, "focusable with no bounds");
        }
        if n.role == Role::Image && !named {
            issue(n, "image without a name (set a11y: {hidden: true} if decorative)");
        }
        if n.live != Live::Off && n.name.is_empty() && n.value.is_empty() && n.children.is_empty() {
            issue(n, "empty live region");
        }
        if n.role.is_interactive() && n.actions.is_empty() && !n.states.contains(States::DISABLED) {
            issue(n, "interactive with no actions");
        }
    });
    if !tree.focus.is_none() && tree.node(tree.focus).is_none() {
        let _ = writeln!(out, "focus #{} is not in the tree", tree.focus.short());
        count += 1;
    }
    let _ = writeln!(out, "-- {count} issues");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(save_checked: bool) -> Tree {
        let mut t = Tree::new();
        let cb = Node {
            id: NodeId(3),
            role: Role::CheckBox,
            name: "Autosave".into(),
            toggle: if save_checked { Toggle::On } else { Toggle::Off },
            states: States::FOCUSABLE,
            actions: Actions::TOGGLE | Actions::FOCUS,
            bounds: Rect { x: 10.0, y: 40.0, w: 100.0, h: 20.0 },
            ..Default::default()
        };
        let btn = Node { id: NodeId(2), role: Role::Button, actions: Actions::PRESS, bounds: Rect { x: 10.0, y: 10.0, w: 80.0, h: 24.0 }, ..Default::default() };
        let root = Node { id: NodeId(1), role: Role::Window, name: "Notes".into(), children: vec![btn.id, cb.id], ..Default::default() };
        t.apply(TreeUpdate { full: true, root: root.id, focus: cb.id, nodes: vec![root, btn, cb], ..Default::default() });
        t
    }

    #[test]
    fn renderings() {
        let t = tree(false);
        let text = tree_text(&t, &TreeOptions::default());
        assert!(text.contains("#1 window \"Notes\""), "{text}");
        assert!(text.contains("  #3 check_box \"Autosave\" off focusable 10,40 100x20 [focus,toggle] <focus"), "{text}");
        let q = TreeOptions { query: Some(Query::parse("role=check_box").unwrap()), ..Default::default() };
        let text = tree_text(&t, &q);
        assert!(!text.contains("button"), "{text}");
        assert!(text.contains("1 matched"), "{text}");
        let d = diff_text(&t, &tree(true));
        assert!(d.contains("~ #3 check_box \"Autosave\": toggle: off -> on"), "{d}");
        assert_eq!(speak_node(&t, t.node(NodeId(3)).unwrap()), "Autosave, check box, not checked");
        let l = lint_text(&t);
        assert!(l.contains("#2 button: interactive without a name"), "{l}");
        assert!(l.contains("-- 1 issues"), "{l}");
    }
}
