use {
    crate::{
        code_editor::CodeEditor,
        dock::{Dock, PanelId},
        document::Document,
        file_tree::FileTree,
        list_logic::ItemId,
        protocol::{self, Request, Response},
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
        path::PathBuf,
        sync::mpsc::{self, Receiver, Sender, TryRecvError},
        thread,
    },
};

/* This is a test comment
 *
 * It spans multiple lines.
 */
pub struct App {
    inner: AppInner,
    state: State,
}

impl App {
    pub fn style(cx: &mut Cx) {
        makepad_widget::set_widget_style(cx);
        CodeEditor::style(cx);
        FileTree::style(cx);
        Splitter::style(cx);
        TabBar::style(cx);
    }

    pub fn new(cx: &mut Cx) -> App {
        App {
            inner: AppInner::new(cx),
            state: State::new(),
        }
    }

    pub fn draw_app(&mut self, cx: &mut Cx) {
        self.inner.draw(cx, &self.state);
    }

    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        self.inner.handle_event(cx, event, &mut self.state);
    }
}

struct AppInner {
    window: DesktopWindow,
    dock: Dock,
    file_tree: FileTree,
    code_editor: CodeEditor,
    request_sender: Sender<Request>,
    response_receiver: Receiver<Response>,
    response_signal: Signal,
}

impl AppInner {
    fn new(cx: &mut Cx) -> AppInner {
        let mut code_editor = CodeEditor::new(cx);
        code_editor.set_session_id(SessionId(0));
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
        AppInner {
            window: DesktopWindow::new(cx),
            dock: Dock::new(cx),
            file_tree: FileTree::new(cx),
            code_editor,
            request_sender,
            response_receiver,
            response_signal,
        }
    }

    fn draw(&mut self, cx: &mut Cx, state: &State) {
        if self.window.begin_desktop_window(cx, None).is_ok() {
            self.draw_panel(cx, state, PanelId(0));
            self.window.end_desktop_window(cx);
        }
    }

    fn draw_panel(&mut self, cx: &mut Cx, state: &State, panel_id: PanelId) {
        let panel = &state.panels_by_panel_id[&panel_id];
        match panel {
            Panel::Splitter { panel_ids } => {
                if self.dock.begin_splitter(cx, panel_id).is_ok() {
                    self.draw_panel(cx, state, panel_ids.0);
                    self.dock.middle_splitter(cx);
                    self.draw_panel(cx, state, panel_ids.1);
                    self.dock.end_splitter(cx);
                }
            }
            Panel::TabBar { item_ids } => {
                if self.dock.begin_tab_bar(cx, panel_id).is_ok() {
                    for item_id in item_ids {
                        self.dock.tab(cx, *item_id, "TODO"); // TODO
                    }
                    self.dock.end_tab_bar(cx);
                }
                if let Some(item_id) = self.dock.tab_bar_mut(cx, panel_id).selected_item_id() {
                    cx.turtle_new_line();
                    self.draw_tab(cx, state, item_id);
                }
            }
        }
    }

    fn draw_tab(&mut self, cx: &mut Cx, state: &State, item_id: ItemId) {
        let tab = &state.tabs_by_item_id[&item_id];
        match tab {
            Tab::FileTree => self.draw_file_tree(cx, state),
            Tab::CodeEditor => {
                self.code_editor
                    .draw(cx, &state.sessions_by_session_id, &state.documents_by_path)
            }
        }
    }

    fn draw_file_tree(&mut self, cx: &mut Cx, state: &State) {
        if self.file_tree.begin(cx).is_ok() {
            self.draw_file_node(cx, state, NodeId(0));
            self.file_tree.end(cx);
        }
    }

    fn draw_file_node(&mut self, cx: &mut Cx, state: &State, node_id: NodeId) {
        let node = &state.file_nodes_by_node_id[&node_id];
        match &node.child_ids {
            Some(child_ids) => {
                if self.file_tree.begin_folder(cx, node_id, &node.name).is_ok() {
                    for child_id in child_ids {
                        self.draw_file_node(cx, state, *child_id);
                    }
                    self.file_tree.end_folder();
                }
            }
            None => {
                self.file_tree.file(cx, node_id, &node.name);
            }
        }
    }

