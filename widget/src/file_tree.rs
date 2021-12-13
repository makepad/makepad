use {
    std::{
        collections::{HashSet, HashMap},
        collections::hash_map::Entry
    },
    crate::{
        genid::GenId,
        scroll_view::ScrollView
    },
    makepad_render::*,
};

live_register!{
    use makepad_render::shader::std::*;
    
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
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            let w = self.rect_size.x;
            let h = self.rect_size.y;
            sdf.box(0. * w, 0.35 * h, 0.87 * w, 0.39 * h, 0.75);
            sdf.box(0. * w, 0.28 * h, 0.5 * w, 0.3 * h, 1.);
            sdf.union();
            return sdf.fill(#80);
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
            apply: {
                hover: 0.0,
                bg_quad: {hover: (hover)}
                name_text: {hover: (hover)}
                icon_quad: {hover: (hover)}
            }
        }
        
        hover_state: {
            from: {all: Play::Forward {duration: 0.1}}
            apply: {hover: [{time: 0.0, value: 1.0}]},
        }
        
        unselected_state: {
            track: select,
            from: {all: Play::Forward {duration: 0.1, redraw: true}}
            apply: {
                selected: 0.0,
                bg_quad: {selected: (selected)}
                name_text: {selected: (selected)}
                icon_quad: {selected: (selected)}
            }
        }
        
        selected_state: {
            track: select,
            from: {all: Play::Forward {duration: 0.1, redraw: true}}
            apply: {
                selected: [{time: 0.0, value: 1.0}],
            }
        }
        
        closed_state: {
            track: open,
            from: {all: Play::Forward {duration: 0.3, redraw: true}}
            apply: {
                opened: [{value: 0.0, ease: Ease::OutExp}],
                bg_quad: {opened: (opened)}
                name_text: {opened: (opened)}
                icon_quad: {opened: (opened)}
            }
        }
        
        opened_state: {
            track: open,
            from: {all: Play::Forward {duration: 0.3, redraw: true}}
            apply: {
                opened: [{value: 1.0, ease: Ease::OutExp}],
            }
        }
        is_folder:false,
        indent_width.{name:"hello world",min:1.0,max:20.0}: 10.0
        min_drag_distance: 10.0
    }
    
    FileTree: {{FileTree}} {
        node_height: 20.0,
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
    
    #[default_state(default_state, unselected_state, closed_state)]
    animator: Animator,
    
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
    
    filler_quad: DrawBgQuad,
    
    node_height: f32,
    
    #[rust] dragging_node_id: Option<FileNodeId>,
    #[rust] selected_node_id: Option<FileNodeId>,
    #[rust] open_nodes: HashSet<FileNodeId>,
    #[rust] visible_nodes: HashSet<FileNodeId>,
    #[rust] gc_nodes: HashSet<FileNodeId>,
    #[rust] tree_nodes: HashMap<FileNodeId, FileTreeNode>,
    #[rust] count: usize,
    #[rust] stack: Vec<f32>,
}

pub enum FileTreeAction {
    WasClicked(FileNodeId),
    ShouldStartDragging(FileNodeId),
}

pub enum FileTreeNodeAction {
    None,
    WasClicked,
    Opening,
    Closing,
    ShouldStartDragging,
}

impl FileTreeNode {
    pub fn set_draw_state(&mut self, is_even: f32, scale: f32) {
        self.bg_quad.scale = scale;
        self.bg_quad.is_even = is_even;
        self.name_text.scale = scale;
        self.name_text.is_even = is_even;
        self.icon_quad.scale = scale;
        self.icon_quad.is_even = is_even;
        self.name_text.font_scale = scale;
    }
    
    pub fn draw_folder(&mut self, cx: &mut Cx, name: &str, is_even: f32, node_height: f32, depth:usize, scale:f32) {
        self.set_draw_state(is_even, scale);
        
        self.layout.walk.height = Height::Fixed(scale * node_height);
        
        self.bg_quad.begin(cx, self.layout);
        
        cx.walk_turtle(self.indent_walk(depth));
        
        self.icon_quad.draw_walk(cx, self.icon_walk);
        cx.turtle_align_y();
        
        self.name_text.draw_walk(cx, name);
        self.bg_quad.end(cx);
    }
    
