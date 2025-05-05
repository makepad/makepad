use {
    crate::{
        app::AppAction,
        build_manager::{build_client::BuildClient, build_protocol::*},
        file_system::file_system::FileSystem,
        makepad_file_server::FileSystemRoots,
        makepad_micro_serde::*,
        makepad_platform::makepad_live_compiler::LiveFileChange,
        makepad_platform::os::cx_stdin::{
            HostToStdin, StdinKeyModifiers, StdinMouseDown, StdinMouseMove, StdinMouseUp,
            StdinScroll, StdinToHost,
        },
        makepad_platform::studio::{
            DesignerComponentPosition,
            DesignerZoomPan,
            AppToStudio, AppToStudioVec, EventSample, GPUSample, StudioToApp, StudioToAppVec,
        },
        makepad_shell::*,
        makepad_widgets::*,
    },
    makepad_code_editor::{
        decoration::{Decoration, DecorationType},
        text,
    },
    makepad_http::server::*,
    std::{
        cell::RefCell,
        collections::{hash_map, HashMap},
        fs::File,
        io::prelude::*,
        net::{SocketAddr, UdpSocket},
        sync::mpsc,
        sync::{Arc, Mutex},
        thread, time,
        time::{Duration, Instant},
    },
};

pub const MAX_SWAPCHAIN_HISTORY: usize = 4;
pub struct ActiveBuild {
    pub root: String,
    pub log_index: String,
    pub process: BuildProcess,
    pub swapchain: HashMap<usize, Option<cx_stdin::Swapchain<Texture>>>,
    pub last_swapchain_with_completed_draws: HashMap<usize, Option<cx_stdin::Swapchain<Texture>>>,
    pub app_area: HashMap<usize, Area>,
    /// Some previous value of `swapchain`, which holds the image still being
    /// the most recent to have been presented after a successful client draw,
    /// and needs to be kept around to avoid deallocating the backing texture.
    ///
    /// While not strictly necessary, it can also accept *new* draws to any of
    /// its images, which allows the client to catch up a frame or two, visually.
    pub aux_chan_host_endpoint: Option<cx_stdin::aux_chan::HostEndpoint>,
}
impl ActiveBuild {
    pub fn swapchain_mut(&mut self, index: usize) -> &mut Option<cx_stdin::Swapchain<Texture>> {
        match self.swapchain.entry(index) {
            hash_map::Entry::Occupied(o) => o.into_mut(),
            hash_map::Entry::Vacant(v) => v.insert(None),
        }
    }
    pub fn last_swapchain_with_completed_draws_mut(
        &mut self,
        index: usize,
    ) -> &mut Option<cx_stdin::Swapchain<Texture>> {
        match self.last_swapchain_with_completed_draws.entry(index) {
            hash_map::Entry::Occupied(o) => o.into_mut(),
            hash_map::Entry::Vacant(v) => v.insert(None),
        }
    }
    pub fn swapchain(&self, index: usize) -> Option<&cx_stdin::Swapchain<Texture>> {
        if let Some(e) = self.swapchain.get(&index) {
            if let Some(e) = e {
                return Some(e);
            }
        }
        None
    }
    pub fn last_swapchain_with_completed_draws(
        &mut self,
        index: usize,
    ) -> Option<&cx_stdin::Swapchain<Texture>> {
        if let Some(e) = self.last_swapchain_with_completed_draws.get(&index) {
            if let Some(e) = e {
                return Some(e);
            }
        }
        None
    }
}

#[derive(Default)]
pub struct ActiveBuilds {
    pub builds: HashMap<LiveId, ActiveBuild>,
}

impl ActiveBuilds {
    pub fn item_id_active(&self, item_id: LiveId) -> bool {
        self.builds.get(&item_id).is_some()
    }

    pub fn any_binary_active(&self, binary: &str) -> bool {
        for (_k, v) in &self.builds {
            if v.process.binary == binary {
                return true;
            }
        }
        false
    }
}

