use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_micro_serde::*,
    splitter::{Splitter, SplitterAction, SplitterAlign, SplitterAxis},
    tab::Tab,
    tab_bar::{TabBar, TabBarAction},
    widget::*,
    widget_tree::CxWidgetExt,
};
use std::collections::HashMap;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawRoundCorner = set_type_default() do #(DrawRoundCorner::script_shader(vm)){
        ..mod.draw.DrawQuad
        border_radius: 20.
        flip: vec2(0.0, 0.0)
    }

    // Register DockItem enum variants for DSL parsing (prefixed to avoid conflict with widgets)
    mod.widgets.DockSplitter = #(DockItemSplitter::script_api(vm))
    mod.widgets.DockTabs = #(DockItemTabs::script_api(vm))
    mod.widgets.DockTab = #(DockItemTab::script_api(vm))

    mod.widgets.DockBase = #(Dock::register_widget(vm))

    mod.widgets.Dock = set_type_default() do mod.widgets.DockBase{
        flow: Down

        tab_bar: TabBarGradientY{}
        splitter: Splitter{}

        padding: Inset{left: theme.dock_border_size, top: 0, right: theme.dock_border_size, bottom: theme.dock_border_size}

        round_corner +: {
            border_radius: 20.
            color: instance(theme.color_bg_app)
            flip: vec2(0.0, 0.0)

            pixel: fn() {
                let pos = vec2(
                    mix(self.pos.x, 1.0 - self.pos.x, self.flip.x)
                    mix(self.pos.y, 1.0 - self.pos.y, self.flip.y)
                )

                let sdf = Sdf2d.viewport(pos * self.rect_size)
                sdf.rect(-10., -10., self.rect_size.x * 2.0, self.rect_size.y * 2.0)
                sdf.box(
                    0.25
                    0.25
                    self.rect_size.x * 2.0
                    self.rect_size.y * 2.0
                    4.0
                )

                sdf.subtract()

                sdf.fill(self.color)
                return sdf.result
            }
        }
        drag_target_preview +: {
            draw_depth: 10.0
            color: theme.color_drag_target_preview
        }
    }

    mod.widgets.DockFlat = mod.widgets.DockBase{
        flow: Down

        tab_bar: TabBarFlat{}
        splitter: Splitter{}

        padding: Inset{left: theme.dock_border_size, top: 0, right: theme.dock_border_size, bottom: theme.dock_border_size}

        round_corner +: {
            border_radius: 20.

            pixel: fn() {
                let pos = vec2(
                    mix(self.pos.x, 1.0 - self.pos.x, self.flip.x)
                    mix(self.pos.y, 1.0 - self.pos.y, self.flip.y)
                )

                let sdf = Sdf2d.viewport(pos * self.rect_size)
                sdf.rect(-10., -10., self.rect_size.x * 2.0, self.rect_size.y * 2.0)
                sdf.box(
                    0.25
                    0.25
                    self.rect_size.x * 2.0
                    self.rect_size.y * 2.0
                    4.0
                )

                sdf.subtract()
                return sdf.fill(theme.color_bg_app)
            }
        }

        drag_target_preview +: {
            draw_depth: 10.0
            color: theme.color_drag_target_preview
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawRoundCorner {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    draw_super: DrawQuad,
    #[live]
    border_radius: f32,
    #[live]
    flip: Vec2f,
}

impl DrawRoundCorner {
    fn draw_corners(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.flip = vec2(0.0, 0.0);
        let rad = dvec2(self.border_radius as f64, self.border_radius as f64);
        let pos = rect.pos;
        let size = rect.size;
        self.draw_abs(cx, Rect { pos, size: rad });
        self.flip = vec2(1.0, 0.0);
        self.draw_abs(
            cx,
            Rect {
                pos: pos + dvec2(size.x - rad.x, 0.),
                size: rad,
            },
        );
        self.flip = vec2(1.0, 1.0);
        self.draw_abs(
            cx,
            Rect {
                pos: pos + dvec2(size.x - rad.x, size.y - rad.y),
                size: rad,
            },
        );
        self.flip = vec2(0.0, 1.0);
        self.draw_abs(
            cx,
            Rect {
                pos: pos + dvec2(0., size.y - rad.y),
                size: rad,
            },
        );
    }
}

#[derive(Script, WidgetRegister, WidgetRef, WidgetSet)]
pub struct Dock {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[rust]
    draw_state: DrawStateWrap<Vec<DrawStackItem>>,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    drop_target_draw_list: DrawList2d,
    #[live]
    round_corner: DrawRoundCorner,
    #[live]
    drag_target_preview: DrawColor,
    #[live]
    ghost_tab_draw_list: DrawList2d,

    #[live]
    tab_bar: ScriptObjectRef,
    #[live]
    splitter: ScriptObjectRef,

    #[rust]
    needs_save: bool,
    #[rust]
    area: Area,

    #[rust]
    tab_bars: ComponentMap<LiveId, TabBarWrap>,
    #[rust]
    splitters: ComponentMap<LiveId, Splitter>,

    #[rust]
    dock_items: HashMap<LiveId, DockItem>,
    #[rust]
    templates: HashMap<LiveId, ScriptObjectRef>,
    #[rust]
    items: ComponentMap<LiveId, (LiveId, WidgetRef)>,
    #[rust]
    drop_state: Option<DropPosition>,
    /// Info about the tab currently being dragged, for ghost tab rendering.
    #[rust]
    dragging_tab: Option<DraggingTab>,
    #[rust]
    dock_item_iter_stack: Vec<(LiveId, usize)>,
    /// Monotonic counter for drag/drop-allocated container IDs. Lives in the
    /// reserved range above INTERNAL_ID_FLOOR so it can't collide with DSL hashes.
    #[rust]
    next_internal_id: u64,
}

/// Floor for internally-generated dock-item IDs. Name-hash LiveIds are 46 bits,
/// so bit 47 keeps our generated IDs safely out of their range.
const INTERNAL_ID_FLOOR: u64 = 1 << 47;

/// Holds a clone of the tab being dragged so we can render it as a ghost overlay.
struct DraggingTab {
    cursor: Vec2d,
    name: String,
    /// The size of the original tab, used for fixed-size rendering.
    size: Vec2d,
    ghost: Tab,
}

impl ScriptHook for Dock {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_reload() {
            self.templates.clear();
        }
        if apply.is_new() {
            self.dock_items.clear();
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        // Collect templates and dock items from the object's vec. Only
        // during template applies (not eval) to avoid storing temporaries.
        //
        // Content templates follow both LiveEdit and style reapplication.
        // Dock items describe the layout, which may no longer contain its
        // initial panes or tabs. ScriptReapply must not reintroduce those
        // missing IDs: a merged-away pane would become an orphan containing
        // duplicate tabs, and a closed tab could reappear. Only a true
        // LiveEdit reload may add new default layout items; existing runtime
        // items remain unchanged as before.
        let is_reload = apply.is_reload();
        let collect_layout = !apply.is_script_reapply();
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        if let Some(id) = kv.key.as_id() {
                            // Check type and parse accordingly
                            if let Some(val_obj) = kv.value.as_object() {
                                if vm.bx.heap.type_matches_id(
                                    val_obj,
                                    DockItemSplitter::script_type_id_static(),
                                ) {
                                    if collect_layout
                                        && (!is_reload || !self.dock_items.contains_key(&id))
                                    {
                                        let splitter =
                                            DockItemSplitter::script_from_value(vm, kv.value);
                                        self.dock_items.insert(id, splitter.to_dock_item());
                                    }
                                } else if vm
                                    .bx
                                    .heap
                                    .type_matches_id(val_obj, DockItemTabs::script_type_id_static())
                                {
                                    if collect_layout
                                        && (!is_reload || !self.dock_items.contains_key(&id))
                                    {
                                        let tabs = DockItemTabs::script_from_value(vm, kv.value);
                                        self.dock_items.insert(id, tabs.to_dock_item());
                                    }
                                } else if vm
                                    .bx
                                    .heap
                                    .type_matches_id(val_obj, DockItemTab::script_type_id_static())
                                {
                                    if collect_layout
                                        && (!is_reload || !self.dock_items.contains_key(&id))
                                    {
                                        let tab = DockItemTab::script_from_value(vm, kv.value);
                                        self.dock_items.insert(id, tab.to_dock_item());
                                    }
                                } else {
                                    // Not a dock item, treat as content template - root it
                                    self.templates
                                        .insert(id, vm.bx.heap.new_object_ref(val_obj));
                                }
                            }
                            // Non-object values can't be rooted, skip them for templates
                        }
                    }
                });
            }
        }

        // Update existing items if templates changed
        if apply.is_reload() {
            for (kind, widget) in self.items.values_mut() {
                if let Some(template_ref) = self.templates.get(kind) {
                    let template_value: ScriptValue = template_ref.as_object().into();
                    widget.script_apply(vm, apply, scope, template_value);
                }
            }

            // Update tab_bars with the tab_bar template
            if !self.tab_bar.is_zero() {
                for tab_bar in self.tab_bars.values_mut() {
                    tab_bar
                        .tab_bar
                        .script_apply(vm, apply, scope, self.tab_bar.as_object().into());
                }
            }

            // Update splitters with the splitter template
            if !self.splitter.is_zero() {
                for splitter in self.splitters.values_mut() {
                    splitter.script_apply(vm, apply, scope, self.splitter.as_object().into());
                }
            }
        }

        // Create items for all tabs if this is new
        if apply.is_new() {
            self.create_all_items_with_vm(vm);
        }
        vm.cx_mut().widget_tree_mark_dirty(self.uid);
    }
}

impl WidgetNode for Dock {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (id, (_, widget)) in self.items.iter() {
            visit(*id, widget.clone());
        }
        // The tabs are widgets too (the design tweaker picks and styles
        // them); the bars that own them are not, so they surface here.
        for (_, tab_bar) in self.tab_bars.iter() {
            for (id, tab) in tab_bar.tab_bar.tab_refs() {
                visit(id, tab);
            }
        }
    }

    fn cancel_children_impl(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) -> bool {
        fn walk(dock: &Dock, id: LiveId, remaining: &mut usize, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
            // Malformed restored layouts can contain cycles.
            if *remaining == 0 {
                return;
            }
            *remaining -= 1;
            match dock.dock_items.get(&id) {
                Some(DockItem::Splitter { a, b, .. }) => {
                    walk(dock, *a, remaining, visit);
                    walk(dock, *b, remaining, visit);
                }
                Some(DockItem::Tabs { tabs, selected, hide_tab_bar, .. }) => {
                    if !hide_tab_bar {
                        if let Some(tab_bar) = dock.tab_bars.get(&id) {
                            for id in tabs {
                                if let Some((name, tab)) = tab_bar.tab_bar.tab_ref(*id) {
                                    visit(name, tab);
                                }
                            }
                        }
                    }
                    if let Some(selected) = tabs.get(*selected) {
                        walk(dock, *selected, remaining, visit);
                    }
                }
                Some(DockItem::Tab { .. }) => {
                    if let Some((_, widget)) = dock.items.get(&id) {
                        visit(id, widget.clone());
                    }
                }
                None => {}
            }
        }
        walk(self, id!(root), &mut self.dock_items.len(), visit);
        true
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
        // A redraw of the dock is a redraw of everything it shows. Each
        // panel's tab content lives behind a RETAINED draw list
        // (`contents_draw_list`, gated with `is_redrawing()` in draw), so
        // marking only the dock's own area leaves every tab's content cached:
        // an app-level `ui.redraw(cx)` would repaint the dock chrome while
        // the panels inside — viewports included — kept showing stale
        // pixels. Mark the retained lists and recurse into the items so the
        // redraw contract (a widget redraws its whole subtree) holds.
        for (_, tab_bar) in self.tab_bars.iter_mut() {
            tab_bar.contents_draw_list.redraw(cx);
        }
        for (_, (_, item)) in self.items.iter_mut() {
            item.redraw(cx);
        }
    }
}

pub struct DockVisibleItemIterator<'a> {
    stack: &'a mut Vec<(LiveId, usize)>,
    dock_items: &'a HashMap<LiveId, DockItem>,
    items: &'a ComponentMap<LiveId, (LiveId, WidgetRef)>,
}

impl<'a> Iterator for DockVisibleItemIterator<'a> {
    type Item = (LiveId, WidgetRef);
    fn next(&mut self) -> Option<Self::Item> {
        while let Some((item_id, index)) = self.stack.pop() {
            if let Some(dock_item) = self.dock_items.get(&item_id) {
                match dock_item {
                    DockItem::Splitter { a, b, .. } => {
                        if index == 0 {
                            self.stack.push((item_id, 1));
                            self.stack.push((*a, 0));
                        } else {
                            self.stack.push((*b, 0));
                        }
                    }
                    DockItem::Tabs { tabs, selected, .. } => {
                        if let Some(tab_id) = tabs.get(*selected) {
                            self.stack.push((*tab_id, 0));
                        }
                    }
                    DockItem::Tab { .. } => {
                        if let Some((_, widget)) = self.items.get(&item_id) {
                            return Some((item_id, widget.clone()));
                        }
                    }
                }
            }
        }
        None
    }
}

struct TabBarWrap {
    tab_bar: TabBar,
    contents_draw_list: DrawList2d,
    contents_rect: Rect,
}

#[derive(Copy, Debug, Clone)]
enum DrawStackItem {
    Invalid,
    SplitLeft { id: LiveId },
    SplitRight { id: LiveId },
    SplitEnd { id: LiveId },
    Tabs { id: LiveId },
    TabLabel { id: LiveId, index: usize },
    Tab { id: LiveId },
    TabContent { id: LiveId },
}

