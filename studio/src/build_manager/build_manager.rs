
use {
    crate::{
        file_system::file_system::FileSystem,
        makepad_micro_serde::*,
        makepad_widgets::*,
        makepad_platform::makepad_live_compiler::LiveFileChange,
        makepad_platform::os::cx_stdin::{
            HostToStdin,
            StdinToHost,
        },
        makepad_platform::studio::{AppToStudioVec,AppToStudio,EventSample, GPUSample},
        build_manager::{
            build_protocol::*,
            build_client::BuildClient
        },
        run_view::*,
        app::AppAction,
        makepad_shell::*,
    },
    makepad_code_editor::{text, decoration::{Decoration, DecorationType}},
    makepad_http::server::*,
    std::{
        collections::HashMap,
        io::prelude::*,
        path::PathBuf,
        path::Path,
        fs::File,
    },
    std::sync::mpsc,
    std::thread,
    std::time,
    std::net::{UdpSocket, SocketAddr},
    std::time::{Instant, Duration},
};

pub const MAX_SWAPCHAIN_HISTORY: usize = 4;
pub struct ActiveBuild {
    pub log_index: String,
    pub process: BuildProcess,
    pub swapchain: Option<cx_stdin::Swapchain<Texture >>,
    /// Some previous value of `swapchain`, which holds the image still being
    /// the most recent to have been presented after a successful client draw,
    /// and needs to be kept around to avoid deallocating the backing texture.
    ///
    /// While not strictly necessary, it can also accept *new* draws to any of
    /// its images, which allows the client to catch up a frame or two, visually.
    pub last_swapchain_with_completed_draws: Option<cx_stdin::Swapchain<Texture >>,
    
    pub aux_chan_host_endpoint: Option<cx_stdin::aux_chan::HostEndpoint>,
}

#[derive(Default)]
pub struct ActiveBuilds {
    pub builds: HashMap<LiveId, ActiveBuild>
}

impl ActiveBuilds {
    pub fn item_id_active(&self, item_id: LiveId) -> bool {
        self.builds.get(&item_id).is_some()
    }
    
    pub fn any_binary_active(&self, binary: &str) -> bool {
        for (_k, v) in &self.builds {
            if v.process.binary == binary {
                return true
            }
        }
        false
    }
}

#[derive(Default)]
pub struct ProfileSampleStore{
    pub event: Vec<EventSample>,
    pub gpu: Vec<GPUSample>,
}

#[derive(Default)]
pub struct BuildManager {
    root_path: PathBuf,
    http_port: usize,
    pub clients: Vec<BuildClient>,
    pub log: Vec<(LiveId, LogItem)>,
    pub profile: HashMap<LiveId, ProfileSampleStore>,
    recompile_timeout: f64,
    recompile_timer: Timer,
    pub binaries: Vec<BuildBinary>,
    pub active: ActiveBuilds,
    pub studio_http: String,
    pub recv_studio_msg: ToUIReceiver<(LiveId,AppToStudioVec)>,
    pub recv_external_ip: ToUIReceiver<SocketAddr>,
    pub send_file_change: FromUISender<LiveFileChange>
}

pub struct BuildBinary {
    pub open: f64,
    pub name: String
}

#[derive(Clone, Debug, DefaultNone)]
pub enum BuildManagerAction {
    StdinToHost {run_view_id: LiveId, msg: StdinToHost},
    None
}

impl BuildManager {
    
    pub fn init(&mut self, cx: &mut Cx, path:&Path) {
         self.http_port = if std::option_env!("MAKEPAD_STUDIO_HTTP").is_some(){
            8002
        }
        else{
             8001
        };
        
        self.root_path = path.to_path_buf();
        self.clients = vec![BuildClient::new_with_local_server(&self.root_path)];
        
        self.update_run_list(cx);
        //self.recompile_timer = cx.start_timeout(self.recompile_timeout);
    }
    
