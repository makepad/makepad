use {
    crate::{
        makepad_derive_widget::*,
        widget::*,
        makepad_draw::*,
        splitter::{SplitterAction, Splitter, SplitterAlign},
        tab_bar::{TabBarAction, TabBar},
    },
};

live_design!{
    import makepad_draw::shader::std::*;
    import crate::tab_bar::TabBar
    import makepad_widgets::splitter::Splitter
    import makepad_widgets::theme::*;
    
    DrawRoundCorner = {{DrawRoundCorner}} {
        draw_depth: 6.0
        border_radius: 10.0
        fn pixel(self) -> vec4 {
            
            let pos = vec2(
                mix(self.pos.x, 1.0 - self.pos.x, self.flip.x),
                mix(self.pos.y, 1.0 - self.pos.y, self.flip.y)
            )
            
            let sdf = Sdf2d::viewport(pos * self.rect_size);
            sdf.rect(-10., -10., self.rect_size.x * 2.0, self.rect_size.y * 2.0);
            sdf.box(
                0.25,
                0.25,
                self.rect_size.x * 2.0,
                self.rect_size.y * 2.0,
                4.0
            );
            
            sdf.subtract()
            return sdf.fill(COLOR_BG_APP);
        }
    }
    
    const BORDER_SIZE: 6.0
    
    Dock = {{Dock}} {
        border_size: (BORDER_SIZE)
        layout: {
            flow: Down
            padding: {left: (BORDER_SIZE), top: 0.0, right: (BORDER_SIZE), bottom: (BORDER_SIZE)}
        }
        padding_fill: {color: (COLOR_BG_APP)}
        drag_quad: {
            draw_depth: 10.0
            color: (COLOR_DRAG_QUAD)
        }
        tab_bar: <TabBar> {}
        splitter: <Splitter> {}
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawRoundCorner {
    #[deref] draw_super: DrawQuad,
    #[live] border_radius: f32,
    #[live] flip: Vec2,
}

impl DrawRoundCorner {
    fn draw_corners(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.flip = vec2(0.0, 0.0);
        let rad = dvec2(self.border_radius as f64, self.border_radius as f64);
        let pos = rect.pos;
        let size = rect.size;
        self.draw_abs(cx, Rect {pos, size: rad});
        self.flip = vec2(1.0, 0.0);
        self.draw_abs(cx, Rect {pos: pos + dvec2(size.x - rad.x, 0.), size: rad});
        self.flip = vec2(1.0, 1.0);
        self.draw_abs(cx, Rect {pos: pos + dvec2(size.x - rad.x, size.y - rad.y), size: rad});
        self.flip = vec2(0.0, 1.0);
        self.draw_abs(cx, Rect {pos: pos + dvec2(0., size.y - rad.y), size: rad});
    }
}

#[derive(Live)]
pub struct Dock {
    #[rust] draw_state: DrawStateWrap<Vec<DrawStackItem >>,
    #[live] walk: Walk,
    #[live] layout: Layout,
    #[live] drop_target_draw_list: DrawList2d,
    #[live] round_corner: DrawRoundCorner,
    #[live] padding_fill: DrawColor,
    #[live] border_size: f64,
    #[live] drag_quad: DrawColor,
    
    #[live] tab_bar: Option<LivePtr>,
    #[live] splitter: Option<LivePtr>,
    
    #[rust] area: Area,
    
    #[rust] tab_bars: ComponentMap<LiveId, TabBarWrap>,
    #[rust] splitters: ComponentMap<LiveId, Splitter>,
    
    #[rust] dock_items: ComponentMap<LiveId, DockItem>,
    #[rust] templates: ComponentMap<LiveId, LivePtr>,
    #[rust] items: ComponentMap<DockItemId, WidgetRef>,
    #[rust] drop_state: Option<DropPosition>,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub struct DockItemId {
    pub kind: LiveId,
    pub id: LiveId
}


struct TabBarWrap {
    tab_bar: TabBar,
    contents_draw_list: DrawList2d,
    contents_rect: Rect
}

#[derive(Copy, Debug, Clone)]
enum DrawStackItem {
    Invalid,
    SplitLeft {id: LiveId},
    SplitRight {id: LiveId},
    SplitEnd {id: LiveId},
    Tabs {id: LiveId},
    TabLabel {id: LiveId, index: usize},
    Tab {id: LiveId},
    TabContent {id: LiveId}
}

impl DrawStackItem {
    fn from_dock_item(id: LiveId, dock_item: Option<&DockItem>) -> Self {
        match dock_item {
            None => DrawStackItem::Invalid,
            Some(DockItem::Splitter {..}) => {
                DrawStackItem::SplitLeft {id}
            }
            Some(DockItem::Tabs {..}) => {
                DrawStackItem::Tabs {id}
            }
            Some(DockItem::Tab {..}) => {
                DrawStackItem::Tab {id}
            }
        }
    }
}

#[derive(Clone, WidgetAction)]
pub enum DockAction {
    SplitPanelChanged {panel_id: LiveId, axis: Axis, align: SplitterAlign},
    TabWasPressed(LiveId),
    TabCloseWasPressed(LiveId),
    ShouldTabStartDrag(LiveId),
    Drag(DragHitEvent),
    Drop(DropHitEvent),
    None
}


#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropPosition {
    part: DropPart,
    rect: Rect,
    id: LiveId
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DropPart {
    Left,
    Right,
    Top,
    Bottom,
    Center,
    TabBar,
    Tab
}

#[derive(Clone, Debug, Live, LiveHook)]
#[live_ignore]
pub enum DockItem {
    #[live {axis: Axis::Vertical, align: SplitterAlign::Weighted(0.5), a: LiveId(0), b: LiveId(0)}]
    Splitter {
        axis: Axis,
        align: SplitterAlign,
        a: LiveId,
        b: LiveId
    },
    #[live {tabs: vec![], selected: 0, no_close:false}]
    Tabs {
        tabs: Vec<LiveId>,
        selected: usize,
        no_close: bool
    },
    #[pick {name: "Tab".to_string(), kind: LiveId(0), no_close: false}]
    Tab {
        name: String,
        no_close: bool,
        kind: LiveId
    }
}

impl LiveHook for Dock {
    fn apply_value_instance(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        let id = nodes[index].id;
        match from {
            ApplyFrom::NewFromDoc {file_id} | ApplyFrom::UpdateFromDoc {file_id} => {
                if nodes[index].origin.has_prop_type(LivePropType::Instance) {
                    if nodes[index].value.is_enum() {
                        let mut dock_item = DockItem::new(cx);
                        let index = dock_item.apply(cx, from, index, nodes);
                        self.dock_items.insert(id, dock_item);
                        
                        return index;
                    }
                    else {
                        let live_ptr = cx.live_registry.borrow().file_id_index_to_live_ptr(file_id, index);
                        self.templates.insert(id, live_ptr);
                        // lets apply this thing over all our childnodes with that template
                        for (DockItemId {kind, ..}, node) in self.items.iter_mut() {
                            if *kind == id {
                                node.apply(cx, from, index, nodes);
                            }
                        }
                    }
                }
                else {
                    cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                }
            }
            _ => ()
        }
        nodes.skip_node(index)
    }
    
    // alright lets update our tabs and splitters as well
    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        if let Some(index) = nodes.child_by_name(index, live_id!(tab_bar).as_field()) {
            for tab_bar in self.tab_bars.values_mut() {
                tab_bar.tab_bar.apply(cx, from, index, nodes);
            }
        }
        if let Some(index) = nodes.child_by_name(index, live_id!(splitter).as_field()) {
            for splitter in self.splitters.values_mut() {
                splitter.apply(cx, from, index, nodes);
            }
        }
    }    
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        // make sure our items exist
        let mut items = Vec::new();
        for (item_id, item) in self.dock_items.iter() {
            if let DockItem::Tab {kind, ..} = item {
                items.push((*item_id, *kind));
            }
        }
        for (item_id, kind) in items {
            self.get_item(cx, item_id, kind);
        }
    }
    
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, Dock)
    }
}

