
use {
    crate::{
        makepad_micro_serde::*,
        makepad_platform::*,
        makepad_widgets::*,
        makepad_platform::makepad_live_compiler::LiveFileChange,
        makepad_platform::thread::*,
        makepad_platform::os::cx_stdin::{
            HostToStdin,
            StdinToHost,
        },
        build_manager::{
            run_view::*,
            build_protocol::*,
            build_client::BuildClient
        },
        makepad_shell::*,
    },
    makepad_http::server::*,
    std::{
        collections::{HashMap},
        env,
    },
    std::sync::mpsc,
    std::thread,
    std::time,
    std::net::{UdpSocket,SocketAddr},
    std::time::{Instant, Duration},
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    
    BuildManager = {{BuildManager}} {
        recompile_timeout: 0.2
    }
}


pub struct ActiveBuild {
    pub item_id: LiveId,
    pub process: BuildProcess,
    pub run_view_id: LiveId,
    pub texture: Texture,
    pub cmd_id: Option<BuildCmdId>,
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct ActiveBuildId(pub LiveId);

#[derive(Default)]
pub struct ActiveBuilds {
    pub builds: HashMap<ActiveBuildId, ActiveBuild>
}

impl ActiveBuilds {
    pub fn item_id_active(&self, item_id:LiveId)->bool{
        for (_k, v) in &self.builds {
            if v.item_id == item_id{
                return true
            }
        }
        false
    }
    
    pub fn any_binary_active(&self, binary:&str)->bool{
        for (_k, v) in &self.builds {
            if v.process.binary == binary{
                return true
            }
        }
        false
    }
    
    pub fn build_id_from_cmd_id(&self, cmd_id: BuildCmdId) -> Option<ActiveBuildId> {
        for (k, v) in &self.builds {
            if v.cmd_id == Some(cmd_id) {
                return Some(*k);
            }
        }
        None
    }
    
    pub fn build_id_from_run_view_id(&self, run_view_id: LiveId) -> Option<ActiveBuildId> {
        for (k, v) in &self.builds {
            if v.run_view_id == run_view_id {
                return Some(*k);
            }
        }
        None
    }
    
    
    pub fn run_view_id_from_cmd_id(&self, cmd_id: BuildCmdId) -> Option<LiveId> {
        for v in self.builds.values() {
            if v.cmd_id == Some(cmd_id) {
                return Some(v.run_view_id);
            }
        }
        None
    }
    
    pub fn cmd_id_from_run_view_id(&self, run_view_id: LiveId) -> Option<BuildCmdId> {
        for v in self.builds.values() {
            if v.run_view_id == run_view_id {
                return v.cmd_id
            }
        }
        None
    }
    
}

#[derive(Live, LiveHook)]
pub struct BuildManager {
    #[live] path: String,
    #[rust] pub clients: Vec<BuildClient>,
    #[rust] pub log: Vec<(ActiveBuildId, LogItem)>,
    #[live] recompile_timeout: f64,
    #[rust] recompile_timer: Timer,
    #[rust] pub binaries: Vec<BuildBinary>,
    #[rust] pub active: ActiveBuilds,
    #[rust] pub studio_http: String,
    #[rust] pub recv_external_ip: ToUIReceiver<SocketAddr>,
    #[rust] pub send_file_change: FromUISender<LiveFileChange>
}

pub struct BuildBinary {
    pub open: f64,
    pub name: String
}


pub enum BuildManagerAction {
    RedrawDoc, // {doc_id: DocumentId},
    StdinToHost {run_view_id: LiveId, msg: StdinToHost},
    RedrawLog,
    ClearLog,
    None
}

impl BuildManager {
    
    pub fn init(&mut self, cx: &mut Cx) {
        // not great but it will do.
        
        self.clients = vec![BuildClient::new_with_local_server(&self.path)];
        self.update_run_list(cx);
        self.recompile_timer = cx.start_timeout(self.recompile_timeout);
        self.discover_external_ip(cx);
    }
    
    
    pub fn get_process_texture(&self, run_view_id: LiveId) -> Option<Texture> {
        for v in self.active.builds.values() {
            if v.run_view_id == run_view_id {
                return Some(v.texture.clone())
            }
        }
        None
    }
    
    pub fn send_host_to_stdin(&self, run_view_id: LiveId, msg: HostToStdin) {
        if let Some(cmd_id) = self.active.cmd_id_from_run_view_id(run_view_id) {
            self.clients[0].send_cmd_with_id(cmd_id, BuildCmd::HostToStdin(msg.to_json()));
        }
    }
    
