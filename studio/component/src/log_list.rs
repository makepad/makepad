use {
    std::{
        collections::{HashSet},
    },
    crate::{
        makepad_component::{
            component_map::ComponentMap,
            fold_button::FoldButton,
            scroll_view::ScrollView,
            link_button::LinkButton,
        },
        makepad_platform::*,
        log_icon::{DrawLogIconQuad, LogIconType}
    },
};

live_register!{
    use makepad_platform::shader::std::*;
    use makepad_component::theme::*;
    
    DrawBgQuad: {{DrawBgQuad}} {
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
    
    DrawNameText: {{DrawNameText}} {
        fn get_color(self) -> vec4 {
            return mix(
                COLOR_TEXT_DEFAULT,
                COLOR_TEXT_SELECTED,
                self.selected
            );
        }
        text_style: FONT_DATA{top_drop: 1.15},
    }
    
    LogListNode: {{LogListNode}} {
        link_button:{
            
        }
        layout: {
            walk: {
                width: Width::Filled,
                height: Height::Fixed(0.0),
            },
            align: {fy: 0.5},
            padding: {left: 5},
        }
        
        icon_walk: {
            width: Width::Fixed((DIM_DATA_ICON_WIDTH)),
            height: Height::Fixed((DIM_DATA_ICON_WIDTH)),
            margin: {
                left: 1,
                right: 0,
            },
        }
        
        default_state: {
            duration: 0.2
            apply: {
                hover: 0.0,
                bg_quad: {hover: (hover)}
                name_text: {hover: (hover)}
                icon_quad: {hover: (hover)}
            }
        }
        
        hover_state: {
            duration: 0.1
            apply: {hover: [{time: 0.0, value: 1.0}]},
        }
        
        unselected_state: {
            track: select,
            duration: 0.1,
            apply: {
                selected: 0.0,
                bg_quad: {selected: (selected)}
                name_text: {selected: (selected)}
                icon_quad: {selected: (selected)}
            }
        }
        
        selected_state: {
            track: select,
            duration: 0.1,
            apply: {
                selected: [{time: 0.0, value: 1.0}],
            }
        }
        
        indent_width: 10.0
        min_drag_distance: 10.0
    }
    
    LogList: {{LogList}} {
        node_height: (DIM_DATA_ITEM_HEIGHT),
        fold_node: LogListNode {}
        scroll_view: {
            view: {
                layout: {direction: Direction::Down}
                debug_id: file_tree_view
            }
        }
    }
}

// TODO support a shared 'inputs' struct on drawshaders
#[derive(Live, LiveHook)]#[repr(C)]
struct DrawBgQuad {
    deref_target: DrawQuad,
    is_even: f32,
    selected: f32,
    hover: f32,
    opened: f32,
}

#[derive(Live, LiveHook)]#[repr(C)]
struct DrawNameText {
    deref_target: DrawText,
    is_even: f32,
    selected: f32,
    hover: f32,
    opened: f32,
}

#[derive(Live, LiveHook)]
pub struct LogListNode {
    bg_quad: DrawBgQuad,
    icon_quad: DrawLogIconQuad,
    name_text: DrawNameText,
    layout: Layout,
    
    #[state(default_state, unselected_state)]
    animator: Animator,
    
    indent_width: f32,
    
    default_state: Option<LivePtr>,
    hover_state: Option<LivePtr>,
    selected_state: Option<LivePtr>,
    unselected_state: Option<LivePtr>,
    
    fold_button: FoldButton,
    link_button: LinkButton,
    
    icon_walk: Walk,
    
    min_drag_distance: f32,
    
    opened: f32,
    hover: f32,
    selected: f32,
}

#[derive(Live)]
pub struct LogList {
    scroll_view: ScrollView,
    fold_node: Option<LivePtr>,
    
    filler_quad: DrawBgQuad,
    
    node_height: f32,
    
    #[rust] selected_node_ids: HashSet<LogListNodeId>,
    #[rust] open_nodes: HashSet<LogListNodeId>,
    
    #[rust] fold_nodes: ComponentMap<LogListNodeId, LogListNode>,
    
    #[rust] count: usize,
    #[rust] stack: Vec<f32>,
}

impl LiveHook for LogList {
    fn after_apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        if let Some(index) = nodes.child_by_name(index, id!(log_node)) {
            for (_, node) in self.fold_nodes.iter_mut() {
                node.apply(cx, apply_from, index, nodes);
            }
        }
        self.scroll_view.redraw(cx);
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
        self.bg_quad.is_even = is_even;
        self.name_text.is_even = is_even;
    }
    
    pub fn draw_node(
        &mut self,
        cx: &mut Cx2d,
        icon_type: LogIconType,
        link: &str,
        body: &str,
        is_even: f32,
        node_height: f32,
        _depth: usize
    ) {
        self.set_draw_state(is_even);
        self.layout.walk.height = Height::Fixed(node_height);
        self.bg_quad.begin(cx, self.layout);
        
        // lets draw a fold button
        self.fold_button.draw(cx);
        cx.turtle_align_y();
        
        // lets draw a fold button
        self.icon_quad.icon_type = icon_type;
        self.icon_quad.draw_walk(cx, self.icon_walk);
        cx.turtle_align_y();
        
        
        self.link_button.draw(cx, Some(link));
        cx.turtle_align_y();
        
        self.name_text.draw_walk(cx, body);
        self.bg_quad.end(cx);
    }
    
    fn _indent_walk(&self, depth: usize) -> Walk {
        Walk {
            width: Width::Fixed(depth as f32 * self.indent_width),
            height: Height::Filled,
            margin: Margin {
                left: depth as f32 * 1.0,
                top: 0.0,
                right: depth as f32 * 4.0,
                bottom: 0.0,
            },
        }
    }
    
    pub fn set_is_selected(&mut self, cx: &mut Cx, is_selected: bool, animate: Animate) {
        self.toggle_animator(cx, is_selected, animate, self.selected_state, self.unselected_state)
    }
    
    pub fn set_is_open(&mut self, cx: &mut Cx, is_open: bool, animate: Animate) {
        self.fold_button.set_is_open(cx, is_open, animate);
    }
    
    pub fn handle_event_with_fn(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, LogNodeAction),
    ) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.bg_quad.draw_vars.redraw(cx);
        }
        
        self.fold_button.handle_event_with_fn(cx, event, &mut | _cx, _action | {
            
        });
        
        self.link_button.handle_event(cx, event);
        
        match event.hits(cx, self.bg_quad.draw_vars.area) {
            HitEvent::FingerHover(f) => {
                cx.set_hover_mouse_cursor(MouseCursor::Hand);
                match f.hover_state {
                    HoverState::In => {
                        self.animate_to(cx, self.hover_state);
                    }
                    HoverState::Out => {
                        self.animate_to(cx, self.default_state);
                    }
                    _ => {}
                }
            }
            HitEvent::FingerMove(f) => {
                if f.abs.distance(&f.abs_start) >= self.min_drag_distance {
                    dispatch_action(cx, LogNodeAction::ShouldStartDragging);
                }
            }
            HitEvent::FingerDown(_) => {
                self.animate_to(cx, self.selected_state);
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
    
    pub fn begin(&mut self, cx: &mut Cx2d) -> Result<(), ()> {
        self.scroll_view.begin(cx) ?;
        self.count = 0;
        Ok(())
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        // lets fill the space left with blanks
        let height_left = cx.get_height_left();
        let mut walk = 0.0;
        while walk < height_left {
            self.count += 1;
            self.filler_quad.is_even = Self::is_even(self.count);
            self.filler_quad.draw_walk(cx, Walk::wh(Width::Filled, Height::Fixed(self.node_height.min(height_left - walk))));
            walk += self.node_height.max(1.0);
        }
        self.scroll_view.end(cx);
        
        let selected_node_ids = &self.selected_node_ids;
        self.fold_nodes.retain_visible_and( | node_id, _ | selected_node_ids.contains(node_id));
    }
    
    pub fn is_even(count: usize) -> f32 {
        if count % 2 == 1 {0.0}else {1.0}
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_view.redraw(cx);
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
            let mut node = LogListNode::new_from_option_ptr(cx, fold_node);
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
        if cx.turtle_line_is_visible(height, self.scroll_view.get_scroll_pos(cx)) {
            return true
        }
        else {
            cx.walk_turtle(Walk::wh(Width::Filled, Height::Fixed(height)));
            return false
        }
    }
    
    /*
    pub fn begin_folder(
        &mut self,
        cx: &mut Cx,
        node_id: FoldListNodeId,
        name: &str,
    ) -> Result<(), ()> {
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        
        if scale > 0.2 {
            self.count += 1;
        }
        
        let is_open = self.open_nodes.contains(&node_id);
        
        if self.should_node_draw(cx) {
            
            let (tree_node, _) = self.tree_nodes.get_or_insert_with_ptr(cx, node_id, self.folder_node, | cx, ptr | {
                let mut tree_node = FileTreeNode::new_from_ptr(cx, ptr);
                if is_open {
                    tree_node.set_folder_is_open(cx, true, false)
                }
                (tree_node, id!(folder_node))
            });
            
            tree_node.draw_folder(cx, name, Self::is_even(self.count), self.node_height, self.stack.len(), scale);
            self.stack.push(tree_node.opened * scale);
            if tree_node.opened == 0.0 {
                self.end_folder();
                return Err(());
            }
        }
        else {
            if is_open {
                self.stack.push(scale * 1.0);
            }
            else {
                return Err(());
            }
        }
        Ok(())
    }*/
    
    pub fn end_folder(&mut self) {
        self.stack.pop();
    }
    /*
    pub fn file(&mut self, cx: &mut Cx, node_id: FileNodeId, name: &str) {
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        
        if scale > 0.2 {
            self.count += 1;
        }
        if self.should_node_draw(cx) {
            let (tree_node, _) = self.tree_nodes.get_or_insert_with_ptr(cx, node_id, self.file_node, | cx, ptr | {
                (FileTreeNode::new_from_ptr(cx, ptr), id!(file_node))
            });
            tree_node.draw_file(cx, name, Self::is_even(self.count), self.node_height, self.stack.len(), scale);
        }
    }
    
    pub fn forget(&mut self) {
        self.tree_nodes.clear();
    }
    
    pub fn forget_node(&mut self, file_node_id: FileNodeId) {
        self.tree_nodes.remove(&file_node_id);
    }
    
    pub fn set_folder_is_open(
        &mut self,
        cx: &mut Cx,
        node_id: FileNodeId,
        is_open: bool,
        should_animate: bool,
    ) {
        if is_open {
            self.open_nodes.insert(node_id);
        }
        else {
            self.open_nodes.remove(&node_id);
        }
        if let Some((tree_node, _)) = self.tree_nodes.get_mut(&node_id) {
            tree_node.set_folder_is_open(cx, is_open, should_animate);
        }
    }
    
    pub fn start_dragging_file_node(
        &mut self,
        cx: &mut Cx,
        node_id: FileNodeId,
        dragged_item: DraggedItem,
    ) {
        self.dragging_node_id = Some(node_id);
        cx.start_dragging(dragged_item);
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_view.redraw(cx);
    }*/
    
    pub fn handle_event_with_fn(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, LogListAction),
    ) {
        if self.scroll_view.handle_event(cx, event) {
            self.scroll_view.redraw(cx);
        }
        
        let mut actions = Vec::new();
        for (node_id, node) in self.fold_nodes.iter_mut() {
            node.handle_event_with_fn(cx, event, &mut | _, e | actions.push((*node_id, e)));
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

