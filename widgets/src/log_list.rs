use {
    std::{
        collections::{HashSet},
    },
    crate::{
        scroll_shadow::ScrollShadow,
        fold_button::FoldButton,
        scroll_bars::ScrollBars,
        link_label::LinkLabel,
        makepad_draw_2d::*,
        log_icon::{DrawLogIconQuad, LogIconType}
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
                COLOR_TEXT_DEFAULT,
                COLOR_TEXT_SELECTED,
                self.selected
            );
        }
        text_style: <FONT_DATA> {top_drop: 1.15},
    }
    
    LogListNode = {{LogListNode}} {
        
        layout: {
            align: {y: 0.5},
            padding: {left: 5},
        }
        name_walk: {
            width: Fit,
            height: Fit,
            margin: {left: 5}
        }
        icon_walk: {
            width: Fixed((DIM_DATA_ICON_WIDTH)),
            height: Fixed((DIM_DATA_ICON_WIDTH)),
            margin: {
                left: 1,
                right: 0,
            },
        }
        
        state: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        hover: 0.0,
                        bg: {hover: (hover)}
                        name: {hover: (hover)}
                        icon: {hover: (hover)}
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
                        icon: {selected: (selected)}
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
    
    LogList = {{LogList}} {
        node_height: (DIM_DATA_ITEM_HEIGHT),
        fold_node: <LogListNode> {}
        layout: {flow: Down, clip_x: true, clip_y: true},
    }
}

// TODO support a shared 'inputs' struct on drawshaders
#[derive(Live, LiveHook)]#[repr(C)]
struct DrawBgQuad {
    draw_super: DrawQuad,
    is_even: f32,
    selected: f32,
    hover: f32,
    opened: f32,
}

#[derive(Live, LiveHook)]#[repr(C)]
struct DrawNameText {
    draw_super: DrawText,
    is_even: f32,
    selected: f32,
    hover: f32,
    opened: f32,
}

#[derive(Live, LiveHook)]
pub struct LogListNode {
    bg: DrawBgQuad,
    icon: DrawLogIconQuad,
    name: DrawNameText,
    layout: Layout,
    
    state: State,
    
    indent_width: f64,
    
    fold_button: FoldButton,
    link_label: LinkLabel,
    
    icon_walk: Walk,
    name_walk: Walk,
    min_drag_distance: f64,
    
    opened: f32,
    hover: f32,
    selected: f32,
}

#[derive(Live)]
pub struct LogList {
    scroll_bars: ScrollBars,
    fold_node: Option<LivePtr>,
    
    filler_quad: DrawBgQuad,
    layout: Layout,
    node_height: f64,
    
    scroll_shadow: ScrollShadow,
    
    #[rust] selected_node_ids: HashSet<LogListNodeId>,
    #[rust] open_nodes: HashSet<LogListNodeId>,
    
    #[rust] fold_nodes: ComponentMap<LogListNodeId, LogListNode>,
    
    #[rust] count: usize,
    #[rust] stack: Vec<f32>,
}

impl LiveHook for LogList {
    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        if let Some(index) = nodes.child_by_name(index, live_id!(log_node).as_field()) {
            for (_, node) in self.fold_nodes.iter_mut() {
                node.apply(cx, from, index, nodes);
            }
        }
        self.scroll_bars.redraw(cx);
    }
}

pub enum LogNodeAction {
    Opening,
    Closing,
    WasClicked,
    ShouldStartDragging,
    None
}

pub enum LogListAction {
    WasClicked(LogListNodeId),
    None,
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct LogListNodeId(pub LiveId);

impl LogListNode {
    pub fn set_draw_state(&mut self, is_even: f32) {
        self.bg.is_even = is_even;
        self.name.is_even = is_even;
    }
    
    
    pub fn draw_node(
        &mut self,
        cx: &mut Cx2d,
        icon_type: LogIconType,
        link: &str,
        body: &str,
        is_even: f32,
        node_height: f64,
        _depth: usize
    ) {
        self.set_draw_state(is_even);
        
        self.bg.begin(cx, Walk::size(Size::Fill, Size::Fixed(node_height)), self.layout);
        
        // lets draw a fold button
        //self.fold_button.draw_walk(cx, self.fold_button.get_walk());
        
        // lets draw a fold button
        self.icon.icon_type = icon_type;
        self.icon.draw_walk(cx, self.icon_walk);
        if link.len()>0 {
            self.link_label.draw_label(cx, link);
        }
        
        self.name.draw_walk(cx, self.name_walk, Align::default(), body);
        self.bg.end(cx);
    }
    
