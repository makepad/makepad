use crate::ai_manager::{AiManager, AiTerminalObservation, AiToolExecutionResult};
use crate::build_manager::BuildManager;
use crate::log_store::{
    query_log_entries, AppendLogEntry, LogQuery, LogStore, ProfilerQuery, ProfilerStore,
};
use crate::script_manager::{ScriptId, ScriptManager, MAKEPAD_SPLASH_RUNNABLE};
use crate::terminal_manager::TerminalManager;
use crate::virtual_fs::VirtualFs;
use crate::worker_pool::WorkerPool;
use backend_proto::{
    AiAgentId, AppSocketInfo, BuildBoxInfo, BuildBoxStatus, BuildBoxToHub, BuildBoxToHubVec,
    BuildInfo, ClientId, ClientToHub, ClientToHubEnvelope, EventSample as HubEventSample,
    GCSample as StudioGCSample, GPUSample as StudioGPUSample, HubToBuildBox, HubToBuildBoxVec,
    HubToClient, LogEntry, LogSource, QueryId, RunItem, RunViewInputVizKind, SaveResult,
    SearchResult, TerminalFramebuffer,
};
use makepad_filesystem_watcher::{FileSystemWatcher, WatchRoot};
use makepad_git::{FileStatus as GitFileStatus, Repository as GitRepository};
use makepad_live_id::LiveId;
use makepad_micro_serde::*;
use makepad_network::NetworkResponse;
use makepad_script_std::makepad_network::ToUISender;
use makepad_studio_protocol::hub_protocol as backend_proto;
use makepad_studio_protocol::{
    AppToStudio, AppToStudioVec, EventSample, GCSample, GPUSample, KeyCode, KeyEvent, KeyModifiers,
    LogLevel, MouseButton, RemoteKeyModifiers, RemoteMouseDown, RemoteMouseUp, ScreenshotRequest,
    StudioToApp, StudioToAppVec, TextInputEvent, WidgetQueryRequest, WidgetSnapshotRequest,
    WidgetTreeDumpRequest,
};
use makepad_terminal_core::{StyleFlags, TermKeyCode, Terminal};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireFormat {
    Binary,
    Text,
}

#[derive(Debug)]
pub enum HubEvent {
    ClientConnected {
        web_socket_id: u64,
        sender: ToUISender<Vec<u8>>,
        typed_sender: Option<ToUISender<HubToClient>>,
    },
    ClientDisconnected {
        web_socket_id: u64,
    },
    ClientEnvelope {
        web_socket_id: u64,
        envelope: ClientToHubEnvelope,
    },
    ClientBinary {
        web_socket_id: u64,
        data: Vec<u8>,
    },
    ClientText {
        web_socket_id: u64,
        text: String,
    },
    AppConnected {
        build_id: Option<QueryId>,
        crate_name: Option<String>,
        web_socket_id: u64,
        sender: Sender<Vec<u8>>,
    },
    AppDisconnected {
        web_socket_id: u64,
    },
    AppBinary {
        web_socket_id: u64,
        data: Vec<u8>,
    },
    ProcessAppMessage {
        build_id: QueryId,
        msg: AppToStudio,
    },
    BuildBoxConnected {
        web_socket_id: u64,
        sender: Sender<Vec<u8>>,
    },
    BuildBoxDisconnected {
        web_socket_id: u64,
    },
    BuildBoxBinary {
        web_socket_id: u64,
        data: Vec<u8>,
    },
    ProcessOutput {
        build_id: QueryId,
        is_stderr: bool,
        line: String,
    },
    ProcessExited {
        build_id: QueryId,
        exit_code: Option<i32>,
    },
    RunItemsUpdated {
        mount: String,
        items: Vec<RunItem>,
    },
    ScriptRunRequest {
        child_build_id: Option<QueryId>,
        mount: String,
        cwd: PathBuf,
        program: String,
        args: Vec<String>,
        env: HashMap<String, String>,
        package: Option<String>,
    },
    ScriptOutput {
        script_id: ScriptId,
        mount: String,
        is_stderr: bool,
        line: String,
    },
    ScriptExited {
        script_id: ScriptId,
        mount: String,
        exit_code: Option<i32>,
    },
    TerminalOutput {
        path: String,
        data: Vec<u8>,
    },
    TerminalResized {
        path: String,
        cols: u16,
        rows: u16,
    },
    TerminalExited {
        path: String,
        exit_code: i32,
    },
    AiHttpResponse {
        response: NetworkResponse,
    },
    AiChatGptOAuthCode {
        mount: String,
        backend_id: String,
        code: String,
    },
    AiToolExecutionDone {
        mount: String,
        agent_id: AiAgentId,
        run_token: u64,
        results: Vec<AiToolExecutionResult>,
    },
    AiOpenTerminalRequest {
        mount: String,
        name: Option<String>,
        command: Option<String>,
        cols: u16,
        rows: u16,
        reply_tx: Sender<Result<String, String>>,
    },
    AiOpenEditorRequest {
        mount: String,
        path: String,
        line: Option<usize>,
        column: Option<usize>,
        reply_tx: Sender<Result<String, String>>,
    },
    AiObserveFilesystemRequest {
        mount: String,
        path: Option<String>,
        limit: usize,
        since_secs: u64,
        reply_tx: Sender<Result<String, String>>,
    },
    AiListTerminalsRequest {
        mount: String,
        reply_tx: Sender<Result<String, String>>,
    },
    AiReadTerminalRequest {
        mount: String,
        path: String,
        rows: Option<u16>,
        top_row: Option<usize>,
        reply_tx: Sender<Result<String, String>>,
    },
    AiSendTerminalTextRequest {
        mount: String,
        path: String,
        text: String,
        submit: Option<bool>,
        bracketed_paste: Option<bool>,
        reply_tx: Sender<Result<String, String>>,
    },
    AiSendTerminalKeyRequest {
        mount: String,
        path: String,
        key: String,
        shift: bool,
        control: bool,
        alt: bool,
        reply_tx: Sender<Result<String, String>>,
    },
    WorkerFindFilesDone {
        client_id: ClientId,
        query_id: QueryId,
        result: Result<Vec<String>, String>,
    },
    WorkerFindInFilesDone {
        client_id: ClientId,
        query_id: QueryId,
        result: Result<Vec<SearchResult>, String>,
    },
    WorkerQueryLogsDone {
        client_id: ClientId,
        query_id: QueryId,
        query: LogQuery,
        live: bool,
        entries: Vec<(usize, LogEntry)>,
    },
    WorkerLoadFileTreeDone {
        mount: String,
        result: Result<backend_proto::FileTreeData, String>,
    },
    WorkerFileTreeDeltaDone {
        mount: String,
        change: backend_proto::FileTreeChange,
    },
    FlushPendingFsEvents,
    FlushPendingFileTreeDiffs,
    MountFsChanged {
        mount: String,
        path: PathBuf,
    },
    SuppressMountRootFsEvents {
        mount: String,
        duration: Duration,
    },
    Shutdown,
}

const FS_EVENT_PATH_DEBOUNCE: Duration = Duration::from_millis(80);
const FS_EVENT_BATCH_FLUSH_DELAY: Duration = Duration::from_millis(80);
const FS_EVENT_BATCH_RELOAD_THRESHOLD: usize = 256;
const FS_EVENT_RELOAD_DEBOUNCE: Duration = Duration::from_millis(120);
const FS_EVENT_HISTORY_PRUNE_INTERVAL: Duration = Duration::from_secs(4);
const FS_EVENT_HISTORY_RETENTION: Duration = Duration::from_secs(12);
const FS_RECENT_CHANGE_RETENTION: Duration = Duration::from_secs(300);
const FS_DELTA_FLUSH_DELAY: Duration = Duration::from_millis(32);
const FS_DELTA_RELOAD_THRESHOLD: usize = 768;
const FS_SELF_SAVE_SUPPRESS: Duration = Duration::from_millis(300);
const AI_TERMINAL_SUBMIT_DELAY: Duration = Duration::from_millis(60);
const GIT_STATUS_CACHE_TTL: Duration = Duration::from_millis(250);
const IN_PROCESS_UI_WEB_SOCKET_ID: u64 = 0;
const MAX_UI_CLIENT_IDS: usize = backend_proto::QUERY_ID_CLIENT_LANES as usize;

fn studio_hub_debug_enabled() -> bool {
    env::var_os("MAKEPAD_STUDIO_HUB_DEBUG").is_some()
}

fn schedule_fs_event_flush(event_tx: Sender<HubEvent>) {
    std::thread::spawn(move || {
        std::thread::sleep(FS_EVENT_BATCH_FLUSH_DELAY);
        let _ = event_tx.send(HubEvent::FlushPendingFsEvents);
    });
}

struct UiClient {
    sender: ToUISender<Vec<u8>>,
    typed_sender: Option<ToUISender<HubToClient>>,
    format: WireFormat,
}

struct AppSocket {
    build_id: Option<QueryId>,
    crate_name: Option<String>,
    sender: Sender<Vec<u8>>,
    mount: Option<String>,
    package: Option<String>,
}

struct BuildBoxSocket {
    sender: Sender<Vec<u8>>,
    info: Option<BuildBoxInfo>,
    tree_hash: Option<String>,
}

struct LiveLogSubscription {
    client_id: ClientId,
    query: LogQuery,
}

struct LiveProfilerSubscription {
    client_id: ClientId,
    query: ProfilerQuery,
}

