//! The tiling layout: a dwindle (BSPWM-style) binary tree per workspace,
//! plus the state Omarchy hangs off individual windows — floating, pinned,
//! pseudo, fullscreen modes, groups and the special "scratchpad" workspace.
//!
//! Behavior read from source, never guessed:
//!
//! - dwindle: `local/vendor/hyprland/src/layout/algorithm/tiled/dwindle/
//!   DwindleAlgorithm.cpp`. Omarchy pins `force_split = 2` (a new window
//!   always opens right/bottom) and `preserve_split = true` (a split's axis
//!   never re-derives; SUPER+J toggles it) — `default/hypr/looknfeel.lua`.
//! - resize: `CDwindleAlgorithm::resizeTarget` moves the NEAREST divider on
//!   that axis by Δ px (`splitRatio += Δ * 2 / box.w`, where ratio 1.0 is a
//!   50/50 split), regardless of which side of it the focused window sits
//!   on. Positive Δ moves the divider right/down.
//! - pseudo: `local/vendor/hyprland/src/layout/target/WindowTarget.cpp:208`
//!   — the window keeps its own size centered in the slot, scaled down
//!   uniformly when it does not fit. A tiled window's pseudo size is set at
//!   map time to `realSize - (10, 10)` (`Window.cpp:1451`).
//! - groups: `local/vendor/hyprland/src/config/shared/actions/
//!   ConfigActions.cpp` — `toggleGroup` creates a group of one or destroys
//!   the whole group; `setGroupActive(i)` is 1-based with `i <= 0` meaning
//!   the last member; a new window opened over a focused group joins it
//!   (`group:auto_group`, default true, `Window.cpp:1409`).
//! - pop-out: `local/vendor/omarchy/bin/omarchy-hyprland-window-pop` —
//!   float + resize 1300x900 + center + pin, and pinned means "shown on
//!   every workspace".
//! - scratchpad: `local/vendor/omarchy/default/hypr/qconsole.lua` — the
//!   special workspace is presented as a Quake console: full width, no
//!   gaps, no border, covering the TOP `share = 0.5` of the work area, and
//!   `binds.hide_special_on_workspace_change = true` closes it on a
//!   workspace change.
//!
//! Pure data structure — no Makepad types — so it unit-tests directly and
//! survives the move to the Wayland session backend unchanged.

pub type ClientId = u64;

pub const WORKSPACES: usize = 10;

/// The special workspace ("special:scratchpad"), stored after the numbered
/// ones so every by-index helper keeps working.
pub const SCRATCHPAD: usize = WORKSPACES;

/// `bin/omarchy-hyprland-window-pop` defaults: `width=${1:-1300}`,
/// `height=${2:-900}`.
pub const POP_W: f64 = 1300.0;
pub const POP_H: f64 = 900.0;

/// `qconsole.lua`: `local share = 0.5` — how much of the work area the
/// scratchpad console covers, measured from the top.
pub const SCRATCHPAD_SHARE: f64 = 0.5;

/// `Window.cpp:1451`: a tiled window's pseudo size is its mapped size minus
/// (10, 10), so SUPER+P visibly insets the window inside its slot.
pub const PSEUDO_INSET: f64 = 10.0;

/// A fresh float is centered at 60% of the desk (mpwm's own default; the
/// Hyprland equivalent is the client's last requested size).
pub const FLOAT_SHARE: f64 = 0.6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl LRect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    /// A rect of `w` x `h` centered inside `self`, clamped to fit.
    pub fn centered(&self, w: f64, h: f64) -> LRect {
        let w = w.min(self.w);
        let h = h.min(self.h);
        LRect::new(
            self.x + (self.w - w) * 0.5,
            self.y + (self.h - h) * 0.5,
            w,
            h,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Side by side (children left|right).
    Horizontal,
    /// Stacked (children top/bottom).
    Vertical,
}

/// How far past the gap on each side a divider band still grabs. The gap
/// between two tiles is `2 * gaps_in` (10px in omarchy), so the target is
/// 14px — comfortably clickable without reaching into either window.
/// Hyprland's border grab (`extend_border_grab_area`) is 15px for the same
/// reason; it just measures from the border instead of the gap.
pub const DIVIDER_SLOP: f64 = 2.0;

/// A grabbed divider: the split whose ratio a gap-drag moves, the box that
/// split divides, and the band the pointer found it by.
///
/// Pressing IN THE GAP between two tiles is Hyprland's `resize_on_border`
/// gesture scoped to the gap. On the border it fights every click near a
/// window edge, which is why omarchy ships `resize_on_border = false`
/// (`default/hypr/looknfeel.lua`); in the gap it needs no modifier and
/// collides with nothing — SUPER+drag still moves and swaps, SUPER+right-
/// drag still does the quadrant resize, and a plain press anywhere on a
/// window still belongs to that window.
///
/// `node` is an opaque handle into the tree. `set_divider_ratio` re-checks
/// it, so a hit that outlived its split is a no-op rather than a resize of
/// whatever took its slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DividerHit {
    node: usize,
    /// Horizontal = a left|right split, so the band is vertical and the
    /// pointer's x moves it.
    pub axis: Axis,
    /// The split's ratio when it was grabbed — a drag is measured from
    /// here, never accumulated frame by frame.
    pub ratio: f64,
    /// The box the split divides (both children plus the gap).
    pub rect: LRect,
    /// The grab band: the gap itself, widened by `DIVIDER_SLOP` on both
    /// sides, running the full length of the split.
    pub band: LRect,
    /// Depth in the tree, root = 0. The deepest band containing a point
    /// wins it.
    pub depth: usize,
}

