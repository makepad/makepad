//! The receiving side of [`TreeUpdate`]s: one [`Tree`] per window, answering everything an OS adapter
//! or a tool asks synchronously (nodes, parents, hit tests, text ranges in the OS's units) without
//! calling into the app.

use crate::*;
use std::collections::HashMap;

/// How many notifications a tree keeps for `ui speak since=` (a diagnostic history; older ones are
/// reported as omitted, never block anything).
pub const NOTIFICATION_HISTORY: usize = 4096;

#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub root: NodeId,
    pub focus: NodeId,
    pub nodes: HashMap<NodeId, Node>,
    parents: HashMap<NodeId, NodeId>,
    /// Updates applied so far.
    pub updates: u64,
    /// Sequence number of the next notification.
    pub notify_seq: u64,
    /// (seq, notification), oldest first.
    pub history: std::collections::VecDeque<(u64, Notification)>,
}

impl Tree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one update; returns the notifications it carried (the adapter raises them).
    pub fn apply(&mut self, u: TreeUpdate) -> Vec<Notification> {
        if u.full {
            self.nodes.clear();
            self.parents.clear();
        }
        for id in &u.removed {
            self.nodes.remove(id);
            self.parents.remove(id);
        }
        for n in u.nodes {
            for c in &n.children {
                self.parents.insert(*c, n.id);
            }
            self.nodes.insert(n.id, n);
        }
        self.root = u.root;
        self.focus = u.focus;
        self.updates += 1;
        if u.full {
            self.prune_unreachable();
        }
        for n in &u.notifications {
            self.history.push_back((self.notify_seq, n.clone()));
            self.notify_seq += 1;
            if self.history.len() > NOTIFICATION_HISTORY {
                self.history.pop_front();
            }
        }
        u.notifications
    }

    /// Drops nodes the root cannot reach (only needed after a full update from a careless producer).
    pub fn prune_unreachable(&mut self) {
        let mut keep = std::collections::HashSet::new();
        let mut stack = vec![self.root];
        while let Some(id) = stack.pop() {
            if let Some(n) = self.nodes.get(&id) {
                if keep.insert(id) {
                    stack.extend(n.children.iter().copied());
                }
            }
        }
        self.nodes.retain(|id, _| keep.contains(id));
        self.parents.retain(|id, _| keep.contains(id));
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        if id == self.root {
            return None;
        }
        let p = *self.parents.get(&id)?;
        // a stale parent link (the node moved and the old parent was updated) is checked against the
        // parent's current children
        self.nodes.get(&p).filter(|pn| pn.children.contains(&id)).map(|_| p)
    }

    pub fn children(&self, id: NodeId) -> impl Iterator<Item = &Node> {
        self.nodes.get(&id).into_iter().flat_map(move |n| n.children.iter().filter_map(move |c| self.nodes.get(c)))
    }

    /// Index of `id` among its parent's children.
    pub fn index_in_parent(&self, id: NodeId) -> Option<usize> {
        let p = self.parent(id)?;
        self.nodes.get(&p)?.children.iter().position(|c| *c == id)
    }

    /// Root-first chain of ancestors (excluding `id`).
    pub fn ancestors(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut cur = id;
        while let Some(p) = self.parent(cur) {
            if out.contains(&p) {
                break;
            }
            out.push(p);
            cur = p;
        }
        out.reverse();
        out
    }

    /// Depth-first, draw order (parents before children).
    pub fn walk<'a>(&'a self, mut f: impl FnMut(&'a Node, usize)) {
        let mut stack = vec![(self.root, 0usize)];
        let mut seen = std::collections::HashSet::new();
        while let Some((id, depth)) = stack.pop() {
            let Some(n) = self.nodes.get(&id) else { continue };
            if !seen.insert(id) {
                continue;
            }
            f(n, depth);
            for c in n.children.iter().rev() {
                stack.push((*c, depth + 1));
            }
        }
    }

    /// The deepest visible node under a window-local point (later-drawn siblings first: they are on
    /// top). Nodes with empty bounds are transparent containers.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<NodeId> {
        fn visit(t: &Tree, id: NodeId, x: f32, y: f32, depth: usize) -> Option<NodeId> {
            let n = t.nodes.get(&id)?;
            if n.states.contains(States::HIDDEN) || depth > 4096 {
                return None;
            }
            let inside = n.bounds.contains(x, y);
            if !inside && !n.bounds.is_empty() {
                return None;
            }
            for c in n.children.iter().rev() {
                if let Some(hit) = visit(t, *c, x, y, depth + 1) {
                    return Some(hit);
                }
            }
            inside.then_some(id)
        }
        visit(self, self.root, x, y, 0)
    }

    /// Recent notifications with sequence number >= `since`, and how many older ones were dropped
    /// from the history.
    pub fn notifications_since(&self, since: u64) -> (Vec<&(u64, Notification)>, u64) {
        let first = self.history.front().map(|(s, _)| *s).unwrap_or(self.notify_seq);
        let omitted = first.saturating_sub(since);
        (self.history.iter().filter(|(s, _)| *s >= since).collect(), omitted)
    }
}

