use {
    crate::{
        app::AppAction,
        build_manager::{build_client::BuildClient, build_protocol::*},
        file_system::file_system::{FileSystem, LiveFileChange},
        makepad_code_editor::{
            decoration::{Decoration, DecorationType},
            text,
        },
        makepad_file_server::FileSystemRoots,
        makepad_micro_serde::*,
        // Using stub LiveFileChange from file_system module
        makepad_platform::studio::{
            AppToStudio, AppToStudioVec, EventSample, GCSample, GPUSample, LocalProfileSample,
            RemoteKeyModifiers, RemoteMouseDown, RemoteMouseMove, RemoteMouseUp, RemoteScroll,
            RemoteTweakRay,
            ScreenshotRequest, ScreenshotResponse, StudioToApp, StudioToAppVec,
            WidgetTreeDumpRequest, WidgetTreeDumpResponse,
        },
        makepad_shell::*,
        makepad_widgets::*,
    },
    makepad_http::server::*,
    std::{
        cell::RefCell,
        collections::{hash_map, BTreeSet, HashMap, HashSet},
        fs::File,
        io::prelude::*,
        net::{SocketAddr, UdpSocket},
        path::PathBuf,
        sync::mpsc,
        sync::{Arc, Mutex},
        thread, time,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    },
};

