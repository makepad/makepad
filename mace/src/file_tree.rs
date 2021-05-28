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
    indent_width: f32,
    node_name: DrawText,
    stack: Vec<f32>,
}

impl FileTree {
    pub fn style(cx: &mut Cx) {
        live_body!(cx, {
            self::node_height: 20.0;
            self::indent_width: 10.0;
            self::node_name_text_style: TextStyle {
                top_drop: 1.3,
                ..makepad_widget::widgetstyle::text_style_normal
            }
        })
    }

    pub fn new(cx: &mut Cx) -> FileTree {
        FileTree {
            view: ScrollView::new_standard_hv(cx),
            tree: Tree::new(),
            node: DrawColor::new(cx, default_shader!()),
            node_height: 0.0,
            indent_width: 0.0,
            node_name: DrawText::new(cx, default_shader!()),
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
        self.indent_width = live_float!(cx, self::indent_width);
    }

    pub fn begin_folder(&mut self, cx: &mut Cx, node_id: NodeId, name: &str) -> Result<(), ()> {
        let info = self.tree.begin_node(node_id);
        println!("BEGIN FOLDER {:?} ({:?})", name, info.is_expanded_fraction);
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        self.node.begin_quad(cx, self.node_layout(scale));
        cx.walk_turtle(self.indent_walk(self.stack.len()));
        self.node_name.font_scale = self.stack.last().cloned().unwrap_or(1.0);
        self.node_name.draw_text_walk(cx, name);
        self.node.end_quad(cx);
        self.tree.set_node_area(node_id, self.node.area());
        cx.turtle_new_line();
        self.stack.push(info.is_expanded_fraction);
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
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        self.tree.begin_node(node_id);
        self.node.begin_quad(cx, self.node_layout(scale));
        cx.walk_turtle(self.indent_walk(self.stack.len()));
        self.node_name.font_scale = scale;
        self.node_name.draw_text_walk(cx, name);
        self.node.end_quad(cx);
        self.tree.set_node_area(node_id, self.node.area());
        cx.turtle_new_line();
        self.tree.end_node();
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
            margin: Margin { l: 1.0, t: 0.0, r: 4.0, b: 0.0 },
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