    pub fn update_run_list(&mut self, _cx: &mut Cx) {
        let cwd = std::env::current_dir().unwrap();
        self.binaries.clear();
        match shell_env_cap(&[], &cwd, "cargo", &["run", "--bin"]) {
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
    
    pub fn handle_tab_close(&mut self, tab_id:LiveId)->bool{
        let len = self.active.builds.len();
        self.active.builds.retain(|_,v|{
            if v.run_view_id == tab_id{
                if let Some(cmd_id) = v.cmd_id {
                    self.clients[0].send_cmd_with_id(cmd_id, BuildCmd::Stop);
                }
                return false
            }
            true
        });
        if len != self.active.builds.len(){
            self.log.clear();
            true
        }
        else{
            false
        }
    }
    
    pub fn start_recompile(&mut self, _cx: &mut Cx) {
        // alright so. a file was changed. now what.
        for active_build in self.active.builds.values_mut() {
            if let Some(cmd_id) = active_build.cmd_id {
                self.clients[0].send_cmd_with_id(cmd_id, BuildCmd::Stop);
            }
            let cmd_id = self.clients[0].send_cmd(BuildCmd::Run(active_build.process.clone(), self.studio_http.clone()));
            active_build.cmd_id = Some(cmd_id);
        }
    }
    
    pub fn clear_active_builds(&mut self) {
        // alright so. a file was changed. now what.
        for active_build in self.active.builds.values_mut() {
            if let Some(cmd_id) = active_build.cmd_id {
                self.clients[0].send_cmd_with_id(cmd_id, BuildCmd::Stop);
            }
        }
        self.active.builds.clear();
    }
    
    pub fn clear_log(&mut self) {
        self.log.clear();
    }
    
    pub fn start_recompile_timer(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        cx.stop_timer(self.recompile_timer);
        self.recompile_timer = cx.start_timeout(self.recompile_timeout);
        for active_build in self.active.builds.values_mut() {
            let view = ui.run_view(&[active_build.run_view_id]);
            view.recompile_started(cx);
        }
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<BuildManagerAction> {
        let mut actions = Vec::new();
        self.handle_event_with(cx, event, &mut | _, action | actions.push(action));
        actions
    }
    
    pub fn live_reload_needed(&mut self, live_file_change:LiveFileChange){
        // lets send this filechange to all our stdin stuff
        for active_build in self.active.builds.values_mut() {
            if let Some(cmd_id) = active_build.cmd_id{
                self.clients[0].send_cmd_with_id(cmd_id, BuildCmd::HostToStdin(HostToStdin::ReloadFile{
                    file: live_file_change.file_name.clone(),
                    contents: live_file_change.content.clone()
                }.to_json()));
            }
        }
        let _ = self.send_file_change.send(live_file_change);
    }
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_event: &mut dyn FnMut(&mut Cx, BuildManagerAction)) {
        if let Event::Signal = event{
            if let Ok(addr) = self.recv_external_ip.try_recv(){
                // alright lets start our http server
                self.start_http_server(addr);
            }
        }
        
        if self.recompile_timer.is_event(event) {
            self.start_recompile(cx);
            self.clear_log();
            /*state.editor_state.messages.clear();
            for doc in &mut state.editor_state.documents.values_mut() {
                if let Some(inner) = &mut doc.inner {
                    inner.msg_cache.clear();
                }
            }*/
            dispatch_event(cx, BuildManagerAction::RedrawLog)
        }
        let log = &mut self.log;
        let active = &mut self.active;
        //let editor_state = &mut state.editor_state;
        self.clients[0].handle_event_with(cx, event, &mut | cx, wrap | {
            //let msg_id = editor_state.messages.len();
            // ok we have a cmd_id in wrap.msg
            match wrap.item {
                LogItem::Location(loc) => {
                    if let Some(id) = active.build_id_from_cmd_id(wrap.cmd_id) {
                        log.push((id, LogItem::Location(loc)));
                        dispatch_event(cx, BuildManagerAction::RedrawLog)
                    }
                    /*if let Some(doc_id) = editor_state.documents_by_path.get(UnixPath::new(&loc.file_name)) {
                        let doc = &mut editor_state.documents[*doc_id];
                        if let Some(inner) = &mut doc.inner {
                            inner.msg_cache.add_range(&inner.text, msg_id, loc.range);
                        }
                        dispatch_event(cx, BuildManagerAction::RedrawDoc {
                            doc_id: *doc_id
                        })
                    }*/
                    //editor_state.messages.push(BuildMsg::Location(loc));
                }
                LogItem::Bare(bare) => {
                    if let Some(id) = active.build_id_from_cmd_id(wrap.cmd_id) {
                        log.push((id, LogItem::Bare(bare)));
                        dispatch_event(cx, BuildManagerAction::RedrawLog)
                    }
                    //editor_state.messages.push(wrap.msg);
                }
                LogItem::StdinToHost(line) => {
                    let msg: Result<StdinToHost, DeJsonErr> = DeJson::deserialize_json(&line);
                    match msg {
                        Ok(msg) => {
                            dispatch_event(cx, BuildManagerAction::StdinToHost {
                                run_view_id: active.run_view_id_from_cmd_id(wrap.cmd_id).unwrap_or(LiveId(0)),
                                msg
                            });
                        }
                        Err(_) => { // we should output a log string
                            if let Some(id) = active.build_id_from_cmd_id(wrap.cmd_id) {
                                log.push((id, LogItem::Bare(LogItemBare {
                                    level: LogItemLevel::Log,
                                    line: line.trim().to_string()
                                })));
                                dispatch_event(cx, BuildManagerAction::RedrawLog)
                            }
                            /*editor_state.messages.push(BuildMsg::Bare(BuildMsgBare {
                                level: BuildMsgLevel::Log,
                                line
                            }));*/
                        }
                    }
                }
            }
        });
    }
    
    pub fn start_http_server(&mut self, mut addr: SocketAddr){
        addr.set_port(41535);
        let (tx_request, rx_request) = mpsc::channel::<HttpServerRequest> ();
        
        self.studio_http = format!("{}",addr);
        log!("Build manager mobile file change http server at {}", self.studio_http);
        start_http_server(HttpServer{
            listen_address:addr,
            post_max_size: 1024*1024,
            request: tx_request
        });
        let receiver = self.send_file_change.receiver();
        std::thread::spawn(move || {
            // lets receive incoming http requests
            // if will answer it with a filechange if we have one
            // otherwise we just return nothing
            let mut change_slot = None;
            let mut addrs = Vec::new();
            while let Ok(message) = rx_request.recv() {
                // only store last change, fix later
                while let Ok(change) = receiver.try_recv(){
                    change_slot = Some(change);
                    addrs.clear();
                }
                
                match message{
                    HttpServerRequest::ConnectWebSocket {web_socket_id:_, response_sender:_, headers:_}=>{
                    },
                    HttpServerRequest::DisconnectWebSocket {web_socket_id:_}=>{
                    },
                    HttpServerRequest::BinaryMessage {web_socket_id:_, response_sender:_, data:_}=>{
                    }
                    HttpServerRequest::Get{headers, response_sender}=>{
                        let mut addr = headers.addr.clone();
                        addr.set_port(0);
                        let body = if addrs.contains(&addr){
                            "".to_string().as_bytes().to_vec()
                        }
                        else{
                            addrs.push(addr);
                            if let Some(change) = &change_slot{
                                format!("{}$$$makepad_live_change$$${}",change.file_name, change.content).as_bytes().to_vec()
                            }
                            else{
                                "".to_string().as_bytes().to_vec()
                            }
                        };
                        let header = format!(
                            "HTTP/1.1 200 OK\r\n\
                            Content-Type: application/json\r\n\
                            Content-encoding: none\r\n\
                            Cache-Control: max-age:0\r\n\
                            Content-Length: {}\r\n\
                            Connection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = response_sender.send(HttpServerResponse{header, body});
                        // protect a little bit against flooding
                        thread::sleep(time::Duration::from_millis(50));
                    }
                    HttpServerRequest::Post{..}=>{//headers, body, response}=>{
                    }
                }
            }
        });
    }
    
    pub fn discover_external_ip(&mut self, _cx:&mut Cx){
        // figure out some kind of unique id. bad but whatever.
        let studio_uid = LiveId::from_str(&format!("{:?}{:?}", Instant::now(), std::time::SystemTime::now()));
        
        let write_discovery = UdpSocket::bind("0.0.0.0:41534");
        if write_discovery.is_err(){
            return
        }
        let write_discovery = write_discovery.unwrap();
        write_discovery.set_read_timeout(Some(Duration::new(0, 1))).unwrap();
        write_discovery.set_broadcast(true).unwrap();
        // start a broadcast
        std::thread::spawn(move || {
            let dummy = studio_uid.0.to_be_bytes();
            loop {
                write_discovery.send_to(&dummy, "255.255.255.255:41533").unwrap();
                thread::sleep(time::Duration::from_millis(100));
            }
        });
        // listen for bounced back udp packets to get our external ip
        let ip_sender = self.recv_external_ip.sender();
        std::thread::spawn(move || {
            let discovery = UdpSocket::bind("0.0.0.0:41533").unwrap();
            discovery.set_read_timeout(Some(Duration::new(0, 1))).unwrap();
            discovery.set_broadcast(true).unwrap();
            
            let mut other_uid = [0u8; 8];
            'outer: loop{
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