    pub fn send_host_to_stdin(&self, item_id: LiveId, msg: HostToStdin) {
        self.clients[0].send_cmd_with_id(item_id, BuildCmd::HostToStdin(msg.to_json()));
    }
    
    pub fn update_run_list(&mut self, _cx: &mut Cx) {
        self.binaries.clear();
        match shell_env_cap(&[], &self.root_path, "cargo", &["run", "--bin"]) {
            Ok(_) => {}
            // we expect it on stderr
            Err(e) => {
                let mut after_av = false;
                for line in e.split("\n") {
                    if after_av {
                        let binary = line.trim().to_string();
                        if binary.len()>0 {
                            self.binaries.push(BuildBinary {
                                open: 0.0,
                                name: binary
                            });
                        }
                    }
                    if line.contains("Available binaries:") {
                        after_av = true;
                    }
                }
            }
        }
    }
    
    pub fn handle_tab_close(&mut self, tab_id: LiveId) -> bool {
        let len = self.active.builds.len();
        if self.active.builds.remove(&tab_id).is_some(){
            self.clients[0].send_cmd_with_id(tab_id, BuildCmd::Stop);
        }
        if len != self.active.builds.len() {
            self.log.clear();
            true
        }
        else {
            false
        }
    }
    
    pub fn start_recompile(&mut self, _cx: &mut Cx) {
        // alright so. a file was changed. now what.
        for (item_id, active_build) in &mut self.active.builds {
            self.clients[0].send_cmd_with_id(*item_id, BuildCmd::Stop);
            self.clients[0].send_cmd_with_id(*item_id, BuildCmd::Run(active_build.process.clone(), self.studio_http.clone()));
            active_build.swapchain = None;
            //active_build.last_swapchain_with_completed_draws = None;
            active_build.aux_chan_host_endpoint = None;
        }
    }
    
    pub fn clear_active_builds(&mut self) {
        // alright so. a file was changed. now what.
        for item_id in self.active.builds.keys() {
            self.clients[0].send_cmd_with_id(*item_id, BuildCmd::Stop);
        }
        self.active.builds.clear();
    }
    
    pub fn clear_log(&mut self, cx: &mut Cx, dock: &DockRef, file_system: &mut FileSystem) {
        // lets clear all log related decorations
        file_system.clear_all_decorations();
        file_system.redraw_all_views(cx, dock);
        self.log.clear();
        self.profile.clear();
    }
    
    pub fn start_recompile_timer(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        cx.stop_timer(self.recompile_timer);
        self.recompile_timer = cx.start_timeout(self.recompile_timeout);
        for item_id in self.active.builds.keys() {
            let view = ui.run_view(&[*item_id]);
            view.recompile_started(cx);
        }
    }
    
