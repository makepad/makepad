use {
    crate::{
        code_editor::CodeEditor,
        dock::{self, Dock, PanelId},
        document::Document,
        file_tree::{self, FileTree},
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
        collections::{HashMap, VecDeque},
        env,
        ffi::OsString,
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
    code_editors_by_panel_id: HashMap<PanelId, CodeEditor>,
    outstanding_requests: VecDeque<Request>,
    request_sender: Sender<Request>,
    response_signal: Signal,
    response_receiver: Receiver<Response>,
}

impl AppInner {
    fn new(cx: &mut Cx) -> AppInner {
        let server = Server::new(env::current_dir().unwrap());
        let (request_sender, request_receiver) = mpsc::channel();
        let response_signal = cx.new_signal();
        let (response_sender, response_receiver) = mpsc::channel();
        let connection = server.connect();
        thread::spawn(move || {
            while let Ok(request) = request_receiver.recv() {
                let response = connection.handle_request(request);
                response_sender.send(response).unwrap();
                Cx::post_signal(response_signal, StatusId::default());
            }
        });
        let mut inner = AppInner {
            window: DesktopWindow::new(cx),
            dock: Dock::new(cx),
            file_tree: FileTree::new(cx),
            code_editors_by_panel_id: HashMap::new(),
            outstanding_requests: VecDeque::new(),
            request_sender,
            response_signal,
            response_receiver,
        };
        inner.send_request(Request::GetFileTree());
        inner
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
                        let tab = &state.tabs_by_item_id[&item_id];
                        self.dock.tab(cx, *item_id, &tab.name);
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
        match tab.kind {
            TabKind::FileTree => self.draw_file_tree(cx, state),
            TabKind::CodeEditor { session_id } => {
                let code_editor = self
                    .code_editors_by_panel_id
                    .entry(tab.panel_id)
                    .or_insert_with(|| {
                        let mut code_editor = CodeEditor::new(cx);
                        code_editor.set_session_id(cx, session_id);
                        code_editor
                    });
                code_editor.draw(cx, &state.sessions_by_session_id, &state.documents_by_path)
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
        match &node.child_edges {
            Some(child_edges) => {
                if self.file_tree.begin_folder(cx, node_id, &node.name).is_ok() {
                    for child_edge in child_edges {
                        self.draw_file_node(cx, state, child_edge.node_id);
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
        self.file_tree.set_node_is_expanded(cx, NodeId(0), true, true);
        self.file_tree.redraw(cx);
    }

    fn send_request(&mut self, request: Request) {
        self.outstanding_requests.push_back(request.clone());
        self.request_sender.send(request).unwrap();
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &mut Event, state: &mut State) {
        self.window.handle_desktop_window(cx, event);
        let mut actions = Vec::new();
        self.dock.handle_event(cx, event, &mut |action| actions.push(action));
        for action in actions {
            match action {
                dock::Action::TabWasPressed(item_id) => {
                    let tab = &state.tabs_by_item_id[&item_id];
                    match tab.kind {
                        TabKind::FileTree => {},
                        TabKind::CodeEditor { session_id } => {
                            let code_editor = self
                                .code_editors_by_panel_id
                                .entry(tab.panel_id)
                                .or_insert_with(|| CodeEditor::new(cx));
                            code_editor.set_session_id(cx, session_id);
                        }
                    }
                }
            }
        }
        let mut actions = Vec::new();
        self.file_tree
            .handle_event(cx, event, &mut |action| actions.push(action));
        for action in actions {
            match action {
                file_tree::Action::FileWasPressed(node_id) => {
                    let node = &state.file_nodes_by_node_id[&node_id];
                    if node.is_file() {
                        let path = state.file_node_path(node_id);
                        if !state.documents_by_path.contains_key(&path) {
                            self.send_request(Request::OpenFile(path));
                        }
                    }
                }
            }
        }
        for code_editor in self.code_editors_by_panel_id.values_mut() {
            code_editor.handle_event(
                cx,
                event,
                &mut state.sessions_by_session_id,
                &mut state.documents_by_path,
            );
        }
        match event {
            Event::Signal(event) if event.signals.contains_key(&self.response_signal) => loop {
                match self.response_receiver.try_recv() {
                    Ok(response) => self.handle_response(cx, state, response),
                    Err(TryRecvError::Empty) => break,
                    _ => panic!(),
                }
            },
            _ => {}
        }
    }

    fn handle_response(&mut self, cx: &mut Cx, state: &mut State, response: Response) {
        let request = self.outstanding_requests.pop_front().unwrap();
        match response {
            Response::GetFileTree(response) => {
                self.set_file_tree(cx, state, response.unwrap());
                self.dock.tab_bar_mut(cx, PanelId(1)).set_selected_item_id(cx, Some(ItemId(0)));
            }
            Response::OpenFile(response) => {
                match request {
                    Request::OpenFile(path) => {
                        let panel_id = PanelId(2); // TODO;
                        let item_id = state.next_item_id;
                        state.next_item_id = ItemId(state.next_item_id.0 + 1);
                        let session_id = state.next_session_id;
                        state.next_session_id = SessionId(state.next_session_id.0 + 1);
                        match state.panels_by_panel_id.get_mut(&panel_id).unwrap() {
                            Panel::TabBar { item_ids } => item_ids.push(item_id),
                            _ => panic!(),
                        }
                        state.tabs_by_item_id.insert(
                            item_id,
                            Tab {
                                panel_id,
                                name: path.file_name().unwrap().to_string_lossy().into_owned(),
                                kind: TabKind::CodeEditor { session_id },
                            },
                        );
                        state
                            .sessions_by_session_id
                            .insert(session_id, Session::new(path.clone()));
                        state
                            .documents_by_path
                            .insert(path, Document::new(response.unwrap()));
                        self.dock.tab_bar_mut(cx, panel_id).set_selected_item_id(cx, Some(item_id));
                        let code_editor = self
                            .code_editors_by_panel_id
                            .entry(panel_id)
                            .or_insert_with(|| CodeEditor::new(cx));
                        code_editor.set_session_id(cx, session_id);
                    }
                    _ => panic!(),
                }
            }
        }
    }
}

struct State {
    next_node_id: NodeId,
    next_item_id: ItemId,
    next_session_id: SessionId,
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
        panels_by_panel_id.insert(PanelId(2), Panel::TabBar { item_ids: vec![] });
        let mut tabs_by_item_id = HashMap::new();
        tabs_by_item_id.insert(
            ItemId(0),
            Tab {
                panel_id: PanelId(1),
                name: String::from("File Tree"),
                kind: TabKind::FileTree,
            },
        );
        let mut file_nodes_by_node_id = HashMap::new();
        file_nodes_by_node_id.insert(
            NodeId(0),
            FileNode {
                parent_edge: None,
                name: String::from("root"),
                child_edges: Some(Vec::new()),
            },
        );
        State {
            next_node_id: NodeId(1),
            next_item_id: ItemId(1),
            next_session_id: SessionId(0),
            panels_by_panel_id,
            tabs_by_item_id,
            file_nodes_by_node_id,
            sessions_by_session_id: HashMap::new(),
            documents_by_path: HashMap::new(),
        }
    }

    fn set_file_tree(&mut self, root: protocol::FileNode) {
        fn create_file_node(
            next_node_id: &mut NodeId,
            file_nodes_by_node_id: &mut HashMap<NodeId, FileNode>,
            parent_edge: Option<FileEdge>,
            node: protocol::FileNode,
        ) -> NodeId {
            let node_id = *next_node_id;
            *next_node_id = NodeId(next_node_id.0 + 1);
            let name = parent_edge.as_ref().map_or_else(
                || String::from("root"),
                |edge| edge.name.to_string_lossy().into_owned(),
            );
            let node = FileNode {
                parent_edge,
                name,
                child_edges: match node {
                    protocol::FileNode::Directory { entries } => Some(
                        entries
                            .into_iter()
                            .map(|entry| FileEdge {
                                name: entry.name.clone(),
                                node_id: create_file_node(
                                    next_node_id,
                                    file_nodes_by_node_id,
                                    Some(FileEdge {
                                        name: entry.name,
                                        node_id,
                                    }),
                                    entry.node,
                                ),
                            })
                            .collect::<Vec<_>>(),
                    ),
                    protocol::FileNode::File => None,
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
            root,
        );
    }

    fn file_node_path(&self, node_id: NodeId) -> PathBuf {
        let mut components = Vec::new();
        let mut node = &self.file_nodes_by_node_id[&node_id];
        while let Some(edge) = &node.parent_edge {
            components.push(&edge.name);
            node = &self.file_nodes_by_node_id[&edge.node_id];
        }
        components.into_iter().rev().collect::<PathBuf>()
    }
}

enum Panel {
    Splitter { panel_ids: (PanelId, PanelId) },
    TabBar { item_ids: Vec<ItemId> },
}

struct Tab {
    panel_id: PanelId,
    name: String,
    kind: TabKind,
}

enum TabKind {
    FileTree,
    CodeEditor { session_id: SessionId },
}

#[derive(Debug)]
struct FileNode {
    parent_edge: Option<FileEdge>,
    name: String,
    child_edges: Option<Vec<FileEdge>>,
}

impl FileNode {
    fn is_file(&self) -> bool {
        self.child_edges.is_none()
    }
}

#[derive(Debug)]
struct FileEdge {
    name: OsString,
    node_id: NodeId,
}
