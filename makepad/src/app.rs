//use syn::Type;
use render::*;
use widget::*;
use editor::*;
use crate::appwindow::*;
use crate::hubui::*;
use crate::filetree::*;
use crate::buildmanager::*;
use crate::workspace_main;
use std::collections::HashMap;
use serde::*;
use hub::*;

#[derive(Debug, Serialize, Deserialize)]
pub struct AppSettings {
    pub build_on_save: bool,
    pub exec_when_done: bool,
    pub builds: Vec<BuildTarget>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            exec_when_done: false,
            build_on_save: true,
            builds: vec![BuildTarget {
                workspace: "makepad".to_string(),
                package: "makepad".to_string(),
                config: "check".to_string()
            }]
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BuildTarget {
    pub workspace: String,
    pub package: String,
    pub config: String
}

pub struct AppStorage {
    pub hub_server: Option<HubServer>,
    pub hub_ui: Option<HubUI>,
    pub settings_changed: Signal,
    pub settings: AppSettings,
    pub file_tree_file_read: FileRead,
    pub app_state_file_read: FileRead,
    pub app_settings_file_read: FileRead,
    pub text_buffers: HashMap<String, AppTextBuffer>
}

pub struct App {
    pub workspaces_request_uid: HubUid,
    pub app_window_state_template: AppWindowState,
    pub app_window_template: AppWindow,
    pub state: AppState,
    pub storage: AppStorage,
    pub build_manager: BuildManager,
    pub windows: Vec<AppWindow>,
}

pub struct AppTextBuffer {
    pub file_read: FileRead,
    pub read_msg: Option<ClientToHubMsg>,
    pub write_msg: Option<ClientToHubMsg>,
    pub text_buffer: TextBuffer,
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct AppState {
    pub windows: Vec<AppWindowState>
}

impl AppStorage {
    
    pub fn load_settings(&mut self, cx: &mut Cx, utf8_data: &str) {
        match ron::de::from_str(utf8_data) {
            Ok(settings) => {
                self.settings = settings;
                cx.send_signal(self.settings_changed, 0);
                //println!("Succesfully loaded makepad_settings.ron");
            },
            Err(e) => {
                println!("Cannot deserialize settings {:?}", e);
            }
        }
    }
    
    pub fn save_state(&mut self, cx: &mut Cx, state: &AppState) {
        let ron = ron::ser::to_string_pretty(state, ron::ser::PrettyConfig::default()).unwrap();
        cx.file_write("makepad_state.ron", ron.as_bytes());
    }
    
    pub fn text_buffer_from_path(&mut self, cx: &mut Cx, path: &str) -> &mut TextBuffer {
        
        // if online, fallback to readfile
        let atb = if !cx.platform_type.is_desktop() || path.find('/').is_none() {
            let atb = self.text_buffers.entry(path.to_string()).or_insert_with( || {
                AppTextBuffer {
                    file_read: cx.file_read(path),
                    read_msg: None,
                    write_msg: None,
                    text_buffer: TextBuffer {
                        is_loading: true,
                        signal: cx.new_signal(),
                        mutation_id: 1,
                        ..Default::default()
                    }
                }
            });
            atb
        }
        else {
            let hub_ui = self.hub_ui.as_mut().unwrap();
            let atb = self.text_buffers.entry(path.to_string()).or_insert_with( || {
                // lets find the right workspace
                let workspace_pos = path.find('/').unwrap();
                let uid = hub_ui.alloc_uid();
                let (workspace, rest) = path.split_at(workspace_pos);
                let (_, rest) = rest.split_at(1);
                let msg = ClientToHubMsg {
                    to: HubMsgTo::Workspace(workspace.to_string()),
                    msg: HubMsg::FileReadRequest {
                        uid: uid.clone(),
                        path: rest.to_string()
                    }
                };
                hub_ui.send(msg.clone());
                AppTextBuffer {
                    file_read: FileRead::default(),
                    read_msg: Some(msg),
                    write_msg: None,
                    text_buffer: TextBuffer {
                        is_loading: true,
                        signal: cx.new_signal(),
                        mutation_id: 1,
                        ..Default::default()
                    }
                }
            });
            atb
        };
        &mut atb.text_buffer
    }
    
