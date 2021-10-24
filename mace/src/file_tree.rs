use {
    crate::{
        id::Id,
        tree_logic::{self, NodeId, TreeLogic},
    },
    makepad_render::*,
    makepad_widget::*,
};

pub struct FileTree {
    view: ScrollView,
    logic: TreeLogic,
    file_node: DrawColor,
    file_node_height: f32,
    file_node_color_even: Vec4,
    file_node_color_odd: Vec4,
    file_node_color_selected: Vec4,
    file_node_color_hovered_even: Vec4,
    file_node_color_hovered_odd: Vec4,
    file_node_color_hovered_selected: Vec4,
    indent_width: f32,
    folder_icon: DrawColor,
    folder_icon_walk: Walk,
    file_node_name: DrawText,
    file_node_name_color_folder: Vec4,
    file_node_name_color_file: Vec4,
    count: usize,
    stack: Vec<f32>,
}

impl FileTree {
    pub fn style(cx: &mut Cx) {
        live_body!(cx, {
            self::folder_icon_shader: Shader {
                use makepad_render::drawcolor::shader::*;

                draw_input: makepad_render::drawcolor::DrawColor;

                fn pixel() -> vec4 {
                    let df = Df::viewport(pos * rect_size);
                    let w = rect_size.x;
                    let h = rect_size.y;
                    df.box(0. * w, 0.35 * h, 0.87 * w, 0.39 * h, 0.75);
                    df.box(0. * w, 0.28 * h, 0.5 * w, 0.3 * h, 1.);
                    df.union();
                    return df.fill(color);
                }
            }

            self::file_node_height: 20.0;
            self::file_node_color_even: #25;
            self::file_node_color_odd: #28;
            self::file_node_color_selected: #x11466E;
            self::file_node_color_hovered_even: #3D;
            self::file_node_color_hovered_odd: #38;
            self::file_node_color_hovered_selected: #x11466E;
            self::indent_width: 10.0;
            self::folder_icon_width: 10.0;
            self::folder_icon_color: #80;
            self::file_node_name_text_style: TextStyle {
                top_drop: 1.3,
                ..makepad_widget::widgetstyle::text_style_normal
            }
            self::file_node_name_color_folder: #FF;
            self::file_node_name_color_file: #9D;
        })
    }

    pub fn new(cx: &mut Cx) -> FileTree {
        FileTree {
            view: ScrollView::new_standard_hv(cx),
            logic: TreeLogic::new(),
            file_node: DrawColor::new(cx, default_shader!()),
            file_node_height: 0.0,
            file_node_color_even: Vec4::default(),
            file_node_color_odd: Vec4::default(),
            file_node_color_selected: Vec4::default(),
            file_node_color_hovered_even: Vec4::default(),
            file_node_color_hovered_odd: Vec4::default(),
            file_node_color_hovered_selected: Vec4::default(),
            indent_width: 0.0,
            folder_icon: DrawColor::new(cx, live_shader!(cx, self::folder_icon_shader)),
            folder_icon_walk: Walk::default(),
            file_node_name: DrawText::new(cx, default_shader!()),
            file_node_name_color_folder: Vec4::default(),
            file_node_name_color_file: Vec4::default(),
            count: 0,
            stack: Vec::new(),
        }
    }

    pub fn begin(&mut self, cx: &mut Cx) -> Result<(), ()> {
        self.view.begin_view(cx, Layout::default())?;
        self.apply_style(cx);
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
        self.file_node.set_area(Area::Empty);
        self.file_node.color = self.file_node_color(count, info.is_hovered, info.is_selected);
        self.file_node.begin_quad(cx, self.file_node_layout(scale));
        cx.walk_turtle(self.indent_walk(self.stack.len()));
        self.folder_icon.draw_quad_walk(cx, self.folder_icon_walk);
        cx.turtle_align_y();
        self.file_node_name.color = self.file_node_name_color_folder;
        self.file_node_name.font_scale = self.stack.last().cloned().unwrap_or(1.0);
        self.file_node_name.draw_text_walk(cx, name);
        self.file_node.end_quad(cx);
        self.logic
            .set_node_area(cx, file_node_id.0, self.file_node.area());
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
        self.file_node.set_area(Area::Empty);
        self.file_node.color = self.file_node_color(count, info.is_hovered, info.is_selected);
        self.file_node.begin_quad(cx, self.file_node_layout(scale));
        cx.walk_turtle(self.indent_walk(self.stack.len()));
        cx.turtle_align_y();
        self.file_node_name.color = self.file_node_name_color_file;
        self.file_node_name.font_scale = scale;
        self.file_node_name.draw_text_walk(cx, name);
        self.file_node.end_quad(cx);
        self.logic
            .set_node_area(cx, file_node_id.0, self.file_node.area());
        cx.turtle_new_line();
        self.logic.end_node();
    }

    fn apply_style(&mut self, cx: &mut Cx) {
        self.file_node_height = live_float!(cx, self::file_node_height);
        self.file_node_color_even = live_vec4!(cx, self::file_node_color_even);
        self.file_node_color_odd = live_vec4!(cx, self::file_node_color_odd);
        self.file_node_color_selected = live_vec4!(cx, self::file_node_color_selected);
        self.file_node_color_hovered_even = live_vec4!(cx, self::file_node_color_hovered_even);
        self.file_node_color_hovered_odd = live_vec4!(cx, self::file_node_color_hovered_odd);
        self.file_node_color_hovered_selected =
            live_vec4!(cx, self::file_node_color_hovered_selected);
        self.indent_width = live_float!(cx, self::indent_width);
        self.folder_icon_walk = Walk {
            width: Width::Fix(live_float!(cx, self::folder_icon_width)),
            height: Height::Fill,
            margin: Margin {
                l: 1.0,
                t: 0.0,
                r: 4.0,
                b: 0.0,
            },
        };
        self.folder_icon.color = live_vec4!(cx, self::folder_icon_color);
        self.file_node_name.text_style = live_text_style!(cx, self::file_node_name_text_style);
        self.file_node_name_color_folder = live_vec4!(cx, self::file_node_name_color_folder);
        self.file_node_name_color_file = live_vec4!(cx, self::file_node_name_color_file);
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
                width: Width::Fill,
                height: Height::Fix(scale * self.file_node_height),
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
            width: Width::Fix(depth as f32 * self.indent_width),
            height: Height::Fill,
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

    pub fn start_drag_file_node(&mut self, cx: &mut Cx, file_node_id: FileNodeId, drag_item: DragItem) {
        self.logic.start_drag_node(cx, file_node_id.0, drag_item);
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
                tree_logic::Action::NodeWasPressed(node_id) => {
                    let file_node_id = FileNodeId(node_id);
                    self.toggle_file_node_is_expanded(cx, file_node_id, true);
                    self.set_selected_file_node_id(cx, file_node_id);
                    dispatch_action(cx, Action::FileNodeWasPressed(file_node_id));
                }
                tree_logic::Action::NodeShouldStartDrag(node_id) => {
                    let file_node_id = FileNodeId(node_id);
                    dispatch_action(cx, Action::FileNodeShouldStartDrag(file_node_id));
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FileNodeId(pub NodeId);

impl AsRef<Id> for FileNodeId {
    fn as_ref(&self) -> &Id {
        self.0.as_ref()
    }
}

pub enum Action {
    FileNodeWasPressed(FileNodeId),
    FileNodeShouldStartDrag(FileNodeId),
}
