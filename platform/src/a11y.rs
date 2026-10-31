//! Draw-time semantic tree builder. This owns plain state, independent of a Cx or OS backend.
use crate::makepad_math::{dvec2, Mat4f, Rect};
use crate::{
    area::Area,
    cx::Cx,
    cx_api::{CxOsApi, CxOsOp},
    draw_list::DrawListId,
    window::WindowId,
};
use makepad_a11y::{
    ActionRequest, ActionResult, Actions, Live, Node, NodeId, Notification, Role, States,
    TreeUpdate,
};
use std::collections::{HashMap, HashSet};

type WindowKey = (usize, u64);
fn key(w: WindowId) -> WindowKey {
    (w.0, w.1)
}

#[derive(Default)]
struct Record {
    node: Node,
    next: Node,
    area: Area,
    list: Option<DrawListId>,
    redraw: u64,
    seen: u64,
    annotated: u64,
    parent: NodeId,
    children: Vec<NodeId>,
    drawn_children: Vec<NodeId>,
    alive: bool,
    emitted: bool,
    reachable: bool,
    semantic_parent: NodeId,
    depth: usize,
    owner: NodeId,
    part_key: u64,
    label_for: NodeId,
    generated_labels: Vec<NodeId>,
    parts: HashMap<u64, NodeId>,
    drawn_parts: Vec<NodeId>,
}
#[derive(Default)]
struct WindowTree {
    records: HashMap<NodeId, Record>,
    links: HashSet<(NodeId, NodeId)>,
    root: NodeId,
    sent_root: NodeId,
    focus: NodeId,
    full: bool,
    ids: Vec<NodeId>,
    walk: Vec<(NodeId, NodeId, usize)>,
    update: TreeUpdate,
    bytes: Vec<u8>,
}
#[derive(Clone, Copy)]
struct Frame {
    id: NodeId,
    window: Option<WindowKey>,
    list: Option<DrawListId>,
}

#[derive(Default)]
pub struct CxA11y {
    pub on: bool,
    consumers: u32,
    frame: u64,
    windows: HashMap<WindowKey, WindowTree>,
    stack: Vec<Frame>,
    suspended: Vec<Frame>,
    outside_windows: HashSet<NodeId>,
    window_stack: Vec<WindowKey>,
    announcements: Vec<Notification>,
    pending: Option<(u64, ActionResult)>,
    requested_focus: NodeId,
}