#[derive(Clone, Debug)]
struct TerminalClientViewport {
    cols: u16,
    rows: u16,
    top_row: usize,
    anchor: TerminalViewportAnchor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TerminalViewportAnchor {
    Bottom,
    TopRow,
}

struct TerminalSession {
    terminal: Terminal,
    cols: u16,
    rows: u16,
    applied_cols: u16,
    applied_rows: u16,
    frame_seq: u64,
    bell_pending: bool,
    subscribers: HashMap<ClientId, TerminalClientViewport>,
}

#[derive(SerJson)]
struct AiTerminalInfo {
    path: String,
    name: String,
    terminal_title: String,
    mode: String,
    summary: String,
    is_codex: bool,
    codex_status: Option<String>,
    cols: u16,
    rows: u16,
    is_tui: bool,
    bracketed_paste: bool,
    cursor_keys_application_mode: bool,
    bell_pending: bool,
}

#[derive(SerJson)]
struct AiTerminalReadResult {
    path: String,
    name: String,
    terminal_title: String,
    cols: u16,
    rows: u16,
    top_row: usize,
    total_lines: usize,
    cursor_col: u16,
    cursor_row: i32,
    cursor_visible: bool,
    is_tui: bool,
    mode: String,
    summary: String,
    is_codex: bool,
    codex_status: Option<String>,
    bracketed_paste: bool,
    cursor_keys_application_mode: bool,
    text: String,
}

#[derive(SerJson)]
struct AiTerminalInputResult {
    path: String,
    name: String,
    bytes_sent: usize,
    submitted: bool,
    bracketed_paste: bool,
    preview: String,
}

#[derive(SerJson)]
struct AiFilesystemChange {
    path: String,
    kind: String,
    seconds_ago: f64,
}

#[derive(SerJson)]
struct AiFilesystemObserveResult {
    mount: String,
    path_filter: Option<String>,
    since_secs: u64,
    changes: Vec<AiFilesystemChange>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum AiTerminalKeyInput {
    Named(TermKeyCode),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AiParsedTerminalKeySpec {
    input: AiTerminalKeyInput,
    shift: bool,
    control: bool,
    alt: bool,
}

#[derive(Default)]
struct GitStatusCache {
    entries: HashMap<PathBuf, GitStatusCacheEntry>,
}

struct GitStatusCacheEntry {
    refreshed_at: Instant,
    status: backend_proto::GitStatus,
}

#[derive(Default)]
struct FsWatchEventBatch {
    events: Mutex<HashSet<(String, PathBuf)>>,
    flush_scheduled: AtomicBool,
}

impl FsWatchEventBatch {
    fn clear(&self) {
        if let Ok(mut events) = self.events.lock() {
            events.clear();
        }
        self.flush_scheduled.store(false, Ordering::Release);
    }

    fn push(&self, mount: String, path: PathBuf) -> bool {
        if let Ok(mut events) = self.events.lock() {
            events.insert((mount, path));
        }
        !self.flush_scheduled.swap(true, Ordering::AcqRel)
    }

    fn take_ready(&self) -> HashSet<(String, PathBuf)> {
        self.flush_scheduled.store(false, Ordering::Release);
        self.events
            .lock()
            .map(|mut events| std::mem::take(&mut *events))
            .unwrap_or_default()
    }
}

pub struct HubCore {
    rx: Receiver<HubEvent>,
    event_tx: Sender<HubEvent>,
    pub vfs: VirtualFs,
    studio_addr: Option<String>,
    studio_ext_addr: Option<String>,
    client_id_in_use: [bool; MAX_UI_CLIENT_IDS],
    next_build_id: u64,
    client_by_web_socket: HashMap<u64, ClientId>,
    ui_clients: HashMap<ClientId, UiClient>,
    app_sockets: HashMap<u64, AppSocket>,
    buildbox_sockets: HashMap<u64, BuildBoxSocket>,
    buildbox_by_name: HashMap<String, u64>,
    build_mount_by_id: HashMap<QueryId, String>,
    run_items_by_mount: HashMap<String, Vec<RunItem>>,
    primary_ui_by_mount: HashMap<String, ClientId>,
    remote_builds: HashMap<QueryId, BuildInfo>,
    remote_build_owner: HashMap<QueryId, String>,
    log_store: LogStore,
    profiler_store: ProfilerStore,
    build_manager: BuildManager,
    script_manager: ScriptManager,
    ai_manager: AiManager,
    terminal_manager: TerminalManager,
    terminal_sessions: HashMap<String, TerminalSession>,
    live_log_queries: HashMap<QueryId, LiveLogSubscription>,
    live_profiler_queries: HashMap<QueryId, LiveProfilerSubscription>,
    cancelled_queries: HashSet<QueryId>,
    worker_pool: WorkerPool,
    regex_search_pool: Arc<WorkerPool>,
    io_worker_pool: WorkerPool,
    git_status_cache: Arc<Mutex<GitStatusCache>>,
    fs_watcher: Option<FileSystemWatcher>,
    fs_watch_events: Arc<FsWatchEventBatch>,
    fs_event_last_by_path: HashMap<String, Instant>,
    fs_recent_change_at_by_path: HashMap<String, Instant>,
    fs_pending_diffs: HashMap<String, Vec<backend_proto::FileTreeChange>>,
    fs_pending_reload_mounts: HashSet<String>,
    pending_mount_root_splash_restarts: HashSet<String>,
    file_tree_load_waiters: HashMap<String, HashSet<ClientId>>,
    fs_diff_flush_scheduled: bool,
    fs_event_last_prune: Instant,
    mount_suppress_fs_until: HashMap<String, Instant>,
    self_save_suppress_until_by_path: HashMap<String, Instant>,
    pending_forward_to_app_by_build: HashMap<QueryId, Vec<Vec<u8>>>,
    stdio_ready_builds: HashSet<QueryId>,
}

#[path = "dispatch/ui.rs"]
mod ui;
#[path = "dispatch/fs.rs"]
mod fs_dispatch;
#[path = "dispatch/app.rs"]
mod app;
#[path = "dispatch/buildbox.rs"]
mod buildbox;
#[path = "dispatch/terminal.rs"]
mod terminal;
#[path = "dispatch/ai.rs"]
mod ai;

impl HubCore {
    pub fn new(
        rx: Receiver<HubEvent>,
        event_tx: Sender<HubEvent>,
        vfs: VirtualFs,
        studio_addr: Option<String>,
        studio_ext_addr: Option<String>,
    ) -> Self {
        let worker_count = std::thread::available_parallelism()
            .map(|v| v.get())
            .unwrap_or(4)
            .clamp(2, 16);
        let regex_search_worker_count = 8;
        let mut this = Self {
            rx,
            event_tx: event_tx.clone(),
            vfs,
            studio_addr,
            studio_ext_addr,
            client_id_in_use: [false; MAX_UI_CLIENT_IDS],
            next_build_id: 1,
            client_by_web_socket: HashMap::new(),
            ui_clients: HashMap::new(),
            app_sockets: HashMap::new(),
            buildbox_sockets: HashMap::new(),
            buildbox_by_name: HashMap::new(),
            build_mount_by_id: HashMap::new(),
            run_items_by_mount: HashMap::new(),
            primary_ui_by_mount: HashMap::new(),
            remote_builds: HashMap::new(),
            remote_build_owner: HashMap::new(),
            log_store: LogStore::default(),
            profiler_store: ProfilerStore::default(),
            build_manager: BuildManager::default(),
            script_manager: ScriptManager::default(),
            ai_manager: AiManager::new(event_tx.clone()),
            terminal_manager: TerminalManager::default(),
            terminal_sessions: HashMap::new(),
            live_log_queries: HashMap::new(),
            live_profiler_queries: HashMap::new(),
            cancelled_queries: HashSet::new(),
            worker_pool: WorkerPool::new(worker_count),
            regex_search_pool: Arc::new(WorkerPool::new(regex_search_worker_count)),
            io_worker_pool: WorkerPool::new(1),
            git_status_cache: Arc::new(Mutex::new(GitStatusCache::default())),
            fs_watcher: None,
            fs_watch_events: Arc::new(FsWatchEventBatch::default()),
            fs_event_last_by_path: HashMap::new(),
            fs_recent_change_at_by_path: HashMap::new(),
            fs_pending_diffs: HashMap::new(),
            fs_pending_reload_mounts: HashSet::new(),
            pending_mount_root_splash_restarts: HashSet::new(),
            file_tree_load_waiters: HashMap::new(),
            fs_diff_flush_scheduled: false,
            fs_event_last_prune: Instant::now(),
            mount_suppress_fs_until: HashMap::new(),
            self_save_suppress_until_by_path: HashMap::new(),
            pending_forward_to_app_by_build: HashMap::new(),
            stdio_ready_builds: HashSet::new(),
        };
        for mount in this.vfs.mounts() {
            this.ai_manager.register_mount(&mount.name, &mount.path);
        }
        this.reset_fs_watcher();
        this
    }

    pub fn run(&mut self) {
        while let Ok(event) = self.rx.recv() {
            if !self.handle_event(event) {
                break;
            }
        }
    }

    pub fn handle_event(&mut self, event: HubEvent) -> bool {
        match event {
            HubEvent::ClientConnected {
                web_socket_id,
                sender,
                typed_sender,
            } => self.on_ui_connected(web_socket_id, sender, typed_sender),
            HubEvent::ClientDisconnected { web_socket_id } => {
                if let Some(client_id) = self.client_by_web_socket.remove(&web_socket_id) {
                    if studio_hub_debug_enabled() {
                        eprintln!(
                            "studio hub debug: ui disconnect web_socket_id={} client_id={:?}",
                            web_socket_id, client_id
                        );
                    }
                    self.ui_clients.remove(&client_id);
                    self.release_client_id(client_id);
                    for session in self.terminal_sessions.values_mut() {
                        session.subscribers.remove(&client_id);
                    }
                    self.live_log_queries
                        .retain(|_, query| query.client_id != client_id);
                    self.live_profiler_queries
                        .retain(|_, query| query.client_id != client_id);
                    for waiters in self.file_tree_load_waiters.values_mut() {
                        waiters.remove(&client_id);
                    }
                    self.primary_ui_by_mount
                        .retain(|_, observer_id| *observer_id != client_id);
                }
            }
            HubEvent::ClientEnvelope {
                web_socket_id,
                envelope,
            } => {
                if let Some(&client_id) = self.client_by_web_socket.get(&web_socket_id) {
                    self.on_ui_envelope(client_id, envelope);
                }
            }
            HubEvent::ClientBinary {
                web_socket_id,
                data,
            } => {
                if let Some(&client_id) = self.client_by_web_socket.get(&web_socket_id) {
                    self.on_ui_message(client_id, WireFormat::Binary, &data);
                }
            }
            HubEvent::ClientText {
                web_socket_id,
                text,
            } => {
                if let Some(&client_id) = self.client_by_web_socket.get(&web_socket_id) {
                    self.on_ui_message(client_id, WireFormat::Text, text.as_bytes());
                }
            }
            HubEvent::AppConnected {
                web_socket_id,
                build_id,
                crate_name,
                sender,
            } => {
                let build_info = build_id.and_then(|build_id| self.build_info_for_id(build_id));
                self.app_sockets.insert(
                    web_socket_id,
                    AppSocket {
                        build_id,
                        crate_name: crate_name.clone(),
                        sender,
                        mount: build_info.as_ref().map(|info| info.mount.clone()),
                        package: build_info
                            .as_ref()
                            .map(|info| info.package.clone())
                            .or(crate_name),
                    },
                );
                if let Some(build_id) = build_id {
                    self.flush_pending_forward_to_app(build_id);
                }
            }
            HubEvent::AppDisconnected { web_socket_id } => {
                self.app_sockets.remove(&web_socket_id);
            }
            HubEvent::AppBinary {
                web_socket_id,
                data,
            } => {
                let Some(socket) = self.app_sockets.get(&web_socket_id) else {
                    return true;
                };
                if let Some(build_id) = socket.build_id {
                    self.on_app_binary(build_id, data);
                }
            }
            HubEvent::ProcessAppMessage { build_id, msg } => {
                self.on_process_app_message(build_id, msg)
            }
            HubEvent::BuildBoxConnected {
                web_socket_id,
                sender,
            } => {
                self.buildbox_sockets.insert(
                    web_socket_id,
                    BuildBoxSocket {
                        sender,
                        info: None,
                        tree_hash: None,
                    },
                );
            }
            HubEvent::BuildBoxDisconnected { web_socket_id } => {
                self.on_buildbox_disconnected(web_socket_id);
            }
            HubEvent::BuildBoxBinary {
                web_socket_id,
                data,
            } => {
                if self.buildbox_sockets.contains_key(&web_socket_id) {
                    self.on_buildbox_binary(web_socket_id, data);
                }
            }
            HubEvent::ProcessOutput {
                build_id,
                is_stderr,
                line,
            } => self.on_process_output(build_id, is_stderr, line),
            HubEvent::ProcessExited {
                build_id,
                exit_code,
            } => self.on_process_exited(build_id, exit_code),
            HubEvent::RunItemsUpdated { mount, items } => self.on_run_items_updated(mount, items),
            HubEvent::ScriptRunRequest {
                child_build_id,
                mount,
                cwd,
                program,
                args,
                env,
                package,
            } => {
                self.on_script_run_request(child_build_id, mount, cwd, program, args, env, package)
            }
            HubEvent::ScriptOutput {
                script_id,
                mount,
                is_stderr,
                line,
            } => self.on_script_output(script_id, mount, is_stderr, line),
            HubEvent::ScriptExited {
                script_id,
                mount,
                exit_code,
            } => self.on_script_exited(script_id, mount, exit_code),
            HubEvent::TerminalOutput { path, data } => self.on_terminal_output(path, data),
            HubEvent::TerminalResized { path, cols, rows } => {
                self.on_terminal_resized(path, cols, rows)
            }
            HubEvent::TerminalExited { path, exit_code } => {
                self.on_terminal_exited(path, exit_code)
            }
            HubEvent::AiHttpResponse { response } => {
                if let Some((mount, state)) = self.ai_manager.handle_http_response(response) {
                    self.broadcast_ui_message(HubToClient::AiMountState { mount, state });
                }
            }
            HubEvent::AiChatGptOAuthCode {
                mount,
                backend_id,
                code,
            } => {
                let state = self
                    .ai_manager
                    .handle_chatgpt_oauth_code(&mount, &backend_id, &code);
                self.broadcast_ui_message(HubToClient::AiMountState { mount, state });
            }
            HubEvent::AiToolExecutionDone {
                mount,
                agent_id,
                run_token,
                results,
            } => {
                if let Some(state) = self
                    .ai_manager
                    .handle_tool_execution_done(&mount, agent_id, run_token, results)
                {
                    self.broadcast_ui_message(HubToClient::AiMountState { mount, state });
                }
            }
            HubEvent::AiOpenTerminalRequest {
                mount,
                name,
                command,
                cols,
                rows,
                reply_tx,
            } => self.on_ai_open_terminal_request(mount, name, command, cols, rows, reply_tx),
            HubEvent::AiOpenEditorRequest {
                mount,
                path,
                line,
                column,
                reply_tx,
            } => self.on_ai_open_editor_request(mount, path, line, column, reply_tx),
            HubEvent::AiObserveFilesystemRequest {
                mount,
                path,
                limit,
                since_secs,
                reply_tx,
            } => self.on_ai_observe_filesystem_request(mount, path, limit, since_secs, reply_tx),
            HubEvent::AiListTerminalsRequest { mount, reply_tx } => {
                self.on_ai_list_terminals_request(mount, reply_tx)
            }
            HubEvent::AiReadTerminalRequest {
                mount,
                path,
                rows,
                top_row,
                reply_tx,
            } => self.on_ai_read_terminal_request(mount, path, rows, top_row, reply_tx),
            HubEvent::AiSendTerminalTextRequest {
                mount,
                path,
                text,
                submit,
                bracketed_paste,
                reply_tx,
            } => self.on_ai_send_terminal_text_request(
                mount,
                path,
                text,
                submit,
                bracketed_paste,
                reply_tx,
            ),
            HubEvent::AiSendTerminalKeyRequest {
                mount,
                path,
                key,
                shift,
                control,
                alt,
                reply_tx,
            } => self
                .on_ai_send_terminal_key_request(mount, path, key, shift, control, alt, reply_tx),
            HubEvent::WorkerFindFilesDone {
                client_id,
                query_id,
                result,
            } => self.on_worker_find_files_done(client_id, query_id, result),
            HubEvent::WorkerFindInFilesDone {
                client_id,
                query_id,
                result,
            } => self.on_worker_find_in_files_done(client_id, query_id, result),
            HubEvent::WorkerQueryLogsDone {
                client_id,
                query_id,
                query,
                live,
                entries,
            } => self.on_worker_query_logs_done(client_id, query_id, query, live, entries),
            HubEvent::WorkerLoadFileTreeDone { mount, result } => {
                self.on_worker_load_file_tree_done(mount, result)
            }
            HubEvent::WorkerFileTreeDeltaDone { mount, change } => {
                self.queue_file_tree_delta_change(mount, change);
            }
            HubEvent::FlushPendingFsEvents => self.flush_pending_mount_fs_events(),
            HubEvent::FlushPendingFileTreeDiffs => self.flush_pending_file_tree_diffs(),
            HubEvent::MountFsChanged { mount, path } => self.queue_mount_fs_changed(mount, path),
            HubEvent::SuppressMountRootFsEvents { mount, duration } => {
                self.suppress_mount_root_fs_events(&mount, duration)
            }
            HubEvent::Shutdown => return false,
        }
        true
    }
}

#[derive(Clone, Debug, Default, DeJson)]
struct RustcCompilerMessage {
    reason: String,
    message: Option<RustcMessage>,
}

#[derive(Clone, Debug, Default, DeJson)]
struct RustcMessage {
    message: String,
    level: String,
    spans: Vec<RustcSpan>,
    rendered: Option<String>,
}

#[derive(Clone, Debug, Default, DeJson)]
struct RustcSpan {
    file_name: String,
    line_start: Option<usize>,
    column_start: Option<usize>,
    is_primary: Option<bool>,
}

enum ParsedCargoOutputLine {
    Structured(ParsedCargoLogEntry),
    IgnoredStructured,
    RawText,
}

struct ParsedCargoLogEntry {
    level: LogLevel,
    message: String,
    file_name: Option<String>,
    line: Option<usize>,
    column: Option<usize>,
}

fn display_name_from_command(program: &str, args: &[String]) -> String {
    if program == "cargo" {
        if let Some(package) = parse_package_name(args) {
            return package;
        }
    }
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(program)
        .to_string()
}

fn terminal_framebuffer_from_terminal(
    terminal: &Terminal,
    cols: u16,
    rows: u16,
    requested_top_row: usize,
    frame_id: u64,
) -> TerminalFramebuffer {
    let cols = cols.max(1);
    let rows = rows.max(1);
    let cols_usize = cols as usize;
    let rows_usize = rows as usize;
    let screen = terminal.screen();
    let is_tui = screen.scroll_top != 0
        || screen.scroll_bottom != screen.rows()
        || terminal.modes.alt_screen;

    let total_lines = if is_tui {
        screen.scrollback_len() + screen.rows()
    } else {
        screen.scrollback_len() + screen.used_rows()
    };
    let max_top = total_lines.saturating_sub(rows_usize);
    let top_row = requested_top_row.min(max_top);
    let mut cells = Vec::with_capacity(cols_usize * rows_usize * 10);
    let palette = &terminal.palette.colors;
    let default_fg = terminal.default_fg;
    let default_bg = terminal.default_bg;
    for row in 0..rows_usize {
        let virtual_row = top_row + row;
        let row_slice = screen.row_slice_virtual(virtual_row);
        for col in 0..cols_usize {
            let (codepoint, fg, bg) = if let Some(cell) = row_slice.and_then(|slice| slice.get(col))
            {
                let mut fg_src = cell.style.fg;
                let mut bg_src = cell.style.bg;
                if cell.style.flags.has(StyleFlags::INVERSE) {
                    std::mem::swap(&mut fg_src, &mut bg_src);
                }
                let fg = fg_src.resolve(palette, default_fg);
                let bg = bg_src.resolve(palette, default_bg);
                // Preserve raw terminal codepoints so clients can distinguish
                // placeholder/continuation cells (e.g. '\0') during copy.
                let codepoint = cell.codepoint as u32;
                (codepoint, fg, bg)
            } else {
                (' ' as u32, default_fg, default_bg)
            };
            cells.extend_from_slice(&codepoint.to_le_bytes());
            cells.push(fg.r);
            cells.push(fg.g);
            cells.push(fg.b);
            cells.push(bg.r);
            cells.push(bg.g);
            cells.push(bg.b);
        }
    }

    let cursor_virtual_row = screen.scrollback_len().saturating_add(terminal.cursor().y);
    let cursor_row = cursor_virtual_row as isize - top_row as isize;
    let cursor_visible =
        terminal.modes.cursor_visible && cursor_row >= 0 && cursor_row < rows_usize as isize;

    TerminalFramebuffer {
        frame_id,
        cols,
        rows,
        top_row,
        total_lines,
        cursor_col: terminal.cursor().x as u16,
        cursor_row: if cursor_visible {
            cursor_row as i32
        } else {
            -1
        },
        cursor_visible,
        default_fg_rgb: rgb_to_u32(default_fg.r, default_fg.g, default_fg.b),
        default_bg_rgb: rgb_to_u32(default_bg.r, default_bg.g, default_bg.b),
        bracketed_paste: terminal.modes.bracketed_paste,
        cursor_keys_application_mode: terminal.modes.cursor_keys,
        is_tui,
        cells,
    }
}

fn terminal_framebuffer_text(frame: &TerminalFramebuffer) -> String {
    let cols = frame.cols as usize;
    let rows = frame.rows as usize;
    let cell_count = cols.saturating_mul(rows);
    let stride = if cell_count == 0 {
        0
    } else {
        (frame.cells.len() / cell_count).max(4)
    };
    let mut out = String::new();
    for row in 0..rows {
        let mut line = String::with_capacity(cols);
        for col in 0..cols {
            let idx = (row * cols + col) * stride;
            let codepoint = if idx + 3 < frame.cells.len() {
                u32::from_le_bytes([
                    frame.cells[idx],
                    frame.cells[idx + 1],
                    frame.cells[idx + 2],
                    frame.cells[idx + 3],
                ])
            } else {
                ' ' as u32
            };
            line.push(match codepoint {
                0 => ' ',
                value => char::from_u32(value).unwrap_or(' '),
            });
        }
        out.push_str(line.trim_end_matches(' '));
        if row + 1 < rows {
            out.push('\n');
        }
    }
    out
}

fn preview_text(text: &str) -> String {
    let normalized = text.replace('\r', "\\r").replace('\n', "\\n");
    let mut out = normalized.chars().take(160).collect::<String>();
    if normalized.chars().count() > 160 {
        out.push_str("...");
    }
    out
}

fn parse_ai_terminal_key_spec(
    key: &str,
    shift: bool,
    control: bool,
    alt: bool,
) -> Result<AiParsedTerminalKeySpec, String> {
    let mut shift = shift;
    let mut control = control;
    let mut alt = alt;
    let raw = key.trim();
    if raw.is_empty() {
        return Err("terminal key cannot be empty".to_string());
    }

    let parts = raw
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return Err("terminal key cannot be empty".to_string());
    }
    let base = parts.last().copied().unwrap_or(raw);
    for modifier in &parts[..parts.len().saturating_sub(1)] {
        match modifier.to_ascii_lowercase().as_str() {
            "shift" => shift = true,
            "ctrl" | "control" => control = true,
            "alt" | "option" => alt = true,
            other => return Err(format!("unsupported terminal key modifier '{}'", other)),
        }
    }

    let input = match base.to_ascii_lowercase().as_str() {
        "enter" | "return" => AiTerminalKeyInput::Named(TermKeyCode::Return),
        "tab" => AiTerminalKeyInput::Named(TermKeyCode::Tab),
        "backspace" | "bs" => AiTerminalKeyInput::Named(TermKeyCode::Backspace),
        "escape" | "esc" => AiTerminalKeyInput::Named(TermKeyCode::Escape),
        "delete" | "del" => AiTerminalKeyInput::Named(TermKeyCode::Delete),
        "up" | "arrowup" => AiTerminalKeyInput::Named(TermKeyCode::Up),
        "down" | "arrowdown" => AiTerminalKeyInput::Named(TermKeyCode::Down),
        "left" | "arrowleft" => AiTerminalKeyInput::Named(TermKeyCode::Left),
        "right" | "arrowright" => AiTerminalKeyInput::Named(TermKeyCode::Right),
        "home" => AiTerminalKeyInput::Named(TermKeyCode::Home),
        "end" => AiTerminalKeyInput::Named(TermKeyCode::End),
        "pageup" | "page_up" | "pgup" => AiTerminalKeyInput::Named(TermKeyCode::PageUp),
        "pagedown" | "page_down" | "pgdown" => AiTerminalKeyInput::Named(TermKeyCode::PageDown),
        "insert" | "ins" => AiTerminalKeyInput::Named(TermKeyCode::Insert),
        "f1" => AiTerminalKeyInput::Named(TermKeyCode::F1),
        "f2" => AiTerminalKeyInput::Named(TermKeyCode::F2),
        "f3" => AiTerminalKeyInput::Named(TermKeyCode::F3),
        "f4" => AiTerminalKeyInput::Named(TermKeyCode::F4),
        "f5" => AiTerminalKeyInput::Named(TermKeyCode::F5),
        "f6" => AiTerminalKeyInput::Named(TermKeyCode::F6),
        "f7" => AiTerminalKeyInput::Named(TermKeyCode::F7),
        "f8" => AiTerminalKeyInput::Named(TermKeyCode::F8),
        "f9" => AiTerminalKeyInput::Named(TermKeyCode::F9),
        "f10" => AiTerminalKeyInput::Named(TermKeyCode::F10),
        "f11" => AiTerminalKeyInput::Named(TermKeyCode::F11),
        "f12" => AiTerminalKeyInput::Named(TermKeyCode::F12),
        "space" => AiTerminalKeyInput::Text(" ".to_string()),
        _ => {
            if base.chars().count() == 1 {
                AiTerminalKeyInput::Text(base.to_string())
            } else {
                return Err(format!("unsupported terminal key '{}'", base));
            }
        }
    };

    Ok(AiParsedTerminalKeySpec {
        input,
        shift,
        control,
        alt,
    })
}

fn encode_ai_terminal_key(terminal: &Terminal, spec: &AiParsedTerminalKeySpec) -> Option<Vec<u8>> {
    match &spec.input {
        AiTerminalKeyInput::Named(key_code) => {
            terminal.encode_key(*key_code, "", spec.shift, spec.control, spec.alt)
        }
        AiTerminalKeyInput::Text(text) => {
            terminal.encode_key(TermKeyCode::None, text, spec.shift, spec.control, spec.alt)
        }
    }
}

fn rgb_to_u32(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

fn sanitize_terminal_stem(raw: &str) -> Option<String> {
    let mut stem = String::new();
    let mut last_was_dash = false;
    for ch in raw.trim().chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_alphanumeric() {
            stem.push(ch);
            last_was_dash = false;
        } else if !stem.is_empty() && matches!(ch, '-' | '_' | ' ' | '.') && !last_was_dash {
            stem.push('-');
            last_was_dash = true;
        }
    }
    while stem.ends_with('-') {
        stem.pop();
    }
    (!stem.is_empty()).then_some(stem)
}

fn terminal_stem_from_command(command: &str) -> Option<String> {
    let token = command.split_whitespace().next()?;
    let token = token.rsplit('/').next().unwrap_or(token);
    sanitize_terminal_stem(token)
}

fn mount_from_virtual_path(path: &str) -> Option<&str> {
    path.split('/').next().filter(|part| !part.is_empty())
}

fn append_terminal_history_bytes(vfs: &VirtualFs, path: &str, data: &[u8]) -> Result<(), String> {
    let disk_path = vfs
        .resolve_path(path)
        .map_err(|err| format!("failed to resolve terminal path {}: {}", path, err))?;
    if let Some(parent) = disk_path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "failed to create terminal history directory {}: {}",
                parent.display(),
                err
            )
        })?;
    }
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&disk_path)
        .map_err(|err| {
            format!(
                "failed to open terminal history {}: {}",
                disk_path.display(),
                err
            )
        })?;
    file.write_all(data).map_err(|err| {
        format!(
            "failed to append terminal history {}: {}",
            disk_path.display(),
            err
        )
    })
}

