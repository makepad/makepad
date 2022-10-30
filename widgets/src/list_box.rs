use {
    std::{
        collections::{HashSet},
    },
    crate::{
        makepad_derive_widget::*,
        scroll_bars::ScrollBars,
        makepad_draw_2d::*,
        widget::*,
    },
};

live_design!{
    import makepad_draw_2d::shader::std::*;
    import makepad_widgets::theme::*;
    
    DrawBgQuad = {{DrawBgQuad}} {
        fn pixel(self) -> vec4 {
            return mix(
                mix(
                    COLOR_BG_EDITOR,
                    COLOR_BG_ODD,
                    self.is_even
                ),
                COLOR_BG_SELECTED,
                self.selected
            );
        }
    }
    
    DrawNameText = {{DrawNameText}} {
        fn get_color(self) -> vec4 {
            return mix(
                mix(
                    COLOR_TEXT_DEFAULT,
                    COLOR_TEXT_SELECTED,
                    self.selected
                ),
                COLOR_TEXT_HOVER,
                self.hover
            )
        }
    }
    
    ListBoxItem = {{ListBoxItem}} {
        layout: {
            align: {y: 0.5},
            padding: {left: 5},
        }
        
        state: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        hover: 0.0,
                        bg_quad: {hover: (hover)}
                        name_text: {hover: (hover)}
                    }
                }
                on = {
                    cursor: Hand
                    from: {all: Snap}
                    apply: {hover: 1.0},
                }
            }
            
            select = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        selected: 0.0,
                        bg_quad: {selected: (selected)}
                        name_text: {selected: (selected)}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {selected: 1.0}
                }
            }
        }
        
        indent_width: 10.0
        min_drag_distance: 10.0
    }
    
    ListBox = {{ListBox}} {
        node_height: (DIM_DATA_ITEM_HEIGHT),
        list_item: ListBoxItem {}
        walk: {width: Fill, height: Fit}
        layout: {flow: Down}
        scroll_bars: {}
    }
}

// TODO support a shared 'inputs' struct on drawshaders
#[derive(Live, LiveHook)]#[repr(C)]
struct DrawBgQuad {
    draw_super: DrawQuad,
    is_even: f32,
    selected: f32,
    hover: f32,
}

#[derive(Live, LiveHook)]#[repr(C)]
struct DrawNameText {
    draw_super: DrawText,
    is_even: f32,
    selected: f32,
    hover: f32,
}

#[derive(Live, LiveHook)]
pub struct ListBoxItem {
    
    bg_quad: DrawBgQuad,
    name_text: DrawNameText,
    
    layout: Layout,
    state: State,
    
    indent_width: f64,
    icon_walk: Walk,
    
    min_drag_distance: f64,
    opened: f32,
    hover: f32,
    selected: f32,
}

#[derive(Live)]
#[live_design_fn(widget_factory!(ListBox))]
pub struct ListBox {
    scroll_bars: ScrollBars,
    list_item: Option<LivePtr>,
    
    filler_quad: DrawBgQuad,
    layout: Layout,
    node_height: f64,
    multi_select: bool,
    
    walk: Walk,
    
    items: Vec<String>,
    
    #[rust] selected_item_ids: HashSet<ListBoxItemId>,
    
    #[rust] list_items: ComponentMap<ListBoxItemId, ListBoxItem>,
    
    #[rust] count: usize,
}

impl LiveHook for ListBox {
    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        if let Some(index) = nodes.child_by_name(index, live_id!(list_item).as_field()) {
            for (_, item) in self.list_items.iter_mut() {
                item.apply(cx, from, index, nodes);
            }
        }
        self.scroll_bars.redraw(cx);
    }
}

pub enum ListBoxNodeAction {
    WasClicked,
    ShouldStartDragging,
    None
}

#[derive(Clone, WidgetAction)]
pub enum ListBoxAction {
    WasClicked(ListBoxItemId),
    None,
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct ListBoxItemId(pub LiveId);

impl ListBoxItem {
    pub fn set_draw_state(&mut self, is_even: f32) {
        self.bg_quad.is_even = is_even;
        self.name_text.is_even = is_even;
    }
    
    pub fn draw_item(
        &mut self,
        cx: &mut Cx2d,
        label: &str,
        is_even: f32,
        node_height: f64,
    ) {
        self.set_draw_state(is_even);
        self.bg_quad.begin(cx, Walk::size(Size::Fill, Size::Fixed(node_height)), self.layout);
        self.name_text.draw_walk(cx, Walk::fit(), Align::default(), label);
        self.bg_quad.end(cx);
    }
    
    pub fn set_is_selected(&mut self, cx: &mut Cx, is_selected: bool, animate: Animate) {
        self.toggle_state(cx, is_selected, animate, id!(select.on), id!(select.off))
    }
    
