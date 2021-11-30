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
    
    FileTreeNode: {{FileTreeNode}} { 
        folder_quad: {
            color: #80
            
            instance even: float = 1.0
            instance selected: float = 0.0
            instance hover: float = 0.0
            
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
            text_style: {
                top_drop: 1.3,
            }
        }
        
        layout: {
            walk: {
                width: Width::Filled,
                height: Height::Fixed(10.0),
            },
            align: {fx: 0.0, fy: 0.5},
            padding: {l: 5.0, t: 0.0, r: 0.0, b: 1.0,},
        }
        
        folder_icon_walk: Walk {
            width: Width::Fixed(14.0),
            height: Height::Filled,
            margin: Margin {
                l: 1.0,
                t: 0.0,
                r: 4.0,
                b: 0.0,
            },
        }
        
        indent_width: 10.0
        file_node_height: 20.0
        file_node_color_even: #25
        file_node_color_odd: #28
        file_node_color_selected: #x11466E
        file_node_color_hovered_even: #3D
        file_node_color_hovered_odd: #38
        file_node_color_hovered_selected: #x11466E
        file_name_color_folder: #FF
        file_name_color_file: #9D
    }
    
    FileTree: {{FileTree}} {
        
        tree_node: FileTreeNode {}
        
        scroll_view: {
            view: {debug_id: file_tree_view}
        }
    }
}


#[derive(Live, LiveHook)]
pub struct FileTreeNode {
    #[live] node_quad: DrawColor,
    #[live] folder_quad: DrawColor,
    #[live] name_text: DrawText,
    
    #[live] layout: Layout,
    
    #[live] file_node_height: f32,
    #[live] indent_width: f32,
    
    #[live] folder_icon_walk: Walk,
    
    #[live] file_node_color_even: Vec4,
    #[live] file_node_color_odd: Vec4,
    #[live] file_node_color_selected: Vec4,
    #[live] file_node_color_hovered_even: Vec4,
    #[live] file_node_color_hovered_odd: Vec4,
    #[live] file_node_color_hovered_selected: Vec4,
    #[live] file_name_color_folder: Vec4,
    #[live] file_name_color_file: Vec4,
}

#[derive(Live, LiveHook)]
pub struct FileTree {
    #[live] scroll_view: ScrollView,
    //#[rust] logic: TreeLogic,
    
    #[live] tree_node: Option<LivePtr>,
    #[rust] tree_nodes: HashMap<FileNodeId, FileTreeNode>,
    
    #[rust] count: usize,
    #[rust] stack: Vec<f32>,
}

impl FileTreeNode {
    pub fn draw_folder(&mut self, cx: &mut Cx, name: &str, stack: &[f32]) {
        let scale = stack.last().cloned().unwrap_or(1.0);
        
        //self.node_quad.draw_vars.area = Area::Empty;
        //self.node_quad.color = self.file_node_color(count, info.is_hovered, info.is_selected);
        //let layout = self.file_node_layout(scale);
        
        self.layout.walk.height = Height::Fixed(scale * self.file_node_height);
        
        self.node_quad.begin(cx, self.layout);
        
        cx.walk_turtle(self.indent_walk(stack.len()));
        
        self.folder_quad.draw_walk(cx, self.folder_icon_walk);
        cx.turtle_align_y();
        
        self.name_text.color = self.file_name_color_folder;
        self.name_text.font_scale = stack.last().cloned().unwrap_or(1.0);
        self.name_text.draw_walk(cx, name);
        self.node_quad.end(cx);
        
        cx.turtle_new_line();
        
        //stack.push(scale * info.is_expanded_fraction);
        /*
        if info.is_fully_collapsed() {
            self.end_folder();
            return Err(());
        }*/
        //Ok(())
    }
    
    pub fn draw_file(&mut self, cx: &mut Cx, name: &str, stack: &[f32]) {
        
        let scale = stack.last().cloned().unwrap_or(1.0);
        //let count = self.count;
        //self.count += 1;
        
        //self.node_quad.draw_vars.area = Area::Empty;
        //self.node_quad.color = self.file_node_color(count, info.is_hovered, info.is_selected);
        //let layout = self.file_node_layout(scale);
        self.layout.walk.height = Height::Fixed(scale * self.file_node_height);
        
        self.node_quad.begin(cx, self.layout);
        
        cx.walk_turtle(self.indent_walk(stack.len()));
        cx.turtle_align_y();
        
        self.name_text.color = self.file_name_color_file;
        self.name_text.font_scale = scale;
        self.name_text.draw_walk(cx, name);
        self.node_quad.end(cx);
        
        //self.logic
        //    .set_node_area(cx, file_node_id.0, self.node_quad.draw_vars.area);
        
        cx.turtle_new_line();
        //self.logic.end_node();
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
    
}


impl FileTree {
    
