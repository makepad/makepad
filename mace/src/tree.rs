use {
    crate::tree_logic::{self, NodeId, TreeLogic},
    makepad_render::*,
    makepad_widget::*,
};

pub struct Tree {
    view: ScrollView,
    logic: TreeLogic,
    node: DrawColor,
    node_height: f32,
    node_color_even: Vec4,
    node_color_odd: Vec4,
    node_color_selected: Vec4,
    node_color_hovered_even: Vec4,
    node_color_hovered_odd: Vec4,
    node_color_hovered_selected: Vec4,
    indent_width: f32,
    folder_icon: DrawColor,
    folder_icon_walk: Walk,
    node_name: DrawText,
    node_name_color_folder: Vec4,
    node_name_color_file: Vec4,
    count: usize,
    stack: Vec<f32>,
}

impl Tree {
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

            self::node_height: 20.0;
            self::node_color_even: #25;
            self::node_color_odd: #28;
            self::node_color_selected: #x11466E;
            self::node_color_hovered_even: #3D;
            self::node_color_hovered_odd: #38;
            self::node_color_hovered_selected: #x11466E;
            self::indent_width: 10.0;
            self::folder_icon_width: 10.0;
            self::folder_icon_color: #80;
            self::node_name_text_style: TextStyle {
                top_drop: 1.3,
                ..makepad_widget::widgetstyle::text_style_normal
            }
            self::node_name_color_folder: #FF;
            self::node_name_color_file: #9D;
        })
    }

    pub fn new(cx: &mut Cx) -> Tree {
        Tree {
            view: ScrollView::new_standard_hv(cx),
            logic: TreeLogic::new(),
            node: DrawColor::new(cx, default_shader!()),
            node_height: 0.0,
            node_color_even: Vec4::default(),
            node_color_odd: Vec4::default(),
            node_color_selected: Vec4::default(),
            node_color_hovered_even: Vec4::default(),
            node_color_hovered_odd: Vec4::default(),
            node_color_hovered_selected: Vec4::default(),
            indent_width: 0.0,
            folder_icon: DrawColor::new(cx, live_shader!(cx, self::folder_icon_shader)),
            folder_icon_walk: Walk::default(),
            node_name: DrawText::new(cx, default_shader!()),
            node_name_color_folder: Vec4::default(),
            node_name_color_file: Vec4::default(),
            count: 0,
            stack: Vec::new(),
        }
    }

    pub fn begin(&mut self, cx: &mut Cx) -> Result<(), ()> {
        println!("BEGIN FILE TREE");
        self.view.begin_view(cx, Layout::default())?;
        self.apply_style(cx);
        self.count = 0;
        self.logic.begin();
        Ok(())
    }

    pub fn end(&mut self, cx: &mut Cx) {
        println!("END FILE TREE");
        println!();
        self.logic.end();
        self.view.end_view(cx);
    }

    fn apply_style(&mut self, cx: &mut Cx) {
        self.node_height = live_float!(cx, self::node_height);
        self.node_color_even = live_vec4!(cx, self::node_color_even);
        self.node_color_odd = live_vec4!(cx, self::node_color_odd);
        self.node_color_selected = live_vec4!(cx, self::node_color_selected);
        self.node_color_hovered_even = live_vec4!(cx, self::node_color_hovered_even);
        self.node_color_hovered_odd = live_vec4!(cx, self::node_color_hovered_odd);
        self.node_color_hovered_selected = live_vec4!(cx, self::node_color_hovered_selected);
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
        self.node_name.text_style = live_text_style!(cx, self::node_name_text_style);
        self.node_name_color_folder = live_vec4!(cx, self::node_name_color_folder);
        self.node_name_color_file = live_vec4!(cx, self::node_name_color_file);
    }

    pub fn begin_branch(&mut self, cx: &mut Cx, node_id: NodeId, name: &str) -> Result<(), ()> {
        let info = self.logic.begin_node(node_id);
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        let count = self.count;
        self.count += 1;
        self.node.color = self.node_color(count, info.is_hovered, info.is_selected);
        self.node.begin_quad(cx, self.node_layout(scale));
        cx.walk_turtle(self.indent_walk(self.stack.len()));
        self.folder_icon.draw_quad_walk(cx, self.folder_icon_walk);
        cx.turtle_align_y();
        self.node_name.color = self.node_name_color_folder;
        self.node_name.font_scale = self.stack.last().cloned().unwrap_or(1.0);
        self.node_name.draw_text_walk(cx, name);
        self.node.end_quad(cx);
        self.logic.set_node_area(node_id, self.node.area());
        cx.turtle_new_line();
        self.stack.push(scale * info.is_expanded_fraction);
        if info.is_fully_collapsed() {
            self.end_branch();
            return Err(());
        }
        Ok(())
    }

    pub fn end_branch(&mut self) {
        self.stack.pop();
        self.logic.end_node();
    }

    pub fn leaf(&mut self, cx: &mut Cx, node_id: NodeId, name: &str) {
        let info = self.logic.begin_node(node_id);
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        let count = self.count;
        self.count += 1;
        self.node.color = self.node_color(count, info.is_hovered, info.is_selected);
        self.node.begin_quad(cx, self.node_layout(scale));
        cx.walk_turtle(self.indent_walk(self.stack.len()));
        cx.turtle_align_y();
        self.node_name.color = self.node_name_color_file;
        self.node_name.font_scale = scale;
        self.node_name.draw_text_walk(cx, name);
        self.node.end_quad(cx);
        self.logic.set_node_area(node_id, self.node.area());
        cx.turtle_new_line();
        self.logic.end_node();
    }

    fn node_color(&self, count: usize, is_hovered: bool, is_selected: bool) -> Vec4 {
        if is_hovered {
            if is_selected {
                self.node_color_hovered_selected
            } else if count % 2 == 0 {
                self.node_color_hovered_even
            } else {
                self.node_color_hovered_odd
            }
        } else {
            if is_selected {
                self.node_color_selected
            } else if count % 2 == 0 {
                self.node_color_even
            } else {
                self.node_color_odd
            }
        }
    }

    fn node_layout(&self, scale: f32) -> Layout {
        Layout {
            walk: Walk {
                width: Width::Fill,
                height: Height::Fix(scale * self.node_height),
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

    pub fn node_is_expanded(&mut self, node_id: NodeId) -> bool {
        self.logic.node_is_expanded(node_id)
    }

    pub fn set_node_is_expanded(&mut self, cx: &mut Cx, node_id: NodeId, is_open: bool, should_animate: bool) {
        if self.logic.set_node_is_expanded(cx, node_id, is_open, should_animate) {
            self.view.redraw_view(cx);
        }
    }

    pub fn toggle_node_is_expanded(&mut self, cx: &mut Cx, node_id: NodeId, should_animate: bool) {
        if self.logic.toggle_node_is_expanded(cx, node_id, should_animate) {
            self.view.redraw_view(cx);
        }
    }

    pub fn set_hovered_node_id(&mut self, cx: &mut Cx, node_id: Option<NodeId>) {
        if self.logic.set_hovered_node_id(node_id) {
            self.view.redraw_view(cx);
        }
    }

    pub fn set_selected_node_id(&mut self, cx: &mut Cx, node_id: NodeId) {
        if self.logic.set_selected_node_id(node_id) {
            self.view.redraw_view(cx);
        }
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) {
        if self.view.handle_scroll_view(cx, event) {
            self.view.redraw_view(cx);
        }
        let mut actions = Vec::new();
        self.logic.handle_event(cx, event, &mut |action| actions.push(action));
        for action in actions {
            match action {
                tree_logic::Action::ToggleNodeIsExpanded(node_id, should_animate) => {
                    self.toggle_node_is_expanded(cx, node_id, should_animate)
                }
                tree_logic::Action::SetHoveredNodeId(node_id) => {
                    self.set_hovered_node_id(cx, node_id);
                }
                tree_logic::Action::SetSelectedNodeId(node_id) => {
                    self.set_selected_node_id(cx, node_id);
                }
                tree_logic::Action::Redraw => {
                    self.view.redraw_view(cx);
                }
            }
        }
    }
}