    pub fn handle_event_fn(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, ListBoxNodeAction),
    ) {
        if self.state_handle_event(cx, event).must_redraw() {
            self.bg_quad.area().redraw(cx);
        }
        
        match event.hits(cx, self.bg_quad.area()) {
            Hit::FingerHoverIn(_) => {
                self.animate_state(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, id!(hover.off));
            }
            Hit::FingerMove(f) => {
                if f.abs.distance(&f.abs_start) >= self.min_drag_distance {
                    dispatch_action(cx, ListBoxNodeAction::ShouldStartDragging);
                }
            }
            Hit::FingerDown(_) => {
                self.animate_state(cx, id!(select.on));
                dispatch_action(cx, ListBoxNodeAction::WasClicked);
            }
            _ => {}
        }
    }
}

impl ListBox {
    
    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.scroll_bars.begin(cx, walk, self.layout);
        self.count = 0;
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        // lets fill the space left with blanks
        let height_left = cx.turtle().height_left();
        let mut walk = 0.0;
        while walk < height_left {
            self.count += 1;
            self.filler_quad.is_even = Self::is_even(self.count);
            self.filler_quad.draw_walk(cx, Walk::size(Size::Fill, Size::Fixed(self.node_height.min(height_left - walk))));
            walk += self.node_height.max(1.0);
        }
        
        self.scroll_bars.end(cx);
        
        let selected_item_ids = &self.selected_item_ids;
        self.list_items.retain_visible_and( | item_id, _ | selected_item_ids.contains(item_id));
    }
    
    pub fn is_even(count: usize) -> f32 {
        if count % 2 == 1 {0.0}else {1.0}
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_bars.redraw(cx);
    }
    
    pub fn draw_node(
        &mut self,
        cx: &mut Cx2d,
        item_id: ListBoxItemId,
        label: &str,
    ) {
        self.count += 1;
        
        let list_item = self.list_item;
        let node = self.list_items.get_or_insert(cx, item_id, | cx | {
            ListBoxItem::new_from_ptr(cx, list_item)
        });
        
        node.draw_item(cx, label, Self::is_even(self.count), self.node_height);
    }
    
    pub fn should_node_draw(&mut self, cx: &mut Cx2d) -> bool {
        let height = self.node_height;
        let walk = Walk::size(Size::Fill, Size::Fixed(height));
        if cx.walk_turtle_would_be_visible(walk) {
            return true
        }
        else {
            cx.walk_turtle(walk);
            return false
        }
    }
    
    pub fn handle_event_fn(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, ListBoxAction),
    ) {
        self.scroll_bars.handle_event_fn(cx, event, &mut | _, _ | {});
        
        let mut actions = Vec::new();
        for (node_id, node) in self.list_items.iter_mut() {
            node.handle_event_fn(cx, event, &mut | _, e | actions.push((*node_id, e)));
        }
        
        for (node_id, action) in actions {
            match action {
                ListBoxNodeAction::WasClicked => {
                    // deselect everything but us
                    for id in &self.selected_item_ids {
                        if *id != node_id {
                            self.list_items.get_mut(id).unwrap().set_is_selected(cx, false, Animate::Yes);
                        }
                    }
                    self.selected_item_ids.clear();
                    self.selected_item_ids.insert(node_id);
                    //dispatch_action(cx, FileTreeAction::WasClicked(node_id));
                }
                ListBoxNodeAction::ShouldStartDragging => {
                }
                _ => ()
            }
        }
    }
}


impl Widget for ListBox {
    fn widget_uid(&self) -> WidgetUid {return WidgetUid(self as *const _ as u64)}

    /*fn bind_read(&mut self, _cx: &mut Cx, _nodes: &[LiveNode]) {
        // lets use enum name to find a selected item here
        
        if let Some(LiveValue::Float(v)) = nodes.read_path(&self.bind) {
            self.set_internal(*v as f32);
            self.update_text_input(cx);
        }
    }*/
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_bars.redraw(cx);
    }
    
    fn handle_widget_event_fn(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let uid = self.widget_uid();
        self.handle_event_fn(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid))
        });
    }
    
    fn get_walk(&self) -> Walk {self.walk}
    
    fn draw_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.begin(cx, walk);
        for (i, item_str) in self.items.iter().enumerate() {
            let node_id = id_num!(listbox, i as u64).into();
            self.count += 1;
            let list_item = self.list_item;
            let item = self.list_items.get_or_insert(cx, node_id, | cx | {
                ListBoxItem::new_from_ptr(cx, list_item)
            });
            
            item.draw_item(cx, &item_str, Self::is_even(self.count), self.node_height);
        }
        self.end(cx);
        WidgetDraw::done()
    }
}