    pub fn begin(&mut self, cx: &mut Cx) -> Result<(), ()> {
        self.scroll_view.begin(cx) ?;
        self.count = 0;
        //println!("{}", std::mem::size_of::<FileTreeNode>());
        //println!("{}", self.tree_nodes.len() * std::mem::size_of::<FileTreeNode>());
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
        
        //let count = self.count;
        self.count += 1;
        
        let tree_node = match self.tree_nodes.entry(node_id) {
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => v.insert(FileTreeNode::new_from_ptr(cx, self.tree_node.unwrap()))
        };
        
        tree_node.draw_folder(cx, name, &self.stack);
        /*
        tree_node.begin_folder(cx, name);
        /*
        self.node_quad.draw_vars.area = Area::Empty;
        self.node_quad.color = self.file_node_color(count, info.is_hovered, info.is_selected);
        let layout = self.file_node_layout(scale);
        self.node_quad.begin(cx, layout);
        */
        cx.walk_turtle(self.indent_walk(self.stack.len()));
        
        self.folder_quad.draw_walk(cx, self.folder_icon_walk);
        cx.turtle_align_y();
        
        self.name_text.color = self.file_node_name_color_folder;
        self.name_text.font_scale = self.stack.last().cloned().unwrap_or(1.0);
        self.name_text.draw_walk(cx, name);
        self.node_quad.end(cx);
        
        self.logic
            .set_node_area(cx, file_node_id.0, self.node_quad.draw_vars.area);
        
        cx.turtle_new_line();
        */
        
        self.stack.push(scale * 1.0);
        
        //if info.is_fully_collapsed() {
        //    self.end_folder();
        //    return Err(());
        // }
        Ok(())
    }
    
    pub fn end_folder(&mut self) {
        self.stack.pop();
    }
    
    pub fn file(&mut self, cx: &mut Cx, node_id: FileNodeId, name: &str) {
        let tree_node = match self.tree_nodes.entry(node_id) {
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => v.insert(FileTreeNode::new_from_ptr(cx, self.tree_node.unwrap()))
        };
        tree_node.draw_file(cx, name, &self.stack);
        cx.turtle_new_line();
    } 
    /*
    fn file_node_color(&self, count: usize, is_hovered: bool, is_selected: bool) -> Vec4 {
        if is_hovered {
            if is_selected {
                self.file_node_color_hovered_selected
            } else if count % 2 == 0 {
                self.file_node_color_hovered_even
            } else {
                self.file_node_color_hovered_odd
            }
        } else {
            if is_selected {
                self.file_node_color_selected
            } else if count % 2 == 0 {
                self.file_node_color_even
            } else {
                self.file_node_color_odd
            }
        }
    }
    
    fn file_node_layout(&self, scale: f32) -> Layout {
        Layout {
            walk: Walk {
                width: Width::Filled,
                height: Height::Fixed(scale * self.file_node_height),
                ..Walk::default()
            },
            align: Align {fx: 0.0, fy: 0.5},
            padding: Padding {
                l: 5.0,
                t: 0.0,
                r: 0.0,
                b: 1.0,
            },
            ..Layout::default()
        }
    }*/
    
    pub fn forget(&mut self) {
        self.tree_nodes.clear();
    }
    
    pub fn forget_node(&mut self, file_node_id: FileNodeId) {
        self.tree_nodes.remove(&file_node_id);
    }
    
    pub fn file_node_is_expanded(&mut self, _file_node_id: FileNodeId) -> bool {
        true
    }
    
    pub fn set_file_node_is_expanded(
        &mut self,
        _cx: &mut Cx,
        _file_node_id: FileNodeId,
        _is_open: bool,
        _should_animate: bool,
    ) {
        /*
        if self.logic.set_node_is_expanded(cx, file_node_id.0, is_open, should_animate)
        {
            self.scroll_view.redraw(cx);
        }*/
    }
    
    pub fn toggle_file_node_is_expanded(
        &mut self,
        _cx: &mut Cx,
        _file_node_id: FileNodeId,
        _should_animate: bool,
    ) {/*
        if self.logic.toggle_node_is_expanded(cx, file_node_id.0, should_animate)
        {
            self.scroll_view.redraw(cx);
        }*/
    }
    
    pub fn set_hovered_file_node_id(&mut self, _cx: &mut Cx, _file_node_id: Option<FileNodeId>) {
        /*if self.logic.set_hovered_node_id(file_node_id.map( | file_node_id | file_node_id.0))
        {
            self.scroll_view.redraw(cx);
        }*/
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


