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
            sdf.move_to(c.x - sz+dx*0.5, c.y - sz+dx);
            sdf.line_to(c.x, c.y + sz);
            sdf.line_to(c.x + sz, c.y - sz);
            sdf.stroke(mix(#fff0,#f, self.selected), 1.0);
            
            return sdf.result;
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
        //text_style: FONT_DATA {top_drop: 1.15},
    }
    
    PopupMenuItem: {{PopupMenuItem}} {
        layout: {
            align: {y: 0.5},
            padding: {left: 15, top: 5, bottom: 5},
        }
        walk:{
            width: Fill,
            height: Fit
        }
        state: {
            hover = {
                default: off
                off = {
                    from: {all: Play::Snap}
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
                    from: {all: Play::Snap}
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
    
    walk: Walk,
    
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
    
    items: Vec<String>,
    
    #[rust] selected_item: PopupMenuItemId,
    
    #[rust] first_tap: bool,
    #[rust] menu_items: ComponentMap<PopupMenuItemId, PopupMenuItem>,
    #[rust] init_select_item: Option<PopupMenuItemId>,
    
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

pub enum PopupMenuItemAction {
    WasSweeped,
    WasSelected,
    MightBeSelected,
    None
}

#[derive(Clone, FrameAction)]
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
        self.bg_quad.begin(cx, self.walk, self.layout);
        self.name_text.draw_walk(cx, Walk::fit(), Align::default(), label);
        self.bg_quad.end(cx);
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        sweep_area: Area,
        dispatch_action: &mut dyn FnMut(&mut Cx, PopupMenuItemAction),
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
                dispatch_action(cx, PopupMenuItemAction::WasSweeped);
                self.animate_state(cx, ids!(hover.on));
                self.animate_state(cx, ids!(select.on));
            }
            Hit::FingerSweepOut(se) => {
                if se.is_finger_up(){
                    if se.was_tap(){// ok this only goes for the first time
                        dispatch_action(cx, PopupMenuItemAction::MightBeSelected);
                    }
                    else{
                        dispatch_action(cx, PopupMenuItemAction::WasSelected);
                    }
                }
                else{
                    self.animate_state(cx, ids!(hover.off));
                    self.animate_state(cx, ids!(select.off));
                }
            }
            _ => {}
        }
    }
}

impl PopupMenu {
    
    pub fn menu_contains_pos(&self, cx:&mut Cx, pos:Vec2)->bool{
        self.scroll_view.area().get_rect(cx).contains(pos)
    }
    
    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk) -> ViewRedrawing {
        self.scroll_view.begin(cx, walk, self.layout) ?;
        self.count = 0;
        ViewRedrawing::yes()
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        self.scroll_view.end(cx);
        self.menu_items.retain_visible();
        if let Some(init_select_item) = self.init_select_item.take(){
            self.select_item_state(cx, init_select_item);
        }
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
        
        menu_item.draw_item(cx, label);
    }
    
    pub fn init_select_item(&mut self, which_id:PopupMenuItemId){
        self.init_select_item = Some(which_id);
        self.first_tap = true;
    }
    
    fn select_item_state(&mut self, cx:&mut Cx, which_id:PopupMenuItemId){
        for (id, item) in &mut *self.menu_items {
            if *id == which_id{
                item.cut_state(cx, ids!(select.on));
                item.cut_state(cx, ids!(hover.on));
            }
            else{
                item.cut_state(cx, ids!(select.off));
                item.cut_state(cx, ids!(hover.off));
            }
        }
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        sweep_area: Area,
        dispatch_action: &mut dyn FnMut(&mut Cx, PopupMenuAction),
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
                PopupMenuItemAction::MightBeSelected=>{
                    // ok so. the first time we encounter this after open its sweep
                    // next time its selection
                    self.select_item_state(cx, node_id);
                    if self.first_tap{
                        self.first_tap = false;
                        dispatch_action(cx, PopupMenuAction::WasSweeped(node_id));
                    }
                    else{
                        dispatch_action(cx, PopupMenuAction::WasSelected(node_id));
                    }
                }
                PopupMenuItemAction::WasSweeped=>{
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