impl CxA11y {
    /// Returns true on activation; the owning Cx must then redraw every window.
    pub fn set_consumers(&mut self, consumers: u32) -> bool {
        let activate = !self.on && consumers != 0;
        if consumers == 0 {
            *self = Self::default();
            return false;
        }
        self.consumers = consumers;
        self.on = true;
        if activate {
            for w in self.windows.values_mut() {
                w.full = true;
            }
        }
        activate
    }
    pub(crate) fn request_full(&mut self) {
        for w in self.windows.values_mut() { w.full = true; }
    }
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        self.stack.clear();
        self.suspended.clear();
        self.outside_windows.clear();
        self.window_stack.clear();
        for tree in self.windows.values_mut() {
            tree.links.clear();
        }
    }
    pub fn begin_window(&mut self, window: WindowId) {
        let key = key(window);
        self.window_stack.push(key);
        let tree = self.windows.entry(key).or_insert_with(|| WindowTree {
            full: true,
            ..Default::default()
        });
        if let Some(frame) = self.stack.last_mut() {
            frame.window = Some(key);
            self.outside_windows.remove(&frame.id);
            tree.root = frame.id;
            Self::touch(tree, frame.id, NodeId::NONE, frame.list, self.frame);
        }
    }
    pub fn end_window(&mut self) {
        self.window_stack.pop();
    }
    pub fn window_root(&self, token: u32) -> u64 {
        self.stack[token as usize]
            .window
            .and_then(|w| self.windows.get(&w))
            .map_or(0, |w| w.root.uid())
    }
    /// A Window opens its own draw list after its WidgetRef has entered the builder.
    pub fn window_draw_list(&mut self, list: DrawListId) {
        if let Some(frame) = self.stack.last_mut() {
            frame.list = Some(list);
            if let Some(w) = frame.window {
                if let Some(rec) = self
                    .windows
                    .get_mut(&w)
                    .and_then(|w| w.records.get_mut(&frame.id))
                {
                    rec.list = Some(list);
                }
            }
        }
    }
    fn touch(
        tree: &mut WindowTree,
        id: NodeId,
        parent: NodeId,
        list: Option<DrawListId>,
        frame: u64,
    ) {
        let rec = tree.records.entry(id).or_default();
        if rec.seen != frame {
            rec.drawn_children.clear();
            rec.drawn_parts.clear();
        }
        rec.seen = frame;
        rec.parent = parent;
        rec.list = list;
        if let Some(p) = tree.records.get_mut(&parent) {
            if tree.links.insert((parent, id)) {
                p.drawn_children.push(id);
            }
        }
    }
    /// Keep the yielded widget as the parent of content drawn by its caller between
    /// DrawStep entries. Its token still closes exactly once, on the current return.
    pub fn suspend_node(&mut self, token: u32) {
        let frame = self.stack[token as usize];
        if frame.window.is_some() {
            self.suspended.push(frame);
        }
    }
    pub fn begin_node(&mut self, uid: u64, list: Option<DrawListId>) -> u32 {
        let id = NodeId::widget(uid);
        let window = self.window_stack.last().copied().or_else(|| {
            // A Root (or another caller outside all windows) can also resume a step.
            // It must not become a child of the yielded list merely by re-entering.
            (!self.outside_windows.contains(&id))
                .then(|| self.suspended.last().and_then(|f| f.window))
                .flatten()
        });
        if let Some(w) = window {
            let tree = self.windows.get_mut(&w).unwrap();
            while self
                .suspended
                .last()
                .is_some_and(|f| f.window == window && ancestor(tree, id, f.id))
            {
                self.suspended.pop();
            }
            let mut parent = self
                .stack
                .last()
                .filter(|f| f.window == window)
                .map_or(NodeId::NONE, |f| f.id);
            if let Some(suspended) = self.suspended.last().filter(|f| f.window == window) {
                if parent.is_none() || ancestor(tree, parent, suspended.id) {
                    parent = suspended.id;
                }
            }
            Self::touch(tree, id, parent, list, self.frame);
        }
        if window.is_none() {
            self.outside_windows.insert(id);
        }
        let token = self.stack.len() as u32;
        self.stack.push(Frame { id, window, list });
        token
    }
    /// Reuse the previous draw's strings and vectors. Semantics implementations overwrite the
    /// fields they own and compare strings before copying; class/path are computed only once.
    pub fn take_node(&mut self, token: u32) -> Node {
        let f = self.stack[token as usize];
        f.window
            .and_then(|w| self.windows.get_mut(&w))
            .and_then(|w| w.records.get_mut(&f.id))
            .map(|r| std::mem::take(&mut r.next))
            .unwrap_or_default()
    }
    pub fn end_node(&mut self, token: u32, mut node: Node, area: Area) {
        debug_assert_eq!(token as usize + 1, self.stack.len());
        let f = self.stack.pop().unwrap();
        let Some(w) = f.window else { return };
        let rec = self
            .windows
            .get_mut(&w)
            .unwrap()
            .records
            .get_mut(&f.id)
            .unwrap();
        node.id = f.id;
        rec.next = node;
        rec.area = area;
        rec.list = area.draw_list_id().or(f.list);
        let list = rec.list;
        let mut parts = std::mem::take(&mut rec.drawn_parts);
        let tree = self.windows.get_mut(&w).unwrap();
        for id in &parts {
            if let Some(part) = tree.records.get_mut(id) { part.area = area; part.list = list; }
        }
        parts.clear();
        tree.records.get_mut(&f.id).unwrap().drawn_parts = parts;
    }
    /// Insert a non-widget child. Keys are stable application item identities, not draw indices.
    /// Overflow of the wire's 16-bit part field uses a fresh widget id from the same allocator;
    /// dispatch always resolves the owner, so large virtual collections do not truncate or collide.
    pub fn set_part(&mut self, token: u32, key: u64, parent_key: Option<u64>, mut node: Node, alloc: impl FnOnce() -> u64) {
        let f = self.stack[token as usize];
        let Some(w) = f.window else { return };
        let tree = self.windows.get_mut(&w).unwrap();
        if !node.id.is_none() {
            if let Some(child) = tree.records.get_mut(&node.id) {
                // An annotation updates a cached description, not its draw/link
                // lifetime. In particular, Dock annotates cached panel bodies:
                // marking them drawn would discard their retained parent link,
                // or revive a body whose content list has already been replaced.
                if child.seen != self.frame && child.annotated != self.frame {
                    reuse_node(&mut child.next, &child.node);
                }
                child.annotated = self.frame;
                child.next.role = node.role;
                child.next.pos_in_set = node.pos_in_set;
                child.next.set_size = node.set_size;
                child.next.level = node.level;
                child.next.states.insert(node.states);
                child.next.actions.insert(node.actions);
                if node.table.is_some() { child.next.table = node.table; }
                if !node.name.is_empty() { child.next.name = node.name; }
            }
            return;
        }
        let owner = tree.records.get_mut(&f.id).unwrap();
        let next = owner.parts.len() + 1;
        let id = *owner.parts.entry(key).or_insert_with(|| {
            u16::try_from(next).map_or_else(|_| NodeId::widget(alloc()), |part| NodeId::part(f.id.uid(), part))
        });
        owner.drawn_parts.push(id);
        let parent = parent_key.and_then(|key| owner.parts.get(&key).copied()).unwrap_or(f.id);
        Self::touch(tree, id, parent, f.list, self.frame);
        node.id = id;
        let rec = tree.records.get_mut(&id).unwrap();
        rec.owner = f.id;
        rec.part_key = key;
        rec.next = node;
    }
    pub fn is_current_node(&self, uid: u64) -> bool { self.stack.last().is_some_and(|f| f.id == NodeId::widget(uid)) }
    pub fn set_label_for(&mut self, token: u32, target: u64) {
        let f = self.stack[token as usize];
        if let Some(rec) = f.window.and_then(|w| self.windows.get_mut(&w)).and_then(|w| w.records.get_mut(&f.id)) { rec.label_for = NodeId::widget(target); }
    }
    pub fn action_owner(&self, id: NodeId) -> NodeId {
        self.windows.values().find_map(|tree| tree.records.get(&id))
            .filter(|r| !r.owner.is_none()).map_or(id, |r| r.owner)
    }
    pub fn action_part_key(&self, id: NodeId) -> Option<u64> {
        self.windows.values().find_map(|tree| tree.records.get(&id))
            .filter(|r| !r.owner.is_none()).map(|r| r.part_key)
    }
    pub fn close_window(&mut self, window: WindowId) {
        self.windows.remove(&key(window));
    }
    pub fn recycle_bytes(&mut self, window: WindowId, mut bytes: Vec<u8>) {
        if let Some(w) = self.windows.get_mut(&key(window)) {
            bytes.clear();
            w.bytes = bytes;
        }
    }
    pub fn record_action(&mut self, req: u64, result: ActionResult) {
        if let Some((id, value)) = self.pending.as_mut() {
            if *id == req && *value == ActionResult::Unhandled {
                *value = result;
            }
        }
    }
    pub fn announce(&mut self, text: &str, live: Live) {
        if self.on {
            self.announcements.push(Notification::Announce {
                text: text.into(),
                live,
            });
        }
    }
    pub fn node_window(&self, id: NodeId) -> Option<WindowId> {
        self.windows.iter().find_map(|(key, tree)| {
            tree.records.get(&id).filter(|r| r.emitted).map(|_| WindowId(key.0, key.1))
        })
    }

    pub fn node_area(
        &self,
        window: WindowId,
        id: NodeId,
    ) -> Option<(Area, makepad_a11y::Rect, States)> {
        let rec = self.windows.get(&key(window))?.records.get(&id)?;
        rec.emitted
            .then_some((rec.area, rec.node.bounds, rec.node.states))
    }

    /// `list_redraw` excludes freed lists, including pool slots awaiting reuse. `bounds` runs
    /// after all turtles finish, also updating cached nodes when their view transform moves.
    pub fn finish_frame(
        &mut self,
        focus: Area,
        mut list_redraw: impl FnMut(DrawListId) -> Option<u64>,
        mut bounds: impl FnMut(Area) -> Option<makepad_a11y::Rect>,
        mut emit: impl FnMut(WindowId, Vec<u8>),
    ) {
        if !self.on {
            return;
        }
        for (window, tree) in &mut self.windows {
            tree.ids.clear();
            tree.ids.extend(tree.records.keys().copied());
            for rec in tree.records.values_mut() {
                rec.reachable = false;
                let redraw = rec.list.and_then(&mut list_redraw);
                rec.alive = rec.seen == self.frame || redraw.is_some_and(|r| r == rec.redraw);
                if rec.seen != self.frame && rec.annotated != self.frame {
                    reuse_node(&mut rec.next, &rec.node);
                }
                if rec.alive {
                    if !rec.owner.is_none() {
                        // Parts carry their own bounds, but focus belongs to the owner's real Area.
                    } else if let Some(b) = bounds(rec.area) {
                        rec.next.bounds = b;
                    } else if !rec.area.is_empty() {
                        rec.next.bounds = Default::default();
                    }
                    if let Some(r) = redraw {
                        rec.redraw = r;
                    }
                    rec.next.relations.labelled_by.retain(|id| !rec.generated_labels.contains(id));
                    rec.generated_labels.clear();
                    rec.next.children.clear();
                    rec.next.states.remove(States::FOCUSED);
                }
            }
            // Repeated DrawStep entries append to drawn_children. Cached children follow them
            // in the previous order; a child reparented this frame belongs only to its new parent.
            for id in &tree.ids {
                let rec = tree.records.get_mut(id).unwrap();
                let mut old = std::mem::take(&mut rec.children);
                let mut drawn = std::mem::take(&mut rec.drawn_children);
                for child in &old {
                    if tree
                        .records
                        .get(child)
                        .is_some_and(|r| r.alive && r.parent == *id && r.seen != self.frame)
                    {
                        drawn.push(*child);
                    }
                }
                drawn.retain(|child| {
                    tree.records
                        .get(child)
                        .is_some_and(|r| r.alive && r.parent == *id)
                });
                old.clear();
                let rec = tree.records.get_mut(id).unwrap();
                rec.children = drawn;
                rec.drawn_children = old;
            }
            tree.walk.clear();
            tree.walk.push((tree.root, NodeId::NONE, 0));
            while let Some((id, parent, depth)) = tree.walk.pop() {
                let Some(rec) = tree
                    .records
                    .get_mut(&id)
                    .filter(|r| r.alive && !r.reachable)
                else {
                    continue;
                };
                rec.reachable = true;
                rec.semantic_parent = parent;
                rec.depth = depth;
                if rec.next.states.contains(States::HIDDEN) {
                    rec.reachable = false;
                    continue;
                }
                let emitted = emits(&rec.next);
                let next_parent = if emitted { id } else { parent };
                tree.walk.extend(
                    rec.children
                        .iter()
                        .rev()
                        .map(|c| (*c, next_parent, depth + 1)),
                );
                if emitted && !parent.is_none() {
                    if let Some(p) = tree.records.get_mut(&parent) {
                        p.next.children.push(id);
                    }
                }
            }
            // Resolve explicit label_for after all draws, including forward references. Labels
            // and fields have the same ids whether the consumer is an OS adapter or a remote tool.
            for id in &tree.ids {
                let Some(label) = tree.records.get(id).filter(|r| r.reachable && !r.label_for.is_none()) else { continue };
                let target = label.label_for;
                if let Some(field) = tree.records.get_mut(&target).filter(|r| r.reachable) {
                    if !field.next.relations.labelled_by.contains(id) { field.next.relations.labelled_by.push(*id); field.generated_labels.push(*id); }
                }
            }
            let next_focus = if tree.records.get(&self.requested_focus).is_some_and(|r| r.reachable && r.area == focus && emits(&r.next)) {
                self.requested_focus
            } else if focus.is_empty() {
                NodeId::NONE
            } else {
                // Several wrappers can alias a child's Area. Prefer the deepest emitted
                // owner; only consult a pruned wrapper's ancestor when no emitted owner matches.
                tree.records
                    .iter()
                    .filter(|(_, r)| r.reachable && r.area == focus && emits(&r.next) && (r.owner.is_none() || r.next.states.contains(States::SELECTED)))
                    .max_by_key(|(id, r)| (r.depth, std::cmp::Reverse(**id)))
                    .map(|(id, _)| *id)
                    .or_else(|| {
                        tree.records
                            .values()
                            .filter(|r| {
                                r.reachable && r.area == focus && !r.semantic_parent.is_none()
                            })
                            .max_by_key(|r| r.depth)
                            .map(|r| r.semantic_parent)
                    })
                    .unwrap_or(NodeId::NONE)
            };
            if let Some(rec) = tree.records.get_mut(&next_focus) {
                rec.next.states.insert(States::FOCUSED);
            }
            let update = &mut tree.update;
            update.full = tree.full;
            update.root = tree
                .records
                .get(&tree.root)
                .filter(|r| r.reachable && emits(&r.next))
                .map_or(NodeId::NONE, |_| tree.root);
            update.focus = next_focus;
            update.nodes.clear();
            update.removed.clear();
            update.notifications.clear();
            if tree.focus != next_focus {
                update
                    .notifications
                    .push(Notification::FocusChanged(next_focus));
            }
            for (id, rec) in &mut tree.records {
                let emitted = rec.reachable && emits(&rec.next);
                if emitted && (tree.full || !rec.emitted || rec.node != rec.next) {
                    if rec.emitted {
                        notifications(&rec.node, &rec.next, &mut update.notifications);
                    } else if rec.next.live != Live::Off
                        && (!rec.next.name.is_empty()
                            || !rec.next.value.is_empty()
                            || rec
                                .next
                                .text
                                .as_ref()
                                .is_some_and(|t| !t.content.is_empty()))
                    {
                        update
                            .notifications
                            .push(Notification::LiveRegionChanged(*id));
                    }
                    update.nodes.push(std::mem::take(&mut rec.next));
                } else if rec.emitted && !emitted {
                    update.removed.push(*id);
                }
                rec.emitted = emitted;
            }
            update
                .notifications
                .extend(self.announcements.iter().cloned());
            if tree.full
                || update.root != tree.sent_root
                || tree.focus != next_focus
                || !update.nodes.is_empty()
                || !update.removed.is_empty()
                || !update.notifications.is_empty()
            {
                makepad_a11y::wire::encode_update(update, &mut tree.bytes);
                emit(
                    WindowId(window.0, window.1),
                    std::mem::take(&mut tree.bytes),
                );
                tree.full = false;
                tree.sent_root = update.root;
            }
            for node in update.nodes.drain(..) {
                let id = node.id;
                tree.records.get_mut(&id).unwrap().next = node;
            }
            for rec in tree.records.values_mut() {
                std::mem::swap(&mut rec.node, &mut rec.next);
            }
            tree.records.retain(|_, r| r.reachable);
            tree.focus = next_focus;
        }
        self.announcements.clear();
    }
}
fn ancestor(tree: &WindowTree, candidate: NodeId, mut child: NodeId) -> bool {
    while !child.is_none() {
        if child == candidate {
            return true;
        }
        let Some(rec) = tree.records.get(&child) else {
            break;
        };
        child = rec.parent;
    }
    false
}