    pub fn live_reload_needed(&mut self, live_file_change: LiveFileChange) {
        // lets send this filechange to all our stdin stuff
       for item_id in self.active.builds.keys() {
            self.clients[0].send_cmd_with_id(*item_id, BuildCmd::HostToStdin(HostToStdin::ReloadFile {
                file: live_file_change.file_name.clone(),
                contents: live_file_change.content.clone()
            }.to_json()));
        }
        let _ = self.send_file_change.send(live_file_change);
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, file_system: &mut FileSystem) {

        if let Event::Signal = event {
            let log = &mut self.log;
            let active = &mut self.active;       
            
            if let Ok(mut addr) = self.recv_external_ip.try_recv() {
                addr.set_port(self.http_port as u16);
                self.studio_http = format!("http://{}/$studio_web_socket", addr);
            }
            
            while let Ok((build_id, msgs)) = self.recv_studio_msg.try_recv() {
                for msg in msgs.0{
                    match msg{
                        AppToStudio::LogItem(item)=>{
                            let start = text::Position {
                                line_index: item.line_start as usize,
                                byte_index: item.column_start as usize
                            };
                            let end = text::Position {
                                line_index: item.line_end as usize,
                                byte_index: item.column_end as usize
                            };
                            //log!("{:?} {:?}", pos, pos + loc.length);
                            if let Some(file_id) = file_system.path_to_file_node_id(&item.file_name) {
                                match item.level{
                                    LogLevel::Warning=>{
                                        file_system.add_decoration(file_id, Decoration::new(
                                            0,
                                            start,
                                            end,
                                            DecorationType::Warning
                                        ));
                                        cx.action(AppAction::RedrawFile(file_id))
                                    }
                                    LogLevel::Error=>{
                                        file_system.add_decoration(file_id, Decoration::new(
                                            0,
                                            start,
                                            end,
                                            DecorationType::Error
                                        ));
                                        cx.action(AppAction::RedrawFile(file_id))
                                    }
                                    _=>()
                                }
                            }
                            log.push((build_id, LogItem::Location(LogItemLocation{
                                level: item.level,
                                file_name: item.file_name,
                                start,
                                end,
                                message: item.message
                            })));
                            cx.action(AppAction::RedrawLog)
                        }
                        AppToStudio::EventSample(sample)=>{  
                            // ok lets push this profile sample into the profiles
                            let values = self.profile.entry(build_id).or_default();
                            values.event.push(sample);
                            cx.action(AppAction::RedrawProfiler)
                        }
                        AppToStudio::GPUSample(sample)=>{  
                            // ok lets push this profile sample into the profiles
                            let values = self.profile.entry(build_id).or_default();
                            values.gpu.push(sample);
                            cx.action(AppAction::RedrawProfiler)
                        }
                    }
                }
            }
            
            while let Ok(wrap) = self.clients[0].msg_receiver.try_recv(){
                match wrap.message {
                    BuildClientMessage::LogItem(LogItem::Location(loc)) => {
                        if let Some(file_id) = file_system.path_to_file_node_id(&loc.file_name) {
                            match loc.level{
                                LogLevel::Warning=>{
                                    file_system.add_decoration(file_id, Decoration::new(
                                        0,
                                        loc.start,
                                        loc.end,
                                        DecorationType::Warning
                                    ));
                                    cx.action(AppAction::RedrawFile(file_id))
                                }
                                LogLevel::Error=>{
                                    file_system.add_decoration(file_id, Decoration::new(
                                        0,
                                        loc.start,
                                        loc.end,
                                        DecorationType::Error
                                    ));
                                    cx.action(AppAction::RedrawFile(file_id))
                                }
                                _=>()
                            }
                        }
                        log.push((wrap.cmd_id, LogItem::Location(loc)));
                        cx.action(AppAction::RedrawLog)
                    }
                    BuildClientMessage::LogItem(LogItem::Bare(bare)) => {
                        //log!("{:?}", bare);
                        log.push((wrap.cmd_id, LogItem::Bare(bare)));
                        cx.action(AppAction::RedrawLog)
                        //editor_state.messages.push(wrap.msg);
                    }
                    BuildClientMessage::LogItem(LogItem::StdinToHost(line)) => {
                        let msg: Result<StdinToHost, DeJsonErr> = DeJson::deserialize_json(&line);
                        match msg {
                            Ok(msg) => {
                                cx.action(BuildManagerAction::StdinToHost {
                                    run_view_id: wrap.cmd_id,
                                    msg
                                })
                            }
                            Err(_) => { // we should output a log string
                                log.push((wrap.cmd_id, LogItem::Bare(LogItemBare {
                                    level: LogLevel::Log,
                                    line: line.trim().to_string()
                                })));
                                cx.action(AppAction::RedrawLog)
                                /*editor_state.messages.push(BuildMsg::Bare(BuildMsgBare {
                                    level: BuildMsgLevel::Log,
                                    line
                                }));*/
                            }
                        }
                    }
                    BuildClientMessage::AuxChanHostEndpointCreated(aux_chan_host_endpoint) => {
                        if let Some(active_build) = active.builds.get_mut(&wrap.cmd_id){
                            active_build.aux_chan_host_endpoint = Some(aux_chan_host_endpoint);                        
                        }
                    }
                }
            };
        }
        
        if self.recompile_timer.is_event(event).is_some() {
            self.start_recompile(cx);
            cx.action(AppAction::RecompileStarted);
            cx.action(AppAction::ClearLog);
        }
    }
    