/// Text unit conversions and range geometry. OS adapters speak UTF-16 (macOS, Windows, Android, web)
/// or code points (AT-SPI); the wire speaks UTF-8 bytes.
pub mod text {
    use crate::*;

    pub fn byte_to_utf16(s: &str, byte: u32) -> u32 {
        let b = floor_boundary(s, byte as usize);
        s[..b].encode_utf16().count() as u32
    }

    pub fn utf16_to_byte(s: &str, unit: u32) -> u32 {
        let mut n = 0u32;
        for (i, c) in s.char_indices() {
            if n >= unit {
                return i as u32;
            }
            n += c.len_utf16() as u32;
        }
        s.len() as u32
    }

    pub fn byte_to_char(s: &str, byte: u32) -> u32 {
        let b = floor_boundary(s, byte as usize);
        s[..b].chars().count() as u32
    }

    pub fn char_to_byte(s: &str, ch: u32) -> u32 {
        s.char_indices().nth(ch as usize).map(|(i, _)| i as u32).unwrap_or(s.len() as u32)
    }

    pub fn floor_boundary(s: &str, mut b: usize) -> usize {
        if b >= s.len() {
            return s.len();
        }
        while !s.is_char_boundary(b) {
            b -= 1;
        }
        b
    }

    /// The line holding byte offset `byte` (the last line for the end of the text).
    pub fn line_for_byte(t: &TextInfo, byte: u32) -> usize {
        match t.lines.iter().rposition(|l| l.start <= byte) {
            Some(i) => i,
            None => 0,
        }
    }

    /// Byte range of line `line` (end exclusive, including its trailing newline if any).
    pub fn line_range(t: &TextInfo, line: usize) -> (u32, u32) {
        let start = t.lines.get(line).map(|l| l.start).unwrap_or(t.content.len() as u32);
        let end = t.lines.get(line + 1).map(|l| l.start).unwrap_or(t.content.len() as u32);
        (start, end)
    }

    /// Offset into `glyph_x` of line `line`'s first boundary, and that line's boundary count.
    fn glyph_span(t: &TextInfo, line: usize) -> Option<(usize, usize)> {
        if t.glyph_x.is_empty() {
            return None;
        }
        let mut off = 0usize;
        for i in 0..t.lines.len() {
            let (s, e) = line_range(t, i);
            let chars = t.content.get(s as usize..e as usize).map(|x| x.chars().count()).unwrap_or(0) + 1;
            if i == line {
                return (off + chars <= t.glyph_x.len()).then_some((off, chars));
            }
            off += chars;
        }
        None
    }

    /// x of the char boundary at byte offset `byte` on its line.
    pub fn x_at_byte(t: &TextInfo, byte: u32) -> f32 {
        let line = line_for_byte(t, byte);
        let Some(lb) = t.lines.get(line) else { return 0.0 };
        let (s, e) = line_range(t, line);
        let byte = byte.clamp(s, e);
        let seg = t.content.get(s as usize..e as usize).unwrap_or("");
        let k = byte_to_char(seg, byte - s) as usize;
        if let Some((off, count)) = glyph_span(t, line) {
            if k < count {
                return t.glyph_x[off + k];
            }
        }
        // no per-glyph data: spread the line box evenly
        let n = seg.chars().count().max(1) as f32;
        lb.rect.x + lb.rect.w * (k as f32 / n)
    }

    /// One rect per line the byte range covers (window-local).
    pub fn range_rects(t: &TextInfo, start: u32, end: u32) -> Vec<Rect> {
        let (start, end) = (start.min(end), start.max(end));
        let mut out = Vec::new();
        if t.lines.is_empty() {
            return out;
        }
        let (l0, l1) = (line_for_byte(t, start), line_for_byte(t, end));
        for line in l0..=l1 {
            let lb = t.lines[line];
            let (s, e) = line_range(t, line);
            let a = start.max(s);
            let b = end.min(e);
            let xa = x_at_byte(t, a);
            let xb = if line < l1 { lb.rect.x + lb.rect.w } else { x_at_byte(t, b) };
            out.push(Rect { x: xa, y: lb.rect.y, w: (xb - xa).max(1.0), h: lb.rect.h });
        }
        out
    }

