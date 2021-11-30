use {
    crate::{
        code_editor::{CodeEditor, CodeEditorViewId},
        code_editor_state::{CodeEditorState, SessionId},
        dock::{Dock, DockAction, DragPosition, PanelId},
        file_tree::{FileTreeAction, FileNodeId, FileTree},
        genid_allocator::GenIdAllocator,
        genid_map::GenIdMap,
        protocol::{self, Notification, Request, Response, ResponseOrNotification},
        server::{Connection, Server},
        tab_bar::TabId,
    },
    makepad_render::*,
    makepad_widget::*,
    std::{
        env,
        ffi::OsString,
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        path::{Path, PathBuf},
        sync::mpsc::{self, Receiver, Sender, TryRecvError},
        thread,
    },
};

live_register!{
    AppInner: {{AppInner}} {
        window: {caption: "Makepad Studio"}
    }
    
    App: {{App}} {
    }
}

#[derive(Live, LiveHook)]
pub struct App {
    inner: AppInner,
    #[rust(AppState::new())] state: AppState,
}

impl App {
    
    pub fn live_register(cx: &mut Cx) {
        makepad_widget::live_register(cx);
        crate::code_editor::live_register(cx);
        crate::file_tree::live_register(cx);
        crate::splitter::live_register(cx);
        crate::tab_button::live_register(cx);
        crate::tab::live_register(cx);
        crate::tab_bar::live_register(cx);
        crate::dock::live_register(cx);
    }
    
    pub fn new_app(cx: &mut Cx) -> Self {
        Self::new_from_module_path_id(cx, &module_path!(), id!(App)).unwrap()
    }
    
    pub fn draw(&mut self, cx: &mut Cx) {
        //cx.profile_start(0);
        self.inner.draw(cx, &self.state);
        //cx.profile_end(0);
        //cx.debug_draw_tree(false);
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) {
        self.inner.handle_event(cx, event, &mut self.state);
    }
}

pub struct AppIO {
    request_sender: Sender<Request>,
    response_or_notification_signal: Signal,
    response_or_notification_receiver: Receiver<ResponseOrNotification>,
}

#[derive(Live, LiveHook)]
pub struct AppInner {
    
    window: DesktopWindow,
    dock: Dock,
    file_tree: FileTree,
    code_editor: CodeEditor,
    
    #[rust(AppIO::new(cx))] io: AppIO
}

impl AppIO {
    fn new(cx: &mut Cx) -> Self {
        let mut server = Server::new(env::current_dir().unwrap());
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
                        move | notification | {
                            response_or_notification_sender
                                .send(ResponseOrNotification::Notification(notification))
                                .unwrap();
                            Cx::post_signal(response_or_notification_signal, 0);
                        }
                    })),
                    response_or_notification_signal,
                    response_or_notification_sender,
                );
            }
        }
        spawn_connection_listener(TcpListener::bind("127.0.0.1:0").unwrap(), server);
        Self {
            request_sender,
            response_or_notification_signal,
            response_or_notification_receiver,
        }
    }
}

impl AppInner {
    
    fn draw(&mut self, cx: &mut Cx, state: &AppState) {
        if self.window.begin(cx, None).is_ok() {
            if self.dock.begin(cx).is_ok() {
                self.draw_panel(cx, state, state.root_panel_id);
                self.dock.end(cx);
            }
            self.window.end(cx);
        }
    }
    
