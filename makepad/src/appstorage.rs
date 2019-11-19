//use syn::Type;
use render::*;
use editor::*;
use hub::*;
use crate::appwindow::*;
use crate::filetree::*;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use crate::workspace_main;

#[derive(Debug, Serialize, Deserialize)]
pub struct AppSettings {
    pub build_on_save: bool,
    pub exec_when_done: bool,
    pub hub_server: HubServerConfig,
    pub workspaces: HashMap<String, HubWsConfig>,
    pub builds: Vec<BuildTarget>,
    pub sync: HashMap<String, Vec<String>>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            exec_when_done: false,
            build_on_save: true,
            hub_server: HubServerConfig::Offline,
            workspaces: {
                let mut cfg = HashMap::new();
                cfg.insert("main".to_string(), HubWsConfig {
                    http_server: HttpServerConfig::Offline,
                    projects: {
                        let mut project = HashMap::new();
                        project.insert("makepad".to_string(), "edit_repo".to_string());
                        project
                    }
                });
                cfg.insert("windows".to_string(), HubWsConfig {
                    http_server: HttpServerConfig::Offline,
                    projects: {
                        let mut project = HashMap::new();
                        project.insert("makepad".to_string(), ".".to_string());
                        project
                    }
                });
                cfg
            },
            sync: {
                let mut sync = HashMap::new();
                sync.insert("main/makepad".to_string(), vec!["windows/makepad".to_string()]);
                sync
            },
            builds: vec![BuildTarget {
                workspace: "main".to_string(),
                project: "makepad".to_string(),
                package: "makepad".to_string(),
                config: "check".to_string()
            }]
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuildTarget {
    pub workspace: String,
    pub project: String,
    pub package: String,
    pub config: String
}

pub struct AppStorage {
    pub workspaces_request_uid: HubUid,
    pub workspace_sync_uid: HubUid,
    pub hub_router: Option<HubRouter>,
    pub hub_server: Option<HubServer>,
    pub hub_ui: Option<HubUI>,
    pub hub_ui_message: Signal,
    pub settings_changed: Signal,
    pub settings: AppSettings,
    pub file_tree_file_read: FileRead,
    pub app_state_file_read: FileRead,
    pub app_settings_file_read: FileRead,
    pub text_buffers: HashMap<String, AppTextBuffer>
}

pub struct AppTextBuffer {
    pub file_read: FileRead,
    pub read_msg: Option<ToHubMsg>,
    //pub write_msg: Option<ToHubMsg>,
    pub text_buffer: TextBuffer,
}

impl AppStorage {
    pub fn style(cx: &mut Cx) -> Self {
        AppStorage {
            workspaces_request_uid: HubUid::zero(),
            workspace_sync_uid: HubUid::zero(),
            hub_router: None,
            hub_server: None,
            hub_ui: None,
            hub_ui_message: cx.new_signal(),
            settings_changed: cx.new_signal(),
            settings: AppSettings::default(),
            //rust_compiler: RustCompiler::style(cx),
            text_buffers: HashMap::new(),
            file_tree_file_read: FileRead::default(),
            app_state_file_read: FileRead::default(),
            app_settings_file_read: FileRead::default()
        }
    }
    
    pub fn init(&mut self, cx: &mut Cx) {
        if cx.platform_type.is_desktop() {
            
            self.app_state_file_read = cx.file_read("makepad_state.ron");
            self.app_settings_file_read = cx.file_read("makepad_settings.ron");
            
            // lets start the router
            let mut hub_router = HubRouter::start_hub_router(HubLog::None);
            
            // lets start the hub UI connected directly
            let hub_ui = HubUI::start_hub_ui_direct(&mut hub_router, {
                let signal = self.hub_ui_message.clone();
                move || {
                    Cx::post_signal(signal, 0);
                }
            });
            
            HubWorkspace::run_workspace_direct("main", &mut hub_router, | ws, htc | {workspace_main::workspace(ws, htc)});
            
            self.hub_router = Some(hub_router);
            self.hub_ui = Some(hub_ui);
        }
        else {
            self.file_tree_file_read = cx.file_read("index.ron");
        }
    }
    