    /// The byte offset nearest to a window-local point.
    pub fn byte_at_point(t: &TextInfo, x: f32, y: f32) -> u32 {
        if t.lines.is_empty() {
            return 0;
        }
        let line = t.lines.iter().rposition(|l| l.rect.y <= y).unwrap_or(0);
        let (s, e) = line_range(t, line);
        let seg = t.content.get(s as usize..e as usize).unwrap_or("");
        let mut best = (f32::MAX, s);
        for (i, _) in seg.char_indices().chain(std::iter::once((seg.len(), ' '))) {
            let b = s + i as u32;
            let d = (x_at_byte(t, b) - x).abs();
            if d < best.0 {
                best = (d, b);
            }
        }
        best.1
    }
}

/// All windows' trees of one process or host, keyed by an opaque window key (the HAL uses
/// (session << 32) | window index).
#[derive(Default)]
pub struct Mirror {
    pub trees: HashMap<u64, Tree>,
}

impl Mirror {
    pub fn apply(&mut self, window: u64, u: TreeUpdate) -> Vec<Notification> {
        self.trees.entry(window).or_default().apply(u)
    }
    pub fn tree(&self, window: u64) -> Option<&Tree> {
        self.trees.get(&window)
    }
    pub fn drop_window(&mut self, window: u64) {
        self.trees.remove(&window);
    }
    /// Finds which window holds a node.
    pub fn window_of(&self, id: NodeId) -> Option<u64> {
        self.trees.iter().find(|(_, t)| t.nodes.contains_key(&id)).map(|(w, _)| *w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Tree {
        let mut t = Tree::new();
        let a = Node { id: NodeId(2), role: Role::Button, bounds: Rect { x: 0.0, y: 0.0, w: 50.0, h: 50.0 }, ..Default::default() };
        let b = Node { id: NodeId(3), role: Role::Button, bounds: Rect { x: 25.0, y: 25.0, w: 50.0, h: 50.0 }, ..Default::default() };
        let g = Node { id: NodeId(4), role: Role::Group, children: vec![a.id, b.id], ..Default::default() };
        let root = Node {
            id: NodeId(1),
            role: Role::Window,
            bounds: Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 },
            children: vec![g.id],
            ..Default::default()
        };
        t.apply(TreeUpdate { full: true, root: root.id, nodes: vec![root, g, a, b], ..Default::default() });
        t
    }

    #[test]
    fn structure_and_hits() {
        let t = tree();
        assert_eq!(t.parent(NodeId(2)), Some(NodeId(4)));
        assert_eq!(t.ancestors(NodeId(3)), vec![NodeId(1), NodeId(4)]);
        assert_eq!(t.hit_test(30.0, 30.0), Some(NodeId(3)), "later sibling is on top");
        assert_eq!(t.hit_test(5.0, 5.0), Some(NodeId(2)));
        assert_eq!(t.hit_test(90.0, 10.0), Some(NodeId(1)));
        assert_eq!(t.hit_test(200.0, 10.0), None);
    }

    #[test]
    fn moves_and_removals() {
        let mut t = tree();
        // move 3 to the root, drop the group's link to it
        let mut g = t.node(NodeId(4)).unwrap().clone();
        g.children = vec![NodeId(2)];
        let mut root = t.node(NodeId(1)).unwrap().clone();
        root.children = vec![NodeId(4), NodeId(3)];
        t.apply(TreeUpdate { root: NodeId(1), nodes: vec![g, root], ..Default::default() });
        assert_eq!(t.parent(NodeId(3)), Some(NodeId(1)));
        t.apply(TreeUpdate { root: NodeId(1), removed: vec![NodeId(2)], ..Default::default() });
        assert!(t.node(NodeId(2)).is_none());
    }

    #[test]
    fn text_units_and_geometry() {
        let s = "aé😀b";
        assert_eq!(text::byte_to_utf16(s, 3), 2);
        assert_eq!(text::byte_to_utf16(s, 7), 4);
        assert_eq!(text::utf16_to_byte(s, 4), 7);
        assert_eq!(text::byte_to_char(s, 7), 3);
        assert_eq!(text::char_to_byte(s, 3), 7);
        let t = TextInfo {
            content: "ab\ncd".into(),
            selection: (0, 0),
            lines: vec![
                LineBox { start: 0, rect: Rect { x: 0.0, y: 0.0, w: 30.0, h: 10.0 }, baseline: 8.0 },
                LineBox { start: 3, rect: Rect { x: 0.0, y: 10.0, w: 20.0, h: 10.0 }, baseline: 18.0 },
            ],
            glyph_x: vec![0.0, 10.0, 20.0, 30.0, 0.0, 10.0, 20.0],
        };
        assert_eq!(text::line_for_byte(&t, 4), 1);
        assert_eq!(text::line_range(&t, 0), (0, 3));
        assert_eq!(text::x_at_byte(&t, 1), 10.0);
        assert_eq!(text::x_at_byte(&t, 4), 10.0);
        let r = text::range_rects(&t, 1, 4);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].x, 10.0);
        assert_eq!(r[1].w, 10.0);
        assert_eq!(text::byte_at_point(&t, 11.0, 15.0), 4);
    }
}