impl Dock {
    
    fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        cx.begin_box(walk, self.layout);
        //self.drop_zones.clear();
    }
    
    fn end(&mut self, cx: &mut Cx2d) {
        
        if self.drop_target_draw_list.begin(cx, Walk::default()).is_redrawing() {
            if let Some(pos) = &self.drop_state {
                self.drag_quad.draw_abs(cx, pos.rect);
            }
            self.drop_target_draw_list.end(cx);
        }
        
        self.tab_bars.retain_visible();
        self.splitters.retain_visible();
        
        // lets draw the corners here
        for splitter in self.splitters.values() {
            self.round_corner.draw_corners(cx, splitter.area_a().get_rect(cx));
            self.round_corner.draw_corners(cx, splitter.area_b().get_rect(cx));
        }
        self.round_corner.draw_corners(cx, cx.r#box().rect());
        
        cx.end_box_with_area(&mut self.area);
    }
    
    fn find_drop_position(&self, cx: &Cx, abs: DVec2) -> Option<DropPosition> {
        for (tab_bar_id, tab_bar) in self.tab_bars.iter() {
            let rect = tab_bar.contents_rect;
            if let Some((tab_id, rect)) = tab_bar.tab_bar.is_over_tab(cx, abs) {
                return Some(DropPosition {
                    part: DropPart::Tab,
                    id: tab_id,
                    rect
                })
            }
            else if let Some(rect) = tab_bar.tab_bar.is_over_tab_bar(cx, abs) {
                return Some(DropPosition {
                    part: DropPart::TabBar,
                    id: *tab_bar_id,
                    rect
                })
            }
            else if rect.contains(abs) {
                let top_left = rect.pos;
                let bottom_right = rect.pos + rect.size;
                if (abs.x - top_left.x) / rect.size.x < 0.1 {
                    return Some(DropPosition {
                        part: DropPart::Left,
                        id: *tab_bar_id,
                        rect: Rect {
                            pos: rect.pos,
                            size: DVec2 {
                                x: rect.size.x / 2.0,
                                y: rect.size.y,
                            },
                        }
                    })
                } else if (bottom_right.x - abs.x) / rect.size.x < 0.1 {
                    return Some(DropPosition {
                        part: DropPart::Right,
                        id: *tab_bar_id,
                        rect: Rect {
                            pos: DVec2 {
                                x: rect.pos.x + rect.size.x / 2.0,
                                y: rect.pos.y,
                            },
                            size: DVec2 {
                                x: rect.size.x / 2.0,
                                y: rect.size.y,
                            },
                        }
                    })
                } else if (abs.y - top_left.y) / rect.size.y < 0.1 {
                    return Some(DropPosition {
                        part: DropPart::Top,
                        id: *tab_bar_id,
                        rect: Rect {
                            pos: rect.pos,
                            size: DVec2 {
                                x: rect.size.x,
                                y: rect.size.y / 2.0,
                            },
                        }
                    })
                } else if (bottom_right.y - abs.y) / rect.size.y < 0.1 {
                    return Some(DropPosition {
                        part: DropPart::Bottom,
                        id: *tab_bar_id,
                        rect: Rect {
                            pos: DVec2 {
                                x: rect.pos.x,
                                y: rect.pos.y + rect.size.y / 2.0,
                            },
                            size: DVec2 {
                                x: rect.size.x,
                                y: rect.size.y / 2.0,
                            },
                        }
                    })
                } else {
                    return Some(DropPosition {
                        part: DropPart::Center,
                        id: *tab_bar_id,
                        rect
                    })
                }
            }
        }
        None
    }
    
    
    
    
    pub fn get_item(&mut self, cx: &mut Cx, entry_id: LiveId, template: LiveId) -> Option<WidgetRef> {
        if let Some(ptr) = self.templates.get(&template) {
            let entry = self.items.get_or_insert(cx, DockItemId {id: entry_id, kind: template}, | cx | {
                WidgetRef::new_from_ptr(cx, Some(*ptr))
            });
            return Some(entry.clone())
        }
        else{
            log!("ListView template not found {}", template);
        }
        None
    }
    
    pub fn items(&mut self) -> &ComponentMap<DockItemId, WidgetRef> {
        &self.items
    }
    
    fn set_parent_split(&mut self, what_item: LiveId, replace_item: LiveId) {
        for item in self.dock_items.values_mut() {
            match item {
                DockItem::Splitter {a, b, ..} => {
                    if what_item == *a {
                        *a = replace_item;
                        return
                    }
                    else if what_item == *b {
                        *b = replace_item;
                        return
                    }
                }
                _ => ()
            }
        }
    }
    
    fn redraw_item(&mut self, cx: &mut Cx, what_item_id: LiveId) {
        if let Some(tab_bar) = self.tab_bars.get_mut(&what_item_id) {
            tab_bar.contents_draw_list.redraw(cx);
        }
        for (item_id, item) in self.items.iter_mut() {
            if item_id.id == what_item_id {
                item.redraw(cx);
            }
        }
    }
    
    fn unsplit_tabs(&mut self, cx: &mut Cx, tabs_id: LiveId) {
        for (splitter_id, item) in self.dock_items.iter_mut() {
            match *item {
                DockItem::Splitter {a, b, ..} => {
                    let splitter_id = *splitter_id;
                    if tabs_id == a {
                        self.set_parent_split(splitter_id, b);
                        self.dock_items.remove(&splitter_id);
                        self.dock_items.remove(&tabs_id);
                        self.redraw_item(cx, b);
                        return
                    }
                    else if tabs_id == b {
                        self.set_parent_split(splitter_id, a);
                        self.dock_items.remove(&splitter_id);
                        self.dock_items.remove(&tabs_id);
                        self.redraw_item(cx, a);
                        return
                    }
                }
                _ => ()
            }
        }
    }
    
    fn select_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
        for (tabs_id, item) in self.dock_items.iter_mut() {
            match item {
                DockItem::Tabs {tabs, selected,..} => if let Some(pos) = tabs.iter().position( | v | *v == tab_id) {
                    *selected = pos;
                    // ok now lets redraw the area of the tab
                    if let Some(tab_bar) = self.tab_bars.get(&tabs_id) {
                        tab_bar.contents_draw_list.redraw(cx);
                    }
                }
                _ => ()
            }
        }
    }
    
    fn redraw_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
        for (tabs_id, item) in self.dock_items.iter_mut() {
            match item {
                DockItem::Tabs {tabs, ..} => if let Some(_) = tabs.iter().position( | v | *v == tab_id) {
                    if let Some(tab_bar) = self.tab_bars.get(&tabs_id) {
                        tab_bar.contents_draw_list.redraw(cx);
                    }
                }
                _ => ()
            }
        }
    }
    
    fn find_tab_bar_from_tab(&mut self, tab_id: LiveId) -> Option<LiveId> {
        for (tabs_id, item) in self.dock_items.iter_mut() {
            match item {
                DockItem::Tabs {tabs, ..} => if let Some(_) = tabs.iter().position( | v | *v == tab_id) {
                    return Some(*tabs_id)
                }
                _ => ()
            }
        }
        None
    }
    
    fn close_tab(&mut self, cx: &mut Cx, tab_id: LiveId, keep_item: bool) -> Option<LiveId> {
        // ok so we have to find the tab id in our tab bars / tabs and remove it
        // if we are the last tab we need to remove a splitter
        for (tabs_id, item) in self.dock_items.iter_mut() {
            match item {
                DockItem::Tabs {tabs, selected, no_close} => if let Some(pos) = tabs.iter().position( | v | *v == tab_id) {
                    // remove from the tabs array
                    let tabs_id = *tabs_id;
                    tabs.remove(pos);
                    if tabs.len() == 0 { // unsplit
                        if !*no_close{
                            self.unsplit_tabs(cx, tabs_id);
                        }
                        self.area.redraw(cx);
                        return None
                    }
                    else {
                        let next_tab = if *selected >= tabs.len() {tabs[*selected - 1]} else {tabs[*selected]};
                        self.select_tab(cx, next_tab);
                        if !keep_item {
                            self.dock_items.remove(&tab_id);
                        }
                        self.area.redraw(cx);
                        return Some(tabs_id)
                    }
                }
                _ => ()
            }
        }
        None
    }
    
    fn check_drop_is_noop(&mut self, tab_id: LiveId, item_id: LiveId) -> bool {
        // ok so we have to find the tab id in our tab bars / tabs and remove it
        // if we are the last tab we need to remove a splitter
        for (tabs_id, item) in self.dock_items.iter_mut() {
            match item {
                DockItem::Tabs {tabs, ..} => if let Some(_) = tabs.iter().position( | v | *v == tab_id) {
                    if *tabs_id == item_id && tabs.len() == 1 {
                        return true
                    }
                }
                _ => ()
            }
        }
        false
    }
    
    fn handle_drop(&mut self, cx: &mut Cx, abs: DVec2, item: LiveId, is_move: bool) -> bool {
        if let Some(pos) = self.find_drop_position(cx, abs) {
            // ok now what
            // we have a pos
            match pos.part {
                DropPart::Left | DropPart::Right | DropPart::Top | DropPart::Bottom => {
                    if is_move {
                        if self.check_drop_is_noop(item, pos.id) {
                            return false
                        }
                        self.close_tab(cx, item, true);
                    }
                    let new_split = LiveId::unique();
                    let new_tabs = LiveId::unique();
                    self.set_parent_split(pos.id, new_split);
                    self.dock_items.insert(new_split, match pos.part {
                        DropPart::Left => DockItem::Splitter {
                            axis: Axis::Horizontal,
                            align: SplitterAlign::Weighted(0.5),
                            a: new_tabs,
                            b: pos.id,
                        },
                        DropPart::Right => DockItem::Splitter {
                            axis: Axis::Horizontal,
                            align: SplitterAlign::Weighted(0.5),
                            a: pos.id,
                            b: new_tabs
                        },
                        DropPart::Top => DockItem::Splitter {
                            axis: Axis::Vertical,
                            align: SplitterAlign::Weighted(0.5),
                            a: new_tabs,
                            b: pos.id,
                        },
                        DropPart::Bottom => DockItem::Splitter {
                            axis: Axis::Vertical,
                            align: SplitterAlign::Weighted(0.5),
                            a: pos.id,
                            b: new_tabs,
                        },
                        _ => panic!()
                    });
                    self.dock_items.insert(new_tabs, DockItem::Tabs {
                        tabs: vec![item],
                        no_close: false,
                        selected: 0,
                    });
                    return true
                }
                DropPart::Center => {
                    if is_move {
                        if self.check_drop_is_noop(item, pos.id) {
                            return false
                        }
                        self.close_tab(cx, item, true);
                    }
                    if let Some(DockItem::Tabs {tabs, selected,..}) = self.dock_items.get_mut(&pos.id) {
                        tabs.push(item);
                        *selected = tabs.len() - 1;
                        if let Some(tab_bar) = self.tab_bars.get(&pos.id) {
                            tab_bar.contents_draw_list.redraw(cx);
                        }
                    }
                    return true
                }
                DropPart::TabBar => {
                    if is_move {
                        if self.check_drop_is_noop(item, pos.id) {
                            return false
                        }
                        self.close_tab(cx, item, true);
                    }
                    if let Some(DockItem::Tabs {tabs, selected,..}) = self.dock_items.get_mut(&pos.id) {
                        tabs.push(item);
                        *selected = tabs.len() - 1;
                        if let Some(tab_bar) = self.tab_bars.get(&pos.id) {
                            tab_bar.contents_draw_list.redraw(cx);
                        }
                    }
                    return true
                }
                // insert the ta
                DropPart::Tab => {
                    if is_move {
                        if pos.id == item {
                            return false
                        }
                        self.close_tab(cx, item, true);
                    }
                    let tab_bar_id = self.find_tab_bar_from_tab(pos.id).unwrap();
                    if let Some(DockItem::Tabs {tabs, selected,..}) = self.dock_items.get_mut(&tab_bar_id) {
                        if let Some(pos) = tabs.iter().position( | v | *v == pos.id) {
                            let old = tabs[pos];
                            tabs[pos] = item;
                            tabs.push(old);
                            *selected = pos;
                            if let Some(tab_bar) = self.tab_bars.get(&tab_bar_id) {
                                tab_bar.contents_draw_list.redraw(cx);
                            }
                        }
                    }
                    return true
                }
            }
        }
        false
    }
    
    fn drop_create(&mut self, cx: &mut Cx, abs: DVec2, item: LiveId, kind: LiveId, name: String) {
        // lets add a tab
        if self.handle_drop(cx, abs, item, false) {
            self.dock_items.insert(item, DockItem::Tab {
                name,
                no_close: false,
                kind
            });
            self.get_item(cx, item, kind);
            self.select_tab(cx, item);
            self.area.redraw(cx);
        }
    }
    
    fn drop_clone(&mut self, cx: &mut Cx, abs: DVec2, item: LiveId, new_item: LiveId) {
        // lets add a tab
        if let Some(DockItem::Tab {name, kind, ..}) = self.dock_items.get(&item) {
            let name = name.clone();
            let kind = kind.clone();
            if self.handle_drop(cx, abs, new_item, false) {
                self.dock_items.insert(new_item, DockItem::Tab {
                    name,
                    no_close: false,
                    kind
                });
                self.get_item(cx, new_item, kind);
                self.select_tab(cx, new_item);
            }
        }
    }
    
    
    fn create_tab(&mut self, cx: &mut Cx, parent: LiveId, item: LiveId, kind: LiveId, name: String) {
        if let Some(DockItem::Tabs {tabs, ..}) = self.dock_items.get_mut(&parent) {
            tabs.push(item);
            self.dock_items.insert(item, DockItem::Tab {
                name,
                no_close: false,
                kind
            });
            self.select_tab(cx, item);
            self.get_item(cx, item, kind);
        }
    }
    
    pub fn get_drawing_item_id(&self) -> Option<LiveId> {
        if let Some(stack) = self.draw_state.as_ref() {
            match stack.last() {
                Some(DrawStackItem::Tab {id}) => {
                    return Some(*id)
                }
                _ => ()
            }
        }
        None
    }
    
}