// Derived Node::clone_from would replace every String. Cached records copy into their
// spare buffers instead, so an unchanged active frame allocates nothing after warmup.
fn reuse_node(to: &mut Node, from: &Node) {
    to.id = from.id;
    to.role = from.role;
    to.states = from.states;
    to.actions = from.actions;
    to.bounds = from.bounds;
    to.range = from.range;
    to.toggle = from.toggle;
    to.level = from.level;
    to.pos_in_set = from.pos_in_set;
    to.set_size = from.set_size;
    to.live = from.live;
    to.table = from.table;
    for (to, from) in [
        (&mut to.name, &from.name),
        (&mut to.description, &from.description),
        (&mut to.value, &from.value),
        (&mut to.placeholder, &from.placeholder),
        (&mut to.key_shortcut, &from.key_shortcut),
        (&mut to.role_description, &from.role_description),
        (&mut to.class, &from.class),
        (&mut to.path, &from.path),
    ] {
        set_string(to, from);
    }
    for (tag, _) in makepad_a11y::Relations::NAMES {
        to.relations
            .get_mut(*tag)
            .unwrap()
            .clone_from(from.relations.get(*tag).unwrap());
    }
    to.custom_actions.clone_from(&from.custom_actions);
    match &from.text {
        Some(from) => {
            let to = to.text.get_or_insert_with(Default::default);
            set_string(&mut to.content, &from.content);
            to.selection = from.selection;
            to.lines.clone_from(&from.lines);
            to.glyph_x.clone_from(&from.glyph_x);
        }
        None => to.text = None,
    }
}

