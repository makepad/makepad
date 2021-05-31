use {
    crate::tree::{NodeId, Tree},
    makepad_render::*,
    makepad_widget::*,
};

pub struct FileTree {
    view: ScrollView,
    tree: Tree,
    node: DrawColor,
    node_height: f32,
    node_color_even: Vec4,
    node_color_odd: Vec4,
    indent_width: f32,
    folder_icon: DrawColor,
    folder_icon_walk: Walk,
    node_name: DrawText,
    node_name_color_folder: Vec4,
    node_name_color_file: Vec4,
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

            self::node_height: 20.0;
            self::node_color_even: #25;
            self::node_color_odd: #28;
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

    pub fn new(cx: &mut Cx) -> FileTree {
        FileTree {
            view: ScrollView::new_standard_hv(cx),
            tree: Tree::new(),
            node: DrawColor::new(cx, default_shader!()),
            node_height: 0.0,
            node_color_even: Vec4::default(),
            node_color_odd: Vec4::default(),
            indent_width: 0.0,
            folder_icon: DrawColor::new(cx, live_shader!(cx, self::folder_icon_shader)),
            folder_icon_walk: Walk::default(),
            node_name: DrawText::new(cx, default_shader!()),
            node_name_color_folder: Vec4::default(),
            node_name_color_file: Vec4::default(),
            stack: Vec::new(),
        }
    }

    pub fn begin(&mut self, cx: &mut Cx) -> Result<(), ()> {
        println!("BEGIN FILE TREE");
        self.apply_style(cx);
        self.node_name.text_style = live_text_style!(cx, self::node_name_text_style);
        self.view.begin_view(cx, Layout::default())?;
        self.tree.begin();
        Ok(())
    }

    pub fn end(&mut self, cx: &mut Cx) {
        println!("END FILE TREE");
        println!();
        self.tree.end();
        self.view.end_view(cx);
    }

    fn apply_style(&mut self, cx: &mut Cx) {
        self.node_height = live_float!(cx, self::node_height);
        self.node_color_even = live_vec4!(cx, self::node_color_even);
        self.node_color_odd = live_vec4!(cx, self::node_color_odd);
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
        self.node_name_color_folder = live_vec4!(cx, self::node_name_color_folder);
        self.node_name_color_file = live_vec4!(cx, self::node_name_color_file);
    }

    pub fn begin_folder(&mut self, cx: &mut Cx, node_id: NodeId, name: &str) -> Result<(), ()> {
        let info = self.tree.begin_node(node_id);
        println!("BEGIN FOLDER {:?} ({:?})", name, info.is_expanded_fraction);
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        self.node.color = self.node_color(info.count);
        self.node.begin_quad(cx, self.node_layout(scale));
        cx.walk_turtle(self.indent_walk(self.stack.len()));
        self.folder_icon.draw_quad_walk(cx, self.folder_icon_walk);
        cx.turtle_align_y();
        self.node_name.color = self.node_name_color_folder;
        self.node_name.font_scale = self.stack.last().cloned().unwrap_or(1.0);
        self.node_name.draw_text_walk(cx, name);
        self.node.end_quad(cx);
        self.tree.set_node_area(node_id, self.node.area());
        cx.turtle_new_line();
        self.stack.push(scale * info.is_expanded_fraction);
        if info.is_fully_collapsed() {
            println!("!!! FULLY COLLAPSED !!!");
            self.end_folder();
            return Err(());
        }
        Ok(())
    }

    pub fn end_folder(&mut self) {
        println!("END FOLDER");
        self.stack.pop();
        self.tree.end_node();
    }

    pub fn file(&mut self, cx: &mut Cx, node_id: NodeId, name: &str) {
        println!("FILE {:?}", name);
        let info = self.tree.begin_node(node_id);
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        self.node.color = self.node_color(info.count);
        self.node.begin_quad(cx, self.node_layout(scale));
        cx.walk_turtle(self.indent_walk(self.stack.len()));
        cx.turtle_align_y();
        self.node_name.color = self.node_name_color_file;
        self.node_name.font_scale = scale;
        self.node_name.draw_text_walk(cx, name);
        self.node.end_quad(cx);
        self.tree.set_node_area(node_id, self.node.area());
        cx.turtle_new_line();
        self.tree.end_node();
    }

    fn node_color(&self, count: usize) -> Vec4 {
        if count % 2 == 0 {
            self.node_color_even
        } else {
            self.node_color_odd
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

    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) {
        if self.view.handle_scroll_view(cx, event) {
            self.view.redraw_view(cx);
        }
        self.tree.handle_event(cx, event);
        if self.tree.needs_redraw() {
            self.view.redraw_view(cx);
        }
    }
}