    pub fn load_settings(&mut self, cx: &mut Cx, utf8_data: &str) {
        match ron::de::from_str(utf8_data) {
            Ok(settings) => {
                self.settings = settings;
                cx.send_signal(self.settings_changed, 0);
                
                // so now, here we restart our hub_server if need be.
                if cx.platform_type.is_desktop() {
                    self.restart_hub_server();
                }
            },
            Err(e) => {
                println!("Cannot deserialize settings {:?}", e);
            }
        }
    }
    
    pub fn restart_hub_server(&mut self) {
        if let Some(hub_server) = &mut self.hub_server {
            hub_server.terminate();
        }
        
        if let Some(hub_router) = &mut self.hub_router {
            let digest = Self::read_or_generate_key_ron();
            // start the server
            self.hub_server = HubServer::start_hub_server(digest, &self.settings.hub_server, hub_router);
        }
    }
    
    pub fn read_or_generate_key_ron() -> Digest {
        // read or generate key.ron
        if let Ok(utf8_data) = std::fs::read_to_string("key.ron") {
            if let Ok(digest) = ron::de::from_str::<Digest>(&utf8_data) {
                return digest
            }
        }
        
        let digest = Digest::generate();
        if let Ok(utf8_data) = ron::ser::to_string(&digest) {
            if std::fs::write("key.ron", utf8_data.as_bytes()).is_err() {
                println!("Cannot generate key.ron");
            }
        }
        digest
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
                    // write_msg: None,
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
                let uid = hub_ui.route_send.alloc_uid();
                let (workspace, rest) = path.split_at(workspace_pos);
                let (_, rest) = rest.split_at(1);
                let msg = ToHubMsg {
                    to: HubMsgTo::Workspace(workspace.to_string()),
                    msg: HubMsg::FileReadRequest {
                        uid: uid.clone(),
                        path: rest.to_string()
                    }
                };
                hub_ui.route_send.send(msg.clone());
                AppTextBuffer {
                    file_read: FileRead::default(),
                    read_msg: Some(msg),
                    // write_msg: None,
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
            if path.find('/').is_some() {
                if let Some(atb) = self.text_buffers.get_mut(path) {
                    let hub_ui = self.hub_ui.as_mut().unwrap();
                    let utf8_data = atb.text_buffer.get_as_string();
                    fn send_file_write_request(hub_ui: &HubUI, uid: HubUid, path: &str, data: &Vec<u8>) {
                        if let Some(workspace_pos) = path.find('/') {
                            let (workspace, rest) = path.split_at(workspace_pos);
                            let (_, rest) = rest.split_at(1);
                            
                            hub_ui.route_send.send(ToHubMsg {
                                to: HubMsgTo::Workspace(workspace.to_string()),
                                msg: HubMsg::FileWriteRequest {
                                    uid: uid.clone(),
                                    path: rest.to_string(),
                                    data: data.clone()
                                }
                            });
                        }
                    }
                    // lets write it as a message
                    let uid = hub_ui.route_send.alloc_uid();
                    let utf8_bytes = utf8_data.into_bytes();
                    send_file_write_request(hub_ui, uid, path, &utf8_bytes);
                    // lets send our file write to all sync points.
                    for (sync, points) in &self.settings.sync{
                        if path.starts_with(sync){
                            let mut sync_path = path.to_string();
                            for point in points{
                                sync_path.replace_range(0..sync.len(), point);
                                send_file_write_request(hub_ui, uid, &sync_path, &utf8_bytes);
                            }
                        }
                    }
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
    
    pub fn reload_workspaces(&mut self) {
        let hub_ui = self.hub_ui.as_mut().unwrap();
        let uid = hub_ui.route_send.alloc_uid();
        hub_ui.route_send.send(ToHubMsg {
            to: HubMsgTo::Hub,
            msg: HubMsg::ListWorkspacesRequest {uid: uid}
        });
        self.workspaces_request_uid = uid;
    }
    
    pub fn handle_hub_msg(&mut self, cx: &mut Cx, htc: FromHubMsg, windows: &mut Vec<AppWindow>, state: &AppState) {
        let hub_ui = self.hub_ui.as_mut().unwrap();
        // only in ConnectUI of ourselves do we list the workspaces
        match htc.msg {
            // our own connectUI message, means we are ready to talk to the hub
            HubMsg::ConnectUI => if hub_ui.route_send.is_own_addr(&htc.from) {
                // now start talking
                self.reload_workspaces();
            },
            HubMsg::DisconnectWorkspace(_) | HubMsg::ConnectWorkspace(_) => {
                self.reload_workspaces();
            },
            HubMsg::ListWorkspacesResponse {uid, workspaces} => if uid == self.workspaces_request_uid {
                let uid = hub_ui.route_send.alloc_uid();
                // from these workspaces query filetrees
                for workspace in &workspaces {
                    // lets look up a workspace and configure it!
                    // lets config it
                    if let Some(workspace_config) = self.settings.workspaces.get(workspace) {
                        hub_ui.route_send.send(ToHubMsg {
                            to: HubMsgTo::Workspace(workspace.clone()),
                            msg: HubMsg::WorkspaceConfig {uid: uid, config: workspace_config.clone()}
                        });
                    }
                    hub_ui.route_send.send(ToHubMsg {
                        to: HubMsgTo::Workspace(workspace.clone()),
                        msg: HubMsg::WorkspaceFileTreeRequest {uid: uid, create_digest: false}
                    });
                    hub_ui.route_send.send(ToHubMsg {
                        to: HubMsgTo::Workspace(workspace.clone()),
                        msg: HubMsg::ListPackagesRequest {uid: uid}
                    });
                }
                self.workspaces_request_uid = uid;
                // add all workspace nodes
                for window in windows {
                    window.file_panel.file_tree.root_node = FileNode::Folder {
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
                    window.file_panel.file_tree.view.redraw_view_area(cx);
                }
                // lets resend the file load we haven't gotten
                for (_path, atb) in &mut self.text_buffers {
                    if let Some(cth_msg) = &atb.read_msg {
                        hub_ui.route_send.send(cth_msg.clone())
                    }
                }
                
            },
            HubMsg::WorkspaceFileTreeResponse {uid, tree} => if uid == self.workspaces_request_uid {
                // replace a workspace node
                if let WorkspaceFileTreeNode::Folder {name, ..} = &tree {
                    let workspace = name.clone();
                    // insert each filetree at the right childnode
                    for (window_index, window) in windows.iter_mut().enumerate() {
                        if let FileNode::Folder {folder, ..} = &mut window.file_panel.file_tree.root_node {
                            for node in folder.iter_mut() {
                                if let FileNode::Folder {name, ..} = node {
                                    if *name == workspace {
                                        *node = hub_to_tree(&tree);
                                        break
                                    }
                                }
                            }
                        }
                        window.file_panel.file_tree.load_open_folders(cx, &state.windows[window_index].open_folders);
                    }
                }
            },
            HubMsg::FileReadResponse {uid, data, ..} => {
                for (_path, atb) in &mut self.text_buffers {
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
            _ => {}
        }
    }
}

pub fn hub_to_tree(node: &WorkspaceFileTreeNode) -> FileNode {
    match node {
        WorkspaceFileTreeNode::File {name, ..} => FileNode::File {
            name: name.clone(),
            draw: None
        },
        WorkspaceFileTreeNode::Folder {name, folder, ..} => {
            FileNode::Folder {
                name: name.clone(),
                folder: folder.iter().map( | v | hub_to_tree(v)).collect(),
                draw: None,
                state: NodeState::Closed
            }
        }
    }
}