pub const MAX_SWAPCHAIN_HISTORY: usize = 4;
pub struct ActiveBuild {
    pub root: String,
    pub log_index: String,
    pub process: BuildProcess,
    pub window_tabs: HashMap<usize, LiveId>,
    pub swapchain: HashMap<usize, Option<shared_framebuf::HostSwapchain>>,
    pub last_swapchain_with_completed_draws: HashMap<usize, Option<shared_framebuf::HostSwapchain>>,
    pub app_area: HashMap<usize, Area>,
    /// Some previous value of `swapchain`, which holds the image still being
    /// the most recent to have been presented after a successful client draw,
    /// and needs to be kept around to avoid deallocating the backing texture.
    ///
    /// While not strictly necessary, it can also accept *new* draws to any of
    /// its images, which allows the client to catch up a frame or two, visually.
    pub aux_chan_host_endpoint: Option<shared_framebuf::aux_chan::HostEndpoint>,
}
impl ActiveBuild {
    pub fn swapchain_mut(&mut self, index: usize) -> &mut Option<shared_framebuf::HostSwapchain> {
        match self.swapchain.entry(index) {
            hash_map::Entry::Occupied(o) => o.into_mut(),
            hash_map::Entry::Vacant(v) => v.insert(None),
        }
    }
    pub fn last_swapchain_with_completed_draws_mut(
        &mut self,
        index: usize,
    ) -> &mut Option<shared_framebuf::HostSwapchain> {
        match self.last_swapchain_with_completed_draws.entry(index) {
            hash_map::Entry::Occupied(o) => o.into_mut(),
            hash_map::Entry::Vacant(v) => v.insert(None),
        }
    }
    pub fn swapchain(&self, index: usize) -> Option<&shared_framebuf::HostSwapchain> {
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
    ) -> Option<&shared_framebuf::HostSwapchain> {
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
    pub fn builds_with_root(&self, root: String) -> impl Iterator<Item = (&LiveId, &ActiveBuild)> {
        self.builds.iter().filter(move |(_, b)| b.root == root)
    }

    pub fn item_id_active(&self, item_id: LiveId) -> bool {
        self.builds.get(&item_id).is_some()
    }

    pub fn any_binary_active(&self, root: &str, binary: &str) -> bool {
        for (_k, v) in &self.builds {
            if v.process.root == root && v.process.binary == binary {
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
    pub gc: Vec<GCSample>,
}

#[derive(Default)]
pub struct BuildManager {
    roots: FileSystemRoots,
    http_port: usize,
    pub clients: Vec<BuildClient>,
    running_processes: HashMap<LiveId, BuildProcess>,
    pending_aux_chan_host_endpoints: HashMap<LiveId, shared_framebuf::aux_chan::HostEndpoint>,
    pub log: Vec<(LiveId, LogItem)>,
    pub profile: HashMap<LiveId, ProfileSampleStore>,
    pub self_profile: ProfileSampleStore,
    profile_time_origin: HashMap<LiveId, f64>,
    self_profile_time_origin: Option<f64>,
    profile_focus_build: Option<LiveId>,
    profiler_running: bool,
    recompile_timeout: f64,
    recompile_timer: Timer,
    pub binaries: Vec<BuildBinary>,
    pub active: ActiveBuilds,
    pub studio_http: String,
    pub recv_studio_msg: ToUIReceiver<(LiveId, AppToStudioVec)>,
    pub recv_studio_disconnect: ToUIReceiver<LiveId>,
    recv_terminal_msg: ToUIReceiver<TerminalToBuildManager>,
    pub recv_external_ip: ToUIReceiver<SocketAddr>,
    pub tick_timer: Timer,
    pub websocket_alive_timer: Timer,
    //pub send_file_change: FromUISender<LiveFileChange>,
    pub active_build_websockets: Arc<Mutex<RefCell<ActiveBuildWebSockets>>>,
    terminal_sockets: HashMap<u64, mpsc::Sender<Vec<u8>>>,
    terminal_build_owners: HashMap<LiveId, u64>,
    terminal_build_counter: u64,
    terminal_screenshot_requests: UniqueIdMap<PendingTerminalScreenshot>,
    terminal_widget_tree_dump_requests: UniqueIdMap<PendingTerminalWidgetTreeDump>,
    terminal_latest_widget_dumps: HashMap<LiveId, String>,
    terminal_startup_queries: HashMap<LiveId, String>,
    terminal_startup_dump_pending: HashSet<LiveId>,
    recompiling_builds: HashSet<LiveId>,
}

#[derive(Default)]
pub struct ActiveBuildWebSockets {
    pub sockets: Vec<ActiveBuildSocket>,
}

impl ActiveBuildWebSockets {
    pub fn send_studio_to_app(&mut self, build_id: LiveId, msg: StudioToApp) -> bool {
        let data = StudioToAppVec(vec![msg]).serialize_bin();
        let mut sent_any = false;
        self.sockets.retain(|socket| {
            if socket.build_id != build_id {
                return true;
            }
            if socket.sender.send(data.clone()).is_ok() {
                sent_any = true;
                true
            } else {
                false
            }
        });
        sent_any
    }
}

pub struct ActiveBuildSocket {
    web_socket_id: u64,
    build_id: LiveId,
    sender: mpsc::Sender<Vec<u8>>,
}

include!("terminal_proto.rs");
include!("studio_proto.rs");

const PROFILE_MAX_SAMPLES_PER_BUILD: usize = 16_384;
pub struct BuildBinary {
    pub open: f64,
    pub root: String,
    pub name: String,
}

#[derive(Clone, Debug, Default)]
pub enum BuildManagerAction {
    AppToStudio {
        build_id: LiveId,
        msg: AppToStudio,
    },
    AiClickViz {
        build_id: LiveId,
        x: f64,
        y: f64,
        phase: AiClickVizPhase,
    },
    #[default]
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AiClickVizPhase {
    #[default]
    Down,
    Up,
}

#[derive(Clone, Debug, Default, DeJson)]
struct CargoMetadata {
    packages: Vec<CargoMetadataPackage>,
}

#[derive(Clone, Debug, Default, DeJson)]
struct CargoMetadataPackage {
    name: String,
    targets: Vec<CargoMetadataTarget>,
}

#[derive(Clone, Debug, Default, DeJson)]
struct CargoMetadataTarget {
    kind: Vec<String>,
}

fn discover_workspace_binary_packages(root_path: &PathBuf) -> Result<Vec<String>, String> {
    let (stdout, stderr, ok) = shell_env_cap_split(
        &[],
        root_path,
        "cargo",
        &["metadata", "--no-deps", "--format-version=1"],
    );
    if !ok {
        return Err(format!(
            "cargo metadata failed in {:?}\n{}\n{}",
            root_path,
            stderr.trim(),
            stdout.trim()
        ));
    }

    let metadata = CargoMetadata::deserialize_json_lenient(&stdout)
        .map_err(|err| format!("failed to parse cargo metadata json: {err:?}"))?;

    let mut binaries = Vec::new();
    let mut seen = HashSet::new();
    for package in metadata.packages {
        let has_bin_target = package
            .targets
            .iter()
            .any(|target| target.kind.iter().any(|kind| kind == "bin"));
        if has_bin_target && seen.insert(package.name.clone()) {
            binaries.push(package.name);
        }
    }
    binaries.sort();
    Ok(binaries)
}

fn log_level_name(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Warning => "warning",
        LogLevel::Error => "error",
        LogLevel::Log => "log",
        LogLevel::Wait => "wait",
        LogLevel::Panic => "panic",
    }
}

impl BuildManager {
    fn studio_addr(&self) -> String {
        self.studio_http
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(self.studio_http.as_str())
            .split_once('/')
            .map(|(host_port, _)| host_port)
            .unwrap_or(self.studio_http.as_str())
            .to_string()
    }

    fn default_root_name(&self) -> Option<String> {
        self.roots.roots.first().map(|(name, _)| name.clone())
    }

    pub fn init(&mut self, cx: &mut Cx, roots: FileSystemRoots) {
        self.http_port = if std::option_env!("STUDIO").is_some() {
            8002
        } else {
            8001
        };

        let local_ip = get_local_ip();
        //self.studio_http = format!("http://172.20.10.4:{}/$studio_web_socket", self.http_port);
        // self.studio_http = format!("http://127.0.0.1:{}/$studio_web_socket", self.http_port);
        self.studio_http = format!(
            "http://{}:{}{}",
            local_ip, self.http_port, STUDIO_WEBSOCKET_PATH
        );

        println!("Studio http : {:?}", self.studio_http);
        self.tick_timer = cx.start_interval(0.008);
        self.roots = roots;
        self.clients = vec![BuildClient::new_with_local_server(self.roots.clone())];
        self.update_run_list(cx);
        self.websocket_alive_timer = cx.start_interval(1.0);
        // Set a small debounce timeout for recompilation (300ms)
        self.recompile_timeout = 0.3;
    }

    pub fn send_host_to_stdin(&self, item_id: LiveId, msg: StudioToApp) {
        let runs_in_studio = self
            .active
            .builds
            .get(&item_id)
            .is_some_and(|build| build.process.target.runs_in_studio());

        if let Ok(sockets) = self.active_build_websockets.lock() {
            if sockets
                .borrow_mut()
                .send_studio_to_app(item_id, msg.clone())
            {
                return;
            }
        }

        if runs_in_studio {
            return;
        }

        self.clients[0].send_cmd_with_id(item_id, BuildCmd::StudioToApp(msg.to_json()));
    }

    pub fn has_active_web_socket(&self, build_id: LiveId) -> bool {
        if let Ok(sockets) = self.active_build_websockets.lock() {
            sockets
                .borrow()
                .sockets
                .iter()
                .any(|socket| socket.build_id == build_id)
        } else {
            false
        }
    }

    pub fn update_run_list(&mut self, _cx: &mut Cx) {
        self.binaries.clear();
        for (root_name, root_path) in &self.roots.roots {
            match discover_workspace_binary_packages(root_path) {
                Ok(packages) => {
                    for package in packages {
                        self.binaries.push(BuildBinary {
                            open: 0.0,
                            root: root_name.clone(),
                            name: package,
                        });
                    }
                }
                Err(err) => {
                    crate::warning!(
                        "run list discovery failed for root {}: {}",
                        root_name,
                        err
                    );
                }
            }
        }
    }

    pub fn any_binary_active(&self, root: &str, binary: &str) -> bool {
        self.running_processes.values().any(|process| {
            process.root == root && process.binary == binary
        })
    }

    pub fn item_id_active(&self, item_id: LiveId) -> bool {
        self.running_processes.contains_key(&item_id)
            || self
                .running_processes
                .values()
                .any(|process| process.as_id() == item_id)
    }

    pub fn process_name(&self, tab_id: LiveId) -> Option<String> {
        if let Some(build) = self.active.builds.get(&tab_id) {
            return Some(build.process.binary.clone());
        }
        if let Some(process) = self.running_processes.get(&tab_id) {
            return Some(process.binary.clone());
        }
        None
    }

    fn trim_profile_samples<T>(samples: &mut Vec<T>) {
        if samples.len() > PROFILE_MAX_SAMPLES_PER_BUILD {
            let remove = samples.len() - PROFILE_MAX_SAMPLES_PER_BUILD;
            samples.drain(0..remove);
        }
    }

    fn rebase_sample_range(start: f64, end: f64, origin: &mut Option<f64>) -> (f64, f64) {
        let base = *origin.get_or_insert(start.min(end));
        let mut rebased_start = start - base;
        let mut rebased_end = end - base;
        if rebased_start < 0.0 && rebased_start > -0.001 {
            rebased_start = 0.0;
        }
        if rebased_end < 0.0 && rebased_end > -0.001 {
            rebased_end = 0.0;
        }
        if rebased_end < rebased_start {
            rebased_end = rebased_start;
        }
        (rebased_start, rebased_end)
    }

    fn push_event_profile_sample(&mut self, build_id: LiveId, mut sample: EventSample) {
        let origin = self.profile_time_origin.entry(build_id).or_insert(sample.start);
        sample.start -= *origin;
        sample.end -= *origin;
        if sample.end < sample.start {
            sample.end = sample.start;
        }
        self.profile_focus_build = Some(build_id);
        let samples = self.profile.entry(build_id).or_default();
        samples.event.push(sample);
        Self::trim_profile_samples(&mut samples.event);
        Cx::set_local_profile_capture_enabled(true);
    }

    fn push_gpu_profile_sample(&mut self, build_id: LiveId, mut sample: GPUSample) {
        let origin = self.profile_time_origin.entry(build_id).or_insert(sample.start);
        sample.start -= *origin;
        sample.end -= *origin;
        if sample.end < sample.start {
            sample.end = sample.start;
        }
        self.profile_focus_build = Some(build_id);
        let samples = self.profile.entry(build_id).or_default();
        samples.gpu.push(sample);
        Self::trim_profile_samples(&mut samples.gpu);
        Cx::set_local_profile_capture_enabled(true);
    }

    fn push_gc_profile_sample(&mut self, build_id: LiveId, mut sample: GCSample) {
        let origin = self.profile_time_origin.entry(build_id).or_insert(sample.start);
        sample.start -= *origin;
        sample.end -= *origin;
        if sample.end < sample.start {
            sample.end = sample.start;
        }
        self.profile_focus_build = Some(build_id);
        let samples = self.profile.entry(build_id).or_default();
        samples.gc.push(sample);
        Self::trim_profile_samples(&mut samples.gc);
        Cx::set_local_profile_capture_enabled(true);
    }

    fn push_self_event_profile_sample(&mut self, mut sample: EventSample) {
        let (start, end) = Self::rebase_sample_range(
            sample.start,
            sample.end,
            &mut self.self_profile_time_origin,
        );
        sample.start = start;
        sample.end = end;
        self.self_profile.event.push(sample);
        Self::trim_profile_samples(&mut self.self_profile.event);
    }

    fn push_self_gpu_profile_sample(&mut self, mut sample: GPUSample) {
        let (start, end) = Self::rebase_sample_range(
            sample.start,
            sample.end,
            &mut self.self_profile_time_origin,
        );
        sample.start = start;
        sample.end = end;
        self.self_profile.gpu.push(sample);
        Self::trim_profile_samples(&mut self.self_profile.gpu);
    }

    fn push_self_gc_profile_sample(&mut self, mut sample: GCSample) {
        let (start, end) = Self::rebase_sample_range(
            sample.start,
            sample.end,
            &mut self.self_profile_time_origin,
        );
        sample.start = start;
        sample.end = end;
        self.self_profile.gc.push(sample);
        Self::trim_profile_samples(&mut self.self_profile.gc);
    }

    fn remove_profile_build(&mut self, build_id: LiveId) {
        self.profile.remove(&build_id);
        self.profile_time_origin.remove(&build_id);
        if self.profile_focus_build == Some(build_id) {
            self.profile_focus_build = self.profile.keys().copied().next();
        }
        if self.profile.is_empty() {
            Cx::set_local_profile_capture_enabled(false);
        }
    }

    pub fn clear_profile_samples(&mut self) {
        self.profile.clear();
        self.self_profile = ProfileSampleStore::default();
        self.profile_time_origin.clear();
        self.self_profile_time_origin = None;
        self.profile_focus_build = None;
        Cx::set_local_profile_capture_enabled(false);
    }

    pub fn current_profile_store(&self) -> Option<(LiveId, &ProfileSampleStore)> {
        if let Some(build_id) = self.profile_focus_build {
            if let Some(samples) = self.profile.get(&build_id) {
                return Some((build_id, samples));
            }
        }
        self.profile
            .iter()
            .next()
            .map(|(build_id, samples)| (*build_id, samples))
    }

    pub fn self_profile_store(&self) -> &ProfileSampleStore {
        &self.self_profile
    }

    pub fn set_profiler_running(&mut self, running: bool) {
        let was_running = self.profiler_running;
        self.profiler_running = running;
        if !running {
            Cx::set_local_profile_capture_enabled(false);
            let _ = Cx::take_local_profile_samples();
        } else if !was_running {
            // Start a fresh profiling session at t=0 when the user presses Run.
            self.profile.clear();
            self.self_profile = ProfileSampleStore::default();
            self.profile_time_origin.clear();
            self.self_profile_time_origin = None;
            self.profile_focus_build = None;
            Cx::set_local_profile_capture_enabled(false);
        }
    }

    fn send_kill_to_build(&self, build_id: LiveId) -> bool {
        if let Ok(sockets) = self.active_build_websockets.lock() {
            return sockets
                .borrow_mut()
                .send_studio_to_app(build_id, StudioToApp::Kill);
        }
        false
    }

    fn build_id_from_tab_id(&self, tab_id: LiveId) -> Option<LiveId> {
        if self.active.builds.contains_key(&tab_id) {
            return Some(tab_id);
        }
        self.active.builds.iter().find_map(|(build_id, build)| {
            build
                .window_tabs
                .values()
                .any(|&id| id == tab_id)
                .then_some(*build_id)
        })
    }

    pub fn register_window_tab(&mut self, build_id: LiveId, window_id: usize, tab_id: LiveId) {
        if let Some(build) = self.active.builds.get_mut(&build_id) {
            build.window_tabs.insert(window_id, tab_id);
        }
    }

    fn ensure_active_build(&mut self, build_id: LiveId) {
        if self.active.builds.contains_key(&build_id) {
            return;
        }

        let process = self
            .running_processes
            .get(&build_id)
            .cloned()
            .unwrap_or_else(|| BuildProcess {
                root: String::new(),
                binary: format!("remote-{}", build_id.0),
                target: BuildTarget::ReleaseStudio,
            });
        let index = self.active.builds.len();
        let aux_chan_host_endpoint = self.pending_aux_chan_host_endpoints.remove(&build_id);
        self.active.builds.insert(
            build_id,
            ActiveBuild {
                root: process.root.clone(),
                log_index: format!("[{}]", index),
                process,
                window_tabs: Default::default(),
                app_area: Default::default(),
                swapchain: Default::default(),
                last_swapchain_with_completed_draws: Default::default(),
                aux_chan_host_endpoint,
            },
        );
    }

    pub fn handle_tab_close(&mut self, tab_id: LiveId) -> Option<LiveId> {
        let build_id = self.build_id_from_tab_id(tab_id)?;
        let process = self
            .active
            .builds
            .get(&build_id)
            .map(|b| b.process.clone())
            .or_else(|| self.running_processes.get(&build_id).cloned());

        self.active.builds.remove(&build_id);
        self.running_processes.remove(&build_id);
        self.remove_profile_build(build_id);
        if let Some(web_socket_id) = self.terminal_build_owners.remove(&build_id) {
            self.send_terminal_response(
                web_socket_id,
                StudioTerminalResponse::Stopped {
                    build_id: build_id.0,
                },
            );
        }
        self.terminal_latest_widget_dumps.remove(&build_id);
        self.terminal_startup_queries.remove(&build_id);
        self.terminal_startup_dump_pending.remove(&build_id);
        self.clear_terminal_screenshots_for_build(build_id);
        self.clear_terminal_widget_tree_dumps_for_build(build_id);

        if !self.send_kill_to_build(build_id) {
            self.clients[0].send_cmd_with_id(build_id, BuildCmd::Stop);
        }

        if process.is_some_and(|process| process.target.runs_in_studio()) {
            return Some(build_id);
        }
        None
    }

    pub fn start_recompile(&mut self, _cx: &mut Cx) {
        let studio_addr = self.studio_addr();
        let known_roots: HashSet<String> = self
            .roots
            .roots
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        let build_ids: Vec<LiveId> = self.active.builds.keys().copied().collect();
        for build_id in build_ids {
            self.terminal_latest_widget_dumps.remove(&build_id);
            self.terminal_startup_queries.remove(&build_id);
            self.terminal_startup_dump_pending.remove(&build_id);
            self.clear_terminal_screenshots_for_build(build_id);
            self.clear_terminal_widget_tree_dumps_for_build(build_id);
        }
        for (build_id, active_build) in &mut self.active.builds {
            if !known_roots.contains(&active_build.process.root) {
                continue;
            }
            self.recompiling_builds.insert(*build_id);
            self.clients[0].send_cmd_with_id(*build_id, BuildCmd::Stop);
            self.clients[0].send_cmd_with_id(
                *build_id,
                BuildCmd::Run(active_build.process.clone(), studio_addr.clone()),
            );

            active_build.swapchain.clear();
            active_build.last_swapchain_with_completed_draws.clear();
            active_build.aux_chan_host_endpoint = None;
        }
    }

    pub fn clear_active_builds(&mut self) {
        // alright so. a file was changed. now what.
        for build_id in self.running_processes.keys() {
            self.clients[0].send_cmd_with_id(*build_id, BuildCmd::Stop);
        }
        self.running_processes.clear();
        self.active.builds.clear();
        self.terminal_build_owners.clear();
        self.terminal_latest_widget_dumps.clear();
        self.terminal_startup_queries.clear();
        self.terminal_startup_dump_pending.clear();
        self.terminal_screenshot_requests.clear();
        self.terminal_widget_tree_dump_requests.clear();
    }

    pub fn clear_log(&mut self, cx: &mut Cx, dock: &DockRef, file_system: &mut FileSystem) {
        // lets clear all log related decorations
        file_system.clear_all_decorations();
        file_system.redraw_all_views(cx, dock);
        self.log.clear();
        self.clear_profile_samples();
    }

    pub fn start_recompile_timer(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.recompile_timer);
        self.recompile_timer = cx.start_timeout(self.recompile_timeout);
    }

    pub fn live_reload_needed(&mut self, live_file_change: LiveFileChange) {
        // lets send this filechange to all our stdin stuff
        /*for item_id in self.active.builds.keys() {
            self.clients[0].send_cmd_with_id(*item_id, BuildCmd::StudioToApp(StudioToApp::ReloadFile {
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

            for socket in d.borrow_mut().sockets.iter_mut() {
                // alright so we have a file_name which includes a 'root'
                // we also have this build_id which contains a root.
                // if they are the same, we strip it
                // if they are not, we send over the full path
                let file_name = if let Some(build) = self.active.builds.get(&socket.build_id) {
                    let mut parts = live_file_change.file_name.splitn(2, "/");
                    let root = parts.next().unwrap();
                    let file = parts.next().unwrap();
                    // file local to the connection
                    if root == build.root {
                        file.to_string()
                    }
                    // nonlocal file, make full path
                    else if let Ok(root) = self.roots.find_root(root) {
                        root.join(file).into_os_string().into_string().unwrap()
                    } else {
                        file.to_string()
                    }
                } else {
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

    pub fn broadcast_to_stdin(&mut self, msg: StudioToApp) {
        let build_ids: Vec<LiveId> = self.active.builds.keys().copied().collect();
        for build_id in build_ids {
            self.send_host_to_stdin(build_id, msg.clone());
        }
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, file_system: &mut FileSystem) {
        if self.profiler_running {
            for sample in Cx::take_local_profile_samples() {
                match sample {
                    LocalProfileSample::Event(sample) => {
                        self.push_self_event_profile_sample(sample);
                    }
                    LocalProfileSample::GPU(sample) => {
                        self.push_self_gpu_profile_sample(sample);
                    }
                    LocalProfileSample::GC(sample) => {
                        self.push_self_gc_profile_sample(sample);
                    }
                }
            }
        } else {
            let _ = Cx::take_local_profile_samples();
        }

        if let Some(_) = self.tick_timer.is_event(event) {
            self.broadcast_to_stdin(StudioToApp::Tick);
        }

        if let Some(_) = self.websocket_alive_timer.is_event(event) {
            if let Ok(d) = self.active_build_websockets.lock() {
                for socket in d.borrow_mut().sockets.iter_mut() {
                    let data = StudioToAppVec(vec![StudioToApp::KeepAlive]).serialize_bin();
                    let _ = socket.sender.send(data.clone());
                }
            }
        }

        match event {
            Event::MouseDown(e) => {
                // we should only send this if it was captured by one of our runviews
                for (build_id, build) in &self.active.builds {
                    for area in build.app_area.values() {
                        if e.handled.get() == *area {
                            if !area.is_valid(cx) {
                                continue;
                            }
                            let rect = area.rect(cx);
                            self.send_host_to_stdin(
                                *build_id,
                                StudioToApp::MouseDown(RemoteMouseDown {
                                    time: e.time,
                                    x: e.abs.x - rect.pos.x,
                                    y: e.abs.y - rect.pos.y,
                                    button_raw_bits: e.button.bits(),
                                    modifiers: RemoteKeyModifiers::from_key_modifiers(&e.modifiers),
                                }),
                            );
                            break;
                        }
                    }
                }
            }
            Event::MouseMove(e) => {
                // RunView can route per-view mouse moves directly. Keep this as a fallback.
                if e.handled.get().is_empty() {
                    for (build_id, build) in &self.active.builds {
                        for area in build.app_area.values() {
                            if !area.is_valid(cx) {
                                continue;
                            }
                            let rect = area.rect(cx);
                            if !rect.contains(e.abs) {
                                continue;
                            }
                            let x = e.abs.x - rect.pos.x;
                            let y = e.abs.y - rect.pos.y;
                            if e.modifiers.logo {
                                self.send_host_to_stdin(
                                    *build_id,
                                    StudioToApp::TweakRay(RemoteTweakRay {
                                        time: e.time,
                                        x,
                                        y,
                                        modifiers: RemoteKeyModifiers::from_key_modifiers(
                                            &e.modifiers,
                                        ),
                                    }),
                                );
                            } else {
                                self.send_host_to_stdin(
                                    *build_id,
                                    StudioToApp::MouseMove(RemoteMouseMove {
                                        time: e.time,
                                        x,
                                        y,
                                        modifiers: RemoteKeyModifiers::from_key_modifiers(
                                            &e.modifiers,
                                        ),
                                    }),
                                );
                            }
                            break;
                        }
                    }
                }
            }
            Event::MouseUp(e) => {
                for (build_id, build) in &self.active.builds {
                    let Some(area) = build.app_area.values().find(|area| area.is_valid(cx)) else {
                        continue;
                    };
                    let rect = area.rect(cx);
                    self.send_host_to_stdin(
                        *build_id,
                        StudioToApp::MouseUp(RemoteMouseUp {
                            time: e.time,
                            button_raw_bits: e.button.bits(),
                            x: e.abs.x - rect.pos.x,
                            y: e.abs.y - rect.pos.y,
                            modifiers: RemoteKeyModifiers::from_key_modifiers(&e.modifiers),
                        }),
                    );
                }
            }
            Event::Scroll(e) => {
                for (build_id, build) in &self.active.builds {
                    let Some(area) = build.app_area.values().find(|area| area.is_valid(cx)) else {
                        continue;
                    };
                    let rect = area.rect(cx);
                    self.send_host_to_stdin(
                        *build_id,
                        StudioToApp::Scroll(RemoteScroll {
                            is_mouse: e.is_mouse,
                            time: e.time,
                            x: e.abs.x - rect.pos.x,
                            y: e.abs.y - rect.pos.y,
                            sx: e.scroll.x,
                            sy: e.scroll.y,
                            modifiers: RemoteKeyModifiers::from_key_modifiers(&e.modifiers),
                        }),
                    );
                }
            }
            _ => (),
        }

        if let Event::Signal = event {
            let mut pending_terminal_logs: Vec<(LiveId, String, String)> = Vec::new();
            self.handle_external_ip_signal();
            self.handle_terminal_signal_messages(cx);
            self.handle_studio_disconnect_signal(cx);
            self.handle_studio_signal_messages(cx, file_system, &mut pending_terminal_logs);

            while let Ok(wrap) = self.clients[0].msg_receiver.try_recv() {
                let log = &mut self.log;
                let active = &mut self.active;
                match wrap.message {
                    BuildClientMessage::LogItem(LogItem::Location(mut loc)) => {
                        let terminal_level = log_level_name(loc.level);
                        let terminal_line = loc.message.clone();
                        loc.file_name = if let Some(build) = active.builds.get(&wrap.cmd_id) {
                            self.roots.map_path(&build.root, &loc.file_name)
                        } else {
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
                        pending_terminal_logs.push((
                            wrap.cmd_id,
                            terminal_level.to_string(),
                            terminal_line,
                        ));
                        cx.action(AppAction::RedrawLog)
                    }
                    BuildClientMessage::LogItem(LogItem::Bare(bare)) => {
                        let terminal_level = log_level_name(bare.level);
                        let terminal_line = bare.line.clone();
                        //log!("{:?}", bare);
                        log.push((wrap.cmd_id, LogItem::Bare(bare)));
                        pending_terminal_logs.push((
                            wrap.cmd_id,
                            terminal_level.to_string(),
                            terminal_line,
                        ));
                        cx.action(AppAction::RedrawLog)
                        //editor_state.messages.push(wrap.msg);
                    }
                    BuildClientMessage::LogItem(LogItem::AppToStudio(line)) => {
                        // stdin-loop control traffic moved to websocket messages.
                        // Any stdout lines in this channel are plain log output.
                        let line = line.trim();
                        if !line.is_empty() {
                            log.push((
                                wrap.cmd_id,
                                LogItem::Bare(LogItemBare {
                                    level: LogLevel::Log,
                                    line: line.to_string(),
                                }),
                            ));
                            pending_terminal_logs.push((
                                wrap.cmd_id,
                                "log".to_string(),
                                line.to_string(),
                            ));
                            cx.action(AppAction::RedrawLog);
                        }
                    }
                    BuildClientMessage::AuxChanHostEndpointCreated(aux_chan_host_endpoint) => {
                        if let Some(active_build) = active.builds.get_mut(&wrap.cmd_id) {
                            active_build.aux_chan_host_endpoint = Some(aux_chan_host_endpoint);
                        } else {
                            self.pending_aux_chan_host_endpoints
                                .insert(wrap.cmd_id, aux_chan_host_endpoint);
                        }
                    }
                }
            }

            for (build_id, level, line) in pending_terminal_logs {
                self.send_terminal_log(build_id, &level, line);
            }
        }

        if self.recompile_timer.is_event(event).is_some() {
            self.start_recompile(cx);
            cx.action(AppAction::RecompileStarted);
            cx.action(AppAction::ClearLog);
        }
    }

    pub fn binary_name_to_id(&self, name: &str) -> Option<usize> {
        self.binaries.iter().position(|v| v.name == name)
    }

    pub fn binary_root_name_to_id(&self, root: &str, name: &str) -> Option<usize> {
        self.binaries
            .iter()
            .position(|v| v.root == root && v.name == name)
    }

    pub fn run_app(&mut self, cx: &mut Cx, binary_name: &str) {
        let binary_id = self.binary_name_to_id(binary_name).unwrap();
        self.start_active_build(cx, binary_id, BuildTarget::Release);
    }

    pub fn start_active_build(&mut self, _cx: &mut Cx, binary_id: usize, target: BuildTarget) {
        let binary = &self.binaries[binary_id];
        let process = BuildProcess {
            root: binary.root.clone(),
            binary: binary.name.clone(),
            target,
        };
        let item_id = process.as_id();
        if self.running_processes.contains_key(&item_id)
            || self
                .running_processes
                .values()
                .any(|running| {
                    running.root == process.root
                        && running.binary == process.binary
                        && running.target == process.target
                })
        {
            return;
        }
        self.clients[0]
            .send_cmd_with_id(item_id, BuildCmd::Run(process.clone(), self.studio_addr()));
        self.running_processes.insert(item_id, process.clone());
        //let run_view_id = LiveId::unique();
        if self.active.builds.get(&item_id).is_none() {
            let index = self.active.builds.len();
            self.active.builds.insert(
                item_id,
                ActiveBuild {
                    root: binary.root.clone(),
                    log_index: format!("[{}]", index),
                    process: process.clone(),
                    window_tabs: Default::default(),
                    app_area: Default::default(),
                    swapchain: Default::default(),
                    last_swapchain_with_completed_draws: Default::default(),
                    aux_chan_host_endpoint: None,
                },
            );
        }
        //if process.target.runs_in_studio(){
        // create the runview tab
        //    cx.action(AppA::Create(item_id, process.binary.clone()))
        //}
    }

    pub fn stop_all_active_builds(&mut self, cx: &mut Cx) {
        while let Some((build_id, process)) = self
            .running_processes
            .iter()
            .next()
            .map(|(build_id, process)| (*build_id, process.clone()))
        {
            self.running_processes.remove(&build_id);
            self.active.builds.remove(&build_id);
            if let Some(web_socket_id) = self.terminal_build_owners.remove(&build_id) {
                self.send_terminal_response(
                    web_socket_id,
                    StudioTerminalResponse::Stopped {
                        build_id: build_id.0,
                    },
                );
            }
            self.terminal_latest_widget_dumps.remove(&build_id);
            self.terminal_startup_queries.remove(&build_id);
            self.terminal_startup_dump_pending.remove(&build_id);
            self.clear_terminal_screenshots_for_build(build_id);
            self.clear_terminal_widget_tree_dumps_for_build(build_id);
            self.clients[0].send_cmd_with_id(build_id, BuildCmd::Stop);
            if process.target.runs_in_studio() {
                cx.action(AppAction::DestroyRunViews {
                    run_view_id: build_id,
                });
            }
        }
    }

    pub fn stop_active_build(&mut self, cx: &mut Cx, binary_id: usize, target: BuildTarget) {
        let binary = &self.binaries[binary_id];

        let process = BuildProcess {
            root: binary.root.clone(),
            binary: binary.name.clone(),
            target,
        };
        let matching_build_ids: Vec<LiveId> = self
            .running_processes
            .iter()
            .filter_map(|(build_id, running)| {
                (running.root == process.root
                    && running.binary == process.binary
                    && running.target == process.target)
                .then_some(*build_id)
            })
            .collect();

        for build_id in matching_build_ids {
            self.running_processes.remove(&build_id);
            let _ = self.active.builds.remove(&build_id);
            if let Some(web_socket_id) = self.terminal_build_owners.remove(&build_id) {
                self.send_terminal_response(
                    web_socket_id,
                    StudioTerminalResponse::Stopped {
                        build_id: build_id.0,
                    },
                );
            }
            self.terminal_latest_widget_dumps.remove(&build_id);
            self.terminal_startup_queries.remove(&build_id);
            self.terminal_startup_dump_pending.remove(&build_id);
            self.clear_terminal_screenshots_for_build(build_id);
            self.clear_terminal_widget_tree_dumps_for_build(build_id);
            self.clients[0].send_cmd_with_id(build_id, BuildCmd::Stop);
            if process.target.runs_in_studio() {
                cx.action(AppAction::DestroyRunViews {
                    run_view_id: build_id,
                })
            }
        }
    }
}
