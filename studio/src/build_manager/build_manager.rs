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
        makepad_platform::os::cx_stdin::{
            HostToStdin, StdinKeyModifiers, StdinMouseDown, StdinMouseMove, StdinMouseUp,
            StdinScroll, StdinToHost,
        },
        makepad_platform::studio::{
            AppToStudio, AppToStudioVec, DesignerComponentPosition, DesignerZoomPan, EventSample,
            GPUSample, StudioScreenshotRequest, StudioScreenshotResponse, StudioToApp,
            StudioToAppVec, StudioWidgetTreeDumpRequest, StudioWidgetTreeDumpResponse,
        },
        makepad_shell::*,
        makepad_widgets::*,
    },
    makepad_http::server::*,
    makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
    makepad_zune_png::PngEncoder,
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
    pub swapchain: HashMap<usize, Option<cx_stdin::HostSwapchain>>,
    pub last_swapchain_with_completed_draws: HashMap<usize, Option<cx_stdin::HostSwapchain>>,
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
    pub fn swapchain_mut(&mut self, index: usize) -> &mut Option<cx_stdin::HostSwapchain> {
        match self.swapchain.entry(index) {
            hash_map::Entry::Occupied(o) => o.into_mut(),
            hash_map::Entry::Vacant(v) => v.insert(None),
        }
    }
    pub fn last_swapchain_with_completed_draws_mut(
        &mut self,
        index: usize,
    ) -> &mut Option<cx_stdin::HostSwapchain> {
        match self.last_swapchain_with_completed_draws.entry(index) {
            hash_map::Entry::Occupied(o) => o.into_mut(),
            hash_map::Entry::Vacant(v) => v.insert(None),
        }
    }
    pub fn swapchain(&self, index: usize) -> Option<&cx_stdin::HostSwapchain> {
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
    ) -> Option<&cx_stdin::HostSwapchain> {
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
}

#[derive(Default)]
pub struct BuildManager {
    roots: FileSystemRoots,
    http_port: usize,
    pub clients: Vec<BuildClient>,
    running_processes: HashMap<LiveId, BuildProcess>,
    pending_aux_chan_host_endpoints: HashMap<LiveId, cx_stdin::aux_chan::HostEndpoint>,
    pub log: Vec<(LiveId, LogItem)>,
    pub profile: HashMap<LiveId, ProfileSampleStore>,
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
    pub designer_state: DesignerState,
    pub websocket_alive_timer: Timer,
    //pub send_file_change: FromUISender<LiveFileChange>,
    pub active_build_websockets: Arc<Mutex<RefCell<ActiveBuildWebSockets>>>,
    terminal_sockets: HashMap<u64, mpsc::Sender<Vec<u8>>>,
    terminal_build_owners: HashMap<LiveId, u64>,
    terminal_build_counter: u64,
    terminal_screenshot_counter: u64,
    terminal_screenshot_requests: HashMap<u64, PendingTerminalScreenshot>,
    terminal_widget_tree_dump_counter: u64,
    terminal_widget_tree_dump_requests: HashMap<u64, PendingTerminalWidgetTreeDump>,
    terminal_build_origins: HashMap<LiveId, (f64, f64)>,
    terminal_build_dpi: HashMap<LiveId, f64>,
    terminal_latest_widget_dumps: HashMap<LiveId, String>,
    terminal_startup_queries: HashMap<LiveId, String>,
    terminal_startup_dump_pending: HashSet<LiveId>,
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

#[derive(Clone, Copy, Debug)]
struct PendingTerminalScreenshot {
    web_socket_id: u64,
    build_id: LiveId,
    kind_id: u32,
}

#[derive(Clone, Debug)]
struct PendingTerminalWidgetTreeDump {
    web_socket_id: u64,
    build_id: LiveId,
    emit_dump: bool,
    startup_query: Option<String>,
}

const STUDIO_TERMINAL_PATH: &str = "/$studio_terminal";
const STUDIO_WEBSOCKET_PATH: &str = "/$studio_web_socket";

#[derive(Debug, Clone, SerJson, DeJson)]
pub enum StudioTerminalRequest {
    ListBuilds,
    CargoRun {
        args: Vec<String>,
        root: Option<String>,
        startup_query: Option<String>,
    },
    Stop {
        build_id: u64,
    },
    HostToStdin {
        build_id: u64,
        msg: HostToStdin,
    },
    TypeText {
        build_id: u64,
        text: String,
        replace_last: Option<bool>,
        was_paste: Option<bool>,
        auto_dump: Option<bool>,
    },
    Return {
        build_id: u64,
        auto_dump: Option<bool>,
    },
    Click {
        build_id: u64,
        x: i64,
        y: i64,
        button: Option<u32>,
        auto_dump: Option<bool>,
    },
    Screenshot {
        build_id: u64,
        kind_id: Option<u32>,
    },
    WidgetTreeDump {
        build_id: u64,
    },
    WidgetQuery {
        build_id: u64,
        query: String,
    },
}

#[derive(Debug, Clone, SerJson, DeJson)]
pub enum StudioTerminalResponse {
    Builds {
        builds: Vec<StudioTerminalBuildInfo>,
    },
    Started {
        build_id: u64,
        root: String,
        package: String,
    },
    Stopped {
        build_id: u64,
    },
    Log {
        build_id: u64,
        level: String,
        line: String,
    },
    Screenshot {
        build_id: u64,
        request_id: u64,
        kind_id: u32,
        path: String,
        width: u32,
        height: u32,
    },
    WidgetTreeDump {
        build_id: u64,
        request_id: u64,
        dump: String,
    },
    WidgetQuery {
        build_id: u64,
        query: String,
        rects: Vec<String>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, SerJson, DeJson)]
pub struct StudioTerminalBuildInfo {
    pub build_id: u64,
    pub root: String,
    pub package: String,
    pub active: bool,
    pub has_web_socket: bool,
}

enum TerminalToBuildManager {
    Connected {
        web_socket_id: u64,
        sender: mpsc::Sender<Vec<u8>>,
    },
    Disconnected {
        web_socket_id: u64,
    },
    Request {
        web_socket_id: u64,
        request: StudioTerminalRequest,
    },
}

#[derive(Default, SerRon, DeRon)]
pub struct DesignerState {
    state: HashMap<LiveId, DesignerStatePerBuildId>,
}

#[derive(Default, SerRon, DeRon)]
pub struct DesignerStatePerBuildId {
    selected_file: String,
    zoom_pan: DesignerZoomPan,
    component_positions: Vec<DesignerComponentPosition>,
}

impl DesignerState {
    fn save_state(&self) {
        let saved = self.serialize_ron();
        let mut f = File::create("makepad_designer.ron").expect("Unable to create file");
        f.write_all(saved.as_bytes()).expect("Unable to write data");
    }