    pub fn draw_file(&mut self, cx: &mut Cx, name: &str, is_even: f32, node_height: f32, depth:usize, scale:f32) {
        self.set_draw_state(is_even, scale);
        
        self.layout.walk.height = Height::Fixed(scale * node_height);
        self.bg_quad.begin(cx, self.layout);
        
        cx.walk_turtle(self.indent_walk(depth));
        cx.turtle_align_y();
        
        self.name_text.draw_walk(cx, name);
        self.bg_quad.end(cx);
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
            self.selected_state.unwrap(),
            self.unselected_state.unwrap()
        )
    }
    
    pub fn set_folder_is_open(
        &mut self,
        cx: &mut Cx,
        is_open: bool,
        should_animate: bool,
    ) {
        self.toggle_animator(
            cx,
            is_open,
            should_animate,
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
        match event.hits(cx, self.bg_quad.draw_vars.area) {
            HitEvent::FingerHover(f) => {
                cx.set_hover_mouse_cursor(MouseCursor::Hand);
                match f.hover_state {
                    HoverState::In => {
                        self.animate_to(cx, self.hover_state.unwrap());
                    }
                    HoverState::Out => {
                        self.animate_to(cx, self.default_state.unwrap());
                    }
                    _ => {}
                }
            }
            HitEvent::FingerMove(f) => {
                if f.abs.distance(&f.abs_start) >= self.min_drag_distance {
                    dispatch_action(cx, FileTreeNodeAction::ShouldStartDragging);
                }
            }
            HitEvent::FingerDown(_) => {
                self.animate_to(cx, self.selected_state.unwrap());
                if self.is_folder {
                    if self.opened > 0.2 {
                        self.animate_to(cx, self.closed_state.unwrap());
                        dispatch_action(cx, FileTreeNodeAction::Closing);
                    }
                    else {
                        self.animate_to(cx, self.opened_state.unwrap());
                        dispatch_action(cx, FileTreeNodeAction::Opening);
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
        self.visible_nodes.clear();
        self.count = 0;
        Ok(())
    }
    
    pub fn end(&mut self, cx: &mut Cx) {
        // lets fill the space left with blanks
        let height_left = cx.get_height_left();
        let mut walk = 0.0;
        while walk < height_left {
            self.count += 1;
            self.filler_quad.is_even = Self::is_even(self.count);
            self.filler_quad.draw_walk(cx, Walk {
                width: Width::Filled,
                height: Height::Fixed(self.node_height.min(height_left - walk)),
                margin: Margin::default()
            });
            walk += self.node_height;
        }
        
        self.scroll_view.end(cx);
        
        // remove all nodes that are invisible
        self.gc_nodes.clear();
        for (node_id, _) in &self.tree_nodes {
            if !self.visible_nodes.contains(node_id) && Some(*node_id) != self.selected_node_id {
                self.gc_nodes.insert(*node_id);
            }
        }
        for node_id in &self.gc_nodes {
            self.tree_nodes.remove(node_id);
        }
    }
    
    pub fn is_even(count: usize) -> f32 {
        if count % 2 == 1 {0.0}else {1.0}
    }
    
    pub fn should_node_draw(&mut self, cx: &mut Cx) -> bool {
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        let height = self.node_height * scale;
        if scale > 0.01 && cx.turtle_line_is_visible(height, self.scroll_view.get_scroll_pos(cx)) {
            return true
        }
        else {
            cx.walk_turtle(Walk {
                width: Width::Filled,
                height: Height::Fixed(height),
                margin: Margin::default()
            });
            return false
        }
    }
    
    pub fn begin_folder(
        &mut self,
        cx: &mut Cx,
        node_id: FileNodeId,
        name: &str,
    ) -> Result<(), ()> {
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        
        if scale > 0.2{
            self.count += 1;
        }
        
        let is_open = self.open_nodes.contains(&node_id);
        
        if self.should_node_draw(cx) {
            self.visible_nodes.insert(node_id);
            let tree_node = match self.tree_nodes.entry(node_id) {
                Entry::Occupied(o) => o.into_mut(),
                Entry::Vacant(v) => v.insert({
                    let mut tree_node = FileTreeNode::new_from_ptr(cx, self.folder_node.unwrap());
                    if is_open {
                        tree_node.set_folder_is_open(cx, true, false)
                    }
                    tree_node
                })
            };
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
    }
    
    pub fn end_folder(&mut self) {
        self.stack.pop();
    }
    
    pub fn file(&mut self, cx: &mut Cx, node_id: FileNodeId, name: &str) {
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        
        if scale > 0.2{
            self.count += 1;
        }
        if self.should_node_draw(cx) {
            self.visible_nodes.insert(node_id);
            let tree_node = match self.tree_nodes.entry(node_id) {
                Entry::Occupied(o) => o.into_mut(),
                Entry::Vacant(v) => v.insert(FileTreeNode::new_from_ptr(cx, self.file_node.unwrap()))
            };
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
        if let Some(tree_node) = self.tree_nodes.get_mut(&node_id) {
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
        
        match event {
            Event::DragEnd => self.dragging_node_id = None,
            _ => ()
        }
        
        let mut actions = Vec::new();
        for (node_id, node) in &mut self.tree_nodes {
            node.handle_event(cx, event, &mut | _, e | actions.push((*node_id, e)));
        }
        
        for (node_id, action) in actions {
            match action {
                FileTreeNodeAction::Opening => {
                    self.open_nodes.insert(node_id);
                }
                FileTreeNodeAction::Closing => {
                    self.open_nodes.remove(&node_id);
                }
                FileTreeNodeAction::WasClicked => {
                    if let Some(last_selected) = self.selected_node_id {
                        if last_selected != node_id {
                            self.tree_nodes.get_mut(&last_selected).unwrap().set_is_selected(cx, false, true);
                        }
                    }
                    self.selected_node_id = Some(node_id);
                    dispatch_action(cx, FileTreeAction::WasClicked(node_id));
                }
                FileTreeNodeAction::ShouldStartDragging => {
                    if self.dragging_node_id.is_none() {
                        dispatch_action(cx, FileTreeAction::ShouldStartDragging(node_id));
                    }
                }
                _ => ()
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