impl Widget for Dock {
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
    
    fn handle_widget_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        // call handle on all tab bars, splitters,
        let uid = self.widget_uid();
        let dock_items = &mut self.dock_items;
        for (panel_id, splitter) in self.splitters.iter_mut() {
            splitter
                .handle_event_with(cx, event, &mut | cx, action | match action {
                SplitterAction::Changed {axis, align} => {
                    // lets move the splitter
                    if let Some(DockItem::Splitter {axis: _axis, align: _align, ..}) = dock_items.get_mut(&panel_id) {
                        *_axis = axis;
                        *_align = align;
                    }
                    dispatch_action(cx, DockAction::SplitPanelChanged {panel_id: *panel_id, axis, align}.into_action(uid));
                },
                _ => ()
            });
        }
        for (panel_id, tab_bar) in self.tab_bars.iter_mut() {
            let contents_view = &mut tab_bar.contents_draw_list;
            for action in tab_bar.tab_bar.handle_event(cx, event) {
                match action {
                    TabBarAction::ShouldTabStartDrag(item) => dispatch_action(
                        cx,
                        DockAction::ShouldTabStartDrag(item).into_action(uid),
                    ),
                    TabBarAction::TabWasPressed(tab_id) => {
                        if let Some(DockItem::Tabs {tabs, selected, ..}) = dock_items.get_mut(&panel_id) {
                            if let Some(sel) = tabs.iter().position( | v | *v == tab_id) {
                                *selected = sel;
                                contents_view.redraw(cx);
                                dispatch_action(cx, DockAction::TabWasPressed(tab_id).into_action(uid))
                            }
                            else {
                                log!("Cannot find tab {}", tab_id.0);
                            }
                        }
                    }
                    TabBarAction::TabCloseWasPressed(tab_id) => {
                        dispatch_action(cx, DockAction::TabCloseWasPressed(tab_id).into_action(uid))
                    }
                }
            };
        }
        for item in self.items.values_mut() {
            item.handle_widget_event_with(cx, event, dispatch_action);
        }
        