    fn load_state(&mut self) {
        if let Ok(contents) = std::fs::read_to_string("makepad_designer.ron") {
            match DesignerState::deserialize_ron(&contents) {
                Ok(state) => *self = state,
                Err(e) => {
                    crate::warning!("Failed to parse makepad_designer.ron: {:?}", e);
                }
            }
        }
    }

    fn get_build_storage<F: FnOnce(&mut DesignerStatePerBuildId)>(
        &mut self,
        build_id: LiveId,
        f: F,
    ) {
        match self.state.entry(build_id) {
            hash_map::Entry::Occupied(mut v) => {
                f(v.get_mut());
            }
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

#[derive(Clone, Debug, Default)]
pub enum BuildManagerAction {
    StdinToHost {
        build_id: LiveId,
        msg: StdinToHost,
    },
    #[default]
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

fn has_message_format_json(args: &[String]) -> bool {
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--message-format=json" {
            return true;
        }
        if arg == "--message-format" && iter.peek().is_some_and(|next| next.as_str() == "json") {
            return true;
        }
        if arg
            .strip_prefix("--message-format=")
            .is_some_and(|value| value == "json")
        {
            return true;
        }
    }
    false
}

fn normalize_terminal_cargo_run_args(raw_args: Vec<String>) -> Result<Vec<String>, String> {
    let mut args = raw_args;
    if args.first().is_some_and(|arg| arg == "run") {
        args.remove(0);
    }
    if args
        .first()
        .is_some_and(|arg| !arg.starts_with('-') && arg != "--")
    {
        return Err(
            "CargoRun expects args after `cargo run` (do not pass a different cargo subcommand)"
                .to_string(),
        );
    }

    let split_index = args
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(args.len());
    let mut cargo_args = args[..split_index].to_vec();
    let mut app_args = if split_index < args.len() {
        args[(split_index + 1)..].to_vec()
    } else {
        Vec::new()
    };

    if !has_message_format_json(&cargo_args) {
        cargo_args.push("--message-format=json".to_string());
    }
    if !has_message_format_json(&app_args) {
        app_args.insert(0, "--message-format=json".to_string());
    }
    if !app_args.iter().any(|arg| arg == "--stdin-loop") {
        app_args.insert(0, "--stdin-loop".to_string());
    }

    let mut final_args = vec!["run".to_string()];
    final_args.extend(cargo_args);
    final_args.push("--".to_string());
    final_args.extend(app_args);
    Ok(final_args)
}

fn cargo_run_is_release(cargo_args: &[String]) -> bool {
    let run_args = if cargo_args.first().is_some_and(|arg| arg == "run") {
        &cargo_args[1..]
    } else {
        cargo_args
    };
    let split_index = run_args
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(run_args.len());
    run_args[..split_index].iter().any(|arg| arg == "--release")
}

fn parse_terminal_package_name(cargo_args: &[String]) -> Option<String> {
    let run_args = if cargo_args.first().is_some_and(|arg| arg == "run") {
        &cargo_args[1..]
    } else {
        cargo_args
    };
    let split_index = run_args
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(run_args.len());
    let args = &run_args[..split_index];
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "-p" | "--package" if i + 1 < args.len() => return Some(args[i + 1].clone()),
            "--bin" if i + 1 < args.len() => return Some(args[i + 1].clone()),
            arg if arg.starts_with("--package=") => {
                return arg.split_once('=').map(|(_, value)| value.to_string());
            }
            arg if arg.starts_with("--bin=") => {
                return arg.split_once('=').map(|(_, value)| value.to_string());
            }
            _ => {}
        }
        i += 1;
    }
    None
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

    let metadata = CargoMetadata::deserialize_json(&stdout)
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

    fn alloc_terminal_build_id(&mut self) -> LiveId {
        loop {
            self.terminal_build_counter = self.terminal_build_counter.wrapping_add(1);
            let build_id = LiveId::from_str("studio-terminal")
                .bytes_append(&self.terminal_build_counter.to_be_bytes());
            if !self.running_processes.contains_key(&build_id)
                && !self.active.builds.contains_key(&build_id)
            {
                return build_id;
            }
        }
    }

    fn alloc_terminal_screenshot_request_id(&mut self) -> u64 {
        loop {
            self.terminal_screenshot_counter = self.terminal_screenshot_counter.wrapping_add(1);
            if self.terminal_screenshot_counter == 0 {
                continue;
            }
            if !self
                .terminal_screenshot_requests
                .contains_key(&self.terminal_screenshot_counter)
            {
                return self.terminal_screenshot_counter;
            }
        }
    }

    fn alloc_terminal_widget_tree_dump_request_id(&mut self) -> u64 {
        loop {
            self.terminal_widget_tree_dump_counter =
                self.terminal_widget_tree_dump_counter.wrapping_add(1);
            if self.terminal_widget_tree_dump_counter == 0 {
                continue;
            }
            if !self
                .terminal_widget_tree_dump_requests
                .contains_key(&self.terminal_widget_tree_dump_counter)
            {
                return self.terminal_widget_tree_dump_counter;
            }
        }
    }

    fn clear_terminal_screenshots_for_socket(&mut self, web_socket_id: u64) {
        self.terminal_screenshot_requests
            .retain(|_, pending| pending.web_socket_id != web_socket_id);
    }

    fn clear_terminal_screenshots_for_build(&mut self, build_id: LiveId) {
        self.terminal_screenshot_requests
            .retain(|_, pending| pending.build_id != build_id);
    }

    fn clear_terminal_widget_tree_dumps_for_socket(&mut self, web_socket_id: u64) {
        self.terminal_widget_tree_dump_requests
            .retain(|_, pending| pending.web_socket_id != web_socket_id);
    }

    fn clear_terminal_widget_tree_dumps_for_build(&mut self, build_id: LiveId) {
        self.terminal_widget_tree_dump_requests
            .retain(|_, pending| pending.build_id != build_id);
    }

    fn parse_dump_origin(dump: &str) -> Option<(f64, f64, f64)> {
        for line in dump.lines() {
            let mut parts = line.split_whitespace();
            if parts.next() != Some("O") {
                continue;
            }
            let Some(x) = parts.next().and_then(|v| v.parse::<f64>().ok()) else {
                continue;
            };
            let Some(y) = parts.next().and_then(|v| v.parse::<f64>().ok()) else {
                continue;
            };
            let raw_dpi = parts
                .next()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(1.0);
            let dpi = if raw_dpi > 32.0 {
                raw_dpi / 1000.0
            } else {
                raw_dpi
            }
            .max(1.0);
            return Some((x, y, dpi));
        }
        None
    }

