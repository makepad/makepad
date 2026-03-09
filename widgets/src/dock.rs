use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_micro_serde::*,
    splitter::{Splitter, SplitterAction, SplitterAlign, SplitterAxis},
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
    #[rust]
    dock_item_iter_stack: Vec<(LiveId, usize)>,
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
        // Collect templates and dock items from the object's vec
        // Only collect during template applies (not eval) to avoid storing temporary objects
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
                                    let splitter =
                                        DockItemSplitter::script_from_value(vm, kv.value);
                                    self.dock_items.insert(id, splitter.to_dock_item());
                                } else if vm
                                    .bx
                                    .heap
                                    .type_matches_id(val_obj, DockItemTabs::script_type_id_static())
                                {
                                    let tabs = DockItemTabs::script_from_value(vm, kv.value);
                                    self.dock_items.insert(id, tabs.to_dock_item());
                                } else if vm
                                    .bx
                                    .heap
                                    .type_matches_id(val_obj, DockItemTab::script_type_id_static())
                                {
                                    let tab = DockItemTab::script_from_value(vm, kv.value);
                                    self.dock_items.insert(id, tab.to_dock_item());
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
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx)
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
}

impl DockItemTabs {
    pub fn to_dock_item(&self) -> DockItem {
        DockItem::Tabs {
            tabs: self.tabs.clone(),
            selected: self.selected,
            closable: self.closable,
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

    fn find_drop_position(&self, cx: &Cx, abs: Vec2d) -> Option<DropPosition> {
        for (tab_bar_id, tab_bar) in self.tab_bars.iter() {
            let rect = tab_bar.contents_rect;
            if let Some((tab_id, rect)) = tab_bar.tab_bar.is_over_tab(cx, abs) {
                return Some(DropPosition {
                    part: DropPart::Tab,
                    id: tab_id,
                    rect,
                });
            } else if let Some(rect) = tab_bar.tab_bar.is_over_tab_bar(cx, abs) {
                return Some(DropPosition {
                    part: DropPart::TabBar,
                    id: *tab_bar_id,
                    rect,
                });
            } else if rect.contains(abs) {
                let top_left = rect.pos;
                let bottom_right = rect.pos + rect.size;
                if (abs.x - top_left.x) / rect.size.x < 0.1 {
                    return Some(DropPosition {
                        part: DropPart::Left,
                        id: *tab_bar_id,
                        rect: Rect {
                            pos: rect.pos,
                            size: Vec2d {
                                x: rect.size.x / 2.0,
                                y: rect.size.y,
                            },
                        },
                    });
                } else if (bottom_right.x - abs.x) / rect.size.x < 0.1 {
                    return Some(DropPosition {
                        part: DropPart::Right,
                        id: *tab_bar_id,
                        rect: Rect {
                            pos: Vec2d {
                                x: rect.pos.x + rect.size.x / 2.0,
                                y: rect.pos.y,
                            },
                            size: Vec2d {
                                x: rect.size.x / 2.0,
                                y: rect.size.y,
                            },
                        },
                    });
                } else if (abs.y - top_left.y) / rect.size.y < 0.1 {
                    return Some(DropPosition {
                        part: DropPart::Top,
                        id: *tab_bar_id,
                        rect: Rect {
                            pos: rect.pos,
                            size: Vec2d {
                                x: rect.size.x,
                                y: rect.size.y / 2.0,
                            },
                        },
                    });
                } else if (bottom_right.y - abs.y) / rect.size.y < 0.1 {
                    return Some(DropPosition {
                        part: DropPart::Bottom,
                        id: *tab_bar_id,
                        rect: Rect {
                            pos: Vec2d {
                                x: rect.pos.x,
                                y: rect.pos.y + rect.size.y / 2.0,
                            },
                            size: Vec2d {
                                x: rect.size.x,
                                y: rect.size.y / 2.0,
                            },
                        },
                    });
                } else {
                    return Some(DropPosition {
                        part: DropPart::Center,
                        id: *tab_bar_id,
                        rect,
                    });
                }
            }
        }
        None
    }

    pub fn item(&self, entry_id: LiveId) -> Option<WidgetRef> {
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

    fn set_parent_split(&mut self, what_item: LiveId, replace_item: LiveId) {
        for item in self.dock_items.values_mut() {
            match item {
                DockItem::Splitter { a, b, .. } => {
                    if what_item == *a {
                        *a = replace_item;
                        return;
                    } else if what_item == *b {
                        *b = replace_item;
                        return;
                    }
                }
                _ => (),
            }
        }
    }

    fn redraw_item(&mut self, cx: &mut Cx, what_item_id: LiveId) {
        if let Some(tab_bar) = self.tab_bars.get_mut(&what_item_id) {
            tab_bar.contents_draw_list.redraw(cx);
        }
        for (item_id, (_kind, item)) in self.items.iter_mut() {
            if *item_id == what_item_id {
                item.redraw(cx);
            }
        }
    }

    fn unsplit_tabs(&mut self, cx: &mut Cx, tabs_id: LiveId) {
        self.needs_save = true;
        for (splitter_id, item) in self.dock_items.iter_mut() {
            match *item {
                DockItem::Splitter { a, b, .. } => {
                    let splitter_id = *splitter_id;
                    if tabs_id == a {
                        self.set_parent_split(splitter_id, b);
                        self.dock_items.remove(&splitter_id);
                        self.dock_items.remove(&tabs_id);
                        self.redraw_item(cx, b);
                        return;
                    } else if tabs_id == b {
                        self.set_parent_split(splitter_id, a);
                        self.dock_items.remove(&splitter_id);
                        self.dock_items.remove(&tabs_id);
                        self.redraw_item(cx, a);
                        return;
                    }
                }
                _ => (),
            }
        }
    }

    fn select_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
        self.needs_save = true;
        for (tabs_id, item) in self.dock_items.iter_mut() {
            match item {
                DockItem::Tabs { tabs, selected, .. } => {
                    if let Some(pos) = tabs.iter().position(|v| *v == tab_id) {
                        *selected = pos;
                        if let Some(tab_bar) = self.tab_bars.get(&tabs_id) {
                            tab_bar.contents_draw_list.redraw(cx);
                        }
                    }
                }
                _ => (),
            }
        }
    }

    fn set_tab_title(&mut self, cx: &mut Cx, tab_id: LiveId, new_name: String) {
        self.needs_save = true;
        if let Some(DockItem::Tab { name, .. }) = self.dock_items.get_mut(&tab_id) {
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

    fn find_tab_bar_of_tab(&mut self, tab_id: LiveId) -> Option<(LiveId, usize)> {
        for (tabs_id, item) in self.dock_items.iter_mut() {
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

    fn check_drop_is_noop(&mut self, tab_id: LiveId, item_id: LiveId) -> bool {
        for (tabs_id, item) in self.dock_items.iter_mut() {
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
        if let Some(pos) = self.find_drop_position(cx, abs) {
            self.needs_save = true;
            match pos.part {
                DropPart::Left | DropPart::Right | DropPart::Top | DropPart::Bottom => {
                    if is_move {
                        if self.check_drop_is_noop(item, pos.id) {
                            return false;
                        }
                        self.close_tab(cx, item, true);
                    }
                    let new_tabs = self.unique_id(self.dock_items.len() as u64);
                    self.dock_items.insert(
                        new_tabs,
                        DockItem::Tabs {
                            tabs: vec![item],
                            closable: true,
                            selected: 0,
                        },
                    );
                    let new_split = self.unique_id(self.dock_items.len() as u64);
                    self.set_parent_split(pos.id, new_split);
                    self.dock_items.insert(
                        new_split,
                        match pos.part {
                            DropPart::Left => DockItem::Splitter {
                                axis: SplitterAxis::Horizontal,
                                align: SplitterAlign::Weighted(0.5),
                                a: new_tabs,
                                b: pos.id,
                            },
                            DropPart::Right => DockItem::Splitter {
                                axis: SplitterAxis::Horizontal,
                                align: SplitterAlign::Weighted(0.5),
                                a: pos.id,
                                b: new_tabs,
                            },
                            DropPart::Top => DockItem::Splitter {
                                axis: SplitterAxis::Vertical,
                                align: SplitterAlign::Weighted(0.5),
                                a: new_tabs,
                                b: pos.id,
                            },
                            DropPart::Bottom => DockItem::Splitter {
                                axis: SplitterAxis::Vertical,
                                align: SplitterAlign::Weighted(0.5),
                                a: pos.id,
                                b: new_tabs,
                            },
                            _ => panic!(),
                        },
                    );

                    return true;
                }
                DropPart::Center => {
                    if is_move {
                        if self.check_drop_is_noop(item, pos.id) {
                            return false;
                        }
                        self.close_tab(cx, item, true);
                    }
                    if let Some(DockItem::Tabs { tabs, selected, .. }) =
                        self.dock_items.get_mut(&pos.id)
                    {
                        tabs.push(item);
                        *selected = tabs.len() - 1;
                        if let Some(tab_bar) = self.tab_bars.get(&pos.id) {
                            tab_bar.contents_draw_list.redraw(cx);
                        }
                    }
                    return true;
                }
                DropPart::TabBar => {
                    if is_move {
                        if self.check_drop_is_noop(item, pos.id) {
                            return false;
                        }
                        self.close_tab(cx, item, true);
                    }
                    if let Some(DockItem::Tabs { tabs, selected, .. }) =
                        self.dock_items.get_mut(&pos.id)
                    {
                        tabs.push(item);
                        *selected = tabs.len() - 1;
                        if let Some(tab_bar) = self.tab_bars.get(&pos.id) {
                            tab_bar.contents_draw_list.redraw(cx);
                        }
                    }
                    return true;
                }
                DropPart::Tab => {
                    if is_move {
                        if pos.id == item {
                            return false;
                        }
                        self.close_tab(cx, item, true);
                    }
                    let (tab_bar_id, pos) = self.find_tab_bar_of_tab(pos.id).unwrap();
                    if let Some(DockItem::Tabs { tabs, selected, .. }) =
                        self.dock_items.get_mut(&tab_bar_id)
                    {
                        let old = tabs[pos];
                        tabs[pos] = item;
                        tabs.push(old);
                        *selected = pos;
                        if let Some(tab_bar) = self.tab_bars.get(&tab_bar_id) {
                            tab_bar.contents_draw_list.redraw(cx);
                        }
                    }
                    return true;
                }
            }
        }
        false
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
        self.area.redraw(cx);
        self.create_all_items(cx);
    }
}

impl Widget for Dock {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
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

        if visible_items_only {
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
            self.drop_target_draw_list.redraw(cx);
        }

        match event.drag_hits(cx, self.area) {
            DragHit::Drag(f) => {
                self.drop_state = None;
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
                self.drop_target_draw_list.redraw(cx);
                cx.widget_action(uid, DockAction::Drop(f.clone()))
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
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
                    if let Some(DockItem::Tabs { selected, .. }) = self.dock_items.get(&id) {
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
                        let walk = tab_bar.tab_bar.walk(cx);
                        tab_bar.tab_bar.begin(cx, Some(*selected), walk);
                        stack.push(DrawStackItem::TabLabel { id, index: 0 });
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
                            tab_bar.tab_bar.end(cx);
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
        if let Some(mut dock) = self.borrow_mut() {
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

    pub fn tab_start_drag(&self, cx: &mut Cx, _tab_id: LiveId, item: DragItem) {
        cx.start_dragging(vec![item]);
    }
}
