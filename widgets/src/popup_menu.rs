use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw_2d::*,
        widget::*,
    },
};

live_design!{
    import makepad_draw_2d::shader::std::*;
    import makepad_widgets::theme::*;
    
    DrawBgQuad = {{DrawBgQuad}} {
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            
            sdf.clear(mix(
                COLOR_BG_EDITOR,
                COLOR_BG_SELECTED,
                self.hover
            ));
            
            // we have 3 points, and need to rotate around its center
            let sz = 3.;
            let dx = 2.0;
            let c = vec2(8.0, 0.5 * self.rect_size.y);
            sdf.move_to(c.x - sz + dx * 0.5, c.y - sz + dx);
            sdf.line_to(c.x, c.y + sz);
            sdf.line_to(c.x + sz, c.y - sz);
            sdf.stroke(mix(#fff0, #f, self.selected), 1.0);
            
            return sdf.result;
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
        //text_style: FONT_DATA {top_drop: 1.15},
    }
    
    PopupMenuItem = {{PopupMenuItem}} {
        layout: {
            align: {y: 0.5},
            padding: {left: 15, top: 5, bottom: 5},
        }
        walk: {
            width: Fill,
            height: Fit
        }
        state: {
            hover = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        hover: 0.0,
                        bg: {hover: (hover)}
                        name: {hover: (hover)}
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
                    from: {all: Snap}
                    apply: {
                        selected: 0.0,
                        bg: {selected: (selected)}
                        name: {selected: (selected)}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {selected: 1.0}
                }
            }
        }
        indent_width: 10.0
    }
    
    PopupMenu = {{PopupMenu}} {
        menu_item: <PopupMenuItem> {}
        layout: {
            flow: Down,
            padding: 5
        }
        bg: {
            shape: ShadowBox,
            radius: 4,
            color: #0
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
    
    bg: DrawBgQuad,
    name: DrawNameText,
    
    layout: Layout,
    state: State,
    walk: Walk,
    
    indent_width: f32,
    icon_walk: Walk,
    
    opened: f32,
    hover: f32,
    selected: f32,
}

#[derive(Live)]
pub struct PopupMenu {
    view: View,
    menu_item: Option<LivePtr>,
    
    bg: DrawShape,
    layout: Layout,
    items: Vec<String>,
    #[rust] first_tap: bool,
    #[rust] menu_items: ComponentMap<PopupMenuItemId, PopupMenuItem>,
    #[rust] init_select_item: Option<PopupMenuItemId>,
    
    #[rust] count: usize,
}

impl LiveHook for PopupMenu {
    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        if let Some(index) = nodes.child_by_name(index, live_id!(list_node).as_field()) {
            for (_, node) in self.menu_items.iter_mut() {
                node.apply(cx, from, index, nodes);
            }
        }
        self.view.redraw(cx);
    }
}

pub enum PopupMenuItemAction {
    WasSweeped,
    WasSelected,
    MightBeSelected,
    None
}

#[derive(Clone, WidgetAction)]
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
        self.bg.begin(cx, self.walk, self.layout);
        self.name.draw_walk(cx, Walk::fit(), Align::default(), label);
        self.bg.end(cx);
    }
    
    pub fn handle_event_fn(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        sweep_area: Area,
        dispatch_action: &mut dyn FnMut(&mut Cx, PopupMenuItemAction),
    ) {
        if self.state_handle_event(cx, event).must_redraw() {
            self.bg.area().redraw(cx);
        }
        
        match event.hits_with_options(
            cx,
            self.bg.area(),
            HitOptions::with_sweep_area(sweep_area)
        ) {
            Hit::FingerHoverIn(_) => {
                self.animate_state(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, id!(hover.off));
            }
            Hit::FingerSweepIn(_) => {
                dispatch_action(cx, PopupMenuItemAction::WasSweeped);
                self.animate_state(cx, id!(hover.on));
                self.animate_state(cx, id!(select.on));
            }
            Hit::FingerSweepOut(se) => {
                if se.is_finger_up() {
                    if se.was_tap() { // ok this only goes for the first time
                        dispatch_action(cx, PopupMenuItemAction::MightBeSelected);
                    }
                    else {
                        dispatch_action(cx, PopupMenuItemAction::WasSelected);
                    }
                }
                else {
                    self.animate_state(cx, id!(hover.off));
                    self.animate_state(cx, id!(select.off));
                }
            }
            _ => {}
        }
    }
}

impl PopupMenu {
    
    pub fn menu_contains_pos(&self, cx: &mut Cx, pos: DVec2) -> bool {
        self.bg.area().get_clipped_rect(cx).contains(pos)
    }
    
    pub fn begin(&mut self, cx: &mut Cx2d, width: f64) {
        self.view.begin_overlay(cx);
        
        cx.begin_overlay_turtle(Layout::flow_down());
        
        // ok so. this thing needs a complete position reset
        self.bg.begin(cx, Walk::size(Size::Fixed(width), Size::Fit), self.layout);
        self.count = 0;
    }
    
    pub fn end(&mut self, cx: &mut Cx2d, shift: DVec2) {
        // ok so.
        let menu_rect1 = cx.turtle().padded_rect_used().translate(shift);
        let pass_rect = Rect {pos: dvec2(0.0, 0.0), size: cx.current_pass_size()};
        let menu_rect2 = pass_rect.add_margin(-dvec2(10.0, 10.0)).contain(menu_rect1);
        cx.turtle_mut().set_shift(shift + (menu_rect2.pos - menu_rect1.pos));
        self.bg.end(cx);
        
        cx.end_overlay_turtle();
        //cx.debug.rect_r(self.bg.area().get_rect(cx));
        self.view.end(cx);
        self.menu_items.retain_visible();
        if let Some(init_select_item) = self.init_select_item.take() {
            self.select_item_state(cx, init_select_item);
        }
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.view.redraw(cx);
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
                item.cut_state(cx, id!(select.on));
                item.cut_state(cx, id!(hover.on));
            }
            else {
                item.cut_state(cx, id!(select.off));
                item.cut_state(cx, id!(hover.off));
            }
        }
    }
    
    pub fn handle_event_fn(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        sweep_area: Area,
        dispatch_action: &mut dyn FnMut(&mut Cx, PopupMenuAction),
    ) {
        let mut actions = Vec::new();
        for (item_id, node) in self.menu_items.iter_mut() {
            node.handle_event_fn(cx, event, sweep_area, &mut | _, e | actions.push((*item_id, e)));
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