    fn set_file_tree(&mut self, cx: &mut Cx, state: &mut State, root: protocol::FileNode) {
        self.file_tree.forget();
        state.set_file_tree(root);
        self.file_tree.redraw(cx);
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &mut Event, state: &mut State) {
        self.window.handle_desktop_window(cx, event);
        self.dock.handle_event(cx, event);
        self.file_tree.handle_event(cx, event);
        self.code_editor.handle_event(
            cx,
            event,
            &mut state.sessions_by_session_id,
            &mut state.documents_by_path,
        );
        match event {
            Event::Signal(event) if event.signals.contains_key(&self.response_signal) => loop {
                match self.response_receiver.try_recv() {
                    Ok(response) => match response {
                        Response::GetFileTree(response) => {
                            self.set_file_tree(cx, state, response.unwrap())
                        }
                    },
                    Err(TryRecvError::Empty) => break,
                    _ => panic!(),
                }
            },
            _ => {}
        }
    }
}

struct State {
    next_node_id: NodeId,
    panels_by_panel_id: HashMap<PanelId, Panel>,
    tabs_by_item_id: HashMap<ItemId, Tab>,
    file_nodes_by_node_id: HashMap<NodeId, FileNode>,
    sessions_by_session_id: HashMap<SessionId, Session>,
    documents_by_path: HashMap<PathBuf, Document>,
}

impl State {
    fn new() -> State {
        let mut panels_by_panel_id = HashMap::new();
        panels_by_panel_id.insert(
            PanelId(0),
            Panel::Splitter {
                panel_ids: (PanelId(1), PanelId(2)),
            },
        );
        panels_by_panel_id.insert(
            PanelId(1),
            Panel::TabBar {
                item_ids: vec![ItemId(0)],
            },
        );
        panels_by_panel_id.insert(
            PanelId(2),
            Panel::TabBar {
                item_ids: vec![ItemId(1)],
            },
        );
        let mut tabs_by_item_id = HashMap::new();
        tabs_by_item_id.insert(ItemId(0), Tab::FileTree);
        tabs_by_item_id.insert(ItemId(1), Tab::CodeEditor);
        let mut file_nodes_by_node_id = HashMap::new();
        file_nodes_by_node_id.insert(
            NodeId(0),
            FileNode {
                parent_id: None,
                name: String::from("root"),
                child_ids: Some(Vec::new()),
            },
        );
        let mut sessions_by_session_id = HashMap::new();
        sessions_by_session_id.insert(SessionId(0), Session::new(PathBuf::from("app.rs")));
        let mut documents_by_path = HashMap::new();
        documents_by_path.insert(
            PathBuf::from("app.rs"),
            Document::new(
                include_str!("app.rs")
                    .lines()
                    .map(|line| line.chars().collect::<Vec<_>>())
                    .collect::<Vec<_>>()
                    .into(),
            ),
        );
        State {
            next_node_id: NodeId(1),
            panels_by_panel_id,
            tabs_by_item_id,
            file_nodes_by_node_id,
            sessions_by_session_id,
            documents_by_path,
        }
    }

    fn set_file_tree(&mut self, root: protocol::FileNode) {
        fn create_file_node(
            next_node_id: &mut NodeId,
            file_nodes_by_node_id: &mut HashMap<NodeId, FileNode>,
            parent_id: Option<NodeId>,
            name: String,
            node: protocol::FileNode,
        ) -> NodeId {
            let node_id = *next_node_id;
            *next_node_id = NodeId(next_node_id.0 + 1);
            let node = FileNode {
                parent_id,
                name,
                child_ids: match node {
                    protocol::FileNode::File => None,
                    protocol::FileNode::Directory { children } => Some(
                        children
                            .into_iter()
                            .map(|(name, child)| {
                                create_file_node(
                                    next_node_id,
                                    file_nodes_by_node_id,
                                    Some(node_id),
                                    name.to_string_lossy().into_owned(),
                                    child,
                                )
                            })
                            .collect::<Vec<_>>(),
                    ),
                },
            };
            file_nodes_by_node_id.insert(node_id, node);
            node_id
        }

        self.next_node_id = NodeId(0);
        self.file_nodes_by_node_id.clear();
        create_file_node(
            &mut self.next_node_id,
            &mut self.file_nodes_by_node_id,
            None,
            String::from("root"),
            root,
        );
    }
}

enum Panel {
    Splitter { panel_ids: (PanelId, PanelId) },
    TabBar { item_ids: Vec<ItemId> },
}

enum Tab {
    FileTree,
    CodeEditor,
}

#[derive(Debug)]
struct FileNode {
    parent_id: Option<NodeId>,
    name: String,
    child_ids: Option<Vec<NodeId>>,
}
