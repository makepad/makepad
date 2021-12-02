use {
    std::{
        collections::HashMap,
        collections::hash_map::Entry
    },
    crate::{
        genid::GenId,
    },
    makepad_render::*,
    makepad_widget::*,
};

live_register!{
    use makepad_render::shader_std::*;
    
    DrawNodeBg: {{DrawNodeBg}} {
        instance is_folder: float = 0.0
        instance selected: float = 0.0
        instance hover: float = 0.0
        instance opened: float = 0.0
        
        const color_even: vec4 = #25
        const color_odd: vec4 = #28
        const color_selected: vec4 = #x11466E
        
        fn pixel(self) -> vec4 {
            return mix(
                mix(
                    color_even,
                    color_odd,
                    self.is_even
                ),
                color_selected,
                self.selected
            ) + #3 * self.hover;
        }
    }
    
    FileTreeNode: {{FileTreeNode}} {
        bg_quad: {}
        
        icon_quad: {
            color: #80
            fn pixel(self) -> vec4 {
                let cx = Sdf2d::viewport(self.pos * self.rect_size);
                let w = self.rect_size.x;
                let h = self.rect_size.y;
                cx.box(0. * w, 0.35 * h, 0.87 * w, 0.39 * h, 0.75);
                cx.box(0. * w, 0.28 * h, 0.5 * w, 0.3 * h, 1.);
                cx.union();
                return cx.fill(self.color);
            }
        }
        
        name_text: {
            instance selected: float = 0.0
            instance hover: float = 0.0
            instance opened: float = 0.0
            instance is_folder: float = 0.0
            
            const color_file: vec4 = #9d
            const color_folder: vec4 = #ff
            
            fn get_color(self)->vec4{
              //  return self.color
                return mix(color_file, color_folder, self.is_folder);//mix(1.0,self.opened,self.is_folder))
            }
            
            text_style: {
                top_drop: 1.3,
            }
        }
        
        layout: {
            walk: {
                width: Width::Filled,
                height: Height::Fixed(0.0),
            },
            align: {fx: 0.0, fy: 0.5},
            padding: {l: 5.0, t: 0.0, r: 0.0, b: 1.0,},
        }
        
        icon_walk: Walk {
            width: Width::Fixed(14.0),
            height: Height::Filled,
            margin: Margin {
                l: 1.0,
                t: 0.0,
                r: 4.0,
                b: 0.0,
            },
        }
        
        default_state: {
            from: {all: Play::Forward {duration: 2.1}}
            hover: 0.0,
            bg_quad: {hover: (hover)}
            name_text: {hover: (hover)}
        }
        
        hover_state: {
            from: {all: Play::Forward {duration: 0.1}}
        }
        
        unselected_state: {
            from: {all: Play::Forward {duration: 0.1}}
            selected: 0.0,
            bg_quad: {selected: (selected)}
            name_text: {selected: (selected)}
        }
        
        selected_state: {
            from: {all: Play::Forward {duration: 0.1}}
            selected: [{time: 0.0, value: 1.0}],
        }
        
        closed_state: {
            from: {all: Play::Forward {duration: 0.3, redraw: true}}
            opened: [{value: 0.0, ease: Ease::OutExp}],
            bg_quad: {opened: (opened)}
            name_text: {opened: (opened)}
        }
        
        opened_state: {
            from: {all: Play::Forward {duration: 0.3, redraw: true}}
            opened: [{value: 1.0, ease: Ease::OutExp}],
        }
        
        opened: 1.0
        indent_width: 10.0
        file_node_height: 20.0
    }
    
    FileTree: {{FileTree}} {
        file_node: FileTreeNode {
            is_folder: false,
            bg_quad:{is_folder:0.0}
            name_text: {is_folder:0.0}
        }
        folder_node: FileTreeNode {
            is_folder: true,
            bg_quad:{is_folder:1.0}
            name_text: {is_folder:1.0}
        }
        test: (1.0 + 2.0)
        scroll_view: {
            view: {debug_id: file_tree_view}
        }
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawNodeBg {
    deref_target: DrawQuad,
    is_even: f32,
}

#[derive(Live, LiveHook)]
pub struct FileTreeNode {
    bg_quad: DrawNodeBg,
    icon_quad: DrawColor,
    name_text: DrawText,
    layout: Layout,
    
    #[track(
        hover = default_state,
        selected = unselected_state,
        opened = closed_state
    )]
    animator: Animator,
    
    file_node_height: f32,
    indent_width: f32,
    
    default_state: Option<LivePtr>,
    hover_state: Option<LivePtr>,
    selected_state: Option<LivePtr>,
    unselected_state: Option<LivePtr>,
    opened_state: Option<LivePtr>,
    closed_state: Option<LivePtr>,
    
    icon_walk: Walk,
    
    is_folder: bool,
    
    opened: f32,
    hover: f32,
    selected: f32,
}