    pub fn text_buffer_file_write(&mut self, cx: &mut Cx, path: &str) {
        if cx.platform_type.is_desktop() {
            if let Some(workspace_pos) = path.find('/') {
                if let Some(atb) = self.text_buffers.get_mut(path) {
                    let hub_ui = self.hub_ui.as_mut().unwrap();
                    let utf8_data = atb.text_buffer.get_as_string();
                    let (workspace, rest) = path.split_at(workspace_pos);
                    let (_, rest) = rest.split_at(1);
                    
                    // lets write it as a message
                    let uid = hub_ui.alloc_uid();
                    let msg = ClientToHubMsg {
                        to: HubMsgTo::Workspace(workspace.to_string()),
                        msg: HubMsg::FileWriteRequest {
                            uid: uid.clone(),
                            path: rest.to_string(),
                            data: utf8_data.into_bytes()
                        }
                    };
                    hub_ui.send(msg.clone());
                    atb.write_msg = Some(msg);
                }
            }
            else { // its not a workspace, its a system (settings) file
                if let Some(atb) = self.text_buffers.get_mut(path) {
                    let utf8_data = atb.text_buffer.get_as_string();
                    cx.file_write(path, utf8_data.as_bytes());
                    // if its the settings, load it
                    if path == "makepad_settings.ron" {
                        self.load_settings(cx, &utf8_data);
                    };
                }
            }
        }
    }
}

impl App {
    
    pub fn style(cx: &mut Cx) -> Self {
        set_dark_style(cx);
        
        Self {
            workspaces_request_uid: HubUid::zero(),
            app_window_template: AppWindow::style(cx),
            app_window_state_template: AppWindowState {
                open_folders: Vec::new(),
                window_inner_size: Vec2::zero(),
                window_position: Vec2::zero(),
                dock_items: DockItem::Splitter {
                    axis: Axis::Vertical,
                    align: SplitterAlign::First,
                    pos: 150.0,
                    first: Box::new(DockItem::TabControl {
                        current: 0,
                        previous: 0,
                        tabs: vec![DockTab {
                            closeable: false,
                            title: "Files".to_string(),
                            item: Panel::FileTree
                        }]
                    }),
                    last: Box::new(DockItem::Splitter {
                        axis: Axis::Horizontal,
                        align: SplitterAlign::Last,
                        pos: 150.0,
                        first: Box::new(DockItem::TabControl {
                            current: 1,
                            previous: 0,
                            tabs: vec![
                                DockTab {
                                    closeable: false,
                                    title: "Edit".to_string(),
                                    item: Panel::FileEditorTarget
                                },
                                DockTab {
                                    closeable: true,
                                    title: "main.rs".to_string(),
                                    item: Panel::FileEditor {
                                        path: "examples/quad_example/src/main.rs".to_string(),
                                        scroll_pos: Vec2::zero(),
                                        editor_id: 1
                                    }
                                }
                            ],
                        }),
                        last: Box::new(DockItem::TabControl {
                            current: 0,
                            previous: 0,
                            tabs: vec![
                                DockTab {
                                    closeable: false,
                                    title: "Log List".to_string(),
                                    item: Panel::LogList
                                },
                                DockTab {
                                    closeable: false,
                                    title: "Log Item".to_string(),
                                    item: Panel::LogItem
                                },
                                DockTab {
                                    closeable: false,
                                    title: "Local Terminal".to_string(),
                                    item: Panel::LocalTerminal {start_path: "./".to_string(), terminal_id: 1}
                                },
                                DockTab {
                                    closeable: false,
                                    title: "Keyboard".to_string(),
                                    item: Panel::Keyboard
                                }
                            ]
                        })
                    })
                },
            },
            windows: vec![],
            build_manager: BuildManager::new(cx),
            state: AppState::default(),
            storage: AppStorage {
                hub_server: None,
                hub_ui: None,
                settings_changed: cx.new_signal(),
                settings: AppSettings::default(),
                //rust_compiler: RustCompiler::style(cx),
                text_buffers: HashMap::new(),
                file_tree_file_read: FileRead::default(),
                app_state_file_read: FileRead::default(),
                app_settings_file_read: FileRead::default()
            }
        }
    }
    
    pub fn default_layout(&mut self, cx: &mut Cx) {
        self.state.windows = vec![self.app_window_state_template.clone()];
        self.windows = vec![self.app_window_template.clone()];
        cx.redraw_child_area(Area::All);
    }
    
    pub fn reload_workspaces(&mut self) {
        let hub_ui = self.storage.hub_ui.as_mut().unwrap();
        let uid = hub_ui.alloc_uid();
        hub_ui.send(ClientToHubMsg {
            to: HubMsgTo::Hub,
            msg: HubMsg::ListWorkspacesRequest {uid: uid}
        });
        self.workspaces_request_uid = uid;
    }
    