    pub fn start_http_server(&mut self) {
        let addr = SocketAddr::new("0.0.0.0".parse().unwrap(), self.http_port as u16);
        let (tx_request, rx_request) = mpsc::channel::<HttpServerRequest> ();
        
        //log!("Http server at http://127.0.0.1:{}/ for wasm examples and mobile", self.http_port);
        start_http_server(HttpServer {
            listen_address: addr,
            post_max_size: 1024 * 1024,
            request: tx_request
        });
        
        let rx_file_change = self.send_file_change.receiver();
        //let (tx_live_file, rx_live_file) = mpsc::channel::<HttpServerRequest> ();
        
        // livecoding observer
        std::thread::spawn(move || {
            loop{
                if let Ok(_change) = rx_file_change.recv() {
                    // lets send this change to all our websocket connections
                }
            }
        });
        let studio_sender = self.recv_studio_msg.sender();
        std::thread::spawn(move || {
            // TODO fix this proper:
            let makepad_path = "./".to_string();
            let abs_makepad_path = std::env::current_dir().unwrap().join(makepad_path.clone()).canonicalize().unwrap().to_str().unwrap().to_string();
            let mut root = "./".to_string();
            for arg in std::env::args(){
                if let Some(prefix) = arg.strip_prefix("--root="){
                    root = prefix.to_string();
                    break;
                }
            }
            let remaps = [
                (format!("/makepad/{}/", abs_makepad_path), makepad_path.clone()),
                (format!("/makepad/{}/", std::env::current_dir().unwrap().display()), "".to_string()),
                ("/makepad//".to_string(), format!("{}/{}",root,makepad_path.clone())),
                ("/makepad/".to_string(), format!("{}/{}",root,makepad_path.clone())),
                ("/".to_string(), "".to_string())
            ];
            let mut socket_id_to_build_id = HashMap::new();
            while let Ok(message) = rx_request.recv() {
                // only store last change, fix later
                match message {
                    HttpServerRequest::ConnectWebSocket {web_socket_id, response_sender: _,headers} => {
                        if let Some(id) = headers.path.rsplit("/").next(){
                            if let Ok(id) = id.parse::<u64>(){
                                socket_id_to_build_id.insert(web_socket_id, LiveId(id));
                            }
                        }
                    },
                    HttpServerRequest::DisconnectWebSocket {web_socket_id} => {
                        socket_id_to_build_id.remove(&web_socket_id);
                    },
                    HttpServerRequest::BinaryMessage {web_socket_id, response_sender: _, data} => {
                        if let Some(id) = socket_id_to_build_id.get(&web_socket_id){
                            if let Ok(msg) = AppToStudioVec::deserialize_bin(&data){
                                let _ = studio_sender.send((*id,msg));
                            }
                        }
                        //println!("GOT BINARY MESSAGE");
                        // new incombing message from client
                    }
                    HttpServerRequest::Get {headers, response_sender} => {
                        let path = &headers.path;
                        // ok so this live connection.. where do we do it
                        // i mean its just a network event msg. we can ignore that
                        // we could just handle this in 'window'
                        // or where shall we handle it
                        // lets give live edit an api so you can codegen/live edit shaders?
                        
                        // alright wasm http server
                        if path == "/$watch" {
                            let header = "HTTP/1.1 200 OK\r\n\
                                Cache-Control: max-age:0\r\n\
                                Connection: close\r\n\r\n".to_string();
                            let _ = response_sender.send(HttpServerResponse {header, body: vec![]});
                            continue
                        }
                        if path == "/favicon.ico" {
                            let header = "HTTP/1.1 200 OK\r\n\r\n".to_string();
                            let _ = response_sender.send(HttpServerResponse {header, body: vec![]});
                            continue
                        }
                        
                        let mime_type = if path.ends_with(".html") {"text/html"}
                        else if path.ends_with(".wasm") {"application/wasm"}
                        else if path.ends_with(".css") {"text/css"}
                        else if path.ends_with(".js") {"text/javascript"}
                        else if path.ends_with(".ttf") {"application/ttf"}
                        else if path.ends_with(".png") {"image/png"}
                        else if path.ends_with(".jpg") {"image/jpg"}
                        else if path.ends_with(".svg") {"image/svg+xml"}
                        else {continue};
                        
                        if path.contains("..") || path.contains('\\') {
                            continue
                        }
                        
                        let mut strip = None;
                        for remap in &remaps {
                            if let Some(s) = path.strip_prefix(&remap.0) {
                                strip = Some(format!("{}{}", remap.1, s));
                                break;
                            }
                        }
                        if let Some(base) = strip {
                            if let Ok(mut file_handle) = File::open(base) {
                                let mut body = Vec::<u8>::new();
                                if file_handle.read_to_end(&mut body).is_ok() {
                                    let header = format!(
                                        "HTTP/1.1 200 OK\r\n\
                                            Content-Type: {}\r\n\
                                            Cross-Origin-Embedder-Policy: require-corp\r\n\
                                            Cross-Origin-Opener-Policy: same-origin\r\n\
                                            Content-encoding: none\r\n\
                                            Cache-Control: max-age:0\r\n\
                                            Content-Length: {}\r\n\
                                            Connection: close\r\n\r\n",
                                        mime_type,
                                        body.len()
                                    );
                                    let _ = response_sender.send(HttpServerResponse {header, body});
                                }
                            }
                        }
                        
                    }
                    HttpServerRequest::Post {..} => { //headers, body, response}=>{
                    }
                }
            }
        });
    }
    
