use {
    crate::{
        code_editor::CodeEditor,
        dock::{ContainerId, Dock},
        document::{Document, DocumentId},
        file_tree::FileTree,
        list_logic::ItemId,
        session::{Session, SessionId},
        splitter::Splitter,
        tab_bar::TabBar,
        tree_logic::NodeId,
    },
    makepad_render::*,
    makepad_widget::*,
    std::collections::HashMap,
};

/* This is a test comment
 *
 * It spans multiple lines.
 */
pub struct App {
    window: DesktopWindow,
    dock: Dock,
    file_tree: FileTree,
    code_editor: CodeEditor,
    sessions: HashMap<SessionId, Session>,
    documents: HashMap<DocumentId, Document>,
}

impl App {
    pub fn style(cx: &mut Cx) {
        makepad_widget::set_widget_style(cx);
        CodeEditor::style(cx);
        FileTree::style(cx);
        Splitter::style(cx);
        TabBar::style(cx);
    }

    pub fn new(cx: &mut Cx) -> Self {
        let mut documents = HashMap::new();
        let document_id = DocumentId(0);
        let document = Document::new(
            include_str!("app.rs")
                .lines()
                .map(|line| line.chars().collect::<Vec<_>>())
                .collect::<Vec<_>>()
                .into(),
        );
        documents.insert(document_id, document);
        let mut sessions = HashMap::new();
        let session_id = SessionId(0);
        let session = Session::new(document_id);
        sessions.insert(session_id, session);
        let mut code_editor = CodeEditor::new(cx);
        code_editor.set_session_id(session_id);
        Self {
            window: DesktopWindow::new(cx),
            dock: Dock::new(cx),
            file_tree: FileTree::new(cx),
            code_editor,
            sessions,
            documents,
        }
    }

    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        self.window.handle_desktop_window(cx, event);
        self.dock.handle_event(cx, event);
        self.file_tree.handle_event(cx, event);
        self.code_editor
            .handle_event(cx, event, &mut self.sessions, &mut self.documents);
    }

    pub fn draw_app(&mut self, cx: &mut Cx) {
        self.dock
            .splitter_mut(cx, ContainerId(2))
            .set_axis(Axis::Vertical);
        if self.window.begin_desktop_window(cx, None).is_ok() {
            if self.dock.begin_splitter(cx, ContainerId(0)).is_ok() {
                if self.dock.begin_tab_bar(cx, ContainerId(1)).is_ok() {
                    self.dock.tab(cx, ItemId(0), "File tree");
                    self.dock.end_tab_bar(cx);
                }
                cx.turtle_new_line();
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
                self.dock.middle_splitter(cx);
                if self.dock.begin_tab_bar(cx, ContainerId(3)).is_ok() {
                    self.dock.tab(cx, ItemId(1), "AAA");
                    self.dock.tab(cx, ItemId(2), "BBB");
                    self.dock.tab(cx, ItemId(3), "CCC");
                    self.dock.end_tab_bar(cx);
                }
                cx.turtle_new_line();
                self.code_editor.draw(cx, &self.sessions, &self.documents);
                self.dock.end_splitter(cx);
            }
            self.window.end_desktop_window(cx);
        }
    }
}
