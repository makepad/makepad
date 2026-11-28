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
    query::Query,
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
    /// The lookup frame that supplied this part; zero for ordinary draw-time parts.
    looked_up: u32,
    /// A part drawn as an area of its own (a list's row): its bounds, and whether it is
    /// offscreen, are that area's once the frame's turtles finish, as a widget's are. Empty:
    /// the part carries its own.
    drawn: Area,
    label_for: NodeId,
    generated_labels: Vec<NodeId>,
    /// The node's name is its tip's text (a nameless control in a tooltip).
    named_by_tip: bool,
    parts: HashMap<u64, NodeId>,
    drawn_parts: Vec<NodeId>,
    /// Where the node's area stood when its text geometry (absolute line boxes and glyph
    /// positions) was measured; see [`CxA11y::set_text_origin`].
    text_at: Option<[f32; 2]>,
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
    /// (control, tip) pairs the frame's tips may name (scratch, reused).
    tipped: Vec<(NodeId, NodeId)>,
    update: TreeUpdate,
    bytes: Vec<u8>,
}
#[derive(Clone, Copy)]
struct Frame {
    id: NodeId,
    window: Option<WindowKey>,
    list: Option<DrawListId>,
}

/// Open semantic scopes and the parents to which a nested draw can attach children.
pub struct A11yUnwindMark {
    nodes: usize,
    windows: usize,
    parent: Option<Frame>,
    outside_changes: usize,
    suspended: Vec<Frame>,
    attachments: [Option<(WindowKey, NodeId, usize)>; 2],
}

/// Where a lookup request originated, and the number its acknowledgement must echo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookupSource {
    /// A sequence from the app's native HAL semantic endpoint.
    Native(u32),
    /// A request number from the host's semantic endpoint.
    Host(u64),
}

#[derive(Default)]
pub struct CxA11y {
    pub on: bool,
    consumers: u32,
    /// A virtual-clock lookup keeps the builder on until the next draw finishes.
    build_on_step: bool,
    frame: u64,
    windows: HashMap<WindowKey, WindowTree>,
    stack: Vec<Frame>,
    suspended: Vec<Frame>,
    outside_windows: HashSet<NodeId>,
    // Nodes can first enter outside a window and then open their own window.
    // Remember membership changes so nested draws can restore that distinction.
    outside_changes: Vec<(NodeId, bool)>,
    // The previous root is needed if a window draw unwinds before end_window.
    window_stack: Vec<(WindowKey, NodeId)>,
    announcements: Vec<Notification>,
    pending: Option<(u64, ActionResult)>,
    requested_focus: NodeId,
    lookups: Vec<(LookupSource, Query, Option<(NodeId, u64)>)>,
    lookup_frame: u32,
    lookup_done: u32,
    host_lookups_done: Vec<u64>,
    snapshot: u32,
    host_snapshots: Vec<u64>,
    sticky: u32,
    /// Nodes whose path the widget layer could not name while they drew (`path_unknown`).
    paths_unknown: Vec<(WindowKey, NodeId)>,
    /// A path joined `paths_unknown` in the frame building now (`path_unknown`).
    paths_added: bool,
    /// No path joined `paths_unknown` in the last frame that built: each one there was asked
    /// again after a later frame than the one that found it, and stayed unnamed. A frame of an
    /// app that did not change names none of them ([`CxA11y::current`]).
    paths_settled: bool,
    /// The last frame that built the tree and covered the events before it (`frame_covers`): the
    /// app's event count once it drew (`Cx::event_id`) and the key focus it built with
    /// ([`Cx::a11y_tree_current`]). None before the first.
    built: Option<(u64, Area)>,
    /// The frame building now covers the events before it, so it may mark the tree built
    /// ([`Cx::a11y_begin_frame`]).
    frame_covers: bool,
}

/// Names a widget node's path (`Node::path`) after the frame drew, for nodes whose path the
/// widget layer could not name during the draw: `(cx, widget uid, its window's root uid)`.
static PATH_RESOLVER: std::sync::OnceLock<fn(&Cx, u64, u64) -> Option<String>> = std::sync::OnceLock::new();

/// The widget layer's path resolver (`PATH_RESOLVER`); the first one set stays.
pub fn set_path_resolver(resolve: fn(&Cx, u64, u64) -> Option<String>) {
    let _ = PATH_RESOLVER.set(resolve);
}

impl CxA11y {
    pub fn unwind_mark(&self) -> A11yUnwindMark {
        let parent = self.stack.last().copied();
        let attachments = [parent, self.suspended.last().copied()].map(|frame| {
            let frame = frame?;
            let window = frame.window?;
            let record = self.windows.get(&window)?.records.get(&frame.id)?;
            Some((window, frame.id, record.drawn_children.len()))
        });
        A11yUnwindMark {
            nodes: self.stack.len(),
            windows: self.window_stack.len(),
            parent,
            outside_changes: self.outside_changes.len(),
            // Re-entering a yielded ancestor can consume an existing suspension.
            suspended: self.suspended.clone(),
            attachments,
        }
    }

    /// Discard unfinished scopes and their attachments. A yielded node has already
    /// closed its token, so restoring the suspended context never ends a node.
    pub fn unwind_to(&mut self, mark: A11yUnwindMark) {
        self.stack.truncate(mark.nodes);
        while self.window_stack.len() > mark.windows {
            let (window, root) = self.window_stack.pop().unwrap();
            self.windows.get_mut(&window).unwrap().root = root;
        }
        if let (Some(top), Some(parent)) = (self.stack.last_mut(), mark.parent) {
            if top.id == parent.id {
                *top = parent;
            }
        }
        self.suspended = mark.suspended;
        while self.outside_changes.len() > mark.outside_changes {
            let (id, was_outside) = self.outside_changes.pop().unwrap();
            if was_outside {
                self.outside_windows.insert(id);
            } else {
                self.outside_windows.remove(&id);
            }
        }
        for (window, parent, count) in mark.attachments.into_iter().flatten() {
            let tree = self.windows.get_mut(&window).unwrap();
            let children = &mut tree.records.get_mut(&parent).unwrap().drawn_children;
            while children.len() > count {
                tree.links.remove(&(parent, children.pop().unwrap()));
            }
        }
    }