#[derive(Live, LiveHook)]
pub struct FileTree {
    scroll_view: ScrollView,
    file_node: Option<LivePtr>,
    folder_node: Option<LivePtr>,
    test: f32,
    
    #[rust] last_selected: Option<FileNodeId>,
    #[rust] tree_nodes: HashMap<FileNodeId, FileTreeNode>,
    #[rust] count: usize,
    #[rust] stack: Vec<f32>,
}

pub enum FileTreeNodeAction {
    None,
    WasClicked,
}

impl FileTreeNode {
    pub fn draw_folder(&mut self, cx: &mut Cx, name: &str, is_even: bool, scale_stack: &[f32]) {
        
        let scale = scale_stack.last().cloned().unwrap_or(1.0);
        
        self.layout.walk.height = Height::Fixed(scale * self.file_node_height);
        self.bg_quad.is_even = if is_even {1.0}else {0.0};
        self.bg_quad.begin(cx, self.layout);
        
        cx.walk_turtle(self.indent_walk(scale_stack.len()));
        
        self.icon_quad.draw_walk(cx, self.icon_walk);
        cx.turtle_align_y();
        
        self.name_text.font_scale = scale_stack.last().cloned().unwrap_or(1.0);
        self.name_text.draw_walk(cx, name);
        self.bg_quad.end(cx);
        
        cx.turtle_new_line();
    }
    
    pub fn draw_file(&mut self, cx: &mut Cx, name: &str, is_even: bool, scale_stack: &[f32]) {
        
        let scale = scale_stack.last().cloned().unwrap_or(1.0);
        
        self.bg_quad.is_even = if is_even {1.0}else {0.0};
        self.layout.walk.height = Height::Fixed(scale * self.file_node_height);
        self.bg_quad.begin(cx, self.layout);
        
        cx.walk_turtle(self.indent_walk(scale_stack.len()));
        cx.turtle_align_y();
        
        self.name_text.font_scale = scale;
        self.name_text.draw_walk(cx, name);
        self.bg_quad.end(cx);
        
        cx.turtle_new_line();
    }
    
    fn indent_walk(&self, depth: usize) -> Walk {
        Walk {
            width: Width::Fixed(depth as f32 * self.indent_width),
            height: Height::Filled,
            margin: Margin {
                l: depth as f32 * 1.0,
                t: 0.0,
                r: depth as f32 * 4.0,
                b: 0.0,
            },
        }
    }
    
    fn set_is_selected(&mut self, cx: &mut Cx, is_selected: bool, should_animate: bool) {
        self.toggle_animator(
            cx,
            is_selected,
            should_animate,
            id!(selected),
            self.selected_state.unwrap(),
            self.unselected_state.unwrap()
        )
    }
    
    pub fn set_folder_is_expanded(
        &mut self,
        cx: &mut Cx,
        is_open: bool,
        should_animate: bool,
    ) {
        self.toggle_animator(
            cx,
            is_open,
            should_animate,
            id!(opened),
            self.opened_state.unwrap(),
            self.closed_state.unwrap()
        );
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, FileTreeNodeAction),
    ) {
        if self.animator_handle_event(cx, event) {
            self.bg_quad.draw_vars.redraw_view(cx);
        }
        match event.hits(cx, self.bg_quad.draw_vars.area, HitOpt::default()) {
            Event::FingerHover(event) => {
                cx.set_hover_mouse_cursor(MouseCursor::Hand);
                match event.hover_state {
                    HoverState::In => {
                        self.animate_to(cx, id!(hover), self.hover_state.unwrap());
                    }
                    HoverState::Out => {
                        self.animate_to(cx, id!(hover), self.default_state.unwrap());
                    }
                    _ => {}
                }
            }
            Event::FingerMove(_event) => {
                //if self.dragging_node_id.is_none()
                //    && event.abs.distance(&event.abs_start) >= MIN_DRAG_DISTANCE
                // {
                //     dispatch_action(TreeAction::NodeShouldStartDragging(*node_id));
                // }
            }
            Event::FingerDown(_event) => {
                self.animate_to(cx, id!(selected), self.selected_state.unwrap());
                if self.is_folder {
                    if self.opened > 0.2 {
                        self.animate_to(cx, id!(opened), self.closed_state.unwrap());
                    }
                    else {
                        self.animate_to(cx, id!(opened), self.opened_state.unwrap());
                    }
                }
                dispatch_action(cx, FileTreeNodeAction::WasClicked);
                //if area.get_rect(cx).contains(event.abs_start) {
                //    dispatch_action(TreeAction::NodeWasClicked(*node_id));
                //}
            }
            _ => {}
        }
    }
}


impl FileTree {
    
    pub fn begin(&mut self, cx: &mut Cx) -> Result<(), ()> {
        self.scroll_view.begin(cx) ?;
        self.count = 0;
        Ok(())
    }
    
