use {
    crate::{
        code_editor::{self, CodeEditor, SessionId, ViewId},
        dock::{self, Dock, PanelId},
        file_tree::{self, FileNodeId, FileTree},
        generational::{Arena, IdAllocator},
        protocol::{self, Request, Response, ResponseOrNotification},
        server::{Connection, Server},
        splitter::Splitter,
        tab_bar::{TabBar, TabId},
        tree_logic::NodeId,
    },
    makepad_render::*,
    makepad_widget::*,
    std::{
        collections::VecDeque,
        env,
        ffi::OsString,
        io::{Read, Write},
        net::{TcpListener, TcpStream},
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
    outstanding_requests: VecDeque<Request>,
    request_sender: Sender<Request>,
    response_or_notification_signal: Signal,
    response_or_notification_receiver: Receiver<ResponseOrNotification>,
}

impl AppInner {
    fn new(cx: &mut Cx) -> AppInner {
        let server = Server::new(env::current_dir().unwrap());
        spawn_connection_listener(TcpListener::bind("127.0.0.1:0").unwrap(), server.clone());
        let (request_sender, request_receiver) = mpsc::channel();
        let response_or_notification_signal = cx.new_signal();
        let (response_or_notification_sender, response_or_notification_receiver) = mpsc::channel();
        match env::args().nth(1) {
            Some(arg) => {
                let stream = TcpStream::connect(arg).unwrap();
                spawn_request_sender(request_receiver, stream.try_clone().unwrap());
                spawn_response_or_notification_receiver(
                    stream,
                    response_or_notification_signal,
                    response_or_notification_sender,
                );
            }
            None => {
                spawn_local_request_handler(
                    request_receiver,
                    server.connect(Box::new({
                        let response_or_notification_sender =
                            response_or_notification_sender.clone();
                        move |notification| {
                            response_or_notification_sender
                                .send(ResponseOrNotification::Notification(notification))
                                .unwrap();
                            Cx::post_signal(response_or_notification_signal, StatusId::default());
                        }
                    })),
                    response_or_notification_signal,
                    response_or_notification_sender,
                );
            }
        }
        let mut inner = AppInner {
            window: DesktopWindow::new(cx),
            dock: Dock::new(cx),
            file_tree: FileTree::new(cx),
            code_editor: CodeEditor::new(cx),
            outstanding_requests: VecDeque::new(),
            request_sender,
            response_or_notification_signal,
            response_or_notification_receiver,
        };
        inner.send_request(Request::GetFileTree());
        inner
    }

    fn draw(&mut self, cx: &mut Cx, state: &State) {
        if self.window.begin_desktop_window(cx, None).is_ok() {
            self.draw_panel(cx, state, state.root_panel_id);
            self.window.end_desktop_window(cx);
        }
    }

    fn draw_panel(&mut self, cx: &mut Cx, state: &State, panel_id: PanelId) {
        let panel = &state.panels[panel_id];
        match panel {
            Panel::Splitter { child_ids } => {
                if self.dock.begin_splitter(cx, panel_id).is_ok() {
                    self.draw_panel(cx, state, child_ids[0]);
                    self.dock.middle_splitter(cx);
                    self.draw_panel(cx, state, child_ids[01]);
                    self.dock.end_splitter(cx);
                }
            }
            Panel::TabBar { tab_ids, .. } => {
                if self.dock.begin_tab_bar(cx, panel_id).is_ok() {
                    for tab_id in tab_ids {
                        let tab = &state.tabs[*tab_id];
                        self.dock.tab(cx, *tab_id, &tab.name);
                    }
                    self.dock.end_tab_bar(cx);
                }
                if let Some(item_id) = self
                    .dock
                    .get_or_create_tab_bar(cx, panel_id)
                    .selected_item_id()
                {
                    cx.turtle_new_line();
                    self.draw_tab(cx, state, item_id);
                }
            }
        }
    }

    fn draw_tab(&mut self, cx: &mut Cx, state: &State, tab_id: TabId) {
        let tab = &state.tabs[tab_id];
        match tab.kind {
            TabKind::FileTree => self.draw_file_tree(cx, state),
            TabKind::CodeEditor { .. } => {
                let panel = &state.panels[tab.panel_id];
                match panel {
                    Panel::TabBar { view_id, .. } => {
                        self.code_editor
                            .draw(cx, &state.code_editor_state, view_id.unwrap());
                    }
                    _ => panic!(),
                }
            }
        }
    }

    fn draw_file_tree(&mut self, cx: &mut Cx, state: &State) {
        if self.file_tree.begin(cx).is_ok() {
            self.draw_file_node(cx, state, state.root_file_node_id);
            self.file_tree.end(cx);
        }
    }

    fn draw_file_node(&mut self, cx: &mut Cx, state: &State, file_node_id: FileNodeId) {
        let file_node = &state.file_nodes[file_node_id];
        match &file_node.child_edges {
            Some(child_edges) => {
                if self
                    .file_tree
                    .begin_folder(cx, file_node_id, &file_node.name)
                    .is_ok()
                {
                    for child_edge in child_edges {
                        self.draw_file_node(cx, state, child_edge.file_node_id);
                    }
                    self.file_tree.end_folder();
                }
            }
            None => {
                self.file_tree.file(cx, file_node_id, &file_node.name);
            }
        }
    }

    fn set_file_tree(&mut self, cx: &mut Cx, state: &mut State, root: protocol::FileNode) {
        self.file_tree.forget();
        state.set_file_tree(root);
        self.file_tree
            .set_file_node_is_expanded(cx, state.root_file_node_id, true, true);
        self.file_tree.redraw(cx);
    }

    fn send_request(&mut self, request: Request) {
        self.outstanding_requests.push_back(request.clone());
        self.request_sender.send(request).unwrap();
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &mut Event, state: &mut State) {
        self.window.handle_desktop_window(cx, event);

        let mut actions = Vec::new();
        self.dock
            .handle_event(cx, event, &mut |action| actions.push(action));
        for action in actions {
            match action {
                dock::Action::TabWasPressed(tab_id) => {
                    let tab = &state.tabs[tab_id];
                    match tab.kind {
                        TabKind::CodeEditor { session_id } => {
                            self.create_or_update_view(cx, state, state.panel_id, session_id);
                        }
                        _ => {}
                    }
                }
            }
        }

        let mut actions = Vec::new();
        self.file_tree
            .handle_event(cx, event, &mut |action| actions.push(action));
        for action in actions {
            match action {
                file_tree::Action::FileNodeWasPressed(file_node_id) => {
                    let file_node = &state.file_nodes[file_node_id];
                    if file_node.is_file() {
                        let path = state.file_node_path(file_node_id);
                        self.send_request(Request::OpenFile(path));
                    }
                }
            }
        }

        let mut panel_id_stack = vec![state.root_panel_id];
        while let Some(panel_id) = panel_id_stack.pop() {
            let panel = &state.panels[panel_id];
            match panel {
                Panel::Splitter { child_ids } => {
                    for child_id in child_ids {
                        panel_id_stack.push(*child_id);
                    }
                }
                Panel::TabBar { view_id, .. } => {
                    if let Some(view_id) = view_id {
                        self.code_editor.handle_event(
                            cx,
                            &mut state.code_editor_state,
                            *view_id,
                            event,
                            &mut {
                                let request_sender = &self.request_sender;
                                move |request| request_sender.send(request).unwrap()
                            },
                        );
                    }
                }
            }
        }

        match event {
            Event::Signal(event)
                if event
                    .signals
                    .contains_key(&self.response_or_notification_signal) =>
            {
                loop {
                    match self.response_or_notification_receiver.try_recv() {
                        Ok(response_or_notification) => self.handle_response_or_notification(
                            cx,
                            state,
                            response_or_notification,
                        ),
                        Err(TryRecvError::Empty) => break,
                        _ => panic!(),
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_response_or_notification(
        &mut self,
        cx: &mut Cx,
        state: &mut State,
        response_or_notification: ResponseOrNotification,
    ) {
        match response_or_notification.clone() {
            ResponseOrNotification::Response(response) => match response {
                Response::GetFileTree(response) => {
                    match self.outstanding_requests.pop_front().unwrap() {
                        Request::GetFileTree() => {
                            self.set_file_tree(cx, state, response.unwrap());
                            let tab = &state.tabs[state.file_tree_tab_id];
                            self.dock
                                .get_or_create_tab_bar(cx, tab.panel_id)
                                .set_selected_item_id(cx, Some(state.file_tree_tab_id));
                        }
                        _ => panic!(),
                    }
                }
                Response::OpenFile(response) => {
                    match self.outstanding_requests.pop_front().unwrap() {
                        Request::OpenFile(path) => {
                            let (revision, text) = response.unwrap();
                            let name = path.file_name().unwrap().to_string_lossy().into_owned();
                            let document_id = state
                                .code_editor_state
                                .create_document(path, revision, text);
                            let session_id = state.code_editor_state.create_session(document_id);
                            let tab_id = state.tab_id_allocator.allocate();
                            match &mut state.panels[state.panel_id] {
                                Panel::TabBar { tab_ids, .. } => tab_ids.push(tab_id),
                                _ => panic!(),
                            }
                            state.tabs.insert(
                                tab_id,
                                Tab {
                                    panel_id: state.panel_id,
                                    name,
                                    kind: TabKind::CodeEditor { session_id },
                                },
                            );
                            self.create_or_update_view(cx, state, state.panel_id, session_id);
                            self.dock
                                .get_or_create_tab_bar(cx, state.panel_id)
                                .set_selected_item_id(cx, Some(tab_id));
                        }
                        _ => panic!(),
                    }
                }
                _ => {}
            },
            _ => {}
        }
        self.code_editor.handle_response_or_notification(
            cx,
            &mut state.code_editor_state,
            response_or_notification,
            &mut {
                let request_sender = &self.request_sender;
                move |request| request_sender.send(request).unwrap()
            },
        );
    }

    fn create_or_update_view(
        &mut self,
        cx: &mut Cx,
        state: &mut State,
        panel_id: PanelId,
        session_id: SessionId,
    ) {
        let panel = &mut state.panels[panel_id];
        match panel {
            Panel::TabBar { view_id, .. } => match view_id {
                Some(view_id) => {
                    self.code_editor.set_view_session_id(
                        &mut state.code_editor_state,
                        *view_id,
                        session_id,
                    );
                    self.code_editor.redraw_view(cx, *view_id);
                }
                None => {
                    *view_id = Some(self.code_editor.create_view(
                        cx,
                        &mut state.code_editor_state,
                        session_id,
                    ));
                    self.code_editor.redraw_view(cx, view_id.unwrap());
                }
            },
            _ => panic!(),
        }
    }
}

struct State {
    panel_id_allocator: IdAllocator,
    panels: Arena<Panel>,
    root_panel_id: PanelId,
    panel_id: PanelId,
    tab_id_allocator: IdAllocator,
    tabs: Arena<Tab>,
    file_tree_tab_id: TabId,
    file_node_id_allocator: IdAllocator,
    file_nodes: Arena<FileNode>,
    root_file_node_id: FileNodeId,
    code_editor_state: code_editor::State,
}

impl State {
    fn new() -> State {
        let mut file_node_id_allocator = IdAllocator::new();
        let mut file_nodes = Arena::new();
        let root_file_node_id = file_node_id_allocator.allocate();
        file_nodes.insert(
            root_file_node_id,
            FileNode {
                parent_edge: None,
                name: String::from("root"),
                child_edges: Some(Vec::new()),
            },
        );

        let mut panel_id_allocator = IdAllocator::new();
        let mut panels = Arena::new();
        let mut tab_id_allocator = IdAllocator::new();
        let mut tabs = Arena::new();

        let panel_id_0 = panel_id_allocator.allocate();
        let file_tree_tab_id = tab_id_allocator.allocate();
        panels.insert(
            panel_id_0,
            Panel::TabBar {
                tab_ids: vec![file_tree_tab_id],
                view_id: None,
            },
        );
        tabs.insert(
            file_tree_tab_id,
            Tab {
                panel_id: panel_id_0,
                name: String::from("File Tree"),
                kind: TabKind::FileTree,
            },
        );

        let panel_id_1 = panel_id_allocator.allocate();
        panels.insert(
            panel_id_1,
            Panel::TabBar {
                tab_ids: vec![],
                view_id: None,
            },
        );

        let root_panel_id = panel_id_allocator.allocate();
        panels.insert(
            root_panel_id,
            Panel::Splitter {
                child_ids: [panel_id_0, panel_id_1],
            },
        );

        State {
            panel_id_allocator,
            panels,
            root_panel_id,
            panel_id: panel_id_1,
            tab_id_allocator,
            tabs,
            file_tree_tab_id,
            file_node_id_allocator,
            file_nodes,
            root_file_node_id,
            code_editor_state: code_editor::State::new(),
        }
    }

    fn set_file_tree(&mut self, root: protocol::FileNode) {
        fn create_file_node(
            file_node_id_allocator: &mut IdAllocator,
            file_nodes: &mut Arena<FileNode>,
            parent_edge: Option<FileEdge>,
            node: protocol::FileNode,
        ) -> NodeId {
            let file_node_id = file_node_id_allocator.allocate();
            let name = parent_edge.as_ref().map_or_else(
                || String::from("root"),
                |edge| edge.name.to_string_lossy().into_owned(),
            );
            let file_node = FileNode {
                parent_edge,
                name,
                child_edges: match node {
                    protocol::FileNode::Directory { entries } => Some(
                        entries
                            .into_iter()
                            .map(|entry| FileEdge {
                                name: entry.name.clone(),
                                file_node_id: create_file_node(
                                    file_node_id_allocator,
                                    file_nodes,
                                    Some(FileEdge {
                                        name: entry.name,
                                        file_node_id,
                                    }),
                                    entry.node,
                                ),
                            })
                            .collect::<Vec<_>>(),
                    ),
                    protocol::FileNode::File => None,
                },
            };
            file_nodes.insert(file_node_id, file_node);
            file_node_id
        }

        self.file_node_id_allocator.clear();
        self.file_nodes.clear();
        self.root_file_node_id = create_file_node(
            &mut self.file_node_id_allocator,
            &mut self.file_nodes,
            None,
            root,
        );
    }

    fn file_node_path(&self, file_node_id: FileNodeId) -> PathBuf {
        let mut components = Vec::new();
        let mut node = &self.file_nodes[file_node_id];
        while let Some(edge) = &node.parent_edge {
            components.push(&edge.name);
            node = &self.file_nodes[edge.file_node_id];
        }
        components.into_iter().rev().collect::<PathBuf>()
    }
}

enum Panel {
    Splitter {
        child_ids: [PanelId; 2],
    },
    TabBar {
        tab_ids: Vec<TabId>,
        view_id: Option<ViewId>,
    },
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
    file_node_id: FileNodeId,
}

fn spawn_connection_listener(listener: TcpListener, server: Server) {
    thread::spawn(move || {
        println!("Server listening on {}", listener.local_addr().unwrap());
        for stream in listener.incoming() {
            let stream = stream.unwrap();
            println!("Incoming connection from {}", stream.peer_addr().unwrap());
            let (response_or_notification_sender, response_or_notification_receiver) =
                mpsc::channel();
            let connection = server.connect(Box::new({
                let response_or_notification_sender = response_or_notification_sender.clone();
                move |notification| {
                    response_or_notification_sender
                        .send(ResponseOrNotification::Notification(notification))
                        .unwrap();
                }
            }));
            spawn_remote_request_handler(
                connection,
                stream.try_clone().unwrap(),
                response_or_notification_sender,
            );
            spawn_response_or_notification_sender(response_or_notification_receiver, stream);
        }
    });
}

fn spawn_remote_request_handler(
    connection: Connection,
    mut stream: TcpStream,
    response_or_notification_sender: Sender<ResponseOrNotification>,
) {
    thread::spawn(move || loop {
        let mut len_bytes = [0; 8];
        stream.read_exact(&mut len_bytes).unwrap();
        let len = usize::from_be_bytes(len_bytes);
        let mut request_bytes = vec![0; len];
        stream.read_exact(&mut request_bytes).unwrap();
        let request = bincode::deserialize_from(request_bytes.as_slice()).unwrap();
        let response = connection.handle_request(request);
        response_or_notification_sender
            .send(ResponseOrNotification::Response(response))
            .unwrap();
    });
}

fn spawn_response_or_notification_sender(
    response_or_notification_receiver: Receiver<ResponseOrNotification>,
    mut stream: TcpStream,
) {
    thread::spawn(move || loop {
        let response_or_notification = response_or_notification_receiver.recv().unwrap();
        let mut response_or_notification_bytes = Vec::new();
        bincode::serialize_into(
            &mut response_or_notification_bytes,
            &response_or_notification,
        )
        .unwrap();
        let len_bytes = response_or_notification_bytes.len().to_be_bytes();
        stream.write_all(&len_bytes).unwrap();
        stream.write_all(&response_or_notification_bytes).unwrap();
    });
}

fn spawn_request_sender(request_receiver: Receiver<Request>, mut stream: TcpStream) {
    thread::spawn(move || loop {
        let request = request_receiver.recv().unwrap();
        let mut request_bytes = Vec::new();
        bincode::serialize_into(&mut request_bytes, &request).unwrap();
        let len_bytes = request_bytes.len().to_be_bytes();
        stream.write_all(&len_bytes).unwrap();
        stream.write_all(&request_bytes).unwrap();
    });
}

fn spawn_response_or_notification_receiver(
    mut stream: TcpStream,
    response_or_notification_signal: Signal,
    response_or_notification_sender: Sender<ResponseOrNotification>,
) {
    thread::spawn(move || loop {
        let mut len_bytes = [0; 8];
        stream.read_exact(&mut len_bytes).unwrap();
        let len = usize::from_be_bytes(len_bytes);
        let mut response_or_notification_bytes = vec![0; len];
        stream
            .read_exact(&mut response_or_notification_bytes)
            .unwrap();
        let response_or_notification =
            bincode::deserialize_from(response_or_notification_bytes.as_slice()).unwrap();
        response_or_notification_sender
            .send(response_or_notification)
            .unwrap();
        Cx::post_signal(response_or_notification_signal, StatusId::default());
    });
}

fn spawn_local_request_handler(
    request_receiver: Receiver<Request>,
    connection: Connection,
    response_or_notification_signal: Signal,
    response_or_notification_sender: Sender<ResponseOrNotification>,
) {
    thread::spawn(move || loop {
        let request = request_receiver.recv().unwrap();
        let response = connection.handle_request(request);
        response_or_notification_sender
            .send(ResponseOrNotification::Response(response))
            .unwrap();
        Cx::post_signal(response_or_notification_signal, StatusId::default());
    });
}
