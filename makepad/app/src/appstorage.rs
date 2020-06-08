//use syn::Type;
use makepad_render::*;
use makepad_widget::*;
use makepad_hub::*;
use makepad_tinyserde::*;
use crate::appwindow::*;
use crate::filetree::*;
use crate::fileeditor::*;
use crate::buildmanager::*;
use std::collections::HashMap;
use crate::builder;
use crate::livemacro::*;

#[derive(Debug, Clone, SerRon, DeRon)]
pub struct AppSettings {
    pub build_on_save: bool,
    pub exec_when_done: bool,
    pub style_options: StyleOptions,
    pub hub_server: HubServerConfig,
    pub builders: HashMap<String, HubBuilderConfig>,
    pub builds: Vec<BuildTarget>,
    pub sync: HashMap<String, Vec<String>>,
}

impl Default for AppSettings {
    
    fn default() -> Self {
        Self {
            exec_when_done: false,
            build_on_save: true,
            style_options: StyleOptions {scale: 1.0, dark: true},
            hub_server: HubServerConfig::Offline,
            builders: HashMap::new(),
            sync: HashMap::new(),
            builds: vec![]
        }
    }
}

impl AppSettings {
    pub fn initial() -> Self {
        Self {
            exec_when_done: false,
            build_on_save: true,
            style_options: StyleOptions {scale: 1.0, dark: true},
            hub_server: HubServerConfig::Offline,
            builders: {
                let mut cfg = HashMap::new();
                cfg.insert("main".to_string(), HubBuilderConfig {
                    http_server: HttpServerConfig::Localhost(8000),
                    workspaces: {
                        let mut workspace = HashMap::new();
                        workspace.insert("makepad".to_string(), ".".to_string());
                        workspace
                    }
                });
                cfg
            },
            sync: {
                let sync = HashMap::new();
                //sync.insert("main/makepad".to_string(), vec!["windows/makepad".to_string()]);
                sync
            },
            builds: vec![
                BuildTarget {
                    builder: "main".to_string(),
                    workspace: "makepad".to_string(),
                    package: "webgl_example_wasm".to_string(),
                    config: "release".to_string()
                }
            ]
        }
    }
}

#[derive(Clone, Debug, SerRon, DeRon, PartialEq)]
pub struct BuildTarget {
    pub builder: String,
    pub workspace: String,
    pub package: String,
    pub config: String
}

pub struct AppStorage {
    pub init_builders_counter: usize,
    pub builders_request_uid: HubUid,
    pub builder_sync_uid: HubUid,
    pub hub_router: Option<HubRouter>,
    pub hub_server: Option<HubServer>,
    pub builder_route_send: Option<HubRouteSend>,
    pub hub_ui: Option<HubUI>,
    pub hub_ui_message: Signal,
    pub settings_changed: Signal,
    pub settings_old: AppSettings,
    pub settings: AppSettings,
    pub file_tree_file_read: FileRead,
    pub app_state_file_read: FileRead,
    pub app_settings_file_read: FileRead,
    pub text_buffer_path_to_id: HashMap<String, AppTextBufferId>,
    pub text_buffer_id_to_path: HashMap<AppTextBufferId, String>,
    pub text_buffers: Vec<AppTextBuffer>,
}


pub struct AppTextBuffer {
    pub file_read: FileRead,
    pub read_msg: Option<ToHubMsg>,
    pub full_path: String,
    pub text_buffer: TextBuffer,
    pub text_buffer_id: AppTextBufferId,
    pub live_macros: LiveMacros
}

#[derive(Clone, Copy, Default, PartialEq, Ord, PartialOrd, Hash, Eq)]
pub struct AppTextBufferId(pub u16);
impl AppTextBufferId{
    pub fn as_index(&self )->usize{return self.0 as usize}
}

