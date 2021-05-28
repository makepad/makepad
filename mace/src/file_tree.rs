use {
    crate::tree::{NodeId, Tree},
    makepad_render::*,
    makepad_widget::*,
};

pub struct FileTree {
    view: ScrollView,
    tree: Tree,
    node: DrawColor,
    node_layout: Layout,
    node_name: DrawText,
}

impl FileTree {
    pub fn style(cx: &mut Cx) {
        live_body!(cx, {
            self::node_layout: Layout {
                walk: Walk { width: Fill, height: Fix(20.0) },
                align: { fx: 0.0, fy: 0.5 },
                padding: { l: 5.0, t: 0.0, r: 0.0, b: 1.0 },
            }
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
            node_layout: Layout::default(),
            node_name: DrawText::new(cx, default_shader!()),
        }
    }

    pub fn apply_style(&mut self, cx: &mut Cx) {
        self.node_layout = live_layout!(cx, self::node_layout);
        self.node_name.text_style = live_text_style!(cx, self::node_name_text_style);
    }

    pub fn begin(&mut self, cx: &mut Cx) -> Result<(), ()> {
        self.apply_style(cx);
        self.view.begin_view(cx, Layout::default())?;
        self.tree.begin();
        Ok(())
    }

    pub fn end(&mut self, cx: &mut Cx) {
        self.tree.end();
        self.view.end_view(cx);
    }

    pub fn begin_folder(&mut self, cx: &mut Cx, node_id: NodeId, name: &str) -> Result<(), ()> {
        let info = self.tree.begin_node(node_id);
        self.node.begin_quad(cx, self.node_layout);
        self.node_name.draw_text_walk(cx, name);
        self.node.end_quad(cx);
        self.tree.set_node_area(node_id, self.node.area());
        cx.turtle_new_line();
        if info.is_fully_collapsed() {
            self.end_folder();
            return Err(());
        }
        Ok(())
    }

    pub fn end_folder(&mut self) {
        self.tree.end_node();
    }

    pub fn file(&mut self, cx: &mut Cx, node_id: NodeId, name: &str) {
        self.tree.begin_node(node_id);
        self.node.begin_quad(cx, self.node_layout);
        self.node_name.draw_text_walk(cx, name);
        self.node.end_quad(cx);
        self.tree.set_node_area(node_id, self.node.area());
        cx.turtle_new_line();
        self.tree.end_node();
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