#[derive(Default)]
pub struct ProfileSampleStore {
    pub event: Vec<EventSample>,
    pub gpu: Vec<GPUSample>,
}

#[derive(Default)]
pub struct BuildManager {
    roots: FileSystemRoots,
    http_port: usize,
    pub clients: Vec<BuildClient>,
    pub log: Vec<(LiveId, LogItem)>,
    pub profile: HashMap<LiveId, ProfileSampleStore>,
    recompile_timeout: f64,
    recompile_timer: Timer,
    pub binaries: Vec<BuildBinary>,
    pub active: ActiveBuilds,
    pub studio_http: String,
    pub recv_studio_msg: ToUIReceiver<(LiveId, AppToStudioVec)>,
    pub recv_external_ip: ToUIReceiver<SocketAddr>,
    pub tick_timer: Timer,
    pub designer_state: DesignerState,
    //pub send_file_change: FromUISender<LiveFileChange>,
    pub active_build_websockets: Arc<Mutex<RefCell<Vec<ActiveBuildSocket>>>>,
}

pub struct ActiveBuildSocket{
    web_socket_id: u64, 
    build_id: LiveId, 
    sender: mpsc::Sender<Vec<u8>>
}

#[derive(Default, SerRon, DeRon)]
pub struct DesignerState{
    state: HashMap<LiveId, DesignerStatePerBuildId>
}

#[derive(Default, SerRon, DeRon)]
pub struct DesignerStatePerBuildId{
    selected_file: String,
    zoom_pan: DesignerZoomPan,
    component_positions: Vec<DesignerComponentPosition>
}

impl DesignerState{
    fn save_state(&self){
        let saved = self.serialize_ron();
        let mut f = File::create("makepad_designer.ron").expect("Unable to create file");
        f.write_all(saved.as_bytes()).expect("Unable to write data");
    }
        
    fn load_state(&mut self){
        if let Ok(contents) = std::fs::read_to_string("makepad_designer.ron") {
            match DesignerState::deserialize_ron(&contents) {
                Ok(state)=>{
                    *self = state
                }
                Err(e)=>{
                    println!("ERR {:?}",e);
                }
            }
        }
    }
    
    fn get_build_storage<F:FnOnce(&mut DesignerStatePerBuildId)>(&mut self, build_id: LiveId, f:F){
        match self.state.entry(build_id) {
            hash_map::Entry::Occupied(mut v) => {
                f(v.get_mut());
            },
            hash_map::Entry::Vacant(v) => {
                let mut db = DesignerStatePerBuildId::default();
                f(&mut db);
                v.insert(db);
            }
        }
    }
}

pub struct BuildBinary {
    pub open: f64,
    pub root: String,
    pub name: String,
}

#[derive(Clone, Debug, DefaultNone)]
pub enum BuildManagerAction {
    StdinToHost { build_id: LiveId, msg: StdinToHost },
    None,
}

// Cross-platform
// Able to dynamically adapt to the current network environment
// whether it is a wired connection, Wi-Fi or VPN.
// But it requires the ability to access external networks.
fn get_local_ip() -> String {
    /*let ipv6 = UdpSocket::bind("[::]:0")
        .and_then(|socket| {
            socket.connect("[2001:4860:4860::8888]:80")?;
            socket.local_addr()
        })
        .ok();
*/
    let ipv4 = UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            socket.connect("8.8.8.8:80")?;
            socket.local_addr()
        })
        .ok();

    match ipv4 {
        Some(SocketAddr::V4(addr)) if !addr.ip().is_loopback() => addr.ip().to_string(),
        _ => "127.0.0.1".to_string(),
    }
}