fn map_platform_log_level(level: LogLevel) -> LogLevel {
    match level {
        LogLevel::Error | LogLevel::Panic => LogLevel::Error,
        LogLevel::Warning | LogLevel::Wait => LogLevel::Warning,
        LogLevel::Log => LogLevel::Log,
    }
}

fn map_platform_event_sample(sample: EventSample) -> HubEventSample {
    HubEventSample {
        at: sample.end,
        label: LiveId(sample.event_u32 as u64),
        event_u32: sample.event_u32,
        event_meta: sample.event_meta,
        start: sample.start,
        end: sample.end,
    }
}

fn map_platform_gpu_sample(sample: GPUSample) -> StudioGPUSample {
    StudioGPUSample {
        at: sample.end,
        label: LiveId(0),
        start: sample.start,
        end: sample.end,
        draw_calls: sample.draw_calls,
        instances: sample.instances,
        vertices: sample.vertices,
        instance_bytes: sample.instance_bytes,
        uniform_bytes: sample.uniform_bytes,
        vertex_buffer_bytes: sample.vertex_buffer_bytes,
        texture_bytes: sample.texture_bytes,
    }
}

fn map_platform_gc_sample(sample: GCSample) -> StudioGCSample {
    StudioGCSample {
        at: sample.end,
        label: LiveId(0),
        start: sample.start,
        end: sample.end,
        heap_live: sample.heap_live,
    }
}

