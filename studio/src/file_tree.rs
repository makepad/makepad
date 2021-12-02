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
    
    DrawBgQuad: {{DrawBgQuad}} {
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
    
    DrawNameText: {{DrawNameText}} {
        const color_file: vec4 = #9d
        const color_folder: vec4 = #ff
        
        fn get_color(self) -> vec4 {
            return mix(color_file, color_folder, self.is_folder) * self.scale
        }
        
        text_style: {
            top_drop: 1.3,
        }
    }
    
    DrawIconQuad: {{DrawIconQuad}} {
        fn pixel(self) -> vec4 {
            let cx = Sdf2d::viewport(self.pos * self.rect_size);
            let w = self.rect_size.x;
            let h = self.rect_size.y;
            cx.box(0. * w, 0.35 * h, 0.87 * w, 0.39 * h, 0.75);
            cx.box(0. * w, 0.28 * h, 0.5 * w, 0.3 * h, 1.);
            cx.union();
            return cx.fill(#80);
        }
    }
    
    FileTreeNode: {{FileTreeNode}} {
        
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
            from: {all: Play::Forward {duration: 0.2}}
            hover: 0.0,
            bg_quad: {hover: (hover)}
            name_text: {hover: (hover)}
            icon_quad: {hover: (hover)}
        }
        
        hover_state: {
            from: {all: Play::Forward {duration: 0.1}}
            hover: [{time: 0.0, value: 1.0}],
        }
        
        unselected_state: {
            from: {all: Play::Forward {duration: 0.1}}
            selected: 0.0,
            bg_quad: {selected: (selected)}
            name_text: {selected: (selected)}
            icon_quad: {selected: (selected)}
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
            icon_quad: {opened: (opened)}
        }
        
        opened_state: {
            from: {all: Play::Forward {duration: 0.3, redraw: true}}
            opened: [{value: 1.0, ease: Ease::OutExp}],
        }
        
        indent_width: 10.0
        file_node_height: 20.0
        min_drag_distance: 10.0
    }
    
    FileTree: {{FileTree}} {
        file_node: FileTreeNode {
            is_folder: false,
            bg_quad: {is_folder: 0.0}
            name_text: {is_folder: 0.0}
        }
        folder_node: FileTreeNode {
            is_folder: true,
            bg_quad: {is_folder: 1.0}
            name_text: {is_folder: 1.0}
        }
        scroll_view: {
            view: {debug_id: file_tree_view}
        }
    }
}

// TODO support a shared 'inputs' struct on drawshaders
#[derive(Live, LiveHook)]#[repr(C)]
struct DrawBgQuad {
    deref_target: DrawQuad,
    is_even: f32,
    scale: f32,
    is_folder: f32,
    selected: f32,
    hover: f32,
    opened: f32,
}

#[derive(Live, LiveHook)]#[repr(C)]
struct DrawNameText {
    deref_target: DrawText,
    is_even: f32,
    scale: f32,
    is_folder: f32,
    selected: f32,
    hover: f32,
    opened: f32,
}

#[derive(Live, LiveHook)]#[repr(C)]
struct DrawIconQuad {
    deref_target: DrawQuad,
    is_even: f32,
    scale: f32,
    is_folder: f32,
    selected: f32,
    hover: f32,
    opened: f32,
}

#[derive(Live, LiveHook)]
pub struct FileTreeNode {
    bg_quad: DrawBgQuad,
    icon_quad: DrawIconQuad,
    name_text: DrawNameText,
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
    min_drag_distance: f32,
    
    opened: f32,
    hover: f32,
    selected: f32,
}

#[derive(Live, LiveHook)]
pub struct FileTree {
    scroll_view: ScrollView,
    file_node: Option<LivePtr>,
    folder_node: Option<LivePtr>,
    
    #[rust] dragging_node_id: Option<FileNodeId>,
    #[rust] last_selected: Option<FileNodeId>,
    #[rust] tree_nodes: HashMap<FileNodeId, FileTreeNode>,
    #[rust] count: usize,
    #[rust] stack: Vec<f32>,
}

pub enum FileTreeNodeAction {
    None,
    WasClicked,
    NodeShouldStartDragging,
}

impl FileTreeNode {
    pub fn set_draw_state(&mut self, is_even: bool, scale: f32) {
        let is_even = if is_even {1.0}else {0.0};
        self.bg_quad.scale = scale;
        self.bg_quad.is_even = is_even;
        self.name_text.scale = scale;
        self.name_text.is_even = is_even;
        self.icon_quad.scale = scale;
        self.icon_quad.is_even = is_even;
        self.name_text.font_scale = scale;
    }
    
    pub fn draw_folder(&mut self, cx: &mut Cx, name: &str, is_even: bool, scale_stack: &[f32]) {
        
        let scale = scale_stack.last().cloned().unwrap_or(1.0);
        self.set_draw_state(is_even, scale);
        
        self.layout.walk.height = Height::Fixed(scale * self.file_node_height);
        self.bg_quad.begin(cx, self.layout);
        
        cx.walk_turtle(self.indent_walk(scale_stack.len()));
        
        self.icon_quad.draw_walk(cx, self.icon_walk);
        cx.turtle_align_y();
        
        self.name_text.draw_walk(cx, name);
        self.bg_quad.end(cx);
        
        cx.turtle_new_line();
    }
    
    pub fn draw_file(&mut self, cx: &mut Cx, name: &str, is_even: bool, scale_stack: &[f32]) {
        
        let scale = scale_stack.last().cloned().unwrap_or(1.0);
        
        self.set_draw_state(is_even, scale);
        
        self.layout.walk.height = Height::Fixed(scale * self.file_node_height);
        self.bg_quad.begin(cx, self.layout);
        
        cx.walk_turtle(self.indent_walk(scale_stack.len()));
        cx.turtle_align_y();
        
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
            Event::FingerMove(event) => {
                if event.abs.distance(&event.abs_start) >= self.min_drag_distance {
                    dispatch_action(cx, FileTreeNodeAction::NodeShouldStartDragging);
                }
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
            Entry::Vacant(v) => v.insert(FileTreeNode::new_from_ptr(cx, self.folder_node.unwrap()))
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
        cx: &mut Cx,
        node_id: FileNodeId,
        dragged_item: DraggedItem,
    ) {
        self.dragging_node_id = Some(node_id);
        cx.start_dragging(dragged_item);
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_view.redraw(cx);
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, FileTreeAction),
    ) {
        if self.scroll_view.handle_event(cx, event) {
            self.scroll_view.redraw(cx);
        }
        
        match event{
            Event::DragEnd => self.dragging_node_id = None,
            _=>()
        }
        
        let mut actions = Vec::new();
        for (node_id, node) in &mut self.tree_nodes {
            node.handle_event(cx, event, &mut | _, e | actions.push((*node_id, e)));
        }
        for (node_id, action) in actions {
            match action{
                FileTreeNodeAction::WasClicked=>{
                    if let Some(last_selected) = self.last_selected {
                        if last_selected != node_id {
                            self.tree_nodes.get_mut(&last_selected).unwrap().set_is_selected(cx, false, true);
                        }
                    }
                    self.last_selected = Some(node_id);
                    dispatch_action(cx, FileTreeAction::FileNodeWasClicked(node_id));
                }
                FileTreeNodeAction::NodeShouldStartDragging=>{
                    if self.dragging_node_id.is_none(){
                        dispatch_action(cx, FileTreeAction::FileNodeShouldStartDragging(node_id));
                    }
                }
                _=>()
            } 
        }
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