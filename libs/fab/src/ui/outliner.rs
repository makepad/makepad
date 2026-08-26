//! Lane D. The outliner: a real tree over `Scene::tree`, drawn as a
//! `PortalList` of our own rows so every column is ours — expand chevron,
//! element-class icon, name, and the eye / selectable toggles lined up in
//! fixed columns down the right edge like Fab's.
//!
//! Everything here does something:
//! * the search field filters (debounced 150 ms) and keeps the ancestors of
//!   every match so matches stay in their tree,
//! * the funnel raises a type filter menu built from the classes actually in
//!   the model,
//! * click selects elements; branch rows select their descendant elements and
//!   their chevrons fold them, while arrow keys move/select the focused row,
//! * the eye hides (`SetHidden`), and a hidden row dims,
//! * right-click raises the context menu (isolate / hide / unhide / frame /
//!   select children),
//! * any viewport selection expands the selected element's storey (and kind
//!   group, when present) and scrolls the row into view.
//!
//! One row template per element class (the type icon is the only difference),
//! because `PortalList` keys its recycling pool by template — that is cheaper
//! and simpler than fourteen hidden icons per row.

use crate::api::*;
use crate::ui::icons::{element_icon, Icon};
use crate::ui::popover::{dropdown_clicked, menu_picked, open_menu, MenuItem, MenuPlace};
use makepad_widgets::*;
use std::collections::{HashMap, HashSet};