    fn query_widget_dump_rects(dump: &str, query: &str) -> Vec<String> {
        let query = query.trim();
        let (mode, needle) = if let Some(v) = query.strip_prefix("id:") {
            ("id", v.trim())
        } else if let Some(v) = query.strip_prefix("type:") {
            ("type", v.trim())
        } else {
            ("any", query)
        };

        let mut rects = Vec::new();
        for line in dump.lines() {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 8 {
                continue;
            }
            if cols[0].starts_with('W') || cols[0] == "O" {
                continue;
            }
            let id = cols[2];
            let ty = cols[3];
            let is_match = match mode {
                "id" => id == needle,
                "type" => ty == needle,
                _ => needle.is_empty() || id.contains(needle) || ty.contains(needle),
            };
            if !is_match {
                continue;
            }
            rects.push(format!(
                "{} {} {} {} {} {} {}",
                cols[0], id, ty, cols[4], cols[5], cols[6], cols[7]
            ));
            if rects.len() >= 256 {
                break;
            }
        }
        rects
    }

    fn terminal_control_log_label(msg: &HostToStdin) -> String {
        match msg {
            HostToStdin::MouseDown(e) => {
                format!(
                    "MouseDown x={:.1} y={:.1} button={}",
                    e.x, e.y, e.button_raw_bits
                )
            }
            HostToStdin::MouseMove(e) => format!("MouseMove x={:.1} y={:.1}", e.x, e.y),
            HostToStdin::MouseUp(e) => {
                format!(
                    "MouseUp x={:.1} y={:.1} button={}",
                    e.x, e.y, e.button_raw_bits
                )
            }
            HostToStdin::Scroll(e) => format!(
                "Scroll x={:.1} y={:.1} sx={:.1} sy={:.1}",
                e.x, e.y, e.sx, e.sy
            ),
            HostToStdin::Tick => "Tick".to_string(),
            HostToStdin::TextCopy => "TextCopy".to_string(),
            HostToStdin::TextCut => "TextCut".to_string(),
            HostToStdin::TextInput(e) => {
                let mut text = e.input.clone();
                if text.len() > 48 {
                    text.truncate(48);
                    text.push_str("...");
                }
                format!("TextInput {:?}", text)
            }
            HostToStdin::KeyDown(e) => format!("KeyDown {:?}", e.key_code),
            HostToStdin::KeyUp(e) => format!("KeyUp {:?}", e.key_code),
            HostToStdin::Swapchain(_) => "Swapchain".to_string(),
            HostToStdin::WindowGeomChange {
                window_id,
                dpi_factor,
                left,
                top,
                width,
                height,
            } => format!(
                "WindowGeomChange window={} left={:.1} top={:.1} width={:.1} height={:.1} dpi={:.3}",
                window_id, left, top, width, height, dpi_factor
            ),
        }
    }

    fn terminal_origin_for_build(&self, cx: &Cx, build_id: LiveId) -> Option<(f64, f64, f64)> {
        if let Some((ox, oy)) = self.terminal_build_origins.get(&build_id).copied() {
            let dpi = self
                .terminal_build_dpi
                .get(&build_id)
                .copied()
                .unwrap_or(1.0);
            return Some((ox, oy, dpi.max(1.0)));
        }
        if let Some(build) = self.active.builds.get(&build_id) {
            if let Some(area) = build
                .app_area
                .get(&0)
                .or_else(|| build.app_area.values().next())
            {
                if area.is_valid(cx) {
                    let rect = area.rect(cx);
                    let dpi = self
                        .terminal_build_dpi
                        .get(&build_id)
                        .copied()
                        .unwrap_or(1.0);
                    return Some((rect.pos.x * dpi, rect.pos.y * dpi, dpi.max(1.0)));
                }
            }
        }
        None
    }

    fn translate_terminal_host_to_stdin(
        msg: HostToStdin,
        origin: Option<(f64, f64, f64)>,
    ) -> Result<HostToStdin, String> {
        let Some((ox, oy, dpi)) = origin else {
            return match msg {
                HostToStdin::MouseDown(_)
                | HostToStdin::MouseMove(_)
                | HostToStdin::MouseUp(_)
                | HostToStdin::Scroll(_) => Err(
                    "no coordinate origin available yet; request WidgetTreeDump first".to_string(),
                ),
                _ => Ok(msg),
            };
        };
        let dpi = dpi.max(1.0);
        Ok(match msg {
            HostToStdin::MouseDown(mut e) => {
                e.x = (e.x + ox) / dpi;
                e.y = (e.y + oy) / dpi;
                HostToStdin::MouseDown(e)
            }
            HostToStdin::MouseMove(mut e) => {
                e.x = (e.x + ox) / dpi;
                e.y = (e.y + oy) / dpi;
                HostToStdin::MouseMove(e)
            }
            HostToStdin::MouseUp(mut e) => {
                e.x = (e.x + ox) / dpi;
                e.y = (e.y + oy) / dpi;
                HostToStdin::MouseUp(e)
            }
            HostToStdin::Scroll(mut e) => {
                e.x = (e.x + ox) / dpi;
                e.y = (e.y + oy) / dpi;
                HostToStdin::Scroll(e)
            }
            other => other,
        })
    }

    fn encode_png_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(4))
            .ok_or_else(|| "screenshot size overflow while encoding png".to_string())?;
        if rgba.len() != expected {
            return Err(format!(
                "invalid rgba length for {}x{}: expected {}, got {}",
                width,
                height,
                expected,
                rgba.len()
            ));
        }

        let options = EncoderOptions::default()
            .set_width(width as usize)
            .set_height(height as usize)
            .set_depth(BitDepth::Eight)
            .set_colorspace(ColorSpace::RGBA);

