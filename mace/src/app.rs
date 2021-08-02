use {
    crate::{
        code_editor::CodeEditor,
        dock::{ContainerId, Dock},
        document::{Document, DocumentId},
        file_tree::FileTree,
        list_logic::ItemId,
        protocol::{Request, Response},
        server::Server,
        session::{Session, SessionId},
        splitter::Splitter,
        tab_bar::TabBar,
        tree_logic::NodeId,
    },
    makepad_render::*,
    makepad_widget::*,
    std::{
        collections::HashMap,
        env,
        sync::mpsc::{self, Receiver, Sender, TryRecvError},
        thread,
    },
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
    file_nodes_by_node_id: HashMap<NodeId, FileNode>,
    sessions: HashMap<SessionId, Session>,
    documents: HashMap<DocumentId, Document>,
    request_sender: Sender<Request>,
    response_receiver: Receiver<Response>,
    response_signal: Signal,
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
        let server = Server::new(env::current_dir().unwrap());
        let (request_sender, request_receiver) = mpsc::channel();
        let (response_sender, response_receiver) = mpsc::channel();
        let response_signal = cx.new_signal();
        let connection = server.connect();
        thread::spawn(move || {
            while let Ok(request) = request_receiver.recv() {
                let response = connection.handle_request(request);
                response_sender.send(response).unwrap();
                Cx::post_signal(response_signal, StatusId::default());
            }
        });
        request_sender.send(Request::GetFileTree()).unwrap();
        Self {
            window: DesktopWindow::new(cx),
            dock: Dock::new(cx),
            file_tree: FileTree::new(cx),
            code_editor,
            file_nodes_by_node_id: HashMap::new(),
            sessions,
            documents,
            request_sender,
            response_receiver,
            response_signal,
        }
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

    fn draw_file_tree(&mut self, cx: &mut Cx) {
        fn draw_file_node(
            cx: &mut Cx,
            file_tree: &mut FileTree,
            file_nodes_by_node_id: &HashMap<NodeId, FileNode>,
            node_id: NodeId,
        ) {
            let node = &file_nodes_by_node_id[&node_id];
            match &node.child_ids {
                Some(child_ids) => {
                    if file_tree.begin_folder(cx, node_id, &node.name).is_ok() {
                        for child_id in child_ids {
                            draw_file_node(cx, file_tree, file_nodes_by_node_id, *child_id);
                        }
                        file_tree.end_folder();
                    }
                }
                None => {
                    file_tree.file(cx, node_id, &node.name);
                }
            }
        }

        if self.file_tree.begin(cx).is_ok() {}
    }

    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        self.window.handle_desktop_window(cx, event);
        self.dock.handle_event(cx, event);
        self.file_tree.handle_event(cx, event);
        self.code_editor
            .handle_event(cx, event, &mut self.sessions, &mut self.documents);
        match event {
            Event::Signal(event) if event.signals.contains_key(&self.response_signal) => {
                loop {
                    match self.response_receiver.try_recv() {
                        Ok(response) => {
                            match response {
                                Response::GetFileTree(response) => {
                                    let response = response.unwrap();
                                    self.file_tree.forget();
                                    // TODO
                                }
                            }
                        }
                        Err(TryRecvError::Empty) => break,
                        _ => panic!(),
                    }
                }
            }
            _ => {}
        }
    }
}

struct FileNode {
    parent_id: Option<NodeId>,
    name: String,
    child_ids: Option<Vec<NodeId>>,
}