    pub fn handle_hub_msg(&mut self, cx: &mut Cx, htc: HubToClientMsg) {
        let hub_ui = self.storage.hub_ui.as_mut().unwrap();
        // only in ConnectUI of ourselves do we list the workspaces
        match htc.msg {
            // our own connectUI message, means we are ready to talk to the hub
            HubMsg::ConnectUI => if hub_ui.is_own_addr(&htc.from) {
                // now start talking
                self.reload_workspaces();
            },
            HubMsg::DisconnectWorkspace(_) | HubMsg::ConnectWorkspace(_) => {
                self.reload_workspaces();
            },
            HubMsg::ListWorkspacesResponse {uid, workspaces} => if uid == self.workspaces_request_uid {
                let uid = hub_ui.alloc_uid();
                // from these workspaces query filetrees
                for workspace in &workspaces {
                    hub_ui.send(ClientToHubMsg {
                        to: HubMsgTo::Workspace(workspace.clone()),
                        msg: HubMsg::WorkspaceFileTreeRequest {uid: uid}
                    });
                    hub_ui.send(ClientToHubMsg {
                        to: HubMsgTo::Workspace(workspace.clone()),
                        msg: HubMsg::PackagesRequest {uid: uid}
                    });
                }
                self.workspaces_request_uid = uid;
                // add all workspace nodes
                for window in &mut self.windows {
                    window.file_tree.root_node = FileNode::Folder {
                        name: "".to_string(),
                        draw: None,
                        state: NodeState::Open,
                        folder: workspaces.iter().map( | v | FileNode::Folder {
                            name: v.clone(),
                            draw: None,
                            state: NodeState::Open,
                            folder: Vec::new()
                        }).chain(std::iter::once(
                            FileNode::File {
                                name: "makepad_settings.ron".to_string(),
                                draw: None
                            }
                        )).collect()
                    };
                    window.file_tree.view.redraw_view_area(cx);
                }
                // lets resend the file load we haven't gotten
                for (_path, atb) in &mut self.storage.text_buffers {
                    if let Some(cth_msg) = &atb.read_msg {
                        hub_ui.send(cth_msg.clone())
                    }
                }
                
            },
            HubMsg::WorkspaceFileTreeResponse {uid, tree} => if uid == self.workspaces_request_uid {
                // replace a workspace node

                if let WorkspaceFileTreeNode::Folder {name, ..} = &tree {
                    let workspace = name.clone();
                    // insert each filetree at the right childnode
                    for (window_index, window) in self.windows.iter_mut().enumerate() {
                        if let FileNode::Folder {folder, ..} = &mut window.file_tree.root_node {
                            for node in folder.iter_mut() {
                                if let FileNode::Folder {name, ..} = node {
                                    if *name == workspace {
                                        *node = hub_to_tree(&tree);
                                        break
                                    }
                                }
                            }
                        }
                        window.file_tree.load_open_folders(cx, &self.state.windows[window_index].open_folders);
                    }
                }
            },
            HubMsg::FileReadResponse {uid, data, ..} => {
                for (_path, atb) in &mut self.storage.text_buffers {
                    if let Some(cth_msg) = &atb.read_msg {
                        if let HubMsg::FileReadRequest {uid: read_uid, ..} = &cth_msg.msg {
                            if *read_uid == uid {
                                atb.read_msg = None;
                                if let Some(data) = data {
                                    if let Ok(utf8_data) = String::from_utf8(data) {
                                        atb.text_buffer.load_from_utf8(cx, &utf8_data);
                                    }
                                }
                                else {
                                    // DO SOMETHING HERE
                                    println!("FILE READ FAILED!")
                                }
                                break
                            }
                        }
                    }
                }
            },
            _ => { // send the rest to cargo_log
                self.build_manager.handle_hub_msg(cx, &mut self.storage, &htc)
            }
        }
    }
    
    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        match event {
            Event::Construct => {
                // start the workspace
                if cx.platform_type.is_desktop() {
                    std::thread::spawn(move || {
                        workspace_main::main();
                    });
                    self.storage.app_state_file_read = cx.file_read("makepad_state.ron");
                    self.storage.app_settings_file_read = cx.file_read("makepad_settings.ron");
                    
                    let key = std::fs::read("./key.bin").unwrap();
                    let mut hub_server = HubServer::start_hub_server_default(&key, HubLog::None);
                    hub_server.start_announce_server_default(&key);
                    let hub_ui = HubUI::new(cx, &key, HubLog::None);
                    
                    self.storage.hub_server = Some(hub_server);
                    
                    self.storage.hub_ui = Some(hub_ui);
                }
                else {
                    self.storage.file_tree_file_read = cx.file_read("index.ron");
                    self.default_layout(cx);
                }
                
            },
            Event::KeyDown(ke) => match ke.key_code {
                KeyCode::KeyR => if ke.modifiers.logo || ke.modifiers.control {
                    self.reload_workspaces();
                },
                _ => ()
            },
            Event::Signal(se) => {
                if let Some(hub_ui) = &mut self.storage.hub_ui {
                    if let Some(mut msgs) = hub_ui.process_signal(se) {
                        for htc in msgs.drain(..) {
                            self.handle_hub_msg(cx, htc);
                        }
                        return
                    }
                }
            },
            Event::FileRead(fr) => {
                // lets see which file we loaded
                if let Some(utf8_data) = self.storage.file_tree_file_read.resolve_utf8(fr) {
                    if let Ok(utf8_data) = utf8_data {
                        if let Ok(tree) = ron::de::from_str(utf8_data){
                            for window in &mut self.windows{
                                window.file_tree.root_node = hub_to_tree(&tree);
                                if let FileNode::Folder {folder, state, name, ..} = &mut window.file_tree.root_node {
                                    *name = "".to_string();
                                    *state = NodeState::Open;
                                     for node in folder.iter_mut() {
                                        if let FileNode::Folder {state, ..} = node {
                                            *state = NodeState::Open
                                        }
                                    }
                                }
                                window.file_tree.view.redraw_view_area(cx);
                            }
                        }
                    }
                }
                else if let Some(utf8_data) = self.storage.app_state_file_read.resolve_utf8(fr) {
                    if let Ok(utf8_data) = utf8_data {
                        if let Ok(state) = ron::de::from_str(&utf8_data) {
                            self.state = state;
                            
                            // create our windows with the serialized positions/size
                            for window_state in &self.state.windows {
                                let mut size = window_state.window_inner_size;
                                
                                if size.x <= 10. {
                                    size.x = 800.;
                                }
                                if size.y <= 10. {
                                    size.y = 600.;
                                }
                                let last_pos = window_state.window_position;
                                let create_pos;
                                if last_pos.x < -1000. || last_pos.y < -1000. {
                                    create_pos = None;
                                }
                                else {
                                    create_pos = Some(last_pos);
                                }
                                self.windows.push(AppWindow {
                                    desktop_window: DesktopWindow {window: Window {
                                        create_inner_size: Some(size),
                                        create_position: create_pos,
                                        ..Window::style(cx)
                                    }, ..self.app_window_template.desktop_window.clone()},
                                    ..self.app_window_template.clone()
                                })
                            }
                            cx.redraw_child_area(Area::All);
                            
                        }
                    }
                    else { // load default window
                        self.default_layout(cx);
                    }
                }
                else if let Some(utf8_data) = self.storage.app_settings_file_read.resolve_utf8(fr) {
                    if let Ok(utf8_data) = utf8_data {
                        self.storage.load_settings(cx, utf8_data);
                    }
                    else { // create default settings file
                        let ron = ron::ser::to_string_pretty(&self.storage.settings, ron::ser::PrettyConfig::default()).expect("cannot serialize settings");
                        cx.file_write("makepad_settings.ron", ron.as_bytes());
                    }
                }
                else {
                    for (_path, atb) in &mut self.storage.text_buffers {
                        if let Some(utf8_data) = atb.file_read.resolve_utf8(fr) {
                            if let Ok(utf8_data) = utf8_data {
                                atb.text_buffer.load_from_utf8(cx, utf8_data);
                                break;
                            }
                        }
                    }
                }
            },
            
            _ => ()
        }
        for (window_index, window) in self.windows.iter_mut().enumerate() {
            window.handle_app_window(cx, event, window_index, &mut self.state, &mut self.storage, &mut self.build_manager);
            // break;
        }
    }
    
    
    pub fn draw_app(&mut self, cx: &mut Cx) {
        //return;
        for (window_index, window) in self.windows.iter_mut().enumerate() {
            window.draw_app_window(cx, window_index, &mut self.state, &mut self.storage, &mut self.build_manager);
            // break;
        }
    }
}

fn hub_to_tree(node: &WorkspaceFileTreeNode) -> FileNode {
    match node {
        WorkspaceFileTreeNode::File {name} => FileNode::File {
            name: name.clone(),
            draw: None
        },
        WorkspaceFileTreeNode::Folder {name, folder} => {
            FileNode::Folder {
                name: name.clone(),
                folder: folder.iter().map( | v | hub_to_tree(v)).collect(),
                draw: None,
                state: NodeState::Closed
            }
        }
    }
}