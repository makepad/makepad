use {
    crate::{
        id::GenId,
        tree_logic::{self, NodeId, TreeLogic},
    },
    makepad_render::*,
    makepad_widget::*,
};

live_register!{
    use makepad_render::shader_std::*;
    FileTree: {{FileTree}} {
        
        folder_icon:{
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
        
        file_node_name:{
            text_style:{
                top_drop: 1.3,
            }
        }
        
        indent_width: 10.0
        file_node_height: 20.0
        
        file_node_color_even: #25
        file_node_color_odd: #28
        file_node_color_selected: #x11466E
        file_node_color_hovered_even: #3D
        file_node_color_hovered_odd: #38
        file_node_color_hovered_selected: #x11466E
        
        folder_icon_color: #80
        folder_icon_width: 10.0
        
        file_node_name_color_folder: #FF
        file_node_name_color_file: #9D
        view:{view:{debug_id:file_tree_view}}
        folder_icon_walk: Walk {
            width: Width::Fixed(10.0),
            height: Height::Filled,
            margin: Margin {
                l: 1.0,
                t: 0.0,
                r: 4.0,
                b: 0.0,
            },
        }
    }
}

#[derive(LiveComponent, LiveApply, LiveCast)]
pub struct FileTree {
    #[live] view: ScrollView,
    #[rust] logic: TreeLogic,

    #[live] file_node: DrawColor,
    #[live] folder_icon: DrawColor,
    #[live] file_node_name: DrawText,
    
    #[live] file_node_color_even: Vec4,
    #[live] file_node_color_odd: Vec4,
    #[live] file_node_color_selected: Vec4,
    #[live] file_node_color_hovered_even: Vec4,
    #[live] file_node_color_hovered_odd: Vec4,
    #[live] file_node_color_hovered_selected: Vec4,

    #[live] folder_icon_color: Vec4,
    #[live] folder_icon_width: f32,

    #[live] file_node_height: f32,
    #[live] indent_width: f32,
    #[live] folder_icon_walk: Walk,

    #[live] file_node_name_color_folder: Vec4,
    #[live] file_node_name_color_file: Vec4,
    #[rust] count: usize,
    #[rust] stack: Vec<f32>,
}

impl FileTree {

    pub fn begin(&mut self, cx: &mut Cx) -> Result<(), ()> {
        self.view.begin_view(cx)?;
        self.count = 0;
        self.logic.begin();
        Ok(())
    }

    pub fn end(&mut self, cx: &mut Cx) {
        self.logic.end();
        self.view.end_view(cx);
    }

    pub fn begin_folder(
        &mut self,
        cx: &mut Cx,
        file_node_id: FileNodeId,
        name: &str,
    ) -> Result<(), ()> {
        let info = self.logic.begin_node(file_node_id.0);
        let scale = self.stack.last().cloned().unwrap_or(1.0);

        let count = self.count;
        self.count += 1;

        self.file_node.draw_vars.area = Area::Empty;
        self.file_node.color = self.file_node_color(count, info.is_hovered, info.is_selected);
        let layout = self.file_node_layout(scale);
        self.file_node.begin_quad(cx, layout);

        cx.walk_turtle(self.indent_walk(self.stack.len()));

        self.folder_icon.draw_quad_walk(cx, self.folder_icon_walk);
        cx.turtle_align_y();

        self.file_node_name.color = self.file_node_name_color_folder;
        self.file_node_name.font_scale = self.stack.last().cloned().unwrap_or(1.0);
        self.file_node_name.draw_text_walk(cx, name);
        self.file_node.end_quad(cx);

        self.logic
            .set_node_area(cx, file_node_id.0, self.file_node.draw_vars.area);

        cx.turtle_new_line();

        self.stack.push(scale * info.is_expanded_fraction);

        if info.is_fully_collapsed() {
            self.end_folder();
            return Err(());
        }
        Ok(())
    }

    pub fn end_folder(&mut self) {
        self.stack.pop();
        self.logic.end_node();
    }

    pub fn file(&mut self, cx: &mut Cx, file_node_id: FileNodeId, name: &str) {
        let info = self.logic.begin_node(file_node_id.0);
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        let count = self.count;
        self.count += 1;

        self.file_node.draw_vars.area = Area::Empty;
        self.file_node.color = self.file_node_color(count, info.is_hovered, info.is_selected);
        let layout = self.file_node_layout(scale);
        self.file_node.begin_quad(cx, layout);

        cx.walk_turtle(self.indent_walk(self.stack.len()));
        cx.turtle_align_y();

        self.file_node_name.color = self.file_node_name_color_file;
        self.file_node_name.font_scale = scale;
        self.file_node_name.draw_text_walk(cx, name);
        self.file_node.end_quad(cx);

        self.logic
            .set_node_area(cx, file_node_id.0, self.file_node.draw_vars.area);

        cx.turtle_new_line();
        self.logic.end_node();
    }

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
            align: Align { fx: 0.0, fy: 0.5 },
            padding: Padding {
                l: 5.0,
                t: 0.0,
                r: 0.0,
                b: 1.0,
            },
            ..Layout::default()
        }
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

    pub fn forget(&mut self) {
        self.logic.forget();
    }

    pub fn forget_node(&mut self, file_node_id: FileNodeId) {
        self.logic.forget_node(file_node_id.0);
    }

    pub fn file_node_is_expanded(&mut self, file_node_id: FileNodeId) -> bool {
        self.logic.node_is_expanded(file_node_id.0)
    }

    pub fn set_file_node_is_expanded(
        &mut self,
        cx: &mut Cx,
        file_node_id: FileNodeId,
        is_open: bool,
        should_animate: bool,
    ) {
        if self
            .logic
            .set_node_is_expanded(cx, file_node_id.0, is_open, should_animate)
        {
            self.view.redraw_view(cx);
        }
    }

    pub fn toggle_file_node_is_expanded(
        &mut self,
        cx: &mut Cx,
        file_node_id: FileNodeId,
        should_animate: bool,
    ) {
        if self
            .logic
            .toggle_node_is_expanded(cx, file_node_id.0, should_animate)
        {
            self.view.redraw_view(cx);
        }
    }

    pub fn set_hovered_file_node_id(&mut self, cx: &mut Cx, file_node_id: Option<FileNodeId>) {
        if self
            .logic
            .set_hovered_node_id(file_node_id.map(|file_node_id| file_node_id.0))
        {
            self.view.redraw_view(cx);
        }
    }

    pub fn set_selected_file_node_id(&mut self, cx: &mut Cx, file_node_id: FileNodeId) {
        if self.logic.set_selected_node_id(file_node_id.0) {
            self.view.redraw_view(cx);
        }
    }

    pub fn start_dragging_file_node(
        &mut self,
        cx: &mut Cx,
        file_node_id: FileNodeId,
        dragged_item: DraggedItem,
    ) {
        self.logic
            .start_dragging_node(cx, file_node_id.0, dragged_item);
    }

    pub fn redraw(&mut self, cx: &mut Cx) {
        self.view.redraw_view(cx);
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, Action),
    ) {
        if self.view.handle_scroll_view(cx, event) {
            self.view.redraw_view(cx);
        }
        let mut actions = Vec::new();
        self.logic
            .handle_event(cx, event, &mut |action| actions.push(action));
        for action in actions {
            match action {
                tree_logic::Action::TreeWasAnimated => {
                    self.redraw(cx);
                }
                tree_logic::Action::NodeWasEntered(node_id) => {
                    let file_node_id = FileNodeId(node_id);
                    self.set_hovered_file_node_id(cx, Some(file_node_id));
                }
                tree_logic::Action::NodeWasExited(node_id) => {
                    if self.logic.hovered_node_id() == Some(node_id) {
                        self.set_hovered_file_node_id(cx, None);
                    }
                }
                tree_logic::Action::NodeWasClicked(node_id) => {
                    let file_node_id = FileNodeId(node_id);
                    self.toggle_file_node_is_expanded(cx, file_node_id, true);
                    self.set_selected_file_node_id(cx, file_node_id);
                    dispatch_action(cx, Action::FileNodeWasClicked(file_node_id));
                }
                tree_logic::Action::NodeShouldStartDragging(node_id) => {
                    let file_node_id = FileNodeId(node_id);
                    dispatch_action(cx, Action::FileNodeShouldStartDragging(file_node_id));
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FileNodeId(pub NodeId);

impl AsRef<GenId> for FileNodeId {
    fn as_ref(&self) -> &GenId {
        self.0.as_ref()
    }
}

pub enum Action {
    FileNodeWasClicked(FileNodeId),
    FileNodeShouldStartDragging(FileNodeId),
}