    pub fn discover_external_ip(&mut self, _cx: &mut Cx) {
        // figure out some kind of unique id. bad but whatever.
        let studio_uid = LiveId::from_str(&format!("{:?}{:?}", Instant::now(), std::time::SystemTime::now()));
        let http_port = self.http_port as u16;
        let write_discovery = UdpSocket::bind(SocketAddr::new("0.0.0.0".parse().unwrap(), http_port*2 as u16 + 1));
        if write_discovery.is_err() {
            return
        }
        let write_discovery = write_discovery.unwrap();
        write_discovery.set_read_timeout(Some(Duration::new(0, 1))).unwrap();
        write_discovery.set_broadcast(true).unwrap();
        // start a broadcast
        std::thread::spawn(move || {
            let dummy = studio_uid.0.to_be_bytes();
            loop {
                let _ = write_discovery.send_to(&dummy, SocketAddr::new("0.0.0.0".parse().unwrap(), http_port*2 as u16));
                thread::sleep(time::Duration::from_millis(100));
            }
        });
        // listen for bounced back udp packets to get our external ip
        let ip_sender = self.recv_external_ip.sender();
        std::thread::spawn(move || {
            let discovery = UdpSocket::bind(SocketAddr::new("0.0.0.0".parse().unwrap(), http_port*2 as u16)).unwrap();
            discovery.set_read_timeout(Some(Duration::new(0, 1))).unwrap();
            discovery.set_broadcast(true).unwrap();
            
            let mut other_uid = [0u8; 8];
            'outer: loop {
                while let Ok((_, addr)) = discovery.recv_from(&mut other_uid) {
                    let recv_uid = u64::from_be_bytes(other_uid);
                    if studio_uid.0 == recv_uid {
                        let _ = ip_sender.send(addr);
                        break 'outer;
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
    }
}