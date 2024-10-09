use {
    crate::{
        makepad_draw::*,
    },
};

live_design!{
    PopupMenuItemBase = {{PopupMenuItem}} {}
    PopupMenuBase = {{PopupMenu}} {}
}


#[derive(Live, LiveHook, LiveRegister)]
pub struct PopupMenuItem {
    
    #[live] draw_bg: DrawQuad,
    #[live] draw_name: DrawText,
    
    #[layout] layout: Layout,
    #[animator] animator: Animator,
    #[walk] walk: Walk,
    
    #[live] indent_width: f32,
    #[live] icon_walk: Walk,
    
    #[live] opened: f32,
    #[live] hover: f32,
    #[live] selected: f32,
}

#[derive(Live, LiveRegister)]
pub struct PopupMenu {
    #[live] draw_list: DrawList2d,
    #[live] menu_item: Option<LivePtr>,
    
    #[live] draw_bg: DrawQuad,
    #[layout] layout: Layout,
    #[walk] walk: Walk,
    #[live] items: Vec<String>,
    #[rust] first_tap: bool,
    #[rust] menu_items: ComponentMap<PopupMenuItemId, PopupMenuItem>,
    #[rust] init_select_item: Option<PopupMenuItemId>,
    
    #[rust] count: usize,
}

impl LiveHook for PopupMenu {
    fn after_apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) {
        if let Some(index) = nodes.child_by_name(index, live_id!(list_node).as_field()) {
            for (_, node) in self.menu_items.iter_mut() {
                node.apply(cx, apply, index, nodes);
            }
        }
        self.draw_list.redraw(cx);
    }
}

pub enum PopupMenuItemAction {
    WasSweeped,
    WasSelected,
    MightBeSelected,
    None
}

#[derive(Clone, DefaultNone)]
pub enum PopupMenuAction {
    WasSweeped(PopupMenuItemId),
    WasSelected(PopupMenuItemId),
    None,
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct PopupMenuItemId(pub LiveId);

impl PopupMenuItem {
    
    pub fn draw_item(
        &mut self,
        cx: &mut Cx2d,
        label: &str,
    ) {
        self.draw_bg.begin(cx, self.walk, self.layout);
        self.draw_name.draw_walk(cx, Walk::fit(), Align::default(), label);
        self.draw_bg.end(cx);
    }
    
    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        sweep_area: Area,
        dispatch_action: &mut dyn FnMut(&mut Cx, PopupMenuItemAction),
    ) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.area().redraw(cx);
        }
        
        match event.hits_with_options(
            cx,
            self.draw_bg.area(),
            HitOptions::new().with_sweep_area(sweep_area)
        ) {
            Hit::FingerHoverIn(_) => {
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            }
            Hit::FingerDown(_) => {
                dispatch_action(cx, PopupMenuItemAction::WasSweeped);
                self.animator_play(cx, id!(hover.on));
                self.animator_play(cx, id!(select.on));
            }
            Hit::FingerUp(se) => {
                if !se.is_sweep {
                    //if se.was_tap() { // ok this only goes for the first time
                    //    dispatch_action(cx, PopupMenuItemAction::MightBeSelected);
                    //    println!("MIGHTBESELECTED");
                    // }
                    //else {
                    dispatch_action(cx, PopupMenuItemAction::WasSelected);
                    //}
                }
                else {
                    self.animator_play(cx, id!(hover.off));
                    self.animator_play(cx, id!(select.off));
                }
            }
            _ => {}
        }
    }
}

impl PopupMenu {
    
    pub fn menu_contains_pos(&self, cx: &mut Cx, pos: DVec2) -> bool {
        self.draw_bg.area().clipped_rect(cx).contains(pos)
    }
    
    pub fn begin(&mut self, cx: &mut Cx2d) {
        self.draw_list.begin_overlay_reuse(cx);
        
        cx.begin_pass_sized_turtle(Layout::flow_down());
        
        // ok so. this thing needs a complete position reset
        self.draw_bg.begin(cx, self.walk, self.layout);
        self.count = 0;
    }
    
    pub fn end(&mut self, cx: &mut Cx2d, shift_area: Area, shift: DVec2) {
        // ok so.
        /*
        let menu_rect1 = cx.turtle().padded_rect_used();
        let pass_rect = Rect {pos: dvec2(0.0, 0.0), size: cx.current_pass_size()};
        let menu_rect2 = pass_rect.add_margin(-dvec2(10.0, 10.0)).contain(menu_rect1);
        */
        //cx.turtle_mut().set_shift(shift + (menu_rect2.pos - menu_rect1.pos));
        //let menu_rect1 = cx.turtle().padded_rect_used();
        self.draw_bg.end(cx);
        
        cx.end_pass_sized_turtle_with_shift(shift_area, shift);
        //cx.debug.rect_r(self.draw_bg.area().get_rect(cx));
        self.draw_list.end(cx);
        self.menu_items.retain_visible();
        if let Some(init_select_item) = self.init_select_item.take() {
            self.select_item_state(cx, init_select_item);
        }
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.draw_list.redraw(cx);
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
        menu_item.draw_item(cx, label);
    }
    
    pub fn init_select_item(&mut self, which_id: PopupMenuItemId) {
        self.init_select_item = Some(which_id);
        self.first_tap = true;
    }
    
    fn select_item_state(&mut self, cx: &mut Cx, which_id: PopupMenuItemId) {
        for (id, item) in &mut *self.menu_items {
            if *id == which_id {
                item.animator_cut(cx, id!(select.on));
                item.animator_cut(cx, id!(hover.on));
            }
            else {
                item.animator_cut(cx, id!(select.off));
                item.animator_cut(cx, id!(hover.off));
            }
        }
    }
    
    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        sweep_area: Area,
        dispatch_action: &mut dyn FnMut(&mut Cx, PopupMenuAction),
    ) {
        let mut actions = Vec::new();
        for (item_id, node) in self.menu_items.iter_mut() {
            node.handle_event_with(cx, event, sweep_area, &mut | _, e | actions.push((*item_id, e)));
        }
        
        for (node_id, action) in actions {
            match action {
                PopupMenuItemAction::MightBeSelected => {
                    if self.first_tap {
                        self.first_tap = false;
                    }
                    else {
                        self.select_item_state(cx, node_id);
                        dispatch_action(cx, PopupMenuAction::WasSelected(node_id));
                    }
                }
                PopupMenuItemAction::WasSweeped => {
                    self.select_item_state(cx, node_id);
                    dispatch_action(cx, PopupMenuAction::WasSweeped(node_id));
                }
                PopupMenuItemAction::WasSelected => {
                    self.select_item_state(cx, node_id);
                    dispatch_action(cx, PopupMenuAction::WasSelected(node_id));
                }
                _ => ()
            }
        }
    }
}

