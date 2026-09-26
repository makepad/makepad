//! TreeView — nested rows that fold, and the arithmetic behind them.
//!
//! A tree is three things a list is not: rows that hide other rows, rows that
//! stand at a depth, and a tick on a branch that has to mean something about
//! the leaves under it. All three are arithmetic, and all three are what the
//! trees already in this repository each rewrite for themselves — the file
//! browser next door owns its own row templates, its drag, its status dots
//! and its scrolling, and the outliner in the modelling app owns a search, a
//! type filter and a hide toggle. Neither can be lent to a host that just has
//! a nested list of names. The flatten, the fold, the walk and the tri-state
//! cascade are lifted out here as free functions with their own tests, and
//! the widget below is only their drawing.
//!
//! What it deliberately does NOT do:
//!
//! * **No virtualisation and no scrolling of its own.** Every visible row is
//!   drawn every pass. That is honest up to a few hundred rows; past that the
//!   answer is a `PortalList` of your own, the way the file browser does it,
//!   not a scrollbar bolted on here.
//! * **No drag, no rename, no context menu.** Those are host policy, and a
//!   tree that guessed at them would be re-skinned by every caller.
//! * **No data source.** The host hands over a model, in nesting form or as
//!   an indented outline, and gets back typed actions.
//!
//! The one look decision worth writing down: the fold mark is a **plus and a
//! minus**, not a chevron, and the tick is a **dot and a dash**, not a check.
//! Small marks drawn as shader paths do not paint reliably here — two
//! mirrored paths in one shader painted one of the pair — so every mark in
//! this file is a rect, a box or a circle.

use crate::{family_api::measure, makepad_derive_widget::*, makepad_draw::*, widget::*};
use std::collections::{HashMap, HashSet};

/// One node as a caller writes it. Nesting is the natural authoring form;
/// `TreeModel` flattens it once so folding never walks a nested `Vec`.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeItem {
    pub id: LiveId,
    pub label: String,
    /// A glyph drawn before the label. It is TEXT, not a resource path, so a
    /// caller has to pick one the default font chain actually carries — most
    /// of the pictorial ranges render as tofu here. `\u{25CF}` and
    /// `\u{25CB}` are safe.
    pub icon: Option<String>,
    pub children: Vec<TreeItem>,
}

impl TreeItem {
    pub fn new(id: LiveId, label: &str) -> Self {
        Self { id, label: label.to_string(), icon: None, children: Vec::new() }
    }

    pub fn with_icon(mut self, icon: &str) -> Self {
        self.icon = Some(icon.to_string());
        self
    }

    pub fn with_children(mut self, children: Vec<TreeItem>) -> Self {
        self.children = children;
        self
    }
}

/// One node in the flattened tree. `parent` and `children` are indices into
/// `TreeModel::nodes`, which are in document order — the order the rows would
/// stand in with everything unfolded.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeNode {
    pub id: LiveId,
    pub label: String,
    pub icon: Option<String>,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

/// The tree, flattened. Built once when the model changes; folding, walking
/// and ticking only read it, so none of them can be O(depth) in allocations.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeModel {
    nodes: Vec<TreeNode>,
    roots: Vec<usize>,
    by_id: HashMap<LiveId, usize>,
}

impl TreeModel {
    pub fn from_items(items: &[TreeItem]) -> Self {
        fn add(model: &mut TreeModel, item: &TreeItem, parent: Option<usize>) {
            let index = model.push(item.id, &item.label, item.icon.clone(), parent);
            for child in &item.children {
                add(model, child, Some(index));
            }
        }
        let mut model = Self::default();
        for item in items {
            add(&mut model, item, None);
        }
        model
    }

    /// A tree written as indented lines: a tab, or every two leading spaces,
    /// is one level. Ids are the line numbers, counting from one.
    ///
    /// An indent that jumps more than one level past the line above it is
    /// clamped to one level rather than rejected — a hand-written outline
    /// with a stray space should still draw the tree its author meant.
    pub fn from_outline(lines: &[String]) -> Self {
        let mut model = Self::default();
        // The node standing at each depth, so a line's parent is whatever is
        // on top after truncating to its own depth.
        let mut stack: Vec<usize> = Vec::new();
        for (line_no, line) in lines.iter().enumerate() {
            let label = line.trim();
            if label.is_empty() {
                continue;
            }
            let depth = outline_depth(line).min(stack.len());
            stack.truncate(depth);
            let parent = stack.last().copied();
            let index = model.push(LiveId(line_no as u64 + 1), label, None, parent);
            stack.push(index);
        }
        model
    }

    fn push(
        &mut self,
        id: LiveId,
        label: &str,
        icon: Option<String>,
        parent: Option<usize>,
    ) -> usize {
        let index = self.nodes.len();
        self.by_id.insert(id, index);
        self.nodes.push(TreeNode {
            id,
            label: label.to_string(),
            icon,
            parent,
            children: Vec::new(),
        });
        match parent {
            Some(parent) => self.nodes[parent].children.push(index),
            None => self.roots.push(index),
        }
        index
    }

    pub fn nodes(&self) -> &[TreeNode] {
        &self.nodes
    }

    pub fn node(&self, index: usize) -> Option<&TreeNode> {
        self.nodes.get(index)
    }

    pub fn index_of(&self, id: LiveId) -> Option<usize> {
        self.by_id.get(&id).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Every node that has children, which is every node that can fold.
    pub fn branch_ids(&self) -> HashSet<LiveId> {
        self.nodes
            .iter()
            .filter(|node| !node.children.is_empty())
            .map(|node| node.id)
            .collect()
    }
}

/// How deep an outline line is indented: a tab, or every two spaces, is one
/// level. An odd trailing space is ignored rather than rounded up.
fn outline_depth(line: &str) -> usize {
    let mut level = 0;
    let mut spaces = 0;
    for c in line.chars() {
        match c {
            '\t' => {
                level += 1;
                spaces = 0;
            }
            ' ' => {
                spaces += 1;
                if spaces == 2 {
                    level += 1;
                    spaces = 0;
                }
            }
            _ => break,
        }
    }
    level
}

/// One row the tree would draw, in the order it draws them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeRow {
    /// Index into `TreeModel::nodes`.
    pub node: usize,
    pub depth: usize,
    pub has_children: bool,
    pub expanded: bool,
}

