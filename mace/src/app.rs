use {
    crate::{dock::{Dock, PanelId}, tab_bar::TabBar, list_logic::ItemId, file_tree::FileTree, splitter::Splitter, tree_logic::NodeId},
    makepad_render::*,
    makepad_widget::*,
};

pub struct App {
    window: DesktopWindow,
    dock: Dock,
    file_tree: FileTree,
}

impl App {
    pub fn style(cx: &mut Cx) {
        makepad_widget::set_widget_style(cx);
        FileTree::style(cx);
        Splitter::style(cx);
        TabBar::style(cx);
    }

    pub fn new(cx: &mut Cx) -> Self {
        Self {
            window: DesktopWindow::new(cx),
            dock: Dock::new(cx),
            file_tree: FileTree::new(cx),
        }
    }

    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        self.window.handle_desktop_window(cx, event);
        self.file_tree.handle_event(cx, event);
    }

    pub fn draw_app(&mut self, cx: &mut Cx) {
        if self.window.begin_desktop_window(cx, None).is_ok() {
            self.dock.begin_split_panel(cx, PanelId(0));
            self.dock.middle_split_panel(cx);
            if self.dock.begin_tab_panel(cx, PanelId(1)).is_ok() {
                self.dock.tab(cx, ItemId(0), "AAA");
                self.dock.tab(cx, ItemId(1), "BBB");
                self.dock.tab(cx, ItemId(2), "CCC");
                self.dock.end_tab_panel(cx);
            }
            self.dock.end_split_panel(cx);
            self.window.end_desktop_window(cx);
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
        }
    }
}