    fn draw_panel(&mut self, cx: &mut Cx, state: &AppState, panel_id: PanelId) {
        let panel = &state.panels_by_panel_id[panel_id];
        match &panel.kind {
            PanelKind::Split(SplitPanel {child_panel_ids}) => {
                self.dock.begin_split_panel(cx, panel_id);
                self.draw_panel(cx, state, child_panel_ids[0]);
                self.dock.middle_split_panel(cx);
                self.draw_panel(cx, state, child_panel_ids[01]);
                self.dock.end_split_panel(cx);
            }
            PanelKind::Tab(TabPanel {tab_ids, ..}) => {
                self.dock.begin_tab_panel(cx, panel_id);
                if self.dock.begin_tab_bar(cx).is_ok() {
                    for tab_id in tab_ids {
                        let tab = &state.tabs_by_tab_id[*tab_id];
                        self.dock.tab(cx, *tab_id, &tab.name);
                    }
                    self.dock.end_tab_bar(cx);
                }
                if let Some(tab_id) = self.dock.selected_tab_id(cx, panel_id) {
                    let tab = &state.tabs_by_tab_id[tab_id];
                    match tab.kind {
                        TabKind::FileTree => {
                            if self.file_tree.begin(cx).is_ok() {
                                self.draw_file_node(cx, state, state.root_file_node_id);
                                self.file_tree.end(cx);
                            }
                        }
                        TabKind::CodeEditor {..} => {
                            let panel = state.panels_by_panel_id[panel_id].as_tab_panel();
                            self.code_editor.draw(
                                cx,
                                &state.code_editor_state,
                                panel.code_editor_view_id.unwrap(),
                            );
                        }
                    }
                }
                self.dock.end_tab_panel(cx);
            }
        }
    }
    
