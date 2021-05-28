use {
    crate::{file_tree::FileTree, tree::NodeId},
    makepad_render::*,
    makepad_widget::*,
};

pub struct App {
    window: DesktopWindow,
    tree: FileTree
}

impl App {
    pub fn style(cx: &mut Cx) {
        makepad_widget::set_widget_style(cx);
        FileTree::style(cx)
    }

    pub fn new(cx: &mut Cx) -> Self {
        Self {
            window: DesktopWindow::new(cx),
            tree: FileTree::new(cx),
        }
    }

    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        self.window.handle_desktop_window(cx, event);
        self.tree.handle_event(cx, event);
    }

    pub fn draw_app(&mut self, cx: &mut Cx) {
        if self.window.begin_desktop_window(cx, None).is_ok() {
            if self.tree.begin(cx).is_ok() {
                if self.tree.begin_folder(cx, NodeId(0), "A").is_ok() {
                    if self.tree.begin_folder(cx, NodeId(1), "B").is_ok() {
                        self.tree.file(cx, NodeId(3), "D");
                        self.tree.file(cx, NodeId(4), "E");
                        self.tree.end_folder();
                    }
                    if self.tree.begin_folder(cx, NodeId(2), "C").is_ok() {
                        self.tree.file(cx, NodeId(5), "F");
                        self.tree.file(cx, NodeId(6), "G");
                        self.tree.end_folder();
                    }
                    self.tree.end_folder();
                }
                self.tree.end(cx);
            }
            self.window.end_desktop_window(cx);
        }
    }
}