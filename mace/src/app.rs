use {
    crate::{tab_bar::TabBar, tab_bar_logic::TabId, file_tree::FileTree},
    makepad_render::*,
    makepad_widget::*,
};

pub struct App {
    window: DesktopWindow,
    _file_tree: FileTree,
    tab_bar: TabBar,
}

impl App {
    pub fn style(cx: &mut Cx) {
        makepad_widget::set_widget_style(cx);
        TabBar::style(cx);
        FileTree::style(cx)
    }

    pub fn new(cx: &mut Cx) -> Self {
        Self {
            window: DesktopWindow::new(cx),
            _file_tree: FileTree::new(cx),
            tab_bar: TabBar::new(cx),
        }
    }

    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        self.window.handle_desktop_window(cx, event);
        // self.file_tree.handle_event(cx, event);
        self.tab_bar.handle_event(cx, event);
    }

    pub fn draw_app(&mut self, cx: &mut Cx) {
        if self.window.begin_desktop_window(cx, None).is_ok() {
            /*
            if self.file_tree.begin(cx).is_ok() {
                if self.file_tree.begin_folder(cx, NodeId(0), "A").is_ok() {
                    if self.file_tree.begin_folder(cx, NodeId(1), "B").is_ok() {
                        self.file_tree.file(cx, NodeId(3), "D");
                        self.file_tree.file(cx, NodeId(4), "E");
                        self.file_tree.end_folder();
                    }
                    if self.file_tree.begin_folder(cx, NodeId(2), "C").is_ok() {
                        self.file_tree.file(cx, NodeId(5), "F");
                        self.file_tree.file(cx, NodeId(6), "G");
                        self.file_tree.end_folder();
                    }
                    self.file_tree.end_folder();
                }
                self.file_tree.end(cx);
            }
            */
            if self.tab_bar.begin(cx).is_ok() {
                self.tab_bar.tab(cx, TabId(0), "AAA");
                self.tab_bar.tab(cx, TabId(1), "BBB");
                self.tab_bar.tab(cx, TabId(2), "CCC");
                self.tab_bar.end(cx);
            }
            self.window.end_desktop_window(cx);
        }
    }
}