fn classify_cargo_log_line(is_stderr: bool, line: &str) -> LogLevel {
    let lower = line.to_ascii_lowercase();
    if lower.contains("error") {
        return LogLevel::Error;
    }
    if lower.contains("warning") {
        return LogLevel::Warning;
    }
    let _ = is_stderr;
    LogLevel::Log
}

fn parse_cargo_output_line(line: &str) -> ParsedCargoOutputLine {
    let Ok(msg) = RustcCompilerMessage::deserialize_json_lenient(line) else {
        return ParsedCargoOutputLine::RawText;
    };
    match msg.reason.as_str() {
        "compiler-message" | "makepad-error-log" => {}
        _ => return ParsedCargoOutputLine::IgnoredStructured,
    }
    let Some(message) = msg.message else {
        return ParsedCargoOutputLine::IgnoredStructured;
    };
    let level = rustc_level_to_log_level(&message.level);
    if matches!(level, LogLevel::Warning)
        && message
            .message
            .starts_with("unstable feature specified for")
    {
        return ParsedCargoOutputLine::IgnoredStructured;
    }

    if let Some(span) = message
        .spans
        .iter()
        .find(|span| span.is_primary.unwrap_or(false))
    {
        let file_name = if span.file_name.is_empty() {
            None
        } else {
            Some(span.file_name.replace('\\', "/"))
        };
        return ParsedCargoOutputLine::Structured(ParsedCargoLogEntry {
            level,
            message: message.message,
            file_name,
            line: span.line_start.filter(|line| *line > 0),
            column: span.column_start.filter(|column| *column > 0),
        });
    }

    let trimmed = message.message.trim();
    if trimmed.starts_with("Some errors have detailed explanations")
        || trimmed.starts_with("For more information about an error")
        || trimmed.contains("warnings emitted")
        || trimmed.contains("warning emitted")
    {
        return ParsedCargoOutputLine::IgnoredStructured;
    }
    let fallback_text = message.rendered.unwrap_or_else(|| message.message);
    ParsedCargoOutputLine::Structured(ParsedCargoLogEntry {
        level,
        message: fallback_text,
        file_name: None,
        line: None,
        column: None,
    })
}

fn rustc_level_to_log_level(level: &str) -> LogLevel {
    match level {
        "error" | "failure-note" | "panic" => LogLevel::Error,
        "warning" => LogLevel::Warning,
        // rustc may emit "note" / "help" / "log"
        _ => LogLevel::Log,
    }
}

fn build_run_cargo_args(process: &str, mut app_args: Vec<String>, standalone: bool) -> Vec<String> {
    if !has_message_format_json_arg(&app_args) {
        app_args.insert(0, "--message-format=json".to_string());
    }
    if standalone {
        app_args.retain(|arg| arg != "--stdin-loop");
    } else if !app_args.iter().any(|arg| arg == "--stdin-loop") {
        app_args.push("--stdin-loop".to_string());
    }

    let mut args = vec![
        "run".to_string(),
        "-p".to_string(),
        process.to_string(),
        "--release".to_string(),
        "--message-format=json".to_string(),
    ];
    args.push("--".to_string());
    args.extend(app_args);
    args
}

fn with_default_cargo_message_format(mut args: Vec<String>) -> Vec<String> {
    if has_message_format_json_arg(&args) {
        return args;
    }
    if cargo_subcommand_supports_message_format(&args) {
        args.push("--message-format=json".to_string());
    }
    args
}

fn cargo_subcommand_supports_message_format(args: &[String]) -> bool {
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--" {
            break;
        }
        if arg.starts_with('+') {
            continue;
        }
        if arg == "--config"
            || arg == "-Z"
            || arg == "--color"
            || arg == "--manifest-path"
            || arg == "--target-dir"
        {
            if !arg.contains('=') && iter.peek().is_some_and(|next| !next.starts_with('-')) {
                iter.next();
            }
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        return matches!(
            arg.as_str(),
            "build" | "check" | "run" | "test" | "bench" | "rustc"
        );
    }
    false
}

fn has_message_format_json_arg(args: &[String]) -> bool {
    args.iter().any(|arg| {
        arg == "--message-format=json"
            || arg == "--message-format"
            || arg.starts_with("--message-format=")
    })
}

fn parse_package_name(args: &[String]) -> Option<String> {
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

fn file_tree_change_path(change: &backend_proto::FileTreeChange) -> &str {
    match change {
        backend_proto::FileTreeChange::Added { path, .. } => path,
        backend_proto::FileTreeChange::Removed { path } => path,
        backend_proto::FileTreeChange::Modified { path, .. } => path,
    }
}

fn path_is_child_of(parent: &str, child: &str) -> bool {
    child.len() > parent.len()
        && child.starts_with(parent)
        && child.as_bytes().get(parent.len()) == Some(&b'/')
}

fn coalesce_file_tree_change(
    changes: &mut Vec<backend_proto::FileTreeChange>,
    change: backend_proto::FileTreeChange,
) {
    match &change {
        backend_proto::FileTreeChange::Removed { path } => {
            if changes.iter().any(|existing| {
                matches!(
                    existing,
                    backend_proto::FileTreeChange::Removed { path: existing_path }
                        if existing_path == path || path_is_child_of(existing_path, path)
                )
            }) {
                return;
            }
            changes.retain(|existing| {
                let existing_path = file_tree_change_path(existing);
                existing_path != path && !path_is_child_of(path, existing_path)
            });
            changes.push(change);
        }
        backend_proto::FileTreeChange::Added { path, .. } => {
            // If the path reappears after a remove event, keep the fresh "Added" state.
            changes.retain(|existing| {
                !matches!(
                    existing,
                    backend_proto::FileTreeChange::Removed { path: removed_path }
                        if removed_path == path || path_is_child_of(removed_path, path)
                )
            });
            if let Some(index) = changes
                .iter()
                .position(|existing| file_tree_change_path(existing) == path)
            {
                changes.remove(index);
            }
            changes.push(change);
        }
        backend_proto::FileTreeChange::Modified { path, git_status } => {
            changes.retain(|existing| {
                !matches!(
                    existing,
                    backend_proto::FileTreeChange::Removed { path: removed_path }
                        if removed_path == path || path_is_child_of(removed_path, path)
                )
            });
            if let Some(existing) = changes
                .iter_mut()
                .find(|existing| file_tree_change_path(existing) == path)
            {
                match existing {
                    backend_proto::FileTreeChange::Added {
                        git_status: status, ..
                    } => {
                        *status = *git_status;
                    }
                    backend_proto::FileTreeChange::Removed { .. } => {}
                    backend_proto::FileTreeChange::Modified {
                        git_status: status, ..
                    } => {
                        *status = *git_status;
                    }
                }
                return;
            }
            changes.push(change);
        }
    }
}

fn compute_filetree_change_for_path(
    git_status_cache: &Arc<Mutex<GitStatusCache>>,
    abs_path: &Path,
    virtual_path: String,
) -> backend_proto::FileTreeChange {
    match fs::metadata(abs_path) {
        Ok(meta) => {
            let node_type = if meta.is_dir() {
                backend_proto::FileNodeType::Dir
            } else {
                backend_proto::FileNodeType::File
            };
            backend_proto::FileTreeChange::Added {
                path: virtual_path,
                node_type,
                git_status: git_status_for_path_cached(git_status_cache, abs_path),
            }
        }
        Err(_) => backend_proto::FileTreeChange::Removed { path: virtual_path },
    }
}

fn git_status_for_path_cached(
    cache: &Arc<Mutex<GitStatusCache>>,
    path: &Path,
) -> backend_proto::GitStatus {
    let cache_key = path.to_path_buf();
    let now = Instant::now();
    if let Ok(cache_guard) = cache.lock() {
        if let Some(entry) = cache_guard.entries.get(&cache_key) {
            if now.saturating_duration_since(entry.refreshed_at) <= GIT_STATUS_CACHE_TTL {
                return entry.status;
            }
        }
    }

    let status = compute_git_status_for_path(path);
    if let Ok(mut cache_guard) = cache.lock() {
        cache_guard.entries.insert(
            cache_key,
            GitStatusCacheEntry {
                refreshed_at: now,
                status,
            },
        );
    }
    status
}

fn compute_git_status_for_path(path: &Path) -> backend_proto::GitStatus {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let Ok(mut repo) = GitRepository::open(&canonical) else {
        return backend_proto::GitStatus::Unknown;
    };
    let rel = match canonical.strip_prefix(&repo.workdir) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => return backend_proto::GitStatus::Unknown,
    };
    if rel.is_empty() {
        return backend_proto::GitStatus::Clean;
    }
    match repo.status_for_path_for_file_tree(&rel) {
        Ok(Some(status)) => git_status_from_file_status(status),
        Ok(None) => backend_proto::GitStatus::Clean,
        Err(_) => backend_proto::GitStatus::Unknown,
    }
}