impl DividerHit {
    fn new(node: usize, axis: Axis, ratio: f64, rect: LRect, gap: f64, depth: usize) -> Self {
        let band = match axis {
            Axis::Horizontal => {
                let aw = (rect.w - gap) * ratio;
                LRect::new(
                    rect.x + aw - DIVIDER_SLOP,
                    rect.y,
                    gap + DIVIDER_SLOP * 2.0,
                    rect.h,
                )
            }
            Axis::Vertical => {
                let ah = (rect.h - gap) * ratio;
                LRect::new(
                    rect.x,
                    rect.y + ah - DIVIDER_SLOP,
                    rect.w,
                    gap + DIVIDER_SLOP * 2.0,
                )
            }
        };
        Self {
            node,
            axis,
            ratio,
            rect,
            band,
            depth,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Omarchy binds three of Hyprland's fullscreen shapes (tiling.lua):
/// SUPER+F `fullscreen`, SUPER+ALT+F `maximized`, and SUPER+CTRL+F
/// `fullscreenstate 0 2` — the last one only *tells the client* it is
/// fullscreen and leaves the layout alone, so it lives on the client flag
/// set rather than here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FullscreenMode {
    #[default]
    None,
    /// SUPER+F: covers the whole desk, the bar hides with it.
    Fullscreen,
    /// SUPER+ALT+F ("Full width"): fills the tile area, the bar stays.
    Maximized,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloatEntry {
    pub client: ClientId,
    pub rect: LRect,
    /// Pinned windows show on every workspace (`window-pop`).
    pub pinned: bool,
    /// Home workspace; ignored while pinned.
    pub ws: usize,
}

/// A tabbed group: one tile slot holding N clients with one visible.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupInfo {
    pub clients: Vec<ClientId>,
    pub active: usize,
    pub rect: LRect,
}

#[derive(Clone, Debug)]
enum Node {
    Leaf {
        /// One client normally; a group holds several with one visible.
        clients: Vec<ClientId>,
        active: usize,
        /// `toggleGroup` on a lone window makes a group of one.
        grouped: bool,
    },
    Split {
        axis: Axis,
        /// Fraction of the area the first (left/top) child takes, 0.1..0.9.
        ratio: f64,
        a: usize,
        b: usize,
    },
}

/// Omarchy ships two tiling layouts and SUPER+L flips a workspace between
/// them (`omarchy-hyprland-workspace-layout-toggle`; `looknfeel.lua` sets
/// `layout = "dwindle"` with `scrolling.column_width = 0.49`). The
/// scrolling algorithm itself is not ported yet — this is the seam it
/// plugs into, and `rects_of` falls back to dwindle until it lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LayoutMode {
    #[default]
    Dwindle,
    /// niri-style side-scrolling columns.
    Scrolling,
}

/// `looknfeel.lua`: `scrolling = { column_width = 0.49 }`.
#[allow(dead_code)] // the scrolling algorithm's own lane reads it
pub const SCROLLING_COLUMN_WIDTH: f64 = 0.49;

#[derive(Clone, Debug, Default)]
pub struct Workspace {
    root: Option<usize>,
    /// Per-workspace, exactly like Hyprland's `layout` workspace rule.
    pub mode: LayoutMode,
    /// The focused client on this workspace (kept even while another
    /// workspace is active).
    pub focus: Option<ClientId>,
    /// Fullscreened client (see `fullscreen_mode`).
    pub fullscreen: Option<ClientId>,
    pub fullscreen_mode: FullscreenMode,
}

pub struct WmLayout {
    nodes: Vec<Option<Node>>,
    free: Vec<usize>,
    /// `WORKSPACES` numbered workspaces plus the scratchpad at `SCRATCHPAD`.
    pub workspaces: Vec<Workspace>,
    pub active: usize,
    /// The previously active workspace (SUPER+CTRL+TAB "former").
    pub former: usize,
    /// Focus history, most recent LAST (Hyprland's focus fallback):
    /// closing a window returns focus to the one focused before it, so
    /// repeated closes unwind in reverse creation/focus order.
    focus_history: Vec<ClientId>,
    /// Floating clients, back to front (the last one draws on top).
    floats: Vec<FloatEntry>,
    /// Where an unfloated window goes back to when it floats again.
    float_memory: Vec<(ClientId, LRect)>,
    /// The tiled NEIGHBOR a window sat next to when it floated/popped out,
    /// so un-floating returns it to where it came from instead of the
    /// current focus's split.
    tile_origin: Vec<(ClientId, ClientId)>,
    /// Pseudo-tiled clients and the natural size they keep.
    pseudo: Vec<(ClientId, f64, f64)>,
    /// `fullscreenstate 0 2`: reported to the client, no layout change.
    client_fullscreen: Vec<ClientId>,
    /// The scratchpad overlay is showing.
    pub scratchpad_open: bool,
    /// The true desk rect (the tile area before the outer gap). Fullscreen
    /// and the scratchpad console reach past the gap, so they need it; when
    /// it is unset the gap is added back to `area` instead.
    outer: Option<LRect>,
}

impl Default for WmLayout {
    fn default() -> Self {
        Self::new()
    }
}

impl WmLayout {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            free: Vec::new(),
            workspaces: (0..WORKSPACES + 1).map(|_| Workspace::default()).collect(),
            active: 0,
            former: 0,
            focus_history: Vec::new(),
            floats: Vec::new(),
            float_memory: Vec::new(),
            tile_origin: Vec::new(),
            pseudo: Vec::new(),
            client_fullscreen: Vec::new(),
            scratchpad_open: false,
            outer: None,
        }
    }

    /// The App hands us the desk rect each time it acts, since the tile
    /// area it passes to `rects` is already inset by the outer gap.
    pub fn set_outer(&mut self, rect: LRect) {
        self.outer = Some(rect);
    }

    fn alloc(&mut self, node: Node) -> usize {
        if let Some(idx) = self.free.pop() {
            self.nodes[idx] = Some(node);
            idx
        } else {
            self.nodes.push(Some(node));
            self.nodes.len() - 1
        }
    }

    fn dealloc(&mut self, idx: usize) {
        self.nodes[idx] = None;
        self.free.push(idx);
    }

    pub fn active_ws(&self) -> &Workspace {
        &self.workspaces[self.active]
    }

    /// The workspace every "act on the focused window" binding works in:
    /// the scratchpad while it is open and holds something, else the
    /// active workspace.
    pub fn focus_ws(&self) -> usize {
        if self.scratchpad_open && !self.clients_on(SCRATCHPAD).is_empty() {
            SCRATCHPAD
        } else {
            self.active
        }
    }

    pub fn focused_client(&self) -> Option<ClientId> {
        let ws = self.focus_ws();
        self.workspaces[ws]
            .focus
            .or_else(|| self.clients_on(ws).first().copied())
    }

    pub fn set_focus(&mut self, client: ClientId) {
        if let Some(ws) = self.workspace_of(client) {
            self.workspaces[ws].focus = Some(client);
        }
    }

    /// Which workspace holds a client, if any (floats included).
    pub fn workspace_of(&self, client: ClientId) -> Option<usize> {
        if let Some(f) = self.floats.iter().find(|f| f.client == client) {
            return Some(f.ws);
        }
        for (i, ws) in self.workspaces.iter().enumerate() {
            if let Some(root) = ws.root {
                if self.find_leaf(root, client).is_some() {
                    return Some(i);
                }
            }
        }
        None
    }

    /// Hyprland's `e+1`/`e-1`: the next OCCUPIED workspace in cyclic
    /// order (never a march through empty ones), the current workspace
    /// included in the ring. With no other occupied workspace this
    /// returns `from` — the cycle is a no-op, not a jump to nowhere.
    pub fn cycle_occupied(&self, from: usize, forward: bool) -> usize {
        for step in 1..=WORKSPACES {
            let ws = if forward {
                (from + step) % WORKSPACES
            } else {
                (from + WORKSPACES - step % WORKSPACES) % WORKSPACES
            };
            if ws == from {
                break;
            }
            if !self.clients_on(ws).is_empty() {
                return ws;
            }
        }
        from
    }

    /// Every client on a workspace — tiled (group members included) and
    /// floating — in tree order, floats last.
    pub fn clients_on(&self, ws: usize) -> Vec<ClientId> {
        let mut out = Vec::new();
        if let Some(root) = self.workspaces[ws].root {
            self.collect_clients(root, &mut out);
        }
        for f in &self.floats {
            if f.ws == ws {
                out.push(f.client);
            }
        }
        out
    }

    /// The clients a workspace actually shows: one per tile slot (a group
    /// shows its active member only) plus its floats.
    pub fn visible_clients_on(&self, ws: usize) -> Vec<ClientId> {
        let mut out = Vec::new();
        if let Some(root) = self.workspaces[ws].root {
            let mut slots = Vec::new();
            self.walk_slots(root, LRect::new(0.0, 0.0, 1.0, 1.0), 0.0, &mut slots);
            for (idx, _) in slots {
                if let Some(c) = self.leaf_visible(idx) {
                    out.push(c);
                }
            }
        }
        for f in &self.floats {
            if f.ws == ws || (f.pinned && ws < WORKSPACES) {
                out.push(f.client);
            }
        }
        out
    }

    fn collect_clients(&self, idx: usize, out: &mut Vec<ClientId>) {
        match self.nodes[idx].as_ref().unwrap() {
            Node::Leaf { clients, .. } => out.extend(clients.iter().copied()),
            Node::Split { a, b, .. } => {
                let (a, b) = (*a, *b);
                self.collect_clients(a, out);
                self.collect_clients(b, out);
            }
        }
    }

    fn leaf_visible(&self, idx: usize) -> Option<ClientId> {
        match self.nodes[idx].as_ref()? {
            Node::Leaf { clients, active, .. } => clients.get(*active).copied(),
            _ => None,
        }
    }

    fn find_leaf(&self, idx: usize, client: ClientId) -> Option<usize> {
        match self.nodes[idx].as_ref().unwrap() {
            Node::Leaf { clients, .. } => clients.contains(&client).then_some(idx),
            Node::Split { a, b, .. } => self
                .find_leaf(*a, client)
                .or_else(|| self.find_leaf(*b, client)),
        }
    }

    fn find_parent(&self, idx: usize, target: usize) -> Option<(usize, bool)> {
        match self.nodes[idx].as_ref().unwrap() {
            Node::Leaf { .. } => None,
            Node::Split { a, b, .. } => {
                if *a == target {
                    Some((idx, true))
                } else if *b == target {
                    Some((idx, false))
                } else {
                    let (a, b) = (*a, *b);
                    self.find_parent(a, target)
                        .or_else(|| self.find_parent(b, target))
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Insert / remove
    // ------------------------------------------------------------------

    /// Insert a client on the active workspace, splitting the focused leaf.
    /// `area` is the tile area (for axis choice on the split). When the
    /// focused slot is a group the client JOINS it (`group:auto_group`).
    pub fn insert(&mut self, client: ClientId, area: LRect, gap: f64) {
        self.insert_on(self.active, client, area, gap);
    }

    pub fn insert_on(&mut self, ws: usize, client: ClientId, area: LRect, gap: f64) {
        self.insert_at(ws, client, area, gap, true);
    }

    /// `auto_group = false` forces a real split even when the target slot
    /// is a group — how a window that just left a group takes a tile.
    fn insert_at(
        &mut self,
        ws: usize,
        client: ClientId,
        area: LRect,
        gap: f64,
        auto_group: bool,
    ) {
        let Some(root) = self.workspaces[ws].root else {
            let leaf = self.alloc(Node::Leaf {
                clients: vec![client],
                active: 0,
                grouped: false,
            });
            self.workspaces[ws].root = Some(leaf);
            self.workspaces[ws].focus = Some(client);
            return;
        };

        // Split at the focused leaf (or the last leaf if focus is floating
        // or gone).
        let target_client = self.workspaces[ws]
            .focus
            .filter(|c| self.find_leaf(root, *c).is_some())
            .or_else(|| {
                let mut tiled = Vec::new();
                self.collect_clients(root, &mut tiled);
                tiled.last().copied()
            });
        let Some(target_client) = target_client else {
            return;
        };
        let target = self.find_leaf(root, target_client).unwrap();

        // group:auto_group — a window opened over a focused group joins it.
        if auto_group {
            if let Some(Node::Leaf {
                clients,
                active,
                grouped: true,
            }) = self.nodes[target].as_mut()
            {
                clients.push(client);
                *active = clients.len() - 1;
                self.workspaces[ws].focus = Some(client);
                return;
            }
        }

        // Axis from the target leaf's current shape: wider => side-by-side.
        let rects = self.slot_rects_of(ws, area, gap);
        let target_rect = rects
            .iter()
            .find(|(c, _)| *c == target_client)
            .map(|(_, r)| *r)
            .unwrap_or(area);
        let axis = if target_rect.w > target_rect.h {
            Axis::Horizontal
        } else {
            Axis::Vertical
        };

        let old = self.nodes[target].take().unwrap();
        let old_leaf = self.alloc(old);
        let new_leaf = self.alloc(Node::Leaf {
            clients: vec![client],
            active: 0,
            grouped: false,
        });
        // force_split = 2: the new window goes right/bottom.
        self.nodes[target] = Some(Node::Split {
            axis,
            ratio: 0.5,
            a: old_leaf,
            b: new_leaf,
        });
        self.workspaces[ws].focus = Some(client);
    }

    /// Detach a client from the tree without touching float/pseudo state.
    /// Returns the workspace it came from.
    fn detach(&mut self, client: ClientId) -> Option<usize> {
        for ws_idx in 0..self.workspaces.len() {
            let Some(root) = self.workspaces[ws_idx].root else {
                continue;
            };
            let Some(leaf) = self.find_leaf(root, client) else {
                continue;
            };
            // A group member just leaves the tab strip.
            let mut collapse = false;
            if let Some(Node::Leaf { clients, active, .. }) = self.nodes[leaf].as_mut() {
                if clients.len() > 1 {
                    let pos = clients.iter().position(|c| *c == client).unwrap();
                    clients.remove(pos);
                    if *active >= clients.len() {
                        *active = clients.len() - 1;
                    } else if pos < *active {
                        *active -= 1;
                    }
                } else {
                    collapse = true;
                }
            }
            if !collapse {
                return Some(ws_idx);
            }
            if leaf == root {
                self.dealloc(leaf);
                self.workspaces[ws_idx].root = None;
            } else {
                let (parent, was_a) = self.find_parent(root, leaf).unwrap();
                let sibling = match self.nodes[parent].as_ref().unwrap() {
                    Node::Split { a, b, .. } => {
                        if was_a {
                            *b
                        } else {
                            *a
                        }
                    }
                    _ => unreachable!(),
                };
                let sibling_node = self.nodes[sibling].take().unwrap();
                self.free.push(sibling);
                self.nodes[parent] = Some(sibling_node);
                self.dealloc(leaf);
            }
            return Some(ws_idx);
        }
        None
    }

    /// Remove a client wherever it is; collapses its parent split.
    /// Record `client` as the most recently focused (call on every focus
    /// change — the close fallback walks this history newest-first).
    pub fn note_focus(&mut self, client: ClientId) {
        self.focus_history.retain(|c| *c != client);
        self.focus_history.push(client);
    }

    pub fn remove(&mut self, client: ClientId) {
        self.focus_history.retain(|c| *c != client);
        self.tile_origin.retain(|(c, _)| *c != client);
        self.floats.retain(|f| f.client != client);
        self.float_memory.retain(|(c, _)| *c != client);
        self.pseudo.retain(|(c, _, _)| *c != client);
        self.client_fullscreen.retain(|c| *c != client);
        let ws = self.detach(client);
        for i in 0..self.workspaces.len() {
            if self.workspaces[i].fullscreen == Some(client) {
                self.workspaces[i].fullscreen = None;
                self.workspaces[i].fullscreen_mode = FullscreenMode::None;
            }
            if self.workspaces[i].focus == Some(client) {
                // The previously focused survivor first (focus history,
                // newest-first) — repeated ⌘Q unwinds windows in reverse
                // creation/focus order instead of jumping to the oldest
                // tile. Tree order only as the last resort.
                let survivors = self.visible_clients_on(i);
                let from_history = self
                    .focus_history
                    .iter()
                    .rev()
                    .find(|c| survivors.contains(c))
                    .copied();
                self.workspaces[i].focus = from_history.or_else(|| survivors.first().copied());
            }
        }
        if ws == Some(SCRATCHPAD) && self.clients_on(SCRATCHPAD).is_empty() {
            self.scratchpad_open = false;
        }
    }

    // ------------------------------------------------------------------
    // Geometry
    // ------------------------------------------------------------------

    /// The raw tile slots of a workspace: one rect per leaf, showing the
    /// leaf's visible client. No pseudo, no fullscreen, no floats.
    pub fn slot_rects_of(&self, ws: usize, area: LRect, gap: f64) -> Vec<(ClientId, LRect)> {
        let mut out = Vec::new();
        if let Some(root) = self.workspaces[ws].root {
            let mut slots = Vec::new();
            self.walk_slots(root, area, gap, &mut slots);
            for (idx, rect) in slots {
                if let Some(client) = self.leaf_visible(idx) {
                    out.push((client, rect));
                }
            }
        }
        out
    }

    /// Everything the active workspace draws, back to front: tiles, then
    /// floats, then the scratchpad console.
    pub fn rects(&self, area: LRect, gap: f64) -> Vec<(ClientId, LRect)> {
        self.rects_of(self.active, area, gap)
    }

    pub fn rects_of(&self, ws: usize, area: LRect, gap: f64) -> Vec<(ClientId, LRect)> {
        let mut out = Vec::new();
        let workspace = &self.workspaces[ws];
        match (workspace.fullscreen, workspace.fullscreen_mode) {
            (Some(fs), FullscreenMode::Fullscreen) => {
                // The whole desk, gaps included — the caller hides the bar.
                out.push((fs, self.outer_rect(area, gap)));
            }
            (Some(fs), FullscreenMode::Maximized) => out.push((fs, area)),
            _ => {
                for (client, slot) in self.slot_rects_of(ws, area, gap) {
                    out.push((client, self.pseudo_rect(client, slot)));
                }
            }
        }
        for f in &self.floats {
            if f.ws == ws || (f.pinned && ws < WORKSPACES && f.ws != ws) {
                out.push((f.client, f.rect));
            }
        }
        if self.scratchpad_open && ws == self.active {
            self.scratchpad_rects(area, gap, &mut out);
        }
        out
    }

    /// The desk rect the caller inset by `gap` to get `area`.
    fn outer_rect(&self, area: LRect, gap: f64) -> LRect {
        if let Some(outer) = self.outer {
            return outer;
        }
        LRect::new(
            area.x - gap,
            area.y - gap,
            area.w + gap * 2.0,
            area.h + gap * 2.0,
        )
    }

    /// The Quake-console band the scratchpad drops into (qconsole.lua):
    /// full width, no outer gaps, the top `share` of the work area.
    pub fn scratchpad_area(&self, area: LRect, gap: f64) -> LRect {
        let outer = self.outer_rect(area, gap);
        LRect::new(
            outer.x,
            outer.y,
            outer.w,
            (outer.h * SCRATCHPAD_SHARE).floor(),
        )
    }

    fn scratchpad_rects(&self, area: LRect, gap: f64, out: &mut Vec<(ClientId, LRect)>) {
        let console = self.scratchpad_area(area, gap);
        // gaps_in = 0 inside the console.
        for (client, slot) in self.slot_rects_of(SCRATCHPAD, console, 0.0) {
            out.push((client, self.pseudo_rect(client, slot)));
        }
        for f in &self.floats {
            if f.ws == SCRATCHPAD {
                out.push((f.client, f.rect));
            }
        }
    }

    /// WindowTarget.cpp:208 — keep the natural size centered in the slot,
    /// scaled down uniformly when it does not fit.
    fn pseudo_rect(&self, client: ClientId, slot: LRect) -> LRect {
        let Some((_, nw, nh)) = self.pseudo.iter().copied().find(|(c, _, _)| *c == client) else {
            return slot;
        };
        let (w, h) = if nw > slot.w || nh > slot.h {
            let mut scale = 1.0;
            if nw > slot.w {
                scale = slot.w / nw;
            }
            if nh * scale > slot.h {
                scale = slot.h / nh;
            }
            (nw * scale, nh * scale)
        } else {
            (nw, nh)
        };
        LRect::new(
            slot.x + (slot.w - w) * 0.5,
            slot.y + (slot.h - h) * 0.5,
            w,
            h,
        )
    }

    fn walk_slots(&self, idx: usize, rect: LRect, gap: f64, out: &mut Vec<(usize, LRect)>) {
        match self.nodes[idx].as_ref().unwrap() {
            Node::Leaf { .. } => out.push((idx, rect)),
            Node::Split { axis, ratio, a, b } => {
                let (a, b, axis, ratio) = (*a, *b, *axis, *ratio);
                match axis {
                    Axis::Horizontal => {
                        let aw = (rect.w - gap) * ratio;
                        self.walk_slots(a, LRect::new(rect.x, rect.y, aw, rect.h), gap, out);
                        self.walk_slots(
                            b,
                            LRect::new(rect.x + aw + gap, rect.y, rect.w - aw - gap, rect.h),
                            gap,
                            out,
                        );
                    }
                    Axis::Vertical => {
                        let ah = (rect.h - gap) * ratio;
                        self.walk_slots(a, LRect::new(rect.x, rect.y, rect.w, ah), gap, out);
                        self.walk_slots(
                            b,
                            LRect::new(rect.x, rect.y + ah + gap, rect.w, rect.h - ah - gap),
                            gap,
                            out,
                        );
                    }
                }
            }
        }
    }

    /// The rect a client currently draws at, if it is on screen.
    pub fn rect_of(&self, client: ClientId, area: LRect, gap: f64) -> Option<LRect> {
        self.rects(area, gap)
            .into_iter()
            .find(|(c, _)| *c == client)
            .map(|(_, r)| r)
    }

    /// Topmost client under a point (floats and the scratchpad win, since
    /// `rects` is back to front).
    pub fn client_at(&self, x: f64, y: f64, area: LRect, gap: f64) -> Option<ClientId> {
        self.rects(area, gap)
            .into_iter()
            .rev()
            .find(|(_, r)| r.contains(x, y))
            .map(|(c, _)| c)
    }

    // ------------------------------------------------------------------
    // Focus / swap
    // ------------------------------------------------------------------

    /// Move focus in a direction, spatially (nearest rect center) — floats
    /// and the scratchpad included, since they are in `rects`.
    pub fn focus_dir(&mut self, dir: Dir, area: LRect, gap: f64) -> bool {
        let Some(next) = self.neighbor(dir, area, gap) else {
            return false;
        };
        let ws = self.focus_ws();
        self.workspaces[ws].focus = Some(next);
        true
    }

    pub fn neighbor(&self, dir: Dir, area: LRect, gap: f64) -> Option<ClientId> {
        let focus = self.focused_client()?;
        let rects = self.rects(area, gap);
        let (_, from) = rects.iter().find(|(c, _)| *c == focus)?;
        let (fx, fy) = from.center();
        rects
            .iter()
            .filter(|(c, _)| *c != focus)
            .filter(|(_, r)| {
                let (cx, cy) = r.center();
                match dir {
                    Dir::Left => cx < fx - 1.0,
                    Dir::Right => cx > fx + 1.0,
                    Dir::Up => cy < fy - 1.0,
                    Dir::Down => cy > fy + 1.0,
                }
            })
            .min_by(|(_, r1), (_, r2)| {
                let d = |r: &LRect| {
                    let (cx, cy) = r.center();
                    // Distance weighted against the off-axis, so straight
                    // neighbors win over diagonal ones.
                    match dir {
                        Dir::Left | Dir::Right => (cx - fx).abs() + (cy - fy).abs() * 3.0,
                        Dir::Up | Dir::Down => (cy - fy).abs() + (cx - fx).abs() * 3.0,
                    }
                };
                d(r1).partial_cmp(&d(r2)).unwrap()
            })
            .map(|(c, _)| *c)
    }

    /// Swap the focused client with its neighbor in a direction. Tiled
    /// windows only (Hyprland's `swapwindow` ignores floats).
    pub fn swap_dir(&mut self, dir: Dir, area: LRect, gap: f64) -> bool {
        let Some(focus) = self.focused_client() else {
            return false;
        };
        let Some(other) = self.neighbor(dir, area, gap) else {
            return false;
        };
        self.swap_clients(focus, other)
    }

    /// Exchange two tiled clients' positions in the tree.
    pub fn swap_clients(&mut self, a: ClientId, b: ClientId) -> bool {
        if a == b || self.is_float(a) || self.is_float(b) {
            return false;
        }
        let (Some(wa), Some(wb)) = (self.workspace_of(a), self.workspace_of(b)) else {
            return false;
        };
        let (Some(ra), Some(rb)) = (self.workspaces[wa].root, self.workspaces[wb].root) else {
            return false;
        };
        let (Some(la), Some(lb)) = (self.find_leaf(ra, a), self.find_leaf(rb, b)) else {
            return false;
        };
        if la == lb {
            return false;
        }
        self.replace_in_leaf(la, a, b);
        self.replace_in_leaf(lb, b, a);
        true
    }

    fn replace_in_leaf(&mut self, leaf: usize, from: ClientId, to: ClientId) {
        if let Some(Node::Leaf { clients, .. }) = self.nodes[leaf].as_mut() {
            if let Some(slot) = clients.iter_mut().find(|c| **c == from) {
                *slot = to;
            }
        }
    }

    /// Toggle the split axis of the focused leaf's parent (SUPER+J).
    pub fn toggle_split(&mut self) -> bool {
        let ws = self.focus_ws();
        let (Some(root), Some(focus)) = (self.workspaces[ws].root, self.focused_client()) else {
            return false;
        };
        let Some(leaf) = self.find_leaf(root, focus) else {
            return false;
        };
        if leaf == root {
            return false;
        }
        let (parent, _) = self.find_parent(root, leaf).unwrap();
        if let Some(Node::Split { axis, .. }) = self.nodes[parent].as_mut() {
            *axis = match *axis {
                Axis::Horizontal => Axis::Vertical,
                Axis::Vertical => Axis::Horizontal,
            };
            return true;
        }
        false
    }

    // ------------------------------------------------------------------
    // Resize
    // ------------------------------------------------------------------

    /// Resize by a ratio delta: move the nearest ancestor divider on
    /// `axis`. Positive grows the first (left/top) child, exactly like
    /// Hyprland's `splitRatio += Δ`.
    pub fn resize(&mut self, axis: Axis, delta: f64) -> bool {
        let Some((parent, _)) = self.resize_target(axis) else {
            return false;
        };
        if let Some(Node::Split { ratio, .. }) = self.nodes[parent].as_mut() {
            *ratio = (*ratio + delta).clamp(0.1, 0.9);
            return true;
        }
        false
    }

    /// Omarchy's `resize({ x = ±100, relative = true })`: move the nearest
    /// divider on `axis` by `px` pixels (positive = right/down), converting
    /// against that divider's own box the way `resizeTarget` does.
    pub fn resize_px(&mut self, axis: Axis, px: f64, area: LRect, gap: f64) -> bool {
        let Some((parent, rect)) = self.resize_target_rect(axis, area, gap) else {
            return false;
        };
        let span = match axis {
            Axis::Horizontal => rect.w - gap,
            Axis::Vertical => rect.h - gap,
        };
        if span <= 1.0 {
            return false;
        }
        if let Some(Node::Split { ratio, .. }) = self.nodes[parent].as_mut() {
            *ratio = (*ratio + px / span).clamp(0.1, 0.9);
            return true;
        }
        false
    }

    /// The nearest ancestor split of the focused leaf on `axis`.
    fn resize_target(&self, axis: Axis) -> Option<(usize, bool)> {
        let ws = self.focus_ws();
        let root = self.workspaces[ws].root?;
        let focus = self.focused_client()?;
        let mut cur = self.find_leaf(root, focus)?;
        while cur != root {
            let (parent, was_a) = self.find_parent(root, cur)?;
            if let Some(Node::Split { axis: pa, .. }) = self.nodes[parent].as_ref() {
                if *pa == axis {
                    return Some((parent, was_a));
                }
            }
            cur = parent;
        }
        None
    }

    fn resize_target_rect(&self, axis: Axis, area: LRect, gap: f64) -> Option<(usize, LRect)> {
        let (parent, _) = self.resize_target(axis)?;
        let ws = self.focus_ws();
        let root = self.workspaces[ws].root?;
        let base = if ws == SCRATCHPAD {
            self.scratchpad_area(area, gap)
        } else {
            area
        };
        let gap = if ws == SCRATCHPAD { 0.0 } else { gap };
        let mut found = None;
        self.walk_node_rects(root, base, gap, 0, &mut |idx, _depth, rect| {
            if idx == parent {
                found = Some(rect);
            }
        });
        found.map(|rect| (parent, rect))
    }

    fn walk_node_rects(
        &self,
        idx: usize,
        rect: LRect,
        gap: f64,
        depth: usize,
        f: &mut impl FnMut(usize, usize, LRect),
    ) {
        f(idx, depth, rect);
        if let Some(Node::Split { axis, ratio, a, b }) = self.nodes[idx].as_ref() {
            let (a, b, axis, ratio) = (*a, *b, *axis, *ratio);
            match axis {
                Axis::Horizontal => {
                    let aw = (rect.w - gap) * ratio;
                    self.walk_node_rects(
                        a,
                        LRect::new(rect.x, rect.y, aw, rect.h),
                        gap,
                        depth + 1,
                        f,
                    );
                    self.walk_node_rects(
                        b,
                        LRect::new(rect.x + aw + gap, rect.y, rect.w - aw - gap, rect.h),
                        gap,
                        depth + 1,
                        f,
                    );
                }
                Axis::Vertical => {
                    let ah = (rect.h - gap) * ratio;
                    self.walk_node_rects(
                        a,
                        LRect::new(rect.x, rect.y, rect.w, ah),
                        gap,
                        depth + 1,
                        f,
                    );
                    self.walk_node_rects(
                        b,
                        LRect::new(rect.x, rect.y + ah + gap, rect.w, rect.h - ah - gap),
                        gap,
                        depth + 1,
                        f,
                    );
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Dividers (dragging IN THE GAP)
    // ------------------------------------------------------------------

    /// The deepest divider band containing a point on the active
    /// workspace, or None.
    pub fn divider_at(&self, x: f64, y: f64, area: LRect, gap: f64) -> Option<DividerHit> {
        self.divider_at_on(self.active, x, y, area, gap)
    }

    pub fn divider_at_on(
        &self,
        ws: usize,
        x: f64,
        y: f64,
        area: LRect,
        gap: f64,
    ) -> Option<DividerHit> {
        // A fullscreened window has no gaps to grab, and the scratchpad
        // console covers whatever is under it (its own tiles are drawn
        // with gaps_in = 0, so there is no band inside it either).
        if self.workspaces[ws].fullscreen.is_some() {
            return None;
        }
        if self.scratchpad_open
            && ws == self.active
            && self.scratchpad_area(area, gap).contains(x, y)
        {
            return None;
        }
        let root = self.workspaces[ws].root?;
        let mut best: Option<DividerHit> = None;
        self.walk_node_rects(root, area, gap, 0, &mut |idx, depth, rect| {
            let Some(Node::Split { axis, ratio, .. }) = self.nodes[idx].as_ref() else {
                return;
            };
            let hit = DividerHit::new(idx, *axis, *ratio, rect, gap, depth);
            // Deepest wins: a nested divider's band crosses its parent's,
            // and the one the eye reads as "this gap" is the inner one.
            if hit.band.contains(x, y) && best.map_or(true, |b| depth >= b.depth) {
                best = Some(hit);
            }
        });
        best
    }

    /// Every client the two sides of a grabbed divider hold — the tiles
    /// that resize with it, which the desk snaps instead of tweening for
    /// the length of the drag.
    pub fn clients_under(&self, hit: &DividerHit) -> Vec<ClientId> {
        let mut out = Vec::new();
        if matches!(self.nodes.get(hit.node), Some(Some(Node::Split { .. }))) {
            self.collect_clients(hit.node, &mut out);
        }
        out
    }

    /// Put a grabbed divider at `ratio`, clamped the way Hyprland clamps a
    /// split (0.1..0.9 of the span). False when the handle no longer names
    /// that split.
    pub fn set_divider_ratio(&mut self, hit: &DividerHit, ratio: f64) -> bool {
        match self.nodes.get_mut(hit.node) {
            Some(Some(Node::Split {
                axis, ratio: r, ..
            })) if *axis == hit.axis => {
                *r = ratio.clamp(0.1, 0.9);
                true
            }
            _ => false,
        }
    }

    /// Move a grabbed divider `px` pixels off where it was grabbed
    /// (positive = right/down), converting against the split's OWN box
    /// exactly like `resize_px` does. Measured from `hit.ratio` rather
    /// than accumulated per frame, so the divider tracks the pointer 1:1
    /// with no drift and a clamp never eats part of the way back.
    pub fn drag_divider_px(&mut self, hit: &DividerHit, px: f64, gap: f64) -> bool {
        let span = match hit.axis {
            Axis::Horizontal => hit.rect.w - gap,
            Axis::Vertical => hit.rect.h - gap,
        };
        if span <= 1.0 {
            return false;
        }
        self.set_divider_ratio(hit, hit.ratio + px / span)
    }

    // ------------------------------------------------------------------
    // Floating / pinning / pop-out
    // ------------------------------------------------------------------

    pub fn is_float(&self, client: ClientId) -> bool {
        self.floats.iter().any(|f| f.client == client)
    }

    pub fn is_pinned(&self, client: ClientId) -> bool {
        self.floats.iter().any(|f| f.client == client && f.pinned)
    }

    pub fn float_rect(&self, client: ClientId) -> Option<LRect> {
        self.floats
            .iter()
            .find(|f| f.client == client)
            .map(|f| f.rect)
    }

    pub fn set_float_rect(&mut self, client: ClientId, rect: LRect) {
        if let Some(f) = self.floats.iter_mut().find(|f| f.client == client) {
            f.rect = rect;
        }
    }

    pub fn floats(&self) -> &[FloatEntry] {
        &self.floats
    }

    /// Raise a float to the top of the stack (`alterzorder top`).
    pub fn raise_float(&mut self, client: ClientId) {
        if let Some(i) = self.floats.iter().position(|f| f.client == client) {
            let entry = self.floats.remove(i);
            self.floats.push(entry);
        }
    }

    /// SUPER+T — `hl.dsp.window.float({ action = "toggle" })`.
    pub fn toggle_float(&mut self, client: ClientId, area: LRect, gap: f64) -> bool {
        if self.is_float(client) {
            self.unfloat(client, area, gap);
        } else {
            self.float(client, None, area, gap);
        }
        true
    }

    /// Put a client straight into the float stack without ever giving it a
    /// tile — a Quick-Look preview popup.
    pub fn add_float(&mut self, client: ClientId, rect: LRect, ws: usize) {
        self.floats.retain(|f| f.client != client);
        self.floats.push(FloatEntry {
            client,
            rect,
            pinned: false,
            ws,
        });
        self.workspaces[ws].focus = Some(client);
    }

    /// A centered popup rect for a preview, clamped into the desk.
    pub fn popup_rect(&self, area: LRect, gap: f64, w: f64, h: f64) -> LRect {
        let _ = gap;
        area.centered(w, h)
    }

    fn float(&mut self, client: ClientId, rect: Option<LRect>, area: LRect, gap: f64) {
        let ws = self.workspace_of(client).unwrap_or(self.active);
        let base = if ws == SCRATCHPAD {
            self.scratchpad_area(area, gap)
        } else {
            area
        };
        let rect = rect
            .or_else(|| {
                self.float_memory
                    .iter()
                    .find(|(c, _)| *c == client)
                    .map(|(_, r)| *r)
            })
            .unwrap_or_else(|| base.centered(base.w * FLOAT_SHARE, base.h * FLOAT_SHARE));
        // Remember the tiled neighbor so a later unfloat returns HERE.
        self.tile_origin.retain(|(c, _)| *c != client);
        if let Some(neighbor) = self.tiled_sibling_of(client) {
            self.tile_origin.push((client, neighbor));
        }
        self.detach(client);
        if self.workspaces[ws].fullscreen == Some(client) {
            self.workspaces[ws].fullscreen = None;
            self.workspaces[ws].fullscreen_mode = FullscreenMode::None;
        }
        self.floats.push(FloatEntry {
            client,
            rect,
            pinned: false,
            ws,
        });
        self.workspaces[ws].focus = Some(client);
    }

    fn unfloat(&mut self, client: ClientId, area: LRect, gap: f64) {
        let Some(i) = self.floats.iter().position(|f| f.client == client) else {
            return;
        };
        let entry = self.floats.remove(i);
        self.float_memory.retain(|(c, _)| *c != client);
        self.float_memory.push((client, entry.rect));
        // Back where it came from: split the remembered old neighbor if it
        // is still tiled on that workspace; the current focus only as the
        // fallback (insert_on's default).
        let origin = self
            .tile_origin
            .iter()
            .find(|(c, _)| *c == client)
            .map(|(_, n)| *n)
            .filter(|n| {
                self.workspace_of(*n) == Some(entry.ws) && !self.is_float(*n)
            });
        if let Some(neighbor) = origin {
            let kept = self.workspaces[entry.ws].focus;
            self.workspaces[entry.ws].focus = Some(neighbor);
            self.insert_on(entry.ws, client, area, gap);
            let _ = kept; // insert focuses the returning client itself
        } else {
            self.insert_on(entry.ws, client, area, gap);
        }
        self.tile_origin.retain(|(c, _)| *c != client);
    }

    /// The nearest tiled neighbor of a tiled client: the first client of
    /// its leaf's SIBLING subtree (None for floats or a lone tile).
    fn tiled_sibling_of(&self, client: ClientId) -> Option<ClientId> {
        let ws = self.workspace_of(client)?;
        let root = self.workspaces[ws].root?;
        let leaf = self.find_leaf(root, client)?;
        let (parent, client_is_a) = self.find_parent(root, leaf)?;
        let Some(Node::Split { a, b, .. }) = self.nodes[parent].as_ref() else {
            return None;
        };
        let sibling = if client_is_a { *b } else { *a };
        let mut out = Vec::new();
        self.collect_clients(sibling, &mut out);
        out.into_iter().find(|c| *c != client)
    }

    /// SUPER+O — `bin/omarchy-hyprland-window-pop`: float + 1300x900 +
    /// center + pin; pinned again means unpin and tile back.
    pub fn pop_out(&mut self, client: ClientId, area: LRect, gap: f64) {
        if self.is_pinned(client) {
            if let Some(f) = self.floats.iter_mut().find(|f| f.client == client) {
                f.pinned = false;
            }
            self.unfloat(client, area, gap);
            return;
        }
        let ws = self.workspace_of(client).unwrap_or(self.active);
        let base = if ws == SCRATCHPAD {
            self.scratchpad_area(area, gap)
        } else {
            area
        };
        let rect = base.centered(POP_W, POP_H);
        if self.is_float(client) {
            self.set_float_rect(client, rect);
        } else {
            self.float(client, Some(rect), area, gap);
        }
        if let Some(f) = self.floats.iter_mut().find(|f| f.client == client) {
            f.pinned = true;
        }
        self.raise_float(client);
        self.workspaces[ws].focus = Some(client);
    }

    // ------------------------------------------------------------------
    // Pseudo
    // ------------------------------------------------------------------

    pub fn is_pseudo(&self, client: ClientId) -> bool {
        self.pseudo.iter().any(|(c, _, _)| *c == client)
    }

    /// SUPER+P. The natural size is the slot size at the moment of the
    /// toggle minus (10, 10), matching Hyprland's map-time pseudo size.
    pub fn toggle_pseudo(&mut self, client: ClientId, area: LRect, gap: f64) -> bool {
        if self.is_pseudo(client) {
            self.pseudo.retain(|(c, _, _)| *c != client);
            return true;
        }
        let ws = match self.workspace_of(client) {
            Some(ws) => ws,
            None => return false,
        };
        if self.is_float(client) {
            return false;
        }
        let (base, gap) = if ws == SCRATCHPAD {
            (self.scratchpad_area(area, gap), 0.0)
        } else {
            (area, gap)
        };
        let Some((_, slot)) = self
            .slot_rects_of(ws, base, gap)
            .into_iter()
            .find(|(c, _)| *c == client)
        else {
            return false;
        };
        self.pseudo.push((
            client,
            (slot.w - PSEUDO_INSET).max(1.0),
            (slot.h - PSEUDO_INSET).max(1.0),
        ));
        true
    }

    // ------------------------------------------------------------------
    // Fullscreen
    // ------------------------------------------------------------------

    /// SUPER+F / SUPER+ALT+F. Toggling the same mode clears it; a different
    /// mode replaces it.
    pub fn toggle_fullscreen_mode(&mut self, mode: FullscreenMode) {
        let ws = self.focus_ws();
        let Some(focus) = self.focused_client() else {
            return;
        };
        if self.is_float(focus) {
            return;
        }
        let w = &mut self.workspaces[ws];
        if w.fullscreen == Some(focus) && w.fullscreen_mode == mode {
            w.fullscreen = None;
            w.fullscreen_mode = FullscreenMode::None;
        } else {
            w.fullscreen = Some(focus);
            w.fullscreen_mode = mode;
        }
    }

    /// Kept for the old call site: SUPER+F.
    pub fn toggle_fullscreen(&mut self) {
        self.toggle_fullscreen_mode(FullscreenMode::Fullscreen);
    }

    pub fn fullscreen_mode(&self) -> FullscreenMode {
        self.workspaces[self.active].fullscreen_mode
    }

    /// SUPER+CTRL+F — `fullscreenstate 0 2`: the client is TOLD it is
    /// fullscreen, the layout does not change.
    pub fn toggle_client_fullscreen(&mut self, client: ClientId) -> bool {
        if let Some(i) = self.client_fullscreen.iter().position(|c| *c == client) {
            self.client_fullscreen.remove(i);
            false
        } else {
            self.client_fullscreen.push(client);
            true
        }
    }

    pub fn is_client_fullscreen(&self, client: ClientId) -> bool {
        self.client_fullscreen.contains(&client)
    }

    // ------------------------------------------------------------------
    // Groups
    // ------------------------------------------------------------------

    /// Group membership of a client: (members, active index).
    pub fn group_of(&self, client: ClientId) -> Option<(Vec<ClientId>, usize)> {
        let ws = self.workspace_of(client)?;
        let root = self.workspaces[ws].root?;
        let leaf = self.find_leaf(root, client)?;
        match self.nodes[leaf].as_ref()? {
            Node::Leaf {
                clients,
                active,
                grouped: true,
            } => Some((clients.clone(), *active)),
            _ => None,
        }
    }

    /// Every group on a workspace with the slot it occupies — the look lane
    /// draws tab bars from this.
    pub fn groups_of(&self, ws: usize, area: LRect, gap: f64) -> Vec<GroupInfo> {
        let mut out = Vec::new();
        let Some(root) = self.workspaces[ws].root else {
            return out;
        };
        let mut slots = Vec::new();
        self.walk_slots(root, area, gap, &mut slots);
        for (idx, rect) in slots {
            if let Some(Node::Leaf {
                clients,
                active,
                grouped: true,
            }) = self.nodes[idx].as_ref()
            {
                out.push(GroupInfo {
                    clients: clients.clone(),
                    active: *active,
                    rect,
                });
            }
        }
        out
    }

    pub fn groups(&self, area: LRect, gap: f64) -> Vec<GroupInfo> {
        self.groups_of(self.active, area, gap)
    }

    fn focused_leaf(&self) -> Option<usize> {
        let ws = self.focus_ws();
        let root = self.workspaces[ws].root?;
        let focus = self.focused_client()?;
        self.find_leaf(root, focus)
    }

    /// SUPER+G — `toggleGroup`: make a group of one, or destroy the group
    /// and re-tile every member.
    pub fn toggle_group(&mut self, area: LRect, gap: f64) -> bool {
        let ws = self.focus_ws();
        let Some(leaf) = self.focused_leaf() else {
            return false;
        };
        let (members, active, grouped) = match self.nodes[leaf].as_ref() {
            Some(Node::Leaf {
                clients,
                active,
                grouped,
            }) => (clients.clone(), *active, *grouped),
            _ => return false,
        };
        if !grouped {
            if let Some(Node::Leaf { grouped, .. }) = self.nodes[leaf].as_mut() {
                *grouped = true;
            }
            return true;
        }
        // Destroy: the visible member keeps the slot, the rest re-tile.
        let keep = members[active];
        if let Some(Node::Leaf {
            clients,
            active,
            grouped,
        }) = self.nodes[leaf].as_mut()
        {
            *clients = vec![keep];
            *active = 0;
            *grouped = false;
        }
        self.workspaces[ws].focus = Some(keep);
        for c in members.into_iter().filter(|c| *c != keep) {
            self.insert_on(ws, c, area, gap);
        }
        self.workspaces[ws].focus = Some(keep);
        true
    }

    /// SUPER+ALT+G — `moveOutOfGroup`: the focused member leaves the tab
    /// strip and takes a tile of its own.
    pub fn move_out_of_group(&mut self, area: LRect, gap: f64) -> bool {
        let ws = self.focus_ws();
        let Some(focus) = self.focused_client() else {
            return false;
        };
        let Some((members, _)) = self.group_of(focus) else {
            return false;
        };
        if members.len() < 2 {
            return false;
        }
        self.detach(focus);
        // Focus stays inside the group (group:focus_removed_window = false),
        // so insert next to it and then hand focus back.
        let stay = self
            .group_of_any(&members, focus)
            .unwrap_or(members[0]);
        self.workspaces[ws].focus = Some(stay);
        // Never straight back into the group it just left.
        self.insert_at(ws, focus, area, gap, false);
        self.workspaces[ws].focus = Some(stay);
        true
    }

    fn group_of_any(&self, members: &[ClientId], not: ClientId) -> Option<ClientId> {
        members.iter().copied().find(|c| *c != not)
    }

    /// SUPER+ALT+<arrow> — `moveIntoGroup`: only lands when the neighbor in
    /// that direction is itself a group (ConfigActions.cpp:1369).
    pub fn move_into_group(&mut self, dir: Dir, area: LRect, gap: f64) -> bool {
        let ws = self.focus_ws();
        let Some(focus) = self.focused_client() else {
            return false;
        };
        let Some(target) = self.neighbor(dir, area, gap) else {
            return false;
        };
        if self.group_of(target).is_none() || self.is_float(focus) {
            return false;
        }
        let Some(root) = self.workspaces[ws].root else {
            return false;
        };
        let Some(target_leaf) = self.find_leaf(root, target) else {
            return false;
        };
        if self.find_leaf(root, focus) == Some(target_leaf) {
            return false;
        }
        self.detach(focus);
        // The tree may have collapsed; find the target's leaf again.
        let Some(root) = self.workspaces[ws].root else {
            return false;
        };
        let Some(target_leaf) = self.find_leaf(root, target) else {
            return false;
        };
        if let Some(Node::Leaf { clients, active, .. }) = self.nodes[target_leaf].as_mut() {
            clients.push(focus);
            *active = clients.len() - 1;
        }
        self.workspaces[ws].focus = Some(focus);
        true
    }

    /// A tiled window DROPPED on another with SHIFT held: it joins that
    /// tile as a tab instead of swapping with it. A target that is already
    /// a group gains a member; a plain tile becomes a group of two. The
    /// dragged window's own leaf leaves the tree, so its slot re-tiles, and
    /// the drop lands ACTIVE — you see what you just dropped.
    pub fn group_drop(&mut self, dragged: ClientId, target: ClientId) -> bool {
        if dragged == target || self.is_float(dragged) || self.is_float(target) {
            return false;
        }
        let Some(ws) = self.workspace_of(target) else {
            return false;
        };
        let Some(root) = self.workspaces[ws].root else {
            return false;
        };
        let Some(target_leaf) = self.find_leaf(root, target) else {
            return false;
        };
        // Already tabs of the same leaf: the drop has nothing to do.
        if self.find_leaf(root, dragged) == Some(target_leaf) {
            return false;
        }
        self.detach(dragged);
        // `detach` collapses the split it emptied, so every leaf index the
        // tree handed out a moment ago may have moved: look the target up
        // again rather than reusing `target_leaf`.
        let Some(root) = self.workspaces[ws].root else {
            return false;
        };
        let Some(target_leaf) = self.find_leaf(root, target) else {
            return false;
        };
        let Some(Node::Leaf {
            clients,
            active,
            grouped,
        }) = self.nodes[target_leaf].as_mut()
        else {
            return false;
        };
        clients.push(dragged);
        *active = clients.len() - 1;
        *grouped = true;
        self.workspaces[ws].focus = Some(dragged);
        true
    }

    /// A groupbar tab dragged OFF its strip: that member leaves the tab
    /// strip and takes a tile of its own beside the group, which is
    /// `move_out_of_group`'s tree move for an arbitrary member instead of
    /// the focused one. Focus follows the torn-out window — it is the one
    /// under the pointer. A group left holding a single tab is no longer a
    /// group; it goes back to being a plain tile.
    pub fn group_tear_out(&mut self, client: ClientId, area: LRect, gap: f64) -> bool {
        let Some((members, _)) = self.group_of(client) else {
            return false;
        };
        if members.len() < 2 {
            return false;
        }
        let Some(ws) = self.workspace_of(client) else {
            return false;
        };
        self.detach(client);
        // Insert beside a member that stayed, with auto_group off so it can
        // never fall straight back into the strip it just left.
        let stay = self.group_of_any(&members, client).unwrap_or(members[0]);
        self.workspaces[ws].focus = Some(stay);
        self.insert_at(ws, client, area, gap, false);
        self.dissolve_lone_group(ws, stay);
        self.workspaces[ws].focus = Some(client);
        true
    }

    /// A tab strip with one tab left is just a window.
    fn dissolve_lone_group(&mut self, ws: usize, member: ClientId) {
        let Some(root) = self.workspaces[ws].root else {
            return;
        };
        let Some(leaf) = self.find_leaf(root, member) else {
            return;
        };
        if let Some(Node::Leaf {
            clients,
            active,
            grouped,
        }) = self.nodes[leaf].as_mut()
        {
            if *grouped && clients.len() <= 1 {
                *grouped = false;
                *active = 0;
            }
        }
    }

    /// SUPER+ALT+TAB / SUPER+CTRL+RIGHT — `changeGroupActive`.
    pub fn group_cycle(&mut self, forward: bool) -> bool {
        let ws = self.focus_ws();
        let Some(leaf) = self.focused_leaf() else {
            return false;
        };
        let next = match self.nodes[leaf].as_mut() {
            Some(Node::Leaf {
                clients,
                active,
                grouped: true,
            }) if clients.len() > 1 => {
                *active = if forward {
                    (*active + 1) % clients.len()
                } else {
                    (*active + clients.len() - 1) % clients.len()
                };
                clients[*active]
            }
            _ => return false,
        };
        self.workspaces[ws].focus = Some(next);
        true
    }

    /// SUPER+ALT+1..5 — `setGroupActive`: 1-based, `index <= 0` selects the
    /// last member, out of range is a no-op.
    pub fn group_set_active(&mut self, index: usize) -> bool {
        let ws = self.focus_ws();
        let Some(leaf) = self.focused_leaf() else {
            return false;
        };
        let next = match self.nodes[leaf].as_mut() {
            Some(Node::Leaf {
                clients,
                active,
                grouped: true,
            }) if clients.len() > 1 => {
                if index == 0 {
                    *active = clients.len() - 1;
                } else if index > clients.len() {
                    return false;
                } else {
                    *active = index - 1;
                }
                clients[*active]
            }
            _ => return false,
        };
        self.workspaces[ws].focus = Some(next);
        true
    }

    // ------------------------------------------------------------------
    // Workspaces / scratchpad
    // ------------------------------------------------------------------

    pub fn switch_workspace(&mut self, n: usize) {
        if n < WORKSPACES && n != self.active {
            self.former = self.active;
            self.active = n;
            // binds.hide_special_on_workspace_change = true.
            self.scratchpad_open = false;
        }
    }

    /// Move the focused client to workspace `n` (follow = switch too).
    pub fn move_focused_to(&mut self, n: usize, follow: bool) {
        self.move_focused_to_ex(n, follow, LRect::new(0.0, 0.0, 1000.0, 600.0), 0.0);
    }

    pub fn move_focused_to_ex(&mut self, n: usize, follow: bool, area: LRect, gap: f64) {
        if n >= self.workspaces.len() {
            return;
        }
        let from = self.focus_ws();
        if n == from {
            return;
        }
        let Some(focus) = self.focused_client() else {
            return;
        };
        if let Some(f) = self.floats.iter_mut().find(|f| f.client == focus) {
            f.ws = n;
            f.pinned = false;
            self.workspaces[from].focus = self.visible_clients_on(from).first().copied();
            self.workspaces[n].focus = Some(focus);
        } else {
            self.detach(focus);
            if self.workspaces[from].fullscreen == Some(focus) {
                self.workspaces[from].fullscreen = None;
                self.workspaces[from].fullscreen_mode = FullscreenMode::None;
            }
            self.workspaces[from].focus = self.visible_clients_on(from).first().copied();
            let base = if n == SCRATCHPAD {
                self.scratchpad_area(area, gap)
            } else {
                area
            };
            let gap = if n == SCRATCHPAD { 0.0 } else { gap };
            self.insert_on(n, focus, base, gap);
        }
        if from == SCRATCHPAD && self.clients_on(SCRATCHPAD).is_empty() {
            self.scratchpad_open = false;
        }
        if follow {
            self.switch_workspace(n);
        }
    }

    /// SUPER+L — flip the focused workspace's tiling layout. The
    /// scrolling algorithm is not written yet; the flag is carried and
    /// `rects_of` keeps drawing dwindle until it is.
    pub fn toggle_workspace_layout(&mut self) -> LayoutMode {
        let ws = self.focus_ws();
        let mode = match self.workspaces[ws].mode {
            LayoutMode::Dwindle => LayoutMode::Scrolling,
            LayoutMode::Scrolling => LayoutMode::Dwindle,
        };
        self.workspaces[ws].mode = mode;
        mode
    }

    pub fn layout_mode(&self) -> LayoutMode {
        self.workspaces[self.focus_ws()].mode
    }

    /// SUPER+S / SUPER+grave — `toggle_special("scratchpad")`.
    pub fn toggle_scratchpad(&mut self) {
        self.scratchpad_open = !self.scratchpad_open;
        if self.scratchpad_open && self.workspaces[SCRATCHPAD].focus.is_none() {
            self.workspaces[SCRATCHPAD].focus =
                self.visible_clients_on(SCRATCHPAD).first().copied();
        }
    }

    /// SUPER+ALT+S / SUPER+SHIFT+grave — move there, do not follow.
    pub fn move_focused_to_scratchpad(&mut self, area: LRect, gap: f64) {
        self.move_focused_to_ex(SCRATCHPAD, false, area, gap);
    }

    /// Cycle focus to the next/previous visible client (ALT+TAB).
    pub fn cycle_focus(&mut self, forward: bool) {
        let ws = self.focus_ws();
        let clients = self.visible_clients_on(ws);
        if clients.is_empty() {
            return;
        }
        let cur = self
            .workspaces[ws]
            .focus
            .and_then(|f| clients.iter().position(|c| *c == f))
            .unwrap_or(0);
        let next = if forward {
            (cur + 1) % clients.len()
        } else {
            (cur + clients.len() - 1) % clients.len()
        };
        let next = clients[next];
        self.workspaces[ws].focus = Some(next);
        // ALT+TAB also "reveals the active window on top" (tiling.lua).
        if self.is_float(next) {
            self.raise_float(next);
        }
    }

    /// Every client mpwm knows about, on every workspace.
    pub fn all_clients(&self) -> Vec<ClientId> {
        let mut out = Vec::new();
        for ws in 0..self.workspaces.len() {
            for c in self.clients_on(ws) {
                if !out.contains(&c) {
                    out.push(c);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: LRect = LRect {
        x: 0.0,
        y: 0.0,
        w: 1000.0,
        h: 600.0,
    };

    fn abc() -> WmLayout {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.insert(2, AREA, 0.0);
        l.insert(3, AREA, 0.0);
        l
    }

    fn rect_of(l: &WmLayout, c: ClientId) -> LRect {
        rect_gap(l, c, 0.0)
    }

    /// Divider maths runs through a ratio, so a moved edge lands on its
    /// pixel within float noise rather than on the bit.
    fn assert_px(actual: f64, expect: f64) {
        assert!(
            (actual - expect).abs() < 1e-9,
            "{} is not {}",
            actual,
            expect
        );
    }

    /// The same, drawn with a real gap — what the divider tests measure,
    /// since a divider IS the gap.
    fn rect_gap(l: &WmLayout, c: ClientId, gap: f64) -> LRect {
        l.rects(AREA, gap)
            .into_iter()
            .find(|(x, _)| *x == c)
            .unwrap()
            .1
    }

    #[test]
    fn dwindle_a_b_c() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.insert(2, AREA, 0.0);
        // A|B side by side (area wider than tall).
        let r = l.rects(AREA, 0.0);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].0, 1);
        assert!(r[0].1.w == 500.0 && r[0].1.h == 600.0);
        assert!(r[1].1.x == 500.0);
        // C splits B (focused): B's rect is 500x600, taller than wide
        // => vertical split, C below.
        l.insert(3, AREA, 0.0);
        let r = l.rects(AREA, 0.0);
        assert_eq!(r.len(), 3);
        let rb = r.iter().find(|(c, _)| *c == 2).unwrap().1;
        let rc = r.iter().find(|(c, _)| *c == 3).unwrap().1;
        assert_eq!(rb, LRect::new(500.0, 0.0, 500.0, 300.0));
        assert_eq!(rc, LRect::new(500.0, 300.0, 500.0, 300.0));
    }

    #[test]
    fn remove_collapses() {
        let mut l = abc();
        l.remove(2);
        let r = l.rects(AREA, 0.0);
        assert_eq!(r.len(), 2);
        // 3 takes B's whole half.
        assert_eq!(rect_of(&l, 3), LRect::new(500.0, 0.0, 500.0, 600.0));
        l.remove(1);
        let r = l.rects(AREA, 0.0);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].1, AREA);
        l.remove(3);
        assert!(l.rects(AREA, 0.0).is_empty());
    }

    #[test]
    fn focus_and_swap() {
        let mut l = abc();
        assert_eq!(l.focused_client(), Some(3));
        assert!(l.focus_dir(Dir::Up, AREA, 0.0));
        assert_eq!(l.focused_client(), Some(2));
        assert!(l.focus_dir(Dir::Left, AREA, 0.0));
        assert_eq!(l.focused_client(), Some(1));
        assert!(!l.focus_dir(Dir::Left, AREA, 0.0));
        // Swap right: 1 <-> 2.
        assert!(l.swap_dir(Dir::Right, AREA, 0.0));
        let r = l.rects(AREA, 0.0);
        assert_eq!(r[0].0, 2);
    }

    #[test]
    fn toggle_split_flips_axis() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.insert(2, AREA, 0.0);
        assert!(l.toggle_split());
        let r = l.rects(AREA, 0.0);
        // Now stacked.
        assert_eq!(r[0].1, LRect::new(0.0, 0.0, 1000.0, 300.0));
        assert_eq!(r[1].1, LRect::new(0.0, 300.0, 1000.0, 300.0));
    }

    #[test]
    fn resize_moves_the_divider_absolutely() {
        // DwindleAlgorithm::resizeTarget: splitRatio += Δ*2/box.w — the
        // divider moves right for a positive Δ whichever side is focused.
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.insert(2, AREA, 0.0);
        assert!(l.resize(Axis::Horizontal, 0.1));
        assert_eq!(rect_of(&l, 1).w, 600.0);
        assert_eq!(rect_of(&l, 2).w, 400.0);
    }

    #[test]
    fn pop_out_returns_to_its_old_neighbor() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.insert(2, AREA, 0.0); // splits 1 → neighbor of 2 is 1
        l.insert(3, AREA, 0.0); // splits 2
        // 3's nearest neighbor is 2 (they share the deepest split). Pop 3
        // out, move focus to 1, pop back in: 3 must return NEXT TO 2 —
        // not split the focused 1.
        assert_eq!(l.tiled_sibling_of(3), Some(2));
        l.pop_out(3, AREA, 0.0);
        l.workspaces[0].focus = Some(1);
        l.pop_out(3, AREA, 0.0);
        assert!(!l.is_float(3));
        assert_eq!(l.tiled_sibling_of(3), Some(2));
    }

    #[test]
    fn closing_unwinds_in_reverse_focus_order() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.insert(2, AREA, 0.0);
        l.insert(3, AREA, 0.0);
        // Created (and focused) 1 → 2 → 3.
        l.note_focus(1);
        l.note_focus(2);
        l.note_focus(3);
        l.remove(3);
        assert_eq!(l.workspaces[0].focus, Some(2));
        l.remove(2);
        assert_eq!(l.workspaces[0].focus, Some(1));
        // Refocusing an older window reorders the unwind.
        let mut l = WmLayout::new();
        for c in 1..=3 {
            l.insert(c, AREA, 0.0);
            l.note_focus(c);
        }
        l.note_focus(1); // user went back to 1
        l.workspaces[0].focus = Some(1);
        l.remove(1);
        assert_eq!(l.workspaces[0].focus, Some(3));
    }

    #[test]
    fn workspace_cycle_visits_only_occupied() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0); // ws 0
        l.insert_on(4, 2, AREA, 0.0); // ws 4
        // From 0 forward: skip 1..3 (empty), land on 4; again wraps to 0.
        assert_eq!(l.cycle_occupied(0, true), 4);
        assert_eq!(l.cycle_occupied(4, true), 0);
        // Backward from 0 wraps to 4 directly.
        assert_eq!(l.cycle_occupied(0, false), 4);
        // From an EMPTY workspace (the "cleared screen" case) either
        // direction reaches an occupied one — Tab always brings you back.
        assert_eq!(l.cycle_occupied(2, true), 4);
        assert_eq!(l.cycle_occupied(2, false), 0);
        // A lone occupied workspace cycles to itself.
        let mut solo = WmLayout::new();
        solo.insert(9, AREA, 0.0);
        assert_eq!(solo.cycle_occupied(0, true), 0);
    }

    #[test]
    fn resize_px_moves_by_pixels() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.insert(2, AREA, 0.0);
        // SUPER+equals: x = +100 => the divider moves 100px right.
        assert!(l.resize_px(Axis::Horizontal, 100.0, AREA, 0.0));
        assert_eq!(rect_of(&l, 1).w, 600.0);
        assert_eq!(rect_of(&l, 2).w, 400.0);
        // SUPER+minus twice: back past the start by 100.
        assert!(l.resize_px(Axis::Horizontal, -100.0, AREA, 0.0));
        assert!(l.resize_px(Axis::Horizontal, -100.0, AREA, 0.0));
        assert_eq!(rect_of(&l, 1).w, 400.0);
        // Vertical has no divider here.
        assert!(!l.resize_px(Axis::Vertical, 100.0, AREA, 0.0));
        // ...but it does once C stacks under B.
        l.insert(3, AREA, 0.0);
        assert!(l.resize_px(Axis::Vertical, 60.0, AREA, 0.0));
        assert_eq!(rect_of(&l, 2).h, 360.0);
        assert_eq!(rect_of(&l, 3).h, 240.0);
    }

    #[test]
    fn resize_px_uses_the_dividers_own_box() {
        // The nested divider spans 500px, so 50px is a tenth of it.
        let mut l = abc();
        assert!(l.resize_px(Axis::Vertical, 30.0, AREA, 0.0));
        assert_eq!(rect_of(&l, 2).h, 330.0);
    }

    #[test]
    fn workspaces_and_move() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.insert(2, AREA, 0.0);
        l.move_focused_to(1, true);
        assert_eq!(l.active, 1);
        assert_eq!(l.clients_on(1), vec![2]);
        assert_eq!(l.clients_on(0), vec![1]);
        l.switch_workspace(0);
        assert_eq!(l.former, 1);
        assert_eq!(l.focused_client(), Some(1));
    }

    #[test]
    fn fullscreen_modes() {
        let mut l = abc();
        // SUPER+ALT+F: maximized fills the tile area, gaps kept.
        l.toggle_fullscreen_mode(FullscreenMode::Maximized);
        let r = l.rects(AREA, 8.0);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0], (3, AREA));
        // SUPER+F from there: fullscreen covers the desk, gaps included.
        l.toggle_fullscreen_mode(FullscreenMode::Fullscreen);
        let r = l.rects(AREA, 8.0);
        assert_eq!(r[0].1, LRect::new(-8.0, -8.0, 1016.0, 616.0));
        // Same mode again clears it.
        l.toggle_fullscreen_mode(FullscreenMode::Fullscreen);
        assert_eq!(l.rects(AREA, 0.0).len(), 3);
    }

    #[test]
    fn client_fullscreen_is_a_flag_only() {
        let mut l = abc();
        assert!(l.toggle_client_fullscreen(3));
        assert!(l.is_client_fullscreen(3));
        // fullscreenstate 0 2 does not touch the layout.
        assert_eq!(l.rects(AREA, 0.0).len(), 3);
        assert!(!l.toggle_client_fullscreen(3));
        assert!(!l.is_client_fullscreen(3));
    }

    #[test]
    fn gaps_apply() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.insert(2, AREA, 0.0);
        let r = l.rects(AREA, 10.0);
        assert_eq!(r[0].1.w, 495.0);
        assert_eq!(r[1].1.x, 505.0);
        assert_eq!(r[1].1.w, 495.0);
    }

    // --------------------------------------------------------------
    // Floating
    // --------------------------------------------------------------

    #[test]
    fn float_leaves_the_tree_and_draws_on_top() {
        let mut l = abc();
        assert!(l.toggle_float(3, AREA, 0.0));
        assert!(l.is_float(3));
        let r = l.rects(AREA, 0.0);
        // Two tiles + the float, the float last (drawn above).
        assert_eq!(r.len(), 3);
        assert_eq!(r[2].0, 3);
        // 2 took the whole right half back.
        assert_eq!(rect_of(&l, 2), LRect::new(500.0, 0.0, 500.0, 600.0));
        // Centered 60% of the desk.
        assert_eq!(r[2].1, LRect::new(200.0, 120.0, 600.0, 360.0));
        // Unfloat reinserts it at the focus.
        l.toggle_float(3, AREA, 0.0);
        assert!(!l.is_float(3));
        assert_eq!(l.rects(AREA, 0.0).len(), 3);
        assert!(l.rects(AREA, 0.0).iter().all(|(_, r)| r.w < 1000.0));
    }

    #[test]
    fn float_focus_is_spatial() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.insert(2, AREA, 0.0);
        l.toggle_float(2, AREA, 0.0);
        l.set_float_rect(2, LRect::new(700.0, 0.0, 200.0, 200.0));
        l.set_focus(1);
        assert!(l.focus_dir(Dir::Right, AREA, 0.0));
        assert_eq!(l.focused_client(), Some(2));
    }

    #[test]
    fn alt_tab_cycles_through_floats_too() {
        let mut l = abc();
        l.toggle_float(3, AREA, 0.0);
        // Tiles first, floats last: 1, 2, then the float.
        assert_eq!(l.visible_clients_on(0), vec![1, 2, 3]);
        l.set_focus(1);
        l.cycle_focus(true);
        assert_eq!(l.focused_client(), Some(2));
        l.cycle_focus(true);
        assert_eq!(l.focused_client(), Some(3));
        l.cycle_focus(true);
        assert_eq!(l.focused_client(), Some(1));
        l.cycle_focus(false);
        assert_eq!(l.focused_client(), Some(3));
    }

    #[test]
    fn a_float_can_be_dragged_and_stays_where_it_is_put() {
        let mut l = abc();
        l.toggle_float(3, AREA, 0.0);
        l.set_float_rect(3, LRect::new(40.0, 50.0, 300.0, 200.0));
        assert_eq!(rect_of(&l, 3), LRect::new(40.0, 50.0, 300.0, 200.0));
        // Unfloat and float again: it comes back where it was left.
        l.toggle_float(3, AREA, 0.0);
        l.toggle_float(3, AREA, 0.0);
        assert_eq!(l.float_rect(3), Some(LRect::new(40.0, 50.0, 300.0, 200.0)));
    }

    #[test]
    fn a_preview_float_never_takes_a_tile() {
        let mut l = abc();
        l.add_float(9, LRect::new(100.0, 100.0, 400.0, 300.0), 0);
        assert!(l.is_float(9));
        let r = l.rects(AREA, 0.0);
        assert_eq!(r.len(), 4);
        assert_eq!(r[3].0, 9);
        assert_eq!(l.focused_client(), Some(9));
        l.remove(9);
        assert_eq!(l.rects(AREA, 0.0).len(), 3);
    }

    #[test]
    fn pop_out_floats_pins_and_toggles_back() {
        let mut l = abc();
        l.pop_out(3, AREA, 0.0);
        assert!(l.is_float(3));
        assert!(l.is_pinned(3));
        // 1300x900 clamped into a 1000x600 desk, centered.
        assert_eq!(l.float_rect(3), Some(LRect::new(0.0, 0.0, 1000.0, 600.0)));
        // Pinned windows show on every workspace.
        l.switch_workspace(4);
        assert!(l.rects(AREA, 0.0).iter().any(|(c, _)| *c == 3));
        l.switch_workspace(0);
        // Again: unpin and tile back.
        l.pop_out(3, AREA, 0.0);
        assert!(!l.is_float(3));
        assert!(!l.is_pinned(3));
        assert_eq!(l.rects(AREA, 0.0).len(), 3);
    }

    // --------------------------------------------------------------
    // Pseudo
    // --------------------------------------------------------------

    #[test]
    fn pseudo_keeps_its_size_centered_and_scales_down() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.insert(2, AREA, 0.0);
        // 2's slot is 500x600 => natural size 490x590, centered.
        assert!(l.toggle_pseudo(2, AREA, 0.0));
        assert_eq!(rect_of(&l, 2), LRect::new(505.0, 5.0, 490.0, 590.0));
        // Shrink the slot to 250x600: uniform scale 250/490, centered.
        l.set_focus(2);
        l.resize_px(Axis::Horizontal, 250.0, AREA, 0.0);
        let slot = LRect::new(750.0, 0.0, 250.0, 600.0);
        let scale = 250.0 / 490.0;
        let (w, h) = (490.0 * scale, 590.0 * scale);
        let r = rect_of(&l, 2);
        assert!((r.w - w).abs() < 1e-9 && (r.h - h).abs() < 1e-9);
        assert!((r.x - (slot.x + (slot.w - w) * 0.5)).abs() < 1e-9);
        assert!((r.y - (slot.y + (slot.h - h) * 0.5)).abs() < 1e-9);
        // Off again.
        assert!(l.toggle_pseudo(2, AREA, 0.0));
        assert!(!l.is_pseudo(2));
        assert_eq!(rect_of(&l, 2), slot);
    }

    // --------------------------------------------------------------
    // Groups
    // --------------------------------------------------------------

    #[test]
    fn group_holds_many_and_shows_one() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.insert(2, AREA, 0.0);
        // Group 2, then open 3 over it: auto_group puts 3 in the group.
        assert!(l.toggle_group(AREA, 0.0));
        l.insert(3, AREA, 0.0);
        assert_eq!(l.group_of(2), Some((vec![2, 3], 1)));
        let r = l.rects(AREA, 0.0);
        assert_eq!(r.len(), 2);
        assert_eq!(r[1], (3, LRect::new(500.0, 0.0, 500.0, 600.0)));
        // The group info the look lane draws tabs from.
        let g = l.groups(AREA, 0.0);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].clients, vec![2, 3]);
        assert_eq!(g[0].active, 1);
        assert_eq!(g[0].rect, LRect::new(500.0, 0.0, 500.0, 600.0));
    }

    #[test]
    fn group_cycle_and_select() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.toggle_group(AREA, 0.0);
        l.insert(2, AREA, 0.0);
        l.insert(3, AREA, 0.0);
        assert_eq!(l.group_of(1), Some((vec![1, 2, 3], 2)));
        assert!(l.group_cycle(true));
        assert_eq!(l.focused_client(), Some(1));
        assert!(l.group_cycle(false));
        assert_eq!(l.focused_client(), Some(3));
        // setGroupActive is 1-based; 0 means the last member.
        assert!(l.group_set_active(2));
        assert_eq!(l.focused_client(), Some(2));
        assert!(l.group_set_active(0));
        assert_eq!(l.focused_client(), Some(3));
        assert!(!l.group_set_active(9));
        assert_eq!(l.focused_client(), Some(3));
        // Only the active member draws.
        assert_eq!(l.rects(AREA, 0.0), vec![(3, AREA)]);
    }

    #[test]
    fn group_destroy_retiles_every_member() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.toggle_group(AREA, 0.0);
        l.insert(2, AREA, 0.0);
        l.insert(3, AREA, 0.0);
        assert!(l.toggle_group(AREA, 0.0));
        assert!(l.group_of(1).is_none());
        assert_eq!(l.rects(AREA, 0.0).len(), 3);
        assert_eq!(l.focused_client(), Some(3));
    }

    #[test]
    fn move_out_of_group_takes_its_own_tile() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.toggle_group(AREA, 0.0);
        l.insert(2, AREA, 0.0);
        assert_eq!(l.group_of(1), Some((vec![1, 2], 1)));
        assert!(l.move_out_of_group(AREA, 0.0));
        assert_eq!(l.group_of(1), Some((vec![1], 0)));
        assert!(l.group_of(2).is_none());
        assert_eq!(l.rects(AREA, 0.0).len(), 2);
        // Focus stays with the group (group:focus_removed_window = false).
        assert_eq!(l.focused_client(), Some(1));
    }

    #[test]
    fn a_shift_drop_makes_a_group_of_two() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.insert(2, AREA, 0.0);
        // Neither is a group: the drop turns the TARGET into one.
        assert!(l.group_drop(1, 2));
        assert_eq!(l.group_of(2), Some((vec![2, 1], 1)));
        // One tile left, showing what was dropped, and it has the focus.
        assert_eq!(l.rects(AREA, 0.0), vec![(1, AREA)]);
        assert_eq!(l.focused_client(), Some(1));
    }

    #[test]
    fn a_shift_drop_onto_a_group_adds_a_tab() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.toggle_group(AREA, 0.0);
        l.insert(2, AREA, 0.0);
        l.insert(3, AREA, 0.0);
        // 3 leaves the group and takes its own tile, then is dropped back.
        assert!(l.move_out_of_group(AREA, 0.0));
        assert_eq!(l.group_of(1), Some((vec![1, 2], 1)));
        assert!(l.group_drop(3, 2));
        assert_eq!(l.group_of(1), Some((vec![1, 2, 3], 2)));
        assert_eq!(l.rects(AREA, 0.0), vec![(3, AREA)]);
    }

    #[test]
    fn a_shift_drop_on_itself_or_its_own_tab_does_nothing() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.insert(2, AREA, 0.0);
        assert!(!l.group_drop(1, 1));
        assert_eq!(l.rects(AREA, 0.0).len(), 2);
        // Two tabs of one group: dropping one on the other changes nothing.
        assert!(l.group_drop(1, 2));
        assert!(!l.group_drop(1, 2));
        assert_eq!(l.group_of(2), Some((vec![2, 1], 1)));
    }

    #[test]
    fn tearing_a_tab_out_gives_it_a_tile() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.toggle_group(AREA, 0.0);
        l.insert(2, AREA, 0.0);
        l.insert(3, AREA, 0.0);
        assert_eq!(l.group_of(1), Some((vec![1, 2, 3], 2)));
        // Tear out the MIDDLE tab — not the focused one.
        assert!(l.group_tear_out(2, AREA, 0.0));
        assert_eq!(l.group_of(1), Some((vec![1, 3], 1)));
        assert!(l.group_of(2).is_none());
        // Two tiles now: the group and the torn-out window, which keeps
        // the focus because it is the one under the pointer.
        assert_eq!(l.rects(AREA, 0.0).len(), 2);
        assert_eq!(l.focused_client(), Some(2));
    }

    #[test]
    fn the_last_tab_out_dissolves_the_group() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.toggle_group(AREA, 0.0);
        l.insert(2, AREA, 0.0);
        assert!(l.group_tear_out(2, AREA, 0.0));
        // What is left is a plain tile, not a one-tab strip.
        assert!(l.group_of(1).is_none());
        assert!(l.groups(AREA, 0.0).is_empty());
        assert_eq!(l.rects(AREA, 0.0).len(), 2);
        assert_eq!(l.focused_client(), Some(2));
        // Nothing to tear out of a window that is not grouped.
        assert!(!l.group_tear_out(1, AREA, 0.0));
        assert!(!l.group_tear_out(2, AREA, 0.0));
    }

    #[test]
    fn move_into_group_only_lands_on_a_group() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.insert(2, AREA, 0.0);
        // 2 is not a group yet: nothing happens.
        l.set_focus(1);
        assert!(!l.move_into_group(Dir::Right, AREA, 0.0));
        // Make 2 a group and try again.
        l.set_focus(2);
        l.toggle_group(AREA, 0.0);
        l.set_focus(1);
        assert!(l.move_into_group(Dir::Right, AREA, 0.0));
        assert_eq!(l.group_of(2), Some((vec![2, 1], 1)));
        assert_eq!(l.rects(AREA, 0.0), vec![(1, AREA)]);
    }

    #[test]
    fn removing_a_group_member_keeps_the_slot() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 0.0);
        l.insert(2, AREA, 0.0);
        l.toggle_group(AREA, 0.0);
        l.insert(3, AREA, 0.0);
        assert_eq!(l.group_of(2), Some((vec![2, 3], 1)));
        l.remove(3);
        assert_eq!(l.group_of(2), Some((vec![2], 0)));
        assert_eq!(l.rects(AREA, 0.0).len(), 2);
    }

    // --------------------------------------------------------------
    // Scratchpad
    // --------------------------------------------------------------

    #[test]
    fn scratchpad_moves_silently_and_overlays_the_top_half() {
        let mut l = abc();
        l.set_focus(3);
        l.move_focused_to_scratchpad(AREA, 0.0);
        // Gone from workspace 0, and no follow.
        assert_eq!(l.active, 0);
        assert!(!l.clients_on(0).contains(&3));
        assert_eq!(l.clients_on(SCRATCHPAD), vec![3]);
        assert_eq!(l.rects(AREA, 0.0).len(), 2);
        // Toggle it on: the Quake console band, over the workspace.
        l.toggle_scratchpad();
        let r = l.rects(AREA, 0.0);
        assert_eq!(r.len(), 3);
        assert_eq!(r[2], (3, LRect::new(0.0, 0.0, 1000.0, 300.0)));
        // While open, the scratchpad owns the focus.
        assert_eq!(l.focused_client(), Some(3));
        // A workspace change hides it (hide_special_on_workspace_change).
        l.switch_workspace(1);
        assert!(!l.scratchpad_open);
    }

    #[test]
    fn scratchpad_closes_when_its_last_window_dies() {
        let mut l = abc();
        l.move_focused_to_scratchpad(AREA, 0.0);
        l.toggle_scratchpad();
        assert!(l.scratchpad_open);
        l.remove(3);
        assert!(!l.scratchpad_open);
        // Focus falls back to the workspace underneath.
        assert_eq!(l.focused_client(), Some(1));
    }

    #[test]
    fn move_to_workspace_silently_keeps_the_view() {
        let mut l = abc();
        l.move_focused_to_ex(2, false, AREA, 0.0);
        assert_eq!(l.active, 0);
        assert_eq!(l.clients_on(2), vec![3]);
        assert_eq!(l.rects(AREA, 0.0).len(), 2);
    }

    // --------------------------------------------------------------
    // Dividers (dragging IN THE GAP)
    // --------------------------------------------------------------

    /// The band is the gap itself plus `DIVIDER_SLOP` on each side, and
    /// nothing wider: a press one pixel past the slop belongs to the
    /// window, not to the divider.
    #[test]
    fn a_divider_band_is_the_gap_plus_the_slop() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 10.0);
        l.insert(2, AREA, 10.0);
        // ratio 0.5 of (1000 - 10): tiles are 0..495 and 505..1000, so the
        // gap is 495..505 and the band 493..507.
        let hit = l.divider_at(500.0, 300.0, AREA, 10.0).unwrap();
        assert_eq!(hit.axis, Axis::Horizontal);
        assert_eq!(hit.ratio, 0.5);
        assert_eq!(hit.rect, AREA);
        assert_eq!(hit.band, LRect::new(493.0, 0.0, 14.0, 600.0));
        assert_eq!(hit.depth, 0);
        // The slop grabs, one pixel past it does not.
        assert!(l.divider_at(493.0, 5.0, AREA, 10.0).is_some());
        assert!(l.divider_at(506.9, 595.0, AREA, 10.0).is_some());
        assert!(l.divider_at(492.0, 300.0, AREA, 10.0).is_none());
        assert!(l.divider_at(508.0, 300.0, AREA, 10.0).is_none());
        // Both tiles resize with it.
        assert_eq!(l.clients_under(&hit), vec![1, 2]);
        // A lone window has no divider at all.
        l.remove(2);
        assert!(l.divider_at(500.0, 300.0, AREA, 10.0).is_none());
    }

    /// A drag tracks the pointer 1:1: the divider's own edge moves by
    /// exactly the pixels the pointer did, on either axis.
    #[test]
    fn dragging_a_divider_moves_it_one_to_one() {
        let mut l = WmLayout::new();
        l.insert(1, AREA, 10.0);
        l.insert(2, AREA, 10.0);
        let hit = l.divider_at(500.0, 300.0, AREA, 10.0).unwrap();
        assert!(l.drag_divider_px(&hit, 200.0, 10.0));
        // 495 + 200: the first tile's right edge is where the pointer is.
        assert_px(rect_gap(&l, 1, 10.0).w, 695.0);
        assert_px(rect_gap(&l, 2, 10.0).x, 705.0);
        // Measured from the GRAB, not accumulated: the same hit replayed
        // with a smaller delta lands where that delta says, not 200 + it.
        assert!(l.drag_divider_px(&hit, 50.0, 10.0));
        assert_px(rect_gap(&l, 1, 10.0).w, 545.0);
        // ...and back to nothing.
        assert!(l.drag_divider_px(&hit, 0.0, 10.0));
        assert_px(rect_gap(&l, 1, 10.0).w, 495.0);
        // Hyprland's clamp: a drag past the end stops at 0.1/0.9 of the
        // span and never inverts the split.
        assert!(l.drag_divider_px(&hit, 5000.0, 10.0));
        assert_px(rect_gap(&l, 1, 10.0).w, 891.0);
        assert!(l.drag_divider_px(&hit, -5000.0, 10.0));
        assert_px(rect_gap(&l, 1, 10.0).w, 99.0);
        // set_divider_ratio clamps the same way.
        assert!(l.set_divider_ratio(&hit, 0.25));
        assert_px(rect_gap(&l, 1, 10.0).w, 247.5);
    }

    /// Nested splits: the DEEPEST band containing the point wins, and each
    /// divider converts against its own box.
    #[test]
    fn nested_dividers_go_to_the_deepest_band() {
        // 1 | (2 / 3): a horizontal root and a vertical split on the right.
        let l0 = abc();
        let hit_v = l0.divider_at(700.0, 300.0, AREA, 10.0).unwrap();
        assert_eq!(hit_v.axis, Axis::Vertical);
        assert_eq!(hit_v.depth, 1);
        // Its box is the RIGHT half, not the desk.
        assert_eq!(hit_v.rect, LRect::new(505.0, 0.0, 495.0, 600.0));
        assert_eq!(hit_v.band, LRect::new(505.0, 293.0, 495.0, 14.0));
        assert_eq!(l0.clients_under(&hit_v), vec![2, 3]);
        // Well clear of the vertical band, the root's band answers.
        let hit_h = l0.divider_at(500.0, 100.0, AREA, 10.0).unwrap();
        assert_eq!(hit_h.axis, Axis::Horizontal);
        assert_eq!(hit_h.depth, 0);
        assert_eq!(l0.clients_under(&hit_h), vec![1, 2, 3]);
        // Where the two bands CROSS, the deeper one takes it.
        let cross = l0.divider_at(505.0, 300.0, AREA, 10.0).unwrap();
        assert_eq!(cross.axis, Axis::Vertical);
        assert_eq!(cross.depth, 1);
        // Neither band: no divider.
        assert!(l0.divider_at(700.0, 100.0, AREA, 10.0).is_none());
        assert!(l0.divider_at(200.0, 400.0, AREA, 10.0).is_none());

        // Dragging the nested one converts against its own 600px-tall box.
        let mut l = abc();
        assert!(l.drag_divider_px(&hit_v, 60.0, 10.0));
        assert_px(rect_gap(&l, 2, 10.0).h, 355.0);
        assert_px(rect_gap(&l, 3, 10.0).y, 365.0);
        assert_px(rect_gap(&l, 3, 10.0).h, 235.0);
        // ...and the root one leaves the nested ratio alone.
        assert!(l.drag_divider_px(&hit_h, -100.0, 10.0));
        assert_px(rect_gap(&l, 1, 10.0).w, 395.0);
        assert_px(rect_gap(&l, 2, 10.0).x, 405.0);
        assert_px(rect_gap(&l, 2, 10.0).h, 355.0);
    }

    /// Four clients, three dividers, both axes — every gap grabs the split
    /// that draws it.
    #[test]
    fn every_gap_of_a_four_way_grabs_its_own_split() {
        // 1 | (2 / (3 | 4)) — insert splits the focused leaf each time.
        let mut l = abc();
        l.insert(4, AREA, 10.0);
        // 3's slot (505..1000, 305..600) split side by side: 3 left, 4
        // right of a vertical gap at x = 505 + (495-10)*0.5 = 747.5.
        let deep = l.divider_at(750.0, 450.0, AREA, 10.0).unwrap();
        assert_eq!(deep.axis, Axis::Horizontal);
        assert_eq!(deep.depth, 2);
        assert_eq!(l.clients_under(&deep), vec![3, 4]);
        // The three bands are distinct splits.
        let root = l.divider_at(500.0, 100.0, AREA, 10.0).unwrap();
        let mid = l.divider_at(600.0, 300.0, AREA, 10.0).unwrap();
        assert_eq!((root.depth, root.axis), (0, Axis::Horizontal));
        assert_eq!((mid.depth, mid.axis), (1, Axis::Vertical));
        assert_ne!(l.clients_under(&root), l.clients_under(&mid));
        // Moving the deepest one moves only 3 and 4.
        let before_2 = rect_gap(&l, 2, 10.0);
        assert!(l.drag_divider_px(&deep, -100.0, 10.0));
        assert_px(rect_gap(&l, 3, 10.0).w, 142.5);
        assert_px(rect_gap(&l, 4, 10.0).x, 657.5);
        assert_eq!(rect_gap(&l, 2, 10.0), before_2);
    }

    /// Handles are re-checked: one that outlived its split does nothing,
    /// and a covered desk offers no divider at all.
    #[test]
    fn a_stale_or_covered_divider_is_a_no_op() {
        let mut l = abc();
        let hit = l.divider_at(700.0, 300.0, AREA, 10.0).unwrap();
        // The split collapses when one side goes.
        l.remove(3);
        assert!(!l.set_divider_ratio(&hit, 0.8));
        assert!(!l.drag_divider_px(&hit, 100.0, 10.0));
        assert_eq!(rect_gap(&l, 2, 10.0), LRect::new(505.0, 0.0, 495.0, 600.0));
        // Fullscreen covers the gaps.
        let mut l = abc();
        assert!(l.divider_at(500.0, 100.0, AREA, 10.0).is_some());
        l.toggle_fullscreen_mode(FullscreenMode::Fullscreen);
        assert!(l.divider_at(500.0, 100.0, AREA, 10.0).is_none());
        l.toggle_fullscreen_mode(FullscreenMode::Fullscreen);
        assert!(l.divider_at(500.0, 100.0, AREA, 10.0).is_some());
        // So does the scratchpad console, over its own top half.
        l.set_focus(3);
        l.move_focused_to_scratchpad(AREA, 10.0);
        l.toggle_scratchpad();
        assert!(l.divider_at(500.0, 100.0, AREA, 10.0).is_none());
        // Below the console the workspace's own dividers still answer.
        assert!(l.divider_at(500.0, 400.0, AREA, 10.0).is_some());
    }

    #[test]
    fn client_at_picks_the_topmost() {
        let mut l = abc();
        l.toggle_float(3, AREA, 0.0);
        // The float covers the middle 60%.
        assert_eq!(l.client_at(500.0, 300.0, AREA, 0.0), Some(3));
        assert_eq!(l.client_at(10.0, 10.0, AREA, 0.0), Some(1));
    }
}