        if let Event::DragEnd = event {
            // end our possible dragstate
            self.drop_state = None;
            self.drop_target_draw_list.redraw(cx);
        }
        
        // alright lets manage the drag areas
        match event.drag_hits(cx, self.area) {
            DragHit::Drag(f) => {
                self.drop_state = None;
                self.drop_target_draw_list.redraw(cx);
                match f.state {
                    DragState::In | DragState::Over => {
                        dispatch_action(cx, DockAction::Drag(f.clone()).into_action(uid))
                    }
                    DragState::Out => {}
                }
            }
            DragHit::Drop(f) => {
                self.drop_state = None;
                self.drop_target_draw_list.redraw(cx);
                dispatch_action(cx, DockAction::Drop(f.clone()).into_action(uid))
            }
            _ => {}
        }
        
    }
    
    fn find_widgets(&mut self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        if let Some(DockItem::Tab {kind, ..}) = self.dock_items.get(&path[0]) {
            if let Some(widget) = self.items.get_mut(&DockItemId {id: path[0], kind: *kind}) {
                if path.len()>1 {
                    widget.find_widgets(&path[1..], cached, results);
                }
                else {
                    results.push(widget.clone());
                }
            }
        }
        else {
            for widget in self.items.values_mut() {
                widget.find_widgets(path, cached, results);
            }
        }
    }
    
    fn get_walk(&self) -> Walk {self.walk}
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if self.draw_state.begin_with(cx, &self.dock_items, | _, dock_items | {
            let id = live_id!(root);
            vec![DrawStackItem::from_dock_item(id, dock_items.get(&id))]
        }) {
            self.begin(cx, walk);
        }
        
        while let Some(stack) = self.draw_state.as_mut() {
            match stack.pop() {
                Some(DrawStackItem::SplitLeft {id}) => {
                    stack.push(DrawStackItem::SplitRight {id});
                    // top becomes splitleft
                    let splitter = self.splitter;
                    let splitter = self.splitters.get_or_insert(cx, id, | cx | {
                        Splitter::new_from_ptr(cx, splitter)
                    });
                    if let Some(DockItem::Splitter {axis, align, a, ..}) = self.dock_items.get(&id) {
                        splitter.set_axis(*axis);
                        splitter.set_align(*align);
                        splitter.begin(cx, Walk::default());
                        stack.push(DrawStackItem::from_dock_item(*a, self.dock_items.get(&a)));
                        continue;
                    }
                    else {panic!()}
                }
                Some(DrawStackItem::SplitRight {id}) => {
                    // lets create the 4 split dropzones of this splitter
                    stack.push(DrawStackItem::SplitEnd {id});
                    let splitter = self.splitters.get_mut(&id).unwrap();
                    splitter.middle(cx);
                    if let Some(DockItem::Splitter {b, ..}) = self.dock_items.get(&id) {
                        stack.push(DrawStackItem::from_dock_item(*b, self.dock_items.get(&b)));
                        continue;
                    }
                    else {panic!()}
                }
                Some(DrawStackItem::SplitEnd {id}) => {
                    // 4 more dropzones
                    let splitter = self.splitters.get_mut(&id).unwrap();
                    splitter.end(cx);
                }
                Some(DrawStackItem::Tabs {id}) => {
                    if let Some(DockItem::Tabs {selected, ..}) = self.dock_items.get(&id) {
                        // lets draw the tabs
                        let tab_bar = self.tab_bar;
                        let tab_bar = self.tab_bars.get_or_insert(cx, id, | cx | {
                            TabBarWrap {
                                tab_bar: TabBar::new_from_ptr(cx, tab_bar),
                                contents_draw_list: DrawList2d::new(cx),
                                contents_rect: Rect::default(),
                                //full_rect: Rect::default(),
                            }
                        });
                        tab_bar.tab_bar.begin(cx, Some(*selected));
                        stack.push(DrawStackItem::TabLabel {id, index: 0});
                    }
                    else {panic!()}
                }
                Some(DrawStackItem::TabLabel {id, index}) => {
                    if let Some(DockItem::Tabs {tabs, selected,..}) = self.dock_items.get(&id) {
                        let tab_bar = self.tab_bars.get_mut(&id).unwrap();
                        if index < tabs.len() {
                            if let Some(DockItem::Tab {name, ..}) = self.dock_items.get(&tabs[index]) {
                                tab_bar.tab_bar.draw_tab(cx, tabs[index].into(), name);
                            }
                            stack.push(DrawStackItem::TabLabel {id, index: index + 1});
                        }
                        else {
                            tab_bar.tab_bar.end(cx);
                            tab_bar.contents_rect = cx.r#box().rect();
                            if tabs.len()>0 && tab_bar.contents_draw_list.begin(cx, Walk::default()).is_redrawing() {
                                stack.push(DrawStackItem::TabContent {id});
                                stack.push(DrawStackItem::Tab {id: tabs[*selected]});
                            }
                        }
                    }
                    else {panic!()}
                }
                Some(DrawStackItem::Tab {id}) => {
                    stack.push(DrawStackItem::Tab {id});
                    if let Some(DockItem::Tab {kind, ..}) = self.dock_items.get(&id) {
                        if let Some(ptr) = self.templates.get(&kind) {
                            let entry = self.items.get_or_insert(cx, DockItemId {id, kind: *kind}, | cx | {
                                WidgetRef::new_from_ptr(cx, Some(*ptr))
                            });
                            entry.draw_widget(cx) ?;
                        }
                    }
                    stack.pop();
                }
                Some(DrawStackItem::TabContent {id}) => {
                    if let Some(DockItem::Tabs {..}) = self.dock_items.get(&id) {
                        let tab_bar = self.tab_bars.get_mut(&id).unwrap();
                        // lets create the full dropzone for this contentview
                        
                        tab_bar.contents_draw_list.end(cx);
                    }
                    else {panic!()}
                }
                Some(DrawStackItem::Invalid) => {}
                None => {
                    break
                }
            }
        }
        
        self.end(cx);
        self.draw_state.end();
        
        WidgetDraw::done()
    }
}

