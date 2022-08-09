use {
    crate::{
        makepad_derive_frame::*,
        scroll_view::ScrollView,
        makepad_draw_2d::*,
        frame::*,
    },
};

live_register!{
    import makepad_draw_2d::shader::std::*;
    import makepad_component::theme::*;
    
    DrawBgQuad: {{DrawBgQuad}} {
        fn pixel(self) -> vec4 {
            return mix(
                COLOR_BG_EDITOR,
                COLOR_BG_SELECTED,
                self.selected
            );
        }
    }
    
    DrawNameText: {{DrawNameText}} {
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
        text_style: FONT_DATA {top_drop: 1.15},
    }
    
    PopupMenuItem: {{PopupMenuItem}} {
        layout: {
            align: {y: 0.5},
            padding: {left: 5},
        }
        
        state: {
            hover = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {
                        hover: 0.0,
                        bg_quad: {hover: (hover)}
                        name_text: {hover: (hover)}
                    }
                }
                on = {
                    cursor: Hand
                    from: {all: Play::Snap}
                    apply: {hover: 1.0},
                }
            }
            
            select = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {
                        selected: 0.0,
                        bg_quad: {selected: (selected)}
                        name_text: {selected: (selected)}
                    }
                }
                on = {
                    from: {all: Play::Snap}
                    apply: {selected: 1.0}
                }
            }
        }
        
        indent_width: 10.0
        min_drag_distance: 10.0
    }
    
    PopupMenu: {{PopupMenu}} {
        node_height: (DIM_DATA_ITEM_HEIGHT),
        menu_item: PopupMenuItem {}
        layout: {flow: Flow::Down}
        scroll_view: {
            view: {
                debug_id: file_tree_view
            }
        }
    }
}

// TODO support a shared 'inputs' struct on drawshaders
#[derive(Live, LiveHook)]#[repr(C)]
struct DrawBgQuad {
    draw_super: DrawQuad,
    selected: f32,
    hover: f32,
}

#[derive(Live, LiveHook)]#[repr(C)]
struct DrawNameText {
    draw_super: DrawText,
    selected: f32,
    hover: f32,
}

#[derive(Live, LiveHook)]
pub struct PopupMenuItem {
    
    bg_quad: DrawBgQuad,
    name_text: DrawNameText,
    
    layout: Layout,
    state: State,
    
    indent_width: f32,
    icon_walk: Walk,
    
    min_drag_distance: f32,
    opened: f32,
    hover: f32,
    selected: f32,
}

#[derive(Live)]
pub struct PopupMenu {
    scroll_view: ScrollView,
    menu_item: Option<LivePtr>,
    
    filler_quad: DrawBgQuad,
    layout: Layout,
    node_height: f32,
    
    items: Vec<String>,
    
    #[rust] menu_items: ComponentMap<PopupMenuItemId, PopupMenuItem>,
    
    #[rust] count: usize,
}

impl LiveHook for PopupMenu {
    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        if let Some(index) = nodes.child_by_name(index, id!(list_node).as_field()) {
            for (_, node) in self.menu_items.iter_mut() {
                node.apply(cx, from, index, nodes);
            }
        }
        self.scroll_view.redraw(cx);
    }
}

pub enum ListBoxNodeAction {
    WasClicked,
    ShouldStartDragging,
    None
}

#[derive(Clone, FrameAction)]
pub enum ListBoxAction {
    WasClicked(PopupMenuItemId),
    None,
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct PopupMenuItemId(pub LiveId);

impl PopupMenuItem {
    
    pub fn draw_item(
        &mut self,
        cx: &mut Cx2d,
        label: &str,
        node_height: f32,
    ) {
        self.bg_quad.begin(cx, Walk::size(Size::Fill, Size::Fixed(node_height)), self.layout);
        self.name_text.draw_walk(cx, Walk::fit(), Align::default(), label);
        self.bg_quad.end(cx);
    }
    
    pub fn set_is_selected(&mut self, cx: &mut Cx, is_selected: bool, animate: Animate) {
        self.toggle_state(cx, is_selected, animate, ids!(select.on), ids!(select.off))
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        sweep_area: Area,
        dispatch_action: &mut dyn FnMut(&mut Cx, ListBoxNodeAction),
    ) {
        if self.state_handle_event(cx, event).must_redraw() {
            self.bg_quad.area().redraw(cx);
        }
        
        match event.hits_with_options(
            cx,
            self.bg_quad.area(),
            HitOptions::with_sweep_area(sweep_area)
        ) {
            Hit::FingerHoverIn(_) => {
                self.animate_state(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, ids!(hover.off));
            }
            Hit::FingerSweepIn(_) => {
                self.animate_state(cx, ids!(select.on));
            }
            Hit::FingerSweepOut(se) => {
                if se.is_up{
                    dispatch_action(cx, ListBoxNodeAction::WasClicked);
                }
                else{
                    self.animate_state(cx, ids!(select.off));
                }
            }
            _ => {}
        }
    }
}


impl PopupMenu {
    
    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk) -> ViewRedrawing {
        self.scroll_view.begin(cx, walk, self.layout) ?;
        self.count = 0;
        ViewRedrawing::yes()
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        // lets fill the space left with blanks
        let height_left = cx.turtle().height_left();
        let mut walk = 0.0;
        while walk < height_left {
            self.count += 1;
            self.filler_quad.draw_walk(cx, Walk::size(Size::Fill, Size::Fixed(self.node_height.min(height_left - walk))));
            walk += self.node_height.max(1.0);
        }
        self.scroll_view.end(cx);
        self.menu_items.retain_visible();
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_view.redraw(cx);
    }
    
    pub fn draw_item(
        &mut self,
        cx: &mut Cx2d,
        item_id: PopupMenuItemId,
        label: &str,
    ) {
        self.count += 1;
        
        let menu_item = self.menu_item;
        let menu_item = self.menu_items.get_or_insert(cx, item_id, | cx | {
            PopupMenuItem::new_from_ptr(cx, menu_item)
        });
        
        menu_item.draw_item(cx, label, self.node_height);
    }
    
    pub fn should_node_draw(&mut self, cx: &mut Cx2d) -> bool {
        let height = self.node_height;
        let walk = Walk::size(Size::Fill, Size::Fixed(height));
        if cx.walk_turtle_would_be_visible(walk, self.scroll_view.get_scroll_pos(cx)) {
            return true
        }
        else {
            cx.walk_turtle(walk);
            return false
        }
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        sweep_area: Area,
        _dispatch_action: &mut dyn FnMut(&mut Cx, ListBoxAction),
    ) {
        if self.scroll_view.handle_event(cx, event) {
            self.scroll_view.redraw(cx);
        }
        
        let mut actions = Vec::new();
        for (item_id, node) in self.menu_items.iter_mut() {
            node.handle_event(cx, event, sweep_area, &mut | _, e | actions.push((*item_id, e)));
        }
        
        for (node_id, action) in actions {
            match action {
                ListBoxNodeAction::WasClicked => {
                    // ok so. lets unselect the other items
                    for (id, item) in &mut *self.menu_items {
                        if *id != node_id {
                            item.set_is_selected(cx, false, Animate::Yes);
                        }
                    }
                }
                ListBoxNodeAction::ShouldStartDragging => {
                }
                _ => ()
            }
        }
    }
}