fn emits(n: &Node) -> bool {
    n.role != Role::Generic || !n.name.is_empty() || !n.actions.is_empty()
}
fn notifications(old: &Node, new: &Node, out: &mut Vec<Notification>) {
    let id = new.id;
    let name = old.name != new.name;
    let value = old.value != new.value || old.toggle != new.toggle || old.range != new.range;
    if name {
        out.push(Notification::NameChanged(id));
    }
    if value {
        out.push(Notification::ValueChanged(id));
    }
    if old.states != new.states {
        out.push(Notification::StatesChanged {
            node: id,
            old: old.states,
            new: new.states,
        });
    }
    if old.children != new.children {
        out.push(Notification::ChildrenChanged(id));
    }
    let a = old.text.as_ref().map_or("", |t| t.content.as_str());
    let b = new.text.as_ref().map_or("", |t| t.content.as_str());
    if a != b {
        let mut prefix = a.bytes().zip(b.bytes()).take_while(|(a, b)| a == b).count();
        while !a.is_char_boundary(prefix) || !b.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let mut suffix = a[prefix..]
            .bytes()
            .rev()
            .zip(b[prefix..].bytes().rev())
            .take_while(|(a, b)| a == b)
            .count();
        while !a.is_char_boundary(a.len() - suffix) || !b.is_char_boundary(b.len() - suffix) {
            suffix -= 1;
        }
        out.push(Notification::TextChanged {
            node: id,
            start: prefix as u32,
            removed: (a.len() - prefix - suffix) as u32,
            inserted: (b.len() - prefix - suffix) as u32,
        });
    }
    if old.text.as_ref().map(|t| t.selection) != new.text.as_ref().map(|t| t.selection) {
        out.push(Notification::CaretMoved(id));
    }
    if new.live != Live::Off && (name || value || a != b) {
        out.push(Notification::LiveRegionChanged(id));
    }
}

/// Copy only when different, retaining the destination's allocation on changes too.
pub fn set_string(to: &mut String, value: &str) {
    if to != value {
        to.clear();
        to.push_str(value);
    }
}

impl Cx {
    pub fn a11y_announce(&mut self, text: &str, live: Live) {
        if self.a11y.on {
            self.a11y.announce(text, live);
            self.redraw_all();
        }
    }
    pub(crate) fn a11y_finish_frame(&mut self) {
        if !self.a11y.on {
            return;
        }
        let mut a11y = std::mem::take(&mut self.a11y);
        let mut ops = std::mem::take(&mut self.platform_ops);
        a11y.finish_frame(
            self.keyboard.key_focus(),
            |id| {
                self.draw_lists
                    .checked_index(id)
                    .filter(|_| !self.draw_lists.is_id_freed(id))
                    .map(|l| l.redraw_id)
            },
            |area| snapshot_rect(&area, self).map(semantic_rect),
            |window, bytes| ops.push_back(CxOsOp::A11yUpdate { window, bytes }),
        );
        self.platform_ops = ops;
        self.a11y = a11y;
    }
}
pub fn semantic_rect(rect: crate::makepad_math::Rect) -> makepad_a11y::Rect {
    makepad_a11y::Rect {
        x: rect.pos.x as f32,
        y: rect.pos.y as f32,
        w: rect.size.x as f32,
        h: rect.size.y as f32,
    }
}