const OUTLINER_MENU: LiveId = live_id!(fab_outliner_menu);
const FILTER_MENU: LiveId = live_id!(fab_outliner_filter);
const INDENT: f64 = 13.0;
const KIND_GROUP_THRESHOLD: usize = 40;

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    let OutlinerRow = View{
        width: Fill
        height: fab.row_height
        flow: Right
        align: Align{x: 0.0 y: 0.0}
        padding: Inset{left: 2 right: 4 top: 2 bottom: 2}
        spacing: 2
        cursor: MouseCursor.Hand
        show_bg: true
        draw_bg +: {
            hover: instance(0.0)
            selected: instance(0.0)
            active: instance(0.0)
            odd: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                let base = fab.color_row_even.mix(fab.color_row_odd, self.odd)
                let c = base
                    .mix(fab.color_row_hover, self.hover * (1.0 - self.selected))
                    .mix(fab.color_selection_bg, self.selected)
                    .mix(fab.color_accent, self.active)
                sdf.fill(c)
                return sdf.result
            }
        }
        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: { draw_bg: {hover: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: { draw_bg: {hover: 1.0} }
                }
            }
        }
        indent := View{ width: 0 height: 2 }
        // `Icon` has no `visible` field (only View/Button do), so the
        // disclosure chevron lives in a View that can be hidden — and that
        // View is also what makes the chevron its own click target.
        FabTip{ text: "Expand or collapse children"
            twist := View{
                visible: false
                width: 12
                height: Fill
                padding: Inset{top: 2 bottom: 2 left: 0 right: 0}
                cursor: MouseCursor.Hand
                tw := IconRotated{
                    width: 12
                    height: 12
                    align: Align{x: 0.0 y: 0.0}
                    icon_walk: Walk{ width: 12 height: 12 }
                    draw_icon +: {
                        color: fab.color_text_dim
                        svg: crate_resource("self://resources/icons/chevron_down.svg")
                        // Rows can have different fold states in one draw;
                        // this must travel with the recycled row instance.
                        rotation_angle: instance(0.0)
                    }
                }
            }
        }
        twist_gap := View{ width: 12 height: Fill visible: false }
        type_icon := mod.widgets.FabIcon{
            width: fab.icon_size
            height: fab.icon_size
            icon_walk: Walk{ width: fab.icon_size height: fab.icon_size }
            margin: Inset{left: 1 right: 3}
            draw_icon +: {
                color: fab.color_text_dim
                svg: crate_resource("self://resources/icons/el_mesh.svg")
            }
        }
        name := mod.widgets.FabLabel{
            width: Fill
            height: Fill
            text: ""
        }
        FabTip{ text: "Hide element"
            eye := mod.widgets.FabIconButton{
                width: 16
                height: 16
                padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
                icon_walk: Walk{ width: fab.icon_size height: fab.icon_size }
                draw_icon +: { svg: crate_resource("self://resources/icons/eye.svg") }
            }
        }
        FabTip{ text: "Show element"
            eye_off := mod.widgets.FabIconButton{
                width: 16
                height: 16
                padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
                icon_walk: Walk{ width: fab.icon_size height: fab.icon_size }
                visible: false
                draw_icon +: {
                    color: fab.color_text_muted
                    svg: crate_resource("self://resources/icons/eye_off.svg")
                }
            }
        }
        // No third toggle: Fab's "selectable / disable in renders" has no
        // counterpart in `api.rs`, and a control that cannot do its job does
        // not belong in the row.
    }

    mod.widgets.FabOutlinerBase = #(FabOutliner::register_widget(vm))
    mod.widgets.FabOutliner = set_type_default() do mod.widgets.FabOutlinerBase{
        width: Fill
        height: Fill
        flow: Down
        show_bg: true
        draw_bg +: {
            color: fab.color_editor
        }
        header := mod.widgets.FabAreaHeader{
            FabTip{ text: "Choose editor"
                editor_type := mod.widgets.FabDropdownButton{ label +: { text: "Outliner" } }
            }
            search := mod.widgets.FabSearch{ width: Fill }
            FabTip{ text: "Filter element types"
                funnel := mod.widgets.FabDropdownButton{
                    tag: @outliner_filter
                    owner: @fab_outliner_filter
                    spacing: 0
                    padding: Inset{left: 2 right: 2 top: 0 bottom: 0}
                    label +: { text: "" }
                    chevron_slot +: { visible: false }
                    ico_slot := View{
                        width: fab.icon_size
                        height: Fill
                        padding: Inset{top: 2 bottom: 2 left: 0 right: 0}
                        ico := mod.widgets.FabIconDim{
                            width: fab.icon_size
                            height: fab.icon_size
                            icon_walk: Walk{ width: fab.icon_size height: fab.icon_size }
                            draw_icon +: { svg: crate_resource("self://resources/icons/filter.svg") }
                        }
                    }
                }
            }
        }
        list := PortalList{
            width: Fill
            height: Fill
            flow: Down
            auto_tail: false
            grab_key_focus: true
            RowWall := OutlinerRow{ type_icon +: { draw_icon +: { svg: crate_resource("self://resources/icons/el_wall.svg") } } }
            RowSlab := OutlinerRow{ type_icon +: { draw_icon +: { svg: crate_resource("self://resources/icons/el_slab.svg") } } }
            RowRoof := OutlinerRow{ type_icon +: { draw_icon +: { svg: crate_resource("self://resources/icons/el_roof.svg") } } }
            RowDoor := OutlinerRow{ type_icon +: { draw_icon +: { svg: crate_resource("self://resources/icons/el_door.svg") } } }
            RowWindow := OutlinerRow{ type_icon +: { draw_icon +: { svg: crate_resource("self://resources/icons/el_window.svg") } } }
            RowColumn := OutlinerRow{ type_icon +: { draw_icon +: { svg: crate_resource("self://resources/icons/el_column.svg") } } }
            RowBeam := OutlinerRow{ type_icon +: { draw_icon +: { svg: crate_resource("self://resources/icons/el_beam.svg") } } }
            RowStair := OutlinerRow{ type_icon +: { draw_icon +: { svg: crate_resource("self://resources/icons/el_stair.svg") } } }
            RowRailing := OutlinerRow{ type_icon +: { draw_icon +: { svg: crate_resource("self://resources/icons/el_railing.svg") } } }
            RowFurniture := OutlinerRow{ type_icon +: { draw_icon +: { svg: crate_resource("self://resources/icons/el_furniture.svg") } } }
            RowSite := OutlinerRow{ type_icon +: { draw_icon +: { svg: crate_resource("self://resources/icons/el_site.svg") } } }
            RowGroup := OutlinerRow{ type_icon +: { draw_icon +: { svg: crate_resource("self://resources/icons/el_group.svg") } } }
            RowStory := OutlinerRow{ type_icon +: { draw_icon +: { svg: crate_resource("self://resources/icons/story.svg") } } }
            RowMesh := OutlinerRow{ }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct KindGroupKey {
    level: ElementId,
    class: ElementClass,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum RowKey {
    Element(ElementId),
    KindGroup(KindGroupKey),
}

#[derive(Clone, Debug)]
struct TreeNode {
    key: RowKey,
    parent: Option<usize>,
    children: Vec<usize>,
}

/// The logical tree is rebuilt only when the document or filter changes.
/// Folding only re-flattens this index; it never creates or destroys widgets.
#[derive(Clone, Debug, Default)]
struct TreeIndex {
    roots: Vec<usize>,
    nodes: Vec<TreeNode>,
    by_key: HashMap<RowKey, usize>,
    element_nodes: Vec<Option<usize>>,
}

impl TreeIndex {
    fn with_element_capacity(len: usize) -> Self {
        Self {
            element_nodes: vec![None; len],
            ..Self::default()
        }
    }

    fn add(&mut self, key: RowKey, parent: Option<usize>) -> usize {
        let index = self.nodes.len();
        if let RowKey::Element(id) = &key {
            if let Some(slot) = self.element_nodes.get_mut(id.index()) {
                *slot = Some(index);
            }
        }
        self.by_key.insert(key.clone(), index);
        self.nodes.push(TreeNode {
            key,
            parent,
            children: Vec::new(),
        });
        if let Some(parent) = parent {
            self.nodes[parent].children.push(index);
        } else {
            self.roots.push(index);
        }
        index
    }

    fn element_node(&self, id: ElementId) -> Option<usize> {
        self.element_nodes.get(id.index()).copied().flatten()
    }
}

#[derive(Clone, Debug, Default)]
struct FoldState {
    open: HashSet<RowKey>,
}

/// One flattened, visible tree row. `node` points into the immutable logical
/// tree, so a fold update only replaces this small vector in O(n).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    node: usize,
    depth: usize,
    has_children: bool,
    expanded: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TreeCacheKey {
    generation: u64,
    search: String,
    classes: Vec<String>,
    pinned: Option<ElementId>,
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabOutliner {
    #[deref]
    view: View,
    #[rust]
    rows: Vec<Row>,
    #[rust]
    tree: TreeIndex,
    #[rust]
    tree_for: Option<TreeCacheKey>,
    #[rust]
    folds: HashMap<u64, FoldState>,
    #[rust]
    rows_dirty: bool,
    #[rust]
    filter_classes: Option<HashSet<String>>,
    #[rust]
    search: String,
    #[rust]
    debounce: Timer,
    #[rust]
    pending_search: Option<String>,
    #[rust]
    last_click: Option<RowKey>,
    #[rust]
    scroll_to: Option<RowKey>,
    #[rust]
    focused: Option<RowKey>,
    #[rust]
    local_selection: bool,
    #[rust]
    list_mouse_down: Option<DVec2>,
}

fn row_template(icon: Icon) -> LiveId {
    match icon {
        Icon::ElWall => live_id!(RowWall),
        Icon::ElSlab => live_id!(RowSlab),
        Icon::ElRoof => live_id!(RowRoof),
        Icon::ElDoor => live_id!(RowDoor),
        Icon::ElWindow => live_id!(RowWindow),
        Icon::ElColumn => live_id!(RowColumn),
        Icon::ElBeam => live_id!(RowBeam),
        Icon::ElStair => live_id!(RowStair),
        Icon::ElRailing => live_id!(RowRailing),
        Icon::ElFurniture => live_id!(RowFurniture),
        Icon::ElSite => live_id!(RowSite),
        Icon::ElGroup => live_id!(RowGroup),
        _ => live_id!(RowMesh),
    }
}

fn is_storey_node(state: &AppState, id: ElementId) -> bool {
    let Some(element) = state.scene.element(id) else {
        return false;
    };
    element
        .story
        .and_then(|story| state.scene.stories.get(story.index()))
        .and_then(|story| story.group)
        == Some(id)
}

fn is_kind_grouped(class: &ElementClass) -> bool {
    matches!(
        class,
        ElementClass::Wall | ElementClass::Slab | ElementClass::Roof | ElementClass::Window
    )
}

fn kind_group_name(class: &ElementClass, count: usize) -> String {
    let plural = match class {
        ElementClass::Wall => "Walls",
        ElementClass::Slab => "Slabs",
        ElementClass::Roof => "Roofs",
        ElementClass::Window => "Windows",
        _ => class.label(),
    };
    format!("{plural} ({count})")
}

fn element_survives(
    state: &AppState,
    id: ElementId,
    needle: &str,
    classes: Option<&HashSet<String>>,
) -> bool {
    let Some(element) = state.scene.element(id) else {
        return false;
    };
    if state.scene_state.selection.active == Some(id) {
        return true;
    }
    let name_matches = needle.is_empty() || element.name.to_lowercase().contains(needle);
    let class_matches = classes.map_or(true, |classes| classes.contains(element.class.label()));
    if name_matches && class_matches {
        return true;
    }
    state
        .scene
        .children(id)
        .iter()
        .any(|child| element_survives(state, *child, needle, classes))
}

fn append_element(
    tree: &mut TreeIndex,
    state: &AppState,
    id: ElementId,
    parent: Option<usize>,
    needle: &str,
    classes: Option<&HashSet<String>>,
    known_visible: bool,
) -> Option<usize> {
    if !known_visible && !element_survives(state, id, needle, classes) {
        return None;
    }
    let node = tree.add(RowKey::Element(id), parent);
    let source_children = state.scene.children(id);
    let visible_children: Vec<ElementId> = source_children
        .iter()
        .copied()
        .filter(|child| element_survives(state, *child, needle, classes))
        .collect();

    if is_storey_node(state, id) && source_children.len() > KIND_GROUP_THRESHOLD {
        for class in [
            ElementClass::Wall,
            ElementClass::Slab,
            ElementClass::Roof,
            ElementClass::Window,
        ] {
            let members: Vec<ElementId> = visible_children
                .iter()
                .copied()
                .filter(|child| {
                    state
                        .scene
                        .element(*child)
                        .map(|element| element.class == class)
                        .unwrap_or(false)
                })
                .collect();
            if members.is_empty() {
                continue;
            }
            let group = tree.add(
                RowKey::KindGroup(KindGroupKey {
                    level: id,
                    class: class.clone(),
                }),
                Some(node),
            );
            for child in members {
                append_element(tree, state, child, Some(group), needle, classes, true);
            }
        }
        for child in visible_children {
            let grouped = state
                .scene
                .element(child)
                .map(|element| is_kind_grouped(&element.class))
                .unwrap_or(false);
            if !grouped {
                append_element(tree, state, child, Some(node), needle, classes, true);
            }
        }
    } else {
        for child in visible_children {
            append_element(tree, state, child, Some(node), needle, classes, true);
        }
    }
    Some(node)
}

fn build_tree(
    state: &AppState,
    needle: &str,
    classes: Option<&HashSet<String>>,
) -> TreeIndex {
    let mut tree = TreeIndex::with_element_capacity(state.scene.elements.len());
    for root in &state.scene.tree.roots {
        append_element(&mut tree, state, *root, None, needle, classes, false);
    }
    tree
}

/// Pure visible-row mapping. Its work is bounded by the logical node count;
/// PortalList widgets are deliberately not involved.
fn visible_rows(tree: &TreeIndex, folds: &FoldState, force_open: bool) -> Vec<Row> {
    fn push(
        out: &mut Vec<Row>,
        tree: &TreeIndex,
        folds: &FoldState,
        node: usize,
        depth: usize,
        force_open: bool,
    ) {
        let Some(tree_node) = tree.nodes.get(node) else {
            return;
        };
        let has_children = !tree_node.children.is_empty();
        let expanded = has_children && (force_open || folds.open.contains(&tree_node.key));
        out.push(Row {
            node,
            depth,
            has_children,
            expanded,
        });
        if expanded {
            for child in &tree_node.children {
                push(out, tree, folds, *child, depth + 1, force_open);
            }
        }
    }

    let mut out = Vec::new();
    for root in &tree.roots {
        push(&mut out, tree, folds, *root, 0, force_open);
    }
    out
}

/// Pure reveal operation: open every ancestor and return the target's new
/// visible-row index. The target itself remains in its previous fold state.
fn reveal_element(
    tree: &TreeIndex,
    folds: &mut FoldState,
    id: ElementId,
    force_open: bool,
) -> Option<usize> {
    let target = tree.element_node(id)?;
    let mut parent = tree.nodes[target].parent;
    while let Some(index) = parent {
        folds.open.insert(tree.nodes[index].key.clone());
        parent = tree.nodes[index].parent;
    }
    visible_rows(tree, folds, force_open)
        .iter()
        .position(|row| row.node == target)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectionGesture {
    Replace,
    ExtendRange,
    Toggle,
}

/// Element IDs represented by one visible row. A leaf represents itself;
/// branch rows represent their descendant leaves, so synthetic storey and
/// kind-group nodes select the actual model elements shown beneath them.
fn row_element_ids(tree: &TreeIndex, node: usize) -> Vec<ElementId> {
    fn collect(tree: &TreeIndex, node: usize, out: &mut Vec<ElementId>) {
        let Some(tree_node) = tree.nodes.get(node) else {
            return;
        };
        if tree_node.children.is_empty() {
            if let RowKey::Element(id) = &tree_node.key {
                out.push(*id);
            }
            return;
        }
        for child in &tree_node.children {
            collect(tree, *child, out);
        }
    }

    let mut out = Vec::new();
    collect(tree, node, &mut out);
    out
}

fn push_unique(out: &mut Vec<ElementId>, seen: &mut HashSet<ElementId>, id: ElementId) {
    if seen.insert(id) {
        out.push(id);
    }
}

/// Pure selection mapping used by pointer handling and unit tests. Range
/// selection extends the existing set and row toggling treats a branch as one
/// logical row: all descendants are removed when all are selected, otherwise
/// all descendants are added.
fn selection_for_row_click(
    tree: &TreeIndex,
    rows: &[Row],
    index: usize,
    anchor: Option<&RowKey>,
    current: &HashSet<ElementId>,
    gesture: SelectionGesture,
) -> Vec<ElementId> {
    let Some(row) = rows.get(index) else {
        return Vec::new();
    };
    let clicked = row_element_ids(tree, row.node);
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    match gesture {
        SelectionGesture::Replace => {
            for id in clicked {
                push_unique(&mut out, &mut seen, id);
            }
        }
        SelectionGesture::ExtendRange => {
            // Put the clicked row first: `SelectSet` preserves its first ID as
            // active, which gives Properties a deterministic representative.
            for id in clicked {
                push_unique(&mut out, &mut seen, id);
            }
            let anchor_index = anchor.and_then(|anchor| {
                rows.iter()
                    .position(|row| tree.nodes[row.node].key == *anchor)
            });
            if let Some(anchor_index) = anchor_index {
                let (start, end) = if anchor_index <= index {
                    (anchor_index, index)
                } else {
                    (index, anchor_index)
                };
                for row in &rows[start..=end] {
                    for id in row_element_ids(tree, row.node) {
                        push_unique(&mut out, &mut seen, id);
                    }
                }
                let mut existing: Vec<ElementId> = current.iter().copied().collect();
                existing.sort();
                for id in existing {
                    push_unique(&mut out, &mut seen, id);
                }
            }
        }
        SelectionGesture::Toggle => {
            let remove = !clicked.is_empty() && clicked.iter().all(|id| current.contains(id));
            if !remove {
                for id in clicked.iter().copied() {
                    push_unique(&mut out, &mut seen, id);
                }
            }
            let mut existing: Vec<ElementId> = current.iter().copied().collect();
            existing.sort();
            for id in existing {
                if !remove || !clicked.contains(&id) {
                    push_unique(&mut out, &mut seen, id);
                }
            }
        }
    }
    out
}

impl FabOutliner {
    fn cache_key(&self, state: &AppState) -> TreeCacheKey {
        let mut classes: Vec<String> = self
            .filter_classes
            .iter()
            .flat_map(|classes| classes.iter().cloned())
            .collect();
        classes.sort();
        TreeCacheKey {
            generation: state.scene.generation,
            search: self.search.to_lowercase(),
            classes,
            // During filtering the active element is an explicit exception,
            // so viewport selection can always be revealed without silently
            // clearing the user's search or type filter.
            pinned: self
                .force_open()
                .then_some(state.scene_state.selection.active)
                .flatten(),
        }
    }

    fn force_open(&self) -> bool {
        !self.search.is_empty() || self.filter_classes.is_some()
    }

    fn ensure_tree(&mut self, state: &AppState) {
        let key = self.cache_key(state);
        if self.tree_for.as_ref() == Some(&key) {
            return;
        }
        self.tree = build_tree(state, &key.search, self.filter_classes.as_ref());
        let is_new_document = !self.folds.contains_key(&key.generation);
        let force_open = self.force_open();
        let folds = self.folds.entry(key.generation).or_default();
        if is_new_document {
            if let Some(id) = state.scene_state.selection.active {
                reveal_element(&self.tree, folds, id, force_open);
                self.scroll_to = Some(RowKey::Element(id));
                self.focused = Some(RowKey::Element(id));
            }
        }
        self.tree_for = Some(key);
        self.rows_dirty = true;
    }

    fn rebuild_rows(&mut self, state: &AppState) {
        self.ensure_tree(state);
        if !self.rows_dirty {
            return;
        }
        let generation = state.scene.generation;
        let force_open = self.force_open();
        let folds = self.folds.entry(generation).or_default();
        self.rows = visible_rows(&self.tree, folds, force_open);
        self.rows_dirty = false;
    }

    fn toggle(&mut self, key: &RowKey) {
        let Some(node) = self.tree.by_key.get(key).copied() else {
            return;
        };
        if self.tree.nodes[node].children.is_empty() || self.force_open() {
            return;
        }
        let Some(generation) = self.tree_for.as_ref().map(|key| key.generation) else {
            return;
        };
        let open = &mut self.folds.entry(generation).or_default().open;
        if !open.remove(key) {
            open.insert(key.clone());
        }
        self.rows_dirty = true;
    }

    /// Apply standard tree navigation. Returns true only when focus moved to a
    /// different row; opening/collapsing a branch alone does not reselect it.
    fn handle_tree_key(&mut self, key_code: KeyCode) -> bool {
        let old_focus = self.focused.clone();
        if self.focused.is_none() {
            self.focused = match key_code {
                KeyCode::ArrowUp => self
                    .rows
                    .last()
                    .map(|row| self.tree.nodes[row.node].key.clone()),
                KeyCode::ArrowDown => self
                    .rows
                    .first()
                    .map(|row| self.tree.nodes[row.node].key.clone()),
                _ => None,
            };
            if self.focused != old_focus {
                self.scroll_to = self.focused.clone();
                return true;
            }
            return false;
        }
        let key = self.focused.clone().unwrap();
        let Some(node) = self.tree.by_key.get(&key).copied() else {
            return false;
        };
        let Some(generation) = self.tree_for.as_ref().map(|key| key.generation) else {
            return false;
        };
        let folds = self.folds.entry(generation).or_default();
        let mut rows_changed = false;
        match key_code {
            KeyCode::ArrowUp | KeyCode::ArrowDown => {
                if let Some(index) = self.rows.iter().position(|row| row.node == node) {
                    let next = if key_code == KeyCode::ArrowUp {
                        index.checked_sub(1)
                    } else {
                        (index + 1 < self.rows.len()).then_some(index + 1)
                    };
                    if let Some(next) = next {
                        self.focused = Some(self.tree.nodes[self.rows[next].node].key.clone());
                    }
                }
            }
            KeyCode::ArrowRight if !self.tree.nodes[node].children.is_empty() => {
                if folds.open.insert(key.clone()) {
                    rows_changed = true;
                } else if let Some(child) = self.tree.nodes[node].children.first() {
                    self.focused = Some(self.tree.nodes[*child].key.clone());
                }
            }
            KeyCode::ArrowLeft => {
                if folds.open.remove(&key) {
                    rows_changed = true;
                } else if let Some(parent) = self.tree.nodes[node].parent {
                    self.focused = Some(self.tree.nodes[parent].key.clone());
                }
            }
            _ => {}
        }
        if rows_changed {
            self.rows_dirty = true;
        }
        let focus_moved = self.focused != old_focus;
        if focus_moved {
            self.scroll_to = self.focused.clone();
        }
        focus_moved
    }

    fn reveal(&mut self, state: &AppState, id: ElementId, scroll: bool) {
        self.ensure_tree(state);
        let force_open = self.force_open();
        let folds = self.folds.entry(state.scene.generation).or_default();
        if reveal_element(&self.tree, folds, id, force_open).is_some() {
            self.rows_dirty = true;
            self.focused = Some(RowKey::Element(id));
            if scroll {
                self.scroll_to = Some(RowKey::Element(id));
            }
        }
    }

    fn select_row(
        &mut self,
        cx: &mut Cx,
        state: &AppState,
        index: usize,
        gesture: SelectionGesture,
    ) {
        let ids = selection_for_row_click(
            &self.tree,
            &self.rows,
            index,
            self.last_click.as_ref(),
            &state.scene_state.selection.set,
            gesture,
        );
        self.local_selection = true;
        cx.action(ShellAction::SelectSet(ids));
    }

    fn clear_selection(&mut self, cx: &mut Cx) {
        self.focused = None;
        self.last_click = None;
        self.local_selection = true;
        cx.action(ShellAction::ClearSelection);
        self.view.redraw(cx);
    }

    fn context_menu_items(state: &AppState) -> Vec<MenuItem> {
        let has_sel = !state.scene_state.selection.is_empty();
        let mut items = vec![
            MenuItem::new(live_id!(select_only), "Select Only"),
            MenuItem::new(live_id!(select_children), "Select With Children"),
            MenuItem::sep(),
            MenuItem::new(live_id!(hide), "Hide").key("H"),
            MenuItem::new(live_id!(isolate), "Isolate").key("Shift+H"),
            MenuItem::new(live_id!(unhide), "Unhide All").key("Alt+H"),
            MenuItem::sep(),
            MenuItem::new(live_id!(frame), "Frame Selected").key("."),
            MenuItem::new(live_id!(expand_all), "Expand All"),
            MenuItem::new(live_id!(collapse_all), "Collapse All"),
        ];
        if !has_sel {
            for it in items.iter_mut() {
                if it.id == live_id!(hide)
                    || it.id == live_id!(isolate)
                    || it.id == live_id!(frame)
                    || it.id == live_id!(select_children)
                {
                    it.enabled = false;
                }
            }
        }
        items
    }

    fn filter_menu_items(&self, state: &AppState) -> Vec<MenuItem> {
        let mut classes: Vec<String> = Vec::new();
        for e in &state.scene.elements {
            let l = e.class.label().to_string();
            if !classes.contains(&l) {
                classes.push(l);
            }
        }
        classes.sort();
        let mut items = vec![MenuItem::new(live_id!(all), "All Types")
            .checked(self.filter_classes.is_none())];
        items.push(MenuItem::sep());
        for c in classes.iter().take(24) {
            let on = self
                .filter_classes
                .as_ref()
                .map(|s| s.contains(c))
                .unwrap_or(false);
            items.push(MenuItem::new(LiveId::from_str(c), c).checked(on));
        }
        items
    }

    fn select_subtree(state: &AppState, id: ElementId, out: &mut Vec<ElementId>) {
        out.push(id);
        for c in state.scene.children(id) {
            Self::select_subtree(state, *c, out);
        }
    }

}

impl Widget for FabOutliner {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);

        if let Event::KeyDown(key) = event {
            let list = self.view.portal_list(cx, ids!(list));
            if cx.has_key_focus(list.area()) {
                if key.key_code == KeyCode::Escape {
                    self.clear_selection(cx);
                } else {
                    let tree_key = matches!(
                        key.key_code,
                        KeyCode::ArrowUp
                            | KeyCode::ArrowDown
                            | KeyCode::ArrowLeft
                            | KeyCode::ArrowRight
                    ) && (!self.force_open()
                        || matches!(key.key_code, KeyCode::ArrowUp | KeyCode::ArrowDown));
                    if tree_key {
                        if let Some(state) = scope.data.get_mut::<AppState>() {
                            self.rebuild_rows(state);
                        }
                        let focus_moved = self.handle_tree_key(key.key_code);
                        if focus_moved {
                            if let Some(index) = self.focused.as_ref().and_then(|focused| {
                                self.rows.iter().position(|row| {
                                    self.tree.nodes[row.node].key == *focused
                                })
                            }) {
                                if let Some(state) = scope.data.get_mut::<AppState>() {
                                    self.select_row(cx, state, index, SelectionGesture::Replace);
                                }
                                self.last_click = self.focused.clone();
                            }
                        }
                        self.view.redraw(cx);
                    }
                }
            }
        }

        // PortalList owns the background hit, while row Views own row hits.
        // Track a raw mouse tap so the uncovered space below the final virtual
        // row can clear selection without stealing clicks from rows or eyes.
        let list = self.view.portal_list(cx, ids!(list));
        match event {
            Event::MouseDown(down)
                if down.button.is_primary()
                    && list.area().clipped_rect(cx).contains(down.abs) =>
            {
                self.list_mouse_down = Some(down.abs);
            }
            Event::MouseUp(up) if up.button.is_primary() => {
                let start = self.list_mouse_down.take();
                let is_tap = start.is_some_and(|start| (start - up.abs).length() < 5.0);
                if is_tap && list.area().clipped_rect(cx).contains(up.abs) {
                    let over_row = self.rows.iter().enumerate().any(|(index, _)| {
                        list.get_item(index)
                            .map(|(_, item)| item.area().clipped_rect(cx).contains(up.abs))
                            .unwrap_or(false)
                    });
                    if !over_row {
                        self.clear_selection(cx);
                    }
                }
            }
            _ => {}
        }

        if self.debounce.is_event(event).is_some() {
            if let Some(t) = self.pending_search.take() {
                self.search = t.clone();
                self.tree_for = None;
                cx.action(ShellAction::SetOutlinerFilter(t));
                self.view.redraw(cx);
            }
        }

        let Event::Actions(actions) = event else {
            return;
        };

        // Search, debounced — retyping should not rebuild the tree per key.
        if let Some(text) = self
            .view
            .text_input(cx, ids!(header.search.row.input))
            .changed(actions)
        {
            self.pending_search = Some(text);
            cx.stop_timer(self.debounce);
            self.debounce = cx.start_timeout(0.15);
        }

        // The funnel.
        if let Some(anchor) = dropdown_clicked(actions, live_id!(outliner_filter)) {
            if let Some(state) = scope.data.get_mut::<AppState>() {
                let items = self.filter_menu_items(state);
                open_menu(cx, FILTER_MENU, items, anchor, MenuPlace::BelowRight);
            }
        }
        if let Some(pick) = menu_picked(actions, FILTER_MENU) {
            if pick == live_id!(all) {
                self.filter_classes = None;
            } else if let Some(state) = scope.data.get_mut::<AppState>() {
                let mut set = self.filter_classes.take().unwrap_or_default();
                let mut hit = None;
                for e in &state.scene.elements {
                    let l = e.class.label().to_string();
                    if LiveId::from_str(&l) == pick {
                        hit = Some(l);
                        break;
                    }
                }
                if let Some(l) = hit {
                    if !set.remove(&l) {
                        set.insert(l);
                    }
                }
                self.filter_classes = if set.is_empty() { None } else { Some(set) };
            }
            self.tree_for = None;
            self.view.redraw(cx);
        }

        if let Some(state) = scope.data.get_mut::<AppState>() {
            self.rebuild_rows(state);
        }

        // Rows.
        let rows = self.rows.clone();
        let list = self.view.portal_list(cx, ids!(list));
        let mut context_at: Option<(ElementId, DVec2)> = None;
        for (index, row) in rows.iter().enumerate() {
            let Some(key) = self.tree.nodes.get(row.node).map(|node| node.key.clone()) else {
                continue;
            };
            let element_id = match &key {
                RowKey::Element(id) => Some(*id),
                RowKey::KindGroup(_) => None,
            };
            let item = list.get_item(index).map(|(_, w)| w);
            let Some(item) = item else { continue };
            if item.button(cx, ids!(eye)).clicked(actions)
                || item.button(cx, ids!(eye_off)).clicked(actions)
            {
                if let (Some(id), Some(state)) =
                    (element_id, scope.data.get_mut::<AppState>())
                {
                    let hidden = state.scene_state.hidden.contains(&id);
                    cx.action(ShellAction::SetHidden(id, !hidden));
                }
                continue;
            }
            if item
                .widget(cx, ids!(twist))
                .as_view()
                .finger_up(actions)
                .map(|up| up.is_over && up.was_tap())
                .unwrap_or(false)
            {
                self.focused = Some(key.clone());
                self.toggle(&key);
                self.view.redraw(cx);
                continue;
            }
            if let Some(up) = item.as_view().finger_up(actions) {
                if !up.is_over || !up.was_tap() {
                    continue;
                }
                let secondary = up
                    .device
                    .mouse_button()
                    .map(|b| !b.is_primary())
                    .unwrap_or(false);
                if secondary {
                    if let Some(id) = element_id {
                        if let Some(state) = scope.data.get_mut::<AppState>() {
                            if !state.scene_state.selection.contains(id) {
                                self.local_selection = true;
                                cx.action(ShellAction::SelectOnly(id));
                            }
                        }
                        context_at = Some((id, up.abs));
                    }
                } else {
                    self.focused = Some(key.clone());
                    let m = up.modifiers;
                    let gesture = if m.shift {
                        SelectionGesture::ExtendRange
                    } else if m.control || m.logo {
                        SelectionGesture::Toggle
                    } else {
                        SelectionGesture::Replace
                    };
                    if let Some(state) = scope.data.get_mut::<AppState>() {
                        self.select_row(cx, state, index, gesture);
                    }
                    if gesture != SelectionGesture::ExtendRange {
                        self.last_click = Some(key);
                    }
                    if up.tap_count >= 2 {
                        cx.action(ShellAction::FrameSelectedAll);
                    }
                }
            }
        }
        if let Some((_, at)) = context_at {
            if let Some(state) = scope.data.get_mut::<AppState>() {
                let items = Self::context_menu_items(state);
                open_menu(
                    cx,
                    OUTLINER_MENU,
                    items,
                    Rect {
                        pos: at,
                        size: dvec2(0.0, 0.0),
                    },
                    MenuPlace::At,
                );
            }
        }

        if let Some(pick) = menu_picked(actions, OUTLINER_MENU) {
            let active = scope
                .data
                .get_mut::<AppState>()
                .and_then(|s| s.scene_state.selection.active);
            match pick {
                x if x == live_id!(hide) => cx.action(ShellAction::HideSelected),
                x if x == live_id!(isolate) => cx.action(ShellAction::IsolateSelected),
                x if x == live_id!(unhide) => cx.action(ShellAction::UnhideAll),
                x if x == live_id!(frame) => cx.action(ShellAction::FrameSelectedAll),
                x if x == live_id!(select_only) => {
                    if let Some(id) = active {
                        cx.action(ShellAction::SelectOnly(id));
                    }
                }
                x if x == live_id!(select_children) => {
                    if let (Some(id), Some(state)) = (active, scope.data.get_mut::<AppState>()) {
                        let mut out = Vec::new();
                        Self::select_subtree(state, id, &mut out);
                        cx.action(ShellAction::SelectSet(out));
                    }
                }
                x if x == live_id!(expand_all) => {
                    if let Some(generation) =
                        self.tree_for.as_ref().map(|key| key.generation)
                    {
                        let folds = self.folds.entry(generation).or_default();
                        for node in &self.tree.nodes {
                            if !node.children.is_empty() {
                                folds.open.insert(node.key.clone());
                            }
                        }
                    }
                    self.rows_dirty = true;
                    self.view.redraw(cx);
                }
                x if x == live_id!(collapse_all) => {
                    if let Some(generation) =
                        self.tree_for.as_ref().map(|key| key.generation)
                    {
                        self.folds.entry(generation).or_default().open.clear();
                    }
                    self.rows_dirty = true;
                    self.view.redraw(cx);
                }
                _ => {}
            }
        }

        // Selection actions have already been applied to AppState by the app's
        // MatchEvent pass. This catches ordinary viewport clicks as well as the
        // explicit context-menu reveal action.
        let explicit_reveal = actions
            .iter()
            .filter_map(|a| a.downcast_ref::<ShellAction>())
            .find_map(|a| match a {
                ShellAction::RevealInOutliner(id) => Some(*id),
                _ => None,
            });
        let selection_changed = actions
            .iter()
            .filter_map(|a| a.downcast_ref::<ShellAction>())
            .any(|action| {
                matches!(
                    action,
                    ShellAction::SelectOnly(_)
                        | ShellAction::SelectToggle(_)
                        | ShellAction::SelectAdd(_)
                        | ShellAction::SelectSet(_)
                        | ShellAction::ClearSelection
                )
            });
        if explicit_reveal.is_some() || selection_changed {
            let from_this_outliner = std::mem::take(&mut self.local_selection);
            if explicit_reveal.is_some() || !from_this_outliner {
                if let Some(state) = scope.data.get_mut::<AppState>() {
                    if let Some(id) = explicit_reveal.or(state.scene_state.selection.active) {
                        self.reveal(state, id, true);
                    }
                }
            }
            self.view.redraw(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if let Some(state) = scope.data.get_mut::<AppState>() {
            self.rebuild_rows(state);
        }
        let rows = self.rows.clone();
        let nodes = self.tree.nodes.clone();
        let focused = self.focused.clone();
        let scroll_to = self.scroll_to.take();

        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let list_ref = step.as_portal_list();
            let Some(mut list) = list_ref.borrow_mut() else {
                continue;
            };
            let Some(state) = scope.data.get_mut::<AppState>() else {
                continue;
            };
            list.set_item_range(cx, 0, rows.len());
            if let Some(key) = scroll_to.as_ref() {
                if let Some(i) = rows
                    .iter()
                    .position(|row| nodes[row.node].key == *key)
                {
                    list.set_first_id_and_scroll(i.saturating_sub(4), 0.0);
                }
            }
            while let Some(i) = list.next_visible_item(cx) {
                let Some(row) = rows.get(i) else { continue };
                let Some(node) = nodes.get(row.node) else { continue };
                let (element_id, class, name, storey) = match &node.key {
                    RowKey::Element(id) => {
                        let Some(element) = state.scene.element(*id) else { continue };
                        (
                            Some(*id),
                            element.class.clone(),
                            element.name.clone(),
                            is_storey_node(state, *id),
                        )
                    }
                    RowKey::KindGroup(group) => (
                        None,
                        group.class.clone(),
                        kind_group_name(&group.class, node.children.len()),
                        false,
                    ),
                };
                let icon = element_icon(&class);
                let template = if storey {
                    live_id!(RowStory)
                } else {
                    row_template(icon)
                };
                let item = list.item(cx, i, template);

                item.label(cx, ids!(name)).set_text(cx, &name);
                let mut indent = item.widget(cx, ids!(indent));
                let w = row.depth as f64 * INDENT;
                script_apply_eval!(cx, indent, { width: #(w) });

                item.widget(cx, ids!(twist)).set_visible(cx, row.has_children);
                item.widget(cx, ids!(twist_gap))
                    .set_visible(cx, !row.has_children);
                if row.has_children {
                    let mut tw = item.widget(cx, ids!(twist.tw));
                    let a: f32 = if row.expanded {
                        0.0
                    } else {
                        -std::f32::consts::FRAC_PI_2
                    };
                    script_apply_eval!(cx, tw, { draw_icon +: { rotation_angle: #(a) } });
                }

                let hidden = element_id
                    .map(|id| state.scene_state.hidden.contains(&id))
                    .unwrap_or(false);
                item.widget(cx, ids!(eye))
                    .set_visible(cx, element_id.is_some() && !hidden);
                item.widget(cx, ids!(eye_off))
                    .set_visible(cx, element_id.is_some() && hidden);

                let represented = row_element_ids(&self.tree, row.node);
                let selected = !represented.is_empty()
                    && represented
                        .iter()
                        .all(|id| state.scene_state.selection.contains(*id));
                let active = element_id == state.scene_state.selection.active
                    || focused.as_ref() == Some(&node.key);
                let mut bg = item.as_view();
                let s = if selected { 1.0f32 } else { 0.0 };
                let a = if active { 1.0f32 } else { 0.0 };
                let odd = if i % 2 == 1 { 1.0f32 } else { 0.0 };
                let dim = if hidden {
                    vec4(0.55, 0.55, 0.55, 1.0)
                } else if active {
                    vec4(1.0, 1.0, 1.0, 1.0)
                } else {
                    vec4(0.90, 0.90, 0.90, 1.0)
                };
                script_apply_eval!(cx, bg, {
                    draw_bg +: { selected: #(s) active: #(a) odd: #(odd) }
                });
                let mut nm = item.widget(cx, ids!(name));
                script_apply_eval!(cx, nm, {
                    draw_text +: { color: #(dim) }
                });

                item.draw_all(cx, &mut Scope::empty());
            }
        }
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(id: usize) -> RowKey {
        RowKey::Element(ElementId::from_index(id))
    }

    fn row_keys(tree: &TreeIndex, rows: &[Row]) -> Vec<RowKey> {
        rows.iter()
            .map(|row| tree.nodes[row.node].key.clone())
            .collect()
    }

    fn grouped_tree() -> (TreeIndex, RowKey) {
        let mut tree = TreeIndex::with_element_capacity(4);
        let level = tree.add(element(0), None);
        let group_key = RowKey::KindGroup(KindGroupKey {
            level: ElementId::from_index(0),
            class: ElementClass::Wall,
        });
        let group = tree.add(group_key.clone(), Some(level));
        tree.add(element(1), Some(group));
        tree.add(element(2), Some(group));
        tree.add(element(3), None);
        (tree, group_key)
    }

    #[test]
    fn fold_state_maps_to_only_visible_rows() {
        let (tree, group) = grouped_tree();
        let mut folds = FoldState::default();

        let folded = visible_rows(&tree, &folds, false);
        assert_eq!(row_keys(&tree, &folded), vec![element(0), element(3)]);

        folds.open.insert(element(0));
        let level_open = visible_rows(&tree, &folds, false);
        assert_eq!(
            row_keys(&tree, &level_open),
            vec![element(0), group.clone(), element(3)]
        );
        assert_eq!(
            level_open.iter().map(|row| row.depth).collect::<Vec<_>>(),
            vec![0, 1, 0]
        );

        folds.open.insert(group.clone());
        let all_open = visible_rows(&tree, &folds, false);
        assert_eq!(
            row_keys(&tree, &all_open),
            vec![element(0), group, element(1), element(2), element(3)]
        );
    }

    #[test]
    fn reveal_opens_ancestors_and_returns_visible_index() {
        let (tree, group) = grouped_tree();
        let mut folds = FoldState::default();

        assert_eq!(
            reveal_element(&tree, &mut folds, ElementId::from_index(2), false),
            Some(3)
        );
        assert!(folds.open.contains(&element(0)));
        assert!(folds.open.contains(&group));
        assert!(!folds.open.contains(&element(2)));
        assert_eq!(
            row_keys(&tree, &visible_rows(&tree, &folds, false)),
            vec![element(0), group, element(1), element(2), element(3)]
        );
    }

    fn all_rows(tree: &TreeIndex, group: &RowKey) -> Vec<Row> {
        let mut folds = FoldState::default();
        folds.open.insert(element(0));
        folds.open.insert(group.clone());
        visible_rows(tree, &folds, false)
    }

    #[test]
    fn rows_map_to_leaf_element_ids() {
        let (tree, group) = grouped_tree();
        let rows = all_rows(&tree, &group);

        assert_eq!(row_element_ids(&tree, rows[0].node), vec![
            ElementId::from_index(1),
            ElementId::from_index(2),
        ]);
        assert_eq!(row_element_ids(&tree, rows[1].node), vec![
            ElementId::from_index(1),
            ElementId::from_index(2),
        ]);
        assert_eq!(
            row_element_ids(&tree, rows[2].node),
            vec![ElementId::from_index(1)]
        );
    }

    #[test]
    fn row_selection_replaces_extends_ranges_and_toggles() {
        let (tree, group) = grouped_tree();
        let rows = all_rows(&tree, &group);
        let id1 = ElementId::from_index(1);
        let id2 = ElementId::from_index(2);
        let id3 = ElementId::from_index(3);

        let current = HashSet::from([id3]);
        assert_eq!(
            selection_for_row_click(
                &tree,
                &rows,
                2,
                None,
                &current,
                SelectionGesture::Replace,
            ),
            vec![id1]
        );
        assert_eq!(
            selection_for_row_click(
                &tree,
                &rows,
                3,
                Some(&element(1)),
                &current,
                SelectionGesture::ExtendRange,
            ),
            vec![id2, id1, id3]
        );

        let pair = HashSet::from([id1, id2]);
        assert_eq!(
            selection_for_row_click(
                &tree,
                &rows,
                3,
                None,
                &pair,
                SelectionGesture::Toggle,
            ),
            vec![id1]
        );
        assert_eq!(
            selection_for_row_click(
                &tree,
                &rows,
                3,
                None,
                &HashSet::from([id1]),
                SelectionGesture::Toggle,
            ),
            vec![id2, id1]
        );
    }

    #[test]
    fn storey_and_kind_rows_toggle_as_one_multi_selection() {
        let (tree, group) = grouped_tree();
        let rows = all_rows(&tree, &group);
        let id1 = ElementId::from_index(1);
        let id2 = ElementId::from_index(2);
        let id3 = ElementId::from_index(3);
        let current = HashSet::from([id1, id2, id3]);

        for index in [0, 1] {
            assert_eq!(
                selection_for_row_click(
                    &tree,
                    &rows,
                    index,
                    None,
                    &current,
                    SelectionGesture::Toggle,
                ),
                vec![id3]
            );
        }
    }
}