#[derive(Clone, Debug, PartialEq, WidgetRef)]
pub struct DockRef(WidgetRef);

impl DockRef {
    pub fn close_tab(&self, cx: &mut Cx, tab_id: LiveId) {
        if let Some(mut dock) = self.borrow_mut() {
            dock.close_tab(cx, tab_id, false);
        }
    }
    
    pub fn clicked_tab_close(&self, actions: &WidgetActions) -> Option<LiveId> {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let DockAction::TabCloseWasPressed(tab_id) = item.action() {
                return Some(tab_id)
            }
        }
        None
    }
    
    pub fn should_tab_start_drag(&self, actions: &WidgetActions) -> Option<LiveId> {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let DockAction::ShouldTabStartDrag(tab_id) = item.action() {
                return Some(tab_id)
            }
        }
        None
    }
    
    pub fn should_accept_drag(&self, actions: &WidgetActions) -> Option<DragHitEvent> {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let DockAction::Drag(drag_event) = item.action() {
                return Some(drag_event.clone())
            }
        }
        None
    }
    
    pub fn has_drop(&self, actions: &WidgetActions) -> Option<DropHitEvent> {
        if let Some(item) = actions.find_single_action(self.widget_uid()) {
            if let DockAction::Drop(drag_event) = item.action() {
                return Some(drag_event.clone())
            }
        }
        None
    }
    
    
    pub fn accept_drag(&self, cx: &mut Cx, dh: DragHitEvent, dr: DragResponse) {
        if let Some(mut dock) = self.borrow_mut() {
            if let Some(pos) = dock.find_drop_position(cx, dh.abs) {
                dh.response.set(dr);
                dock.drop_state = Some(pos);
            }
            else {
                dock.drop_state = None;
            }
        }
    }
    
    pub fn get_drawing_item_id(&self) -> Option<LiveId> {
        if let Some(dock) = self.borrow() {
            return dock.get_drawing_item_id();
        }
        None
    }
    
    pub fn drop_clone(&self, cx: &mut Cx, abs: DVec2, old_item: LiveId, new_item: LiveId) {
        if let Some(mut dock) = self.borrow_mut() {
            dock.drop_clone(cx, abs, old_item, new_item);
        }
    }
    
    pub fn drop_move(&self, cx: &mut Cx, abs: DVec2, item: LiveId) {
        if let Some(mut dock) = self.borrow_mut() {
            dock.handle_drop(cx, abs, item, true);
        }
    }
    
    pub fn drop_create(&self, cx: &mut Cx, abs: DVec2, item: LiveId, kind: LiveId, name: String) {
        if let Some(mut dock) = self.borrow_mut() {
            dock.drop_create(cx, abs, item, kind, name);
        }
    }
    
    pub fn create_tab(&self, cx: &mut Cx, parent: LiveId, item: LiveId, kind: LiveId, name: String) {
        if let Some(mut dock) = self.borrow_mut() {
            dock.create_tab(cx, parent, item, kind, name);
        }
    }
    
    pub fn redraw_tab(&self, cx: &mut Cx, tab_id: LiveId) {
        if let Some(mut dock) = self.borrow_mut() {
            dock.redraw_tab(cx, tab_id);
        }
    }
    
    
    pub fn tab_start_drag(&self, cx: &mut Cx, _tab_id: LiveId, item: DragItem) {
        cx.start_dragging(vec![item]);
    }
}

#[derive(Clone, WidgetSet)]
pub struct DockSet(WidgetSet);