/// Clipped window-local bounds, using the same transform as widget snapshots.
pub fn snapshot_rect(area: &Area, cx: &Cx) -> Option<Rect> {
    if !area.is_valid(cx) {
        return None;
    }
    let rect = area.clipped_rect(cx);
    if rect.size.x <= 0.0 || rect.size.y <= 0.0 {
        return None;
    }
    let draw_list = &cx.draw_lists[area.draw_list_id()?];
    // This is the final uniform consumed by the shader. Parent transforms are
    // already assigned to children by set_view_transform, not composed here.
    let matrix = draw_list.draw_list_uniforms.view_transform;
    if matrix.v == Mat4f::identity().v {
        return Some(rect);
    }
    if !matrix.v.iter().all(|value| value.is_finite()) {
        return None;
    }
    // Preserve the existing reporting for 3D/perspective lists: projecting
    // those correctly also needs the pass camera and the widget's draw depth.
    if matrix.v[3] != 0.0
        || matrix.v[7] != 0.0
        || matrix.v[11] != 0.0
        || matrix.v[15] != 1.0
        || matrix.v[8] != 0.0
        || matrix.v[9] != 0.0
    {
        return Some(rect);
    }
    let mut min = dvec2(f64::INFINITY, f64::INFINITY);
    let mut max = dvec2(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for corner in [
        rect.pos,
        rect.pos + dvec2(rect.size.x, 0.0),
        rect.pos + dvec2(0.0, rect.size.y),
        rect.pos + rect.size,
    ] {
        let point = dvec2(
            corner.x * matrix.v[0] as f64 + corner.y * matrix.v[4] as f64 + matrix.v[12] as f64,
            corner.x * matrix.v[1] as f64 + corner.y * matrix.v[5] as f64 + matrix.v[13] as f64,
        );
        if !point.x.is_finite() || !point.y.is_finite() {
            return None;
        }
        min.x = min.x.min(point.x);
        min.y = min.y.min(point.y);
        max.x = max.x.max(point.x);
        max.y = max.y.max(point.y);
    }
    let rect = Rect {
        pos: min,
        size: max - min,
    };
    (rect.size.x > 0.0 && rect.size.y > 0.0).then_some(rect)
}

impl Cx {
    pub fn dispatch_a11y_action(&mut self, window: WindowId, request: ActionRequest) {
        #[cfg(not(target_arch = "wasm32"))]
        let _origin = crate::remote::remote_input_scope(self);
        use crate::{
            event::Event,
            studio::{RemoteMouseDown, RemoteMouseUp, StudioToApp},
        };
        let target = self.a11y.node_area(window, request.node);
        let result = match target {
            None => ActionResult::NoSuchNode,
            Some((_, _, states)) if states.intersects(States::DISABLED | States::HIDDEN) => {
                ActionResult::Disabled
            }
            Some((area, bounds, _)) => {
                if request.action == Actions::FOCUS { self.a11y.requested_focus = request.node; }
                self.a11y.pending = Some((request.req, ActionResult::Unhandled));
                self.call_event_handler(&Event::A11yAction(request.clone()));
                let handled = self
                    .a11y
                    .pending
                    .take()
                    .map_or(ActionResult::Unhandled, |(_, r)| r);
                if handled != ActionResult::Unhandled {
                    // Shader-only animations can update pixels without recording widgets.
                    // Record the target's list once so its semantic state changes as well.
                    if matches!(handled, ActionResult::Handled | ActionResult::Fallback) {
                        self.redraw_area(area);
                    }
                    handled
                } else if request.action == Actions::FOCUS {
                    if area.is_valid(self) {
                        self.a11y.requested_focus = request.node;
                        self.set_key_focus(area);
                        self.inner_key_focus_change();
                        self.redraw_all();
                        ActionResult::Fallback
                    } else {
                        ActionResult::Unsupported
                    }
                } else if matches!(
                    request.action,
                    Actions::PRESS | Actions::TOGGLE | Actions::SELECT
                ) && !bounds.is_empty()
                {
                    let (x, y) = bounds.center();
                    let time = self.seconds_since_app_start();
                    self.dispatch_studio_msg(
                        StudioToApp::MouseDown(RemoteMouseDown {
                            time,
                            x: x as f64,
                            y: y as f64,
                            button_raw_bits: 1,
                            modifiers: Default::default(),
                        }),
                        window,
                        dvec2(0.0, 0.0),
                    );
                    self.dispatch_studio_msg(
                        StudioToApp::MouseUp(RemoteMouseUp {
                            time: time + 0.001,
                            x: x as f64,
                            y: y as f64,
                            button_raw_bits: 1,
                            modifiers: Default::default(),
                        }),
                        window,
                        dvec2(0.0, 0.0),
                    );
                    ActionResult::Fallback
                } else {
                    ActionResult::Unsupported
                }
            }
        };
        self.platform_ops.push_back(CxOsOp::A11yActionDone {
            req: request.req,
            result: result as u8,
        });
    }

    /// `ui type`'s fallback for a focused widget without REPLACE_TEXT. Preserve the
    /// normal text-input handler while marking this input as injected.
    pub(crate) fn a11y_type_fallback(&mut self, text: String) {
        #[cfg(not(target_arch = "wasm32"))]
        let _origin = crate::remote::remote_input_scope(self);
        self.call_event_handler(&crate::event::Event::TextInput(crate::event::TextInputEvent {
            input: text,
            ..Default::default()
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{area::RectArea, draw_list::CxDrawListPool};
    use makepad_a11y::{mirror::Tree, TextInfo};

    fn node(role: Role, name: &str) -> Node {
        Node {
            role,
            name: name.into(),
            ..Default::default()
        }
    }
    fn root(b: &mut CxA11y, window: WindowId, uid: u64, list: DrawListId) -> u32 {
        let token = b.begin_node(uid, None);
        b.begin_window(window);
        b.window_draw_list(list);
        token
    }
    fn end_root(b: &mut CxA11y, token: u32) {
        b.end_window();
        b.end_node(token, node(Role::Window, "Test"), Area::Empty);
    }
    fn finish(
        b: &mut CxA11y,
        focus: Area,
        redraw: impl FnMut(DrawListId) -> Option<u64>,
    ) -> Vec<TreeUpdate> {
        let mut messages = Vec::new();
        b.finish_frame(
            focus,
            redraw,
            |_| None,
            |w, bytes| messages.push((w, bytes)),
        );
        messages
            .into_iter()
            .map(|(w, bytes)| {
                let update = makepad_a11y::wire::decode_update(&bytes).unwrap();
                b.recycle_bytes(w, bytes);
                update
            })
            .collect()
    }
    fn active() -> CxA11y {
        let mut b = CxA11y::default();
        assert!(b.set_consumers(4));
        b
    }

    #[test]
    fn annotating_cached_panels_preserves_links_without_reviving_replaced_content() {
        let mut pool = CxDrawListPool::default();
        let main = pool.alloc();
        let content = pool.alloc();
        let mut builder = active();
        let mut tree = Tree::new();
        for frame in 1..=3 {
            builder.begin_frame();
            let root = root(&mut builder, WindowId(0, 1), 1, main.id());
            if frame != 2 {
                let uid = if frame == 1 { 2 } else { 4 };
                let panel = builder.begin_node(uid, Some(content.id()));
                let button = builder.begin_node(uid + 1, Some(content.id()));
                builder.end_node(button, node(Role::Button, "Panel control"), Area::Empty);
                builder.end_node(panel, node(Role::Group, "Content"), Area::Empty);
            }
            // Dock annotates bodies whose areas still exist, including the cached
            // selected body and the previous body after its list was replaced.
            for uid in [2, 4] {
                builder.set_part(root, uid, None, Node {
                    id: NodeId::widget(uid),
                    role: Role::TabPanel,
                    name: "Panel".into(),
                    ..Default::default()
                }, || unreachable!("existing widget"));
            }
            end_root(&mut builder, root);
            for update in finish(&mut builder, Area::Empty, |list| {
                Some(if list == main.id() { frame } else if frame == 3 { 3 } else { 1 })
            }) {
                tree.apply(update);
            }
            let panel = NodeId::widget(if frame == 3 { 4 } else { 2 });
            assert_eq!(tree.node(NodeId::widget(1)).unwrap().children, [panel]);
            assert_eq!(tree.node(panel).unwrap().role, Role::TabPanel);
            assert_eq!(tree.node(panel).unwrap().children, [NodeId::widget(panel.uid() + 1)]);
            assert_eq!(tree.nodes.len(), 3);
        }
    }

    #[test]
    fn hiding_a_cached_popup_prunes_its_cached_children() {
        let mut pool = CxDrawListPool::default();
        let main = pool.alloc();
        let popup = pool.alloc();
        let mut builder = active();
        builder.begin_frame();
        let root = root(&mut builder, WindowId(0, 1), 1, main.id());
        let menu = builder.begin_node(2, Some(popup.id()));
        let item = builder.begin_node(3, Some(popup.id()));
        builder.end_node(item, node(Role::MenuItem, "Open"), Area::Empty);
        builder.end_node(menu, node(Role::Menu, "Menu"), Area::Empty);
        end_root(&mut builder, root);
        let mut tree = Tree::new();
        tree.apply(finish(&mut builder, Area::Empty, |_| Some(1)).pop().unwrap());
        assert_eq!(tree.nodes.len(), 3);

        builder.begin_frame();
        let root = self::root(&mut builder, WindowId(0, 1), 1, main.id());
        builder.set_part(root, 0, None, Node {
            id: NodeId::widget(2),
            role: Role::Menu,
            states: States::HIDDEN,
            ..Default::default()
        }, || unreachable!("an existing widget needs no part id"));
        end_root(&mut builder, root);
        tree.apply(finish(&mut builder, Area::Empty, |list| {
            Some(if list == main.id() { 2 } else { 1 })
        }).pop().unwrap());
        assert_eq!(tree.nodes.len(), 1);
        assert!(tree.node(NodeId::widget(1)).unwrap().children.is_empty());
    }

    #[test]
    fn pruning_focus_and_reactivation() {
        let mut pool = CxDrawListPool::default();
        let list = pool.alloc();
        let area = Area::Rect(RectArea {
            draw_list_id: list.id(),
            rect_id: 0,
            redraw_id: 1,
        });
        let mut b = active();
        b.begin_frame();
        let r = root(&mut b, WindowId(0, 1), 1, list.id());
        let g = b.begin_node(2, Some(list.id()));
        let c = b.begin_node(3, Some(list.id()));
        b.end_node(c, node(Role::Button, "Save"), Area::Empty);
        b.end_node(g, Node::default(), area);
        end_root(&mut b, r);
        let u = finish(&mut b, area, |_| Some(1)).pop().unwrap();
        assert!(u.full);
        assert_eq!(
            u.focus,
            NodeId(1),
            "pruned focus owner maps to nearest emitted ancestor"
        );
        assert_eq!(u.nodes.len(), 2);
        assert_eq!(
            u.nodes.iter().find(|n| n.id == NodeId(1)).unwrap().children,
            [NodeId(3)]
        );
        assert!(u
            .notifications
            .contains(&Notification::FocusChanged(NodeId(1))));
        assert!(!b.set_consumers(6));
        b.begin_frame();
        assert!(
            finish(&mut b, area, |_| Some(1)).is_empty(),
            "cached unchanged tree sends nothing"
        );
        assert!(!b.set_consumers(0));
        assert!(b.windows.is_empty() && b.stack.capacity() == 0);
        assert!(b.set_consumers(2));
        b.begin_frame();
        let r = root(&mut b, WindowId(0, 1), 1, list.id());
        end_root(&mut b, r);
        assert!(finish(&mut b, Area::Empty, |_| Some(2))[0].full);
    }

    #[test]
    fn yielded_content_keeps_its_parent_after_the_window_returns_a_step() {
        let mut pool = CxDrawListPool::default();
        let list = pool.alloc();
        let mut b = active();
        b.begin_frame();
        let outer = b.begin_node(9, None);
        let r = root(&mut b, WindowId(0, 1), 1, list.id());
        let p = b.begin_node(2, Some(list.id()));
        b.suspend_node(p);
        b.end_node(p, node(Role::List, "Rows"), Area::Empty);
        end_root(&mut b, r);
        b.end_node(outer, Node::default(), Area::Empty);
        // The caller fills the yielded list outside the Window's Rust stack frame.
        let row = b.begin_node(3, Some(list.id()));
        let child = b.begin_node(4, Some(list.id()));
        b.end_node(child, node(Role::Button, "Row action"), Area::Empty);
        b.end_node(row, node(Role::ListItem, "Row"), Area::Empty);
        let outer = b.begin_node(9, None);
        assert_eq!(b.window_root(outer), 0);
        let r = root(&mut b, WindowId(0, 1), 1, list.id());
        let p = b.begin_node(2, Some(list.id()));
        b.end_node(p, node(Role::List, "Rows"), Area::Empty);
        end_root(&mut b, r);
        b.end_node(outer, Node::default(), Area::Empty);
        let mut tree = Tree::new();
        tree.apply(finish(&mut b, Area::Empty, |_| Some(1)).pop().unwrap());
        assert_eq!(tree.node(NodeId(1)).unwrap().children, [NodeId(2)]);
        assert_eq!(tree.node(NodeId(2)).unwrap().children, [NodeId(3)]);
        assert_eq!(tree.node(NodeId(3)).unwrap().children, [NodeId(4)]);
        assert!(b.suspended.is_empty());
    }

    #[test]
    fn focus_prefers_emitted_owner_over_pruned_alias() {
        let mut pool = CxDrawListPool::default();
        let list = pool.alloc();
        let area = Area::Rect(RectArea {
            draw_list_id: list.id(),
            rect_id: 0,
            redraw_id: 1,
        });
        let mut b = active();
        b.begin_frame();
        let r = root(&mut b, WindowId(0, 1), 1, list.id());
        let wrapper = b.begin_node(2, Some(list.id()));
        let input = b.begin_node(3, Some(list.id()));
        b.end_node(input, node(Role::TextField, "Edit"), area);
        b.end_node(wrapper, Node::default(), area);
        end_root(&mut b, r);
        assert_eq!(finish(&mut b, area, |_| Some(1))[0].focus, NodeId(3));
    }

    #[test]
    fn cached_segments_append_in_order_then_redrawn_and_freed_segments_disappear() {
        let mut pool = CxDrawListPool::default();
        let main = pool.alloc();
        let cached = pool.alloc();
        let mut b = active();
        b.begin_frame();
        let r = root(&mut b, WindowId(0, 1), 1, main.id());
        for uid in [2, 3] {
            let t = b.begin_node(uid, Some(cached.id()));
            b.end_node(t, node(Role::Button, "Cached"), Area::Empty);
        }
        end_root(&mut b, r);
        let mut mirror = Tree::new();
        mirror.apply(finish(&mut b, Area::Empty, |_| Some(1)).pop().unwrap());
        b.begin_frame();
        let r = root(&mut b, WindowId(0, 1), 1, main.id());
        let t = b.begin_node(4, Some(main.id()));
        b.end_node(t, node(Role::StaticText, "New"), Area::Empty);
        end_root(&mut b, r);
        let u = finish(&mut b, Area::Empty, |id| {
            Some(if id == main.id() { 2 } else { 1 })
        })
        .pop()
        .unwrap();
        assert!(u.removed.is_empty());
        mirror.apply(u);
        assert_eq!(
            mirror.node(NodeId(1)).unwrap().children,
            [NodeId(4), NodeId(2), NodeId(3)]
        );
        // Only the cached list is redrawn; its omitted node 3 goes away.
        b.begin_frame();
        let r = root(&mut b, WindowId(0, 1), 1, main.id());
        let t = b.begin_node(2, Some(cached.id()));
        b.end_node(t, node(Role::Button, "Changed"), Area::Empty);
        end_root(&mut b, r);
        let u = finish(&mut b, Area::Empty, |_| Some(2)).pop().unwrap();
        assert_eq!(u.removed, [NodeId(3)]);
        mirror.apply(u);
        assert_eq!(
            mirror.node(NodeId(1)).unwrap().children,
            [NodeId(2), NodeId(4)]
        );
        b.begin_frame();
        let u = finish(&mut b, Area::Empty, |id| (id == main.id()).then_some(2))
            .pop()
            .unwrap();
        assert_eq!(u.removed, [NodeId(2)]);
        mirror.apply(u);
        assert_eq!(mirror.node(NodeId(1)).unwrap().children, [NodeId(4)]);
    }

    #[test]
    fn resumed_draws_merge_children_without_duplicates_and_windows_are_isolated() {
        let mut pool = CxDrawListPool::default();
        let list = pool.alloc();
        let mut b = active();
        b.begin_frame();
        for child in [2, 3, 2] {
            let r = root(&mut b, WindowId(0, 1), 1, list.id());
            let t = b.begin_node(child, Some(list.id()));
            b.end_node(t, node(Role::Button, "Step"), Area::Empty);
            end_root(&mut b, r);
        }
        let r = root(&mut b, WindowId(1, 1), 4, list.id());
        end_root(&mut b, r);
        let updates = finish(&mut b, Area::Empty, |_| Some(1));
        assert_eq!(updates.len(), 2);
        let first = updates.iter().find(|u| u.root == NodeId(1)).unwrap();
        assert_eq!(
            first
                .nodes
                .iter()
                .find(|n| n.id == NodeId(1))
                .unwrap()
                .children,
            [NodeId(2), NodeId(3)]
        );
        assert_eq!(
            updates
                .iter()
                .find(|u| u.root == NodeId(4))
                .unwrap()
                .nodes
                .len(),
            1
        );
        b.close_window(WindowId(0, 1));
        assert_eq!(b.windows.len(), 1);
    }

    #[test]
    fn precise_diff_notifications_utf8_caret_live_and_focus() {
        let mut pool = CxDrawListPool::default();
        let list = pool.alloc();
        let area = Area::Rect(RectArea {
            draw_list_id: list.id(),
            rect_id: 0,
            redraw_id: 1,
        });
        let mut b = active();
        for (i, content) in ["aéz", "aê😀z"].iter().enumerate() {
            b.begin_frame();
            let r = root(&mut b, WindowId(0, 1), 1, list.id());
            let t = b.begin_node(2, Some(list.id()));
            let mut n = node(Role::TextField, if i == 0 { "Old" } else { "New" });
            n.live = Live::Polite;
            n.value = content.to_string();
            n.states = States::FOCUSABLE;
            n.text = Some(TextInfo {
                content: content.to_string(),
                selection: (content.len() as u32, content.len() as u32),
                ..Default::default()
            });
            b.end_node(t, n, area);
            end_root(&mut b, r);
            if i == 1 {
                b.announce("Saved", Live::Assertive);
            }
            let u = finish(&mut b, if i == 0 { Area::Empty } else { area }, |_| {
                Some(i as u64 + 1)
            })
            .pop()
            .unwrap();
            if i == 1 {
                assert!(!u.full);
                assert_eq!(u.nodes.len(), 1);
                assert_eq!(u.focus, NodeId(2));
                for expected in [
                    Notification::NameChanged(NodeId(2)),
                    Notification::ValueChanged(NodeId(2)),
                    Notification::CaretMoved(NodeId(2)),
                    Notification::LiveRegionChanged(NodeId(2)),
                    Notification::FocusChanged(NodeId(2)),
                    Notification::TextChanged {
                        node: NodeId(2),
                        start: 1,
                        removed: 2,
                        inserted: 6,
                    },
                    Notification::Announce {
                        text: "Saved".into(),
                        live: Live::Assertive,
                    },
                ] {
                    assert!(
                        u.notifications.contains(&expected),
                        "missing {expected:?}: {:?}",
                        u.notifications
                    );
                }
                assert!(u.notifications.iter().any(|n| matches!(
                    n,
                    Notification::StatesChanged {
                        node: NodeId(2),
                        ..
                    }
                )));
            }
        }
        b.begin_frame();
        let mut updates = Vec::new();
        b.finish_frame(
            area,
            |_| Some(2),
            |_| {
                Some(makepad_a11y::Rect {
                    x: 42.0,
                    y: 0.0,
                    w: 10.0,
                    h: 10.0,
                })
            },
            |_, bytes| updates.push(makepad_a11y::wire::decode_update(&bytes).unwrap()),
        );
        assert_eq!(
            updates[0].nodes[0].bounds.x, 42.0,
            "retained areas follow transforms without a redraw"
        );
        assert!(updates[0].notifications.is_empty());
    }

    #[test]
    fn actions_claim_focus_tap_reject_disabled_and_report_unknown_nodes() {
        use crate::{
            draw_list::{CxRectArea, DrawList},
            event::Event,
            window::WindowHandle,
        };
        use makepad_a11y::ActionArg;
        use std::{cell::RefCell, rc::Rc};
        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = events.clone();
        let mut cx = Cx::new(Box::new(move |cx, event| match event {
            Event::A11yAction(req) => {
                observed.borrow_mut().push("action");
                if req.action == Actions::SET_VALUE {
                    cx.a11y.record_action(req.req, ActionResult::Handled);
                }
            }
            Event::MouseDown(_) => observed.borrow_mut().push("down"),
            Event::MouseUp(_) => observed.borrow_mut().push("up"),
            _ => {}
        }));
        let window = WindowHandle::new(&mut cx);
        let wid = window.window_id();
        let list = DrawList::new(&mut cx);
        cx.draw_lists[list.id()].redraw_id = 1;
        cx.draw_lists[list.id()].rect_areas.push(CxRectArea {
            rect: Rect {
                pos: dvec2(10.0, 20.0),
                size: dvec2(80.0, 24.0),
            },
            draw_clip: (dvec2(0.0, 0.0), dvec2(100.0, 100.0)),
        });
        let area = Area::Rect(RectArea {
            draw_list_id: list.id(),
            rect_id: 0,
            redraw_id: 1,
        });
        cx.a11y.set_consumers(4);
        cx.a11y.begin_frame();
        let r = root(&mut cx.a11y, wid, 1, list.id());
        let t = cx.a11y.begin_node(2, Some(list.id()));
        let mut n = node(Role::Button, "Save");
        n.bounds = makepad_a11y::Rect {
            x: 10.0,
            y: 20.0,
            w: 80.0,
            h: 24.0,
        };
        cx.a11y.end_node(t, n, area);
        end_root(&mut cx.a11y, r);
        cx.a11y_finish_frame();
        for (action, node, expected) in [
            (Actions::FOCUS, NodeId(2), ActionResult::Fallback),
            (Actions::PRESS, NodeId(2), ActionResult::Fallback),
            (Actions::SET_VALUE, NodeId(2), ActionResult::Handled),
            (Actions::INCREMENT, NodeId(2), ActionResult::Unsupported),
            (Actions::PRESS, NodeId(99), ActionResult::NoSuchNode),
        ] {
            cx.dispatch_a11y_action(
                wid,
                ActionRequest {
                    req: 17,
                    node,
                    action,
                    arg: ActionArg::None,
                },
            );
            let Some(CxOsOp::A11yActionDone { req, result }) = cx.platform_ops.pop_back() else {
                panic!("missing completion")
            };
            assert_eq!(req, 17);
            assert_eq!(result, expected as u8, "action {action:?}");
        }
        assert_eq!(cx.keyboard.key_focus(), area);
        assert_eq!(
            events.borrow().as_slice(),
            ["action", "action", "down", "up", "action", "action"]
        );
        cx.a11y
            .windows
            .get_mut(&key(wid))
            .unwrap()
            .records
            .get_mut(&NodeId(2))
            .unwrap()
            .node
            .states
            .insert(States::DISABLED);
        events.borrow_mut().clear();
        cx.dispatch_a11y_action(
            wid,
            ActionRequest {
                req: 18,
                node: NodeId(2),
                action: Actions::PRESS,
                arg: ActionArg::None,
            },
        );
        assert!(events.borrow().is_empty());
        assert_eq!(
            cx.platform_ops.pop_back(),
            Some(CxOsOp::A11yActionDone {
                req: 18,
                result: ActionResult::Disabled as u8
            })
        );
    }

    #[test]
    fn off_does_no_builder_work() {
        let mut b = CxA11y::default();
        for _ in 0..100_000 {
            if std::hint::black_box(b.on) {
                b.begin_frame();
            }
            b.finish_frame(
                Area::Empty,
                |_| panic!("list lookup while off"),
                |_| panic!("bounds lookup while off"),
                |_, _| panic!("update while off"),
            );
        }
        assert_eq!(b.frame, 0);
        assert_eq!(b.windows.capacity(), 0);
        assert_eq!(b.stack.capacity(), 0);
        assert_eq!(b.window_stack.capacity(), 0);
        assert_eq!(b.announcements.capacity(), 0);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod allocation_tests {
    use super::*;
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
    };
    thread_local! {
        static COUNT: Cell<Option<usize>> = const { Cell::new(None) };
    }
    struct CountAllocator;
    fn note() {
        COUNT.with(|c| {
            if let Some(n) = c.get() {
                c.set(Some(n + 1));
            }
        });
    }
    unsafe impl GlobalAlloc for CountAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            note();
            System.alloc(layout)
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            note();
            System.alloc_zeroed(layout)
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            note();
            System.realloc(ptr, layout, size)
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            System.dealloc(ptr, layout)
        }
    }
    #[global_allocator]
    static ALLOCATOR: CountAllocator = CountAllocator;

    fn measured(f: impl FnOnce()) -> usize {
        COUNT.with(|c| c.set(Some(0)));
        f();
        COUNT.with(|c| c.replace(None).unwrap())
    }
    #[test]
    fn off_and_unchanged_active_frames_allocate_nothing() {
        let mut builder = CxA11y::default();
        let off = measured(|| {
            for _ in 0..100_000 {
                if std::hint::black_box(builder.on) {
                    builder.begin_frame();
                }
                builder.finish_frame(
                    Area::Empty,
                    |_| unreachable!(),
                    |_| unreachable!(),
                    |_, _| unreachable!(),
                );
            }
        });
        assert_eq!(off, 0);
        builder.set_consumers(makepad_a11y::consumer::TEST);
        let mut pool = crate::draw_list::CxDrawListPool::default();
        let list = pool.alloc();
        let frame = |builder: &mut CxA11y| {
            builder.begin_frame();
            let outer = builder.begin_node(9, None);
            let token = builder.begin_node(1, None);
            builder.begin_window(WindowId(0, 1));
            builder.window_draw_list(list.id());
            let child = builder.begin_node(2, Some(list.id()));
            let mut node = builder.take_node(child);
            node.role = Role::Button;
            set_string(&mut node.name, "Save");
            builder.end_node(child, node, Area::Empty);
            let mut node = builder.take_node(token);
            node.role = Role::Window;
            set_string(&mut node.name, "Test");
            builder.end_window();
            builder.end_node(token, node, Area::Empty);
            builder.end_node(outer, Node::default(), Area::Empty);
            let mut payload = None;
            builder.finish_frame(
                Area::Empty,
                |_| Some(1),
                |_| None,
                |w, bytes| payload = Some((w, bytes)),
            );
            if let Some((w, bytes)) = payload {
                builder.recycle_bytes(w, bytes);
            }
        };
        for _ in 0..4 {
            frame(&mut builder);
        }
        let active = measured(|| {
            for _ in 0..1_000 {
                frame(&mut builder);
            }
        });
        assert_eq!(active, 0);
        let cached = measured(|| {
            for _ in 0..1_000 {
                builder.begin_frame();
                builder.finish_frame(Area::Empty, |_| Some(1), |_| None, |_, _| unreachable!());
            }
        });
        assert_eq!(cached, 0);
        eprintln!(
            "a11y allocation probe: off=0/100000 frames; active unchanged=0/1000; cached=0/1000"
        );
    }
}