impl AppStorage {
    pub fn new(cx: &mut Cx) -> Self {
        AppStorage {
            init_builders_counter: 2,
            builders_request_uid: HubUid::zero(),
            builder_sync_uid: HubUid::zero(),
            builder_route_send: None,
            hub_router: None,
            hub_server: None,
            hub_ui: None,
            hub_ui_message: cx.new_signal(),
            settings_changed: cx.new_signal(),
            settings_old: AppSettings::default(),
            settings: AppSettings::default(),
            //rust_compiler: RustCompiler::style(cx),
            text_buffer_path_to_id: HashMap::new(),
            text_buffer_id_to_path: HashMap::new(),
            text_buffers: Vec::new(),
            file_tree_file_read: FileRead::default(),
            app_state_file_read: FileRead::default(),
            app_settings_file_read: FileRead::default()
        }
    }
     
    pub fn status_new_message() -> StatusId {uid!()}
    pub fn status_settings_changed() -> StatusId {uid!()}
    
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
                    Cx::post_signal(signal, Self::status_new_message());
                }
            });
            
            let send = HubBuilder::run_builder_direct("main", &mut hub_router, | ws, htc | {builder::builder(ws, htc)});
            self.builder_route_send = Some(send);
            self.hub_router = Some(hub_router);
            self.hub_ui = Some(hub_ui);
        }
        else {
            self.file_tree_file_read = cx.file_read("index.ron");
        }
    }
    
    pub fn load_settings(&mut self, cx: &mut Cx, utf8_data: &str) {
        match DeRon::deserialize_ron(utf8_data) {
            Ok(settings) => {
                self.settings_old = self.settings.clone();
                self.settings = settings;
                self.settings.style_options.scale = self.settings.style_options.scale.min(3.0).max(0.3);
                cx.send_signal(self.settings_changed, Self::status_settings_changed());
                
                // so now, here we restart our hub_server if need be.
                if cx.platform_type.is_desktop() {
                    if self.settings_old.hub_server != self.settings.hub_server {
                        self.restart_hub_server();
                    }
                }
            },
            Err(e) => {
                println!("Cannot deserialize settings {:?}", e);
            }
        }
    }
    
    pub fn save_settings(&mut self, cx: &mut Cx) {
        let utf8_data = self.settings.serialize_ron();
        let path = "makepad_settings.ron";
        if let Some(tb_id) = self.text_buffer_path_to_id.get(path) {
            let atb = &mut self.text_buffers[tb_id.0 as usize];
            atb.text_buffer.load_from_utf8(&utf8_data);
            atb.text_buffer.send_textbuffer_loaded_signal(cx);
        }
        cx.file_write(path, utf8_data.as_bytes());
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
            if let Ok(digest) = DeRon::deserialize_ron(&utf8_data) {
                return digest
            }
        }
        let digest = Digest::generate();
        let utf8_data = digest.serialize_ron();
        if std::fs::write("key.ron", utf8_data.as_bytes()).is_err() {
            println!("Cannot generate key.ron");
        }
        digest
    }
    
    pub fn save_state(&mut self, cx: &mut Cx, state: &AppState) {
        let ron = state.serialize_ron();
        cx.file_write("makepad_state.ron", ron.as_bytes());
    }
    
    pub fn remap_sync_path(&self, path: &str) -> String {
        let mut path = path.to_string();
        for (key, sync_to) in &self.settings.sync {
            for sync in sync_to {
                if path.starts_with(sync) {
                    path.replace_range(0..sync.len(), key);
                    break
                }
            }
        }
        path
    }
    
    
    pub fn text_buffer_from_path(&mut self, cx: &mut Cx, path: &str) -> &mut AppTextBuffer {
        
        // if online, fallback to readfile
        if !cx.platform_type.is_desktop() || path.find('/').is_none() {
            if let Some(tb_id) = self.text_buffer_path_to_id.get(path) {
                &mut self.text_buffers[tb_id.0 as usize]
            }
            else {
                let tb_id = AppTextBufferId(self.text_buffers.len() as u16);
                self.text_buffer_path_to_id.insert(path.to_string(), tb_id);
                self.text_buffer_id_to_path.insert(tb_id, path.to_string());
                self.text_buffers.push(AppTextBuffer {
                    file_read: cx.file_read(path),
                    read_msg: None,
                    full_path: path.to_string(),
                    text_buffer_id: tb_id,
                    live_macros: LiveMacros::default(),
                    // write_msg: None,
                    text_buffer: TextBuffer {
                        signal: cx.new_signal(),
                        ..TextBuffer::default()
                    }
                });
                &mut self.text_buffers[tb_id.0 as usize]
            }
        }
        else {
            let hub_ui = self.hub_ui.as_mut().unwrap();
            if let Some(tb_id) = self.text_buffer_path_to_id.get(path) {
                &mut self.text_buffers[tb_id.0 as usize]
            }
            else {
                let builder_pos = path.find('/').unwrap();
                let uid = hub_ui.route_send.alloc_uid();
                let (builder, rest) = path.split_at(builder_pos);
                let (_, rest) = rest.split_at(1);
                let msg = ToHubMsg {
                    to: HubMsgTo::Builder(builder.to_string()),
                    msg: HubMsg::FileReadRequest {
                        uid: uid.clone(),
                        path: rest.to_string()
                    }
                };
                hub_ui.route_send.send(msg.clone());
                
                let tb_id = AppTextBufferId(self.text_buffers.len() as u16);
                self.text_buffer_path_to_id.insert(path.to_string(), tb_id);
                self.text_buffer_id_to_path.insert(tb_id, path.to_string());
                self.text_buffers.push(AppTextBuffer {
                    file_read: FileRead::default(),
                    read_msg: Some(msg),
                    full_path: path.to_string(),
                    text_buffer_id: tb_id,
                    live_macros: LiveMacros::default(),
                    // write_msg: None,
                    text_buffer: TextBuffer {
                        signal: cx.new_signal(),
                        ..TextBuffer::default()
                    }
                    
                });
                &mut self.text_buffers[tb_id.0 as usize]
            }
        }
    }
    
    pub fn text_buffer_file_write(&mut self, cx: &mut Cx, path: &str) {
        if cx.platform_type.is_desktop() {
            if path.find('/').is_some() {
                if let Some(tb_id) = self.text_buffer_path_to_id.get(path) {
                    let atb = &self.text_buffers[tb_id.0 as usize];
                    let hub_ui = self.hub_ui.as_mut().unwrap();
                    let utf8_data = atb.text_buffer.get_as_string();
                    fn send_file_write_request(hub_ui: &HubUI, uid: HubUid, path: &str, data: &Vec<u8>) {
                        if let Some(builder_pos) = path.find('/') {
                            let (builder, rest) = path.split_at(builder_pos);
                            let (_, rest) = rest.split_at(1);
                            
                            hub_ui.route_send.send(ToHubMsg {
                                to: HubMsgTo::Builder(builder.to_string()),
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
                    for (sync, points) in &self.settings.sync {
                        if path.starts_with(sync) {
                            for point in points {
                                let mut sync_path = path.to_string();
                                sync_path.replace_range(0..sync.len(), point);
                                send_file_write_request(hub_ui, uid, &sync_path, &utf8_bytes);
                            }
                        }
                    }
                }
            }
            else { // its not a workspace, its a system (settings) file
                if let Some(tb_id) = self.text_buffer_path_to_id.get(path) {
                    let atb = &self.text_buffers[tb_id.0 as usize];
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
    
    pub fn reload_builders(&mut self) {
        let hub_ui = self.hub_ui.as_mut().unwrap();
        let uid = hub_ui.route_send.alloc_uid();
        hub_ui.route_send.send(ToHubMsg {
            to: HubMsgTo::Hub,
            msg: HubMsg::ListBuildersRequest {uid: uid}
        });
        self.builders_request_uid = uid;
    }
    
    pub fn handle_hub_msg(&mut self, cx: &mut Cx, htc: &FromHubMsg, windows: &mut Vec<AppWindow>, state: &AppState, build_manager: &mut BuildManager) {
        let hub_ui = self.hub_ui.as_mut().unwrap();
        // only in ConnectUI of ourselves do we list the workspaces
        match &htc.msg {
            // our own connectUI message, means we are ready to talk to the hub
            HubMsg::ConnectUI => if hub_ui.route_send.is_own_addr(&htc.from) {
                // now start talking
            },
            HubMsg::DisconnectBuilder(_) | HubMsg::ConnectBuilder(_) => {
                let own = if let Some(send) = &self.builder_route_send {send.is_own_addr(&htc.from)}else {false};
                if !own {
                    self.reload_builders();
                }
            },
            HubMsg::ListBuildersResponse {uid, builders} => if *uid == self.builders_request_uid {
                let uid = hub_ui.route_send.alloc_uid();
                // from these workspaces query filetrees
                for builder in builders {
                    // lets look up a workspace and configure it!
                    // lets config it
                    if let Some(builder_config) = self.settings.builders.get(builder) {
                        hub_ui.route_send.send(ToHubMsg {
                            to: HubMsgTo::Builder(builder.clone()),
                            msg: HubMsg::BuilderConfig {uid: uid, config: builder_config.clone()}
                        });
                    }
                    hub_ui.route_send.send(ToHubMsg {
                        to: HubMsgTo::Builder(builder.clone()),
                        msg: HubMsg::BuilderFileTreeRequest {uid: uid, create_digest: false}
                    });
                    hub_ui.route_send.send(ToHubMsg {
                        to: HubMsgTo::Builder(builder.clone()),
                        msg: HubMsg::ListPackagesRequest {uid: uid}
                    });
                }
                self.builders_request_uid = uid;
                // add all workspace nodes
                for window in windows {
                    window.file_panel.file_tree.root_node = FileNode::Folder {
                        name: "".to_string(),
                        draw: None,
                        state: NodeState::Open,
                        folder: builders.iter().map( | v | FileNode::Folder {
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
                for atb in &mut self.text_buffers {
                    if let Some(cth_msg) = &atb.read_msg {
                        hub_ui.route_send.send(cth_msg.clone())
                    }
                }
            },
            HubMsg::BuilderFileTreeResponse {uid, tree} => if *uid == self.builders_request_uid {
                // replace a workspace node
                if let BuilderFileTreeNode::Folder {name, ..} = &tree {
                    let workspace = name.clone();
                    // insert each filetree at the right childnode
                    for (window_index, window) in windows.iter_mut().enumerate() {
                        let mut paths = Vec::new();
                        if let FileNode::Folder {folder, ..} = &mut window.file_panel.file_tree.root_node {
                            for node in folder.iter_mut() {
                                if let FileNode::Folder {name, ..} = node {
                                    if *name == workspace {
                                        *node = hub_to_tree(&tree, "", &mut paths);
                                        break
                                    }
                                }
                            }
                        }
                        // lets load the file
                        for path in &paths {
                            self.text_buffer_from_path(cx, path);
                        }
                        window.file_panel.file_tree.load_open_folders(cx, &state.windows[window_index].open_folders);
                    }
                }
            },
            HubMsg::FileReadResponse {uid, data, ..} => {
                for (path, tb_id) in &mut self.text_buffer_path_to_id {
                    let atb = &mut self.text_buffers[tb_id.0 as usize];
                    if let Some(cth_msg) = &atb.read_msg {
                        if let HubMsg::FileReadRequest {uid: read_uid, ..} = &cth_msg.msg {
                            if *read_uid == *uid {
                                atb.read_msg = None;
                                if let Some(data) = data {
                                    if let Ok(utf8_data) = std::str::from_utf8(data) {
                                        atb.text_buffer.load_from_utf8(&utf8_data);
                                        atb.text_buffer.send_textbuffer_loaded_signal(cx);
                                        FileEditor::update_token_chunks(&path, atb, &mut build_manager.search_index);
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

pub fn hub_to_tree(node: &BuilderFileTreeNode, base: &str, paths: &mut Vec<String>) -> FileNode {
    match node {
        BuilderFileTreeNode::File {name, ..} => {
            let path = format!("{}/{}", base, name);
            paths.push(path);
            FileNode::File {
                name: name.clone(),
                draw: None
            }
        },
        BuilderFileTreeNode::Folder {name, folder, ..} => {
            let path = format!("{}/{}", base, name);
            FileNode::Folder {
                name: name.clone(),
                folder: folder.iter().map( | v | hub_to_tree(v, if base == "" {name}else {&path}, paths)).collect(),
                draw: None,
                state: NodeState::Closed
            }
        }
    }
}