        let mut encoder = PngEncoder::new(rgba, options);
        let mut out = Vec::new();
        encoder
            .encode(&mut out)
            .map_err(|err| format!("png encode failed: {err:?}"))?;
        Ok(out)
    }

    fn bgra_to_rgba(bgra: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(4))
            .ok_or_else(|| "screenshot size overflow while converting pixels".to_string())?;
        if bgra.len() != expected {
            return Err(format!(
                "invalid bgra length for {}x{}: expected {}, got {}",
                width,
                height,
                expected,
                bgra.len()
            ));
        }

        let mut rgba = Vec::with_capacity(bgra.len());
        for px in bgra.chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
        Ok(rgba)
    }

    fn write_terminal_screenshot_png(
        &self,
        build_id: LiveId,
        kind_id: u32,
        request_id: u64,
        png: &[u8],
    ) -> Result<PathBuf, String> {
        let mut dir = std::env::temp_dir();
        dir.push("makepad_studio_terminal");
        std::fs::create_dir_all(&dir).map_err(|err| {
            format!(
                "failed to create screenshot temp dir {}: {err}",
                dir.display()
            )
        })?;

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| format!("system time error: {err}"))?
            .as_millis();
        let file_name = format!(
            "build-{}-kind-{}-req-{}-{}.png",
            build_id.0, kind_id, request_id, now_ms
        );
        let path = dir.join(file_name);
        std::fs::write(&path, png)
            .map_err(|err| format!("failed to write screenshot png {}: {err}", path.display()))?;
        Ok(path)
    }

    fn send_terminal_response_to_sender(
        sender: &mpsc::Sender<Vec<u8>>,
        response: StudioTerminalResponse,
    ) {
        let _ = sender.send(response.serialize_json().into_bytes());
    }

    fn send_terminal_response(&self, web_socket_id: u64, response: StudioTerminalResponse) {
        if let Some(sender) = self.terminal_sockets.get(&web_socket_id) {
            Self::send_terminal_response_to_sender(sender, response);
        }
    }

    fn send_terminal_log(&self, build_id: LiveId, level: &str, line: String) {
        let Some(web_socket_id) = self.terminal_build_owners.get(&build_id).copied() else {
            return;
        };
        self.send_terminal_response(
            web_socket_id,
            StudioTerminalResponse::Log {
                build_id: build_id.0,
                level: level.to_string(),
                line,
            },
        );
    }

    fn send_terminal_error(&self, web_socket_id: u64, message: impl Into<String>) {
        self.send_terminal_response(
            web_socket_id,
            StudioTerminalResponse::Error {
                message: message.into(),
            },
        );
    }

    fn log_terminal_bridge_event(&mut self, cx: &mut Cx, line: String) {
        self.log.push((
            LiveId::from_str("studio_terminal_bridge"),
            LogItem::Bare(LogItemBare {
                level: LogLevel::Log,
                line,
            }),
        ));
        cx.action(AppAction::RedrawLog);
    }

    fn request_terminal_screenshot(
        &mut self,
        cx: &mut Cx,
        web_socket_id: u64,
        build_id: LiveId,
        kind_id: u32,
    ) -> Result<(), String> {
        let request_id = self.alloc_terminal_screenshot_request_id();
        self.terminal_screenshot_requests.insert(
            request_id,
            PendingTerminalScreenshot {
                web_socket_id,
                build_id,
                kind_id,
            },
        );

        let sent = if let Ok(sockets) = self.active_build_websockets.lock() {
            sockets.borrow_mut().send_studio_to_app(
                build_id,
                StudioToApp::Screenshot(StudioScreenshotRequest {
                    request_id,
                    kind_id,
                }),
            )
        } else {
            false
        };

        if sent {
            self.log_terminal_bridge_event(
                cx,
                format!(
                    "studio_terminal -> child build={} Screenshot request_id={} kind_id={}",
                    build_id.0, request_id, kind_id
                ),
            );
            Ok(())
        } else {
            self.terminal_screenshot_requests.remove(&request_id);
            Err(format!(
                "build {} has no active studio websocket connection",
                build_id.0
            ))
        }
    }

    fn request_terminal_widget_tree_dump(
        &mut self,
        cx: &mut Cx,
        web_socket_id: u64,
        build_id: LiveId,
        startup_query: Option<String>,
        emit_dump: bool,
    ) -> Result<(), String> {
        let request_id = self.alloc_terminal_widget_tree_dump_request_id();
        self.terminal_widget_tree_dump_requests.insert(
            request_id,
            PendingTerminalWidgetTreeDump {
                web_socket_id,
                build_id,
                emit_dump,
                startup_query,
            },
        );

        let sent = if let Ok(sockets) = self.active_build_websockets.lock() {
            sockets.borrow_mut().send_studio_to_app(
                build_id,
                StudioToApp::WidgetTreeDump(StudioWidgetTreeDumpRequest { request_id }),
            )
        } else {
            false
        };

        if sent {
            self.log_terminal_bridge_event(
                cx,
                format!(
                    "studio_terminal -> child build={} WidgetTreeDump request_id={}",
                    build_id.0, request_id
                ),
            );
            Ok(())
        } else {
            self.terminal_widget_tree_dump_requests.remove(&request_id);
            Err(format!(
                "build {} has no active studio websocket connection",
                build_id.0
            ))
        }
    }

    fn handle_terminal_screenshot_response(
        &mut self,
        build_id: LiveId,
        screenshot: &StudioScreenshotResponse,
    ) {
        let pending: Vec<(u64, PendingTerminalScreenshot)> = screenshot
            .request_ids
            .iter()
            .filter_map(|request_id| {
                self.terminal_screenshot_requests
                    .remove(request_id)
                    .map(|pending| (*request_id, pending))
            })
            .collect();

        if pending.is_empty() {
            return;
        }

        let Some(image) = screenshot.image.as_deref() else {
            for (request_id, pending_request) in pending {
                self.send_terminal_error(
                    pending_request.web_socket_id,
                    format!(
                        "screenshot request {} for build {} returned no image",
                        request_id, build_id.0
                    ),
                );
            }
            return;
        };

        let rgba = match Self::bgra_to_rgba(image, screenshot.width, screenshot.height) {
            Ok(rgba) => rgba,
            Err(err) => {
                for (_, pending_request) in pending {
                    self.send_terminal_error(
                        pending_request.web_socket_id,
                        format!(
                            "failed to convert screenshot for build {}: {err}",
                            build_id.0
                        ),
                    );
                }
                return;
            }
        };

        let png = match Self::encode_png_rgba(screenshot.width, screenshot.height, &rgba) {
            Ok(png) => png,
            Err(err) => {
                for (_, pending_request) in pending {
                    self.send_terminal_error(
                        pending_request.web_socket_id,
                        format!(
                            "failed to encode screenshot for build {}: {err}",
                            build_id.0
                        ),
                    );
                }
                return;
            }
        };

        for (request_id, pending_request) in pending {
            match self.write_terminal_screenshot_png(
                pending_request.build_id,
                pending_request.kind_id,
                request_id,
                &png,
            ) {
                Ok(path) => self.send_terminal_response(
                    pending_request.web_socket_id,
                    StudioTerminalResponse::Screenshot {
                        build_id: pending_request.build_id.0,
                        request_id,
                        kind_id: pending_request.kind_id,
                        path: path.to_string_lossy().into_owned(),
                        width: screenshot.width,
                        height: screenshot.height,
                    },
                ),
                Err(err) => self.send_terminal_error(pending_request.web_socket_id, err),
            }
        }
    }

    fn handle_terminal_widget_tree_dump_response(
        &mut self,
        build_id: LiveId,
        dump_response: StudioWidgetTreeDumpResponse,
    ) {
        let Some(pending_request) = self
            .terminal_widget_tree_dump_requests
            .remove(&dump_response.request_id)
        else {
            return;
        };

        if pending_request.build_id != build_id {
            self.send_terminal_error(
                pending_request.web_socket_id,
                format!(
                    "widget tree dump request {} expected build {}, got {}",
                    dump_response.request_id, pending_request.build_id.0, build_id.0
                ),
            );
            return;
        }

        let dump = dump_response.dump;
        if let Some((ox, oy, dpi)) = Self::parse_dump_origin(&dump) {
            self.terminal_build_origins.insert(build_id, (ox, oy));
            self.terminal_build_dpi.insert(build_id, dpi.max(1.0));
        }
        self.terminal_latest_widget_dumps
            .insert(build_id, dump.clone());
        if pending_request.emit_dump {
            self.send_terminal_response(
                pending_request.web_socket_id,
                StudioTerminalResponse::WidgetTreeDump {
                    build_id: pending_request.build_id.0,
                    request_id: dump_response.request_id,
                    dump: dump.clone(),
                },
            );
        }
        if let Some(query) = pending_request.startup_query {
            let rects = Self::query_widget_dump_rects(&dump, &query);
            self.send_terminal_response(
                pending_request.web_socket_id,
                StudioTerminalResponse::WidgetQuery {
                    build_id: pending_request.build_id.0,
                    query,
                    rects,
                },
            );
        }
    }

    fn terminal_now() -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|v| v.as_secs_f64())
            .unwrap_or(0.0)
    }

    fn send_terminal_host_to_stdin(
        &mut self,
        cx: &mut Cx,
        web_socket_id: u64,
        build_id: LiveId,
        msg: HostToStdin,
        auto_dump: bool,
    ) -> Result<(), String> {
        let origin = self.terminal_origin_for_build(cx, build_id);
        let msg = Self::translate_terminal_host_to_stdin(
            msg,
            origin,
        )?;
        let msg_label = Self::terminal_control_log_label(&msg);
        let origin_label = origin
            .map(|(ox, oy, dpi)| format!("origin=({ox:.1},{oy:.1}) dpi={dpi:.3}"))
            .unwrap_or_else(|| "origin=(none)".to_string());
        let sent = if let Ok(sockets) = self.active_build_websockets.lock() {
            sockets
                .borrow_mut()
                .send_studio_to_app(build_id, StudioToApp::HostToStdin(msg))
        } else {
            false
        };
        if !sent {
            return Err(format!(
                "build {} has no active studio websocket connection",
                build_id.0
            ));
        }
        self.log_terminal_bridge_event(
            cx,
            format!(
                "studio_terminal -> child build={} {} {}",
                build_id.0, msg_label, origin_label
            ),
        );
        if auto_dump {
            self.request_terminal_widget_tree_dump(cx, web_socket_id, build_id, None, true)?;
        }
        Ok(())
    }

    fn handle_terminal_request(
        &mut self,
        cx: &mut Cx,
        web_socket_id: u64,
        request: StudioTerminalRequest,
    ) {
        match request {
            StudioTerminalRequest::ListBuilds => {
                let mut build_ids: BTreeSet<LiveId> = BTreeSet::new();
                build_ids.extend(self.running_processes.keys().copied());
                build_ids.extend(self.active.builds.keys().copied());
                let builds: Vec<StudioTerminalBuildInfo> = build_ids
                    .into_iter()
                    .map(|build_id| {
                        let process = self
                            .running_processes
                            .get(&build_id)
                            .or_else(|| self.active.builds.get(&build_id).map(|b| &b.process));
                        let (root, package) = if let Some(process) = process {
                            (process.root.clone(), process.binary.clone())
                        } else {
                            ("".to_string(), "".to_string())
                        };
                        let has_web_socket = if let Ok(sockets) = self.active_build_websockets.lock()
                        {
                            sockets.borrow().sockets.iter().any(|s| s.build_id == build_id)
                        } else {
                            false
                        };
                        StudioTerminalBuildInfo {
                            build_id: build_id.0,
                            root,
                            package,
                            active: self.active.builds.contains_key(&build_id),
                            has_web_socket,
                        }
                    })
                    .collect();
                self.send_terminal_response(web_socket_id, StudioTerminalResponse::Builds { builds });
            }
            StudioTerminalRequest::CargoRun {
                args,
                root,
                startup_query,
            } => match self.start_terminal_cargo_run(web_socket_id, args, root, startup_query) {
                Ok((build_id, root, package)) => {
                    self.send_terminal_response(
                        web_socket_id,
                        StudioTerminalResponse::Started {
                            build_id: build_id.0,
                            root,
                            package,
                        },
                    );
                }
                Err(message) => {
                    self.send_terminal_response(
                        web_socket_id,
                        StudioTerminalResponse::Error { message },
                    );
                }
            },
            StudioTerminalRequest::Stop { build_id } => {
                let build_id = LiveId(build_id);
                let removed_running = self.running_processes.remove(&build_id).is_some();
                let removed_active = self.active.builds.remove(&build_id).is_some();
                self.terminal_build_owners.remove(&build_id);
                self.terminal_build_origins.remove(&build_id);
                self.terminal_build_dpi.remove(&build_id);
                self.terminal_latest_widget_dumps.remove(&build_id);
                self.terminal_startup_queries.remove(&build_id);
                self.terminal_startup_dump_pending.remove(&build_id);
                self.clear_terminal_screenshots_for_build(build_id);
                self.clear_terminal_widget_tree_dumps_for_build(build_id);
                self.clients[0].send_cmd_with_id(build_id, BuildCmd::Stop);
                if removed_active {
                    cx.action(AppAction::DestroyRunViews {
                        run_view_id: build_id,
                    });
                }
                if removed_running || removed_active {
                    self.send_terminal_response(
                        web_socket_id,
                        StudioTerminalResponse::Stopped {
                            build_id: build_id.0,
                        },
                    );
                } else {
                    self.send_terminal_response(
                        web_socket_id,
                        StudioTerminalResponse::Error {
                            message: format!("unknown build id {}", build_id.0),
                        },
                    );
                }
            }
            StudioTerminalRequest::HostToStdin { build_id, msg } => {
                let build_id = LiveId(build_id);
                let should_auto_dump =
                    !matches!(msg, HostToStdin::MouseMove(_) | HostToStdin::Scroll(_));
                if let Err(message) = self.send_terminal_host_to_stdin(
                    cx,
                    web_socket_id,
                    build_id,
                    msg,
                    should_auto_dump,
                ) {
                    self.send_terminal_error(
                        web_socket_id,
                        format!("build {}: {message}", build_id.0),
                    );
                }
            }
            StudioTerminalRequest::TypeText {
                build_id,
                text,
                replace_last,
                was_paste,
                auto_dump,
            } => {
                let build_id = LiveId(build_id);
                let msg = HostToStdin::TextInput(TextInputEvent {
                    input: text,
                    replace_last: replace_last.unwrap_or(false),
                    was_paste: was_paste.unwrap_or(false),
                });
                if let Err(message) = self.send_terminal_host_to_stdin(
                    cx,
                    web_socket_id,
                    build_id,
                    msg,
                    auto_dump.unwrap_or(false),
                )
                {
                    self.send_terminal_error(
                        web_socket_id,
                        format!("build {}: {message}", build_id.0),
                    );
                }
            }
            StudioTerminalRequest::Return {
                build_id,
                auto_dump,
            } => {
                let build_id = LiveId(build_id);
                let now = Self::terminal_now();
                let modifiers = KeyModifiers::default();
                let auto_dump = auto_dump.unwrap_or(false);
                let msgs = [
                    (
                        HostToStdin::KeyDown(KeyEvent {
                            key_code: KeyCode::ReturnKey,
                            is_repeat: false,
                            modifiers,
                            time: now,
                        }),
                        false,
                    ),
                    (
                        HostToStdin::KeyUp(KeyEvent {
                            key_code: KeyCode::ReturnKey,
                            is_repeat: false,
                            modifiers,
                            time: now + 0.01,
                        }),
                        auto_dump,
                    ),
                ];
                for (msg, auto_dump) in msgs {
                    if let Err(message) = self.send_terminal_host_to_stdin(
                        cx,
                        web_socket_id,
                        build_id,
                        msg,
                        auto_dump,
                    ) {
                        self.send_terminal_error(
                            web_socket_id,
                            format!("build {}: {message}", build_id.0),
                        );
                        break;
                    }
                }
            }
            StudioTerminalRequest::Click {
                build_id,
                x,
                y,
                button,
                auto_dump,
            } => {
                let build_id = LiveId(build_id);
                let button_raw_bits = button.unwrap_or(1);
                let auto_dump = auto_dump.unwrap_or(false);
                let now = Self::terminal_now();
                let modifiers = StdinKeyModifiers::default();
                let msgs = [
                    (
                        HostToStdin::MouseMove(StdinMouseMove {
                            time: now,
                            x: x as f64,
                            y: y as f64,
                            modifiers,
                        }),
                        false,
                    ),
                    (
                        HostToStdin::MouseDown(StdinMouseDown {
                            button_raw_bits,
                            x: x as f64,
                            y: y as f64,
                            time: now,
                            modifiers,
                        }),
                        false,
                    ),
                    (
                        HostToStdin::MouseUp(StdinMouseUp {
                            time: now + 0.01,
                            button_raw_bits,
                            x: x as f64,
                            y: y as f64,
                            modifiers,
                        }),
                        auto_dump,
                    ),
                ];
                for (msg, auto_dump) in msgs {
                    if let Err(message) = self.send_terminal_host_to_stdin(
                        cx,
                        web_socket_id,
                        build_id,
                        msg,
                        auto_dump,
                    ) {
                        self.send_terminal_error(
                            web_socket_id,
                            format!("build {}: {message}", build_id.0),
                        );
                        break;
                    }
                }
            }
            StudioTerminalRequest::Screenshot { build_id, kind_id } => {
                let build_id = LiveId(build_id);
                if let Err(message) =
                    self.request_terminal_screenshot(cx, web_socket_id, build_id, kind_id.unwrap_or(0))
                {
                    self.send_terminal_error(web_socket_id, message);
                }
            }
            StudioTerminalRequest::WidgetTreeDump { build_id } => {
                let build_id = LiveId(build_id);
                if let Err(message) =
                    self.request_terminal_widget_tree_dump(cx, web_socket_id, build_id, None, true)
                {
                    self.send_terminal_error(web_socket_id, message);
                }
            }
            StudioTerminalRequest::WidgetQuery { build_id, query } => {
                let build_id = LiveId(build_id);
                let Some(dump) = self.terminal_latest_widget_dumps.get(&build_id) else {
                    self.send_terminal_error(
                        web_socket_id,
                        format!(
                            "build {} has no cached widget tree yet; wait for startup dump",
                            build_id.0
                        ),
                    );
                    return;
                };
                let rects = Self::query_widget_dump_rects(dump, &query);
                self.send_terminal_response(
                    web_socket_id,
                    StudioTerminalResponse::WidgetQuery {
                        build_id: build_id.0,
                        query,
                        rects,
                    },
                );
            }
        }
    }

    fn start_terminal_cargo_run(
        &mut self,
        web_socket_id: u64,
        args: Vec<String>,
        root: Option<String>,
        startup_query: Option<String>,
    ) -> Result<(LiveId, String, String), String> {
        let root = if let Some(root) = root {
            root
        } else {
            self.default_root_name()
                .ok_or_else(|| "studio has no configured roots".to_string())?
        };

        self.roots
            .find_root(&root)
            .map_err(|_| format!("unknown root '{root}'"))?;

        let cargo_args = normalize_terminal_cargo_run_args(args)?;
        let build_id = self.alloc_terminal_build_id();
        let package = parse_terminal_package_name(&cargo_args)
            .unwrap_or_else(|| format!("cargo-run-{}", build_id.0));
        let target = if cargo_run_is_release(&cargo_args) {
            BuildTarget::ReleaseStudio
        } else {
            BuildTarget::DebugStudio
        };
        let process = BuildProcess {
            root: root.clone(),
            binary: package.clone(),
            target,
        };

        self.clients[0].send_cmd_with_id(
            build_id,
            BuildCmd::RunCargo(process.clone(), cargo_args, self.studio_addr()),
        );
        self.running_processes.insert(build_id, process);
        self.terminal_build_owners.insert(build_id, web_socket_id);
        self.terminal_startup_dump_pending.insert(build_id);
        if let Some(query) = startup_query.map(|q| q.trim().to_string()) {
            if !query.is_empty() {
                self.terminal_startup_queries.insert(build_id, query);
            }
        }
        Ok((build_id, root, package))
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
        self.designer_state.load_state();
        self.update_run_list(cx);
        self.websocket_alive_timer = cx.start_interval(1.0);
        // Set a small debounce timeout for recompilation (300ms)
        self.recompile_timeout = 0.3;
    }

    pub fn send_host_to_stdin(&self, item_id: LiveId, msg: HostToStdin) {
        let runs_in_studio = self
            .active
            .builds
            .get(&item_id)
            .is_some_and(|build| build.process.target.runs_in_studio());

        if let Ok(sockets) = self.active_build_websockets.lock() {
            if sockets
                .borrow_mut()
                .send_studio_to_app(item_id, StudioToApp::HostToStdin(msg.clone()))
            {
                return;
            }
        }

        if runs_in_studio {
            return;
        }

        self.clients[0].send_cmd_with_id(item_id, BuildCmd::HostToStdin(msg.to_json()));
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
        if let Some(web_socket_id) = self.terminal_build_owners.remove(&build_id) {
            self.send_terminal_response(
                web_socket_id,
                StudioTerminalResponse::Stopped {
                    build_id: build_id.0,
                },
            );
        }
        self.terminal_build_origins.remove(&build_id);
        self.terminal_build_dpi.remove(&build_id);
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
            self.terminal_build_origins.remove(&build_id);
            self.terminal_build_dpi.remove(&build_id);
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
        self.terminal_build_origins.clear();
        self.terminal_build_dpi.clear();
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
        self.profile.clear();
    }

    pub fn start_recompile_timer(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.recompile_timer);
        self.recompile_timer = cx.start_timeout(self.recompile_timeout);
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

    pub fn broadcast_to_stdin(&mut self, msg: HostToStdin) {
        let build_ids: Vec<LiveId> = self.active.builds.keys().copied().collect();
        for build_id in build_ids {
            self.send_host_to_stdin(build_id, msg.clone());
        }
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, file_system: &mut FileSystem) {
        if let Some(_) = self.tick_timer.is_event(event) {
            self.broadcast_to_stdin(HostToStdin::Tick);
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
                            self.send_host_to_stdin(
                                *build_id,
                                HostToStdin::MouseDown(StdinMouseDown {
                                    time: e.time,
                                    x: e.abs.x,
                                    y: e.abs.y,
                                    button_raw_bits: e.button.bits(),
                                    modifiers: StdinKeyModifiers::from_key_modifiers(&e.modifiers),
                                }),
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
            let mut pending_terminal_logs: Vec<(LiveId, String, String)> = Vec::new();
            if let Ok(mut addr) = self.recv_external_ip.try_recv() {
                addr.set_port(self.http_port as u16);
                self.studio_http = format!("http://{}{}", addr, STUDIO_WEBSOCKET_PATH);
            }

            while let Ok(msg) = self.recv_terminal_msg.try_recv() {
                match msg {
                    TerminalToBuildManager::Connected {
                        web_socket_id,
                        sender,
                    } => {
                        self.terminal_sockets.insert(web_socket_id, sender);
                    }
                    TerminalToBuildManager::Disconnected { web_socket_id } => {
                        self.terminal_sockets.remove(&web_socket_id);
                        let owned_builds: Vec<LiveId> = self
                            .terminal_build_owners
                            .iter()
                            .filter_map(|(build_id, owner)| {
                                (*owner == web_socket_id).then_some(*build_id)
                            })
                            .collect();
                        self.terminal_build_owners
                            .retain(|_, owner| *owner != web_socket_id);
                        for build_id in owned_builds {
                            self.terminal_build_origins.remove(&build_id);
                            self.terminal_build_dpi.remove(&build_id);
                            self.terminal_latest_widget_dumps.remove(&build_id);
                            self.terminal_startup_queries.remove(&build_id);
                            self.terminal_startup_dump_pending.remove(&build_id);
                        }
                        self.clear_terminal_screenshots_for_socket(web_socket_id);
                        self.clear_terminal_widget_tree_dumps_for_socket(web_socket_id);
                    }
                    TerminalToBuildManager::Request {
                        web_socket_id,
                        request,
                    } => {
                        self.handle_terminal_request(cx, web_socket_id, request);
                    }
                }
            }

            while let Ok(build_id) = self.recv_studio_disconnect.try_recv() {
                let had_local_process = self.running_processes.remove(&build_id).is_some();
                self.active.builds.remove(&build_id);
                if let Some(web_socket_id) = self.terminal_build_owners.remove(&build_id) {
                    self.send_terminal_response(
                        web_socket_id,
                        StudioTerminalResponse::Stopped {
                            build_id: build_id.0,
                        },
                    );
                }
                self.terminal_build_origins.remove(&build_id);
                self.terminal_build_dpi.remove(&build_id);
                self.terminal_latest_widget_dumps.remove(&build_id);
                self.terminal_startup_queries.remove(&build_id);
                self.terminal_startup_dump_pending.remove(&build_id);
                self.clear_terminal_screenshots_for_build(build_id);
                self.clear_terminal_widget_tree_dumps_for_build(build_id);
                if had_local_process {
                    self.clients[0].send_cmd_with_id(build_id, BuildCmd::Stop);
                }
                cx.action(AppAction::DestroyRunViews {
                    run_view_id: build_id,
                });
            }

            while let Ok((build_id, msgs)) = self.recv_studio_msg.try_recv() {
                self.ensure_active_build(build_id);
                for msg in msgs.0 {
                    match msg {
                        AppToStudio::StdinToHost(msg) => {
                            let auto_request_widget_tree_dump = self
                                .terminal_startup_dump_pending
                                .contains(&build_id)
                                && matches!(&msg, StdinToHost::DrawCompleteAndFlip(_));
                            cx.action(BuildManagerAction::StdinToHost { build_id, msg });
                            if auto_request_widget_tree_dump {
                                if let Some(web_socket_id) =
                                    self.terminal_build_owners.get(&build_id).copied()
                                {
                                    let startup_query =
                                        self.terminal_startup_queries.get(&build_id).cloned();
                                    let emit_dump = startup_query.is_none();
                                    if let Err(message) = self.request_terminal_widget_tree_dump(
                                        cx,
                                        web_socket_id,
                                        build_id,
                                        startup_query,
                                        emit_dump,
                                    ) {
                                        self.send_terminal_error(web_socket_id, message);
                                    } else {
                                        self.terminal_startup_dump_pending.remove(&build_id);
                                    }
                                }
                            }
                        }
                        AppToStudio::LogItem(item) => {
                            let terminal_level = log_level_name(item.level);
                            let terminal_line = item.message.clone();
                            let file_name = if let Some(build) = self.active.builds.get(&build_id) {
                                self.roots.map_path(&build.root, &item.file_name)
                            } else {
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
                            if let Some(file_id) = file_system.path_to_file_node_id(&file_name) {
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
                            self.log.push((
                                build_id,
                                LogItem::Location(LogItemLocation {
                                    level: item.level,
                                    file_name,
                                    start,
                                    end,
                                    message: item.message,
                                    explanation: item.explanation,
                                }),
                            ));
                            pending_terminal_logs.push((
                                build_id,
                                terminal_level.to_string(),
                                terminal_line,
                            ));
                            cx.action(AppAction::RedrawLog)
                        }
                        AppToStudio::Screenshot(screenshot) => {
                            self.handle_terminal_screenshot_response(build_id, &screenshot);

                            // Keep legacy snapshot path for studio snapshots.
                            if let Some(build) = self.active.builds.get(&build_id) {
                                if let Some(image) = screenshot.image {
                                    file_system.save_snapshot_image(
                                        cx,
                                        &build.root,
                                        "qtest",
                                        screenshot.width as _,
                                        screenshot.height as _,
                                        image,
                                    )
                                }
                            }
                        }
                        AppToStudio::WidgetTreeDump(dump_response) => {
                            self.handle_terminal_widget_tree_dump_response(build_id, dump_response);
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
                        AppToStudio::DesignerComponentMoved(mv) => {
                            self.designer_state.get_build_storage(build_id, |bs| {
                                if let Some(v) =
                                    bs.component_positions.iter_mut().find(|v| v.id == mv.id)
                                {
                                    *v = mv;
                                } else {
                                    bs.component_positions.push(mv);
                                }
                            });
                            self.designer_state.save_state();
                        }
                        AppToStudio::DesignerZoomPan(zp) => {
                            self.designer_state.get_build_storage(build_id, |bs| {
                                bs.zoom_pan = zp;
                            });
                            self.designer_state.save_state();
                        }
                        AppToStudio::DesignerStarted => {
                            // send the app the select file init message
                            if let Ok(d) = self.active_build_websockets.lock() {
                                if let Some(bs) = self.designer_state.state.get(&build_id) {
                                    let data = StudioToAppVec(vec![
                                        StudioToApp::DesignerLoadState {
                                            zoom_pan: bs.zoom_pan.clone(),
                                            positions: bs.component_positions.clone(),
                                        },
                                        StudioToApp::DesignerSelectFile {
                                            file_name: bs.selected_file.clone(),
                                        },
                                    ])
                                    .serialize_bin();

                                    for socket in d.borrow_mut().sockets.iter_mut() {
                                        if socket.build_id == build_id {
                                            let _ = socket.sender.send(data.clone());
                                        }
                                    }
                                }
                            }
                        }
                        AppToStudio::DesignerFileSelected { file_name } => {
                            // alright now what. lets
                            self.designer_state.get_build_storage(build_id, |bs| {
                                bs.selected_file = file_name;
                            });
                            self.designer_state.save_state();
                        }
                    }
                }
            }

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
                    BuildClientMessage::LogItem(LogItem::StdinToHost(line)) => {
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

    pub fn start_http_server(&mut self) {
        let (tx_request, rx_request) = mpsc::channel::<HttpServerRequest>();
        const MAX_HTTP_PORT_RETRIES: u16 = 32;

        let mut bound_port = None;
        for offset in 0..MAX_HTTP_PORT_RETRIES {
            let Some(port) = (self.http_port as u16).checked_add(offset) else {
                break;
            };
            let addr = SocketAddr::new("0.0.0.0".parse().unwrap(), port);
            if start_http_server(HttpServer {
                listen_address: addr,
                post_max_size: 1024 * 1024,
                request: tx_request.clone(),
            })
            .is_some()
            {
                bound_port = Some(port as usize);
                break;
            }
        }

        let Some(bound_port) = bound_port else {
            println!(
                "Cannot bind studio http server on ports {}..{}",
                self.http_port,
                self.http_port + (MAX_HTTP_PORT_RETRIES as usize).saturating_sub(1)
            );
            return;
        };

        if bound_port != self.http_port {
            self.http_port = bound_port;
            let local_ip = get_local_ip();
            self.studio_http = format!(
                "http://{}:{}{}",
                local_ip, self.http_port, STUDIO_WEBSOCKET_PATH
            );
            println!("Studio http fallback : {:?}", self.studio_http);
        }
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
        let studio_disconnect_sender = self.recv_studio_disconnect.sender();
        let terminal_sender = self.recv_terminal_msg.sender();
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
            enum SocketKind {
                App(LiveId),
                Terminal,
            }
            let mut socket_kinds: HashMap<u64, SocketKind> = HashMap::new();
            while let Ok(message) = rx_request.recv() {
                // only store last change, fix later
                match message {
                    HttpServerRequest::ConnectWebSocket {
                        web_socket_id,
                        response_sender,
                        headers,
                    } => {
                        if headers.path == STUDIO_TERMINAL_PATH {
                            socket_kinds.insert(web_socket_id, SocketKind::Terminal);
                            let _ = terminal_sender.send(TerminalToBuildManager::Connected {
                                web_socket_id,
                                sender: response_sender,
                            });
                            continue;
                        }

                        let build_id = headers
                            .path
                            .rsplit('/')
                            .next()
                            .and_then(|id| id.parse::<u64>().ok())
                            .map(LiveId)
                            .unwrap_or(LiveId(web_socket_id));
                        socket_kinds.insert(web_socket_id, SocketKind::App(build_id));
                        active_build_websockets
                            .lock()
                            .unwrap()
                            .borrow_mut()
                            .sockets
                            .push(ActiveBuildSocket {
                                web_socket_id,
                                build_id,
                                sender: response_sender,
                        });
                    }
                    HttpServerRequest::DisconnectWebSocket { web_socket_id } => {
                        if let Some(kind) = socket_kinds.remove(&web_socket_id) {
                            match kind {
                                SocketKind::App(build_id) => {
                                    let still_connected = socket_kinds.values().any(|kind| {
                                        matches!(kind, SocketKind::App(id) if *id == build_id)
                                    });
                                    if !still_connected {
                                        let _ = studio_disconnect_sender.send(build_id);
                                    }
                                }
                                SocketKind::Terminal => {
                                    let _ = terminal_sender.send(
                                        TerminalToBuildManager::Disconnected { web_socket_id },
                                    );
                                }
                            }
                        }
                        active_build_websockets
                            .lock()
                            .unwrap()
                            .borrow_mut()
                            .sockets
                            .retain(|v| v.web_socket_id != web_socket_id);
                    }
                    HttpServerRequest::TextMessage {
                        web_socket_id,
                        response_sender,
                        string,
                    } => {
                        if matches!(socket_kinds.get(&web_socket_id), Some(SocketKind::Terminal)) {
                            match StudioTerminalRequest::deserialize_json(&string) {
                                Ok(request) => {
                                    let _ = terminal_sender.send(TerminalToBuildManager::Request {
                                        web_socket_id,
                                        request,
                                    });
                                }
                                Err(err) => {
                                    let message =
                                        format!("invalid terminal request: {err:?} json={string}");
                                    BuildManager::send_terminal_response_to_sender(
                                        &response_sender,
                                        StudioTerminalResponse::Error {
                                            message: message.clone(),
                                        },
                                    );
                                }
                            }
                        }
                    }
                    HttpServerRequest::BinaryMessage {
                        web_socket_id,
                        response_sender: _,
                        data,
                    } => {
                        if let Some(SocketKind::App(id)) = socket_kinds.get(&web_socket_id) {
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
                        } else if path.ends_with(".md") {
                            "text/markdown"
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
            self.terminal_build_origins.remove(&build_id);
            self.terminal_build_dpi.remove(&build_id);
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
            self.terminal_build_origins.remove(&build_id);
            self.terminal_build_dpi.remove(&build_id);
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