    fn draw_file_node(&mut self, cx: &mut Cx, state: &AppState, file_node_id: FileNodeId) {
        let file_node = &state.file_nodes_by_file_node_id[file_node_id];
        match &file_node.child_edges {
            Some(child_edges) => {
                if self.file_tree.begin_folder(cx, file_node_id, &file_node.name).is_ok()
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
    
    fn handle_event(&mut self, cx: &mut Cx, event: &mut Event, state: &mut AppState) {
        self.window.handle_event(cx, event);
        
        if let Event::Construct = event {
            self.send_request(Request::GetFileTree());
        }
        
        let mut actions = Vec::new();
        self.dock.handle_event(cx, event, &mut | _, action | actions.push(action));
        for action in actions {
            match action {
                DockAction::SplitPanelChanged(panel_id) => {
                    self.dock.redraw(cx);
                    self.redraw_panel(cx, state, panel_id);
                }
                DockAction::TabBarReceivedDraggedItem(panel_id, item) => {
                    for file_url in &item.file_urls {
                        let path = Path::new(&file_url[7..]).to_path_buf();
                        self.create_code_editor_tab(cx, state, panel_id, None, path);
                    }
                }
                DockAction::TabWasPressed(panel_id, tab_id) => {
                    self.select_tab(cx, state, panel_id, tab_id)
                }
                DockAction::TabButtonWasPressed(panel_id, tab_id) => {
                    let tab = &state.tabs_by_tab_id[tab_id];
                    match tab.kind {
                        TabKind::CodeEditor {session_id} => {
                            let panel = state
                                .panels_by_panel_id
                                .get_mut(panel_id)
                                .unwrap()
                                .as_tab_panel_mut();
                            if let Some(code_editor_view_id) = panel.code_editor_view_id {
                                self.code_editor.set_view_session_id(
                                    cx,
                                    &mut state.code_editor_state,
                                    code_editor_view_id,
                                    None,
                                );
                            }
                            state.code_editor_state.destroy_session(session_id, &mut {
                                let request_sender = &self.io.request_sender;
                                move | request | request_sender.send(request).unwrap()
                            });
                            panel.tab_ids.remove(
                                panel
                                    .tab_ids
                                    .iter()
                                    .position( | existing_tab_id | *existing_tab_id == tab_id)
                                    .unwrap(),
                            );
                            state.tabs_by_tab_id.remove(tab_id);
                            state.tab_id_allocator.deallocate(tab_id.0);
                            self.dock.set_selected_tab_id(cx, panel_id, None);
                            self.dock.redraw_tab_bar(cx, panel_id);
                        }
                        _ => {}
                    }
                }
                DockAction::TabReceivedDraggedItem(panel_id, tab_id, item) => {
                    for file_url in &item.file_urls {
                        let path = Path::new(&file_url[7..]).to_path_buf();
                        self.create_code_editor_tab(cx, state, panel_id, Some(tab_id), path);
                    }
                }
                DockAction::ContentsReceivedDraggedItem(panel_id, position, item) => {
                    let panel_id = match position {
                        DragPosition::Center => panel_id,
                        _ => self.split_tab_panel(cx, state, panel_id, position),
                    };
                    for file_url in &item.file_urls {
                        let path = Path::new(&file_url[7..]).to_path_buf();
                        self.create_code_editor_tab(cx, state, panel_id, None, path);
                    }
                }
            }
        }
        
        let mut actions = Vec::new();
        self.file_tree.handle_event(cx, event, &mut | _cx, action | actions.push(action));
        for action in actions {
            match action {
                FileTreeAction::FileNodeWasClicked(file_node_id) => {
                    let node = &state.file_nodes_by_file_node_id[file_node_id];
                    if node.is_file() {
                        let path = state.file_node_path(file_node_id);
                        self.create_code_editor_tab(cx, state, state.selected_panel_id, None, path);
                    }
                }
                FileTreeAction::FileNodeShouldStartDragging(file_node_id) => {
                    let path = state.file_node_path(file_node_id);
                    self.file_tree.start_dragging_file_node(
                        cx,
                        file_node_id,
                        DraggedItem {
                            file_urls: vec![
                                String::from("file://") + &*path.into_os_string().to_string_lossy(),
                            ],
                        },
                    )
                }
            }
        }
        
        let mut panel_id_stack = vec![state.root_panel_id];
        while let Some(panel_id) = panel_id_stack.pop() {
            let panel = &state.panels_by_panel_id[panel_id];
            match &panel.kind {
                PanelKind::Split(SplitPanel {child_panel_ids}) => {
                    for child_id in child_panel_ids {
                        panel_id_stack.push(*child_id);
                    }
                }
                PanelKind::Tab(TabPanel {
                    code_editor_view_id,
                    ..
                }) => {
                    if let Some(code_editor_view_id) = code_editor_view_id {
                        self.code_editor.handle_event(
                            cx,
                            &mut state.code_editor_state,
                            *code_editor_view_id,
                            event,
                            &mut {
                                let request_sender = &self.io.request_sender;
                                move | request | request_sender.send(request).unwrap()
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
                .contains_key(&self.io.response_or_notification_signal) =>
            {
                loop {
                    match self.io.response_or_notification_receiver.try_recv() {
                        Ok(ResponseOrNotification::Response(response)) => {
                            self.handle_response(cx, state, response)
                        }
                        Ok(ResponseOrNotification::Notification(notification)) => {
                            self.handle_notification(cx, state, notification)
                        }
                        Err(TryRecvError::Empty) => break,
                        _ => panic!(),
                    }
                }
            }
            _ => {}
        }
    }
    
    fn handle_response(&mut self, cx: &mut Cx, state: &mut AppState, response: Response) {
        match response {
            Response::GetFileTree(response) => {
                self.set_file_tree(cx, state, response.unwrap());
                self.select_tab(cx, state, state.side_bar_panel_id, state.file_tree_tab_id);
            }
            response => {
                self.code_editor.handle_response(cx, &mut state.code_editor_state, response, &mut {
                    let request_sender = &self.io.request_sender;
                    move | request | request_sender.send(request).unwrap()
                })
            }
        };
    }
    
    fn handle_notification(&mut self, cx: &mut Cx, state: &mut AppState, notification: Notification) {
        match notification {
            notification => {
                self.code_editor
                    .handle_notification(cx, &mut state.code_editor_state, notification)
            }
        }
    }
    
    fn set_file_tree(&mut self, cx: &mut Cx, state: &mut AppState, file_tree: protocol::FileTree) {
        self.file_tree.forget();
        state.set_file_tree(file_tree);
        self.file_tree.set_file_node_is_expanded(cx, state.root_file_node_id, true, true);
        self.file_tree.redraw(cx);
    }
    
    fn split_tab_panel(
        &mut self,
        cx: &mut Cx,
        state: &mut AppState,
        panel_id: PanelId,
        position: DragPosition,
    ) -> PanelId {
        let panel = &state.panels_by_panel_id[panel_id];
        let parent_panel_id = panel.parent_panel_id;
        let new_parent_panel_id = PanelId(state.panel_id_allocator.allocate());
        let new_panel_id = PanelId(state.panel_id_allocator.allocate());
        
        let panel = &mut state.panels_by_panel_id[panel_id];
        panel.parent_panel_id = Some(new_parent_panel_id);
        
        state.panels_by_panel_id.insert(
            new_panel_id,
            Panel {
                parent_panel_id: Some(new_parent_panel_id),
                kind: PanelKind::Tab(TabPanel {
                    tab_ids: Vec::new(),
                    code_editor_view_id: None,
                }),
            },
        );
        
        state.panels_by_panel_id.insert(
            new_parent_panel_id,
            Panel {
                parent_panel_id,
                kind: PanelKind::Split(SplitPanel {
                    child_panel_ids: match position {
                        DragPosition::Left | DragPosition::Top => [new_panel_id, panel_id],
                        DragPosition::Right | DragPosition::Bottom => [panel_id, new_panel_id],
                        _ => panic!(),
                    },
                }),
            },
        );
        
        if let Some(parent_panel_id) = parent_panel_id {
            let parent_panel = &mut state.panels_by_panel_id[parent_panel_id].as_split_panel_mut();
            let position = parent_panel
                .child_panel_ids
                .iter()
                .position( | child_panel_id | *child_panel_id == panel_id)
                .unwrap();
            parent_panel.child_panel_ids[position] = new_parent_panel_id;
        }
        
        self.dock.set_split_panel_axis(
            cx,
            new_parent_panel_id,
            match position {
                DragPosition::Left | DragPosition::Right => Axis::Horizontal,
                DragPosition::Top | DragPosition::Bottom => Axis::Vertical,
                _ => panic!(),
            },
        );
        
        self.dock.redraw(cx);
        self.redraw_panel(cx, state, panel_id);
        
        new_panel_id
    }
    
    fn create_code_editor_tab(
        &mut self,
        cx: &mut Cx,
        state: &mut AppState,
        panel_id: PanelId,
        next_tab_id: Option<TabId>,
        path: PathBuf,
    ) {
        let tab_id = TabId(state.tab_id_allocator.allocate());
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let session_id = state.code_editor_state.create_session(path, &mut {
            let request_sender = &self.io.request_sender;
            move | request | request_sender.send(request).unwrap()
        });
        state.tabs_by_tab_id.insert(
            tab_id,
            Tab {
                name,
                kind: TabKind::CodeEditor {session_id},
            },
        );
        let panel = state
            .panels_by_panel_id
            .get_mut(panel_id)
            .unwrap()
            .as_tab_panel_mut();
        match next_tab_id {
            Some(next_tab_id) => {
                panel.tab_ids.insert(
                    panel
                        .tab_ids
                        .iter()
                        .position( | existing_tab_id | *existing_tab_id == next_tab_id)
                        .unwrap(),
                    tab_id,
                );
            }
            None => panel.tab_ids.push(tab_id),
        }
        self.select_tab(cx, state, panel_id, tab_id);
    }
    
    fn select_tab(&mut self, cx: &mut Cx, state: &mut AppState, panel_id: PanelId, tab_id: TabId) {
        let tab = &state.tabs_by_tab_id[tab_id];
        self.dock.set_selected_tab_id(cx, panel_id, Some(tab_id));
        self.dock.redraw_tab_bar(cx, panel_id);
        match tab.kind {
            TabKind::CodeEditor {session_id} => {
                self.set_code_editor_view_session_id(cx, state, panel_id, session_id);
            }
            _ => {}
        }
    }
    
    fn set_code_editor_view_session_id(
        &mut self,
        cx: &mut Cx,
        state: &mut AppState,
        panel_id: PanelId,
        session_id: SessionId,
    ) {
        let panel = state
            .panels_by_panel_id
            .get_mut(panel_id)
            .unwrap()
            .as_tab_panel_mut();
        match panel.code_editor_view_id {
            Some(view_id) => {
                self.code_editor.set_view_session_id(
                    cx,
                    &mut state.code_editor_state,
                    view_id,
                    Some(session_id),
                );
            }
            None => {
                panel.code_editor_view_id = Some(self.code_editor.create_view(
                    cx,
                    &mut state.code_editor_state,
                    Some(session_id),
                ));
            }
        }
    }
    
    fn send_request(&mut self, request: Request) {
        self.io.request_sender.send(request).unwrap();
    }
    
    fn redraw_panel(&mut self, cx: &mut Cx, state: &AppState, panel_id: PanelId) {
        match &state.panels_by_panel_id[panel_id].kind {
            PanelKind::Split(panel) => {
                for child_panel_id in panel.child_panel_ids {
                    self.redraw_panel(cx, state, child_panel_id);
                }
            }
            PanelKind::Tab(panel) => {
                self.dock.redraw_tab_bar(cx, panel_id);
                if let Some(code_editor_view_id) = panel.code_editor_view_id {
                    self.code_editor.redraw_view(cx, code_editor_view_id);
                }
            }
        }
    }
}

struct AppState {
    panel_id_allocator: GenIdAllocator,
    panels_by_panel_id: GenIdMap<PanelId, Panel>,
    root_panel_id: PanelId,
    side_bar_panel_id: PanelId,
    selected_panel_id: PanelId,
    tab_id_allocator: GenIdAllocator,
    tabs_by_tab_id: GenIdMap<TabId, Tab>,
    file_tree_tab_id: TabId,
    file_node_id_allocator: GenIdAllocator,
    file_nodes_by_file_node_id: GenIdMap<FileNodeId, FileNode>,
    path: PathBuf,
    root_file_node_id: FileNodeId,
    code_editor_state: CodeEditorState,
}

impl AppState {
    fn new() -> AppState {
        let mut file_node_id_allocator = GenIdAllocator::new();
        let mut file_nodes_by_file_node_id = GenIdMap::new();
        let root_file_node_id = FileNodeId(file_node_id_allocator.allocate());
        file_nodes_by_file_node_id.insert(
            root_file_node_id,
            FileNode {
                parent_edge: None,
                name: String::from("root"),
                child_edges: Some(Vec::new()),
            },
        );
        
        let mut panel_id_allocator = GenIdAllocator::new();
        let mut panels_by_panel_id = GenIdMap::new();
        let mut tab_id_allocator = GenIdAllocator::new();
        let mut tabs_by_tab_id = GenIdMap::new();
        
        let root_panel_id = PanelId(panel_id_allocator.allocate());
        let side_bar_panel_id = PanelId(panel_id_allocator.allocate());
        let file_tree_tab_id = TabId(tab_id_allocator.allocate());
        
        panels_by_panel_id.insert(
            side_bar_panel_id,
            Panel {
                parent_panel_id: Some(root_panel_id),
                kind: PanelKind::Tab(TabPanel {
                    tab_ids: vec![file_tree_tab_id],
                    code_editor_view_id: None,
                }),
            },
        );
        
        tabs_by_tab_id.insert(
            file_tree_tab_id,
            Tab {
                name: String::from("File Tree"),
                kind: TabKind::FileTree,
            },
        );
        
        let content_panel_id = PanelId(panel_id_allocator.allocate());
        panels_by_panel_id.insert(
            content_panel_id,
            Panel {
                parent_panel_id: Some(root_panel_id),
                kind: PanelKind::Tab(TabPanel {
                    tab_ids: vec![],
                    code_editor_view_id: None,
                }),
            },
        );
        
        panels_by_panel_id.insert(
            root_panel_id,
            Panel {
                parent_panel_id: None,
                kind: PanelKind::Split(SplitPanel {
                    child_panel_ids: [side_bar_panel_id, content_panel_id],
                }),
            },
        );
        
        AppState {
            panel_id_allocator,
            panels_by_panel_id,
            root_panel_id,
            side_bar_panel_id,
            selected_panel_id: content_panel_id,
            tab_id_allocator,
            tabs_by_tab_id,
            file_tree_tab_id,
            file_node_id_allocator,
            file_nodes_by_file_node_id,
            path: PathBuf::new(),
            root_file_node_id,
            code_editor_state: CodeEditorState::new(),
        }
    }
    
    fn file_node_path(&self, file_node_id: FileNodeId) -> PathBuf {
        let mut components = Vec::new();
        let mut file_node = &self.file_nodes_by_file_node_id[file_node_id];
        while let Some(edge) = &file_node.parent_edge {
            components.push(&edge.name);
            file_node = &self.file_nodes_by_file_node_id[edge.file_node_id];
        }
        self.path
            .join(components.into_iter().rev().collect::<PathBuf>())
    }
    
    fn set_file_tree(&mut self, file_tree: protocol::FileTree) {
        fn create_file_node(
            file_node_id_allocator: &mut GenIdAllocator,
            file_nodes_by_file_node_id: &mut GenIdMap<FileNodeId, FileNode>,
            parent_edge: Option<FileEdge>,
            node: protocol::FileNode,
        ) -> FileNodeId {
            let file_node_id = FileNodeId(file_node_id_allocator.allocate());
            let name = parent_edge.as_ref().map_or_else(
                || String::from("root"),
                | edge | edge.name.to_string_lossy().into_owned(),
            );
            let node = FileNode {
                parent_edge,
                name,
                child_edges: match node {
                    protocol::FileNode::Directory {entries} => Some(
                        entries
                            .into_iter()
                            .map( | entry | FileEdge {
                            name: entry.name.clone(),
                            file_node_id: create_file_node(
                                file_node_id_allocator,
                                file_nodes_by_file_node_id,
                                Some(FileEdge {
                                    name: entry.name,
                                    file_node_id,
                                }),
                                entry.node,
                            ),
                        })
                            .collect::<Vec<_ >> (),
                    ),
                    protocol::FileNode::File => None,
                },
            };
            file_nodes_by_file_node_id.insert(file_node_id, node);
            file_node_id
        }
        
        self.path = file_tree.path;
        self.file_node_id_allocator.clear();
        self.file_nodes_by_file_node_id.clear();
        self.root_file_node_id = create_file_node(
            &mut self.file_node_id_allocator,
            &mut self.file_nodes_by_file_node_id,
            None,
            file_tree.root,
        );
    }
}

#[derive(Debug)]
struct Panel {
    parent_panel_id: Option<PanelId>,
    kind: PanelKind,
}

impl Panel {
    fn as_split_panel_mut(&mut self) -> &mut SplitPanel {
        match &mut self.kind {
            PanelKind::Split(panel) => panel,
            _ => panic!(),
        }
    }
    
    fn as_tab_panel(&self) -> &TabPanel {
        match &self.kind {
            PanelKind::Tab(panel) => panel,
            _ => panic!(),
        }
    }
    
    fn as_tab_panel_mut(&mut self) -> &mut TabPanel {
        match &mut self.kind {
            PanelKind::Tab(panel) => panel,
            _ => panic!(),
        }
    }
}

#[derive(Clone, Debug)]
enum PanelKind {
    Split(SplitPanel),
    Tab(TabPanel),
}

#[derive(Clone, Debug)]
struct SplitPanel {
    child_panel_ids: [PanelId; 2],
}

#[derive(Clone, Debug)]
struct TabPanel {
    tab_ids: Vec<TabId>,
    code_editor_view_id: Option<CodeEditorViewId>,
}

struct Tab {
    name: String,
    kind: TabKind,
}

enum TabKind {
    FileTree,
    CodeEditor {session_id: SessionId},
}

#[derive(Debug)]
struct FileNode {
    parent_edge: Option<FileEdge>,
    name: String,
    child_edges: Option<Vec<FileEdge >>,
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

fn spawn_connection_listener(listener: TcpListener, mut server: Server) {
    thread::spawn(move || {
        println!("Server listening on {}", listener.local_addr().unwrap());
        for stream in listener.incoming() {
            let stream = stream.unwrap();
            println!("Incoming connection from {}", stream.peer_addr().unwrap());
            let (response_or_notification_sender, response_or_notification_receiver) =
            mpsc::channel();
            let connection = server.connect(Box::new({
                let response_or_notification_sender = response_or_notification_sender.clone();
                move | notification | {
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
        
        let request = DeBin::deserialize_bin(request_bytes.as_slice()).unwrap();
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
        
        response_or_notification.ser_bin(&mut response_or_notification_bytes);
        
        let len_bytes = response_or_notification_bytes.len().to_be_bytes();
        stream.write_all(&len_bytes).unwrap();
        stream.write_all(&response_or_notification_bytes).unwrap();
    });
}

fn spawn_request_sender(request_receiver: Receiver<Request>, mut stream: TcpStream) {
    thread::spawn(move || loop {
        let request = request_receiver.recv().unwrap();
        let mut request_bytes = Vec::new();
        request.ser_bin(&mut request_bytes);
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
        DeBin::deserialize_bin(response_or_notification_bytes.as_slice()).unwrap();
        response_or_notification_sender
            .send(response_or_notification)
            .unwrap();
        Cx::post_signal(response_or_notification_signal, 0);
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
        Cx::post_signal(response_or_notification_signal, 0);
    });
}