    pub fn set_is_selected(&mut self, cx: &mut Cx, is_selected: bool, animate: Animate) {
        self.toggle_state(cx, is_selected, animate, id!(select.on), id!(select.off))
    }
    
    pub fn set_is_open(&mut self, cx: &mut Cx, is_open: bool, animate: Animate) {
        self.fold_button.set_is_open(cx, is_open, animate);
    }
    
    pub fn handle_event_fn(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, LogNodeAction),
    ) {
        if self.state_handle_event(cx, event).must_redraw() {
            self.bg.area().redraw(cx);
        }
        
        self.fold_button.handle_event_fn(cx, event, &mut | _, _ | {});
        
        self.link_label.handle_event_fn(cx, event, &mut | _, _ | {});
        
        match event.hits(cx, self.bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.animate_state(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animate_state(cx, id!(hover.off));
            }
            Hit::FingerMove(f) => {
                if f.abs.distance(&f.abs_start) >= self.min_drag_distance {
                    dispatch_action(cx, LogNodeAction::ShouldStartDragging);
                }
            }
            Hit::FingerDown(_) => {
                self.animate_state(cx, id!(select.on));
                /*
                if self.opened > 0.2 {
                    self.animate_to(cx, self.closed_state);
                    dispatch_action(cx, FoldNodeAction::Closing);
                }
                else {
                    self.animate_to(cx, self.opened_state);
                    dispatch_action(cx, FoldNodeAction::Opening);
                }*/
                dispatch_action(cx, LogNodeAction::WasClicked);
            }
            _ => {}
        }
    }
}


impl LogList {
    
    pub fn begin(&mut self, cx: &mut Cx2d) {
        self.scroll_bars.begin(cx, Walk::default(), self.layout);
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
        self.scroll_shadow.draw(cx, dvec2(0., 0.));
        self.scroll_bars.end(cx);
        
        let selected_node_ids = &self.selected_node_ids;
        self.fold_nodes.retain_visible_and( | node_id, _ | selected_node_ids.contains(node_id));
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
        log_icon: LogIconType,
        node_id: LogListNodeId,
        file: &str,
        body: &str,
        _has_open: bool
    ) -> f32 {
        self.count += 1;
        
        let is_open = self.open_nodes.contains(&node_id);
        
        // if self.should_node_draw(cx) {
        let fold_node = self.fold_node;
        let node = self.fold_nodes.get_or_insert(cx, node_id, | cx | {
            let mut node = LogListNode::new_from_ptr(cx, fold_node);
            if is_open {
                node.set_is_open(cx, true, Animate::No)
            }
            node
        });
        
        node.draw_node(cx, log_icon, file, body, Self::is_even(self.count), self.node_height, self.stack.len());
        
        if node.opened == 0.0 {
            return 0.0;
        }
        return node.opened;
        //}
        //return 0.0;
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
    
    pub fn end_folder(&mut self) {
        self.stack.pop();
    }
    
    pub fn handle_event_fn(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, LogListAction),
    ) {
        //let view_area = self.view_area;
        self.scroll_bars.handle_event_fn(cx, event, &mut | _, _ | {});
        
        let mut actions = Vec::new();
        for (node_id, node) in self.fold_nodes.iter_mut() {
            node.handle_event_fn(cx, event, &mut | _, e | actions.push((*node_id, e)));
        }
        
        for (node_id, action) in actions {
            match action {
                LogNodeAction::Opening => {
                    self.open_nodes.insert(node_id);
                }
                LogNodeAction::Closing => {
                    self.open_nodes.remove(&node_id);
                }
                LogNodeAction::WasClicked => {
                    // deselect everything but us
                    for id in &self.selected_node_ids {
                        if *id != node_id {
                            self.fold_nodes.get_mut(id).unwrap().set_is_selected(cx, false, Animate::Yes);
                        }
                    }
                    self.selected_node_ids.clear();
                    self.selected_node_ids.insert(node_id);
                    //dispatch_action(cx, FileTreeAction::WasClicked(node_id));
                }
                LogNodeAction::ShouldStartDragging => {
                    //if self.dragging_node_id.is_none() {
                    //    dispatch_action(cx, FileTreeAction::ShouldStartDragging(node_id));
                    // }
                }
                _ => ()
            }
        }
    }
}