impl BuildManager {
    pub fn init(&mut self, cx: &mut Cx, roots: FileSystemRoots) {
        self.http_port = if std::option_env!("MAKEPAD_STUDIO_HTTP").is_some() {
            8002
        } else {
            8001
        };

        let local_ip = get_local_ip();
        //self.studio_http = format!("http://172.20.10.4:{}/$studio_web_socket", self.http_port);
        // self.studio_http = format!("http://127.0.0.1:{}/$studio_web_socket", self.http_port);
        self.studio_http = format!("http://{}:{}/$studio_web_socket", local_ip, self.http_port);
        
        println!("Studio http : {:?}", self.studio_http);
        self.tick_timer = cx.start_interval(0.008);
        self.roots = roots;
        self.clients = vec![BuildClient::new_with_local_server(self.roots.clone())];
        self.designer_state.load_state();
        self.update_run_list(cx);
        //self.recompile_timer = cx.start_timeout(self.recompile_timeout);
    }

    pub fn send_host_to_stdin(&self, item_id: LiveId, msg: HostToStdin) {
        self.clients[0].send_cmd_with_id(item_id, BuildCmd::HostToStdin(msg.to_json()));
    }

    pub fn update_run_list(&mut self, _cx: &mut Cx) {
        self.binaries.clear();
        for (root_name, root_path) in &self.roots.roots{
            match shell_env_cap(&[], root_path, "cargo", &["run", "--bin"]) {
                Ok(_) => {}
                // we expect it on stderr
                Err(e) => {
                    let mut after_av = false;
                    for line in e.split("\n") {
                        if after_av {
                            let binary = line.trim().to_string();
                            if binary.len() > 0 {
                                self.binaries.push(BuildBinary {
                                    open: 0.0,
                                    root: root_name.clone(),
                                    name: binary,
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
    }

    pub fn process_name(&mut self, tab_id: LiveId) -> Option<String> {
        if let Some(build) = self.active.builds.get(&tab_id) {
            return Some(build.process.binary.clone());
        }
        None
    }

    pub fn handle_tab_close(&mut self, tab_id: LiveId) -> bool {
        let len = self.active.builds.len();
        if self.active.builds.remove(&tab_id).is_some() {
            self.clients[0].send_cmd_with_id(tab_id, BuildCmd::Stop);
        }
        if len != self.active.builds.len() {
            self.log.clear();
            true
        } else {
            false
        }
    }

    pub fn start_recompile(&mut self, _cx: &mut Cx) {
        // alright so. a file was changed. now what.
        for (build_id, active_build) in &mut self.active.builds {
            self.clients[0].send_cmd_with_id(*build_id, BuildCmd::Stop);
            self.clients[0].send_cmd_with_id(
                *build_id,
                BuildCmd::Run(active_build.process.clone(), self.studio_http.clone()),
            );

            active_build.swapchain.clear();
            active_build.last_swapchain_with_completed_draws.clear();
            active_build.aux_chan_host_endpoint = None;
        }
    }

    pub fn clear_active_builds(&mut self) {
        // alright so. a file was changed. now what.
        for build_id in self.active.builds.keys() {
            self.clients[0].send_cmd_with_id(*build_id, BuildCmd::Stop);
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

    pub fn start_recompile_timer(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.recompile_timer);
        self.recompile_timer = cx.start_timeout(self.recompile_timeout);
        /*for item_id in self.active.builds.keys() {
            let view = ui.run_view(&[*item_id]);
            view.recompile_started(cx);
        }*/
    }

    pub fn live_reload_needed(&mut self, live_file_change: LiveFileChange) {
        // lets send this filechange to all our stdin stuff
        /*for item_id in self.active.builds.keys() {
            self.clients[0].send_cmd_with_id(*item_id, BuildCmd::HostToStdin(HostToStdin::ReloadFile {
                file: live_file_change.file_name.clone(),
                contents: live_file_change.content.clone()
            }.to_json()));
        }*/
        // alright what do we need to do here.
        
        // so first off we need to find the root this thing belongs to
        // if its 'makepad' we might need to send over 2 file names
        // one local to the repo and one full path
        
        if let Ok(d) = self.active_build_websockets.lock() {
            // ok so. if we have a makepad repo file
            // we send over the full path and the stripped path
            // if not makepad, we have to only send it to the right project
            
            for socket in d.borrow_mut().iter_mut() {
                // alright so we have a file_name which includes a 'root'
                // we also have this build_id which contains a root.
                // if they are the same, we strip it
                // if they are not, we send over the full path
                let file_name = if let Some(build) = self.active.builds.get(&socket.build_id){
                    let mut parts = live_file_change.file_name.splitn(2,"/");
                    let root = parts.next().unwrap();
                    let file = parts.next().unwrap();
                    // file local to the connection
                    if root == build.root{ 
                        file.to_string()
                    }
                    // nonlocal file, make full path
                    else if let Ok(root) = self.roots.find_root(root){
                        root.join(file).into_os_string().into_string().unwrap()
                    }
                    else{
                        file.to_string()
                    }
                }
                else{
                    live_file_change.file_name.clone()
                };
                let data = StudioToAppVec(vec![StudioToApp::LiveChange {
                    file_name,
                    content: live_file_change.content.clone(),
                }])
                .serialize_bin();
                let _ = socket.sender.send(data.clone());
            }
        }
    }

    pub fn broadcast_to_stdin(&mut self, msg: HostToStdin) {
        for build_id in self.active.builds.keys() {
            self.clients[0].send_cmd_with_id(*build_id, BuildCmd::HostToStdin(msg.to_json()));
        }
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, file_system: &mut FileSystem) {
        if let Some(_) = self.tick_timer.is_event(event) {
            self.broadcast_to_stdin(HostToStdin::Tick);
        }

        match event {
            Event::MouseDown(e) => {
                // we should only send this if it was captured by one of our runviews
                for (build_id, build) in &self.active.builds {
                    for area in build.app_area.values() {
                        if e.handled.get() == *area {
                            self.clients[0].send_cmd_with_id(
                                *build_id,
                                BuildCmd::HostToStdin(
                                    HostToStdin::MouseDown(StdinMouseDown {
                                        time: e.time,
                                        x: e.abs.x,
                                        y: e.abs.y,
                                        button_raw_bits: e.button.bits(),
                                        modifiers: StdinKeyModifiers::from_key_modifiers(
                                            &e.modifiers,
                                        ),
                                    })
                                    .to_json(),
                                ),
                            );
                            break;
                        }
                    }
                }
            }
            Event::MouseMove(e) => {
                // we send this one to what window exactly?
                self.broadcast_to_stdin(HostToStdin::MouseMove(StdinMouseMove {
                    time: e.time,
                    x: e.abs.x,
                    y: e.abs.y,
                    modifiers: StdinKeyModifiers::from_key_modifiers(&e.modifiers),
                }));
            }
            Event::MouseUp(e) => {
                self.broadcast_to_stdin(HostToStdin::MouseUp(StdinMouseUp {
                    time: e.time,
                    button_raw_bits: e.button.bits(),
                    x: e.abs.x,
                    y: e.abs.y,
                    modifiers: StdinKeyModifiers::from_key_modifiers(&e.modifiers),
                }));
            }
            Event::Scroll(e) => {
                self.broadcast_to_stdin(HostToStdin::Scroll(StdinScroll {
                    is_mouse: e.is_mouse,
                    time: e.time,
                    x: e.abs.x,
                    y: e.abs.y,
                    sx: e.scroll.x,
                    sy: e.scroll.y,
                    modifiers: StdinKeyModifiers::from_key_modifiers(&e.modifiers),
                }));
            }
            _ => (),
        }

        if let Event::Signal = event {
            let log = &mut self.log;
            let active = &mut self.active;

            if let Ok(mut addr) = self.recv_external_ip.try_recv() {
                addr.set_port(self.http_port as u16);
                self.studio_http = format!("http://{}/$studio_web_socket", addr);
            }

            while let Ok((build_id, msgs)) = self.recv_studio_msg.try_recv() {
                for msg in msgs.0 {
                    match msg {
                        AppToStudio::LogItem(item) => {
                            let file_name = if let Some(build) = active.builds.get(&build_id){
                                self.roots.map_path(&build.root, &item.file_name)
                            }
                            else{
                                self.roots.map_path("", &item.file_name)
                            };
                            
                            let start = text::Position {
                                line_index: item.line_start as usize,
                                byte_index: item.column_start as usize,
                            };
                            let end = text::Position {
                                line_index: item.line_end as usize,
                                byte_index: item.column_end as usize,
                            };
                            //log!("{:?} {:?}", pos, pos + loc.length);
                            if let Some(file_id) = file_system.path_to_file_node_id(&file_name)
                            {
                                match item.level {
                                    LogLevel::Warning => {
                                        file_system.add_decoration(
                                            file_id,
                                            Decoration::new(0, start, end, DecorationType::Warning),
                                        );
                                        cx.action(AppAction::RedrawFile(file_id))
                                    }
                                    LogLevel::Error => {
                                        file_system.add_decoration(
                                            file_id,
                                            Decoration::new(0, start, end, DecorationType::Error),
                                        );
                                        cx.action(AppAction::RedrawFile(file_id))
                                    }
                                    _ => (),
                                }
                            }
                            log.push((
                                build_id,
                                LogItem::Location(LogItemLocation {
                                    level: item.level,
                                    file_name,
                                    start,
                                    end,
                                    message: item.message,
                                    explanation: item.explanation
                                }),
                            ));
                            cx.action(AppAction::RedrawLog)
                        }
                        AppToStudio::EventSample(sample) => {
                            // ok lets push this profile sample into the profiles
                            let values = self.profile.entry(build_id).or_default();
                            values.event.push(sample);
                            cx.action(AppAction::RedrawProfiler)
                        }
                        AppToStudio::GPUSample(sample) => {
                            // ok lets push this profile sample into the profiles
                            let values = self.profile.entry(build_id).or_default();
                            values.gpu.push(sample);
                            cx.action(AppAction::RedrawProfiler)
                        }
                        AppToStudio::FocusDesign => cx.action(AppAction::FocusDesign(build_id)),
                        AppToStudio::PatchFile(ef) => cx.action(AppAction::PatchFile(ef)),
                        AppToStudio::EditFile(ef) => cx.action(AppAction::EditFile(ef)),
                        AppToStudio::JumpToFile(jt) => {
                            cx.action(AppAction::JumpTo(jt));
                        }
                        AppToStudio::SelectInFile(jt) => {
                            cx.action(AppAction::SelectInFile(jt));
                        }
                        AppToStudio::SwapSelection(ss) => {
                            // alright now what do we do
                            cx.action(AppAction::SwapSelection(ss));
                        }
                        AppToStudio::DesignerComponentMoved(mv)=>{
                            self.designer_state.get_build_storage(build_id, |bs|{
                                if let Some(v) =  bs.component_positions.iter_mut().find(|v| v.id == mv.id){
                                    *v = mv;
                                }
                                else{
                                    bs.component_positions.push(mv);
                                }
                            });
                            self.designer_state.save_state();
                        }
                        AppToStudio::DesignerZoomPan(zp)=>{
                            self.designer_state.get_build_storage(build_id, |bs|{
                                bs.zoom_pan = zp;
                            });
                            self.designer_state.save_state();
                        }
                        AppToStudio::DesignerStarted=>{
                            // send the app the select file init message
                            if let Ok(d) = self.active_build_websockets.lock() {
                                if let Some(bs) = self.designer_state.state.get(&build_id){
                                    let data = StudioToAppVec(vec![
                                        StudioToApp::DesignerLoadState{
                                            zoom_pan: bs.zoom_pan.clone(),
                                            positions: bs.component_positions.clone()
                                        },
                                        StudioToApp::DesignerSelectFile {
                                            file_name: bs.selected_file.clone()
                                        },
                                    ]).serialize_bin();
                                    
                                    for socket in d.borrow_mut().iter_mut() {
                                        if socket.build_id == build_id{
                                            let _ = socket.sender.send(data.clone());
                                        }
                                    }
                                }
                            }
                            
                        }
                        AppToStudio::DesignerFileSelected{file_name}=>{
                            // alright now what. lets 
                            self.designer_state.get_build_storage(build_id, |bs|{
                                bs.selected_file = file_name;
                            });
                            self.designer_state.save_state();
                        }
                    }
                }
            }

            while let Ok(wrap) = self.clients[0].msg_receiver.try_recv() {
                match wrap.message {
                    BuildClientMessage::LogItem(LogItem::Location(mut loc)) => {
                        loc.file_name = if let Some(build) = active.builds.get(&wrap.cmd_id){
                            self.roots.map_path(&build.root, &loc.file_name)
                        }
                        else{
                            self.roots.map_path("", &loc.file_name)
                        };
                        if let Some(file_id) = file_system.path_to_file_node_id(&loc.file_name) {
                            match loc.level {
                                LogLevel::Warning => {
                                    file_system.add_decoration(
                                        file_id,
                                        Decoration::new(
                                            0,
                                            loc.start,
                                            loc.end,
                                            DecorationType::Warning,
                                        ),
                                    );
                                    cx.action(AppAction::RedrawFile(file_id))
                                }
                                LogLevel::Error => {
                                    file_system.add_decoration(
                                        file_id,
                                        Decoration::new(
                                            0,
                                            loc.start,
                                            loc.end,
                                            DecorationType::Error,
                                        ),
                                    );
                                    cx.action(AppAction::RedrawFile(file_id))
                                }
                                _ => (),
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
                            Ok(msg) => cx.action(BuildManagerAction::StdinToHost {
                                build_id: wrap.cmd_id,
                                msg,
                            }),
                            Err(_) => {
                                // we should output a log string
                                log.push((
                                    wrap.cmd_id,
                                    LogItem::Bare(LogItemBare {
                                        level: LogLevel::Log,
                                        line: line.trim().to_string(),
                                    }),
                                ));
                                cx.action(AppAction::RedrawLog)
                                /*editor_state.messages.push(BuildMsg::Bare(BuildMsgBare {
                                    level: BuildMsgLevel::Log,
                                    line
                                }));*/
                            }
                        }
                    }
                    BuildClientMessage::AuxChanHostEndpointCreated(aux_chan_host_endpoint) => {
                        if let Some(active_build) = active.builds.get_mut(&wrap.cmd_id) {
                            active_build.aux_chan_host_endpoint = Some(aux_chan_host_endpoint);
                        }
                    }
                }
            }
        }

        if self.recompile_timer.is_event(event).is_some() {
            self.start_recompile(cx);
            cx.action(AppAction::RecompileStarted);
            cx.action(AppAction::ClearLog);
        }
    }

    pub fn start_http_server(&mut self) {
        let addr = SocketAddr::new("0.0.0.0".parse().unwrap(), self.http_port as u16);
        let (tx_request, rx_request) = mpsc::channel::<HttpServerRequest>();
        //log!("Http server at http://127.0.0.1:{}/ for wasm examples and mobile", self.http_port);
        start_http_server(HttpServer {
            listen_address: addr,
            post_max_size: 1024 * 1024,
            request: tx_request,
        });
        /*
        let rx_file_change = self.send_file_change.receiver();
        //let (tx_live_file, rx_live_file) = mpsc::channel::<HttpServerRequest> ();

        let active_build_websockets = self.active_build_websockets.clone();
        // livecoding observer
        std::thread::spawn(move || {
            loop{
                if let Ok(_change) = rx_file_change.recv() {
                    // lets send this change to all our websocket connections
                }
            }
        });*/

        let studio_sender = self.recv_studio_msg.sender();
        let active_build_websockets = self.active_build_websockets.clone();
        std::thread::spawn(move || {
            // TODO fix this proper:
            let makepad_path = "./".to_string();
            let abs_makepad_path = std::env::current_dir()
                .unwrap()
                .join(makepad_path.clone())
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            let mut root = "./".to_string();
            for arg in std::env::args() {
                if let Some(prefix) = arg.strip_prefix("--root=") {
                    root = prefix.to_string();
                    break;
                }
            }
            let remaps = [
                (
                    format!("/makepad/{}/", abs_makepad_path),
                    makepad_path.clone(),
                ),
                (
                    format!("/makepad/{}/", std::env::current_dir().unwrap().display()),
                    "".to_string(),
                ),
                (
                    "/makepad//".to_string(),
                    format!("{}/{}", root, makepad_path.clone()),
                ),
                (
                    "/makepad/".to_string(),
                    format!("{}/{}", root, makepad_path.clone()),
                ),
                ("/".to_string(), "".to_string()),
            ];
            let mut socket_id_to_build_id = HashMap::new();
            while let Ok(message) = rx_request.recv() {
                // only store last change, fix later
                match message {
                    HttpServerRequest::ConnectWebSocket {
                        web_socket_id,
                        response_sender,
                        headers,
                    } => {
                        if let Some(id) = headers.path.rsplit("/").next() {
                            if let Ok(id) = id.parse::<u64>() {
                                socket_id_to_build_id.insert(web_socket_id, LiveId(id));
                                active_build_websockets
                                    .lock()
                                    .unwrap()
                                    .borrow_mut()
                                    .push(ActiveBuildSocket{
                                        web_socket_id, 
                                        build_id: LiveId(id), 
                                        sender: response_sender
                                    });
                            }
                        }
                    }
                    HttpServerRequest::DisconnectWebSocket { web_socket_id } => {
                        socket_id_to_build_id.remove(&web_socket_id);
                        active_build_websockets
                            .lock()
                            .unwrap()
                            .borrow_mut()
                            .retain(|v| v.web_socket_id != web_socket_id);
                    }
                    HttpServerRequest::BinaryMessage {
                        web_socket_id,
                        response_sender: _,
                        data,
                    } => {
                        if let Some(id) = socket_id_to_build_id.get(&web_socket_id) {
                            if let Ok(msg) = AppToStudioVec::deserialize_bin(&data) {
                                let _ = studio_sender.send((*id, msg));
                            }
                        }
                        // new incombing message from client
                    }
                    HttpServerRequest::Get {
                        headers,
                        response_sender,
                    } => {
                        let path = &headers.path;
                        if path == "/$watch" {
                            let header = "HTTP/1.1 200 OK\r\n\
                                Cache-Control: max-age:0\r\n\
                                Connection: close\r\n\r\n"
                                .to_string();
                            let _ = response_sender.send(HttpServerResponse {
                                header,
                                body: vec![],
                            });
                            continue;
                        }
                        if path == "/favicon.ico" {
                            let header = "HTTP/1.1 200 OK\r\n\r\n".to_string();
                            let _ = response_sender.send(HttpServerResponse {
                                header,
                                body: vec![],
                            });
                            continue;
                        }

                        let mime_type = if path.ends_with(".html") {
                            "text/html"
                        } else if path.ends_with(".wasm") {
                            "application/wasm"
                        } else if path.ends_with(".css") {
                            "text/css"
                        } else if path.ends_with(".js") {
                            "text/javascript"
                        } else if path.ends_with(".ttf") {
                            "application/ttf"
                        } else if path.ends_with(".png") {
                            "image/png"
                        } else if path.ends_with(".jpg") {
                            "image/jpg"
                        } else if path.ends_with(".svg") {
                            "image/svg+xml"
                        } else {
                            continue;
                        };

                        if path.contains("..") || path.contains('\\') {
                            continue;
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
                                    let _ =
                                        response_sender.send(HttpServerResponse { header, body });
                                }
                            }
                        }
                    }
                    HttpServerRequest::Post { .. } => { //headers, body, response}=>{
                    }
                }
            }
        });
    }

    pub fn discover_external_ip(&mut self, _cx: &mut Cx) {
        // figure out some kind of unique id. bad but whatever.
        let studio_uid = LiveId::from_str(&format!(
            "{:?}{:?}",
            Instant::now(),
            std::time::SystemTime::now()
        ));
        let http_port = self.http_port as u16;
        let write_discovery = UdpSocket::bind(SocketAddr::new(
            "0.0.0.0".parse().unwrap(),
            http_port * 2 as u16 + 1,
        ));
        if write_discovery.is_err() {
            return;
        }
        let write_discovery = write_discovery.unwrap();
        write_discovery
            .set_read_timeout(Some(Duration::new(0, 1)))
            .unwrap();
        write_discovery.set_broadcast(true).unwrap();
        // start a broadcast
        std::thread::spawn(move || {
            let dummy = studio_uid.0.to_be_bytes();
            loop {
                let _ = write_discovery.send_to(
                    &dummy,
                    SocketAddr::new("0.0.0.0".parse().unwrap(), http_port * 2 as u16),
                );
                thread::sleep(time::Duration::from_millis(100));
            }
        });
        // listen for bounced back udp packets to get our external ip
        let ip_sender = self.recv_external_ip.sender();
        std::thread::spawn(move || {
            let discovery = UdpSocket::bind(SocketAddr::new(
                "0.0.0.0".parse().unwrap(),
                http_port * 2 as u16,
            ))
            .unwrap();
            discovery
                .set_read_timeout(Some(Duration::new(0, 1)))
                .unwrap();
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
    
            
    pub fn binary_name_to_id(&self, name:&str)->Option<usize>{
        self.binaries.iter().position(|v| v.name == name)
    }
        
    pub fn run_app(&mut self, cx:&mut Cx, binary_name:&str){
        let binary_id = self.binary_name_to_id(binary_name).unwrap();
        self.start_active_build(cx, binary_id, BuildTarget::Release);
    }
            
    pub fn start_active_build(&mut self, _cx:&mut Cx, binary_id:usize, target: BuildTarget) {
        let binary = &self. binaries[binary_id];
        let process = BuildProcess {
            root: binary.root.clone(),
            binary: binary.name.clone(),
            target
        };
        let item_id = process.as_id();
        self.clients[0].send_cmd_with_id(item_id, BuildCmd::Run(process.clone(),self.studio_http.clone()));
        //let run_view_id = LiveId::unique();
        if self.active.builds.get(&item_id).is_none() {
            let index = self.active.builds.len();
            self.active.builds.insert(item_id, ActiveBuild {
                root: binary.root.clone(),
                log_index: format!("[{}]", index),
                process: process.clone(),
                app_area: Default::default(),
                swapchain: Default::default(),
                last_swapchain_with_completed_draws: Default::default(),
                aux_chan_host_endpoint: None,
            });
        }
        //if process.target.runs_in_studio(){
            // create the runview tab
        //    cx.action(AppA::Create(item_id, process.binary.clone()))
        //}
    }
    
    pub fn stop_all_active_builds(&mut self, cx:&mut Cx){
        while self.active.builds.len()>0{
            let build = &self.active.builds.values().next().unwrap();
            let binary_id = self.binary_name_to_id(&build.process.binary).unwrap();
            let target = build.process.target;
            self.stop_active_build(cx, binary_id, target);
        }
    }
        
    pub fn stop_active_build(&mut self, cx:&mut Cx, binary_id: usize, target: BuildTarget) {
        let binary = &self. binaries[binary_id];
                
        let process = BuildProcess {
            root: binary.root.clone(),
            binary: binary.name.clone(),
            target
        };
        let build_id = process.as_id().into();
        if let Some(_) = self.active.builds.remove(&build_id) {
            self.clients[0].send_cmd_with_id(build_id, BuildCmd::Stop);
            if process.target.runs_in_studio(){
                cx.action(AppAction::DestroyRunViews{run_view_id:build_id})
            }
        }
    }
    
}