/// The rows a fold state leaves standing, in document order.
///
/// `open` holds the ids of the branches that are unfolded, so a node that
/// leaves the model and comes back keeps its fold — which is what a host
/// re-handing a refreshed tree expects, and what an index-keyed fold state
/// cannot do. A leaf is never expanded whatever the set says.
pub fn visible_rows(model: &TreeModel, open: &HashSet<LiveId>) -> Vec<TreeRow> {
    fn walk(
        out: &mut Vec<TreeRow>,
        model: &TreeModel,
        open: &HashSet<LiveId>,
        node: usize,
        depth: usize,
    ) {
        let Some(tree_node) = model.node(node) else {
            return;
        };
        let has_children = !tree_node.children.is_empty();
        let expanded = has_children && open.contains(&tree_node.id);
        out.push(TreeRow { node, depth, has_children, expanded });
        if expanded {
            for child in &tree_node.children {
                walk(out, model, open, *child, depth + 1);
            }
        }
    }

    let mut out = Vec::new();
    for root in &model.roots {
        walk(&mut out, model, open, *root, 0);
    }
    out
}

/// What one checkbox says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeTick {
    Off,
    /// Some of the leaves under this branch are ticked and some are not.
    Mixed,
    On,
}

/// What a branch's box shows.
///
/// A branch is **derived, never stored**: its state is read back out of its
/// children every time it is asked for. That is the whole of why a parent
/// can never disagree with its children, and why `cascade_tick` writes to
/// the leaves rather than to the branch it was handed.
pub fn tick_of(model: &TreeModel, ticked: &HashSet<LiveId>, node: usize) -> TreeTick {
    let Some(tree_node) = model.node(node) else {
        return TreeTick::Off;
    };
    if tree_node.children.is_empty() {
        return if ticked.contains(&tree_node.id) { TreeTick::On } else { TreeTick::Off };
    }
    let mut any_on = false;
    let mut any_off = false;
    for child in &tree_node.children {
        match tick_of(model, ticked, *child) {
            TreeTick::On => any_on = true,
            TreeTick::Off => any_off = true,
            TreeTick::Mixed => {
                any_on = true;
                any_off = true;
            }
        }
    }
    match (any_on, any_off) {
        (true, false) => TreeTick::On,
        (false, true) => TreeTick::Off,
        _ => TreeTick::Mixed,
    }
}

/// Tick or untick a node and everything under it.
///
/// Only leaves are written. Storing a branch as well would give the set two
/// answers for the same question the moment a child changed, and the stale
/// one is the one a naive reader would find first.
pub fn cascade_tick(
    model: &TreeModel,
    ticked: &mut HashSet<LiveId>,
    node: usize,
    on: bool,
) {
    let Some(tree_node) = model.node(node) else {
        return;
    };
    if tree_node.children.is_empty() {
        if on {
            ticked.insert(tree_node.id);
        } else {
            ticked.remove(&tree_node.id);
        }
        return;
    }
    for child in &tree_node.children {
        cascade_tick(model, ticked, *child, on);
    }
}

/// The ticked leaves, in document order.
pub fn ticked_ids(model: &TreeModel, ticked: &HashSet<LiveId>) -> Vec<LiveId> {
    model
        .nodes()
        .iter()
        .filter(|node| node.children.is_empty() && ticked.contains(&node.id))
        .map(|node| node.id)
        .collect()
}

/// How a click asks the selection to change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeGesture {
    /// This row and nothing else.
    Replace,
    /// Everything between the anchor and this row, added to what was there.
    Range,
    /// This row on or off, leaving the rest alone.
    Toggle,
}

/// The selection a click produces, in document order.
///
/// A branch selects **itself**, not its descendants: this is a general tree,
/// and a host that wants "select the subtree" can walk the model it handed
/// over. Ids that are currently folded away stay selected — a fold hides a
/// row, it does not deselect it, and dropping them would make a stray
/// ctrl-click silently throw away work.
pub fn selection_for_click(
    model: &TreeModel,
    rows: &[TreeRow],
    index: usize,
    anchor: Option<usize>,
    current: &HashSet<LiveId>,
    gesture: TreeGesture,
) -> Vec<LiveId> {
    let Some(row) = rows.get(index) else {
        return Vec::new();
    };
    let Some(id) = model.node(row.node).map(|node| node.id) else {
        return Vec::new();
    };

    let mut target: HashSet<LiveId> = match gesture {
        TreeGesture::Replace => HashSet::new(),
        _ => current.clone(),
    };
    match gesture {
        TreeGesture::Replace => {
            target.insert(id);
        }
        TreeGesture::Range => {
            let anchor = anchor.unwrap_or(index).min(rows.len().saturating_sub(1));
            let (from, to) = if anchor <= index { (anchor, index) } else { (index, anchor) };
            for row in &rows[from..=to] {
                if let Some(node) = model.node(row.node) {
                    target.insert(node.id);
                }
            }
        }
        TreeGesture::Toggle => {
            if !target.remove(&id) {
                target.insert(id);
            }
        }
    }

    // Document order, not click order: the caller reads this back as a set
    // of rows, and two clicks that end at the same selection should hand
    // back the same list.
    model
        .nodes()
        .iter()
        .filter(|node| target.contains(&node.id))
        .map(|node| node.id)
        .collect()
}

/// What a tree reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum TreeAction {
    /// The selection changed and this row is the one that was acted on.
    Selected(LiveId),
    /// A branch was folded or unfolded; true means it is now folded shut.
    Folded(LiveId, bool),
    /// A box was set; the cascade has already run, so read `ticked()` for
    /// the whole answer.
    Ticked(LiveId, bool),
    #[default]
    None,
}

/// Which part of a row a press landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Zone {
    Fold,
    Tick,
    Body,
}