fn git_status_from_file_status(status: GitFileStatus) -> backend_proto::GitStatus {
    match status {
        GitFileStatus::Modified => backend_proto::GitStatus::Modified,
        GitFileStatus::Deleted => backend_proto::GitStatus::Deleted,
        GitFileStatus::Untracked => backend_proto::GitStatus::Untracked,
        GitFileStatus::Staged => backend_proto::GitStatus::Staged,
        GitFileStatus::StagedDeleted => backend_proto::GitStatus::Deleted,
        GitFileStatus::StagedNew => backend_proto::GitStatus::Added,
    }
}

fn percent_encode_local(input: &str) -> String {
    let mut out = String::new();
    for b in input.bytes() {
        let safe = b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.';
        if safe {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(hex_local((b >> 4) & 0x0F));
            out.push(hex_local(b & 0x0F));
        }
    }
    out
}

#[cfg(target_os = "macos")]
fn normalize_macos_private_alias(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix("/private/") {
        PathBuf::from(format!("/{}", rest))
    } else {
        path.to_path_buf()
    }
}

fn hex_local(v: u8) -> char {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    HEX[v as usize] as char
}

fn file_tree_diff(
    before: &backend_proto::FileTreeData,
    after: &backend_proto::FileTreeData,
) -> Vec<backend_proto::FileTreeChange> {
    let mut before_by_path = HashMap::new();
    for node in &before.nodes {
        before_by_path.insert(node.path.as_str(), (&node.node_type, node.git_status));
    }
    let mut after_by_path = HashMap::new();
    for node in &after.nodes {
        after_by_path.insert(node.path.as_str(), (&node.node_type, node.git_status));
    }

    let mut changes = Vec::new();
    for node in &before.nodes {
        if !after_by_path.contains_key(node.path.as_str()) {
            changes.push(backend_proto::FileTreeChange::Removed {
                path: node.path.clone(),
            });
        }
    }
    for node in &after.nodes {
        match before_by_path.get(node.path.as_str()) {
            None => changes.push(backend_proto::FileTreeChange::Added {
                path: node.path.clone(),
                node_type: node.node_type.clone(),
                git_status: node.git_status,
            }),
            Some((_, before_status)) if *before_status != node.git_status => {
                changes.push(backend_proto::FileTreeChange::Modified {
                    path: node.path.clone(),
                    git_status: node.git_status,
                });
            }
            Some(_) => {}
        }
    }

    changes.sort_by(|a, b| {
        let a_path = match a {
            backend_proto::FileTreeChange::Added { path, .. } => path,
            backend_proto::FileTreeChange::Removed { path } => path,
            backend_proto::FileTreeChange::Modified { path, .. } => path,
        };
        let b_path = match b {
            backend_proto::FileTreeChange::Added { path, .. } => path,
            backend_proto::FileTreeChange::Removed { path } => path,
            backend_proto::FileTreeChange::Modified { path, .. } => path,
        };
        a_path.cmp(b_path)
    });
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_script_std::makepad_network::ToUIReceiver;
    use std::sync::mpsc;

    #[test]
    fn parse_cargo_output_line_extracts_primary_span() {
        let line = r#"{"reason":"compiler-message","message":{"message":"cannot find value `x` in this scope","level":"error","spans":[{"file_name":"src/main.rs","line_start":7,"column_start":13,"is_primary":true}],"rendered":"rendered text"}}"#;
        let parsed = parse_cargo_output_line(line);
        let ParsedCargoOutputLine::Structured(parsed) = parsed else {
            panic!("expected structured parsed output");
        };
        assert!(matches!(parsed.level, LogLevel::Error));
        assert_eq!(parsed.message, "cannot find value `x` in this scope");
        assert_eq!(parsed.file_name.as_deref(), Some("src/main.rs"));
        assert_eq!(parsed.line, Some(7));
        assert_eq!(parsed.column, Some(13));
    }

    #[test]
    fn parse_cargo_output_line_ignores_non_diagnostic_json() {
        let line = r#"{"reason":"compiler-artifact","package_id":"demo 0.1.0"}"#;
        let parsed = parse_cargo_output_line(line);
        assert!(matches!(parsed, ParsedCargoOutputLine::IgnoredStructured));
    }

    #[test]
    fn parse_cargo_output_line_falls_back_for_raw_text() {
        let line = "Compiling makepad-studio-backend v0.1.0";
        let parsed = parse_cargo_output_line(line);
        assert!(matches!(parsed, ParsedCargoOutputLine::RawText));
    }

    #[test]
    fn classify_cargo_progress_stderr_as_log() {
        let level = classify_cargo_log_line(true, "Compiling makepad-studio-backend v0.1.0");
        assert!(matches!(level, LogLevel::Log));
    }

    #[test]
    fn classify_cargo_warning_and_error_text() {
        let warning = classify_cargo_log_line(true, "warning: unused import: `foo`");
        let error = classify_cargo_log_line(false, "error: could not compile `demo`");
        assert!(matches!(warning, LogLevel::Warning));
        assert!(matches!(error, LogLevel::Error));
    }

    #[test]
    fn build_run_cargo_args_defaults_to_release_and_stdin_loop() {
        let normalized = build_run_cargo_args("makepad-example-splash", Vec::new(), false);
        assert_eq!(
            normalized,
            vec![
                "run".to_string(),
                "-p".to_string(),
                "makepad-example-splash".to_string(),
                "--release".to_string(),
                "--message-format=json".to_string(),
                "--".to_string(),
                "--message-format=json".to_string(),
                "--stdin-loop".to_string(),
            ]
        );
    }

    #[test]
    fn build_run_cargo_args_honors_standalone() {
        let app_args = vec![
            "--foo".to_string(),
            "bar".to_string(),
            "--stdin-loop".to_string(),
        ];
        let normalized = build_run_cargo_args("makepad-example-splash", app_args, true);
        assert_eq!(
            normalized,
            vec![
                "run".to_string(),
                "-p".to_string(),
                "makepad-example-splash".to_string(),
                "--release".to_string(),
                "--message-format=json".to_string(),
                "--".to_string(),
                "--message-format=json".to_string(),
                "--foo".to_string(),
                "bar".to_string(),
            ]
        );
    }

    #[test]
    fn build_run_cargo_args_keeps_message_format_if_provided() {
        let app_args = vec![
            "--message-format=json".to_string(),
            "--stdin-loop".to_string(),
        ];
        let normalized = build_run_cargo_args("makepad-example-splash", app_args, false);
        assert_eq!(
            normalized,
            vec![
                "run".to_string(),
                "-p".to_string(),
                "makepad-example-splash".to_string(),
                "--release".to_string(),
                "--message-format=json".to_string(),
                "--".to_string(),
                "--message-format=json".to_string(),
                "--stdin-loop".to_string(),
            ]
        );
    }

    #[test]
    fn with_default_cargo_message_format_injects_for_supported_subcommands() {
        let args = vec![
            "check".to_string(),
            "-p".to_string(),
            "makepad-example-splash".to_string(),
        ];
        let normalized = with_default_cargo_message_format(args);
        assert_eq!(
            normalized,
            vec![
                "check".to_string(),
                "-p".to_string(),
                "makepad-example-splash".to_string(),
                "--message-format=json".to_string(),
            ]
        );
    }

    #[test]
    fn with_default_cargo_message_format_keeps_unsupported_commands_unchanged() {
        let args = vec!["--version".to_string()];
        let normalized = with_default_cargo_message_format(args.clone());
        assert_eq!(normalized, args);
    }

    fn test_core_with_ui(root: &Path) -> (HubCore, ToUIReceiver<Vec<u8>>) {
        let (event_tx, event_rx) = mpsc::channel::<HubEvent>();
        let mut vfs = VirtualFs::new();
        vfs.mount("repo", root.to_path_buf()).expect("mount repo");
        let mut core = HubCore::new(event_rx, event_tx, vfs, None, None);

        let ui_rx = ToUIReceiver::<Vec<u8>>::default();
        core.handle_event(HubEvent::ClientConnected {
            web_socket_id: 1,
            sender: ui_rx.sender(),
            typed_sender: None,
        });
        let _ = ui_rx.receiver.recv_timeout(Duration::from_millis(250)); // hello
        (core, ui_rx)
    }

    fn pump_core(core: &mut HubCore, max_wait: Duration) {
        let deadline = Instant::now() + max_wait;
        while Instant::now() < deadline {
            match core.rx.recv_timeout(Duration::from_millis(20)) {
                Ok(event) => {
                    if !core.handle_event(event) {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    fn recv_ui_messages(rx: &ToUIReceiver<Vec<u8>>, max_wait: Duration) -> Vec<HubToClient> {
        let deadline = Instant::now() + max_wait;
        let mut out = Vec::new();
        while Instant::now() < deadline {
            match rx.receiver.recv_timeout(Duration::from_millis(25)) {
                Ok(data) => {
                    if let Ok(msg) = HubToClient::deserialize_bin(&data) {
                        out.push(msg);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        out
    }

    fn render_like_sparse_codex(terminal: &mut Terminal, cols: u16, rows: u16) {
        // Codex-style app keeps a custom scroll region but only redraws a small
        // subset of rows while idle.
        terminal.process_bytes(b"\x1b[r");
        terminal.process_bytes(format!("\x1b[3;{}r", rows - 2).as_bytes());

        terminal.process_bytes(b"\x1b[1;1H\x1b[K");
        let header = format!("{:<width$}", "=== Codex ===", width = cols as usize);
        terminal.process_bytes(header.as_bytes());

        terminal.process_bytes(b"\x1b[2;1H\x1b[K");
        let sep = "-".repeat(cols as usize);
        terminal.process_bytes(sep.as_bytes());

        for r in 3..=6.min(rows.saturating_sub(2)) {
            terminal.process_bytes(format!("\x1b[{};1H\x1b[K", r).as_bytes());
            let content = format!("idle {}", r);
            terminal.process_bytes(content.as_bytes());
        }

        // Keep cursor in content area (not bottom), matching sparse idle state.
        terminal.process_bytes(b"\x1b[6;6H");
    }

    fn seed_history(terminal: &mut Terminal, count: usize) {
        for i in 0..count {
            let line = format!("history line {:03}\r\n", i);
            terminal.process_bytes(line.as_bytes());
        }
    }

    fn decode_frame_row(frame: &TerminalFramebuffer, row: usize) -> String {
        let cols = frame.cols as usize;
        let mut out = String::with_capacity(cols);
        for col in 0..cols {
            let idx = (row * cols + col) * 10;
            let codepoint = u32::from_le_bytes([
                frame.cells[idx],
                frame.cells[idx + 1],
                frame.cells[idx + 2],
                frame.cells[idx + 3],
            ]);
            out.push(char::from_u32(codepoint).unwrap_or(' '));
        }
        out.trim_end().to_string()
    }

    fn decode_frame_codepoint(frame: &TerminalFramebuffer, row: usize, col: usize) -> u32 {
        let cols = frame.cols as usize;
        let idx = (row * cols + col) * 10;
        u32::from_le_bytes([
            frame.cells[idx],
            frame.cells[idx + 1],
            frame.cells[idx + 2],
            frame.cells[idx + 3],
        ])
    }

    #[test]
    fn terminal_framebuffer_preserves_nul_cells() {
        let mut term = Terminal::new(4, 1);
        term.screen_mut().grid.cell_mut(0, 0).codepoint = 'A';
        term.screen_mut().grid.cell_mut(1, 0).codepoint = '\0';
        term.screen_mut().grid.cell_mut(2, 0).codepoint = 'B';

        let frame = terminal_framebuffer_from_terminal(&term, 4, 1, 0, 1);
        assert_eq!(decode_frame_codepoint(&frame, 0, 0), 'A' as u32);
        assert_eq!(decode_frame_codepoint(&frame, 0, 1), 0);
        assert_eq!(decode_frame_codepoint(&frame, 0, 2), 'B' as u32);
    }

    #[test]
    fn terminal_framebuffer_text_trims_rows_and_hides_nul_cells() {
        let mut term = Terminal::new(4, 2);
        term.screen_mut().grid.cell_mut(0, 0).codepoint = 'A';
        term.screen_mut().grid.cell_mut(1, 0).codepoint = '\0';
        term.screen_mut().grid.cell_mut(2, 0).codepoint = 'B';
        term.screen_mut().grid.cell_mut(0, 1).codepoint = 'C';

        let frame = terminal_framebuffer_from_terminal(&term, 4, 2, 0, 1);
        assert_eq!(terminal_framebuffer_text(&frame), "A B\nC");
    }

    #[test]
    fn parse_ai_terminal_key_spec_supports_modifiers_and_named_keys() {
        let spec = parse_ai_terminal_key_spec("ctrl+shift+tab", false, false, false).unwrap();
        assert_eq!(
            spec,
            AiParsedTerminalKeySpec {
                input: AiTerminalKeyInput::Named(TermKeyCode::Tab),
                shift: true,
                control: true,
                alt: false,
            }
        );

        let spec = parse_ai_terminal_key_spec("F5", false, false, false).unwrap();
        assert_eq!(
            spec,
            AiParsedTerminalKeySpec {
                input: AiTerminalKeyInput::Named(TermKeyCode::F5),
                shift: false,
                control: false,
                alt: false,
            }
        );
    }

    #[test]
    fn encode_ai_terminal_key_supports_ctrl_letters() {
        let spec = parse_ai_terminal_key_spec("ctrl+c", false, false, false).unwrap();
        let terminal = Terminal::new(80, 24);
        assert_eq!(encode_ai_terminal_key(&terminal, &spec), Some(vec![0x03]));
    }

    #[test]
    fn terminal_auto_submit_ai_text_detects_agent_terminals() {
        assert!(HubCore::terminal_auto_submit_ai_text(
            "repo/.makepad/codex.term",
            "",
            "",
            "write a poem into poem.txt"
        ));
        assert!(HubCore::terminal_auto_submit_ai_text(
            "repo/.makepad/a.term",
            "Claude Code",
            "",
            "continue"
        ));
        assert!(HubCore::terminal_auto_submit_ai_text(
            "repo/.makepad/a.term",
            "zsh",
            "› Enter a prompt...",
            "write a poem into poem.txt"
        ));
        assert!(!HubCore::terminal_auto_submit_ai_text(
            "repo/.makepad/shell.term",
            "zsh",
            "",
            "echo hi"
        ));
        assert!(!HubCore::terminal_auto_submit_ai_text(
            "repo/.makepad/codex.term",
            "",
            "",
            "already has newline\n"
        ));
    }

    #[test]
    fn terminal_framebuffer_sparse_codex_roundtrip_after_30_15_30_resize_without_history() {
        let cols = 120u16;
        let rows_large = 30u16;
        let rows_small = 15u16;
        let viewport_rows = rows_large + 1;

        let mut term = Terminal::new(cols as usize, rows_large as usize);
        render_like_sparse_codex(&mut term, cols, rows_large);
        assert!(
            term.screen().used_rows() < rows_large as usize,
            "test precondition failed: expected sparse grid, used_rows={}, rows={}",
            term.screen().used_rows(),
            rows_large
        );

        // Crunch and redraw.
        term.resize(cols as usize, rows_small as usize);
        render_like_sparse_codex(&mut term, cols, rows_small);
        // Expand and redraw.
        term.resize(cols as usize, rows_large as usize);
        render_like_sparse_codex(&mut term, cols, rows_large);

        let after = terminal_framebuffer_from_terminal(&term, cols, viewport_rows, 0, 1);

        let mut fresh = Terminal::new(cols as usize, rows_large as usize);
        render_like_sparse_codex(&mut fresh, cols, rows_large);
        let expected = terminal_framebuffer_from_terminal(&fresh, cols, viewport_rows, 0, 1);

        assert_eq!(after.top_row, 0);
        assert_eq!(after.total_lines, expected.total_lines);
        assert_eq!(
            after.cells,
            expected.cells,
            "row6='{}' row20='{}'",
            decode_frame_row(&after, 5),
            decode_frame_row(&after, 19)
        );
    }

    #[test]
    fn terminal_framebuffer_sparse_codex_roundtrip_after_30_15_30_resize_with_history() {
        let cols = 120u16;
        let rows_large = 30u16;
        let rows_small = 15u16;
        let viewport_rows = rows_large + 1;

        let mut term = Terminal::new(cols as usize, rows_large as usize);
        seed_history(&mut term, 200);
        render_like_sparse_codex(&mut term, cols, rows_large);

        term.resize(cols as usize, rows_small as usize);
        render_like_sparse_codex(&mut term, cols, rows_small);
        term.resize(cols as usize, rows_large as usize);
        render_like_sparse_codex(&mut term, cols, rows_large);

        let after = terminal_framebuffer_from_terminal(&term, cols, viewport_rows, 0, 1);

        let mut fresh = Terminal::new(cols as usize, rows_large as usize);
        seed_history(&mut fresh, 200);
        render_like_sparse_codex(&mut fresh, cols, rows_large);
        let expected = terminal_framebuffer_from_terminal(&fresh, cols, viewport_rows, 0, 1);

        assert_eq!(after.top_row, 0);
        assert_eq!(after.total_lines, expected.total_lines);
        assert_eq!(after.cells, expected.cells);
    }

    #[test]
    fn ui_envelope_uses_typed_channel_for_in_process_clients() {
        let dir = crate::test_support::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

        let (event_tx, event_rx) = mpsc::channel::<HubEvent>();
        let mut vfs = VirtualFs::new();
        vfs.mount("repo", dir.path().to_path_buf())
            .expect("mount repo");
        let mut core = HubCore::new(event_rx, event_tx, vfs, None, None);

        let ui_rx_bin = ToUIReceiver::<Vec<u8>>::default();
        let ui_rx_typed = ToUIReceiver::<HubToClient>::default();
        core.handle_event(HubEvent::ClientConnected {
            web_socket_id: 1,
            sender: ui_rx_bin.sender(),
            typed_sender: Some(ui_rx_typed.sender()),
        });

        let hello = ui_rx_typed
            .receiver
            .recv_timeout(Duration::from_millis(250))
            .expect("typed hello");
        let client_id = match hello {
            HubToClient::Hello { client_id } => client_id,
            other => panic!("expected Hello, got {:?}", other),
        };

        let query_id = QueryId::new(client_id, 0);
        core.handle_event(HubEvent::ClientEnvelope {
            web_socket_id: 1,
            envelope: ClientToHubEnvelope {
                query_id,
                msg: ClientToHub::LoadFileTree {
                    mount: "repo".to_string(),
                },
            },
        });
        pump_core(&mut core, Duration::from_millis(300));

        let msg = ui_rx_typed
            .receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("typed FileTree");
        match msg {
            HubToClient::FileTree { mount, data } => {
                assert_eq!(mount, "repo");
                assert!(data.nodes.iter().any(|node| node.path == "repo/src/lib.rs"));
            }
            other => panic!("expected FileTree, got {:?}", other),
        }

        assert!(ui_rx_bin.receiver.try_recv().is_err());
    }

    #[test]
    fn ui_envelope_rejects_mismatched_client_id() {
        let dir = crate::test_support::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

        let (event_tx, event_rx) = mpsc::channel::<HubEvent>();
        let mut vfs = VirtualFs::new();
        vfs.mount("repo", dir.path().to_path_buf())
            .expect("mount repo");
        let mut core = HubCore::new(event_rx, event_tx, vfs, None, None);

        let ui_rx = ToUIReceiver::<Vec<u8>>::default();
        core.handle_event(HubEvent::ClientConnected {
            web_socket_id: 1,
            sender: ui_rx.sender(),
            typed_sender: None,
        });
        let hello_bin = ui_rx
            .receiver
            .recv_timeout(Duration::from_millis(250))
            .expect("hello");
        let client_id = match HubToClient::deserialize_bin(&hello_bin).expect("deserialize hello") {
            HubToClient::Hello { client_id } => client_id,
            other => panic!("expected Hello, got {:?}", other),
        };
        let wrong_client_id = if client_id.0 == 0 {
            ClientId(1)
        } else {
            ClientId(0)
        };

        core.handle_event(HubEvent::ClientEnvelope {
            web_socket_id: 1,
            envelope: ClientToHubEnvelope {
                query_id: QueryId::new(wrong_client_id, 0),
                msg: ClientToHub::ListBuilds,
            },
        });

        pump_core(&mut core, Duration::from_millis(250));
        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(300));
        assert!(messages.iter().any(|msg| {
            matches!(
                msg,
                HubToClient::Error { message }
                    if message.contains("query_id.client_id does not match assigned client")
            )
        }));
    }

    #[test]
    fn ui_binary_rejects_mismatched_client_id() {
        let dir = crate::test_support::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

        let (event_tx, event_rx) = mpsc::channel::<HubEvent>();
        let mut vfs = VirtualFs::new();
        vfs.mount("repo", dir.path().to_path_buf())
            .expect("mount repo");
        let mut core = HubCore::new(event_rx, event_tx, vfs, None, None);

        let ui_rx = ToUIReceiver::<Vec<u8>>::default();
        core.handle_event(HubEvent::ClientConnected {
            web_socket_id: 1,
            sender: ui_rx.sender(),
            typed_sender: None,
        });
        let hello_bin = ui_rx
            .receiver
            .recv_timeout(Duration::from_millis(250))
            .expect("hello");
        let client_id = match HubToClient::deserialize_bin(&hello_bin).expect("deserialize hello") {
            HubToClient::Hello { client_id } => client_id,
            other => panic!("expected Hello, got {:?}", other),
        };
        let wrong_client_id = if client_id.0 == 0 {
            ClientId(1)
        } else {
            ClientId(0)
        };
        let data = ClientToHubEnvelope {
            query_id: QueryId::new(wrong_client_id, 0),
            msg: ClientToHub::ListBuilds,
        }
        .serialize_bin();

        core.handle_event(HubEvent::ClientBinary {
            web_socket_id: 1,
            data,
        });

        pump_core(&mut core, Duration::from_millis(250));
        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(300));
        assert!(messages.iter().any(|msg| {
            matches!(
                msg,
                HubToClient::Error { message }
                    if message.contains("query_id.client_id does not match assigned client")
            )
        }));
    }

    #[test]
    fn secondary_ui_click_is_accepted_and_visualized_for_primary_observer() {
        let dir = crate::test_support::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

        let (event_tx, event_rx) = mpsc::channel::<HubEvent>();
        let mut vfs = VirtualFs::new();
        vfs.mount("repo", dir.path().to_path_buf())
            .expect("mount repo");
        let mut core = HubCore::new(event_rx, event_tx, vfs, None, None);

        let primary_ui = ToUIReceiver::<Vec<u8>>::default();
        core.handle_event(HubEvent::ClientConnected {
            web_socket_id: 1,
            sender: primary_ui.sender(),
            typed_sender: None,
        });
        let primary_client_id = match HubToClient::deserialize_bin(
            &primary_ui
                .receiver
                .recv_timeout(Duration::from_millis(250))
                .expect("primary hello"),
        )
        .expect("decode primary hello")
        {
            HubToClient::Hello { client_id } => client_id,
            other => panic!("expected Hello, got {:?}", other),
        };

        let secondary_ui = ToUIReceiver::<Vec<u8>>::default();
        core.handle_event(HubEvent::ClientConnected {
            web_socket_id: 2,
            sender: secondary_ui.sender(),
            typed_sender: None,
        });
        let secondary_client_id = match HubToClient::deserialize_bin(
            &secondary_ui
                .receiver
                .recv_timeout(Duration::from_millis(250))
                .expect("secondary hello"),
        )
        .expect("decode secondary hello")
        {
            HubToClient::Hello { client_id } => client_id,
            other => panic!("expected Hello, got {:?}", other),
        };

        let build_id = QueryId::new(secondary_client_id, 42);
        core.build_mount_by_id.insert(build_id, "repo".to_string());

        let (app_tx, app_rx) = mpsc::channel::<Vec<u8>>();
        core.handle_event(HubEvent::AppConnected {
            build_id: Some(build_id),
            crate_name: Some("makepad-example-xr".to_string()),
            web_socket_id: 77,
            sender: app_tx,
        });

        core.handle_event(HubEvent::ClientEnvelope {
            web_socket_id: 1,
            envelope: ClientToHubEnvelope {
                query_id: QueryId::new(primary_client_id, 0),
                msg: ClientToHub::ObserveMount {
                    mount: "repo".to_string(),
                    primary: Some(true),
                },
            },
        });

        core.handle_event(HubEvent::ClientEnvelope {
            web_socket_id: 2,
            envelope: ClientToHubEnvelope {
                query_id: QueryId::new(secondary_client_id, 0),
                msg: ClientToHub::Click {
                    build_id,
                    x: 12,
                    y: 34,
                },
            },
        });

        let sent_to_app = app_rx
            .recv_timeout(Duration::from_millis(250))
            .expect("click payload to app");
        let StudioToAppVec(app_msgs) =
            StudioToAppVec::deserialize_bin(&sent_to_app).expect("decode app payload");
        assert!(app_msgs
            .iter()
            .any(|msg| matches!(msg, StudioToApp::MouseDown(_))));
        assert!(app_msgs
            .iter()
            .any(|msg| matches!(msg, StudioToApp::MouseUp(_))));

        let primary_messages = recv_ui_messages(&primary_ui, Duration::from_millis(300));
        assert!(primary_messages.iter().any(|msg| {
            matches!(
                msg,
                HubToClient::RunViewInputViz {
                    build_id: id,
                    kind: RunViewInputVizKind::ClickDown,
                    x: Some(x),
                    y: Some(y),
                } if *id == build_id && *x == 12.0 && *y == 34.0
            )
        }));
        assert!(primary_messages.iter().any(|msg| {
            matches!(
                msg,
                HubToClient::RunViewInputViz {
                    build_id: id,
                    kind: RunViewInputVizKind::ClickUp,
                    x: Some(x),
                    y: Some(y),
                } if *id == build_id && *x == 12.0 && *y == 34.0
            )
        }));

        let secondary_messages = recv_ui_messages(&secondary_ui, Duration::from_millis(300));
        assert!(!secondary_messages
            .iter()
            .any(|msg| matches!(msg, HubToClient::Error { .. })));
    }

    #[test]
    fn bootstrap_forward_is_queued_until_app_socket_connects() {
        let dir = crate::test_support::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

        let (event_tx, event_rx) = mpsc::channel::<HubEvent>();
        let mut vfs = VirtualFs::new();
        vfs.mount("repo", dir.path().to_path_buf())
            .expect("mount repo");
        let mut core = HubCore::new(event_rx, event_tx, vfs, None, None);

        let ui_rx = ToUIReceiver::<Vec<u8>>::default();
        core.handle_event(HubEvent::ClientConnected {
            web_socket_id: 1,
            sender: ui_rx.sender(),
            typed_sender: None,
        });
        let hello = HubToClient::deserialize_bin(
            &ui_rx
                .receiver
                .recv_timeout(Duration::from_millis(250))
                .expect("hello"),
        )
        .expect("decode hello");
        let client_id = match hello {
            HubToClient::Hello { client_id } => client_id,
            other => panic!("expected Hello, got {:?}", other),
        };

        let build_id = QueryId::new(client_id, 42);
        core.handle_event(HubEvent::ClientEnvelope {
            web_socket_id: 1,
            envelope: ClientToHubEnvelope {
                query_id: QueryId::new(client_id, 0),
                msg: ClientToHub::ForwardToApp {
                    build_id,
                    msg_bin: StudioToAppVec(vec![StudioToApp::WindowGeomChange {
                        window_id: 0,
                        dpi_factor: 1.0,
                        left: 0.0,
                        top: 0.0,
                        width: 640.0,
                        height: 480.0,
                    }])
                    .serialize_bin(),
                },
            },
        });

        let queued_messages = recv_ui_messages(&ui_rx, Duration::from_millis(150));
        assert!(!queued_messages
            .iter()
            .any(|msg| matches!(msg, HubToClient::Error { .. })));

        let (app_tx, app_rx) = mpsc::channel::<Vec<u8>>();
        core.handle_event(HubEvent::AppConnected {
            build_id: Some(build_id),
            crate_name: Some("makepad-example-xr".to_string()),
            web_socket_id: 77,
            sender: app_tx,
        });

        let sent_to_app = app_rx
            .recv_timeout(Duration::from_millis(250))
            .expect("queued bootstrap to app");
        let StudioToAppVec(app_msgs) =
            StudioToAppVec::deserialize_bin(&sent_to_app).expect("decode app payload");
        assert_eq!(app_msgs.len(), 1);
        match &app_msgs[0] {
            StudioToApp::WindowGeomChange {
                window_id,
                width,
                height,
                ..
            } => {
                assert_eq!(*window_id, 0);
                assert_eq!(*width, 640.0);
                assert_eq!(*height, 480.0);
            }
            other => panic!("unexpected app message: {:?}", other),
        }
    }

    #[test]
    fn mount_fs_changed_file_path_emits_added_diff() {
        let dir = crate::test_support::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

        let (mut core, ui_rx) = test_core_with_ui(dir.path());
        fs::write(dir.path().join("src/new_file.rs"), "pub fn new_file() {}\n").unwrap();
        core.handle_event(HubEvent::MountFsChanged {
            mount: "repo".to_string(),
            path: dir.path().join("src/new_file.rs"),
        });

        pump_core(&mut core, Duration::from_millis(400));
        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(300));
        assert!(
            messages.iter().any(|msg| {
                matches!(
                    msg,
                    HubToClient::FileTreeDiff { mount, changes }
                        if mount == "repo"
                            && changes.iter().any(|change| {
                                matches!(
                                    change,
                                    backend_proto::FileTreeChange::Added { path, .. }
                                        if path == "repo/src/new_file.rs"
                                )
                            })
                )
            }),
            "expected Added diff for repo/src/new_file.rs"
        );
    }

    #[test]
    fn mount_fs_changed_file_path_ignores_mount_root_suppress_window() {
        let dir = crate::test_support::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

        let (mut core, ui_rx) = test_core_with_ui(dir.path());
        core.mount_suppress_fs_until
            .insert("repo".to_string(), Instant::now() + Duration::from_secs(2));
        fs::write(dir.path().join("src/new_file.rs"), "pub fn new_file() {}\n").unwrap();
        core.handle_event(HubEvent::MountFsChanged {
            mount: "repo".to_string(),
            path: dir.path().join("src/new_file.rs"),
        });

        pump_core(&mut core, Duration::from_millis(400));
        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(300));
        assert!(
            messages.iter().any(|msg| {
                matches!(
                    msg,
                    HubToClient::FileTreeDiff { mount, changes }
                        if mount == "repo"
                            && changes.iter().any(|change| {
                                matches!(
                                    change,
                                    backend_proto::FileTreeChange::Added { path, .. }
                                        if path == "repo/src/new_file.rs"
                                )
                            })
                )
            }),
            "expected path-level fs event to bypass mount-root suppress window"
        );
    }

    #[test]
    fn mount_fs_changed_mount_root_still_honors_suppress_window() {
        let dir = crate::test_support::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

        let (mut core, ui_rx) = test_core_with_ui(dir.path());
        core.mount_suppress_fs_until
            .insert("repo".to_string(), Instant::now() + Duration::from_secs(2));
        core.handle_event(HubEvent::MountFsChanged {
            mount: "repo".to_string(),
            path: dir.path().to_path_buf(),
        });

        pump_core(&mut core, Duration::from_millis(400));
        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(350));
        assert!(
            !messages.iter().any(|msg| {
                matches!(
                    msg,
                    HubToClient::FileTree { mount, .. } | HubToClient::FileTreeDiff { mount, .. }
                        if mount == "repo"
                ) || matches!(msg, HubToClient::FileChanged { path } if path == "repo")
            }),
            "expected mount-root fs event to remain suppressed"
        );
    }

    #[test]
    fn suppress_mount_root_fs_events_event_suppresses_mount_root_fallback() {
        let dir = crate::test_support::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".makepad/ai_chats")).unwrap();
        fs::write(dir.path().join(".makepad/ai_chats/chat.json"), "{}\n").unwrap();

        let (mut core, ui_rx) = test_core_with_ui(dir.path());
        core.handle_event(HubEvent::SuppressMountRootFsEvents {
            mount: "repo".to_string(),
            duration: Duration::from_secs(2),
        });
        core.handle_event(HubEvent::MountFsChanged {
            mount: "repo".to_string(),
            path: dir.path().to_path_buf(),
        });

        pump_core(&mut core, Duration::from_millis(400));
        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(350));
        assert!(
            !messages.iter().any(|msg| {
                matches!(
                    msg,
                    HubToClient::FileTree { mount, .. } | HubToClient::FileTreeDiff { mount, .. }
                        if mount == "repo"
                ) || matches!(msg, HubToClient::FileChanged { path } if path == "repo")
            }),
            "expected persisted .makepad chat root fallback to remain suppressed"
        );
    }

    #[test]
    fn mount_fs_changed_directory_path_triggers_full_tree_reload() {
        let dir = crate::test_support::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

        let (mut core, ui_rx) = test_core_with_ui(dir.path());
        fs::write(dir.path().join("src/from_dir_event.rs"), "pub fn d() {}\n").unwrap();
        core.handle_event(HubEvent::MountFsChanged {
            mount: "repo".to_string(),
            path: dir.path().join("src"),
        });

        pump_core(&mut core, Duration::from_millis(400));
        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(350));
        assert!(
            messages.iter().any(|msg| {
                matches!(
                    msg,
                    HubToClient::FileTree { mount, data }
                        if mount == "repo"
                            && data
                                .nodes
                                .iter()
                                .any(|node| node.path == "repo/src/from_dir_event.rs")
                )
            }),
            "expected full FileTree reload to include repo/src/from_dir_event.rs"
        );
    }

    #[test]
    fn mount_fs_changed_git_metadata_path_triggers_full_tree_reload() {
        let dir = crate::test_support::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();
        fs::write(dir.path().join(".git/index"), "").unwrap();

        let (mut core, ui_rx) = test_core_with_ui(dir.path());
        core.handle_event(HubEvent::MountFsChanged {
            mount: "repo".to_string(),
            path: dir.path().join(".git/index"),
        });

        pump_core(&mut core, Duration::from_millis(400));
        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(350));
        assert!(
            messages
                .iter()
                .any(|msg| matches!(msg, HubToClient::FileTree { mount, .. } if mount == "repo")),
            "expected .git metadata fs event to trigger a full FileTree reload"
        );
    }

    #[test]
    fn full_file_tree_reload_payload_is_much_larger_than_single_file_diff() {
        let dir = crate::test_support::tempdir().unwrap();
        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        for i in 0..1200usize {
            let path = src_dir.join(format!("f{:04}.rs", i));
            fs::write(path, format!("pub fn f{:04}() {{}}\n", i)).unwrap();
        }

        let (mut core, ui_rx) = test_core_with_ui(dir.path());

        // Trigger a full reload path (directory event).
        core.handle_event(HubEvent::MountFsChanged {
            mount: "repo".to_string(),
            path: src_dir.clone(),
        });
        pump_core(&mut core, Duration::from_secs(2));

        let mut full_reload_bytes = None;
        let full_deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < full_deadline {
            let Ok(raw) = ui_rx.receiver.recv_timeout(Duration::from_millis(25)) else {
                continue;
            };
            let Ok(msg) = HubToClient::deserialize_bin(&raw) else {
                continue;
            };
            if matches!(msg, HubToClient::FileTree { ref mount, .. } if mount == "repo") {
                full_reload_bytes = Some(raw.len());
                break;
            }
        }
        let full_reload_bytes = full_reload_bytes.expect("expected full FileTree payload");

        let changed_path = src_dir.join("f0007.rs");
        fs::write(&changed_path, "pub fn f0007() { let _x = 1; }\n").unwrap();
        core.handle_event(HubEvent::MountFsChanged {
            mount: "repo".to_string(),
            path: changed_path,
        });
        pump_core(&mut core, Duration::from_secs(2));

        let mut diff_bytes = None;
        let diff_deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < diff_deadline {
            let Ok(raw) = ui_rx.receiver.recv_timeout(Duration::from_millis(25)) else {
                continue;
            };
            let Ok(msg) = HubToClient::deserialize_bin(&raw) else {
                continue;
            };
            if matches!(msg, HubToClient::FileTreeDiff { ref mount, .. } if mount == "repo") {
                diff_bytes = Some(raw.len());
                break;
            }
        }
        let diff_bytes = diff_bytes.expect("expected FileTreeDiff payload");

        eprintln!(
            "full FileTree payload={} bytes, single FileTreeDiff payload={} bytes",
            full_reload_bytes, diff_bytes
        );
        assert!(
            full_reload_bytes > diff_bytes.saturating_mul(20),
            "expected full reload payload to be far larger than single-file diff (full={} diff={})",
            full_reload_bytes,
            diff_bytes
        );
    }

    #[test]
    fn mount_fs_changed_removed_directory_emits_removed_diff() {
        let dir = crate::test_support::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src/nested")).unwrap();
        fs::write(dir.path().join("src/nested/mod.rs"), "pub fn nested() {}\n").unwrap();

        let (mut core, ui_rx) = test_core_with_ui(dir.path());
        fs::remove_dir_all(dir.path().join("src/nested")).unwrap();
        core.handle_event(HubEvent::MountFsChanged {
            mount: "repo".to_string(),
            path: dir.path().join("src/nested"),
        });

        pump_core(&mut core, Duration::from_millis(400));
        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(300));
        assert!(
            messages.iter().any(|msg| {
                matches!(
                    msg,
                    HubToClient::FileTreeDiff { mount, changes }
                        if mount == "repo"
                            && changes.iter().any(|change| {
                                matches!(
                                    change,
                                    backend_proto::FileTreeChange::Removed { path }
                                        if path == "repo/src/nested"
                                )
                            })
                )
            }),
            "expected Removed diff for repo/src/nested"
        );
    }

    #[test]
    fn raw_fs_event_storm_collapses_before_worker_deltas() {
        let dir = crate::test_support::tempdir().unwrap();
        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        for index in 0..(FS_EVENT_BATCH_RELOAD_THRESHOLD + 16) {
            fs::write(
                src_dir.join(format!("generated_{index}.rs")),
                format!("pub fn generated_{index}() {{}}\n"),
            )
            .unwrap();
        }

        let (mut core, ui_rx) = test_core_with_ui(dir.path());
        for index in 0..(FS_EVENT_BATCH_RELOAD_THRESHOLD + 16) {
            core.handle_event(HubEvent::MountFsChanged {
                mount: "repo".to_string(),
                path: src_dir.join(format!("generated_{index}.rs")),
            });
        }

        pump_core(&mut core, Duration::from_millis(900));
        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(350));
        let file_tree_count = messages
            .iter()
            .filter(|msg| matches!(msg, HubToClient::FileTree { mount, .. } if mount == "repo"))
            .count();
        let file_tree_diff_count = messages
            .iter()
            .filter(|msg| matches!(msg, HubToClient::FileTreeDiff { mount, .. } if mount == "repo"))
            .count();
        let file_changed_count = messages
            .iter()
            .filter(|msg| matches!(msg, HubToClient::FileChanged { path } if path == "repo"))
            .count();

        assert_eq!(
            file_tree_count, 1,
            "expected one full tree reload for raw watcher storm"
        );
        assert_eq!(
            file_tree_diff_count, 0,
            "expected raw watcher storm to avoid per-path worker diffs"
        );
        assert_eq!(
            file_changed_count, 1,
            "expected one mount-level editor refresh signal"
        );
    }

    #[test]
    fn worker_deltas_batch_and_coalesce_removed_descendants() {
        let dir = crate::test_support::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src/nested")).unwrap();
        let (mut core, ui_rx) = test_core_with_ui(dir.path());

        core.handle_event(HubEvent::WorkerFileTreeDeltaDone {
            mount: "repo".to_string(),
            change: backend_proto::FileTreeChange::Removed {
                path: "repo/src/nested/a.rs".to_string(),
            },
        });
        core.handle_event(HubEvent::WorkerFileTreeDeltaDone {
            mount: "repo".to_string(),
            change: backend_proto::FileTreeChange::Removed {
                path: "repo/src/nested/b.rs".to_string(),
            },
        });
        core.handle_event(HubEvent::WorkerFileTreeDeltaDone {
            mount: "repo".to_string(),
            change: backend_proto::FileTreeChange::Removed {
                path: "repo/src/nested".to_string(),
            },
        });
        core.handle_event(HubEvent::WorkerFileTreeDeltaDone {
            mount: "repo".to_string(),
            change: backend_proto::FileTreeChange::Removed {
                path: "repo/src/nested/c.rs".to_string(),
            },
        });

        pump_core(&mut core, Duration::from_millis(500));
        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(350));
        let diffs: Vec<Vec<backend_proto::FileTreeChange>> = messages
            .into_iter()
            .filter_map(|msg| match msg {
                HubToClient::FileTreeDiff { mount, changes } if mount == "repo" => Some(changes),
                _ => None,
            })
            .collect();
        assert_eq!(
            diffs.len(),
            1,
            "expected exactly one coalesced diff message"
        );
        let changes = &diffs[0];
        assert_eq!(changes.len(), 1, "expected descendant removals to collapse");
        assert!(matches!(
            &changes[0],
            backend_proto::FileTreeChange::Removed { path } if path == "repo/src/nested"
        ));
    }

    #[test]
    fn worker_remove_then_add_same_path_keeps_added_state() {
        let dir = crate::test_support::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        let (mut core, ui_rx) = test_core_with_ui(dir.path());

        core.handle_event(HubEvent::WorkerFileTreeDeltaDone {
            mount: "repo".to_string(),
            change: backend_proto::FileTreeChange::Removed {
                path: "repo/src/lib.rs".to_string(),
            },
        });
        core.handle_event(HubEvent::WorkerFileTreeDeltaDone {
            mount: "repo".to_string(),
            change: backend_proto::FileTreeChange::Added {
                path: "repo/src/lib.rs".to_string(),
                node_type: backend_proto::FileNodeType::File,
                git_status: backend_proto::GitStatus::Modified,
            },
        });

        pump_core(&mut core, Duration::from_millis(500));
        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(350));
        let diffs: Vec<Vec<backend_proto::FileTreeChange>> = messages
            .into_iter()
            .filter_map(|msg| match msg {
                HubToClient::FileTreeDiff { mount, changes } if mount == "repo" => Some(changes),
                _ => None,
            })
            .collect();
        assert_eq!(diffs.len(), 1, "expected exactly one diff message");
        assert_eq!(diffs[0].len(), 1, "expected a single merged change");
        assert!(matches!(
            &diffs[0][0],
            backend_proto::FileTreeChange::Added { path, .. } if path == "repo/src/lib.rs"
        ));
    }

    #[test]
    fn worker_delta_storm_falls_back_to_single_tree_reload() {
        let dir = crate::test_support::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

        let (mut core, ui_rx) = test_core_with_ui(dir.path());
        for index in 0..(FS_DELTA_RELOAD_THRESHOLD + 16) {
            core.handle_event(HubEvent::WorkerFileTreeDeltaDone {
                mount: "repo".to_string(),
                change: backend_proto::FileTreeChange::Removed {
                    path: format!("repo/src/storm/file_{index}.rs"),
                },
            });
        }

        pump_core(&mut core, Duration::from_millis(700));
        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(350));
        let saw_reload = messages
            .iter()
            .any(|msg| matches!(msg, HubToClient::FileTree { mount, .. } if mount == "repo"));
        let saw_diff = messages
            .iter()
            .any(|msg| matches!(msg, HubToClient::FileTreeDiff { mount, .. } if mount == "repo"));
        assert!(
            saw_reload,
            "expected full tree reload for large delta storm"
        );
        assert!(
            !saw_diff,
            "expected storm fallback to suppress per-path diff emission"
        );
    }
}

fn write_screenshot_png(
    build_id: QueryId,
    kind_id: u32,
    request_id: u64,
    png: &[u8],
) -> Result<String, String> {
    let mut dir = std::env::temp_dir();
    dir.push("makepad_studio_hub");
    fs::create_dir_all(&dir)
        .map_err(|err| format!("failed to create screenshot dir {}: {}", dir.display(), err))?;

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("system time error: {}", err))?
        .as_millis();
    let file_name = format!(
        "build-{}-kind-{}-req-{}-{}.png",
        build_id.0, kind_id, request_id, now_ms
    );
    let path = dir.join(file_name);
    fs::write(&path, png)
        .map_err(|err| format!("failed to write screenshot {}: {}", path.display(), err))?;
    Ok(path.to_string_lossy().to_string())
}