impl DrawStackItem {
    fn from_dock_item(id: LiveId, dock_item: Option<&DockItem>) -> Self {
        match dock_item {
            None => DrawStackItem::Invalid,
            Some(DockItem::Splitter { .. }) => DrawStackItem::SplitLeft { id },
            Some(DockItem::Tabs { .. }) => DrawStackItem::Tabs { id },
            Some(DockItem::Tab { .. }) => DrawStackItem::Tab { id },
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum DockAction {
    SplitPanelChanged {
        panel_id: LiveId,
        axis: SplitterAxis,
        align: SplitterAlign,
    },
    TabWasPressed(LiveId),
    TabCloseWasPressed(LiveId),
    ShouldTabStartDrag(LiveId),
    Drag(DragHitEvent),
    Drop(DropHitEvent),
    #[default]
    None,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropPosition {
    part: DropPart,
    rect: Rect,
    id: LiveId,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DropPart {
    Left,
    Right,
    Top,
    Bottom,
    Center,
    TabBar,
    Tab,
    /// The bar between two panels: the newcomer goes BETWEEN them
    /// rather than inside either. `DropPosition::id` names the
    /// splitter, not a tab container.
    Bar,
}

/// DSL-parseable wrapper for DockItem::Splitter
#[derive(Script, ScriptHook, Default)]
pub struct DockItemSplitter {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub axis: SplitterAxis,
    #[live]
    pub align: SplitterAlign,
    #[live]
    pub a: LiveId,
    #[live]
    pub b: LiveId,
}

impl DockItemSplitter {
    pub fn to_dock_item(&self) -> DockItem {
        DockItem::Splitter {
            axis: self.axis,
            align: self.align,
            a: self.a,
            b: self.b,
        }
    }
}

/// DSL-parseable wrapper for DockItem::Tabs
#[derive(Script, ScriptHook, Default)]
pub struct DockItemTabs {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub tabs: Vec<LiveId>,
    #[live]
    pub selected: usize,
    #[live(true)]
    pub closable: bool,
    #[live]
    pub hide_tab_bar: bool,
}

impl DockItemTabs {
    pub fn to_dock_item(&self) -> DockItem {
        DockItem::Tabs {
            tabs: self.tabs.clone(),
            selected: self.selected,
            closable: self.closable,
            hide_tab_bar: self.hide_tab_bar,
        }
    }
}

/// DSL-parseable wrapper for DockItem::Tab
#[derive(Script, ScriptHook, Default)]
pub struct DockItemTab {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub name: String,
    #[live]
    pub template: LiveId,
    #[live]
    pub kind: LiveId,
}

impl DockItemTab {
    pub fn to_dock_item(&self) -> DockItem {
        DockItem::Tab {
            name: self.name.clone(),
            template: self.template,
            kind: self.kind,
        }
    }
}

#[derive(Clone, Debug, SerRon, DeRon)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum DockItem {
    Splitter {
        axis: SplitterAxis,
        align: SplitterAlign,
        a: LiveId,
        b: LiveId,
    },
    Tabs {
        tabs: Vec<LiveId>,
        selected: usize,
        closable: bool,
        #[cfg_attr(feature = "serde", serde(default))]
        hide_tab_bar: bool,
    },
    Tab {
        name: String,
        template: LiveId,
        kind: LiveId,
    },
}

impl Default for DockItem {
    fn default() -> Self {
        DockItem::Tab {
            name: "Tab".to_string(),
            template: id!(PermanentTab),
            kind: LiveId(0),
        }
    }
}

impl DockItem {
    pub fn splitter(axis: SplitterAxis, align: SplitterAlign, a: LiveId, b: LiveId) -> Self {
        DockItem::Splitter { axis, align, a, b }
    }

    pub fn tabs(tabs: Vec<LiveId>, selected: usize, closable: bool) -> Self {
        DockItem::Tabs {
            tabs,
            selected,
            closable,
            hide_tab_bar: false,
        }
    }

    pub fn tab(name: String, kind: LiveId, template: LiveId) -> Self {
        DockItem::Tab {
            name,
            template,
            kind,
        }
    }
}

fn preserve_item_for_layout(
    dock_items: &HashMap<LiveId, DockItem>,
    id: LiveId,
    old_kind: LiveId,
) -> bool {
    match dock_items.get(&id) {
        Some(DockItem::Tab { kind, .. }) => *kind == old_kind,
        // Not in this layout: retain it as a dormant tab body.
        None => true,
        // A container took this ID, so it cannot also name a tab.
        Some(_) => false,
    }
}

#[derive(Clone, Debug, Default)]
pub struct DockCompactDump {
    pub tabs: Vec<DockCompactTabsInfo>,
    pub tab_headers: Vec<DockCompactTabInfo>,
}

#[derive(Clone, Debug)]
pub struct DockCompactTabsInfo {
    pub tabs_id: LiveId,
    pub selected_tab_id: Option<LiveId>,
    pub tab_count: usize,
    pub rect: Rect,
}

#[derive(Clone, Debug)]
pub struct DockCompactTabInfo {
    pub tabs_id: LiveId,
    pub tab_id: LiveId,
    pub is_active: bool,
    pub title: String,
    pub rect: Rect,
}

/// The share of a panel, along each of its edges, that splits it rather
/// than joining it. A tenth on all four sides.
const DROP_EDGE: f64 = 0.1;

/// How close to the outside of the whole dock a drop has to be, in
/// layout points, to lay a panel across everything rather than beside
/// one member of it. A fixed distance, not a share: it is the same
/// gesture whether the dock is a strip or a wall.
const OUTER_EDGE: f64 = 24.0;

/// Where the bar goes when a newcomer joins `stacked` panels that are
/// already sharing this axis, so that all of them end up the same size.
/// `first` is whether the newcomer takes the near side.
///
/// Two panels sharing a square top and bottom, and a third dropped
/// under them: the newcomer takes a third and the pair keep two
/// thirds, which they were already halving — so all three are thirds.
/// Nothing here is final; every bar can still be dragged.
fn equal_share(stacked: usize, first: bool) -> SplitterAlign {
    let share = 1.0 / (stacked.max(1) as f64 + 1.0);
    SplitterAlign::Weighted(if first { share } else { 1.0 - share })
}

/// What is left of a panel under its own tab bar.
///
/// A panel reports the rect of its own turtle, and that turtle was begun
/// above the strip, so the rect includes it. The strip is a drop target
/// of its own and is claimed first — which meant the top tenth measured
/// from the whole rect fell entirely inside the strip, the top edge could
/// never be reached, and a panel could only ever be split downwards.
fn panel_body(whole: Rect, bar: Rect) -> Rect {
    let below = (bar.pos.y + bar.size.y) - whole.pos.y;
    if bar.size.y <= 0.0 || below <= 0.0 || below >= whole.size.y {
        return whole;
    }
    Rect {
        pos: Vec2d { x: whole.pos.x, y: whole.pos.y + below },
        size: Vec2d { x: whole.size.x, y: whole.size.y - below },
    }
}

/// How a splitter divides itself once a panel is put between its two
/// children: where the splitter's own bar goes, where the new bar
/// between the newcomer and its neighbour goes, and which side of the
/// seam the newcomer is nested on.
struct BetweenSplit {
    outer: SplitterAlign,
    inner: SplitterAlign,
    /// True when the new pair is (near child, newcomer) rather than
    /// (newcomer, far child).
    near_side: bool,
}

/// Work out that division. `p` and `q` are how many panels the near
/// and far children already hold along this axis; the newcomer is one
/// more, so there are p + q + 1 of them to satisfy.
///
/// A bar the person pinned a fixed distance from one side is saying
/// that pane's size is not up for negotiation. The newcomer is then
/// nested on the other side and the pinned pane is left exactly where
/// it is: equalising it would quietly turn a fixed sidebar into a
/// proportional one, which is not what anybody asked for by dropping a
/// panel somewhere else.
fn between_split(align: SplitterAlign, p: usize, q: usize) -> BetweenSplit {
    let (p, q) = (p.max(1), q.max(1));
    match align {
        SplitterAlign::FromA(px) => BetweenSplit {
            outer: SplitterAlign::FromA(px),
            inner: equal_share(q, true),
            near_side: false,
        },
        SplitterAlign::FromB(px) => BetweenSplit {
            outer: SplitterAlign::FromB(px),
            inner: equal_share(p, false),
            near_side: true,
        },
        // Both sides are shares, so every panel along this axis can
        // have the same one. The near child keeps p of the p + q + 1;
        // the far child keeps q + 1 and splits them so the newcomer
        // gets exactly one. Two panels and a newcomer: a third each.
        SplitterAlign::Weighted(_) => {
            let total = (p + q + 1) as f64;
            BetweenSplit {
                outer: SplitterAlign::Weighted(p as f64 / total),
                inner: equal_share(q, true),
                near_side: false,
            }
        }
    }
}

/// Where along the splitter the newcomer will end up, as an offset and
/// a length. This is what the preview covers, so that what is shown
/// during the drag is the slot the drop actually makes rather than the
/// bar that was aimed at, which is somewhere else once the shares move.
///
/// The bars themselves take a few points out of the panes either side,
/// which this ignores: it would make the preview exact and the
/// arithmetic untestable, and every bar can be dragged afterwards.
fn newcomer_slot(length: f64, align: SplitterAlign, p: usize, q: usize) -> (f64, f64) {
    let (p, q) = (p.max(1) as f64, q.max(1) as f64);
    match align {
        SplitterAlign::Weighted(_) => {
            let total = p + q + 1.0;
            (length * p / total, length / total)
        }
        SplitterAlign::FromA(px) => {
            let pinned = px.clamp(0.0, length);
            (pinned, (length - pinned) / (q + 1.0))
        }
        SplitterAlign::FromB(px) => {
            let room = length - px.clamp(0.0, length);
            let len = room / (p + 1.0);
            (room - len, len)
        }
    }
}

/// The half of `rect` an edge drop would take.
fn edge_half(rect: Rect, part: DropPart) -> Rect {
    let half_x = Vec2d { x: rect.size.x / 2.0, y: rect.size.y };
    let half_y = Vec2d { x: rect.size.x, y: rect.size.y / 2.0 };
    match part {
        DropPart::Left => Rect { pos: rect.pos, size: half_x },
        DropPart::Right => Rect {
            pos: Vec2d { x: rect.pos.x + half_x.x, y: rect.pos.y },
            size: half_x,
        },
        DropPart::Top => Rect { pos: rect.pos, size: half_y },
        DropPart::Bottom => Rect {
            pos: Vec2d { x: rect.pos.x, y: rect.pos.y + half_y.y },
            size: half_y,
        },
        _ => rect,
    }
}

/// Which edge of `rect` a point is nearest, if it is within `edge` of one
/// of them. `edge` is a distance in points along each axis, so the caller
/// decides whether the band is a share of the panel or a fixed margin.
fn nearest_edge(rect: Rect, at: Vec2d, edge_x: f64, edge_y: f64) -> Option<DropPart> {
    if !rect.contains(at) || rect.size.x <= 0.0 || rect.size.y <= 0.0 {
        return None;
    }
    // Left and right before top and bottom, so a corner splits sideways.
    // Whichever way round it went, one of the two would have to lose.
    if at.x - rect.pos.x < edge_x {
        return Some(DropPart::Left);
    }
    if rect.pos.x + rect.size.x - at.x < edge_x {
        return Some(DropPart::Right);
    }
    if at.y - rect.pos.y < edge_y {
        return Some(DropPart::Top);
    }
    if rect.pos.y + rect.size.y - at.y < edge_y {
        return Some(DropPart::Bottom);
    }
    None
}

/// Where a drop lands in one panel's body, and the region a preview of it
/// should cover. The four edges each split it; the middle joins it.
fn drop_band(body: Rect, at: Vec2d) -> Option<(DropPart, Rect)> {
    if !body.contains(at) || body.size.x <= 0.0 || body.size.y <= 0.0 {
        return None;
    }
    match nearest_edge(body, at, body.size.x * DROP_EDGE, body.size.y * DROP_EDGE) {
        Some(part) => Some((part, edge_half(body, part))),
        None => Some((DropPart::Center, body)),
    }
}

/// Where a drop lands against the outside of the whole dock. Only the
/// four edges: the middle of the dock belongs to whichever panel is
/// under it.
///
/// This is what lays a panel ACROSS a stack rather than inside one of
/// its members. With two panels sharing a square top and bottom, a drop
/// down the far left gives the newcomer the whole left half and leaves
/// the other two stacked in the right half.
fn outer_band(whole: Rect, at: Vec2d) -> Option<(DropPart, Rect)> {
    // A dock too small to have an inside is all edge, and every drop in
    // it would split the root. Leave those to the panel under the pointer.
    if whole.size.x < OUTER_EDGE * 3.0 || whole.size.y < OUTER_EDGE * 3.0 {
        return None;
    }
    let part = nearest_edge(whole, at, OUTER_EDGE, OUTER_EDGE)?;
    Some((part, edge_half(whole, part)))
}
impl Dock {
    pub fn unique_id(&self, base: u64) -> LiveId {
        let mut id = LiveId(base);
        let mut i = 0u32;
        while self.dock_items.get(&id).is_some() {
            id = id.bytes_append(&i.to_be_bytes());
            i += 1;
        }
        id
    }

    /// Hands out a fresh LiveId for a drag/drop-created Tabs or Splitter.
    /// First call after load_state scans the loaded state to seed past the max
    /// existing ID, so subsequent allocations can't collide.
    fn next_internal_id(&mut self) -> LiveId {
        if self.next_internal_id < INTERNAL_ID_FLOOR {
            let max = self.dock_items.keys()
                .map(|k| k.0)
                .filter(|v| *v >= INTERNAL_ID_FLOOR)
                .max();
            self.next_internal_id = max.map(|m| m + 1).unwrap_or(INTERNAL_ID_FLOOR);
        }
        let id = LiveId(self.next_internal_id);
        self.next_internal_id += 1;
        id
    }

    pub fn compact_dump(&self, cx: &Cx) -> DockCompactDump {
        let mut tabs = Vec::new();
        let mut tab_headers = Vec::new();
        let mut tabs_ids = Vec::new();
        tabs_ids.extend(self.tab_bars.keys().copied());
        tabs_ids.sort_by_key(|id| id.0);

        for tabs_id in tabs_ids {
            let Some(DockItem::Tabs {
                tabs: tab_ids,
                selected,
                ..
            }) = self.dock_items.get(&tabs_id)
            else {
                continue;
            };
            let Some(tab_bar) = self.tab_bars.get(&tabs_id) else {
                continue;
            };

            let bar_rect = tab_bar.tab_bar.bar_rect(cx);
            tabs.push(DockCompactTabsInfo {
                tabs_id,
                selected_tab_id: tab_ids.get(*selected).copied(),
                tab_count: tab_ids.len(),
                rect: bar_rect,
            });

            for (index, tab_id) in tab_ids.iter().enumerate() {
                let Some(tab_rect) = tab_bar.tab_bar.tab_rect(cx, *tab_id) else {
                    continue;
                };
                // A tab bar with more tabs than room scrolls them sideways,
                // and a tab past its edge is drawn clipped (or not at all).
                // What is reported is the part a person could click: a tab
                // half out of view has a rect whose centre is still ON it,
                // not on whatever the neighbouring panel shows there, and a
                // tab wholly out of view is not reported as present. Only the
                // scroll axis is clipped: a tab's own height is its own, and
                // the bar's box does not say where the label sits in it.
                let tab_rect = tab_rect.clip((
                    dvec2(bar_rect.pos.x, tab_rect.pos.y),
                    dvec2(bar_rect.pos.x + bar_rect.size.x, tab_rect.pos.y + tab_rect.size.y),
                ));
                if tab_rect.size.x < 1.0 {
                    continue;
                }
                let title = match self.dock_items.get(tab_id) {
                    Some(DockItem::Tab { name, .. }) => name.clone(),
                    _ => String::new(),
                };
                tab_headers.push(DockCompactTabInfo {
                    tabs_id,
                    tab_id: *tab_id,
                    is_active: index == *selected,
                    title,
                    rect: tab_rect,
                });
            }
        }

        DockCompactDump { tabs, tab_headers }
    }

    fn create_all_items(&mut self, cx: &mut Cx) {
        let mut items = Vec::new();
        for (item_id, item) in self.dock_items.iter() {
            if let DockItem::Tab { kind, .. } = item {
                items.push((*item_id, *kind));
            }
        }
        for (item_id, kind) in items {
            self.item_or_create(cx, item_id, kind);
        }
    }

    fn create_all_items_with_vm(&mut self, vm: &mut ScriptVm) {
        let mut items = Vec::new();
        for (item_id, item) in self.dock_items.iter() {
            if let DockItem::Tab { kind, .. } = item {
                items.push((*item_id, *kind));
            }
        }
        for (item_id, kind) in items {
            self.item_or_create_with_vm(vm, item_id, kind);
        }
    }

    fn item_or_create_with_vm(
        &mut self,
        vm: &mut ScriptVm,
        entry_id: LiveId,
        template: LiveId,
    ) -> Option<WidgetRef> {
        // Check if item already exists
        if let Some(entry) = self.items.get(&entry_id) {
            return Some(entry.1.clone());
        }

        // Get template and create new item
        if let Some(template_ref) = self.templates.get(&template) {
            let template_value: ScriptValue = template_ref.as_object().into();
            let widget = WidgetRef::script_from_value(vm, template_value);
            let cx = vm.cx_mut();
            self.items
                .get_or_insert(cx, entry_id, |_cx| (template, widget.clone()));
            cx.widget_tree_insert_child_deep(self.uid, entry_id, widget.clone());
            Some(widget)
        } else {
            warning!("Template not found: {template}. Did you add it to the <Dock> instance?");
            None
        }
    }

    fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        cx.begin_turtle(walk, self.layout);
    }

    fn end(&mut self, cx: &mut Cx2d) {
        if self
            .drop_target_draw_list
            .begin(cx, Walk::default())
            .is_redrawing()
        {
            if let Some(pos) = &self.drop_state {
                self.drag_target_preview.draw_abs(cx, pos.rect);
            }
            self.drop_target_draw_list.end(cx);
        }

        // Draw the ghost tab overlay BEFORE retaining/ending, so it's
        // registered as the last overlay and renders on top of everything.
        if self.dragging_tab.is_some() {
            self.ghost_tab_draw_list.begin_overlay_last(cx);
            let size = cx.current_pass_size();
            cx.begin_root_turtle(size, Layout::default());
            let dt = self.dragging_tab.as_mut().unwrap();
            let ghost_pos = dvec2(dt.cursor.x + 8.0, dt.cursor.y - 30.0);
            dt.ghost.walk = Walk {
                abs_pos: Some(ghost_pos),
                width: Size::Fixed(dt.size.x),
                height: Size::Fixed(dt.size.y),
                ..Walk::default()
            };
            // Ensure the ghost tab renders above the drop target preview (draw_depth 10.0).
            dt.ghost.set_draw_depth(20.0);
            dt.ghost.draw(cx, &dt.name);
            cx.end_pass_sized_turtle();
            self.ghost_tab_draw_list.end(cx);
        } else {
            // Clear the ghost tab overlay when not dragging.
            self.ghost_tab_draw_list.begin_always(cx);
            self.ghost_tab_draw_list.end(cx);
        }

        self.tab_bars.retain_visible();
        self.splitters.retain_visible();

        for splitter in self.splitters.values() {
            self.round_corner
                .draw_corners(cx, splitter.area_a().rect(cx));
            self.round_corner
                .draw_corners(cx, splitter.area_b().rect(cx));
        }
        self.round_corner.draw_corners(cx, cx.turtle().rect());

        cx.end_turtle_with_area(&mut self.area);
    }

    /// Where a drop at `abs` would land: which part of which container,
    /// and the region to preview.
    ///
    /// Read in five passes. The answers overlap, and the order between
    /// them IS the behaviour:
    ///
    /// 1. A tab. Dropping on one puts the newcomer beside it, and that
    ///    has to be possible wherever the tab happens to be.
    /// 2. The outside of the whole dock, which lays the newcomer across
    ///    everything in it. This has to beat the tab bars, or the top
    ///    edge is unreachable: the topmost panels' bars run along it, so
    ///    a tab-bar pass in front of this one meant a drop could take
    ///    the bottom half of the dock and never the top. Nothing is
    ///    lost by it — dropping in the middle of a panel joins that
    ///    panel just as its bar does.
    /// 3. The bar between two panels, which puts the newcomer between
    ///    them.
    /// 4. The empty part of a tab bar, which joins that panel.
    /// 5. The edges and middle of whichever panel is under the pointer,
    ///    which splits or joins that one alone.
    fn find_drop_position(&self, cx: &Cx, abs: Vec2d) -> Option<DropPosition> {
        for (tab_bar_id, tab_bar) in self.tab_bars.iter() {
            if self.hides_its_tab_bar(*tab_bar_id) {
                continue;
            }
            if let Some((tab_id, rect)) = tab_bar.tab_bar.is_over_tab(cx, abs) {
                return Some(DropPosition { part: DropPart::Tab, id: tab_id, rect });
            }
        }
        if let Some((part, rect)) = outer_band(self.area.rect(cx), abs) {
            return Some(DropPosition { part, id: id!(root), rect });
        }
        if let Some(split_id) = self.bar_under(cx, abs) {
            if let Some(rect) = self.bar_slot(cx, split_id) {
                return Some(DropPosition { part: DropPart::Bar, id: split_id, rect });
            }
        }
        for (tab_bar_id, tab_bar) in self.tab_bars.iter() {
            if self.hides_its_tab_bar(*tab_bar_id) {
                continue;
            }
            if let Some(rect) = tab_bar.tab_bar.is_over_tab_bar(cx, abs) {
                return Some(DropPosition { part: DropPart::TabBar, id: *tab_bar_id, rect });
            }
        }
        for (tab_bar_id, tab_bar) in self.tab_bars.iter() {
            if self.hides_its_tab_bar(*tab_bar_id) {
                continue;
            }
            let body = panel_body(tab_bar.contents_rect, tab_bar.tab_bar.bar_rect(cx));
            if let Some((part, rect)) = drop_band(body, abs) {
                return Some(DropPosition { part, id: *tab_bar_id, rect });
            }
        }
        None
    }

    /// The splitter whose bar is under this point.
    ///
    /// Where two bars meet at a T their slop laps over the corner, so
    /// the nearer bar wins and the lower id settles the rest: a hash
    /// map is no order to settle anything by, and a drop that lands
    /// somewhere different each time is worse than one that is wrong.
    fn bar_under(&self, cx: &Cx, abs: Vec2d) -> Option<LiveId> {
        let mut best: Option<(f64, LiveId)> = None;
        for (split_id, splitter) in self.splitters.iter() {
            if !matches!(self.dock_items.get(split_id), Some(DockItem::Splitter { .. })) {
                continue;
            }
            let band = splitter.bar_grab_rect(cx);
            if band.size.x <= 0.0 || band.size.y <= 0.0 || !band.contains(abs) {
                continue;
            }
            let across = match splitter.axis() {
                SplitterAxis::Horizontal => (abs.x - band.center().x).abs(),
                SplitterAxis::Vertical => (abs.y - band.center().y).abs(),
            };
            let better = match best {
                None => true,
                Some((near, id)) => across < near || (across == near && split_id.0 < id.0),
            };
            if better {
                best = Some((across, *split_id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// The slot a panel dropped on this splitter's bar would take, for
    /// the preview to cover. The ground the two panes stand on, cut
    /// where the newcomer's share falls.
    fn bar_slot(&self, cx: &Cx, split_id: LiveId) -> Option<Rect> {
        let splitter = self.splitters.get(&split_id)?;
        let DockItem::Splitter { axis, align, a, b } = self.dock_items.get(&split_id)? else {
            return None;
        };
        // The unclipped rects, because every other pass here measures
        // in layout space and a preview in a different frame would be
        // wrong by the scroll with nothing on screen to say so.
        let ground = splitter.area_a().rect(cx).hull(splitter.area_b().rect(cx));
        if ground.size.x <= 0.0 || ground.size.y <= 0.0 {
            return None;
        }
        let p = Self::axis_leaves(&self.dock_items, *a, *axis);
        let q = Self::axis_leaves(&self.dock_items, *b, *axis);
        Some(match axis {
            SplitterAxis::Horizontal => {
                let (offset, len) = newcomer_slot(ground.size.x, *align, p, q);
                Rect {
                    pos: Vec2d { x: ground.pos.x + offset, y: ground.pos.y },
                    size: Vec2d { x: len, y: ground.size.y },
                }
            }
            SplitterAxis::Vertical => {
                let (offset, len) = newcomer_slot(ground.size.y, *align, p, q);
                Rect {
                    pos: Vec2d { x: ground.pos.x, y: ground.pos.y + offset },
                    size: Vec2d { x: ground.size.x, y: len },
                }
            }
        })
    }

    /// A panel with no tab bar showing is no drop target: there is nothing
    /// to aim at and nothing to report having hit.
    fn hides_its_tab_bar(&self, tabs_id: LiveId) -> bool {
        matches!(
            self.dock_items.get(&tabs_id),
            Some(DockItem::Tabs { hide_tab_bar: true, .. })
        )
    }
    pub fn item(&self, entry_id: LiveId) -> Option<WidgetRef> {
        // `load_state_preserving_items` may keep a tab body resident while it
        // is absent from the current layout. Resident is not the same as
        // visible: callers asking for a current Dock item must not see that
        // cache entry until its Tab node is loaded again.
        if !matches!(self.dock_items.get(&entry_id), Some(DockItem::Tab { .. })) {
            return None;
        }
        if let Some(entry) = self.items.get(&entry_id) {
            return Some(entry.1.clone());
        }
        None
    }

    fn drop_target_tab_id(&self, cx: &Cx, abs: Vec2d) -> Option<LiveId> {
        let pos = self.find_drop_position(cx, abs)?;
        match pos.part {
            DropPart::Tab => Some(pos.id),
            DropPart::TabBar
            | DropPart::Left
            | DropPart::Right
            | DropPart::Top
            | DropPart::Bottom
            | DropPart::Center => {
                let DockItem::Tabs { tabs, selected, .. } = self.dock_items.get(&pos.id)? else {
                    return None;
                };
                tabs.get(*selected).copied()
            }
            // A seam names a splitter, and a splitter has no tab under
            // the pointer to answer with.
            DropPart::Bar => None,
        }
    }

    pub fn item_or_create(
        &mut self,
        cx: &mut Cx,
        entry_id: LiveId,
        template: LiveId,
    ) -> Option<WidgetRef> {
        if let Some(template_ref) = self.templates.get(&template) {
            let template_value: ScriptValue = template_ref.as_object().into();
            let existed = self.items.contains_key(&entry_id);
            let entry = self.items.get_or_insert(cx, entry_id, |cx| {
                cx.with_vm(|vm| (template, WidgetRef::script_from_value(vm, template_value)))
            });
            if !existed {
                cx.widget_tree_insert_child_deep(self.uid, entry_id, entry.1.clone());
            }
            Some(entry.1.clone())
        } else {
            warning!("Template not found: {template}. Did you add it to the <Dock> instance?");
            None
        }
    }

    pub fn items(&mut self) -> &ComponentMap<LiveId, (LiveId, WidgetRef)> {
        &self.items
    }

    pub fn visible_items(&mut self) -> DockVisibleItemIterator<'_> {
        self.dock_item_iter_stack.clear();
        self.dock_item_iter_stack.push((id!(root), 0));
        DockVisibleItemIterator {
            stack: &mut self.dock_item_iter_stack,
            dock_items: &self.dock_items,
            items: &self.items,
        }
    }

    fn set_parent_split_in_items(
        dock_items: &mut HashMap<LiveId, DockItem>,
        what_item: LiveId,
        replace_item: LiveId,
    ) -> bool {
        for item in dock_items.values_mut() {
            match item {
                DockItem::Splitter { a, b, .. } => {
                    if what_item == *a {
                        *a = replace_item;
                        return true;
                    } else if what_item == *b {
                        *b = replace_item;
                        return true;
                    }
                }
                _ => (),
            }
        }
        false
    }

    fn remove_tabs_container_from_tree(
        dock_items: &mut HashMap<LiveId, DockItem>,
        tabs_id: LiveId,
    ) -> Option<LiveId> {
        let mut found: Option<(LiveId, LiveId)> = None;
        for (splitter_id, item) in dock_items.iter() {
            if let DockItem::Splitter { a, b, .. } = item {
                if tabs_id == *a {
                    found = Some((*splitter_id, *b));
                    break;
                } else if tabs_id == *b {
                    found = Some((*splitter_id, *a));
                    break;
                }
            }
        }
        let (splitter_id, sibling_id) = found?;
        if !dock_items.contains_key(&sibling_id) {
            return None;
        }

        if splitter_id == id!(root) {
            // Can't just remove root, the walking code starts there. Copy the
            // sibling's content into the root slot instead, and drop the sibling.
            if let Some(sibling_item) = dock_items.remove(&sibling_id) {
                dock_items.insert(splitter_id, sibling_item);
                dock_items.remove(&tabs_id);
                return Some(splitter_id);
            }
            return None;
        }

        if !Self::set_parent_split_in_items(dock_items, splitter_id, sibling_id) {
            return None;
        }
        dock_items.remove(&splitter_id);
        dock_items.remove(&tabs_id);
        Some(sibling_id)
    }

    /// How many panels a container already stacks along this axis. A
    /// splitter of the other axis counts as one, because along this one
    /// it is a single band.
    fn axis_leaves(
        dock_items: &HashMap<LiveId, DockItem>,
        id: LiveId,
        axis: SplitterAxis,
        ) -> usize {
        Self::axis_leaves_within(dock_items, id, axis, 0)
    }

    fn axis_leaves_within(
        dock_items: &HashMap<LiveId, DockItem>,
        id: LiveId,
        axis: SplitterAxis,
        depth: usize,
    ) -> usize {
        // The tree this walks is built by this file and cannot loop, but
        // it is also read back from saved layouts, so the walk is bounded
        // rather than trusting.
        if depth > 32 {
            return 1;
        }
        match dock_items.get(&id) {
            Some(DockItem::Splitter { axis: on, a, b, .. }) if *on == axis => {
                Self::axis_leaves_within(dock_items, *a, axis, depth + 1)
                    + Self::axis_leaves_within(dock_items, *b, axis, depth + 1)
            }
            _ => 1,
        }
    }

    /// Put a panel between the two children of a splitter, so it lies
    /// with them rather than inside either. The splitter keeps its own
    /// id and its near child keeps its slot; the far side is pushed
    /// down into a new splitter that holds the newcomer beside it.
    fn insert_between_split_children_in_items(
        dock_items: &mut HashMap<LiveId, DockItem>,
        split_id: LiveId,
        new_tabs_id: LiveId,
        new_split_id: LiveId,
    ) -> bool {
        let Some(DockItem::Splitter { axis, align, a, b }) = dock_items.get(&split_id) else {
            return false;
        };
        let (axis, align, a, b) = (*axis, *align, *a, *b);
        if !matches!(dock_items.get(&new_tabs_id), Some(DockItem::Tabs { .. })) {
            return false;
        }
        if new_tabs_id == split_id || new_tabs_id == a || new_tabs_id == b {
            return false;
        }
        if new_split_id == split_id || dock_items.contains_key(&new_split_id) {
            return false;
        }
        let p = Self::axis_leaves(dock_items, a, axis);
        let q = Self::axis_leaves(dock_items, b, axis);
        let plan = between_split(align, p, q);
        let (inner_a, inner_b) = if plan.near_side {
            (a, new_tabs_id)
        } else {
            (new_tabs_id, b)
        };
        let (outer_a, outer_b) = if plan.near_side {
            (new_split_id, b)
        } else {
            (a, new_split_id)
        };
        dock_items.insert(
            new_split_id,
            DockItem::Splitter { axis, align: plan.inner, a: inner_a, b: inner_b },
        );
        dock_items.insert(
            split_id,
            DockItem::Splitter { axis, align: plan.outer, a: outer_a, b: outer_b },
        );
        true
    }

    fn split_tabs_container_in_items(
        dock_items: &mut HashMap<LiveId, DockItem>,
        target_tabs_id: LiveId,
        new_tabs_id: LiveId,
        new_split_id: LiveId,
        old_root_id: Option<LiveId>,
        part: DropPart,
    ) -> bool {
        if !matches!(
            part,
            DropPart::Left | DropPart::Right | DropPart::Top | DropPart::Bottom
        ) {
            return false;
        }
        // A splitter is a container too. Dropping against the outside
        // of the dock aims at the root, which is a splitter as soon as
        // there is more than one panel, and refusing it there is what
        // stopped a panel being laid across a stack.
        if !matches!(
            dock_items.get(&target_tabs_id),
            Some(DockItem::Tabs { .. }) | Some(DockItem::Splitter { .. })
        ) {
            return false;
        }
        if new_tabs_id == target_tabs_id
            || !matches!(dock_items.get(&new_tabs_id), Some(DockItem::Tabs { .. }))
        {
            return false;
        }

        let root = id!(root);
        if target_tabs_id == root {
            let Some(old_root_id) = old_root_id else {
                return false;
            };
            if dock_items.contains_key(&old_root_id) {
                return false;
            }
        } else if dock_items.contains_key(&new_split_id) {
            return false;
        }

        let target_child_id = if target_tabs_id == root {
            let Some(old_root_id) = old_root_id else {
                return false;
            };
            let Some(old_root_item) = dock_items.remove(&root) else {
                return false;
            };
            dock_items.insert(old_root_id, old_root_item);
            old_root_id
        } else {
            if !Self::set_parent_split_in_items(dock_items, target_tabs_id, new_split_id) {
                return false;
            }
            target_tabs_id
        };

        let split_id = if target_tabs_id == root { root } else { new_split_id };
        let axis = match part {
            DropPart::Left | DropPart::Right => SplitterAxis::Horizontal,
            _ => SplitterAxis::Vertical,
        };
        let first = matches!(part, DropPart::Left | DropPart::Top);
        let align = equal_share(
            Self::axis_leaves(dock_items, target_child_id, axis),
            first,
        );
        let (a, b) = if first {
            (new_tabs_id, target_child_id)
        } else {
            (target_child_id, new_tabs_id)
        };
        let split = DockItem::Splitter { axis, align, a, b };
        dock_items.insert(split_id, split);
        true
    }

    fn split_tabs_container(
        &mut self,
        cx: &mut Cx,
        target_tabs_id: LiveId,
        new_tabs_id: LiveId,
        part: DropPart,
    ) -> bool {
        let root = id!(root);
        let old_root_id = if target_tabs_id == root {
            Some(self.next_internal_id())
        } else {
            None
        };
        let new_split_id = if target_tabs_id == root {
            root
        } else {
            self.next_internal_id()
        };
        let did_split = Self::split_tabs_container_in_items(
            &mut self.dock_items,
            target_tabs_id,
            new_tabs_id,
            new_split_id,
            old_root_id,
            part,
        );
        if did_split {
            self.redraw_item(cx, if target_tabs_id == root { root } else { new_split_id });
            self.area.redraw(cx);
        }
        did_split
    }

    fn remap_drop_tabs_after_move_in_items(
        dock_items: &HashMap<LiveId, DockItem>,
        tabs_id: LiveId,
    ) -> Option<LiveId> {
        // Splitters count as well as tab containers: an outer drop aims
        // at the root, and the root is a splitter as soon as the dock
        // holds more than one panel.
        if dock_items.contains_key(&tabs_id) {
            Some(tabs_id)
        } else if tabs_id != id!(root) && dock_items.contains_key(&id!(root)) {
            Some(id!(root))
        } else {
            None
        }
    }

    fn remap_drop_tabs_after_move(&self, tabs_id: LiveId) -> Option<LiveId> {
        Self::remap_drop_tabs_after_move_in_items(&self.dock_items, tabs_id)
    }

    fn push_tab_into_tabs(&mut self, cx: &mut Cx, tabs_id: LiveId, tab_id: LiveId) -> bool {
        if let Some(DockItem::Tabs { tabs, selected, .. }) = self.dock_items.get_mut(&tabs_id) {
            if let Some(pos) = tabs.iter().position(|id| *id == tab_id) {
                *selected = pos;
            } else {
                tabs.push(tab_id);
                *selected = tabs.len() - 1;
            }
            if let Some(tab_bar) = self.tab_bars.get(&tabs_id) {
                tab_bar.contents_draw_list.redraw(cx);
            }
            true
        } else {
            false
        }
    }

    fn insert_tab_before_tab_in_items(
        dock_items: &mut HashMap<LiveId, DockItem>,
        target_tab_id: LiveId,
        tab_id: LiveId,
    ) -> Option<(LiveId, usize)> {
        for (tabs_id, item) in dock_items.iter_mut() {
            if let DockItem::Tabs { tabs, selected, .. } = item {
                if let Some(pos) = tabs.iter().position(|id| *id == target_tab_id) {
                    tabs.insert(pos, tab_id);
                    *selected = pos;
                    return Some((*tabs_id, pos));
                }
            }
        }
        None
    }

    fn insert_tab_before_tab(
        &mut self,
        cx: &mut Cx,
        target_tab_id: LiveId,
        tab_id: LiveId,
    ) -> bool {
        let Some((tab_bar_id, _pos)) =
            Self::insert_tab_before_tab_in_items(&mut self.dock_items, target_tab_id, tab_id)
        else {
            return false;
        };
        if let Some(tab_bar) = self.tab_bars.get(&tab_bar_id) {
            tab_bar.contents_draw_list.redraw(cx);
        }
        true
    }

    fn redraw_item(&mut self, cx: &mut Cx, what_item_id: LiveId) {
        if let Some(tab_bar) = self.tab_bars.get_mut(&what_item_id) {
            tab_bar.contents_draw_list.redraw(cx);
        }
        if let Some((_kind, item)) = self.items.get_mut(&what_item_id) {
            item.redraw(cx);
        }
    }

    fn splitter_position(&self, splitter_id: LiveId) -> Option<f64> {
        self.splitters
            .get(&splitter_id)
            .map(|splitter| splitter.position())
    }

    fn set_splitter_align(
        &mut self,
        cx: &mut Cx,
        splitter_id: LiveId,
        align: SplitterAlign,
        mark_dirty: bool,
    ) -> bool {
        let Some(DockItem::Splitter {
            a,
            b,
            align: current_align,
            ..
        }) = self.dock_items.get_mut(&splitter_id)
        else {
            return false;
        };

        let a = *a;
        let b = *b;
        *current_align = align;
        if let Some(splitter) = self.splitters.get_mut(&splitter_id) {
            splitter.set_align(align);
        }
        self.redraw_item(cx, a);
        self.redraw_item(cx, b);
        self.area.redraw(cx);
        if mark_dirty {
            self.needs_save = true;
        }
        true
    }

    fn unsplit_tabs(&mut self, cx: &mut Cx, tabs_id: LiveId) {
        self.needs_save = true;
        if let Some(replacement_id) =
            Self::remove_tabs_container_from_tree(&mut self.dock_items, tabs_id)
        {
            self.redraw_item(cx, replacement_id);
            self.area.redraw(cx);
        }
    }

    fn select_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
        for (tabs_id, item) in self.dock_items.iter_mut() {
            match item {
                DockItem::Tabs { tabs, selected, .. } => {
                    if let Some(pos) = tabs.iter().position(|v| *v == tab_id) {
                        if *selected == pos {
                            return;
                        }
                        self.needs_save = true;
                        *selected = pos;
                        if let Some(tab_bar) = self.tab_bars.get(&tabs_id) {
                            tab_bar.contents_draw_list.redraw(cx);
                        }
                        return;
                    }
                }
                _ => (),
            }
        }
    }

    fn set_tab_title(&mut self, cx: &mut Cx, tab_id: LiveId, new_name: String) {
        if let Some(DockItem::Tab { name, .. }) = self.dock_items.get_mut(&tab_id) {
            if *name == new_name {
                return;
            }
            self.needs_save = true;
            *name = new_name;
            self.redraw_tab(cx, tab_id);
        }
    }

    fn redraw_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
        for (tabs_id, item) in self.dock_items.iter_mut() {
            match item {
                DockItem::Tabs { tabs, .. } => {
                    if tabs.iter().any(|v| *v == tab_id) {
                        if let Some(tab_bar) = self.tab_bars.get(&tabs_id) {
                            tab_bar.contents_draw_list.redraw(cx);
                        }
                    }
                }
                _ => (),
            }
        }
    }

    fn find_tab_bar_of_tab(&self, tab_id: LiveId) -> Option<(LiveId, usize)> {
        for (tabs_id, item) in self.dock_items.iter() {
            match item {
                DockItem::Tabs { tabs, .. } => {
                    if let Some(pos) = tabs.iter().position(|v| *v == tab_id) {
                        return Some((*tabs_id, pos));
                    }
                }
                _ => (),
            }
        }
        None
    }

    fn close_tab(&mut self, cx: &mut Cx, tab_id: LiveId, keep_item: bool) -> Option<LiveId> {
        self.needs_save = true;
        for (tabs_id, item) in self.dock_items.iter_mut() {
            match item {
                DockItem::Tabs {
                    tabs,
                    selected,
                    closable,
                    ..
                } => {
                    if let Some(pos) = tabs.iter().position(|v| *v == tab_id) {
                        let tabs_id = *tabs_id;
                        tabs.remove(pos);
                        if tabs.is_empty() {
                            if *closable {
                                self.unsplit_tabs(cx, tabs_id);
                            }
                            if !keep_item {
                                self.dock_items.remove(&tab_id);
                                self.items.remove(&tab_id);
                            }
                            self.area.redraw(cx);
                            return None;
                        } else {
                            let next_tab = if *selected >= tabs.len() {
                                tabs[*selected - 1]
                            } else {
                                tabs[*selected]
                            };
                            self.select_tab(cx, next_tab);
                            // When the closed tab was the active one, the next tab usually
                            // lands at the same selected index, so select_tab's
                            // index-unchanged shortcut skips the redraw. The displayed
                            // contents still changed to a different item, so redraw them
                            // explicitly or the closed tab's pixels stay on screen.
                            if let Some(tab_bar) = self.tab_bars.get(&tabs_id) {
                                tab_bar.contents_draw_list.redraw(cx);
                            }
                            if !keep_item {
                                self.dock_items.remove(&tab_id);
                                self.items.remove(&tab_id);
                            }
                            self.area.redraw(cx);
                            return Some(tabs_id);
                        }
                    }
                }
                _ => (),
            }
        }
        None
    }

    fn check_drop_is_noop(&self, tab_id: LiveId, item_id: LiveId) -> bool {
        for (tabs_id, item) in self.dock_items.iter() {
            match item {
                DockItem::Tabs { tabs, .. } => {
                    if tabs.iter().any(|v| *v == tab_id) {
                        if *tabs_id == item_id && tabs.len() == 1 {
                            return true;
                        }
                    }
                }
                _ => (),
            }
        }
        false
    }

    fn handle_drop(&mut self, cx: &mut Cx, abs: Vec2d, item: LiveId, is_move: bool) -> bool {
        let Some(pos) = self.find_drop_position(cx, abs) else {
            return false;
        };
        self.handle_drop_position(cx, pos, item, is_move)
    }

    fn handle_drop_position(
        &mut self,
        cx: &mut Cx,
        mut pos: DropPosition,
        item: LiveId,
        is_move: bool,
    ) -> bool {
        if is_move
            && (!matches!(self.dock_items.get(&item), Some(DockItem::Tab { .. }))
                || self.find_tab_bar_of_tab(item).is_none())
        {
            return false;
        }
        // The bar between two panels is its own target: it belongs to a
        // splitter rather than to a tab container, so the checks below do not
        // describe it, and `drop_between` validates it for itself.
        if matches!(pos.part, DropPart::Bar) {
            self.needs_save = true;
            return self.drop_between(cx, pos.id, item, is_move);
        }
        // Validate before detaching the source. A programmatic target need not
        // have a drawn tab bar, but it must belong to the current layout.
        match pos.part {
            DropPart::Tab => {
                if !matches!(self.dock_items.get(&pos.id), Some(DockItem::Tab { .. }))
                    || self.find_tab_bar_of_tab(pos.id).is_none()
                    || (is_move && pos.id == item)
                {
                    return false;
                }
            }
            // An edge drop may land against a whole split as well as a single
            // container: that is how a panel goes above one, or across them all.
            DropPart::Left | DropPart::Right | DropPart::Top | DropPart::Bottom => {
                if !matches!(
                    self.dock_items.get(&pos.id),
                    Some(DockItem::Tabs { .. }) | Some(DockItem::Splitter { .. })
                ) || (is_move && self.check_drop_is_noop(item, pos.id))
                {
                    return false;
                }
            }
            _ => {
                if !matches!(self.dock_items.get(&pos.id), Some(DockItem::Tabs { .. }))
                    || (is_move && self.check_drop_is_noop(item, pos.id))
                {
                    return false;
                }
            }
        }
        self.needs_save = true;
        match pos.part {
            DropPart::Left | DropPart::Right | DropPart::Top | DropPart::Bottom => {
                if is_move {
                    self.close_tab(cx, item, true);
                    let Some(remapped_id) = self.remap_drop_tabs_after_move(pos.id) else {
                        return false;
                    };
                    pos.id = remapped_id;
                }
                let new_tabs = self.next_internal_id();
                self.dock_items.insert(
                    new_tabs,
                    DockItem::Tabs {
                        tabs: vec![item],
                        closable: true,
                        hide_tab_bar: false,
                        selected: 0,
                    },
                );
                if !self.split_tabs_container(cx, pos.id, new_tabs, pos.part) {
                    self.dock_items.remove(&new_tabs);
                    return false;
                }
                true
            }
            DropPart::Center | DropPart::TabBar => {
                if is_move {
                    self.close_tab(cx, item, true);
                    let Some(remapped_id) = self.remap_drop_tabs_after_move(pos.id) else {
                        return false;
                    };
                    pos.id = remapped_id;
                }
                self.push_tab_into_tabs(cx, pos.id, item)
            }
            DropPart::Tab => {
                if is_move {
                    self.close_tab(cx, item, true);
                }
                self.insert_tab_before_tab(cx, pos.id, item)
            }
            // Answered before anything was validated or detached.
            DropPart::Bar => false,
        }
    }

    fn move_tab(&mut self, cx: &mut Cx, item: LiveId, target: LiveId, part: DropPart) -> bool {
        if item == target || !matches!(self.dock_items.get(&target), Some(DockItem::Tab { .. })) {
            return false;
        }
        let Some((target_tabs, _)) = self.find_tab_bar_of_tab(target) else {
            return false;
        };
        let pos = DropPosition {
            part,
            id: if part == DropPart::Tab {
                target
            } else {
                target_tabs
            },
            rect: Rect::default(),
        };
        if !self.handle_drop_position(cx, pos, item, true) {
            return false;
        }
        self.select_tab(cx, target);
        self.select_tab(cx, item);
        self.area.redraw(cx);
        true
    }

    /// Put a tab between the two panels a splitter divides.
    ///
    /// The order here is the whole of the care. A tab has to leave
    /// where it is before it can go anywhere else, and leaving can heal
    /// a splitter away: the panel it empties takes its parent with it,
    /// and when that parent is the root it takes the root's OTHER child
    /// instead, which may be exactly the splitter that was aimed at. So
    /// the target is looked up again after the tab has gone, and if
    /// there is no seam left the tab is put back. A tab that no panel
    /// lists cannot be dragged back, cannot be selected and cannot be
    /// reopened, and the layout is saved in that state.
    fn drop_between(&mut self, cx: &mut Cx, split_id: LiveId, item: LiveId, is_move: bool) -> bool {
        let mut split_id = split_id;
        let came_from = self.find_tab_bar_of_tab(item);
        if is_move {
            if self.between_drop_is_noop(item, split_id) {
                return false;
            }
            self.close_tab(cx, item, true);
            match self.remap_drop_tabs_after_move(split_id) {
                Some(remapped) => split_id = remapped,
                None => {
                    self.restore_tab(cx, came_from, item);
                    return false;
                }
            }
        }
        if !matches!(self.dock_items.get(&split_id), Some(DockItem::Splitter { .. })) {
            if is_move {
                self.restore_tab(cx, came_from, item);
            }
            return false;
        }
        let new_tabs = self.next_internal_id();
        let new_split = self.next_internal_id();
        self.dock_items.insert(
            new_tabs,
            DockItem::Tabs { tabs: vec![item], closable: true, hide_tab_bar: false, selected: 0 },
        );
        if !Self::insert_between_split_children_in_items(
            &mut self.dock_items,
            split_id,
            new_tabs,
            new_split,
        ) {
            self.dock_items.remove(&new_tabs);
            if is_move {
                self.restore_tab(cx, came_from, item);
            }
            return false;
        }
        self.redraw_split(cx, split_id);
        true
    }

    /// Whether moving this tab onto this seam would change nothing. It
    /// would not if the tab is the only one in a panel the seam already
    /// divides: the same panels in the same order, under new ids and
    /// with the bar shoved back to the middle.
    fn between_drop_is_noop(&self, item: LiveId, split_id: LiveId) -> bool {
        let Some(DockItem::Splitter { a, b, .. }) = self.dock_items.get(&split_id) else {
            return false;
        };
        let Some((tabs_id, _)) = self.find_tab_bar_of_tab(item) else {
            return false;
        };
        if tabs_id != *a && tabs_id != *b {
            return false;
        }
        matches!(
            self.dock_items.get(&tabs_id),
            Some(DockItem::Tabs { tabs, .. }) if tabs.len() == 1
        )
    }

    /// Put a tab back after a drop gave up part way through.
    ///
    /// Where it came from is tried first. If that panel went with it,
    /// any panel will do: somewhere the person can see it and move it
    /// again is better than a tab nothing lists, which is gone for good
    /// and gets saved that way.
    fn restore_tab(&mut self, cx: &mut Cx, came_from: Option<(LiveId, usize)>, item: LiveId) {
        if let Some((tabs_id, at)) = came_from {
            if let Some(DockItem::Tabs { tabs, selected, .. }) = self.dock_items.get_mut(&tabs_id) {
                let at = at.min(tabs.len());
                tabs.insert(at, item);
                *selected = at;
                self.area.redraw(cx);
                return;
            }
        }
        let anywhere = self
            .dock_items
            .iter()
            .find(|(_, held)| matches!(held, DockItem::Tabs { .. }))
            .map(|(id, _)| *id);
        match anywhere {
            Some(tabs_id) => {
                self.push_tab_into_tabs(cx, tabs_id, item);
            }
            None => warning!("Dock: nowhere to put {:?} back", item),
        }
    }

    /// Redraw a splitter whose division changed.
    ///
    /// Both children, not just the area: a pane that only changed SIZE
    /// keeps its own draw list, and that list compares against the last
    /// frame's measurement and decides it has nothing to do. This is the
    /// same reason the splitter's own drag redraws its areas and their
    /// children rather than trusting the parent.
    fn redraw_split(&mut self, cx: &mut Cx, split_id: LiveId) {
        if let Some(DockItem::Splitter { a, b, .. }) = self.dock_items.get(&split_id) {
            let (a, b) = (*a, *b);
            self.redraw_item(cx, a);
            self.redraw_item(cx, b);
        }
        if let Some(splitter) = self.splitters.get(&split_id) {
            let (area_a, area_b) = (splitter.area_a(), splitter.area_b());
            cx.redraw_area_and_children(area_a);
            cx.redraw_area_and_children(area_b);
        }
        self.area.redraw(cx);
    }

    fn drop_create(
        &mut self,
        cx: &mut Cx,
        abs: Vec2d,
        item: LiveId,
        kind: LiveId,
        name: String,
        template: LiveId,
    ) {
        if self.handle_drop(cx, abs, item, false) {
            self.needs_save = true;
            self.dock_items.insert(
                item,
                DockItem::Tab {
                    name,
                    template,
                    kind,
                },
            );
            self.item_or_create(cx, item, kind);
            self.select_tab(cx, item);
            self.area.redraw(cx);
        }
    }

    fn drop_clone(
        &mut self,
        cx: &mut Cx,
        abs: Vec2d,
        item: LiveId,
        new_item: LiveId,
        template: LiveId,
    ) {
        if let Some(DockItem::Tab { name, kind, .. }) = self.dock_items.get(&item) {
            let name = name.clone();
            let kind = *kind;
            if self.handle_drop(cx, abs, new_item, false) {
                self.needs_save = true;
                self.dock_items.insert(
                    new_item,
                    DockItem::Tab {
                        name,
                        template,
                        kind,
                    },
                );
                self.item_or_create(cx, new_item, kind);
                self.select_tab(cx, new_item);
            }
        }
    }

    fn create_and_select_tab(
        &mut self,
        cx: &mut Cx,
        parent: LiveId,
        item: LiveId,
        kind: LiveId,
        name: String,
        template: LiveId,
        insert_after: Option<usize>,
    ) -> Option<WidgetRef> {
        if let Some(widgetref) = self.items.get(&item).map(|(_, w)| w.clone()) {
            self.select_tab(cx, item);
            Some(widgetref)
        } else {
            let ret = self.create_tab(cx, parent, item, kind, name, template, insert_after);
            self.select_tab(cx, item);
            ret
        }
    }

    fn create_tab(
        &mut self,
        cx: &mut Cx,
        parent: LiveId,
        item: LiveId,
        kind: LiveId,
        name: String,
        template: LiveId,
        insert_after: Option<usize>,
    ) -> Option<WidgetRef> {
        if let Some(DockItem::Tabs { tabs, .. }) = self.dock_items.get_mut(&parent) {
            if let Some(after) = insert_after {
                tabs.insert(after + 1, item);
            } else {
                tabs.push(item);
            }
            self.needs_save = true;
            self.dock_items.insert(
                item,
                DockItem::Tab {
                    name,
                    template,
                    kind,
                },
            );
            self.item_or_create(cx, item, kind)
        } else {
            None
        }
    }

    fn replace_tab(
        &mut self,
        cx: &mut Cx,
        tab_item_id: LiveId,
        new_kind: LiveId,
        new_name: Option<String>,
        select: bool,
    ) -> Option<(WidgetRef, bool)> {
        let Some(DockItem::Tab { name, kind, .. }) = self.dock_items.get_mut(&tab_item_id) else {
            return None;
        };
        if let Some(template_ref) = self.templates.get(&new_kind) {
            let template_value: ScriptValue = template_ref.as_object().into();
            let Some((existing_kind, existing_widgetref)) = self.items.get_mut(&tab_item_id) else {
                return None;
            };
            let (new_widgetref, was_replaced) = if *existing_kind == new_kind {
                (existing_widgetref.clone(), false)
            } else {
                *existing_kind = new_kind;
                *existing_widgetref =
                    cx.with_vm(|vm| WidgetRef::script_from_value(vm, template_value));
                *kind = new_kind;
                (existing_widgetref.clone(), true)
            };

            if let Some(new_name) = new_name {
                *name = new_name;
            }
            if select {
                self.select_tab(cx, tab_item_id);
            }
            self.needs_save = true;
            self.redraw_tab(cx, tab_item_id);
            Some((new_widgetref, was_replaced))
        } else {
            warning!("Template not found: {new_kind}. Did you add it to the <Dock> instance?");
            None
        }
    }

    pub fn drawing_item_id(&self) -> Option<LiveId> {
        if let Some(stack) = self.draw_state.as_ref() {
            match stack.last() {
                Some(DrawStackItem::Tab { id }) => return Some(*id),
                _ => (),
            }
        }
        None
    }

    pub fn load_state(&mut self, cx: &mut Cx, dock_items: HashMap<LiveId, DockItem>) {
        self.dock_items = dock_items;
        self.items.clear();
        self.tab_bars.clear();
        self.splitters.clear();
        // Reset so the next next_internal_id call re-seeds from loaded state.
        self.next_internal_id = 0;
        self.area.redraw(cx);
        self.create_all_items(cx);
    }

    /// Load a new node map without destroying stable tab bodies.
    ///
    /// This is for alternate layouts of the same logical editors. A tab
    /// absent from `dock_items` stays resident in `items`, ready to be drawn
    /// again when a later layout names it. If a present tab reuses an ID with
    /// a different kind, the old body is dropped and recreated normally.
    /// Splitters and tab bars are layout objects and are always rebuilt.
    pub fn load_state_preserving_items(
        &mut self,
        cx: &mut Cx,
        dock_items: HashMap<LiveId, DockItem>,
    ) {
        self.items
            .retain(|id, (old_kind, _)| preserve_item_for_layout(&dock_items, *id, *old_kind));
        self.dock_items = dock_items;
        self.tab_bars.clear();
        self.splitters.clear();
        // Reset so the next next_internal_id call re-seeds from loaded state.
        self.next_internal_id = 0;
        self.area.redraw(cx);
        self.create_all_items(cx);
    }
}

/// Whether `Dock::handle_event_with_bodies` dispatches to the tab bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyDispatch {
    Existing,
    Skip,
}

impl Dock {
    /// The full Dock event handler with the tab-body dispatch made explicit.
    /// `Existing` is the ordinary behaviour; `Skip` handles only the Dock's
    /// own chrome (splitters, tab bars, drag/drop) and leaves the bodies to
    /// the caller.
    pub fn handle_event_with_bodies(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        scope: &mut Scope,
        bodies: BodyDispatch,
    ) {
        let uid = self.widget_uid();
        let dock_items = &mut self.dock_items;
        for (panel_id, splitter) in self.splitters.iter_mut() {
            for action in cx.capture_actions(|cx| splitter.handle_event(cx, event, scope)) {
                match action.as_widget_action().cast() {
                    SplitterAction::Changed { axis, align } => {
                        if let Some(DockItem::Splitter {
                            axis: _axis,
                            align: _align,
                            ..
                        }) = dock_items.get_mut(&panel_id)
                        {
                            *_axis = axis;
                            *_align = align;
                        }
                        self.needs_save = true;
                        cx.widget_action(
                            uid,
                            DockAction::SplitPanelChanged {
                                panel_id: *panel_id,
                                axis,
                                align,
                            },
                        );
                    }
                    _ => (),
                }
            }
        }
        for (panel_id, tab_bar) in self.tab_bars.iter_mut() {
            let contents_view = &mut tab_bar.contents_draw_list;
            for action in cx.capture_actions(|cx| tab_bar.tab_bar.handle_event(cx, event, scope)) {
                match action.as_widget_action().cast() {
                    TabBarAction::ShouldTabStartDrag(item) => {
                        let name = if let Some(DockItem::Tab { name, .. }) = dock_items.get(&item) {
                            name.clone()
                        } else {
                            String::new()
                        };
                        let tab_size = tab_bar
                            .tab_bar
                            .tab_rect(cx, item)
                            .map(|r| r.size)
                            .unwrap_or(dvec2(100.0, 30.0));
                        if let Some(ghost) = tab_bar.tab_bar.create_ghost_tab(cx, item) {
                            self.dragging_tab = Some(DraggingTab {
                                cursor: Vec2d::default(),
                                name,
                                size: tab_size,
                                ghost,
                            });
                        } else {
                            warning!("Dock: could not create ghost tab for {:?}", item);
                        }
                        cx.widget_action(uid, DockAction::ShouldTabStartDrag(item))
                    }
                    TabBarAction::TabWasPressed(tab_id) => {
                        self.needs_save = true;
                        if let Some(DockItem::Tabs { tabs, selected, .. }) =
                            dock_items.get_mut(&panel_id)
                        {
                            if let Some(sel) = tabs.iter().position(|v| *v == tab_id) {
                                *selected = sel;
                                contents_view.redraw(cx);
                                cx.widget_action(uid, DockAction::TabWasPressed(tab_id))
                            } else {
                                log!("Cannot find tab {}", tab_id.0);
                            }
                        }
                    }
                    TabBarAction::TabCloseWasPressed(tab_id) => {
                        cx.widget_action(uid, DockAction::TabCloseWasPressed(tab_id));
                        self.needs_save = true;
                    }
                    TabBarAction::None => (),
                }
            }
        }
        // Drag/drop hit-testing must stay scoped to the visible tab content.
        // Otherwise hidden cached tab items can claim the drop before the
        // selected tab sees it.
        let visible_items_only = event.requires_visibility()
            || matches!(event, Event::Drag(_) | Event::Drop(_) | Event::DragEnd);

        if bodies == BodyDispatch::Skip {
            // The caller pumps the tab bodies itself (Studio's resident pump
            // delivers async events once across several presentations).
        } else if visible_items_only {
            for (_id, item) in self.visible_items() {
                item.handle_event(cx, event, scope);
            }
        } else {
            for (_id, (_templ_id, item)) in self.items.iter_mut() {
                item.handle_event(cx, event, scope);
            }
        }

        if let Event::DragEnd = event {
            self.drop_state = None;
            self.dragging_tab = None;
            let redraw_id = cx.redraw_id;
            let ghost_dl_id = self.ghost_tab_draw_list.draw_list_id();
            let recording_gen = cx.next_uniform_gen();
            let uniforms_gen = cx.next_uniform_gen();
            cx.draw_lists[ghost_dl_id].clear_draw_items(
                redraw_id,
                recording_gen,
                uniforms_gen,
            );
            if let Some(pass_id) = cx.draw_lists[ghost_dl_id].draw_pass_id {
                cx.repaint_pass_and_child_passes(pass_id);
            }
            let drop_dl_id = self.drop_target_draw_list.draw_list_id();
            let recording_gen = cx.next_uniform_gen();
            let uniforms_gen = cx.next_uniform_gen();
            cx.draw_lists[drop_dl_id].clear_draw_items(
                redraw_id,
                recording_gen,
                uniforms_gen,
            );
            self.area.redraw(cx);
        }

        match event.drag_hits(cx, self.area) {
            DragHit::Drag(f) => {
                self.drop_state = None;
                // Update ghost tab cursor position.
                if let Some(ref mut dt) = self.dragging_tab {
                    dt.cursor = f.abs;
                }
                self.area.redraw(cx);
                self.drop_target_draw_list.redraw(cx);
                match f.state {
                    DragState::In | DragState::Over => {
                        cx.widget_action(uid, DockAction::Drag(f.clone()))
                    }
                    DragState::Out => {}
                }
            }
            DragHit::Drop(f) => {
                self.needs_save = true;
                self.drop_state = None;
                self.dragging_tab = None;
                let redraw_id = cx.redraw_id;
                let recording_gen = cx.next_uniform_gen();
                let uniforms_gen = cx.next_uniform_gen();
                cx.draw_lists[self.ghost_tab_draw_list.draw_list_id()].clear_draw_items(
                    redraw_id,
                    recording_gen,
                    uniforms_gen,
                );
                let recording_gen = cx.next_uniform_gen();
                let uniforms_gen = cx.next_uniform_gen();
                cx.draw_lists[self.drop_target_draw_list.draw_list_id()]
                    .clear_draw_items(redraw_id, recording_gen, uniforms_gen);
                self.area.redraw(cx);
                cx.widget_action(uid, DockAction::Drop(f.clone()))
            }
            DragHit::DragEnd => {
                self.drop_state = None;
                self.dragging_tab = None;
                let redraw_id = cx.redraw_id;
                let recording_gen = cx.next_uniform_gen();
                let uniforms_gen = cx.next_uniform_gen();
                cx.draw_lists[self.ghost_tab_draw_list.draw_list_id()].clear_draw_items(
                    redraw_id,
                    recording_gen,
                    uniforms_gen,
                );
                let recording_gen = cx.next_uniform_gen();
                let uniforms_gen = cx.next_uniform_gen();
                cx.draw_lists[self.drop_target_draw_list.draw_list_id()]
                    .clear_draw_items(redraw_id, recording_gen, uniforms_gen);
                self.area.redraw(cx);
            }
            _ => {}
        }
    }

}

impl Widget for Dock {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.handle_event_with_bodies(cx, event, scope, BodyDispatch::Existing)
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let dock_uid = self.widget_uid();
        if self
            .draw_state
            .begin_with(cx, &self.dock_items, |_, dock_items| {
                let id = id!(root);
                let root_item = dock_items.get(&id);
                vec![DrawStackItem::from_dock_item(id, root_item)]
            })
        {
            self.begin(cx, walk);
        }

        while let Some(stack) = self.draw_state.as_mut() {
            let item = stack.pop();
            match item {
                Some(DrawStackItem::SplitLeft { id }) => {
                    stack.push(DrawStackItem::SplitRight { id });
                    let splitter_template = self.splitter.clone();
                    let splitter = self.splitters.get_or_insert(cx, id, |cx| {
                        cx.with_vm(|vm| {
                            Splitter::script_from_value(vm, splitter_template.as_object().into())
                        })
                    });
                    if let Some(DockItem::Splitter { axis, align, a, .. }) =
                        self.dock_items.get(&id)
                    {
                        splitter.set_axis(*axis);
                        splitter.set_align(*align);
                        splitter.begin(cx, Walk::fill());
                        stack.push(DrawStackItem::from_dock_item(*a, self.dock_items.get(a)));
                        continue;
                    } else {
                        panic!()
                    }
                }
                Some(DrawStackItem::SplitRight { id }) => {
                    stack.push(DrawStackItem::SplitEnd { id });
                    let splitter = self.splitters.get_mut(&id).unwrap();
                    splitter.middle(cx);
                    if let Some(DockItem::Splitter { b, .. }) = self.dock_items.get(&id) {
                        stack.push(DrawStackItem::from_dock_item(*b, self.dock_items.get(b)));
                        continue;
                    } else {
                        panic!()
                    }
                }
                Some(DrawStackItem::SplitEnd { id }) => {
                    let splitter = self.splitters.get_mut(&id).unwrap();
                    splitter.end(cx);
                }
                Some(DrawStackItem::Tabs { id }) => {
                    if let Some(DockItem::Tabs {
                        selected,
                        hide_tab_bar,
                        ..
                    }) = self.dock_items.get(&id)
                    {
                        let tab_bar_template = self.tab_bar.clone();
                        let tab_bar = self.tab_bars.get_or_insert(cx, id, |cx| {
                            cx.with_vm(|vm| TabBarWrap {
                                tab_bar: TabBar::script_from_value(
                                    vm,
                                    tab_bar_template.as_object().into(),
                                ),
                                contents_draw_list: DrawList2d::script_new(vm),
                                contents_rect: Rect::default(),
                            })
                        });
                        if !*hide_tab_bar {
                            let walk = tab_bar.tab_bar.walk(cx);
                            tab_bar.tab_bar.tree_parent = dock_uid;
                            tab_bar.tab_bar.begin(cx, Some(*selected), walk);
                            stack.push(DrawStackItem::TabLabel { id, index: 0 });
                        } else {
                            // Skip the tab bar entirely, go straight to content.
                            stack.push(DrawStackItem::TabLabel {
                                id,
                                index: usize::MAX,
                            });
                        }
                    } else {
                        panic!()
                    }
                }
                Some(DrawStackItem::TabLabel { id, index }) => {
                    if let Some(DockItem::Tabs { tabs, selected, .. }) = self.dock_items.get(&id) {
                        let tab_bar = self.tab_bars.get_mut(&id).unwrap();
                        if index < tabs.len() {
                            if let Some(DockItem::Tab { name, template, .. }) =
                                self.dock_items.get(&tabs[index])
                            {
                                tab_bar
                                    .tab_bar
                                    .draw_tab(cx, tabs[index].into(), name, *template);
                            }
                            stack.push(DrawStackItem::TabLabel {
                                id,
                                index: index + 1,
                            });
                        } else {
                            if index != usize::MAX {
                                tab_bar.tab_bar.end(cx);
                            }
                            tab_bar.contents_rect = cx.turtle().rect();
                            if !tabs.is_empty()
                                && tab_bar
                                    .contents_draw_list
                                    .begin(cx, Walk::default())
                                    .is_redrawing()
                            {
                                stack.push(DrawStackItem::TabContent { id });
                                if *selected < tabs.len() {
                                    stack.push(DrawStackItem::Tab {
                                        id: tabs[*selected],
                                    });
                                }
                            }
                        }
                    } else {
                        panic!()
                    }
                }
                Some(DrawStackItem::Tab { id }) => {
                    stack.push(DrawStackItem::Tab { id });
                    if let Some(DockItem::Tab { kind, .. }) = self.dock_items.get(&id) {
                        if let Some(template_ref) = self.templates.get(kind) {
                            let template_value: ScriptValue = template_ref.as_object().into();
                            let kind_copy = *kind;
                            let existed = self.items.contains_key(&id);
                            let (_, entry) = self.items.get_or_insert(cx, id, |cx| {
                                cx.with_vm(|vm| {
                                    (kind_copy, WidgetRef::script_from_value(vm, template_value))
                                })
                            });
                            if !existed {
                                cx.widget_tree_insert_child_deep(self.uid, id, entry.clone());
                            }
                            entry.draw(cx, scope)?;
                        }
                    }
                    stack.pop();
                }
                Some(DrawStackItem::TabContent { id }) => {
                    if let Some(DockItem::Tabs { .. }) = self.dock_items.get(&id) {
                        let tab_bar = self.tab_bars.get_mut(&id).unwrap();
                        tab_bar.contents_draw_list.end(cx);
                    } else {
                        panic!()
                    }
                }
                Some(DrawStackItem::Invalid) => {}
                None => break,
            }
        }

        self.end(cx);
        self.draw_state.end();

        DrawStep::done()
    }
}

impl DockRef {
    pub fn item(&self, entry_id: LiveId) -> WidgetRef {
        if let Some(dock) = self.borrow() {
            if let Some(item) = dock.item(entry_id) {
                return item;
            }
        }
        WidgetRef::empty()
    }

    pub fn item_or_create(
        &self,
        cx: &mut Cx,
        entry_id: LiveId,
        template: LiveId,
    ) -> Option<WidgetRef> {
        if let Some(mut dock) = self.borrow_mut() {
            return dock.item_or_create(cx, entry_id, template);
        }
        None
    }

    pub fn close_tab(&self, cx: &mut Cx, tab_id: LiveId) {
        if let Some(mut dock) = self.borrow_mut() {
            dock.close_tab(cx, tab_id, false);
        }
    }

    pub fn accept_drag(&self, cx: &mut Cx, dh: DragHitEvent, dr: DragResponse) {
        if let Some(mut dock) = self.borrow_mut() {
            if let Some(pos) = dock.find_drop_position(cx, dh.abs) {
                *dh.response.lock().unwrap() = dr;
                dock.drop_state = Some(pos);
            } else {
                dock.drop_state = None;
            }
        }
    }

    pub fn drawing_item_id(&self) -> Option<LiveId> {
        if let Some(dock) = self.borrow() {
            return dock.drawing_item_id();
        }
        None
    }

    pub fn drop_clone(
        &self,
        cx: &mut Cx,
        abs: Vec2d,
        old_item: LiveId,
        new_item: LiveId,
        template: LiveId,
    ) {
        if let Some(mut dock) = self.borrow_mut() {
            dock.drop_clone(cx, abs, old_item, new_item, template);
        }
    }

    pub fn drop_move(&self, cx: &mut Cx, abs: Vec2d, item: LiveId) {
        if let Some(mut dock) = self.borrow_mut() {
            dock.handle_drop(cx, abs, item, true);
        }
    }

    /// Moves an existing tab relative to another tab without needing drawn
    /// geometry. Edge parts split the target's pane, Center/TabBar append to
    /// that pane, and Tab inserts before the target. Live tab bodies are kept.
    /// Returns false for unknown tabs or a self target without changing layout.
    pub fn move_tab(&self, cx: &mut Cx, item: LiveId, target: LiveId, part: DropPart) -> bool {
        self.borrow_mut()
            .is_some_and(|mut dock| dock.move_tab(cx, item, target, part))
    }

    pub fn drop_create(
        &self,
        cx: &mut Cx,
        abs: Vec2d,
        item: LiveId,
        kind: LiveId,
        name: String,
        template: LiveId,
    ) {
        if let Some(mut dock) = self.borrow_mut() {
            dock.drop_create(cx, abs, item, kind, name, template);
        }
    }

    pub fn create_and_select_tab(
        &self,
        cx: &mut Cx,
        parent: LiveId,
        item: LiveId,
        kind: LiveId,
        name: String,
        template: LiveId,
        insert_after: Option<usize>,
    ) -> Option<WidgetRef> {
        if let Some(mut dock) = self.borrow_mut() {
            dock.create_and_select_tab(cx, parent, item, kind, name, template, insert_after)
        } else {
            None
        }
    }

    pub fn create_tab(
        &self,
        cx: &mut Cx,
        parent: LiveId,
        item: LiveId,
        kind: LiveId,
        name: String,
        template: LiveId,
        insert_after: Option<usize>,
    ) -> Option<WidgetRef> {
        if let Some(mut dock) = self.borrow_mut() {
            dock.create_tab(cx, parent, item, kind, name, template, insert_after)
        } else {
            None
        }
    }

    pub fn replace_tab(
        &self,
        cx: &mut Cx,
        tab_item_id: LiveId,
        new_kind: LiveId,
        new_name: Option<String>,
        select: bool,
    ) -> Option<(WidgetRef, bool)> {
        let Some(mut dock) = self.borrow_mut() else {
            return None;
        };
        dock.replace_tab(cx, tab_item_id, new_kind, new_name, select)
    }

    pub fn set_tab_title(&self, cx: &mut Cx, tab: LiveId, title: String) {
        if let Some(mut dock) = self.borrow_mut() {
            dock.set_tab_title(cx, tab, title);
        }
    }

    pub fn find_tab_bar_of_tab(&self, tab_id: LiveId) -> Option<(LiveId, usize)> {
        if let Some(dock) = self.borrow() {
            return dock.find_tab_bar_of_tab(tab_id);
        }
        None
    }

    pub fn drop_target_tab_id(&self, cx: &Cx, abs: Vec2d) -> Option<LiveId> {
        if let Some(dock) = self.borrow() {
            return dock.drop_target_tab_id(cx, abs);
        }
        None
    }

    pub fn select_tab(&self, cx: &mut Cx, item: LiveId) {
        if let Some(mut dock) = self.borrow_mut() {
            dock.select_tab(cx, item);
        }
    }

    pub fn redraw_tab(&self, cx: &mut Cx, tab_id: LiveId) {
        if let Some(mut dock) = self.borrow_mut() {
            dock.redraw_tab(cx, tab_id);
        }
    }

    pub fn splitter_position(&self, splitter_id: LiveId) -> Option<f64> {
        self.borrow()
            .and_then(|dock| dock.splitter_position(splitter_id))
    }

    pub fn set_splitter_align(
        &self,
        cx: &mut Cx,
        splitter_id: LiveId,
        align: SplitterAlign,
        mark_dirty: bool,
    ) -> bool {
        self.borrow_mut()
            .is_some_and(|mut dock| dock.set_splitter_align(cx, splitter_id, align, mark_dirty))
    }

    pub fn unique_id(&self, base: u64) -> LiveId {
        if let Some(dock) = self.borrow() {
            return dock.unique_id(base);
        }
        LiveId(0)
    }

    pub fn check_and_clear_need_save(&self) -> bool {
        if let Some(mut dock) = self.borrow_mut() {
            if dock.needs_save {
                dock.needs_save = false;
                return true;
            }
        }
        false
    }

    pub fn clone_state(&self) -> Option<HashMap<LiveId, DockItem>> {
        if let Some(dock) = self.borrow() {
            return Some(dock.dock_items.clone());
        }
        None
    }

    pub fn load_state(&self, cx: &mut Cx, dock_items: HashMap<LiveId, DockItem>) {
        if let Some(mut dock) = self.borrow_mut() {
            dock.load_state(cx, dock_items);
        }
    }

    pub fn load_state_preserving_items(
        &self,
        cx: &mut Cx,
        dock_items: HashMap<LiveId, DockItem>,
    ) {
        if let Some(mut dock) = self.borrow_mut() {
            dock.load_state_preserving_items(cx, dock_items);
        }
    }

    pub fn tab_start_drag(&self, cx: &mut Cx, _tab_id: LiveId, item: DragItem) {
        cx.start_dragging(vec![item]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { pos: Vec2d { x, y }, size: Vec2d { x: w, y: h } }
    }

    fn at(x: f64, y: f64) -> Vec2d {
        Vec2d { x, y }
    }

    fn weight(align: SplitterAlign) -> f64 {
        match align {
            SplitterAlign::Weighted(w) => w,
            _ => panic!("not a weighted split"),
        }
    }
    fn pinned_from_a(align: SplitterAlign) -> f64 {
        match align {
            SplitterAlign::FromA(px) => px,
            _ => panic!("not pinned to the near side"),
        }
    }

    fn pinned_from_b(align: SplitterAlign) -> f64 {
        match align {
            SplitterAlign::FromB(px) => px,
            _ => panic!("not pinned to the far side"),
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    /// A splitter of two tab containers, sharing the axis evenly.
    fn two_panels(axis: SplitterAxis) -> (HashMap<LiveId, DockItem>, LiveId, LiveId, LiveId) {
        let (near, far, root) = (LiveId(1), LiveId(2), id!(root));
        let mut items = HashMap::new();
        items.insert(near, DockItem::tabs(vec![LiveId(10)], 0, false));
        items.insert(far, DockItem::tabs(vec![LiveId(11)], 0, false));
        items.insert(root, DockItem::Splitter {
            axis,
            align: SplitterAlign::Weighted(0.5),
            a: near,
            b: far,
        });
        (items, near, far, root)
    }

    #[test]
    fn a_panel_dropped_between_two_halves_makes_three_thirds() {
        let (mut items, near, far, root) = two_panels(SplitterAxis::Vertical);
        let (newcomer, new_split) = (LiveId(3), LiveId(4));
        items.insert(newcomer, DockItem::tabs(vec![LiveId(12)], 0, false));
        assert!(Dock::insert_between_split_children_in_items(
            &mut items, root, newcomer, new_split,
        ));
        let Some(DockItem::Splitter { axis, align, a, b }) = items.get(&root) else {
            panic!("the seam keeps its own id");
        };
        assert_eq!(*axis, SplitterAxis::Vertical);
        assert!(close(weight(*align), 1.0 / 3.0));
        assert_eq!((*a, *b), (near, new_split));
        let Some(DockItem::Splitter { align, a, b, .. }) = items.get(&new_split) else {
            panic!("the far side should have been pushed down");
        };
        // The near panel keeps a third; the other two halve the rest.
        assert!(close(weight(*align), 0.5));
        assert_eq!((*a, *b), (newcomer, far));
    }

    #[test]
    fn everything_along_the_axis_keeps_an_equal_share() {
        // The near side is already a stack of two, so the answer is not
        // thirds: it is four panels, and the near pair keep half between
        // them because they are two of the four.
        let plan = between_split(SplitterAlign::Weighted(0.5), 2, 1);
        assert!(close(weight(plan.outer), 0.5));
        assert!(close(weight(plan.inner), 0.5));
        assert!(!plan.near_side);
        // And the other way round: one near, two far.
        let plan = between_split(SplitterAlign::Weighted(0.5), 1, 2);
        assert!(close(weight(plan.outer), 0.25));
        assert!(close(weight(plan.inner), 1.0 / 3.0));
    }

    #[test]
    fn a_pinned_pane_is_left_where_it_was_pinned() {
        // A bar held a fixed distance from one side is a pane whose size
        // somebody decided. Equalising it would turn a fixed sidebar into
        // a proportional one, so the newcomer goes on the other side of
        // the seam and the pinned pane does not move.
        let plan = between_split(SplitterAlign::FromA(310.0), 1, 1);
        assert_eq!(pinned_from_a(plan.outer), 310.0);
        assert!(close(weight(plan.inner), 0.5));
        assert!(!plan.near_side);

        let plan = between_split(SplitterAlign::FromB(120.0), 2, 1);
        assert_eq!(pinned_from_b(plan.outer), 120.0);
        // The newcomer joins the two on the near side and all three
        // share what the pinned pane left them.
        assert!(close(weight(plan.inner), 2.0 / 3.0));
        assert!(plan.near_side);
    }

    #[test]
    fn the_preview_covers_the_slot_the_drop_would_make() {
        // Not the bar that was aimed at: the bar moves.
        let (offset, len) = newcomer_slot(900.0, SplitterAlign::Weighted(0.5), 1, 1);
        assert!(close(offset, 300.0));
        assert!(close(len, 300.0));
        // Pinned near: the newcomer starts at the pin and takes half of
        // what is left.
        let (offset, len) = newcomer_slot(900.0, SplitterAlign::FromA(300.0), 1, 1);
        assert!(close(offset, 300.0));
        assert!(close(len, 300.0));
        // Pinned far: the newcomer ends at the pin.
        let (offset, len) = newcomer_slot(900.0, SplitterAlign::FromB(300.0), 1, 1);
        assert!(close(offset, 300.0));
        assert!(close(len, 300.0));
    }

    #[test]
    fn a_pin_wider_than_the_splitter_leaves_no_negative_slot() {
        let (offset, len) = newcomer_slot(200.0, SplitterAlign::FromA(900.0), 1, 1);
        assert!(close(offset, 200.0));
        assert!(close(len, 0.0));
        let (offset, len) = newcomer_slot(200.0, SplitterAlign::FromB(900.0), 1, 1);
        assert!(close(offset, 0.0));
        assert!(close(len, 0.0));
    }

    #[test]
    fn a_seam_drop_refuses_what_it_cannot_do() {
        let (mut items, near, _far, root) = two_panels(SplitterAxis::Horizontal);
        let newcomer = LiveId(3);
        items.insert(newcomer, DockItem::tabs(vec![LiveId(12)], 0, false));
        // Not a splitter.
        assert!(!Dock::insert_between_split_children_in_items(
            &mut items, near, newcomer, LiveId(4),
        ));
        // The newcomer is one of the two already.
        assert!(!Dock::insert_between_split_children_in_items(
            &mut items, root, near, LiveId(4),
        ));
        // The id for the new seam is taken.
        assert!(!Dock::insert_between_split_children_in_items(
            &mut items, root, newcomer, near,
        ));
        // Not a tab container.
        assert!(!Dock::insert_between_split_children_in_items(
            &mut items, root, LiveId(99), LiveId(4),
        ));
        // None of that touched anything.
        let Some(DockItem::Splitter { align, .. }) = items.get(&root) else { panic!() };
        assert!(close(weight(*align), 0.5));
    }

    #[test]
    fn two_drops_on_the_same_seam_end_up_as_quarters() {
        let (mut items, near, far, root) = two_panels(SplitterAxis::Vertical);
        for (n, (newcomer, new_split)) in
            [(LiveId(3), LiveId(4)), (LiveId(5), LiveId(6))].into_iter().enumerate()
        {
            items.insert(newcomer, DockItem::tabs(vec![LiveId(20 + n as u64)], 0, false));
            assert!(Dock::insert_between_split_children_in_items(
                &mut items, root, newcomer, new_split,
            ));
        }
        // Four panels along one axis, so the first bar stands a quarter in.
        let Some(DockItem::Splitter { align, .. }) = items.get(&root) else { panic!() };
        assert!(close(weight(*align), 0.25));
        assert_eq!(Dock::axis_leaves(&items, root, SplitterAxis::Vertical), 4);
        let _ = (near, far);
    }

    #[test]
    fn a_panels_body_starts_under_its_tab_bar() {
        let body = panel_body(rect(0., 0., 300., 340.), rect(0., 0., 300., 34.));
        assert_eq!(body, rect(0., 34., 300., 306.));
    }

    #[test]
    fn a_panel_with_no_tab_bar_is_all_body() {
        let whole = rect(10., 20., 300., 340.);
        assert_eq!(panel_body(whole, rect(0., 0., 0., 0.)), whole);
    }

    #[test]
    fn the_top_edge_can_be_reached_under_the_tab_bar() {
        // Measured against the whole panel the top tenth is 34 points —
        // exactly the strip — so every press meant for the top edge was a
        // press on the tab bar, and the only vertical split anybody could
        // make was downwards.
        let whole = rect(0., 0., 300., 340.);
        let body = panel_body(whole, rect(0., 0., 300., 34.));
        let just_below_the_bar = at(150., 40.);
        assert_eq!(drop_band(body, just_below_the_bar).unwrap().0, DropPart::Top);
        assert_eq!(drop_band(whole, just_below_the_bar).unwrap().0, DropPart::Center);
    }

    #[test]
    fn all_four_edges_of_a_panel_split_and_the_middle_joins() {
        let body = rect(0., 0., 300., 300.);
        assert_eq!(drop_band(body, at(5., 150.)).unwrap().0, DropPart::Left);
        assert_eq!(drop_band(body, at(295., 150.)).unwrap().0, DropPart::Right);
        assert_eq!(drop_band(body, at(150., 5.)).unwrap().0, DropPart::Top);
        assert_eq!(drop_band(body, at(150., 295.)).unwrap().0, DropPart::Bottom);
        assert_eq!(drop_band(body, at(150., 150.)).unwrap().0, DropPart::Center);
        assert!(drop_band(body, at(400., 150.)).is_none());
    }

    #[test]
    fn a_split_preview_covers_the_half_it_would_take() {
        let body = rect(100., 200., 300., 400.);
        assert_eq!(drop_band(body, at(110., 400.)).unwrap().1, rect(100., 200., 150., 400.));
        assert_eq!(drop_band(body, at(390., 400.)).unwrap().1, rect(250., 200., 150., 400.));
        assert_eq!(drop_band(body, at(250., 210.)).unwrap().1, rect(100., 200., 300., 200.));
        assert_eq!(drop_band(body, at(250., 590.)).unwrap().1, rect(100., 400., 300., 200.));
    }

    #[test]
    fn only_the_outside_of_the_dock_is_an_outer_drop() {
        let whole = rect(0., 0., 800., 600.);
        assert_eq!(outer_band(whole, at(4., 300.)).unwrap().0, DropPart::Left);
        assert_eq!(outer_band(whole, at(796., 300.)).unwrap().0, DropPart::Right);
        assert_eq!(outer_band(whole, at(400., 4.)).unwrap().0, DropPart::Top);
        assert_eq!(outer_band(whole, at(400., 596.)).unwrap().0, DropPart::Bottom);
        // Well inside, and on a panel edge that is nowhere near the
        // outside: the panel under the pointer answers for it.
        assert!(outer_band(whole, at(400., 300.)).is_none());
        assert!(outer_band(whole, at(120., 300.)).is_none());
    }

    #[test]
    fn an_outer_drop_previews_half_the_whole_dock() {
        // Not half a panel: the point of it is that it lies across them.
        let whole = rect(0., 0., 800., 600.);
        assert_eq!(outer_band(whole, at(4., 300.)).unwrap().1, rect(0., 0., 400., 600.));
        assert_eq!(outer_band(whole, at(400., 596.)).unwrap().1, rect(0., 300., 800., 300.));
    }

    #[test]
    fn a_dock_too_small_to_have_an_inside_has_no_outer_band() {
        assert!(outer_band(rect(0., 0., 60., 600.), at(4., 300.)).is_none());
    }

    #[test]
    fn a_newcomer_beside_one_panel_halves_it() {
        assert_eq!(weight(equal_share(1, true)), 0.5);
        assert_eq!(weight(equal_share(1, false)), 0.5);
    }

    #[test]
    fn a_newcomer_under_two_stacked_panels_makes_three_thirds() {
        // The pair keep two thirds and are already halving it, so all
        // three end up the same height.
        assert!((weight(equal_share(2, false)) - 2.0 / 3.0).abs() < 1e-12);
        assert!((weight(equal_share(2, true)) - 1.0 / 3.0).abs() < 1e-12);
        assert!((weight(equal_share(3, true)) - 0.25).abs() < 1e-12);
    }

    #[test]
    fn a_stack_is_counted_along_its_own_axis_only() {
        let (top, bottom, root) = (LiveId(1), LiveId(2), id!(root));
        let mut items = HashMap::new();
        items.insert(top, DockItem::tabs(vec![LiveId(10)], 0, false));
        items.insert(bottom, DockItem::tabs(vec![LiveId(11)], 0, false));
        items.insert(root, DockItem::Splitter {
            axis: SplitterAxis::Vertical,
            align: SplitterAlign::Weighted(0.5),
            a: top,
            b: bottom,
        });
        // Two along the axis they share; across it the pair is one band,
        // which is why a drop down the side gives a half and not a third.
        assert_eq!(Dock::axis_leaves(&items, root, SplitterAxis::Vertical), 2);
        assert_eq!(Dock::axis_leaves(&items, root, SplitterAxis::Horizontal), 1);
        assert_eq!(Dock::axis_leaves(&items, top, SplitterAxis::Vertical), 1);
    }

    #[test]
    fn a_panel_laid_across_a_stack_leaves_the_stack_stacked() {
        // A over B, and a newcomer dropped down the left of everything.
        let (top, bottom, root) = (LiveId(1), LiveId(2), id!(root));
        let (newcomer, moved_root, new_split) = (LiveId(3), LiveId(4), LiveId(5));
        let mut items = HashMap::new();
        items.insert(top, DockItem::tabs(vec![LiveId(10)], 0, false));
        items.insert(bottom, DockItem::tabs(vec![LiveId(11)], 0, false));
        items.insert(newcomer, DockItem::tabs(vec![LiveId(12)], 0, false));
        items.insert(root, DockItem::Splitter {
            axis: SplitterAxis::Vertical,
            align: SplitterAlign::Weighted(0.5),
            a: top,
            b: bottom,
        });
        assert!(Dock::split_tabs_container_in_items(
            &mut items, root, newcomer, new_split, Some(moved_root), DropPart::Left,
        ));
        let Some(DockItem::Splitter { axis, align, a, b }) = items.get(&root) else {
            panic!("the root should be the new split");
        };
        assert_eq!(*axis, SplitterAxis::Horizontal);
        assert_eq!(weight(*align), 0.5);
        assert_eq!(*a, newcomer);
        assert_eq!(*b, moved_root);
        // The pair that were sharing the square are still sharing what is
        // left of it, top and bottom.
        let Some(DockItem::Splitter { axis, a, b, .. }) = items.get(&moved_root) else {
            panic!("the old root should have moved aside");
        };
        assert_eq!(*axis, SplitterAxis::Vertical);
        assert_eq!((*a, *b), (top, bottom));
    }

    #[test]
    fn a_panel_dropped_under_a_stack_joins_it_as_an_equal() {
        let (top, bottom, root) = (LiveId(1), LiveId(2), id!(root));
        let (newcomer, moved_root, new_split) = (LiveId(3), LiveId(4), LiveId(5));
        let mut items = HashMap::new();
        items.insert(top, DockItem::tabs(vec![LiveId(10)], 0, false));
        items.insert(bottom, DockItem::tabs(vec![LiveId(11)], 0, false));
        items.insert(newcomer, DockItem::tabs(vec![LiveId(12)], 0, false));
        items.insert(root, DockItem::Splitter {
            axis: SplitterAxis::Vertical,
            align: SplitterAlign::Weighted(0.5),
            a: top,
            b: bottom,
        });
        assert!(Dock::split_tabs_container_in_items(
            &mut items, root, newcomer, new_split, Some(moved_root), DropPart::Bottom,
        ));
        let Some(DockItem::Splitter { axis, align, a, b }) = items.get(&root) else {
            panic!("the root should be the new split");
        };
        assert_eq!(*axis, SplitterAxis::Vertical);
        assert!((weight(*align) - 2.0 / 3.0).abs() < 1e-12);
        assert_eq!((*a, *b), (moved_root, newcomer));
    }

    #[test]
    fn splitting_one_panel_of_a_stack_nests_inside_it() {
        // The other reading of the same gesture: aimed at a panel rather
        // than at the dock, only that panel is divided.
        let (top, bottom, root) = (LiveId(1), LiveId(2), id!(root));
        let (newcomer, new_split) = (LiveId(3), LiveId(5));
        let mut items = HashMap::new();
        items.insert(top, DockItem::tabs(vec![LiveId(10)], 0, false));
        items.insert(bottom, DockItem::tabs(vec![LiveId(11)], 0, false));
        items.insert(newcomer, DockItem::tabs(vec![LiveId(12)], 0, false));
        items.insert(root, DockItem::Splitter {
            axis: SplitterAxis::Vertical,
            align: SplitterAlign::Weighted(0.5),
            a: top,
            b: bottom,
        });
        assert!(Dock::split_tabs_container_in_items(
            &mut items, top, newcomer, new_split, None, DropPart::Right,
        ));
        let Some(DockItem::Splitter { a, b, .. }) = items.get(&root) else {
            panic!("the root is still the stack");
        };
        assert_eq!((*a, *b), (new_split, bottom));
        let Some(DockItem::Splitter { axis, align, a, b }) = items.get(&new_split) else {
            panic!("the top panel should have become a split");
        };
        assert_eq!(*axis, SplitterAxis::Horizontal);
        assert_eq!(weight(*align), 0.5);
        assert_eq!((*a, *b), (top, newcomer));
    }

    fn dock_with_tabs(cx: &mut Cx) -> (DockRef, Vec<(LiveId, WidgetRef)>) {
        cx.with_vm(crate::script_mod);
        let dock = cx.with_vm(Dock::script_new);
        let dock = WidgetRef::new_with_inner(Box::new(dock)).as_dock();
        let mut bodies = Vec::new();
        for id in [id!(first), id!(second), id!(third)] {
            let body = cx.with_vm(crate::label::Label::script_new);
            bodies.push((id, WidgetRef::new_with_inner(Box::new(body))));
        }
        {
            let mut dock = dock.borrow_mut().unwrap();
            dock.dock_items.insert(
                id!(root),
                DockItem::tabs(bodies.iter().map(|(id, _)| *id).collect(), 0, true),
            );
            for (id, body) in &bodies {
                dock.dock_items.insert(
                    *id,
                    DockItem::tab(id.to_string(), id!(TestBody), id!(TestTab)),
                );
                dock.items.insert(*id, (id!(TestBody), body.clone()));
            }
        }
        (dock, bodies)
    }

    fn assert_intact(dock: &DockRef, bodies: &[(LiveId, WidgetRef)], selected_tab: LiveId) {
        let dock = dock.borrow().unwrap();
        // Every container and tab remains reachable once; there are no empty
        // panes left behind by repeatedly moving their final tab away.
        let mut pending = vec![id!(root)];
        let mut visited = std::collections::HashSet::new();
        let mut selected_found = false;
        while let Some(id) = pending.pop() {
            assert!(visited.insert(id), "duplicate or cyclic node: {id}");
            match dock.dock_items.get(&id).unwrap() {
                DockItem::Splitter { a, b, .. } => pending.extend([*a, *b]),
                DockItem::Tabs { tabs, selected, .. } => {
                    assert!(!tabs.is_empty());
                    assert!(*selected < tabs.len());
                    selected_found |= tabs[*selected] == selected_tab;
                    pending.extend(tabs.iter().copied());
                }
                DockItem::Tab { .. } => {}
            }
        }
        assert_eq!(visited.len(), dock.dock_items.len());
        assert!(selected_found);
        for (id, original) in bodies {
            let current = dock.item(*id).unwrap();
            assert!(&current == original, "moving a tab replaced its live body");
            assert!(current.borrow::<crate::label::Label>().is_some());
        }
    }

    #[test]
    fn identity_moves_repeatedly_split_and_merge_without_replacing_bodies() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (dock, bodies) = dock_with_tabs(&mut cx);
        // These moves run before any draw, as required by automation tools.
        for side in [
            DropPart::Left,
            DropPart::Right,
            DropPart::Top,
            DropPart::Bottom,
        ] {
            for merge in [DropPart::Center, DropPart::TabBar, DropPart::Tab] {
                assert!(dock.move_tab(&mut cx, id!(second), id!(first), side));
                assert_intact(&dock, &bodies, id!(second));
                assert!(matches!(
                    dock.borrow().unwrap().dock_items.get(&id!(root)),
                    Some(DockItem::Splitter { .. })
                ));
                assert!(dock.move_tab(&mut cx, id!(second), id!(first), merge));
                assert_intact(&dock, &bodies, id!(second));
                assert_eq!(dock.borrow().unwrap().dock_items.len(), 4);
                assert!(dock.check_and_clear_need_save());
            }
        }
    }

    #[test]
    fn moving_the_last_source_tab_remaps_a_target_promoted_to_root() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (dock, bodies) = dock_with_tabs(&mut cx);
        assert!(dock.move_tab(&mut cx, id!(first), id!(second), DropPart::Left));
        // Closing the first tab's source collapses the root splitter. The
        // target pane gets promoted to root before the new split is inserted.
        assert!(dock.move_tab(&mut cx, id!(first), id!(third), DropPart::Bottom));
        assert_intact(&dock, &bodies, id!(first));
        assert!(dock.move_tab(&mut cx, id!(third), id!(first), DropPart::Tab));
        assert!(dock.move_tab(&mut cx, id!(second), id!(first), DropPart::Center));
        assert_intact(&dock, &bodies, id!(second));
        let dock = dock.borrow().unwrap();
        match dock.dock_items.get(&id!(root)).unwrap() {
            DockItem::Tabs { tabs, selected, .. } => {
                assert_eq!(tabs, &[id!(third), id!(first), id!(second)]);
                assert_eq!(*selected, 2);
            }
            _ => panic!("all tabs should have merged back into the root pane"),
        }
    }

    #[test]
    fn invalid_identity_moves_leave_layout_and_dirty_state_unchanged() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (dock, bodies) = dock_with_tabs(&mut cx);
        let layout = format!("{:?}", dock.borrow().unwrap().dock_items);
        for part in [
            DropPart::Left,
            DropPart::Right,
            DropPart::Top,
            DropPart::Bottom,
            DropPart::Center,
            DropPart::TabBar,
            DropPart::Tab,
        ] {
            for (item, target) in [
                (id!(first), id!(first)),
                (id!(missing), id!(first)),
                (id!(first), id!(missing)),
                (id!(root), id!(first)),
                (id!(first), id!(root)),
            ] {
                assert!(!dock.move_tab(&mut cx, item, target, part));
                assert_eq!(format!("{:?}", dock.borrow().unwrap().dock_items), layout);
                assert!(!dock.check_and_clear_need_save());
                assert_intact(&dock, &bodies, id!(first));
            }
        }
    }

    fn initial_style_test_dock(vm: &mut ScriptVm) -> ScriptValue {
        crate::script_eval!(vm, {
            use mod.widgets.*
            Dock{
                root := DockSplitter{
                    axis: Horizontal align: SplitterAlign.FromA(216.0)
                    a: @project_tabs b: @work_tabs
                }
                project_tabs := DockTabs{tabs: [@first] selected: 0 closable: true}
                work_tabs := DockTabs{tabs: [@second @third] selected: 1 closable: true}
                first := DockTab{name: "first" template: @CloseableTab kind: @Body}
                second := DockTab{name: "second" template: @CloseableTab kind: @Body}
                third := DockTab{name: "third" template: @CloseableTab kind: @Body}
                Body := Label{text: "initial text"}
            }
        })
    }

    fn layout_fingerprint(dock: &DockRef) -> Vec<(u64, String)> {
        let mut layout: Vec<_> = dock
            .clone_state()
            .unwrap()
            .iter()
            .map(|(id, item)| (id.0, item.serialize_ron()))
            .collect();
        layout.sort_by_key(|(id, _)| *id);
        layout
    }

    #[test]
    fn style_reapply_keeps_removed_default_panes_and_tabs_removed() {
        use crate::desktop_style::{install, DesktopStyle, StyleSheet};

        let mut cx = Cx::new(Box::new(|_, _| {}));
        let dock = cx.with_vm(|vm| {
            crate::script_mod(vm);
            let value = initial_style_test_dock(vm);
            Dock::script_from_value(vm, value)
        });
        let dock = WidgetRef::new_with_inner(Box::new(dock)).as_dock();
        let first = dock.item(id!(first));
        first.set_text(&mut cx, "unsaved live content");
        assert!(dock.move_tab(&mut cx, id!(second), id!(first), DropPart::Center));
        assert!(dock.move_tab(&mut cx, id!(third), id!(first), DropPart::Center));
        // The merge removed both initial pane IDs by promoting their content
        // to root. A closed default tab must not be resurrected either.
        dock.close_tab(&mut cx, id!(second));
        let runtime = dock
            .create_and_select_tab(
                &mut cx,
                id!(root),
                id!(runtime_tab),
                id!(Body),
                "runtime".into(),
                id!(CloseableTab),
                None,
            )
            .unwrap();
        runtime.set_text(&mut cx, "runtime tab content");
        assert!(dock.move_tab(&mut cx, id!(runtime_tab), id!(first), DropPart::Right));
        let bodies = vec![
            (id!(first), first),
            (id!(third), dock.item(id!(third))),
            (id!(runtime_tab), runtime),
        ];
        let layout = layout_fingerprint(&dock);
        assert!(dock.check_and_clear_need_save());

        for (style, dark) in [
            (DesktopStyle::Windows, false),
            (DesktopStyle::Macos, true),
            (DesktopStyle::Macos, false),
        ] {
            cx.with_vm(|vm| {
                install(vm, StyleSheet::load_with_appearance(style, dark));
                vm.with_reload(crate::script_mod);
                let value = initial_style_test_dock(vm);
                dock.borrow_mut().unwrap().script_apply(
                    vm,
                    &Apply::ScriptReapply,
                    &mut Scope::empty(),
                    value,
                );
                assert!(vm.take_errors().is_empty());
            });
            assert_eq!(layout_fingerprint(&dock), layout, "{}", style.id());
            assert_intact(&dock, &bodies, id!(runtime_tab));
            assert!(!dock.check_and_clear_need_save());
            assert!(dock.item(id!(second)).is_empty());
            assert_eq!(dock.item(id!(first)).text(), "unsaved live content");
            assert_eq!(dock.item(id!(runtime_tab)).text(), "runtime tab content");
            let dock = dock.borrow().unwrap();
            assert!(!dock.dock_items.contains_key(&id!(project_tabs)));
            assert!(!dock.dock_items.contains_key(&id!(work_tabs)));
            assert!(!dock.dock_items.contains_key(&id!(second)));
            assert!(dock.templates.contains_key(&id!(Body)));
        }
    }

    #[test]
    fn preserving_layout_keeps_absent_and_matching_tab_bodies() {
        let present = LiveId(1);
        let absent = LiveId(2);
        let kind = LiveId(3);
        let mut layout = HashMap::new();
        layout.insert(present, DockItem::tab("present".into(), kind, LiveId(4)));

        assert!(preserve_item_for_layout(&layout, present, kind));
        assert!(preserve_item_for_layout(&layout, absent, kind));
        assert!(!preserve_item_for_layout(&layout, present, LiveId(5)));
    }

    #[test]
    fn preserving_layout_drops_a_tab_when_its_id_becomes_a_container() {
        let reused = LiveId(1);
        let mut layout = HashMap::new();
        layout.insert(reused, DockItem::tabs(Vec::new(), 0, false));

        assert!(!preserve_item_for_layout(&layout, reused, LiveId(2)));
    }
}