/// Where one row was drawn, so a press can be answered without laying the
/// tree out again.
#[derive(Clone, Copy, Debug)]
struct DrawnRow {
    rect: Rect,
    fold: Option<Rect>,
    tick: Option<Rect>,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // EVERY value on these four draw types is plain and every one has a
    // field on the draw struct behind it. Mixing uniform() into a type that
    // also carries instance fields moves the instance slots out from under
    // a caller who overrides one, and the mark then reads a stroke width of
    // garbage. instance() is equally wrong: it never binds to the Rust
    // field, and the whole tree would draw in one row's state.
    mod.widgets.DrawTreeGroundBase = #(DrawTreeGround::script_component(vm))
    set_type_default() do #(DrawTreeGround::script_shader(vm)){
        ..mod.draw.DrawQuad
        // The tree's own rect. Every row is placed absolutely and so leaves
        // the widget nothing to be hovered by, redrawn by, or found by.
        // Transparent by default; a host that wants the tree on a panel of
        // its own sets a colour here rather than wrapping it in a View.
        /** the ground under every row */
        color: #00000000
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
            sdf.fill(self.color)
            return sdf.result
        }
    }

    mod.widgets.DrawTreeRowBase = #(DrawTreeRow::script_component(vm))
    set_type_default() do #(DrawTreeRow::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: #00000000
        color_hover: theme.color_surface_container_high
        color_selected: theme.color_primary_container
        /** the ring marking where the arrow keys are standing */
        color_ring: theme.color_primary
        /** row corner radius 0..12 step 0.5 */
        radius: 4.0
        /** how thick the keyboard ring is 0..3 step 0.5 */
        ring_size: 1.0
        hover: 0.0
        selected: 0.0
        focused: 0.0
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
            sdf.fill_keep(
                self.color
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_selected, self.selected)
            )
            sdf.stroke(vec4(0.0, 0.0, 0.0, 0.0).mix(self.color_ring, self.focused), self.ring_size)
            return sdf.result
        }
    }

    mod.widgets.DrawTreeGuideBase = #(DrawTreeGuide::script_component(vm))
    set_type_default() do #(DrawTreeGuide::script_shader(vm)){
        ..mod.draw.DrawQuad
        /** the hairline standing under each ancestor's fold mark */
        color: theme.color_outline_variant
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
            sdf.fill(self.color)
            return sdf.result
        }
    }

    mod.widgets.DrawTreeFoldBase = #(DrawTreeFold::script_component(vm))
    set_type_default() do #(DrawTreeFold::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: theme.color_text_meta
        color_hover: theme.color_text_hover
        /** how thick the bars of the mark are 1..3 step 0.5 */
        bar: 1.5
        open: 0.0
        hover: 0.0
        pixel: fn() {
            // A plus and a minus, drawn as two rects. A chevron would want a
            // path, and paths of this size do not paint reliably here.
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let cx2 = self.rect_size.x * 0.5
            let cy2 = self.rect_size.y * 0.5
            let arm = min(self.rect_size.x, self.rect_size.y) * 0.32
            sdf.rect(cx2 - arm, cy2 - self.bar * 0.5, arm * 2.0, self.bar)
            if self.open < 0.5 {
                sdf.rect(cx2 - self.bar * 0.5, cy2 - arm, self.bar, arm * 2.0)
                sdf.union()
            }
            sdf.fill(self.color.mix(self.color_hover, self.hover))
            return sdf.result
        }
    }

    mod.widgets.DrawTreeTickBase = #(DrawTreeTick::script_component(vm))
    set_type_default() do #(DrawTreeTick::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: #00000000
        /** the box once something under it is ticked */
        color_set: theme.color_primary
        /** the dot and the dash inside a set box */
        color_mark: theme.color_on_primary
        border_color: theme.color_outline
        /** the empty box's stroke 0..3 step 0.5 */
        border_size: 1.0
        /** box corner radius 0..8 step 0.5 */
        radius: 3.0
        on: 0.0
        mixed: 0.0
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let s = min(self.rect_size.x, self.rect_size.y)
            let x0 = (self.rect_size.x - s) * 0.5
            let y0 = (self.rect_size.y - s) * 0.5
            let set = max(self.on, self.mixed)
            sdf.box(
                x0 + self.border_size,
                y0 + self.border_size,
                s - self.border_size * 2.0,
                s - self.border_size * 2.0,
                self.radius
            )
            sdf.fill_keep(self.color.mix(self.color_set, set))
            sdf.stroke(self.border_color.mix(self.color_set, set), self.border_size)
            // On is a dot, mixed is a dash. Both are shapes the sdf paints
            // every time; a tick drawn as a path is the one that vanished.
            if self.on > 0.5 {
                sdf.circle(self.rect_size.x * 0.5, self.rect_size.y * 0.5, s * 0.17)
                sdf.fill(self.color_mark)
            }
            if self.mixed > 0.5 {
                sdf.rect(
                    x0 + s * 0.27,
                    self.rect_size.y * 0.5 - s * 0.07,
                    s * 0.46,
                    s * 0.14
                )
                sdf.fill(self.color_mark)
            }
            return sdf.result
        }
    }

    mod.widgets.TreeViewBase = #(TreeView::register_widget(vm))

    /** Nested rows that fold: an indent guide, a fold mark, an optional
     * glyph and a label, with selection of one row or many and boxes that
     * cascade to the leaves under them. */
    mod.widgets.TreeView = set_type_default() do mod.widgets.TreeViewBase{
        width: Fill
        height: Fit
        padding: Inset{left: 4. right: 4. top: 2. bottom: 2.}
        // A Fit tree is exactly as tall as its rows and clips nothing. A
        // host that gives it a Fill height smaller than its rows gets them
        // cut off at the edge rather than painted over whatever is below.
        clip_x: true
        clip_y: true

        /** the tree as indented lines: a tab, or every two spaces, is one level */
        outline: []
        /** one glyph per outline line, in the same order; short or empty means none */
        icons: []
        /** how tall one row is 16..48 step 1 */
        row_height: 24.
        /** how far one level is set in from its parent 8..40 step 1 */
        indent: 16.
        /** the fold mark's box 8..24 step 1 */
        fold_size: 12.
        /** the checkbox's box 10..24 step 1 */
        tick_size: 14.
        /** the room between two columns of a row 0..16 step 1 */
        spacing: 6.
        /** how wide the indent hairline is 0..3 step 0.5 */
        guide_size: 1.
        /** show a box on every row, cascading to the leaves under it */
        checkable: false
        /** more than one row can be selected at a time */
        multi_select: false
        /** draw the hairline standing under each ancestor's fold mark */
        show_guides: true
        /** a tree arrives unfolded; off means every branch starts shut */
        start_open: true

        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
        draw_text_selected +: {
            color: theme.color_on_primary_container
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
        draw_icon +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
    }
}