    /// The consumer bits (`makepad_a11y::consumer`) currently held.
    pub fn consumers(&self) -> u32 {
        self.consumers
    }
    /// Returns true on activation; the owning Cx must then redraw every window.
    /// Off keeps the records so ids from the last snapshot still resolve through
    /// `action_owner`, `action_part_key` and `request_lookup` id terms and act; only building stops.
    pub fn set_consumers(&mut self, consumers: u32) -> bool {
        let activate = !self.on && consumers != 0;
        self.consumers = consumers;
        self.on = consumers != 0 || self.build_on_step;
        if !self.on {
            // Host waiters end through their own timeout or disconnect when building stops.
            self.lookups.clear();
            self.host_snapshots.clear();
            self.snapshot = 0;
            self.stack.clear();
            self.suspended.clear();
            self.outside_windows.clear();
            self.outside_changes.clear();
            self.window_stack.clear();
            self.announcements.clear();
            return false;
        }
        if activate {
            for w in self.windows.values_mut() {
                w.full = true;
            }
        }
        activate
    }
    /// Back on for `consumers` (not zero) over the records the last built frame left, while they
    /// are still the app's ([`Cx::a11y_tree_current`]): no window is rebuilt in full and no frame
    /// is owed; the next frame that draws builds on them as a builder that stayed on does.
    pub fn resume(&mut self, consumers: u32) {
        debug_assert!(consumers != 0, "resume needs a consumer");
        self.consumers = consumers;
        self.on = true;
    }
    /// The tree the last built frame published is still the app's: since that frame (its event
    /// count `event_id` once it drew, and the key focus `focus` it built with) no event reached
    /// the app, which may change what the tree shows without a redraw; no redraw waits; and
    /// nothing the builder holds waits for a frame (a lookup, a snapshot, an action's result, an
    /// announcement, a path still unnamed that no later frame has asked again).
    pub fn current(&self, event_id: u64, focus: Area, redraw_pending: bool) -> bool {
        self.built == Some((event_id, focus))
            && !redraw_pending
            && self.lookups.is_empty()
            && self.snapshot == 0
            && self.host_snapshots.is_empty()
            && self.pending.is_none()
            && self.announcements.is_empty()
            && (self.paths_unknown.is_empty() || self.paths_settled)
    }
    /// Requests one lookup frame while consumers are active; an empty query requests only
    /// a snapshot acknowledgement. A true result requires the caller to redraw every window.
    /// Invalid queries are logged and leave pending work intact.
    pub fn request_lookup(&mut self, source: LookupSource, query: &str) -> bool {
        if !self.on || source == LookupSource::Native(0) {
            return false;
        }
        if query.trim().is_empty() {
            match source {
                LookupSource::Native(seq) => self.snapshot = self.snapshot.max(seq),
                LookupSource::Host(req) => self.host_snapshots.push(req),
            }
            return true;
        }
        let query = match Query::parse(query) {
            Ok(query) => query,
            Err(error) => { crate::error!("semantic lookup: {error}"); return false; }
        };
        if !query.is_lookup() {
            return false;
        }
        let owner = query.id_term().and_then(|id| {
            if let Some(key) = self.action_part_key(id) {
                return Some((self.action_owner(id), key));
            }
            // Part records expire, but their owner's stable key-to-id table does not.
            // Packed ids name that owner directly; overflow ids need a table search.
            if id.part_index() != 0 {
                let owner = NodeId::widget(id.uid());
                self.windows.values().find_map(|tree| {
                    tree.records.get(&owner)?.parts.iter()
                        .find_map(|(key, part)| (*part == id).then_some((owner, *key)))
                })
            } else {
                self.windows.values().find_map(|tree| {
                    tree.records.iter().find_map(|(owner, rec)| {
                        rec.parts.iter().find_map(|(key, part)| (*part == id).then_some((*owner, *key)))
                    })
                })
            }
        });
        if self.lookups.is_empty() {
            self.lookup_frame = self.lookup_frame.wrapping_add(1).max(1);
        }
        self.lookups.push((source, query, owner));
        true
    }
    /// Takes the completed lookup sequence once; zero means no acknowledgement is pending.
    pub fn take_lookup_done(&mut self) -> u32 {
        std::mem::take(&mut self.lookup_done)
    }
    /// Takes every completed host request once.
    pub fn take_host_lookups_done(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.host_lookups_done)
    }
    /// The lookups for the node being built: `(query, None)` asks for matching terms;
    /// `(query, Some(key))` asks only its resolved owner to refresh that stable part key.
    /// With no pending lookup the iterator makes a single emptiness check.
    pub fn lookups_for(&self, token: u32) -> impl Iterator<Item = (&Query, Option<u64>)> {
        self.lookups.iter().filter_map(move |(_, query, owner)| match owner {
            Some((owner, key)) => (self.stack[token as usize].id == *owner).then_some((query, Some(*key))),
            None => Some((query, None)),
        })
    }
    pub(crate) fn request_full(&mut self) {
        for w in self.windows.values_mut() { w.full = true; }
    }
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        self.frame_covers = true;
        self.stack.clear();
        self.suspended.clear();
        self.outside_windows.clear();
        self.outside_changes.clear();
        self.window_stack.clear();
        for tree in self.windows.values_mut() {
            tree.links.clear();
        }
    }
    pub fn begin_window(&mut self, window: WindowId) {
        let key = key(window);
        let tree = self.windows.entry(key).or_insert_with(|| WindowTree {
            full: true,
            ..Default::default()
        });
        self.window_stack.push((key, tree.root));
        if let Some(frame) = self.stack.last_mut() {
            frame.window = Some(key);
            if self.outside_windows.remove(&frame.id) {
                self.outside_changes.push((frame.id, true));
            }
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
            rec.text_at = None;
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
        let window = self.window_stack.last().map(|&(window, _)| window).or_else(|| {
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
        if window.is_none() && self.outside_windows.insert(id) {
            self.outside_changes.push((id, false));
        }
        let token = self.stack.len() as u32;
        self.stack.push(Frame { id, window, list });
        token
    }
    /// Where the node's area stands as its semantics measure its text. A widget's semantics run
    /// right after it draws, before the parents around it align their children, so its line
    /// boxes and glyph positions (absolute, unlike its bounds, which come from its area once
    /// every turtle has finished) would keep the place it was drawn at; `finish_frame` moves
    /// them as far as the area has moved since.
    pub fn set_text_origin(&mut self, token: u32, at: [f32; 2]) {
        let f = self.stack[token as usize];
        if let Some(rec) = f.window.and_then(|w| self.windows.get_mut(&w)).and_then(|t| t.records.get_mut(&f.id)) {
            rec.text_at = Some(at);
        }
    }
    /// Reuse the previous draw's strings and vectors. Semantics implementations overwrite the
    /// fields they own and compare strings before copying; class/path are computed only once.
    pub fn take_node(&mut self, token: u32) -> Node {
        let f = self.stack[token as usize];
        f.window
            .and_then(|w| self.windows.get_mut(&w))
            .and_then(|w| w.records.get_mut(&f.id))
            .map(|r| {
                let mut node = std::mem::take(&mut r.next);
                // The widget names its node itself; a tip's name is given again after the frame.
                if r.named_by_tip {
                    node.name.clear();
                }
                node
            })
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
        let text_at = rec.text_at;
        let mut parts = std::mem::take(&mut rec.drawn_parts);
        let tree = self.windows.get_mut(&w).unwrap();
        for id in &parts {
            if let Some(part) = tree.records.get_mut(id) {
                part.area = area;
                part.list = list;
                if part.drawn.is_empty() {
                    part.text_at = text_at;
                }
            }
        }
        parts.clear();
        tree.records.get_mut(&f.id).unwrap().drawn_parts = parts;
    }
    /// Insert a non-widget child. Keys are stable application item identities, not draw indices.
    /// Overflow of the wire's 16-bit part field uses a fresh widget id from the same allocator;
    /// dispatch always resolves the owner, so large virtual collections do not truncate or collide.
    /// `drawn`: the area the part drew as, when it drew as one of its own (see [`Record::drawn`]).
    pub fn set_part(&mut self, token: u32, key: u64, parent_key: Option<u64>, node: Node, drawn: Area, alloc: impl FnOnce() -> u64) {
        self.set_part_inner(token, key, parent_key, node, drawn, 0, alloc);
    }
    /// Insert a lookup answer through the same stable ids and action routing as [`Self::set_part`].
    /// Its record stays until the next lookup replaces it, even across ordinary redraws.
    pub fn set_lookup_part(&mut self, token: u32, key: u64, parent_key: Option<u64>, node: Node, drawn: Area, alloc: impl FnOnce() -> u64) {
        if self.lookups.is_empty() { return; }
        self.set_part_inner(token, key, parent_key, node, drawn, self.lookup_frame, alloc);
    }
    fn set_part_inner(&mut self, token: u32, key: u64, parent_key: Option<u64>, mut node: Node, drawn: Area, looked_up: u32, alloc: impl FnOnce() -> u64) {
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
        rec.looked_up = looked_up;
        rec.drawn = drawn;
        rec.next = node;
    }
    /// The path of the node being built could not be named yet: it is asked again once the
    /// frame has drawn (`set_path_resolver`), and after every later frame until it is named.
    pub fn path_unknown(&mut self, token: u32) {
        let f = self.stack[token as usize];
        if let Some(w) = f.window {
            if !self.paths_unknown.contains(&(w, f.id)) {
                self.paths_unknown.push((w, f.id));
                self.paths_added = true;
            }
        }
    }
    /// Whether the node being built still waits for its path (`path_unknown`): its widget names
    /// it again when it draws.
    pub fn path_pending(&self, token: u32) -> bool {
        if self.paths_unknown.is_empty() {
            return false;
        }
        let f = self.stack[token as usize];
        f.window.is_some_and(|w| self.paths_unknown.contains(&(w, f.id)))
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

    /// Update an action target from its widget when it redrew between semantic frames.
    pub fn refresh_node_area(&mut self, id: NodeId, area: Area) {
        if let Some(record) = self.windows.values_mut().find_map(|tree| tree.records.get_mut(&id)) {
            record.area = area;
        }
    }

    /// `list_redraw` excludes freed lists, including pool slots awaiting reuse. `bounds` runs
    /// after all turtles finish, also updating cached nodes when their view transform moves.
    /// `origin` is an area's unclipped top-left then, which a node's text follows.
    pub fn finish_frame(
        &mut self,
        focus: Area,
        mut list_redraw: impl FnMut(DrawListId) -> Option<u64>,
        mut bounds: impl FnMut(Area) -> Option<makepad_a11y::Rect>,
        mut origin: impl FnMut(Area) -> Option<[f32; 2]>,
        mut emit: impl FnMut(WindowId, Vec<u8>),
    ) {
        if !self.on {
            return;
        }
        let serving = !self.lookups.is_empty();
        let sticky = if serving { self.lookup_frame } else { self.sticky };
        for (window, tree) in &mut self.windows {
            tree.ids.clear();
            tree.ids.extend(tree.records.keys().copied());
            for rec in tree.records.values_mut() {
                rec.reachable = false;
                let redraw = rec.list.and_then(&mut list_redraw);
                rec.alive = if rec.looked_up != 0 {
                    rec.looked_up == sticky
                } else {
                    rec.seen == self.frame || redraw.is_some_and(|r| r == rec.redraw)
                };
                if rec.seen != self.frame && rec.annotated != self.frame {
                    reuse_node(&mut rec.next, &rec.node);
                    // A name a tip gave is given again below, from the tip as it is now.
                    if rec.named_by_tip {
                        rec.next.name.clear();
                    }
                }
                rec.named_by_tip = false;
                if rec.alive {
                    if !rec.owner.is_none() {
                        // Parts carry their own bounds, but focus belongs to the owner's real
                        // Area; one drawn as an area of its own stands where that area shows.
                        if !rec.drawn.is_empty() && (rec.looked_up == 0 || rec.seen == self.frame) {
                            shown_at(&mut rec.next, bounds(rec.drawn));
                        } else if let (Some(at), Some(now)) = (rec.text_at, origin(rec.area)) {
                            let [dx, dy] = [now[0] - at[0], now[1] - at[1]];
                            if dx != 0.0 || dy != 0.0 {
                                rec.next.bounds.x += dx;
                                rec.next.bounds.y += dy;
                                follow_origin(&mut rec.next, [dx, dy]);
                                rec.text_at = Some(now);
                            }
                        }
                    } else {
                        if !rec.area.is_empty() {
                            shown_at(&mut rec.next, bounds(rec.area));
                        }
                        if let (Some(at), Some(now)) = (rec.text_at, origin(rec.area)) {
                            follow_origin(&mut rec.next, [now[0] - at[0], now[1] - at[1]]);
                            rec.text_at = Some(now);
                        }
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
            // A control with no name of its own inside a tip is named by the tip's text (a
            // tooltip is a name's last resort, as an HTML `title` is): what a pointer reads on
            // hover is what a screen reader hears, from one source.
            tree.tipped.clear();
            for id in &tree.ids {
                let Some(tip) = tree.records.get(id).filter(|r| r.reachable && r.next.role == Role::Tooltip && !r.next.description.is_empty()) else { continue };
                tree.tipped.extend(tip.next.children.iter().map(|child| (*child, *id)));
            }
            for &(child, tip) in &tree.tipped {
                if !tree.records.get(&child).is_some_and(|r| r.next.name.is_empty() && r.next.role.is_interactive()) {
                    continue;
                }
                let text = std::mem::take(&mut tree.records.get_mut(&tip).unwrap().next.description);
                let rec = tree.records.get_mut(&child).unwrap();
                set_string(&mut rec.next.name, &text);
                rec.named_by_tip = true;
                tree.records.get_mut(&tip).unwrap().next.description = text;
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
        self.lookup_done = std::mem::take(&mut self.snapshot);
        if serving {
            self.sticky = self.lookup_frame;
            for (source, _, _) in self.lookups.drain(..) {
                match source {
                    LookupSource::Native(seq) => self.lookup_done = self.lookup_done.max(seq),
                    LookupSource::Host(req) => self.host_lookups_done.push(req),
                }
            }
        }
        self.host_lookups_done.extend(std::mem::take(&mut self.host_snapshots));
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

/// A node's text geometry moved with its area (a parent aligned it after its semantics ran).
fn follow_origin(node: &mut Node, [dx, dy]: [f32; 2]) {
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    if let Some(text) = node.text.as_mut() {
        for line in &mut text.lines {
            line.rect.x += dx;
            line.rect.y += dy;
            line.baseline += dy;
        }
        for x in &mut text.glyph_x {
            *x += dx;
        }
    }
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
        if !self.a11y.paths_unknown.is_empty() {
            // Kept until named: the graph may read a widget made mid-draw only after a later
            // frame. A node whose window or record went is dropped.
            let mut unknown = std::mem::take(&mut self.a11y.paths_unknown);
            if let Some(resolve) = PATH_RESOLVER.get() {
                unknown.retain(|(w, id)| {
                    let Some(root) = self.a11y.windows.get(w).filter(|t| t.records.contains_key(id)).map(|t| t.root.uid()) else {
                        return false;
                    };
                    let Some(path) = resolve(self, id.uid(), root) else { return true };
                    if let Some(rec) = self.a11y.windows.get_mut(w).and_then(|t| t.records.get_mut(id)) {
                        rec.next.path = path;
                    }
                    false
                });
            } else {
                unknown.clear();
            }
            self.a11y.paths_unknown = unknown;
        }
        self.a11y.paths_settled = !std::mem::take(&mut self.a11y.paths_added);
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
            |area| area.is_valid(self).then(|| {
                let pos = area.rect(self).pos;
                [pos.x as f32, pos.y as f32]
            }),
            |window, bytes| ops.push_back(CxOsOp::A11yUpdate { window, bytes }),
        );
        let seq = a11y.take_lookup_done();
        if seq != 0 {
            ops.push_back(CxOsOp::A11yLookupDone { seq });
        }
        for req in a11y.take_host_lookups_done() {
            ops.push_back(CxOsOp::A11yHostLookupDone { req });
        }
        if a11y.frame_covers {
            a11y.built = Some((self.event_id, self.keyboard.key_focus()));
        }
        if a11y.build_on_step {
            a11y.build_on_step = false;
            a11y.set_consumers(a11y.consumers);
        }
        self.platform_ops = ops;
        self.a11y = a11y;
    }
    /// A frame begins to draw (`redraw_all`: every list records again). It marks the tree built
    /// only if it covers the events before it: it collects every node again, or no event reached
    /// the app since the tree was last built. A frame that draws some lists keeps the others'
    /// nodes as they were, and an event may have changed what their widgets describe without a
    /// redraw (a pointer a view took on hover, drawing only the status bar): the tree is then
    /// not current, and the next snapshot draws every window. Under the virtual clock every frame
    /// covers its events: the app draws only on a `/step`, and a caller that wants a fresher tree
    /// steps a frame first ([`Self::a11y_lookup`]).
    pub(crate) fn a11y_begin_frame(&mut self, redraw_all: bool) {
        let covers = redraw_all || crate::remote::app_clock::enabled()
            || self.a11y.built == Some((self.event_id, self.keyboard.key_focus()));
        self.a11y.begin_frame();
        self.a11y.frame_covers = covers;
    }
    /// The tree the builder last built is still the app's ([`CxA11y::current`]): a snapshot of it
    /// needs no frame.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn a11y_tree_current(&self) -> bool {
        self.a11y.current(self.event_id, self.keyboard.key_focus(), self.need_redrawing())
    }
    /// The HAL's consumers changed (`consumers`, the app host's added): a builder that comes back
    /// on redraws every window to build its tree, unless the tree it built last is still the
    /// app's ([`Self::a11y_tree_current`]). A snapshot consumer (a remote or test request) holds
    /// the builder only while its request runs, so between requests that draw nothing, each one
    /// would otherwise draw every window again.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn a11y_consumers_changed(&mut self, consumers: u32) {
        if consumers != 0 && !self.a11y.on && self.a11y_tree_current() {
            self.a11y.resume(consumers);
            return;
        }
        if self.a11y.set_consumers(consumers) {
            self.a11y_redraw_for_request();
        }
    }
    /// A semantic lookup from the HAL (`seq`, `query`): a snapshot (an empty query) of a tree that
    /// is still the app's is acknowledged at once, with no frame; anything else is built and
    /// acknowledged by the next frame. Under the virtual clock the app draws only on a `/step`, so
    /// between two steps the tree its last frame built is what it shows: a snapshot is
    /// acknowledged at once over it too. Waiting for a frame there waited out each read's
    /// timeout while the caller that steps waited for the read (an app run's 5 fps pass: every
    /// layout read 5 s, its counter's clicks unseen, 11-19). The caller's hold ends with its
    /// read, before it steps, so under the virtual clock a lookup also keeps the builder on
    /// through the next frame that draws (`build_on_step`): that frame publishes what changed,
    /// and the next snapshot shows it. Without it no stepped frame built a tree while no read
    /// was running, and every later snapshot showed the first one's tree (11-26). The read asks
    /// for no frame of its own: a read never makes the app unsettled. After a partial frame the
    /// tree may keep a node a widget that did not redraw left behind; under the virtual clock
    /// that is accepted: a caller that wants a fresher tree steps a frame first.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn a11y_lookup(&mut self, seq: u32, query: &str) {
        self.a11y_lookup_with_clock(seq, query, crate::remote::app_clock::enabled());
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn a11y_lookup_with_clock(&mut self, seq: u32, query: &str, virtual_clock: bool) {
        let shown = self.a11y_tree_current() || (virtual_clock && self.a11y.built.is_some());
        if seq != 0 && query.trim().is_empty() && self.a11y.on && shown {
            self.platform_ops.push_back(CxOsOp::A11yLookupDone { seq });
            self.a11y.build_on_step = virtual_clock;
            return;
        }
        if self.a11y.request_lookup(LookupSource::Native(seq), query) {
            self.a11y.build_on_step = virtual_clock;
            self.a11y_redraw_for_request();
        }
    }
    /// Redraws every window for a tree request. The frame is the request's alone
    /// ([`Self::a11y_answer_frame`]) when nothing else waits for one: no redraw, no next frame,
    /// none but this request's own asks.
    #[cfg(not(target_arch = "wasm32"))]
    fn a11y_redraw_for_request(&mut self) {
        let alone = self.a11y_answer_frame == Some(self.frame_requests)
            || !(self.need_redrawing() || !self.new_next_frames.is_empty());
        self.redraw_all();
        self.a11y_answer_frame = alone.then_some(self.frame_requests);
    }
    /// The frame about to draw takes the request's claim: true when it is drawn only to answer a
    /// tree request, with no frame asked for by anything else since the request asked
    /// (its CPU is the request's, [`crate::remote::charge_answered`]).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn a11y_take_answer_frame(&mut self) -> bool {
        self.a11y_answer_frame.take() == Some(self.frame_requests)
    }
}
/// A drawn node's bounds: its area clipped by every clip it drew under, on every side. None:
/// nothing of it is inside them (a row a list scrolled past an edge, a panel a scroll view
/// moved away), so it is offscreen, with no bounds.
fn shown_at(node: &mut Node, shown: Option<makepad_a11y::Rect>) {
    node.states.set(States::OFFSCREEN, shown.is_none());
    node.bounds = shown.unwrap_or_default();
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
                let area = self.a11y.node_area(window, request.node).map_or(area, |(a, _, _)| a);
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
                        self.set_key_focus_from_user(area);
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
                    // The bounds are layout points; the studio pointer path remaps native points
                    // into layout (a window's DPI override), so hand it native points.
                    let (x, y) = bounds.center();
                    let center = dvec2(x as f64, y as f64);
                    let native = if self.windows.is_valid(window) { self.windows[window].layout_vec2d_to_native_points(center) } else { center };
                    let (x, y) = (native.x, native.y);
                    let time = self.seconds_since_app_start();
                    self.dispatch_studio_msg(
                        StudioToApp::MouseDown(RemoteMouseDown {
                            time,
                            x,
                            y,
                            button_raw_bits: 1,
                            modifiers: Default::default(),
                        }),
                        window,
                        dvec2(0.0, 0.0),
                    );
                    self.dispatch_studio_msg(
                        StudioToApp::MouseUp(RemoteMouseUp {
                            time: time + 0.001,
                            x,
                            y,
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
        finish_shown(b, focus, redraw, |_| None)
    }
    /// [`finish`] with the areas' on-screen bounds (`None`: nothing of it on screen).
    fn finish_shown(
        b: &mut CxA11y,
        focus: Area,
        redraw: impl FnMut(DrawListId) -> Option<u64>,
        bounds: impl FnMut(Area) -> Option<makepad_a11y::Rect>,
    ) -> Vec<TreeUpdate> {
        let mut messages = Vec::new();
        b.finish_frame(
            focus,
            redraw,
            bounds,
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
    fn lookup_parts_stay_until_replaced_and_resolve_by_stable_key() {
        let mut pool = CxDrawListPool::default();
        let list = pool.alloc();
        let other_list = pool.alloc();
        let mut b = active();
        let mut tree = Tree::new();
        let bounds = makepad_a11y::Rect { x: 12.0, y: 34.0, w: 20.0, h: 10.0 };
        let draw = |b: &mut CxA11y, expected: Option<Option<u64>>, answer: bool, ordinary: bool, redraw| {
            b.begin_frame();
            let r = root(b, WindowId(0, 1), 1, list.id());
            let terms = expected == Some(None);
            assert_eq!(b.lookups_for(r).next().is_some(), terms, "only a terms lookup visits the first root");
            let owner = b.begin_node(2, Some(list.id()));
            assert_eq!(b.lookups_for(owner).next().map(|(_, key)| key), expected, "the owner receives terms, its key, or no lookup");
            if answer {
                let (query, key) = b.lookups_for(owner).next().expect("answers require a pending lookup");
                let part = Node { bounds, actions: Actions::PRESS, ..node(Role::Button, "Item") };
                assert!(key.is_some() || query.matches(&part), "a terms answer matches the complete query");
                b.set_lookup_part(owner, key.unwrap_or(7), None, part, Area::Empty, || unreachable!("a packed part id"));
            }
            if ordinary {
                b.set_part(owner, 8, None, node(Role::Button, "Ordinary"), Area::Empty, || unreachable!("a packed part id"));
            }
            b.end_node(owner, node(Role::Group, "Owner"), Area::Empty);
            let sibling = b.begin_node(3, Some(list.id()));
            assert_eq!(b.lookups_for(sibling).next().is_some(), terms, "a key lookup never visits another owner");
            b.end_node(sibling, node(Role::Group, "Other"), Area::Empty);
            end_root(b, r);
            let other = root(b, WindowId(1, 1), 10, other_list.id());
            assert_eq!(b.lookups_for(other).next().is_some(), terms, "no pending lookup visits no token, including other windows");
            end_root(b, other);
            finish(b, Area::Empty, |_| Some(redraw))
        };
        let apply = |tree: &mut Tree, updates: Vec<TreeUpdate>| {
            for update in updates {
                if update.root == NodeId::widget(1) { tree.apply(update); }
            }
        };
        let acknowledge = |b: &mut CxA11y, seq| {
            assert_eq!(b.take_lookup_done(), seq, "the frame acknowledges exactly its requested lookup, or none");
            assert_eq!(b.take_lookup_done(), 0, "a completed lookup is acknowledged only once per frame");
        };

        assert!(!b.request_lookup(LookupSource::Native(1), "role=button"), "structural filters do not request materialisation");
        apply(&mut tree, draw(&mut b, None, false, true, 1));
        acknowledge(&mut b, 0);
        let ordinary = tree.nodes.values().find(|n| n.name == "Ordinary").expect("ordinary part drawn").id;
        assert!(b.request_lookup(LookupSource::Native(1), "name=Item"), "a named lookup requests every window redraw");
        let updates = draw(&mut b, Some(None), true, false, 2);
        assert_eq!(updates.len(), 1, "only the changed window emits an update");
        assert_eq!(updates[0].root, NodeId::widget(1), "the unchanged window needs no update to acknowledge the lookup");
        acknowledge(&mut b, 1);
        apply(&mut tree, updates);
        let part = tree.nodes.values().find(|n| n.name == "Item").expect("lookup answer is in the tree").id;
        assert!(tree.node(ordinary).is_none(), "ordinary parts retain their existing redraw lifetime");
        assert_eq!(b.action_owner(part), NodeId::widget(2), "lookup actions route to the owning widget");
        assert_eq!(b.action_part_key(part), Some(7), "lookup actions retain the stable part key");

        b.request_full();
        let updates = draw(&mut b, None, false, false, 3);
        acknowledge(&mut b, 0);
        apply(&mut tree, updates);
        assert_eq!(tree.node(part).map(|n| n.bounds), Some(bounds), "sticky parts keep their last bounds across redraws");

        assert!(b.request_lookup(LookupSource::Native(2), "name=Missing"), "a second query requests another frame");
        // The list's cached redraw number is unchanged: lookup expiry must take precedence.
        apply(&mut tree, draw(&mut b, Some(None), false, false, 3));
        assert!(tree.node(part).is_none(), "an empty lookup drops the previous sticky answer");
        acknowledge(&mut b, 2);
        assert_eq!(b.action_part_key(part), None, "the expired part record is removed");

        let query = format!("id={}", part.short());
        assert!(b.request_lookup(LookupSource::Native(3), &query), "an expired id can request its item again");
        assert_eq!(b.lookups.first().and_then(|(_, _, owner)| *owner), Some((NodeId::widget(2), 7)), "the owner's parts table resolves an expired id");
        apply(&mut tree, draw(&mut b, Some(Some(7)), true, false, 4));
        acknowledge(&mut b, 3);
        assert_eq!(tree.nodes.values().find(|n| n.name == "Item").map(|n| n.id), Some(part), "successive lookups keep the same NodeId after expiry");

        assert!(b.request_lookup(LookupSource::Native(4), &query), "a live part id requests a refresh too");
        assert_eq!(b.lookups.first().and_then(|(_, _, owner)| *owner), Some((NodeId::widget(2), 7)), "a live part resolves to its owner and key");
        let updates = draw(&mut b, Some(Some(7)), true, false, 5);
        assert!(updates.is_empty(), "an unchanged lookup needs no tree updates");
        acknowledge(&mut b, 4);
        apply(&mut tree, updates);

        // An overflow id has no packed owner. Seed just its retained allocation, without
        // creating a large collection, to exercise the same expired-record recovery path.
        let overflow = NodeId::widget(99);
        b.windows.get_mut(&key(WindowId(0, 1))).expect("window exists").records
            .get_mut(&NodeId::widget(2)).expect("owner exists").parts.insert(9, overflow);
        assert!(b.request_lookup(LookupSource::Native(5), &format!("id={}", overflow.short())), "an overflow id requests its owner");
        apply(&mut tree, draw(&mut b, Some(Some(9)), true, false, 6));
        acknowledge(&mut b, 5);
        assert!(tree.node(overflow).is_some(), "overflow ids recover through owner tables without reallocating");
        assert!(tree.node(part).is_none(), "a key lookup replaces the previous sticky set");

        // Native and hosted consumers can ask for different items before the same frame.
        assert!(b.request_lookup(LookupSource::Native(6), "name=NativeItem"), "a native lookup joins the next serving frame");
        assert!(b.request_lookup(LookupSource::Host(40), "name=HostedItem"), "a host lookup joins the same serving frame");
        b.begin_frame();
        let r = root(&mut b, WindowId(0, 1), 1, list.id());
        let owner = b.begin_node(2, Some(list.id()));
        assert_eq!(b.lookups_for(owner).count(), 2, "the owner receives both pending lookup queries");
        let items = [(10, node(Role::Button, "NativeItem")), (11, node(Role::Button, "HostedItem"))];
        let mut answers = Vec::new();
        for (query, key) in b.lookups_for(owner) {
            assert_eq!(key, None, "both concurrent name queries use terms lookup");
            for (key, item) in &items {
                if query.matches(item) { answers.push((*key, item.clone())); }
            }
        }
        assert_eq!(answers.len(), 2, "the serving frame answers both distinct item queries");
        for (key, item) in answers {
            b.set_lookup_part(owner, key, None, item, Area::Empty, || unreachable!("a packed part id"));
        }
        b.end_node(owner, node(Role::Group, "Owner"), Area::Empty);
        let sibling = b.begin_node(3, Some(list.id()));
        b.end_node(sibling, node(Role::Group, "Other"), Area::Empty);
        end_root(&mut b, r);
        let other = root(&mut b, WindowId(1, 1), 10, other_list.id());
        end_root(&mut b, other);
        apply(&mut tree, finish(&mut b, Area::Empty, |_| Some(7)));
        for (_, item) in &items {
            assert!(tree.nodes.values().any(|n| n.name == item.name), "both sources' answers appear in one tree: {}", item.name);
        }
        acknowledge(&mut b, 6);
        assert_eq!(b.take_host_lookups_done(), vec![40], "the serving frame acknowledges its host request");
        assert!(b.take_host_lookups_done().is_empty(), "a host lookup is acknowledged only once");

        apply(&mut tree, draw(&mut b, None, false, false, 8));
        acknowledge(&mut b, 0);
        assert!(b.take_host_lookups_done().is_empty(), "an ordinary frame acknowledges no host lookup");
        for (_, item) in &items {
            assert!(tree.nodes.values().any(|n| n.name == item.name), "the union sticky set survives an ordinary frame: {}", item.name);
        }

        assert!(b.request_lookup(LookupSource::Native(7), "name=Item"), "the next lookup requests a replacement sticky set");
        apply(&mut tree, draw(&mut b, Some(None), true, false, 8));
        acknowledge(&mut b, 7);
        assert!(tree.node(part).is_some(), "the next lookup installs its replacement answer");
        for (_, item) in &items {
            assert!(!tree.nodes.values().any(|n| n.name == item.name), "the next lookup replaces both sources' prior answers: {}", item.name);
        }
        assert!(b.take_host_lookups_done().is_empty(), "a later native lookup does not repeat the host acknowledgement");

        assert!(b.request_lookup(LookupSource::Host(41), ""), "an empty host query requests a snapshot frame");
        assert!(b.lookups.is_empty(), "an empty host query creates no materialisation query");
        assert!(b.take_host_lookups_done().is_empty(), "a host snapshot is not acknowledged before its frame");
        apply(&mut tree, draw(&mut b, None, false, false, 9));
        acknowledge(&mut b, 0);
        assert_eq!(b.take_host_lookups_done(), vec![41], "the next frame acknowledges the empty host query");
        assert!(b.take_host_lookups_done().is_empty(), "a host snapshot is acknowledged only once");
        assert!(tree.node(part).is_some(), "a host snapshot preserves the current sticky answer");

        b.set_consumers(0);
        assert!(!b.request_lookup(LookupSource::Native(6), "name=Item"), "disabled consumers cannot schedule a lookup");
        assert!(finish(&mut b, Area::Empty, |_| Some(6)).is_empty(), "disabled consumers emit no updates");
        acknowledge(&mut b, 0);
    }

    /// A node drawn with nothing of it on screen (clipped away, a row scrolled past a list's
    /// edge) is offscreen, with no bounds; scrolled back, it is on screen where it stands. So is
    /// a part drawn as an area of its own (a list's row that is no widget), where its area
    /// shows when the frame finishes; a part drawn as none keeps the bounds it carries.
    #[test]
    fn a_node_clipped_out_of_view_is_offscreen_until_it_shows() {
        let mut pool = CxDrawListPool::default();
        let list = pool.alloc();
        let area = |rect_id| Area::Rect(RectArea { draw_list_id: list.id(), rect_id, redraw_id: 1 });
        let (row, part_row) = (area(0), area(1));
        let carried = makepad_a11y::Rect { x: 0.0, y: 90.0, w: 100.0, h: 20.0 };
        let mut b = active();
        let mut tree = makepad_a11y::mirror::Tree::default();
        let frame = |b: &mut CxA11y, tree: &mut makepad_a11y::mirror::Tree, shown: Option<makepad_a11y::Rect>| {
            b.begin_frame();
            let r = root(b, WindowId(0, 1), 1, list.id());
            let t = b.begin_node(2, Some(list.id()));
            b.end_node(t, node(Role::ListItem, "Row"), row);
            let owner = b.begin_node(3, Some(list.id()));
            b.set_part(owner, 7, None, node(Role::ListItem, "Part row"), part_row, || unreachable!("a part id"));
            b.set_part(owner, 8, None, Node { bounds: carried, ..node(Role::ListItem, "Own bounds") }, Area::Empty, || unreachable!("a part id"));
            b.end_node(owner, node(Role::List, "List"), Area::Empty);
            end_root(b, r);
            b.finish_frame(Area::Empty, |_| Some(1), |a| if a == row || a == part_row { shown } else { None }, |_| None, |_, bytes| {
                tree.apply(makepad_a11y::wire::decode_update(&bytes).unwrap());
            });
            let get = |id| tree.node(id).cloned().expect("drawn");
            (get(NodeId::widget(2)), get(NodeId::part(3, 1)), get(NodeId::part(3, 2)))
        };
        let (row, part, own) = frame(&mut b, &mut tree, None);
        assert!(row.states.contains(States::OFFSCREEN) && row.bounds.is_empty(), "clipped away: offscreen, no bounds");
        assert!(part.states.contains(States::OFFSCREEN) && part.bounds.is_empty(), "a part's row clipped away too");
        assert!(!own.states.contains(States::OFFSCREEN) && own.bounds == carried, "a part with no area of its own keeps its bounds");
        let on = makepad_a11y::Rect { x: 0.0, y: 40.0, w: 100.0, h: 20.0 };
        let (row, part, own) = frame(&mut b, &mut tree, Some(on));
        assert!(!row.states.contains(States::OFFSCREEN), "scrolled back into view: on screen");
        assert_eq!(row.bounds, on);
        assert!(!part.states.contains(States::OFFSCREEN) && part.bounds == on, "the part's row where its area shows");
        assert_eq!(own.bounds, carried);
    }

    #[test]
    fn text_follows_an_alignment_that_moves_its_node_after_its_semantics() {
        let mut pool = CxDrawListPool::default();
        let list = pool.alloc();
        let mut b = active();
        b.begin_frame();
        let r = root(&mut b, WindowId(0, 1), 1, list.id());
        let t = b.begin_node(2, Some(list.id()));
        // the field drew at y 66 and measured its line there; the row it sits in then centred it
        // 9 lower and the list moved it 2 right
        let mut field = node(Role::TextField, "Search");
        field.text = Some(TextInfo {
            content: "ab".into(),
            lines: vec![makepad_a11y::LineBox { start: 0, rect: makepad_a11y::Rect { x: 104.0, y: 68.5, w: 12.0, h: 13.0 }, baseline: 78.5 }],
            glyph_x: vec![104.0, 110.0, 116.0],
            ..Default::default()
        });
        b.set_text_origin(t, [104.0, 66.0]);
        b.end_node(t, field, Area::Empty);
        end_root(&mut b, r);
        let mut messages = Vec::new();
        b.finish_frame(Area::Empty, |_| Some(1), |_| None, |_| Some([106.0, 75.0]), |w, bytes| messages.push((w, bytes)));
        let update = makepad_a11y::wire::decode_update(&messages[0].1).unwrap();
        let text = update.nodes.iter().find(|n| n.id == NodeId::widget(2)).and_then(|n| n.text.clone()).expect("the field's text");
        let line = text.lines[0];
        assert_eq!((line.rect.x, line.rect.y, line.baseline, text.glyph_x), (106.0, 77.5, 87.5, vec![106.0, 112.0, 118.0]), "the line box and glyphs are where the field was put, not where it drew");
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
                }, Area::Empty, || unreachable!("existing widget"));
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

    /// An icon button wrapped in a tip has no name of its own: the tip's text names it, and a
    /// control with a name keeps it. A frame that redraws nothing keeps the tip's name; a new
    /// tip text renames the control.
    #[test]
    fn a_nameless_control_in_a_tip_is_named_by_the_tip() {
        let mut pool = CxDrawListPool::default();
        let main = pool.alloc();
        let content = pool.alloc();
        let mut builder = active();
        let mut tree = Tree::new();
        let frame = |builder: &mut CxA11y, tree: &mut Tree, frame: u64, tip: Option<&str>, content_redraw: u64| {
            builder.begin_frame();
            let root = root(builder, WindowId(0, 1), 1, main.id());
            if let Some(text) = tip {
                for (tip, control, name) in [(2, 3, ""), (4, 5, "Open")] {
                    let t = builder.begin_node(tip, Some(content.id()));
                    let c = builder.begin_node(control, Some(content.id()));
                    builder.end_node(c, node(Role::Button, name), Area::Empty);
                    builder.end_node(t, Node { role: Role::Tooltip, description: text.into(), ..Default::default() }, Area::Empty);
                }
            }
            end_root(builder, root);
            for update in finish(builder, Area::Empty, |list| Some(if list == main.id() { frame } else { content_redraw })) {
                tree.apply(update);
            }
            let name = |uid| tree.node(NodeId::widget(uid)).expect("drawn").name.clone();
            (name(3), name(5))
        };
        let named = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(frame(&mut builder, &mut tree, 1, Some("Fit the whole map"), 1), named("Fit the whole map", "Open"));
        assert_eq!(frame(&mut builder, &mut tree, 2, None, 1), named("Fit the whole map", "Open"), "cached: the tip's name stays");
        assert_eq!(frame(&mut builder, &mut tree, 3, Some("Home"), 2), named("Home", "Open"), "a new tip text renames it");
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
        }, Area::Empty, || unreachable!("an existing widget needs no part id"));
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
        assert!(!b.on, "no consumers stops building");
        assert_eq!(b.windows[&key(WindowId(0, 1))].records.len(), 3, "off keeps the window's records, including pruned owners");
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
            // the field on screen where it drew
            let drawn = makepad_a11y::Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 };
            let u = finish_shown(&mut b, if i == 0 { Area::Empty } else { area }, |_| {
                Some(i as u64 + 1)
            }, |a| (a == area).then_some(drawn))
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
            |_| None,
            |_, bytes| updates.push(makepad_a11y::wire::decode_update(&bytes).unwrap()),
        );
        assert_eq!(
            updates[0].nodes[0].bounds.x, 42.0,
            "retained areas follow transforms without a redraw"
        );
        assert!(updates[0].notifications.is_empty());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_snapshot_request_draws_only_when_the_app_changed_since_its_tree_was_built() {
        use crate::{draw_list::DrawList, event::Event, window::WindowHandle};
        use makepad_a11y::consumer::REMOTE;
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let window = WindowHandle::new(&mut cx);
        let wid = window.window_id();
        let list = DrawList::new(&mut cx);
        cx.draw_lists[list.id()].redraw_id = 1;
        // one snapshot request as the HAL sends it: the builder on, an empty lookup, the frame it
        // asks for (if any), then the builder off; whether it owed a frame, and that frame only
        // to the request (its CPU the request's), whether it published a tree, what it acknowledged
        let request = |cx: &mut Cx, seq: u32, unnamed: bool| {
            cx.platform_ops.clear();
            cx.a11y_consumers_changed(REMOTE);
            cx.a11y_lookup(seq, "");
            let drew = cx.need_redrawing();
            let answer = cx.a11y_take_answer_frame();
            if drew {
                cx.new_draw_event = Default::default();
                cx.a11y.begin_frame();
                let r = root(&mut cx.a11y, wid, 1, list.id());
                let t = cx.a11y.begin_node(2, Some(list.id()));
                if unnamed { cx.a11y.path_unknown(t); }
                cx.a11y.end_node(t, node(Role::Button, "Save"), Area::Empty);
                end_root(&mut cx.a11y, r);
                cx.a11y_finish_frame();
            }
            let acked: Vec<u32> = cx.platform_ops.iter().filter_map(|op| match op { CxOsOp::A11yLookupDone { seq } => Some(*seq), _ => None }).collect();
            let updated = cx.platform_ops.iter().any(|op| matches!(op, CxOsOp::A11yUpdate { .. }));
            cx.a11y_consumers_changed(0);
            (drew, answer, updated, acked)
        };
        assert_eq!(request(&mut cx, 5, false), (true, true, true, vec![5]), "the first request builds the tree in a frame only it asked for, which acknowledges it");
        assert!(!cx.a11y.on, "between requests the builder is off");
        assert_eq!(request(&mut cx, 6, false), (false, false, false, vec![6]), "with nothing changed since, a request is acknowledged at once, with no frame and no update");
        assert_eq!(request(&mut cx, 7, false), (false, false, false, vec![7]), "and so is the next one");
        cx.call_event_handler(&Event::Signal);
        let (drew, answer, ..) = request(&mut cx, 8, false);
        assert_eq!((drew, answer), (true, true), "an event that reached the app (it may change the tree without a redraw) makes the next request build, in a frame only it asked for");
        // a node whose path the widget layer cannot name (one never in its graph): the frame after
        // the one that found it asks again, and once that one could not name it either, the tree
        // of an app that did not change is answered at once
        set_path_resolver(|_, _, _| None);
        cx.call_event_handler(&Event::Signal);
        assert!(request(&mut cx, 11, true).0, "a request after a change builds, and finds the unnamed path");
        assert!(request(&mut cx, 12, true).0, "the next one builds again: a later frame may name it");
        assert!(!request(&mut cx, 13, true).0, "it stayed unnamed: with nothing changed since, the tree is answered at once");
        cx.call_event_handler(&Event::Signal);
        cx.a11y_consumers_changed(REMOTE);
        cx.a11y_lookup(9, "");
        cx.redraw_all();
        assert!(!cx.a11y_take_answer_frame(), "a frame the app asks for before the request's draws is the app's too");
        cx.a11y_consumers_changed(0);
        cx.a11y_consumers_changed(REMOTE);
        cx.a11y_lookup(10, "");
        assert!(!cx.platform_ops.iter().any(|op| matches!(op, CxOsOp::A11yLookupDone { seq: 10 })), "a redraw waiting is a change: the frame acknowledges the request, not the request itself");
        assert!(!cx.a11y_take_answer_frame(), "a frame the app owed already is the app's, not the request's");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_virtual_clock_snapshot_refreshes_on_the_next_draw_after_release() {
        use crate::event::Event;
        use makepad_a11y::consumer::REMOTE;
        let mut title = "Welcome";
        let mut cx = Cx::new(Box::new(move |cx, event| match event {
            Event::Signal => title = "Ready",
            Event::Draw(_) if cx.a11y.on => {
                let root = cx.a11y.begin_node(1, None);
                cx.a11y.begin_window(WindowId(0, 1));
                cx.a11y.end_window();
                cx.a11y.end_node(root, node(Role::Window, title), Area::Empty);
            }
            _ => {}
        }));
        let request = |cx: &mut Cx, seq| {
            cx.a11y_consumers_changed(REMOTE);
            cx.a11y_lookup_with_clock(seq, "", true);
            let acked = cx.platform_ops.iter().any(|op|
                matches!(op, CxOsOp::A11yLookupDone { seq: done } if *done == seq)
            );
            assert!(acked, "a virtual-clock snapshot with a built tree must be acknowledged before another step");
            cx.a11y_consumers_changed(0);
        };
        cx.a11y_consumers_changed(REMOTE);
        cx.call_draw_event(0.0);
        cx.a11y_consumers_changed(0);
        cx.redraw_all();
        request(&mut cx, 1);

        cx.platform_ops.clear();
        cx.call_event_handler(&Event::Signal);
        cx.call_draw_event(1.0);
        let published = cx.platform_ops.iter().any(|op| match op {
            CxOsOp::A11yUpdate { bytes, .. } => makepad_a11y::wire::decode_update(bytes).unwrap()
                .nodes.iter().any(|n| n.name == "Ready"),
            _ => false,
        });
        assert!(published, "the stepped frame must publish the changed window title after the request released its hold");
        assert!(!cx.a11y.on, "one stepped draw must finish the outstanding tree build");
        request(&mut cx, 2);
        assert!(!cx.need_redrawing(), "reading the current tree must not ask for a frame: a read never makes the app unsettled");
        assert!(cx.a11y.on, "the builder stays on after the read for the next stepped draw");
    }

    /// A frame that draws some lists keeps the other lists' nodes as they were built, so it makes
    /// the tree current only when no event reached the app since: an event can change what a
    /// widget describes without a redraw (a 3D view took a hover's pointer and only the status
    /// bar drew). Before, every frame marked the tree current, and each snapshot after it was
    /// answered with the view's description from before the move, for as long as the app idled.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_frame_that_draws_some_lists_after_an_event_leaves_the_tree_to_build_again() {
        use crate::{draw_list::DrawList, event::Event};
        use makepad_a11y::consumer::REMOTE;
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let list = DrawList::new(&mut cx);
        cx.a11y_consumers_changed(REMOTE);
        cx.call_draw_event(0.0);
        assert!(cx.a11y_tree_current(), "the frame that draws every window builds the tree");
        cx.redraw_list(list.id());
        cx.call_draw_event(0.0);
        assert!(cx.a11y_tree_current(), "a frame that draws some lists with no event since keeps it current");
        cx.call_event_handler(&Event::Signal);
        cx.redraw_list(list.id());
        cx.call_draw_event(0.0);
        assert!(!cx.a11y_tree_current(), "after an event, a frame that draws some lists keeps the others' nodes as they were: not current");
        cx.platform_ops.clear();
        cx.a11y_lookup(3, "");
        let acked = cx.platform_ops.iter().any(|op| matches!(op, CxOsOp::A11yLookupDone { seq: 3 }));
        assert!(cx.need_redrawing() && !acked, "so a snapshot draws every window, not answered from that tree");
        cx.call_draw_event(0.0);
        let acked = cx.platform_ops.iter().any(|op| matches!(op, CxOsOp::A11yLookupDone { seq: 3 }));
        assert!(acked && cx.a11y_tree_current(), "the frame that draws every window answers it and builds the tree");
    }

    #[test]
    fn a11y_action_uses_the_area_its_widget_refreshed() {
        use crate::{draw_list::{CxRectArea, DrawList}, event::Event, window::WindowHandle};
        use std::{cell::Cell, rc::Rc};

        let widget_area = Rc::new(Cell::new(None));
        let handler_area = widget_area.clone();
        let mut cx = Cx::new(Box::new(move |cx, event| {
            if let Event::A11yAction(req) = event {
                if let Some(area) = handler_area.get() {
                    cx.a11y.refresh_node_area(req.node, area);
                }
            }
        }));
        let window = WindowHandle::new(&mut cx);
        let wid = window.window_id();
        let list = DrawList::new(&mut cx);
        cx.draw_lists[list.id()].redraw_id = 1;
        cx.draw_lists[list.id()].rect_areas.push(CxRectArea {
            rect: Rect { pos: dvec2(0.0, 0.0), size: dvec2(80.0, 24.0) },
            draw_clip: (dvec2(0.0, 0.0), dvec2(80.0, 24.0)),
        });
        let area = RectArea { draw_list_id: list.id(), rect_id: 0, redraw_id: 1 };
        cx.a11y.set_consumers(makepad_a11y::consumer::REMOTE);
        cx.a11y.begin_frame();
        let r = root(&mut cx.a11y, wid, 1, list.id());
        let field = cx.a11y.begin_node(2, Some(list.id()));
        cx.a11y.end_node(field, node(Role::TextField, "Name"), Area::Rect(area));
        end_root(&mut cx.a11y, r);
        cx.a11y_finish_frame();
        cx.a11y.set_consumers(0);
        cx.draw_lists[list.id()].redraw_id = 2;
        let fresh_area = Area::Rect(RectArea { redraw_id: 2, ..area });

        let focus = |cx: &mut Cx, req| {
            cx.dispatch_a11y_action(wid, ActionRequest {
                req, node: NodeId::widget(2), action: Actions::FOCUS, arg: makepad_a11y::ActionArg::None,
            });
            let Some(CxOsOp::A11yActionDone { req: done, result }) = cx.platform_ops.pop_back() else {
                panic!("missing focus completion");
            };
            assert_eq!(done, req);
            result
        };
        assert_eq!(focus(&mut cx, 1), ActionResult::Unsupported as u8);
        assert_ne!(cx.keyboard.key_focus(), fresh_area);

        widget_area.set(Some(fresh_area));
        cx.new_draw_event = Default::default();
        assert_eq!(focus(&mut cx, 2), ActionResult::Fallback as u8);
        assert_eq!(cx.keyboard.key_focus(), fresh_area);
        assert!(cx.need_redrawing(), "the focus fallback requests a redraw");
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
            // the node's widget answers its key focus the way a text field does
            Event::KeyFocus(kf) => cx.set_key_focus_takes_text(kf.focus, true),
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
            if action == Actions::FOCUS {
                assert!(cx.key_focus_takes_text(), "a FOCUS action hands the focus over as a press does: its holder says it takes text");
            }
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
                |_| panic!("origin lookup while off"),
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
    use crate::test_alloc::measured;

    #[test]
    fn off_snapshots_preserve_ids_and_os_continuity() {
        use makepad_a11y::consumer::{OS, REMOTE};
        let mut builder = CxA11y::default();
        let mut pool = crate::draw_list::CxDrawListPool::default();
        let list = pool.alloc();
        let part_key = 7;
        let draw = |builder: &mut CxA11y, name: &str| {
            builder.begin_frame();
            let root = builder.begin_node(1, None);
            builder.begin_window(WindowId(0, 1));
            builder.window_draw_list(list.id());
            let owner = builder.begin_node(2, Some(list.id()));
            builder.set_part(owner, part_key, None, Node {
                role: Role::Button, name: name.into(), ..Default::default()
            }, Area::Empty, || unreachable!("the part has a packed id"));
            builder.end_node(owner, Node { role: Role::Group, name: "Owner".into(), ..Default::default() }, Area::Empty);
            builder.end_window();
            builder.end_node(root, Node { role: Role::Window, name: "Test".into(), ..Default::default() }, Area::Empty);
            let mut updates = Vec::new();
            builder.finish_frame(
                Area::Empty, |_| Some(1), |_| None, |_| None,
                |_, bytes| updates.push(makepad_a11y::wire::decode_update(&bytes).expect("valid frame update")),
            );
            updates
        };

        assert!(builder.set_consumers(REMOTE), "the first snapshot activates building");
        let first = draw(&mut builder, "Item");
        assert_eq!(first.len(), 1, "the setup publishes one window");
        let part = first[0].nodes.iter().find(|n| n.name == "Item").expect("the setup publishes its part").id;
        let frame = builder.frame;
        let stack_capacity = builder.stack.capacity();
        builder.set_consumers(0);
        assert!(!builder.on && builder.consumers() == 0, "releasing the last consumer stops building");
        assert_eq!(builder.stack.capacity(), stack_capacity, "turning off retains scratch allocation");
        let off = measured(|| {
            for _ in 0..100_000 {
                if std::hint::black_box(builder.on) {
                    builder.begin_frame();
                }
                builder.finish_frame(
                    Area::Empty,
                    |_| unreachable!("off never inspects draw lists"),
                    |_| unreachable!("off never resolves bounds"),
                    |_| unreachable!("off never resolves origins"),
                    |_, _| unreachable!("off never emits a tree update"),
                );
            }
        });
        assert_eq!(off, 0, "100000 off frames allocate nothing after a populated snapshot");
        assert_eq!(builder.frame, frame, "off frames do not advance the builder frame");
        assert_eq!(builder.action_owner(part), NodeId::widget(2), "off preserves the part's action owner");
        assert_eq!(builder.action_part_key(part), Some(part_key), "off preserves the part's stable key");
        assert!(!builder.request_lookup(LookupSource::Native(9), ""), "off rejects snapshot requests");

        assert!(builder.set_consumers(REMOTE), "reactivation requests a full redraw");
        assert!(builder.request_lookup(LookupSource::Native(10), ""), "an empty query requests a snapshot frame");
        assert!(builder.request_lookup(LookupSource::Native(8), " \t"), "whitespace also requests a snapshot frame");
        assert!(builder.lookups.is_empty(), "snapshot requests build no lookup query");
        let next = draw(&mut builder, "Item");
        assert_eq!(next.len(), 1, "reactivation publishes the retained window");
        assert!(next[0].full, "the reactivation update is full");
        assert_eq!(builder.windows[&key(WindowId(0, 1))].records[&NodeId::widget(2)].parts[&part_key], part, "the retained key maps to the same part id");
        assert!(next[0].nodes.iter().any(|n| n.id == part && n.name == "Item"), "the rebuilt snapshot publishes the same part id");
        assert_eq!(builder.take_lookup_done(), 10, "the built frame acknowledges the greatest snapshot sequence");

        assert!(!builder.set_consumers(OS | REMOTE), "adding OS to a live snapshot does not reactivate");
        assert!(!builder.set_consumers(OS), "releasing REMOTE leaves OS continuous");
        assert!(builder.on, "OS keeps building between snapshot requests");
        assert!(draw(&mut builder, "Item").is_empty(), "an unchanged OS frame emits nothing");
        let changed = draw(&mut builder, "Changed");
        assert_eq!(changed.len(), 1, "a changed OS frame emits one update");
        assert!(!changed[0].full, "continuous OS updates remain incremental");
        assert_eq!(changed[0].nodes.len(), 1, "the OS update contains only the changed node");
        assert_eq!(changed[0].nodes[0].id, part, "the incremental update identifies the changed part");
        assert_eq!(changed[0].nodes[0].name, "Changed", "the incremental update carries the new node value");
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
            // an icon button its tip names
            let tip = builder.begin_node(3, Some(list.id()));
            let icon = builder.begin_node(4, Some(list.id()));
            let mut node = builder.take_node(icon);
            node.role = Role::Button;
            builder.end_node(icon, node, Area::Empty);
            let mut node = builder.take_node(tip);
            node.role = Role::Tooltip;
            set_string(&mut node.description, "Fit the whole map");
            builder.end_node(tip, node, Area::Empty);
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
                builder.finish_frame(Area::Empty, |_| Some(1), |_| None, |_| None, |_, _| unreachable!());
            }
        });
        assert_eq!(cached, 0);
        eprintln!(
            "a11y allocation probe: off=0/100000 frames; active unchanged=0/1000; cached=0/1000"
        );
    }
}