    pub fn end(&mut self, cx: &mut Cx) {
        self.scroll_view.end(cx);
    }
    
    pub fn begin_folder(
        &mut self,
        cx: &mut Cx,
        node_id: FileNodeId,
        name: &str,
    ) -> Result<(), ()> {
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        
        self.count += 1;
        
        let tree_node = match self.tree_nodes.entry(node_id) {
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => v.insert(FileTreeNode::new_from_ptr_debug(cx, self.folder_node.unwrap()))
        };
        
        tree_node.draw_folder(cx, name, self.count % 2 == 1, &self.stack);
        
        self.stack.push(tree_node.opened * scale);
        
        if tree_node.opened == 0.0 {
            self.end_folder();
            return Err(());
        }
        Ok(())
    }
    
    pub fn end_folder(&mut self) {
        self.stack.pop();
    }
    
    pub fn file(&mut self, cx: &mut Cx, node_id: FileNodeId, name: &str) {
        self.count += 1;
        let tree_node = match self.tree_nodes.entry(node_id) {
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => v.insert(FileTreeNode::new_from_ptr(cx, self.file_node.unwrap()))
        };
        tree_node.draw_file(cx, name, self.count % 2 == 1, &self.stack);
        cx.turtle_new_line();
    }
    
    pub fn forget(&mut self) {
        self.tree_nodes.clear();
    }
    
    pub fn forget_node(&mut self, file_node_id: FileNodeId) {
        self.tree_nodes.remove(&file_node_id);
    }
    
    pub fn file_node_is_expanded(&mut self, _file_node_id: FileNodeId) -> bool {
        true
    }
    
    pub fn set_folder_is_expanded(
        &mut self,
        cx: &mut Cx,
        node_id: FileNodeId,
        is_open: bool,
        should_animate: bool,
    ) {
        let tree_node = match self.tree_nodes.entry(node_id) {
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => v.insert(FileTreeNode::new_from_ptr(cx, self.folder_node.unwrap()))
        };
        tree_node.set_folder_is_expanded(cx, is_open, should_animate);
    }
    
    pub fn set_selected_file_node_id(&mut self, _cx: &mut Cx, _file_node_id: FileNodeId) {
        /*if self.logic.set_selected_node_id(file_node_id.0) {
            self.scroll_view.redraw(cx);
        }*/
    }
    
    pub fn start_dragging_file_node(
        &mut self,
        _cx: &mut Cx,
        _file_node_id: FileNodeId,
        _dragged_item: DraggedItem,
    ) {
        /*self.logic.start_dragging_node(cx, file_node_id.0, dragged_item);*/
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_view.redraw(cx);
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, FileTreeAction),
    ) {
        if self.scroll_view.handle_event(cx, event) {
            self.scroll_view.redraw(cx);
        }
        
        let mut actions = Vec::new();
        for (key, node) in &mut self.tree_nodes {
            node.handle_event(cx, event, &mut | _, e | actions.push((*key, e)));
        }
        for (key, action) in actions {
            if let FileTreeNodeAction::WasClicked = action {
                if let Some(last_selected) = self.last_selected {
                    if last_selected != key {
                        self.tree_nodes.get_mut(&last_selected).unwrap().set_is_selected(cx, false, true);
                    }
                }
                self.last_selected = Some(key);
            }
        }
        /*
        let mut actions = Vec::new();
        self.logic.handle_event(cx, event, &mut | action | actions.push(action));
        for action in actions {
            match action {
                TreeAction::TreeWasAnimated => {
                    self.redraw(cx);
                }
                TreeAction::NodeWasEntered(node_id) => {
                    let file_node_id = FileNodeId(node_id);
                    self.set_hovered_file_node_id(cx, Some(file_node_id));
                }
                TreeAction::NodeWasExited(node_id) => {
                    if self.logic.hovered_node_id() == Some(node_id) {
                        self.set_hovered_file_node_id(cx, None);
                    }
                }
                TreeAction::NodeWasClicked(node_id) => {
                    let file_node_id = FileNodeId(node_id);
                    self.toggle_file_node_is_expanded(cx, file_node_id, true);
                    self.set_selected_file_node_id(cx, file_node_id);
                    dispatch_action(cx, FileTreeAction::FileNodeWasClicked(file_node_id));
                }
                TreeAction::NodeShouldStartDragging(node_id) => {
                    let file_node_id = FileNodeId(node_id);
                    dispatch_action(cx, FileTreeAction::FileNodeShouldStartDragging(file_node_id));
                }
            }
        }*/
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FileNodeId(pub GenId);

impl AsRef<GenId> for FileNodeId {
    fn as_ref(&self) -> &GenId {
        &self.0
    }
}

pub enum FileTreeAction {
    FileNodeWasClicked(FileNodeId),
    FileNodeShouldStartDragging(FileNodeId),
}