/// The tree's ground. Transparent by default, and the only place a host can
/// put a panel colour behind the rows without wrapping the tree in a View.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTreeGround {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
}

/// The field order here IS the instance layout, so it matches the order the
/// shader block declares, and every field is written per row from Rust.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTreeRow {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    color_hover: Vec4f,
    #[live]
    color_selected: Vec4f,
    #[live]
    color_ring: Vec4f,
    #[live]
    radius: f32,
    #[live]
    ring_size: f32,
    #[live]
    hover: f32,
    #[live]
    selected: f32,
    #[live]
    focused: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTreeGuide {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTreeFold {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    color_hover: Vec4f,
    #[live]
    bar: f32,
    #[live]
    open: f32,
    #[live]
    hover: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTreeTick {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    color_set: Vec4f,
    #[live]
    color_mark: Vec4f,
    #[live]
    border_color: Vec4f,
    #[live]
    border_size: f32,
    #[live]
    radius: f32,
    #[live]
    on: f32,
    #[live]
    mixed: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct TreeView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// The tree's own rect, so a press anywhere in it lands somewhere and
    /// hovering one row repaints the whole tree rather than one word of it.
    #[redraw]
    #[live]
    draw_bg: DrawTreeGround,
    #[live]
    draw_row: DrawTreeRow,
    #[live]
    draw_guide: DrawTreeGuide,
    #[live]
    draw_fold: DrawTreeFold,
    #[live]
    draw_tick: DrawTreeTick,
    #[live]
    pub draw_text: DrawText,
    #[live]
    pub draw_text_selected: DrawText,
    #[live]
    pub draw_icon: DrawText,

    /// A tree declared in markup, as indented lines.
    #[live]
    pub outline: Vec<String>,
    /// One glyph per outline line. A list of strings is the only list the
    /// script layer carries, which is also why the outline is one.
    #[live]
    pub icons: Vec<String>,
    #[live(24.0)]
    pub row_height: f64,
    #[live(16.0)]
    pub indent: f64,
    #[live(12.0)]
    pub fold_size: f64,
    #[live(14.0)]
    pub tick_size: f64,
    #[live(6.0)]
    pub spacing: f64,
    #[live(1.0)]
    pub guide_size: f64,
    #[live]
    pub checkable: bool,
    #[live]
    pub multi_select: bool,
    #[live(true)]
    pub show_guides: bool,
    #[live(true)]
    pub start_open: bool,

    #[rust]
    model: TreeModel,
    #[rust]
    open: HashSet<LiveId>,
    #[rust]
    ticked: HashSet<LiveId>,
    #[rust]
    selected: HashSet<LiveId>,
    /// Where the arrow keys are standing. Not the same as the selection: a
    /// shift-range has one focus and many selected rows.
    #[rust]
    focus_id: Option<LiveId>,
    /// Where a shift-range measures from.
    #[rust]
    anchor: Option<LiveId>,
    /// A host has handed over a model, so the markup outline stops seeding.
    #[rust]
    host_set: bool,
    #[rust]
    seeded_from: Vec<String>,
    #[rust]
    rows: Vec<TreeRow>,
    #[rust]
    drawn: Vec<DrawnRow>,
    #[rust]
    hover: Option<(usize, Zone)>,
    #[rust]
    area: Area,
}

impl TreeView {
    /// The tree, as nested items. Handing over the same tree again costs
    /// nothing and keeps the folds, which is what a host refreshing on a
    /// timer needs.
    pub fn set_items(&mut self, cx: &mut Cx, items: &[TreeItem]) {
        let model = TreeModel::from_items(items);
        if self.host_set && self.model == model {
            return;
        }
        let first = !self.host_set;
        self.host_set = true;
        self.adopt(model, first);
        self.redraw(cx);
    }

    /// Whatever survived the model change, plus a starting fold state for a
    /// tree being seen for the first time.
    fn adopt(&mut self, model: TreeModel, first: bool) {
        let open = if first && self.start_open {
            model.branch_ids()
        } else {
            self.open.iter().copied().filter(|id| model.index_of(*id).is_some()).collect()
        };
        let ticked = self.ticked.iter().copied().filter(|id| model.index_of(*id).is_some()).collect();
        let selected =
            self.selected.iter().copied().filter(|id| model.index_of(*id).is_some()).collect();
        self.focus_id = self.focus_id.filter(|id| model.index_of(*id).is_some());
        self.anchor = self.anchor.filter(|id| model.index_of(*id).is_some());
        self.open = open;
        self.ticked = ticked;
        self.selected = selected;
        self.model = model;
        self.hover = None;
    }

    /// A tree written in markup shows itself without the host saying
    /// anything, and shows itself again after a live reload changes the
    /// outline. A host that has called `set_items` owns the model from then
    /// on, and the outline stops being read.
    fn seed(&mut self) {
        if self.host_set || self.outline.is_empty() || self.seeded_from == self.outline {
            return;
        }
        let mut model = TreeModel::from_outline(&self.outline);
        for (line_no, icon) in self.icons.iter().enumerate() {
            if icon.is_empty() {
                continue;
            }
            if let Some(index) = model.index_of(LiveId(line_no as u64 + 1)) {
                model.nodes[index].icon = Some(icon.clone());
            }
        }
        self.seeded_from = self.outline.clone();
        self.adopt(model, true);
    }

    pub fn selected_ids(&self) -> Vec<LiveId> {
        self.model
            .nodes()
            .iter()
            .filter(|node| self.selected.contains(&node.id))
            .map(|node| node.id)
            .collect()
    }

    pub fn ticked_ids(&self) -> Vec<LiveId> {
        ticked_ids(&self.model, &self.ticked)
    }

    pub fn tick_of_id(&self, id: LiveId) -> TreeTick {
        match self.model.index_of(id) {
            Some(index) => tick_of(&self.model, &self.ticked, index),
            None => TreeTick::Off,
        }
    }

    pub fn label_of(&self, id: LiveId) -> Option<String> {
        let index = self.model.index_of(id)?;
        self.model.node(index).map(|node| node.label.clone())
    }

    pub fn set_open(&mut self, cx: &mut Cx, id: LiveId, open: bool) {
        let changed = if open { self.open.insert(id) } else { self.open.remove(&id) };
        if changed {
            self.redraw(cx);
        }
    }

    pub fn expand_all(&mut self, cx: &mut Cx) {
        self.open = self.model.branch_ids();
        self.redraw(cx);
    }

    pub fn collapse_all(&mut self, cx: &mut Cx) {
        self.open.clear();
        self.redraw(cx);
    }

    /// The widest a row's label column would need, used to resolve a Fit.
    fn natural_width(&self, cx: &mut Cx2d, rows: &[TreeRow], icon_w: f64) -> f64 {
        let mut widest = 0.0f64;
        for row in rows {
            let Some(node) = self.model.node(row.node) else {
                continue;
            };
            let x = self.label_offset(row.depth, icon_w);
            widest = widest.max(x + measure(&self.draw_text, cx, &node.label));
        }
        widest + self.layout.padding.left + self.layout.padding.right
    }

    /// Where a row's label starts, measured from the tree's inner left edge.
    fn label_offset(&self, depth: usize, icon_w: f64) -> f64 {
        let mut x = depth as f64 * self.indent + self.fold_size + self.spacing;
        if self.checkable {
            x += self.tick_size + self.spacing;
        }
        if icon_w > 0.0 {
            x += icon_w + self.spacing;
        }
        x
    }

    /// The icon column's width: the widest glyph any node carries, and zero
    /// when none does. Reserved for the whole tree rather than per row, or
    /// the labels of a tree where only folders have glyphs come out ragged.
    fn icon_column(&self, cx: &mut Cx2d) -> f64 {
        let mut widest = 0.0f64;
        for node in self.model.nodes() {
            if let Some(icon) = &node.icon {
                widest = widest.max(measure(&self.draw_icon, cx, icon));
            }
        }
        widest
    }

    fn row_id(&self, index: usize) -> Option<LiveId> {
        let row = self.rows.get(index)?;
        self.model.node(row.node).map(|node| node.id)
    }

    fn zone_at(&self, pos: Vec2d) -> Option<(usize, Zone)> {
        for (index, drawn) in self.drawn.iter().enumerate() {
            if !drawn.rect.contains(pos) {
                continue;
            }
            if drawn.fold.is_some_and(|rect| rect.contains(pos)) {
                return Some((index, Zone::Fold));
            }
            if drawn.tick.is_some_and(|rect| rect.contains(pos)) {
                return Some((index, Zone::Tick));
            }
            return Some((index, Zone::Body));
        }
        None
    }

    fn click_row(&mut self, cx: &mut Cx, index: usize, gesture: TreeGesture) {
        let Some(id) = self.row_id(index) else {
            return;
        };
        let anchor = self
            .anchor
            .and_then(|anchor| self.rows.iter().position(|row| self.model.node(row.node).map(|n| n.id) == Some(anchor)));
        let picked =
            selection_for_click(&self.model, &self.rows, index, anchor, &self.selected, gesture);
        self.selected = picked.into_iter().collect();
        self.focus_id = Some(id);
        if gesture != TreeGesture::Range {
            self.anchor = Some(id);
        }
        let uid = self.uid;
        self.redraw(cx);
        cx.widget_action(uid, TreeAction::Selected(id));
    }

    fn toggle_fold(&mut self, cx: &mut Cx, index: usize) -> bool {
        let Some(row) = self.rows.get(index).copied() else {
            return false;
        };
        if !row.has_children {
            return false;
        }
        let Some(id) = self.row_id(index) else {
            return false;
        };
        let now_folded = self.open.remove(&id);
        if !now_folded {
            self.open.insert(id);
        }
        self.focus_id = Some(id);
        let uid = self.uid;
        // `rows` is deliberately NOT recomputed here: it indexes the same
        // rows `drawn` does, and moving one without the other would let a
        // second press before the redraw answer for a different row.
        self.redraw(cx);
        cx.widget_action(uid, TreeAction::Folded(id, now_folded));
        true
    }

    fn toggle_tick(&mut self, cx: &mut Cx, index: usize) -> bool {
        let Some(row) = self.rows.get(index).copied() else {
            return false;
        };
        let Some(id) = self.row_id(index) else {
            return false;
        };
        // Mixed resolves to on, the way a select-all box does: the useful
        // next step from "some of them" is "all of them", not "none".
        let on = tick_of(&self.model, &self.ticked, row.node) != TreeTick::On;
        cascade_tick(&self.model, &mut self.ticked, row.node, on);
        self.focus_id = Some(id);
        let uid = self.uid;
        self.redraw(cx);
        cx.widget_action(uid, TreeAction::Ticked(id, on));
        true
    }

    /// The visible row the arrow keys are standing on, or the row an
    /// unfocused tree should start from.
    fn focus_index(&self) -> Option<usize> {
        let id = self.focus_id?;
        self.rows.iter().position(|row| self.model.node(row.node).map(|n| n.id) == Some(id))
    }

    fn focus_row(&mut self, cx: &mut Cx, index: usize) -> bool {
        // Walking with the keys selects as it goes: a tree where the arrows
        // move a ring and leave the selection behind makes the reader press
        // a second key to mean what they already meant.
        self.click_row(cx, index, TreeGesture::Replace);
        true
    }

    fn step_focus(&mut self, cx: &mut Cx, by: isize) -> bool {
        if self.rows.is_empty() {
            return false;
        }
        let last = self.rows.len() - 1;
        let to = match self.focus_index() {
            Some(at) => (at as isize + by).clamp(0, last as isize) as usize,
            None => {
                if by > 0 {
                    0
                } else {
                    last
                }
            }
        };
        self.focus_row(cx, to)
    }

    /// Right: open a shut branch, then step into it. Not both at once — the
    /// first press should show the children, not skip past them.
    fn open_or_descend(&mut self, cx: &mut Cx) -> bool {
        let Some(at) = self.focus_index() else {
            return self.step_focus(cx, 1);
        };
        let Some(row) = self.rows.get(at).copied() else {
            return false;
        };
        if row.has_children && !row.expanded {
            return self.toggle_fold(cx, at);
        }
        if row.expanded && at + 1 < self.rows.len() {
            return self.focus_row(cx, at + 1);
        }
        false
    }

    /// Left: shut an open branch, otherwise climb to the parent. A leaf's
    /// left key is the only way back up a deep tree without the mouse.
    fn close_or_ascend(&mut self, cx: &mut Cx) -> bool {
        let Some(at) = self.focus_index() else {
            return self.step_focus(cx, -1);
        };
        let Some(row) = self.rows.get(at).copied() else {
            return false;
        };
        if row.has_children && row.expanded {
            return self.toggle_fold(cx, at);
        }
        let Some(parent) = self.model.node(row.node).and_then(|node| node.parent) else {
            return false;
        };
        let Some(up) = self.rows.iter().position(|row| row.node == parent) else {
            return false;
        };
        self.focus_row(cx, up)
    }
}

impl Widget for TreeView {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.seed();
        self.rows = visible_rows(&self.model, &self.open);
        let rows = self.rows.clone();

        // A Fill inside a Fit resolves to nothing, so a Fit tree has to
        // work out its own size before a single row is drawn.
        let icon_w = self.icon_column(cx);
        let natural_w = self.natural_width(cx, &rows, icon_w);
        let natural_h = rows.len() as f64 * self.row_height
            + self.layout.padding.top
            + self.layout.padding.bottom;
        let walk = Walk {
            width: match walk.width {
                Size::Fit { .. } => Size::Fixed(natural_w),
                other => other,
            },
            height: match walk.height {
                Size::Fit { .. } => Size::Fixed(natural_h),
                other => other,
            },
            ..walk
        };

        // Read before anything paints: the ring means "the arrow keys are
        // here", which says nothing to someone using the mouse, and setting
        // it afterwards lands on rows that have already drawn.
        let keyed = cx.cx.cx.has_key_focus(self.area);

        self.draw_bg.begin(cx, walk, self.layout);
        // The INNER rect: the turtle's own rect is the outer one, and rows
        // laid out against that ignore the padding the preset asks for.
        let inner = cx.turtle().inner_rect();
        self.drawn.clear();

        let row_h = self.row_height;
        let line = 14.0f64.min(row_h);
        // A text run has no height of its own until it is laid out, so it is
        // given a line box and centred in that.
        let place = move |x: f64, w: f64, y: f64| Walk {
            abs_pos: Some(dvec2(x, y + (row_h - line) * 0.5)),
            width: Size::Fixed(w.max(0.0)),
            height: Size::Fixed(line),
            ..Walk::default()
        };
        let left = Align { x: 0.0, y: 0.5 };
        let centre = Align { x: 0.5, y: 0.5 };

        for (index, row) in rows.iter().enumerate() {
            let (id, label, icon) = match self.model.node(row.node) {
                Some(node) => (node.id, node.label.clone(), node.icon.clone()),
                None => continue,
            };
            let y = inner.pos.y + index as f64 * row_h;
            let rect = Rect { pos: dvec2(inner.pos.x, y), size: dvec2(inner.size.x, row_h) };
            let selected = self.selected.contains(&id);
            let hovered = matches!(self.hover, Some((at, _)) if at == index);

            self.draw_row.hover = if hovered { 1.0 } else { 0.0 };
            self.draw_row.selected = if selected { 1.0 } else { 0.0 };
            self.draw_row.focused = if keyed && self.focus_id == Some(id) { 1.0 } else { 0.0 };
            self.draw_row.draw_abs(cx, rect);

            // One hairline per ancestor, standing where that ancestor's own
            // fold mark stands, so a deep row can be traced back by eye.
            if self.show_guides && self.guide_size > 0.0 {
                for level in 0..row.depth {
                    let gx = inner.pos.x + level as f64 * self.indent
                        + (self.fold_size - self.guide_size) * 0.5;
                    self.draw_guide.draw_abs(
                        cx,
                        Rect { pos: dvec2(gx, y), size: dvec2(self.guide_size, row_h) },
                    );
                }
            }

            let fold_rect = Rect {
                pos: dvec2(
                    inner.pos.x + row.depth as f64 * self.indent,
                    y + (row_h - self.fold_size) * 0.5,
                ),
                size: dvec2(self.fold_size, self.fold_size),
            };
            let fold = if row.has_children {
                self.draw_fold.open = if row.expanded { 1.0 } else { 0.0 };
                self.draw_fold.hover =
                    if matches!(self.hover, Some((at, Zone::Fold)) if at == index) { 1.0 } else { 0.0 };
                self.draw_fold.draw_abs(cx, fold_rect);
                Some(fold_rect)
            } else {
                None
            };

            let mut x = inner.pos.x + row.depth as f64 * self.indent + self.fold_size + self.spacing;
            let tick = if self.checkable {
                let tick_rect = Rect {
                    pos: dvec2(x, y + (row_h - self.tick_size) * 0.5),
                    size: dvec2(self.tick_size, self.tick_size),
                };
                let state = tick_of(&self.model, &self.ticked, row.node);
                self.draw_tick.on = if state == TreeTick::On { 1.0 } else { 0.0 };
                self.draw_tick.mixed = if state == TreeTick::Mixed { 1.0 } else { 0.0 };
                self.draw_tick.draw_abs(cx, tick_rect);
                x += self.tick_size + self.spacing;
                Some(tick_rect)
            } else {
                None
            };

            if icon_w > 0.0 {
                if let Some(icon) = icon {
                    self.draw_icon.draw_walk(cx, place(x, icon_w, y), centre, &icon);
                }
                x += icon_w + self.spacing;
            }

            let label_w = inner.pos.x + inner.size.x - x;
            if selected {
                self.draw_text_selected.draw_walk(cx, place(x, label_w, y), left, &label);
            } else {
                self.draw_text.draw_walk(cx, place(x, label_w, y), left, &label);
            }

            self.drawn.push(DrawnRow { rect, fold, tick });
        }

        self.draw_bg.end(cx);
        self.area = self.draw_bg.area();
        // ONE stop for the whole tree: Tab reaches it, the arrows move
        // inside it, and Tab again leaves it rather than walking every row.
        cx.add_nav_stop(self.area, NavRole::TextInput, Inset::default());
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let at = self.zone_at(fe.abs);
                if at != self.hover {
                    self.hover = at;
                    cx.set_cursor(if at.is_some() {
                        MouseCursor::Hand
                    } else {
                        MouseCursor::Default
                    });
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.redraw(cx);
                }
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                cx.set_key_focus(self.area);
                let Some((index, zone)) = self.zone_at(fe.abs) else {
                    self.redraw(cx);
                    return;
                };
                match zone {
                    Zone::Fold => {
                        self.toggle_fold(cx, index);
                    }
                    Zone::Tick => {
                        self.toggle_tick(cx, index);
                    }
                    Zone::Body => {
                        let gesture = if !self.multi_select {
                            TreeGesture::Replace
                        } else if fe.modifiers.shift {
                            TreeGesture::Range
                        } else if fe.modifiers.control || fe.modifiers.logo {
                            TreeGesture::Toggle
                        } else {
                            TreeGesture::Replace
                        };
                        self.click_row(cx, index, gesture);
                    }
                }
            }
            Hit::KeyFocus(_) | Hit::KeyFocusLost(_) => self.redraw(cx),
            Hit::KeyDown(ke) => {
                // The fold state may have moved since the last draw, so the
                // walk is over rows recomputed here rather than drawn ones.
                self.rows = visible_rows(&self.model, &self.open);
                match ke.key_code {
                    KeyCode::ArrowUp => {
                        self.step_focus(cx, -1);
                    }
                    KeyCode::ArrowDown => {
                        self.step_focus(cx, 1);
                    }
                    KeyCode::ArrowLeft => {
                        self.close_or_ascend(cx);
                    }
                    KeyCode::ArrowRight => {
                        self.open_or_descend(cx);
                    }
                    KeyCode::Home => {
                        if !self.rows.is_empty() {
                            self.focus_row(cx, 0);
                        }
                    }
                    KeyCode::End => {
                        if !self.rows.is_empty() {
                            let last = self.rows.len() - 1;
                            self.focus_row(cx, last);
                        }
                    }
                    // Without this the boxes are reachable by pointer only,
                    // which makes a checkable tree unusable from the keyboard.
                    KeyCode::Space if self.checkable => {
                        if let Some(at) = self.focus_index() {
                            self.toggle_tick(cx, at);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// The row the keys are standing on, so a test can read the tree in one
    /// line without walking rects.
    fn text(&self) -> String {
        self.focus_id.and_then(|id| self.label_of(id)).unwrap_or_default()
    }
}

impl TreeViewRef {
    pub fn set_items(&self, cx: &mut Cx, items: &[TreeItem]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_items(cx, items);
        }
    }

    /// The selected rows, in document order.
    pub fn selected(&self) -> Vec<LiveId> {
        self.borrow().map(|inner| inner.selected_ids()).unwrap_or_default()
    }

    /// The ticked leaves, in document order. Branches are derived and so are
    /// never in this list.
    pub fn ticked(&self) -> Vec<LiveId> {
        self.borrow().map(|inner| inner.ticked_ids()).unwrap_or_default()
    }

    pub fn tick_of(&self, id: LiveId) -> TreeTick {
        self.borrow().map(|inner| inner.tick_of_id(id)).unwrap_or(TreeTick::Off)
    }

    pub fn label_of(&self, id: LiveId) -> Option<String> {
        self.borrow().and_then(|inner| inner.label_of(id))
    }

    pub fn focused(&self) -> Option<LiveId> {
        self.borrow().and_then(|inner| inner.focus_id)
    }

    pub fn set_open(&self, cx: &mut Cx, id: LiveId, open: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_open(cx, id, open);
        }
    }

    pub fn expand_all(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.expand_all(cx);
        }
    }

    pub fn collapse_all(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.collapse_all(cx);
        }
    }

    /// The row acted on this pass, if the selection changed.
    pub fn chosen(&self, actions: &Actions) -> Option<LiveId> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<TreeAction>() {
            TreeAction::Selected(id) => Some(id),
            _ => None,
        }
    }

    /// The branch folded or unfolded this pass; true means folded shut.
    pub fn folded(&self, actions: &Actions) -> Option<(LiveId, bool)> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<TreeAction>() {
            TreeAction::Folded(id, folded) => Some((id, folded)),
            _ => None,
        }
    }

    /// The box set this pass, and what it was set to.
    pub fn tick_changed(&self, actions: &Actions) -> Option<(LiveId, bool)> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<TreeAction>() {
            TreeAction::Ticked(id, on) => Some((id, on)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small tree, written the way a caller would:
    ///
    /// ```text
    /// src            1
    ///   ui           2
    ///     row.rs     3
    ///     panel.rs   4
    ///   main.rs      5
    /// docs           6
    /// ```
    fn sample() -> TreeModel {
        TreeModel::from_outline(&[
            "src".to_string(),
            "  ui".to_string(),
            "    row.rs".to_string(),
            "    panel.rs".to_string(),
            "  main.rs".to_string(),
            "docs".to_string(),
        ])
    }

    /// The rows as their line numbers, which is what `from_outline` uses for
    /// ids and so the shortest way to write down an expected tree.
    fn lines(model: &TreeModel, rows: &[TreeRow]) -> Vec<u64> {
        rows.iter().map(|row| model.nodes()[row.node].id.0).collect()
    }

    fn open(lines: &[u64]) -> HashSet<LiveId> {
        lines.iter().map(|line| LiveId(*line)).collect()
    }

    /// Nothing open is the roots and only the roots, whatever depth the rest
    /// of the tree stands at.
    #[test]
    fn a_shut_tree_is_its_roots() {
        let model = sample();
        let rows = visible_rows(&model, &HashSet::new());
        assert_eq!(lines(&model, &rows), vec![1, 6]);
        assert!(rows[0].has_children, "src has children whether or not it is open");
        assert!(!rows[0].expanded);
        assert!(!rows[1].has_children);
    }

    /// Opening a branch shows its children and stops there: a grandchild
    /// needs its own parent open, not just its grandparent.
    #[test]
    fn opening_a_branch_shows_one_level_not_the_subtree() {
        let model = sample();
        let rows = visible_rows(&model, &open(&[1]));
        assert_eq!(lines(&model, &rows), vec![1, 2, 5, 6]);
        assert_eq!(rows.iter().map(|row| row.depth).collect::<Vec<_>>(), vec![0, 1, 1, 0]);

        let rows = visible_rows(&model, &open(&[1, 2]));
        assert_eq!(lines(&model, &rows), vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(
            rows.iter().map(|row| row.depth).collect::<Vec<_>>(),
            vec![0, 1, 2, 2, 1, 0]
        );
    }

    /// Folding a branch takes its whole subtree with it, however deep — and
    /// leaves the fold state of what it hid alone, so unfolding it again
    /// gives back exactly the tree that was there.
    #[test]
    fn folding_a_branch_hides_its_whole_subtree_and_gives_it_back() {
        let model = sample();
        let all = open(&[1, 2]);
        let full = lines(&model, &visible_rows(&model, &all));

        let mut shut = all.clone();
        shut.remove(&LiveId(1));
        assert_eq!(lines(&model, &visible_rows(&model, &shut)), vec![1, 6]);

        assert_eq!(lines(&model, &visible_rows(&model, &all)), full);
    }

    /// An id in the open set that names a leaf, or nothing at all, changes
    /// nothing — a fold state outliving the model it was built for must not
    /// invent rows.
    #[test]
    fn opening_a_leaf_or_a_stranger_does_nothing() {
        let model = sample();
        assert_eq!(lines(&model, &visible_rows(&model, &open(&[6, 99]))), vec![1, 6]);
    }

    /// Two leading spaces are one level, a tab is one level, and an indent
    /// that jumps a level is clamped rather than dropped.
    #[test]
    fn an_outline_indents_into_a_tree() {
        let model = TreeModel::from_outline(&[
            "root".to_string(),
            "\tchild".to_string(),
            "      way over there".to_string(),
            "second".to_string(),
        ]);
        let rows = visible_rows(&model, &model.branch_ids());
        assert_eq!(
            rows.iter().map(|row| row.depth).collect::<Vec<_>>(),
            vec![0, 1, 2, 0],
            "six spaces would be depth three; the tree only has room for two"
        );
        assert_eq!(model.nodes()[1].label, "child", "the indent is not part of the name");
    }

    /// Checking a parent reaches every leaf under it, however deep.
    #[test]
    fn checking_a_parent_checks_every_leaf_under_it() {
        let model = sample();
        let src = model.index_of(LiveId(1)).unwrap();
        let mut ticked = HashSet::new();
        cascade_tick(&model, &mut ticked, src, true);

        assert_eq!(ticked_ids(&model, &ticked), vec![LiveId(3), LiveId(4), LiveId(5)]);
        assert_eq!(tick_of(&model, &ticked, src), TreeTick::On);
        assert_eq!(
            tick_of(&model, &ticked, model.index_of(LiveId(6)).unwrap()),
            TreeTick::Off,
            "the other root was not under it"
        );
    }

    /// A parent whose children disagree is mixed, and so is its parent.
    #[test]
    fn a_parent_whose_children_disagree_is_mixed() {
        let model = sample();
        let mut ticked = HashSet::new();
        cascade_tick(&model, &mut ticked, model.index_of(LiveId(3)).unwrap(), true);

        assert_eq!(tick_of(&model, &ticked, model.index_of(LiveId(2)).unwrap()), TreeTick::Mixed);
        assert_eq!(
            tick_of(&model, &ticked, model.index_of(LiveId(1)).unwrap()),
            TreeTick::Mixed,
            "mixed travels all the way up, not one level"
        );

        // Complete the set and the whole spine turns on.
        cascade_tick(&model, &mut ticked, model.index_of(LiveId(4)).unwrap(), true);
        assert_eq!(tick_of(&model, &ticked, model.index_of(LiveId(2)).unwrap()), TreeTick::On);
        assert_eq!(
            tick_of(&model, &ticked, model.index_of(LiveId(1)).unwrap()),
            TreeTick::Mixed,
            "main.rs is still off, so src is still mixed"
        );
    }

    /// Unchecking a parent clears the subtree and nothing else.
    #[test]
    fn unchecking_a_parent_clears_only_its_own_subtree() {
        let model = sample();
        let mut ticked = HashSet::new();
        cascade_tick(&model, &mut ticked, model.index_of(LiveId(1)).unwrap(), true);
        cascade_tick(&model, &mut ticked, model.index_of(LiveId(6)).unwrap(), true);
        cascade_tick(&model, &mut ticked, model.index_of(LiveId(2)).unwrap(), false);

        assert_eq!(ticked_ids(&model, &ticked), vec![LiveId(5), LiveId(6)]);
    }

    /// A branch is derived, so a set that claims a branch is ticked while
    /// its children are not cannot make the branch lie.
    #[test]
    fn a_branch_can_never_disagree_with_its_children() {
        let model = sample();
        let ticked: HashSet<LiveId> = [LiveId(1), LiveId(2)].into_iter().collect();
        assert_eq!(tick_of(&model, &ticked, model.index_of(LiveId(1)).unwrap()), TreeTick::Off);
        assert!(ticked_ids(&model, &ticked).is_empty(), "branches are not leaves");
    }

    /// Replace takes one row, a range takes everything between the anchor
    /// and the click, and a toggle leaves the rest of the selection alone.
    #[test]
    fn a_click_replaces_a_range_extends_and_a_toggle_flips_one() {
        let model = sample();
        let rows = visible_rows(&model, &open(&[1, 2]));
        let none = HashSet::new();

        assert_eq!(
            selection_for_click(&model, &rows, 2, None, &none, TreeGesture::Replace),
            vec![LiveId(3)]
        );
        assert_eq!(
            selection_for_click(&model, &rows, 4, Some(1), &none, TreeGesture::Range),
            vec![LiveId(2), LiveId(3), LiveId(4), LiveId(5)]
        );

        let one: HashSet<LiveId> = [LiveId(3)].into_iter().collect();
        assert_eq!(
            selection_for_click(&model, &rows, 5, None, &one, TreeGesture::Toggle),
            vec![LiveId(3), LiveId(6)]
        );
        assert_eq!(
            selection_for_click(&model, &rows, 2, None, &one, TreeGesture::Toggle),
            Vec::<LiveId>::new(),
            "toggling the only selected row clears it"
        );
    }

    /// A row folded away stays selected: a fold hides a row, it does not
    /// deselect it, and dropping it would throw work away on a stray click.
    #[test]
    fn a_selection_survives_being_folded_out_of_sight() {
        let model = sample();
        let rows = visible_rows(&model, &open(&[1]));
        let hidden: HashSet<LiveId> = [LiveId(3)].into_iter().collect();
        assert_eq!(
            selection_for_click(&model, &rows, 3, None, &hidden, TreeGesture::Toggle),
            vec![LiveId(3), LiveId(6)]
        );
    }
}
