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
    // Latest viewport size requested by UI (desired PTY size).
    cols: u16,
    rows: u16,
    // Size currently applied to the terminal emulator model.
    applied_cols: u16,
    applied_rows: u16,
    // Monotonic frame sequence to let clients drop stale frames.
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

    fn reserve_client_id(&mut self, client_id: ClientId) -> bool {
        let Some(slot) = self.client_id_in_use.get_mut(client_id.0 as usize) else {
            return false;
        };
        if *slot {
            return false;
        }
        *slot = true;
        true
    }

    fn alloc_client_id(&mut self) -> Option<ClientId> {
        for client_id in 1..(MAX_UI_CLIENT_IDS as u16) {
            if self.reserve_client_id(ClientId(client_id)) {
                return Some(ClientId(client_id));
            }
        }
        None
    }

    fn release_client_id(&mut self, client_id: ClientId) {
        if let Some(slot) = self.client_id_in_use.get_mut(client_id.0 as usize) {
            *slot = false;
        }
    }

    fn alloc_build_id(&mut self) -> QueryId {
        let build_id = QueryId(self.next_build_id);
        self.next_build_id = self.next_build_id.wrapping_add(1);
        if self.next_build_id == 0 {
            self.next_build_id = 1;
        }
        build_id
    }

    fn on_ui_connected(
        &mut self,
        web_socket_id: u64,
        sender: ToUISender<Vec<u8>>,
        typed_sender: Option<ToUISender<HubToClient>>,
    ) {
        if studio_hub_debug_enabled() {
            let used_lanes = self
                .client_id_in_use
                .iter()
                .copied()
                .filter(|used| *used)
                .count();
            eprintln!(
                "studio hub debug: on_ui_connected web_socket_id={} typed_sender={} used_lanes={} ui_clients={}",
                web_socket_id,
                typed_sender.is_some(),
                used_lanes,
                self.ui_clients.len()
            );
        }
        let client_id = if web_socket_id == IN_PROCESS_UI_WEB_SOCKET_ID {
            let reserved = ClientId(0);
            if !self.reserve_client_id(reserved) {
                if studio_hub_debug_enabled() {
                    eprintln!(
                        "studio hub debug: ui connect failed web_socket_id={} reason=in_process_client_id_0_in_use",
                        web_socket_id
                    );
                }
                if let Some(typed_sender) = &typed_sender {
                    let _ = typed_sender.send(HubToClient::Error {
                        message: "client id 0 already in use".to_string(),
                    });
                } else {
                    let _ = sender.send(
                        HubToClient::Error {
                            message: "client id 0 already in use".to_string(),
                        }
                        .serialize_bin(),
                    );
                }
                return;
            }
            reserved
        } else {
            let Some(client_id) = self.alloc_client_id() else {
                if studio_hub_debug_enabled() {
                    let active_client_ids: Vec<u16> =
                        self.ui_clients.keys().map(|id| id.0).collect();
                    eprintln!(
                        "studio hub debug: ui connect failed web_socket_id={} reason=no_client_lane active_client_ids={:?}",
                        web_socket_id, active_client_ids
                    );
                }
                let _ = sender.send(
                    HubToClient::Error {
                        message: "client id space exhausted".to_string(),
                    }
                    .serialize_bin(),
                );
                let _ = sender.send(Vec::new());
                return;
            };
            client_id
        };

        if self.ui_clients.contains_key(&client_id) {
            if studio_hub_debug_enabled() {
                eprintln!(
                    "studio hub debug: ui connect failed web_socket_id={} reason=duplicate_client_id client_id={:?}",
                    web_socket_id, client_id
                );
            }
            self.release_client_id(client_id);
            if let Some(typed_sender) = &typed_sender {
                let _ = typed_sender.send(HubToClient::Error {
                    message: format!("client id {:?} already in use", client_id),
                });
            } else {
                let _ = sender.send(
                    HubToClient::Error {
                        message: format!("client id {:?} already in use", client_id),
                    }
                    .serialize_bin(),
                );
            }
            let _ = sender.send(Vec::new());
            return;
        }

        self.client_by_web_socket.insert(web_socket_id, client_id);
        self.ui_clients.insert(
            client_id,
            UiClient {
                sender,
                typed_sender,
                format: WireFormat::Binary,
            },
        );
        if studio_hub_debug_enabled() {
            eprintln!(
                "studio hub debug: ui connected web_socket_id={} client_id={:?} ui_clients={}",
                web_socket_id,
                client_id,
                self.ui_clients.len()
            );
        }
        self.send_ui_message(
            client_id,
            HubToClient::Hello { client_id },
            WireFormat::Binary,
        );
        if studio_hub_debug_enabled() {
            eprintln!(
                "studio hub debug: ui hello sent web_socket_id={} client_id={:?}",
                web_socket_id, client_id
            );
        }
    }

    fn on_ui_envelope(&mut self, client_id: ClientId, envelope: ClientToHubEnvelope) {
        if !self.ui_clients.contains_key(&client_id) {
            return;
        }
        if envelope.query_id.client_id() != client_id {
            self.send_ui_error(
                client_id,
                "query_id.client_id does not match assigned client".to_string(),
            );
            return;
        }
        self.handle_ui_message(client_id, envelope);
    }

    fn on_ui_message(&mut self, client_id: ClientId, format: WireFormat, data: &[u8]) {
        let Some(client) = self.ui_clients.get_mut(&client_id) else {
            return;
        };
        client.format = format;
        let envelope = match format {
            WireFormat::Binary => ClientToHubEnvelope::deserialize_bin(data).map_err(|e| e.msg),
            WireFormat::Text => std::str::from_utf8(data)
                .map_err(|err| err.to_string())
                .and_then(|text| ClientToHubEnvelope::deserialize_json(text).map_err(|e| e.msg)),
        };

        let envelope = match envelope {
            Ok(v) => v,
            Err(err) => {
                self.send_ui_error(client_id, format!("invalid UI envelope: {}", err));
                return;
            }
        };

        if envelope.query_id.client_id() != client_id {
            self.send_ui_error(
                client_id,
                "query_id.client_id does not match assigned client".to_string(),
            );
            return;
        }

        self.handle_ui_message(client_id, envelope);
    }

    fn handle_ui_message(&mut self, client_id: ClientId, envelope: ClientToHubEnvelope) {
        let query_id = envelope.query_id;
        match envelope.msg {
            ClientToHub::Mount { name, path } => match self.vfs.mount(&name, path) {
                Ok(()) => {
                    self.reset_fs_watcher();
                    if let Ok(root) = self.vfs.resolve_mount(&name) {
                        self.ai_manager.register_mount(&name, &root);
                    }
                    match self.vfs.load_file_tree(&name) {
                        Ok(data) => self
                            .send_ui_reply(client_id, HubToClient::FileTree { mount: name, data }),
                        Err(err) => self.send_ui_error(client_id, err.to_string()),
                    }
                }
                Err(err) => self.send_ui_error(client_id, err.to_string()),
            },
            ClientToHub::Unmount { name } => {
                let changes = match self.vfs.load_file_tree(&name) {
                    Ok(tree) => tree
                        .nodes
                        .into_iter()
                        .map(|node| backend_proto::FileTreeChange::Removed { path: node.path })
                        .collect(),
                    Err(_) => Vec::new(),
                };
                self.vfs.unmount(&name);
                self.reset_fs_watcher();
                self.primary_ui_by_mount.remove(&name);
                self.pending_mount_root_splash_restarts.remove(&name);
                self.build_mount_by_id.retain(|_, mount| mount != &name);
                self.run_items_by_mount.remove(&name);
                let ai_state = self.ai_manager.remove_mount(&name);
                self.send_ui_reply(
                    client_id,
                    HubToClient::FileTree {
                        mount: name.clone(),
                        data: backend_proto::FileTreeData { nodes: Vec::new() },
                    },
                );
                self.send_ui_reply(
                    client_id,
                    HubToClient::FileTreeDiff {
                        mount: name.clone(),
                        changes,
                    },
                );
                self.send_ui_reply(
                    client_id,
                    HubToClient::AiMountState {
                        mount: name.clone(),
                        state: ai_state,
                    },
                );
            }
            ClientToHub::ObserveMount { mount, primary } => {
                let primary = primary.unwrap_or(true);
                if primary {
                    self.primary_ui_by_mount.insert(mount.clone(), client_id);
                    if let Err(err) = self.ensure_mount_root_splash_running(&mount) {
                        self.send_ui_error(client_id, err);
                    }
                } else if self.primary_ui_by_mount.get(&mount) == Some(&client_id) {
                    self.primary_ui_by_mount.remove(&mount);
                }
                if let Some(items) = self.run_items_by_mount.get(&mount).cloned() {
                    self.send_ui_reply(
                        client_id,
                        HubToClient::RunItems {
                            mount: mount.clone(),
                            items,
                        },
                    );
                }
                let state = self.ai_manager.get_state(&mount);
                self.send_ui_reply(client_id, HubToClient::AiMountState { mount, state });
            }
            ClientToHub::LoadFileTree { mount } => {
                self.enqueue_file_tree_load_for_client(mount, client_id);
            }
            ClientToHub::OpenTextFile { path } => match self.vfs.open_text_file(&path) {
                Ok(content) => self.send_ui_reply(
                    client_id,
                    HubToClient::TextFileOpened {
                        path,
                        content,
                        git_status: backend_proto::GitStatus::Unknown,
                        line: None,
                        column: None,
                    },
                ),
                Err(err) => self.send_ui_error(client_id, err.to_string()),
            },
            ClientToHub::ReadTextFile { path } => match self.vfs.read_text_file(&path) {
                Ok(content) => {
                    self.send_ui_reply(client_id, HubToClient::TextFileRead { path, content })
                }
                Err(err) => self.send_ui_error(client_id, err.to_string()),
            },
            ClientToHub::ReadTextRange {
                path,
                start_line,
                end_line,
            } => match self.vfs.read_text_range(&path, start_line, end_line) {
                Ok((content, total_lines)) => self.send_ui_reply(
                    client_id,
                    HubToClient::TextFileRange {
                        path,
                        start_line,
                        end_line,
                        total_lines,
                        content,
                    },
                ),
                Err(err) => self.send_ui_error(client_id, err.to_string()),
            },
            ClientToHub::SaveTextFile { path, content } => {
                let result = match self.vfs.save_text_file(&path, &content) {
                    Ok(()) => SaveResult::Ok,
                    Err(err) => SaveResult::Err(err.into()),
                };
                let save_ok = matches!(result, SaveResult::Ok);
                self.send_ui_reply(
                    client_id,
                    HubToClient::TextFileSaved {
                        path: path.clone(),
                        result,
                    },
                );
                if save_ok {
                    if path.ends_with(".rs") {
                        if let Ok(disk_path) = self.vfs.resolve_path(&path) {
                            let disk_path = disk_path
                                .canonicalize()
                                .unwrap_or_else(|_| disk_path.clone());
                            self.forward_live_change_to_builds(
                                "save",
                                &path,
                                disk_path.to_string_lossy().replace('\\', "/"),
                                content.clone(),
                            );
                        }
                    }
                    if let Some((mount, rest)) = path.split_once('/') {
                        if rest == MAKEPAD_SPLASH_RUNNABLE {
                            self.request_mount_root_splash_reload(mount);
                        }
                    }
                    self.self_save_suppress_until_by_path
                        .insert(path.clone(), Instant::now() + FS_SELF_SAVE_SUPPRESS);
                    self.broadcast_ui_message_except(
                        client_id,
                        HubToClient::FileChanged { path: path.clone() },
                    );
                    self.enqueue_file_tree_delta_for_virtual_path(&path);
                }
            }
            ClientToHub::DeleteFile { path } => {
                self.terminal_manager.close_terminal(&path);
                let disk_path = self.vfs.resolve_path(&path).ok();
                if let Err(err) = self.vfs.delete_path(&path) {
                    self.send_ui_error(client_id, err.to_string());
                } else if let Some(disk_path) = disk_path {
                    self.enqueue_file_tree_delta_for_known_path(&path, disk_path);
                }
            }
            ClientToHub::FindFiles {
                mount,
                pattern,
                is_regex: _,
                max_results,
            } => {
                self.cancelled_queries.remove(&query_id);
                let mount = mount.clone();
                let pattern = pattern.clone();
                let vfs = self.vfs.clone_for_search();
                let event_tx = self.event_tx.clone();
                self.worker_pool.execute(move || {
                    let result = vfs
                        .find_files(mount.as_deref(), &pattern, max_results)
                        .map_err(|err| err.to_string());
                    let _ = event_tx.send(HubEvent::WorkerFindFilesDone {
                        client_id,
                        query_id,
                        result,
                    });
                });
            }
            ClientToHub::SearchFiles {
                mount,
                pattern,
                is_regex,
                glob,
                max_results,
            }
            | ClientToHub::FindInFiles {
                mount,
                pattern,
                is_regex,
                glob,
                max_results,
            } => {
                self.cancelled_queries.remove(&query_id);
                let mount = mount.clone();
                let pattern = pattern.clone();
                let is_regex = is_regex.unwrap_or(false);
                let glob = glob.clone();
                let vfs = self.vfs.clone_for_search();
                let event_tx = self.event_tx.clone();
                let regex_search_pool = Arc::clone(&self.regex_search_pool);
                self.worker_pool.execute(move || {
                    let result = vfs
                        .find_in_files(
                            mount.as_deref(),
                            &pattern,
                            is_regex,
                            glob.as_deref(),
                            max_results,
                            if is_regex {
                                Some(regex_search_pool.as_ref())
                            } else {
                                None
                            },
                        )
                        .map_err(|err| err.to_string());
                    let _ = event_tx.send(HubEvent::WorkerFindInFilesDone {
                        client_id,
                        query_id,
                        result,
                    });
                });
            }
            ClientToHub::GitLog { mount, max_count } => {
                match self.vfs.git_log(&mount, max_count.unwrap_or(100)) {
                    Ok(log) => self.send_ui_reply(client_id, HubToClient::GitLog { mount, log }),
                    Err(err) => self.send_ui_error(client_id, err.to_string()),
                }
            }
            ClientToHub::CreateBranch {
                mount,
                name,
                from_ref,
            } => {
                let before = self.vfs.load_file_tree(&mount).ok();
                let result = self.vfs.create_branch(&mount, &name, from_ref.as_deref());
                self.send_branch_op_result(client_id, mount, before, result);
            }
            ClientToHub::DeleteBranch { mount, name } => {
                let before = self.vfs.load_file_tree(&mount).ok();
                let result = self.vfs.delete_branch(&mount, &name);
                self.send_branch_op_result(client_id, mount, before, result);
            }
            ClientToHub::ListBuilds => {
                self.send_ui_reply(
                    client_id,
                    HubToClient::Builds {
                        builds: self.list_all_builds(),
                    },
                );
            }
            ClientToHub::ListAppSockets => {
                self.send_ui_reply(
                    client_id,
                    HubToClient::AppSockets {
                        sockets: self.list_app_sockets(),
                    },
                );
            }
            ClientToHub::RunItem { mount, name } => {
                let build_id = self.alloc_build_id();
                if let Err(err) = self
                    .script_manager
                    .invoke_script_run_item(&mount, &name, build_id)
                {
                    self.send_ui_error(client_id, err);
                }
            }
            ClientToHub::Cargo {
                mount,
                args: raw_args,
                env,
                buildbox,
            } => {
                let args = with_default_cargo_message_format(raw_args);
                let build_id = self.alloc_build_id();
                if let Some(buildbox_name) = buildbox {
                    let package =
                        parse_package_name(&args).unwrap_or_else(|| "unknown".to_string());
                    let env = env.unwrap_or_default();
                    let msg = HubToBuildBox::CargoBuild {
                        build_id,
                        mount: mount.clone(),
                        args,
                        env,
                    };
                    if let Err(err) = self.send_to_buildbox_name(&buildbox_name, msg) {
                        self.send_ui_error(client_id, err);
                        return;
                    }

                    let info = BuildInfo {
                        build_id,
                        mount: mount.clone(),
                        package,
                        active: true,
                    };
                    self.remote_build_owner
                        .insert(build_id, buildbox_name.clone());
                    self.remote_builds.insert(build_id, info.clone());
                    self.build_mount_by_id.insert(build_id, mount);
                    self.set_buildbox_status(&buildbox_name, BuildBoxStatus::Building { build_id });
                    self.broadcast_ui_message(HubToClient::BuildStarted {
                        build_id: info.build_id,
                        mount: info.mount,
                        package: info.package,
                    });
                    return;
                }

                let cwd = match self.vfs.resolve_mount(&mount) {
                    Ok(cwd) => cwd,
                    Err(err) => {
                        self.send_ui_error(client_id, err.to_string());
                        return;
                    }
                };
                match self.build_manager.start_cargo_run(
                    build_id,
                    mount.clone(),
                    &cwd,
                    args,
                    env.unwrap_or_default(),
                    self.studio_addr.clone(),
                    self.event_tx.clone(),
                ) {
                    Ok(info) => {
                        self.build_mount_by_id
                            .insert(info.build_id, info.mount.clone());
                        self.broadcast_ui_message(HubToClient::BuildStarted {
                            build_id: info.build_id,
                            mount: info.mount,
                            package: info.package,
                        });
                    }
                    Err(err) => self.send_ui_error(client_id, err),
                }
            }
            ClientToHub::Run {
                mount,
                process,
                args: app_args,
                standalone,
                env,
                buildbox,
            } => {
                if process == MAKEPAD_SPLASH_RUNNABLE {
                    if buildbox.is_some() {
                        self.send_ui_error(
                            client_id,
                            "makepad.splash runs are not supported on buildboxes yet".to_string(),
                        );
                        return;
                    }
                    if env.as_ref().is_some_and(|env| !env.is_empty()) {
                        self.send_ui_error(
                            client_id,
                            "makepad.splash env overrides are not supported yet".to_string(),
                        );
                        return;
                    }
                    if standalone.unwrap_or(false) {
                        self.send_ui_error(
                            client_id,
                            "makepad.splash does not use standalone mode".to_string(),
                        );
                        return;
                    }
                    if !app_args.is_empty() {
                        self.send_ui_error(
                            client_id,
                            "makepad.splash args are not supported yet".to_string(),
                        );
                        return;
                    }

                    let cwd = match self.vfs.resolve_mount(&mount) {
                        Ok(cwd) => cwd,
                        Err(err) => {
                            self.send_ui_error(client_id, err.to_string());
                            return;
                        }
                    };
                    match self.script_manager.start_script(
                        mount.clone(),
                        &cwd,
                        self.studio_addr.clone(),
                        self.studio_ext_addr.clone(),
                        self.event_tx.clone(),
                    ) {
                        Ok(_) => {}
                        Err(err) => self.send_ui_error(client_id, err),
                    }
                    return;
                }

                let cargo_args =
                    build_run_cargo_args(&process, app_args, standalone.unwrap_or(false));
                let build_id = self.alloc_build_id();
                if let Some(buildbox_name) = buildbox {
                    let env = env.unwrap_or_default();
                    let msg = HubToBuildBox::CargoBuild {
                        build_id,
                        mount: mount.clone(),
                        args: cargo_args,
                        env,
                    };
                    if let Err(err) = self.send_to_buildbox_name(&buildbox_name, msg) {
                        self.send_ui_error(client_id, err);
                        return;
                    }

                    let info = BuildInfo {
                        build_id,
                        mount: mount.clone(),
                        package: process,
                        active: true,
                    };
                    self.remote_build_owner
                        .insert(build_id, buildbox_name.clone());
                    self.remote_builds.insert(build_id, info.clone());
                    self.build_mount_by_id.insert(build_id, mount);
                    self.set_buildbox_status(&buildbox_name, BuildBoxStatus::Building { build_id });
                    self.broadcast_ui_message(HubToClient::BuildStarted {
                        build_id: info.build_id,
                        mount: info.mount,
                        package: info.package,
                    });
                    return;
                }

                let cwd = match self.vfs.resolve_mount(&mount) {
                    Ok(cwd) => cwd,
                    Err(err) => {
                        self.send_ui_error(client_id, err.to_string());
                        return;
                    }
                };
                match self.build_manager.start_cargo_run(
                    build_id,
                    mount.clone(),
                    &cwd,
                    cargo_args,
                    env.unwrap_or_default(),
                    self.studio_addr.clone(),
                    self.event_tx.clone(),
                ) {
                    Ok(info) => {
                        self.build_mount_by_id
                            .insert(info.build_id, info.mount.clone());
                        self.broadcast_ui_message(HubToClient::BuildStarted {
                            build_id: info.build_id,
                            mount: info.mount,
                            package: info.package,
                        });
                    }
                    Err(err) => self.send_ui_error(client_id, err),
                }
            }
            ClientToHub::StopBuild { build_id } => {
                if self.build_manager.stop_build(build_id).is_ok() {
                    return;
                }
                let Some(buildbox_name) = self.remote_build_owner.get(&build_id).cloned() else {
                    self.send_ui_error(client_id, format!("unknown build: {}", build_id.0));
                    return;
                };
                if let Err(err) = self
                    .send_to_buildbox_name(&buildbox_name, HubToBuildBox::StopBuild { build_id })
                {
                    self.send_ui_error(client_id, err);
                }
            }
            ClientToHub::ClearBuild { build_id } => {
                if self.build_manager.stop_build(build_id).is_ok() {
                    self.send_build_cleanup_message(build_id);
                    return;
                }
                let Some(buildbox_name) = self.remote_build_owner.get(&build_id).cloned() else {
                    self.send_ui_error(client_id, format!("unknown build: {}", build_id.0));
                    return;
                };
                if let Err(err) = self
                    .send_to_buildbox_name(&buildbox_name, HubToBuildBox::StopBuild { build_id })
                {
                    self.send_ui_error(client_id, err);
                } else {
                    self.send_build_cleanup_message(build_id);
                }
            }
            ClientToHub::ForwardToApp { build_id, msg_bin } => {
                let parsed_msgs = StudioToAppVec::deserialize_bin(&msg_bin)
                    .ok()
                    .map(|msgs| msgs.0);
                let is_bootstrap = parsed_msgs.as_ref().is_some_and(|msgs| {
                    msgs.iter().any(|msg| {
                        matches!(
                            msg,
                            StudioToApp::WindowGeomChange { .. } | StudioToApp::Swapchain(_)
                        )
                    })
                });
                match self.send_to_app(build_id, msg_bin.clone()) {
                    Ok(()) => {}
                    Err(err) if err.starts_with("no app socket for build") => {
                        self.queue_pending_forward_to_app(build_id, msg_bin, is_bootstrap);
                    }
                    Err(err) => self.send_ui_error(client_id, err),
                }
            }
            ClientToHub::TypeText { build_id, text } => {
                if let Err(err) = self.send_app_msg(
                    build_id,
                    StudioToApp::TextInput(TextInputEvent {
                        input: text,
                        replace_last: false,
                        was_paste: false,
                        ..Default::default()
                    }),
                ) {
                    self.send_ui_error(client_id, err);
                } else {
                    self.send_runview_message(
                        build_id,
                        HubToClient::RunViewInputViz {
                            build_id,
                            kind: RunViewInputVizKind::TypeText,
                            x: None,
                            y: None,
                        },
                    );
                }
            }
            ClientToHub::Return {
                build_id,
                auto_dump: _,
            } => {
                let key = KeyEvent {
                    key_code: KeyCode::ReturnKey,
                    is_repeat: false,
                    modifiers: KeyModifiers::default(),
                    time: 0.0,
                };
                if let Err(err) = self.send_app_msgs(
                    build_id,
                    vec![StudioToApp::KeyDown(key), StudioToApp::KeyUp(key)],
                ) {
                    self.send_ui_error(client_id, err);
                } else {
                    self.send_runview_message(
                        build_id,
                        HubToClient::RunViewInputViz {
                            build_id,
                            kind: RunViewInputVizKind::Return,
                            x: None,
                            y: None,
                        },
                    );
                }
            }
            ClientToHub::Click { build_id, x, y } => {
                let mouse_down = RemoteMouseDown {
                    button_raw_bits: MouseButton::PRIMARY.bits(),
                    x: x as f64,
                    y: y as f64,
                    time: 0.0,
                    modifiers: RemoteKeyModifiers::default(),
                };
                let mouse_up = RemoteMouseUp {
                    button_raw_bits: MouseButton::PRIMARY.bits(),
                    x: x as f64,
                    y: y as f64,
                    time: 0.0,
                    modifiers: RemoteKeyModifiers::default(),
                };
                if let Err(err) = self.send_app_msgs(
                    build_id,
                    vec![
                        StudioToApp::MouseDown(mouse_down),
                        StudioToApp::MouseUp(mouse_up),
                    ],
                ) {
                    self.send_ui_error(client_id, err);
                } else {
                    let x = x as f64;
                    let y = y as f64;
                    self.send_runview_message(
                        build_id,
                        HubToClient::RunViewInputViz {
                            build_id,
                            kind: RunViewInputVizKind::ClickDown,
                            x: Some(x),
                            y: Some(y),
                        },
                    );
                    self.send_runview_message(
                        build_id,
                        HubToClient::RunViewInputViz {
                            build_id,
                            kind: RunViewInputVizKind::ClickUp,
                            x: Some(x),
                            y: Some(y),
                        },
                    );
                }
            }
            ClientToHub::Screenshot { build_id, kind_id } => {
                if let Err(err) = self.send_app_msg(
                    build_id,
                    StudioToApp::Screenshot(ScreenshotRequest {
                        request_id: query_id.0,
                        kind_id: kind_id.unwrap_or(0),
                    }),
                ) {
                    self.send_ui_error(client_id, err);
                }
            }
            ClientToHub::WidgetTreeDump { build_id } => {
                if let Err(err) = self.send_app_msg(
                    build_id,
                    StudioToApp::WidgetTreeDump(WidgetTreeDumpRequest {
                        request_id: query_id.0,
                    }),
                ) {
                    self.send_ui_error(client_id, err);
                }
            }
            ClientToHub::WidgetQuery { build_id, query } => {
                if let Err(err) = self.send_app_msg(
                    build_id,
                    StudioToApp::WidgetQuery(WidgetQueryRequest {
                        request_id: query_id.0,
                        query,
                    }),
                ) {
                    self.send_ui_error(client_id, err);
                }
            }
            ClientToHub::WidgetSnapshot { build_id } => {
                if let Err(err) = self.send_app_msg(
                    build_id,
                    StudioToApp::WidgetSnapshot(WidgetSnapshotRequest {
                        request_id: query_id.0,
                    }),
                ) {
                    self.send_ui_error(client_id, err);
                }
            }
            ClientToHub::RunViewInput {
                build_id,
                window_id,
                msg_bin,
            } => {
                let _ = window_id;
                if let Err(err) = self.send_to_app(build_id, msg_bin) {
                    self.send_ui_error(client_id, err);
                }
            }
            ClientToHub::RunViewResize {
                build_id,
                window_id,
                width,
                height,
                dpi,
            } => {
                if let Err(err) = self.send_app_msg(
                    build_id,
                    StudioToApp::WindowGeomChange {
                        window_id,
                        dpi_factor: dpi,
                        left: 0.0,
                        top: 0.0,
                        width,
                        height,
                    },
                ) {
                    self.send_ui_error(client_id, err);
                }
            }
            ClientToHub::TerminalOpen {
                path,
                cols,
                rows,
                env,
            } => match self.ensure_terminal_session_open(&path, cols, rows, env) {
                Ok(_opened_now) => {
                    self.send_ui_reply(
                        client_id,
                        HubToClient::TerminalOpened { path: path.clone() },
                    );
                    self.send_terminal_title_to_client(client_id, &path);
                    self.send_terminal_viewport_for_client(
                        client_id,
                        &path,
                        cols,
                        rows,
                        rows,
                        usize::MAX,
                    );
                    self.process_ai_terminal_observation_for_path(&path);
                }
                Err(err) => self.send_ui_error(client_id, err),
            },
            ClientToHub::TerminalInput { path, data } => {
                match self.terminal_manager.send_input(&path, data) {
                    Ok(()) => {
                        self.set_terminal_bell_state(&path, false);
                        self.process_ai_terminal_input_for_path(&path);
                    }
                    Err(err) => self.send_ui_error(client_id, err),
                }
            }
            ClientToHub::TerminalViewportRequest {
                path,
                cols,
                rows,
                pty_rows,
                top_row,
            } => {
                self.send_terminal_viewport_for_client(
                    client_id, &path, cols, rows, pty_rows, top_row,
                );
            }
            ClientToHub::TerminalClose { path } => {
                self.terminal_manager.close_terminal(&path);
            }
            ClientToHub::AiGetState { mount } => {
                let state = self.ai_manager.get_state(&mount);
                self.send_ui_reply(client_id, HubToClient::AiMountState { mount, state });
            }
            ClientToHub::AiCreateAgent { mount, title } => {
                let state = self.ai_manager.create_agent(&mount, title);
                self.broadcast_ui_message(HubToClient::AiMountState { mount, state });
            }
            ClientToHub::AiDeleteAgent { mount, agent_id } => {
                let state = self.ai_manager.delete_agent(&mount, agent_id);
                self.broadcast_ui_message(HubToClient::AiMountState { mount, state });
            }
            ClientToHub::AiSelectAgent { mount, agent_id } => {
                let state = self.ai_manager.select_agent(&mount, agent_id);
                self.broadcast_ui_message(HubToClient::AiMountState { mount, state });
            }
            ClientToHub::AiSetBackend { mount, backend_id } => {
                let state = self.ai_manager.set_backend(&mount, &backend_id);
                self.broadcast_ui_message(HubToClient::AiMountState { mount, state });
            }
            ClientToHub::AiSendPrompt {
                mount,
                agent_id,
                text,
            } => {
                let state = self.ai_manager.send_prompt(&mount, agent_id, &text);
                self.broadcast_ui_message(HubToClient::AiMountState { mount, state });
            }
            ClientToHub::AiCancelPrompt { mount, agent_id } => {
                let state = self.ai_manager.cancel_prompt(&mount, agent_id);
                self.broadcast_ui_message(HubToClient::AiMountState { mount, state });
            }
            ClientToHub::QueryLogs {
                build_id,
                level,
                source,
                file,
                pattern,
                is_regex: _,
                since_index,
                live,
            } => {
                let live = live.unwrap_or(false);
                let query = LogQuery {
                    build_id,
                    level,
                    source,
                    file,
                    pattern,
                    since_index,
                };
                self.cancelled_queries.remove(&query_id);
                let entries_handle = self.log_store.entries_handle();
                let event_tx = self.event_tx.clone();
                self.worker_pool.execute(move || {
                    let entries = {
                        let entries = entries_handle
                            .read()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        query_log_entries(&entries, &query)
                    };
                    let _ = event_tx.send(HubEvent::WorkerQueryLogsDone {
                        client_id,
                        query_id,
                        query,
                        live,
                        entries,
                    });
                });
            }
            ClientToHub::QueryProfiler {
                build_id,
                sample_type,
                time_start,
                time_end,
                max_samples,
                live,
            } => {
                let live = live.unwrap_or(false);
                let query = ProfilerQuery {
                    build_id,
                    sample_type,
                    time_start,
                    time_end,
                    max_samples,
                };
                let (event_samples, gpu_samples, gc_samples, total_in_window) =
                    self.profiler_store.query(&query);
                self.send_ui_reply(
                    client_id,
                    HubToClient::QueryProfilerResults {
                        query_id,
                        event_samples,
                        gpu_samples,
                        gc_samples,
                        total_in_window,
                        done: !live,
                    },
                );
                if live {
                    self.live_profiler_queries
                        .insert(query_id, LiveProfilerSubscription { client_id, query });
                }
            }
            ClientToHub::CancelQuery { query_id } => {
                self.cancelled_queries.insert(query_id);
                self.live_log_queries.remove(&query_id);
                self.live_profiler_queries.remove(&query_id);
                self.send_ui_reply(client_id, HubToClient::QueryCancelled { query_id });
            }
            ClientToHub::LogClear => {
                self.log_store.clear();
                self.send_ui_reply(client_id, HubToClient::LogCleared);
            }
            ClientToHub::ListBuildBoxes => {
                self.send_ui_reply(
                    client_id,
                    HubToClient::BuildBoxes {
                        boxes: self.list_buildboxes(),
                    },
                );
            }
            ClientToHub::BuildBoxSyncNow { name } => {
                if let Err(err) = self.send_to_buildbox_name(&name, HubToBuildBox::RequestTreeHash)
                {
                    self.send_ui_error(client_id, err);
                    return;
                }
                self.set_buildbox_status(&name, BuildBoxStatus::Syncing);
                self.send_ui_reply(
                    client_id,
                    HubToClient::BuildBoxes {
                        boxes: self.list_buildboxes(),
                    },
                );
            }
            ClientToHub::ListScriptTasks => {
                self.send_ui_reply(client_id, HubToClient::ScriptTasks { tasks: Vec::new() });
            }
            other => {
                self.send_ui_error(
                    client_id,
                    format!("message not implemented yet: {:?}", other),
                );
            }
        }
    }

    fn reset_fs_watcher(&mut self) {
        self.fs_watcher.take();
        self.fs_watch_events.clear();
        self.fs_event_last_by_path.clear();
        self.fs_recent_change_at_by_path.clear();
        self.fs_pending_diffs.clear();
        self.fs_pending_reload_mounts.clear();
        self.fs_diff_flush_scheduled = false;
        self.fs_event_last_prune = Instant::now();
        self.mount_suppress_fs_until.clear();
        self.self_save_suppress_until_by_path.clear();

        let roots: Vec<WatchRoot> = self
            .vfs
            .mounts()
            .into_iter()
            .map(|mount| WatchRoot {
                mount: mount.name,
                path: mount.path,
            })
            .collect();
        if roots.is_empty() {
            return;
        }

        let event_tx = self.event_tx.clone();
        let fs_watch_events = Arc::clone(&self.fs_watch_events);
        match FileSystemWatcher::start(roots, move |event| {
            if fs_watch_events.push(event.mount, event.path) {
                schedule_fs_event_flush(event_tx.clone());
            }
        }) {
            Ok(watcher) => {
                self.fs_watcher = Some(watcher);
            }
            Err(err) => {
                eprintln!("[studio2-backend] filesystem watcher unavailable: {}", err);
            }
        }
    }

    fn queue_mount_fs_changed(&mut self, mount: String, path: PathBuf) {
        if self.fs_watch_events.push(mount, path) {
            schedule_fs_event_flush(self.event_tx.clone());
        }
    }

    fn flush_pending_mount_fs_events(&mut self) {
        let pending = self.fs_watch_events.take_ready();
        if pending.is_empty() {
            return;
        }

        let mut by_mount: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for (mount, path) in pending {
            by_mount.entry(mount).or_default().push(path);
        }
        for (mount, paths) in by_mount {
            self.flush_pending_mount_fs_events_for_mount(mount, paths);
        }
    }

    fn flush_pending_mount_fs_events_for_mount(&mut self, mount: String, paths: Vec<PathBuf>) {
        let mut paths_to_process = Vec::with_capacity(paths.len());
        let mut saw_git_status_change = false;
        for path in paths {
            let Some(virtual_path) = self.mount_path_to_virtual(&mount, &path) else {
                paths_to_process.push(path);
                continue;
            };
            if self.is_git_status_watch_virtual_path(&mount, &virtual_path) {
                saw_git_status_change = true;
                continue;
            }
            if self.should_ignore_fs_watch_virtual_path(&mount, &virtual_path) {
                continue;
            }
            paths_to_process.push(path);
        }

        if saw_git_status_change {
            self.invalidate_git_status_cache_for_mount(&mount);
            self.reload_mount_file_tree_broadcast(&mount);
        }

        if paths_to_process.is_empty() {
            return;
        }
        paths_to_process.sort();
        paths_to_process.dedup();

        if paths_to_process.len() > FS_EVENT_BATCH_RELOAD_THRESHOLD {
            self.process_mount_fs_storm(&mount);
            return;
        }

        for path in paths_to_process {
            self.process_mount_fs_changed(mount.clone(), path);
        }
    }

    fn enqueue_file_tree_load_for_client(&mut self, mount: String, client_id: ClientId) {
        let mut waiters = HashSet::new();
        waiters.insert(client_id);
        self.enqueue_file_tree_load(mount, waiters);
    }

    fn enqueue_file_tree_load_for_all_clients(&mut self, mount: &str) {
        let waiters: HashSet<ClientId> = self.ui_clients.keys().copied().collect();
        self.enqueue_file_tree_load(mount.to_string(), waiters);
    }

    fn enqueue_file_tree_load(&mut self, mount: String, new_waiters: HashSet<ClientId>) {
        if new_waiters.is_empty() {
            return;
        }
        let waiters = self
            .file_tree_load_waiters
            .entry(mount.clone())
            .or_default();
        let first_request = waiters.is_empty();
        waiters.extend(new_waiters);
        if !first_request {
            return;
        }

        let mount_name = mount.clone();
        let vfs = self.vfs.clone_for_search();
        let event_tx = self.event_tx.clone();
        self.worker_pool.execute(move || {
            let result = vfs
                .load_file_tree(&mount_name)
                .map_err(|err| err.to_string());
            let _ = event_tx.send(HubEvent::WorkerLoadFileTreeDone {
                mount: mount_name,
                result,
            });
        });
    }

    fn process_mount_fs_changed(&mut self, mount: String, path: PathBuf) {
        let now = Instant::now();
        let path_is_file = path.is_file();
        let path_is_dir = path.is_dir();
        if self
            .mount_suppress_fs_until
            .get(&mount)
            .is_some_and(|until| now >= *until)
        {
            self.mount_suppress_fs_until.remove(&mount);
        }
        let Some(virtual_path) = self.mount_path_to_virtual(&mount, &path) else {
            self.reload_mount_file_tree_broadcast(&mount);
            return;
        };
        if self.is_git_status_watch_virtual_path(&mount, &virtual_path) {
            self.invalidate_git_status_cache_for_mount(&mount);
            self.reload_mount_file_tree_broadcast(&mount);
            return;
        }
        if self.should_ignore_fs_watch_virtual_path(&mount, &virtual_path) {
            return;
        }
        if virtual_path == mount {
            if self
                .mount_suppress_fs_until
                .get(&mount)
                .is_some_and(|until| now < *until)
            {
                return;
            }
            if self.should_suppress_self_save_mount_root_event(&mount, now) {
                return;
            }
            self.record_recent_fs_change(mount.clone(), now);
            self.process_ai_path_change(&mount, &mount);
            // Some watcher implementations only report "mount root changed".
            // Broadcast a mount-level FileChanged so UI can refresh open tabs.
            self.broadcast_ui_message(HubToClient::FileChanged {
                path: mount.clone(),
            });
            self.maybe_revive_mount_root_splash_from_fs_fallback(&mount);
            self.reload_mount_file_tree_broadcast(&mount);
            return;
        }
        if self.should_suppress_self_save_event(&virtual_path, now) {
            return;
        }
        self.record_recent_fs_change(virtual_path.clone(), now);
        self.process_ai_path_change(&mount, &virtual_path);
        if Self::is_mount_root_splash_virtual_path(&mount, &virtual_path) {
            self.request_mount_root_splash_reload(&mount);
        }
        if path_is_file && !self.should_ignore_virtual_path(&mount, &virtual_path) {
            self.broadcast_ui_message(HubToClient::FileChanged {
                path: virtual_path.clone(),
            });
            if virtual_path.ends_with(".rs") {
                if let Ok(content) = fs::read_to_string(&path) {
                    let file_name = path.canonicalize().unwrap_or_else(|_| path.clone());
                    self.forward_live_change_to_builds(
                        "watch",
                        &virtual_path,
                        file_name.to_string_lossy().replace('\\', "/"),
                        content,
                    );
                }
            }
        }
        if path_is_dir {
            self.maybe_revive_mount_root_splash_from_fs_fallback(&mount);
            self.reload_mount_file_tree_broadcast(&mount);
            return;
        }
        let (path, virtual_path) =
            self.collapse_removed_path_to_missing_ancestor(&mount, path, virtual_path);
        self.enqueue_file_tree_delta(&mount, &virtual_path, path, now);
    }

    fn process_mount_fs_storm(&mut self, mount: &str) {
        let now = Instant::now();
        self.record_recent_fs_change(mount.to_string(), now);
        self.process_ai_path_change(mount, mount);
        self.broadcast_ui_message(HubToClient::FileChanged {
            path: mount.to_string(),
        });
        self.maybe_revive_mount_root_splash_from_fs_fallback(mount);
        self.reload_mount_file_tree_broadcast(mount);
    }

    fn suppress_mount_root_fs_events(&mut self, mount: &str, duration: Duration) {
        let until = Instant::now() + duration;
        self.mount_suppress_fs_until
            .entry(mount.to_string())
            .and_modify(|existing| {
                if *existing < until {
                    *existing = until;
                }
            })
            .or_insert(until);
    }

    fn collapse_removed_path_to_missing_ancestor(
        &self,
        mount: &str,
        path: PathBuf,
        virtual_path: String,
    ) -> (PathBuf, String) {
        if path.exists() {
            return (path, virtual_path);
        }
        let mount_root = match self.vfs.resolve_mount(mount) {
            Ok(root) => root,
            Err(_) => return (path, virtual_path),
        };
        let mut probe = path.clone();
        let mut collapsed = None;
        loop {
            if !probe.starts_with(&mount_root) || probe.exists() {
                break;
            }
            collapsed = Some(probe.clone());
            if probe == mount_root || !probe.pop() {
                break;
            }
        }
        let Some(collapsed_path) = collapsed else {
            return (path, virtual_path);
        };
        let Some(collapsed_virtual) = self.mount_path_to_virtual(mount, &collapsed_path) else {
            return (path, virtual_path);
        };
        if collapsed_virtual == mount {
            return (path, virtual_path);
        }
        (collapsed_path, collapsed_virtual)
    }

    fn mount_path_to_virtual(&self, mount: &str, path: &Path) -> Option<String> {
        let mount_root = self.vfs.resolve_mount(mount).ok()?;
        let path = path
            .strip_prefix(&mount_root)
            .ok()
            .map(Path::to_path_buf)
            .or_else(|| {
                #[cfg(target_os = "macos")]
                {
                    let normalized_mount_root = normalize_macos_private_alias(&mount_root);
                    let normalized_path = normalize_macos_private_alias(path);
                    normalized_path
                        .strip_prefix(&normalized_mount_root)
                        .ok()
                        .map(Path::to_path_buf)
                }
                #[cfg(not(target_os = "macos"))]
                {
                    None
                }
            })?;
        if path.as_os_str().is_empty() {
            return Some(mount.to_string());
        }
        let path_string = path.to_string_lossy().replace('\\', "/");
        if let Some(rest) = path_string.strip_prefix("branch/") {
            if let Some((branch, tail)) = rest.split_once('/') {
                let encoded = percent_encode_local(branch);
                return Some(format!("{}/@{}/{}", mount, encoded, tail));
            }
            let encoded = percent_encode_local(rest);
            return Some(format!("{}/@{}", mount, encoded));
        }
        Some(format!("{}/{}", mount, path_string))
    }

    fn enqueue_file_tree_delta_for_virtual_path(&mut self, virtual_path: &str) {
        let Some((_mount, _)) = virtual_path.split_once('/') else {
            return;
        };
        let disk_path = match self.vfs.resolve_path(virtual_path) {
            Ok(path) => path,
            Err(_) => return,
        };
        self.enqueue_file_tree_delta_for_known_path(virtual_path, disk_path);
    }

    fn enqueue_file_tree_delta_for_known_path(&mut self, virtual_path: &str, disk_path: PathBuf) {
        let Some((mount, _)) = virtual_path.split_once('/') else {
            return;
        };
        self.enqueue_file_tree_delta(mount, virtual_path, disk_path, Instant::now());
    }

    fn enqueue_file_tree_delta(
        &mut self,
        mount: &str,
        virtual_path: &str,
        disk_path: PathBuf,
        now: Instant,
    ) {
        if self.should_ignore_virtual_path(mount, virtual_path) {
            return;
        }
        self.prune_fs_event_history(now);
        if let Some(last) = self.fs_event_last_by_path.get(virtual_path).copied() {
            if now.saturating_duration_since(last) < FS_EVENT_PATH_DEBOUNCE {
                return;
            }
        }
        self.fs_event_last_by_path
            .insert(virtual_path.to_string(), now);

        if let Ok(mut cache_guard) = self.git_status_cache.lock() {
            cache_guard.entries.remove(&disk_path);
        }

        let mount = mount.to_string();
        let virtual_path = virtual_path.to_string();
        let event_tx = self.event_tx.clone();
        let git_status_cache = Arc::clone(&self.git_status_cache);
        self.worker_pool.execute(move || {
            let change =
                compute_filetree_change_for_path(&git_status_cache, &disk_path, virtual_path);
            let _ = event_tx.send(HubEvent::WorkerFileTreeDeltaDone { mount, change });
        });
    }

    fn invalidate_git_status_cache_for_mount(&mut self, mount: &str) {
        let Ok(root) = self.vfs.resolve_mount(mount) else {
            return;
        };
        if let Ok(mut cache_guard) = self.git_status_cache.lock() {
            cache_guard
                .entries
                .retain(|path, _| !path.starts_with(&root));
        }
    }

    fn is_git_status_watch_virtual_path(&self, mount: &str, virtual_path: &str) -> bool {
        let prefix = format!("{}/", mount);
        let Some(rest) = virtual_path.strip_prefix(&prefix) else {
            return false;
        };
        rest == ".git" || rest.starts_with(".git/")
    }

    fn should_ignore_fs_watch_virtual_path(&self, mount: &str, virtual_path: &str) -> bool {
        let prefix = format!("{}/", mount);
        let Some(rest) = virtual_path.strip_prefix(&prefix) else {
            return false;
        };
        rest == ".git"
            || rest.starts_with(".git/")
            || rest == ".makepad"
            || rest.starts_with(".makepad/")
    }

    fn should_ignore_virtual_path(&self, mount: &str, virtual_path: &str) -> bool {
        if virtual_path == mount {
            return true;
        }
        let prefix = format!("{}/", mount);
        let Some(rest) = virtual_path.strip_prefix(&prefix) else {
            return true;
        };
        rest == "target"
            || rest.starts_with("target/")
            || rest == ".git"
            || rest.starts_with(".git/")
            || rest == ".makepad"
            || rest.starts_with(".makepad/")
    }

    fn reload_mount_file_tree_broadcast(&mut self, mount: &str) {
        let now = Instant::now();
        self.prune_fs_event_history(now);
        let reload_key = format!("__mount_reload__/{}", mount);
        if let Some(last) = self.fs_event_last_by_path.get(&reload_key).copied() {
            if now.saturating_duration_since(last) < FS_EVENT_RELOAD_DEBOUNCE {
                // Don't drop the reload: re-queue it so bursty fs events still
                // produce one eventual tree refresh after debounce.
                self.fs_pending_reload_mounts.insert(mount.to_string());
                self.schedule_fs_diff_flush();
                return;
            }
        }
        self.fs_event_last_by_path.insert(reload_key, now);
        self.enqueue_file_tree_load_for_all_clients(mount);
    }

    fn queue_file_tree_delta_change(
        &mut self,
        mount: String,
        change: backend_proto::FileTreeChange,
    ) {
        if self.fs_pending_reload_mounts.contains(&mount) {
            self.schedule_fs_diff_flush();
            return;
        }
        let pending = self.fs_pending_diffs.entry(mount.clone()).or_default();
        coalesce_file_tree_change(pending, change);
        if pending.len() >= FS_DELTA_RELOAD_THRESHOLD {
            self.fs_pending_diffs.remove(&mount);
            self.fs_pending_reload_mounts.insert(mount);
        }
        self.schedule_fs_diff_flush();
    }

    fn schedule_fs_diff_flush(&mut self) {
        if self.fs_diff_flush_scheduled {
            return;
        }
        self.fs_diff_flush_scheduled = true;
        let event_tx = self.event_tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(FS_DELTA_FLUSH_DELAY);
            let _ = event_tx.send(HubEvent::FlushPendingFileTreeDiffs);
        });
    }

    fn flush_pending_file_tree_diffs(&mut self) {
        self.fs_diff_flush_scheduled = false;

        let reload_mounts: Vec<String> = self.fs_pending_reload_mounts.drain().collect();
        for mount in reload_mounts {
            self.reload_mount_file_tree_broadcast(&mount);
        }

        let pending = std::mem::take(&mut self.fs_pending_diffs);
        for (mount, mut changes) in pending {
            if changes.is_empty() {
                continue;
            }
            changes.sort_by(|a, b| file_tree_change_path(a).cmp(file_tree_change_path(b)));
            self.broadcast_ui_message(HubToClient::FileTreeDiff { mount, changes });
        }
    }

    fn prune_fs_event_history(&mut self, now: Instant) {
        if now.saturating_duration_since(self.fs_event_last_prune) < FS_EVENT_HISTORY_PRUNE_INTERVAL
        {
            return;
        }
        self.fs_event_last_prune = now;
        self.fs_event_last_by_path
            .retain(|_, ts| now.saturating_duration_since(*ts) < FS_EVENT_HISTORY_RETENTION);
        self.fs_recent_change_at_by_path
            .retain(|_, ts| now.saturating_duration_since(*ts) < FS_RECENT_CHANGE_RETENTION);
        self.self_save_suppress_until_by_path
            .retain(|_, until| *until > now);
    }

    fn record_recent_fs_change(&mut self, path: String, now: Instant) {
        self.fs_recent_change_at_by_path.insert(path, now);
    }

    fn should_suppress_self_save_event(&mut self, virtual_path: &str, now: Instant) -> bool {
        self.self_save_suppress_until_by_path
            .retain(|_, until| *until > now);
        self.self_save_suppress_until_by_path
            .get(virtual_path)
            .is_some_and(|until| now < *until)
    }

    fn should_suppress_self_save_mount_root_event(&mut self, mount: &str, now: Instant) -> bool {
        self.self_save_suppress_until_by_path
            .retain(|_, until| *until > now);
        let mount_prefix = format!("{}/", mount);
        self.self_save_suppress_until_by_path
            .iter()
            .any(|(path, until)| now < *until && path.starts_with(&mount_prefix))
    }

    fn on_worker_find_files_done(
        &mut self,
        client_id: ClientId,
        query_id: QueryId,
        result: Result<Vec<String>, String>,
    ) {
        if self.cancelled_queries.remove(&query_id) {
            return;
        }

        match result {
            Ok(paths) => self.send_ui_reply(
                client_id,
                HubToClient::FindFileResults {
                    query_id,
                    paths,
                    done: true,
                },
            ),
            Err(err) => self.send_ui_error(client_id, err),
        }
    }

    fn on_worker_find_in_files_done(
        &mut self,
        client_id: ClientId,
        query_id: QueryId,
        result: Result<Vec<SearchResult>, String>,
    ) {
        if self.cancelled_queries.remove(&query_id) {
            return;
        }

        match result {
            Ok(results) => self.send_ui_reply(
                client_id,
                HubToClient::SearchFileResults {
                    query_id,
                    results,
                    done: true,
                },
            ),
            Err(err) => self.send_ui_error(client_id, err),
        }
    }

    fn on_worker_query_logs_done(
        &mut self,
        client_id: ClientId,
        query_id: QueryId,
        query: LogQuery,
        live: bool,
        entries: Vec<(usize, LogEntry)>,
    ) {
        if self.cancelled_queries.remove(&query_id) {
            return;
        }

        self.send_ui_reply(
            client_id,
            HubToClient::QueryLogResults {
                query_id,
                entries,
                done: !live,
            },
        );

        if live && self.ui_clients.contains_key(&client_id) {
            self.live_log_queries
                .insert(query_id, LiveLogSubscription { client_id, query });
        }
    }

    fn on_worker_load_file_tree_done(
        &mut self,
        mount: String,
        result: Result<backend_proto::FileTreeData, String>,
    ) {
        let waiters = self
            .file_tree_load_waiters
            .remove(&mount)
            .unwrap_or_default();
        if waiters.is_empty() {
            return;
        }
        match result {
            Ok(data) => {
                for client_id in waiters {
                    self.send_ui_reply(
                        client_id,
                        HubToClient::FileTree {
                            mount: mount.clone(),
                            data: data.clone(),
                        },
                    );
                }
            }
            Err(err) => {
                for client_id in waiters {
                    self.send_ui_error(client_id, err.clone());
                }
            }
        }
    }

    fn send_to_app_with_socket(&self, build_id: QueryId, msg_bin: Vec<u8>) -> Result<u64, String> {
        let mut candidates: Vec<(u64, Sender<Vec<u8>>)> = self
            .app_sockets
            .iter()
            .filter_map(|(web_socket_id, socket)| {
                (socket.build_id == Some(build_id))
                    .then_some((*web_socket_id, socket.sender.clone()))
            })
            .collect();
        candidates.sort_by_key(|(web_socket_id, _)| *web_socket_id);
        let socket_ids = candidates
            .iter()
            .map(|(web_socket_id, _)| *web_socket_id)
            .collect::<Vec<_>>();
        let Some((socket_id, sender)) = candidates.pop() else {
            return Err(format!("no app socket for build {}", build_id.0));
        };
        sender.send(msg_bin).map_err(|_| {
            format!(
                "failed to send app message for build {} socket={} sockets_for_build={:?}",
                build_id.0, socket_id, socket_ids
            )
        })?;
        Ok(socket_id)
    }

    fn send_to_process_stdin(&self, build_id: QueryId, msg_bin: Vec<u8>) -> Result<(), String> {
        let msgs = StudioToAppVec::deserialize_bin(&msg_bin)
            .map_err(|err| format!("failed to decode app payload: {}", err.msg))?;
        for msg in msgs.0 {
            let mut line = msg.serialize_json();
            line.push('\n');
            self.build_manager.send_stdin(build_id, &line)?;
        }
        Ok(())
    }

    fn queue_pending_forward_to_app(
        &mut self,
        build_id: QueryId,
        msg_bin: Vec<u8>,
        is_bootstrap: bool,
    ) {
        // Before an app socket exists, only bootstrap packets matter for RunView bring-up.
        // Dropping pre-socket Tick/input traffic avoids queue churn and stale replays.
        if !is_bootstrap {
            return;
        }
        let queue = self
            .pending_forward_to_app_by_build
            .entry(build_id)
            .or_default();
        if let Some(existing) = queue.first() {
            if let Some(merged) = Self::merge_pending_bootstrap_msgs(existing, &msg_bin) {
                queue.clear();
                queue.push(merged);
                return;
            }
        }
        queue.clear();
        queue.push(msg_bin);
    }

    fn merge_pending_bootstrap_msgs(existing: &[u8], incoming: &[u8]) -> Option<Vec<u8>> {
        let existing = StudioToAppVec::deserialize_bin(existing).ok()?.0;
        let incoming = StudioToAppVec::deserialize_bin(incoming).ok()?.0;

        let mut window_geom = None;
        let mut swapchain = None;
        let mut frame_request = None;
        let mut saw_tick = false;

        for msg in existing.into_iter().chain(incoming.into_iter()) {
            match msg {
                StudioToApp::WindowGeomChange { .. } => window_geom = Some(msg),
                StudioToApp::Swapchain(_) => swapchain = Some(msg),
                StudioToApp::RunViewFrameRequest(request) => frame_request = Some(request),
                StudioToApp::Tick => saw_tick = true,
                _ => {}
            }
        }

        let mut merged = Vec::new();
        if let Some(msg) = window_geom {
            merged.push(msg);
        }
        if let Some(msg) = swapchain {
            merged.push(msg);
        }
        if let Some(request) = frame_request {
            merged.push(StudioToApp::RunViewFrameRequest(request));
        }
        if saw_tick {
            merged.push(StudioToApp::Tick);
        }
        (!merged.is_empty()).then_some(StudioToAppVec(merged).serialize_bin())
    }

    fn flush_pending_forward_to_app(&mut self, build_id: QueryId) {
        let Some(mut pending) = self.pending_forward_to_app_by_build.remove(&build_id) else {
            return;
        };
        while let Some(msg_bin) = pending.first().cloned() {
            match self.send_to_app(build_id, msg_bin) {
                Ok(()) => {
                    pending.remove(0);
                }
                Err(_) => {
                    self.pending_forward_to_app_by_build
                        .insert(build_id, pending);
                    return;
                }
            }
        }
    }

    fn send_to_app(&self, build_id: QueryId, msg_bin: Vec<u8>) -> Result<(), String> {
        if self.stdio_ready_builds.contains(&build_id) {
            if studio_hub_debug_enabled() {
                eprintln!(
                    "studio hub debug: forwarding build {} to stdio bridge",
                    build_id.0
                );
            }
            return self.send_to_process_stdin(build_id, msg_bin);
        }
        self.send_to_app_with_socket(build_id, msg_bin).map(|_| ())
    }

    fn build_ids_for_virtual_path(&self, virtual_path: &str) -> Vec<QueryId> {
        let mut build_ids = HashSet::new();
        for (build_id, mount) in &self.build_mount_by_id {
            if Self::virtual_path_matches_build_mount(virtual_path, mount) {
                build_ids.insert(*build_id);
            }
        }
        let mut build_ids: Vec<QueryId> = build_ids.into_iter().collect();
        build_ids.sort_by_key(|build_id| build_id.0);
        build_ids
    }

    fn virtual_path_matches_build_mount(virtual_path: &str, build_mount: &str) -> bool {
        if virtual_path == build_mount {
            return true;
        }
        let Some(rest) = virtual_path.strip_prefix(build_mount) else {
            return false;
        };
        let Some(rest) = rest.strip_prefix('/') else {
            return false;
        };
        let build_is_branch = build_mount
            .split('/')
            .nth(1)
            .is_some_and(|segment| segment.starts_with('@'));
        if !build_is_branch && rest.starts_with('@') {
            return false;
        }
        true
    }

    fn forward_live_change_to_builds(
        &self,
        _source: &str,
        virtual_path: &str,
        file_name: String,
        content: String,
    ) {
        let build_ids = self.build_ids_for_virtual_path(virtual_path);
        if build_ids.is_empty() {
            return;
        }
        for build_id in build_ids {
            if let Err(err) = self.send_app_msg(
                build_id,
                StudioToApp::LiveChange {
                    file_name: file_name.clone(),
                    content: content.clone(),
                },
            ) {
                if err.starts_with("no app socket for build ") {
                    continue;
                }
                eprintln!(
                    "[studio-hotreload] failed build={} virtual_path={} error={}",
                    build_id.0, virtual_path, err
                );
            }
        }
    }

    fn send_app_msg(&self, build_id: QueryId, msg: StudioToApp) -> Result<(), String> {
        self.send_to_app(build_id, StudioToAppVec(vec![msg]).serialize_bin())
    }

    fn send_app_msgs(&self, build_id: QueryId, msgs: Vec<StudioToApp>) -> Result<(), String> {
        self.send_to_app(build_id, StudioToAppVec(msgs).serialize_bin())
    }

    fn send_to_buildbox_name(&self, name: &str, msg: HubToBuildBox) -> Result<(), String> {
        let Some(web_socket_id) = self.buildbox_by_name.get(name).copied() else {
            return Err(format!("buildbox '{}' is not connected", name));
        };
        let Some(socket) = self.buildbox_sockets.get(&web_socket_id) else {
            return Err(format!("buildbox '{}' socket is missing", name));
        };
        socket
            .sender
            .send(HubToBuildBoxVec(vec![msg]).serialize_bin())
            .map_err(|_| format!("failed to send message to buildbox '{}'", name))
    }

    fn list_buildboxes(&self) -> Vec<BuildBoxInfo> {
        let mut boxes: Vec<BuildBoxInfo> = self
            .buildbox_sockets
            .values()
            .filter_map(|socket| socket.info.clone())
            .collect();
        boxes.sort_by(|a, b| a.name.cmp(&b.name));
        boxes
    }

    fn list_all_builds(&self) -> Vec<BuildInfo> {
        let mut builds = self.build_manager.list_builds();
        builds.extend(self.remote_builds.values().cloned());
        builds.sort_by_key(|build| build.build_id.0);
        builds
    }

    fn build_info_for_id(&self, build_id: QueryId) -> Option<BuildInfo> {
        self.build_manager
            .list_builds()
            .into_iter()
            .find(|build| build.build_id == build_id)
            .or_else(|| self.remote_builds.get(&build_id).cloned())
    }

    fn list_app_sockets(&self) -> Vec<AppSocketInfo> {
        let mut sockets = self
            .app_sockets
            .iter()
            .map(|(web_socket_id, socket)| {
                let build_info = socket
                    .build_id
                    .and_then(|build_id| self.build_info_for_id(build_id));
                AppSocketInfo {
                    web_socket_id: *web_socket_id,
                    build_id: socket.build_id,
                    crate_name: socket
                        .crate_name
                        .clone()
                        .or_else(|| build_info.as_ref().map(|info| info.package.clone()))
                        .or_else(|| socket.package.clone()),
                    mount: build_info
                        .as_ref()
                        .map(|info| info.mount.clone())
                        .or_else(|| socket.mount.clone()),
                    package: build_info
                        .as_ref()
                        .map(|info| info.package.clone())
                        .or_else(|| socket.package.clone()),
                    build_active: build_info.as_ref().map(|info| info.active).unwrap_or(false),
                }
            })
            .collect::<Vec<_>>();
        sockets.sort_by_key(|socket| {
            (
                socket.crate_name.clone().unwrap_or_default(),
                socket.build_id.map(|id| id.0).unwrap_or(u64::MAX),
                socket.web_socket_id,
            )
        });
        sockets
    }

    fn mount_has_root_splash(&self, mount: &str) -> bool {
        self.vfs
            .resolve_mount(mount)
            .map(|cwd| cwd.join(MAKEPAD_SPLASH_RUNNABLE).is_file())
            .unwrap_or(false)
    }

    fn is_mount_root_splash_virtual_path(mount: &str, virtual_path: &str) -> bool {
        virtual_path == format!("{}/{}", mount, MAKEPAD_SPLASH_RUNNABLE)
    }

    fn mount_root_splash_running(&self, mount: &str) -> bool {
        self.script_manager.is_running_for_mount(mount)
    }

    fn ensure_mount_root_splash_running(&mut self, mount: &str) -> Result<bool, String> {
        if !self.mount_has_root_splash(mount) || self.mount_root_splash_running(mount) {
            return Ok(false);
        }

        let cwd = self
            .vfs
            .resolve_mount(mount)
            .map_err(|err| err.to_string())?;
        self.script_manager.start_script(
            mount.to_string(),
            &cwd,
            self.studio_addr.clone(),
            self.studio_ext_addr.clone(),
            self.event_tx.clone(),
        )?;
        Ok(true)
    }

    fn start_mount_root_splash_with_reporting(&mut self, mount: &str) {
        if let Err(err) = self.ensure_mount_root_splash_running(mount) {
            if let Some(client_id) = self.primary_ui_for_mount(mount) {
                self.send_ui_error(client_id, err);
            } else {
                eprintln!(
                    "[studio2-backend] failed to start {} for mount {}: {}",
                    MAKEPAD_SPLASH_RUNNABLE, mount, err
                );
            }
        }
    }

    fn maybe_revive_mount_root_splash_from_fs_fallback(&mut self, mount: &str) {
        if self.mount_root_splash_running(mount) {
            return;
        }
        if self.primary_ui_for_mount(mount).is_none() || !self.mount_has_root_splash(mount) {
            return;
        }
        self.start_mount_root_splash_with_reporting(mount);
    }

    fn request_mount_root_splash_reload(&mut self, mount: &str) {
        if !self.mount_root_splash_running(mount) {
            if self.primary_ui_for_mount(mount).is_some() && self.mount_has_root_splash(mount) {
                self.start_mount_root_splash_with_reporting(mount);
            }
            return;
        }

        if self.mount_has_root_splash(mount) {
            self.pending_mount_root_splash_restarts
                .insert(mount.to_string());
        } else {
            self.pending_mount_root_splash_restarts.remove(mount);
        }

        if let Err(err) = self.script_manager.stop_script_for_mount(mount) {
            if let Some(client_id) = self.primary_ui_for_mount(mount) {
                self.send_ui_error(client_id, err);
            } else {
                eprintln!(
                    "[studio2-backend] failed to stop {} for mount {}: {}",
                    MAKEPAD_SPLASH_RUNNABLE, mount, err
                );
            }
        }
    }

    fn maybe_restart_pending_mount_root_splash(&mut self, mount: &str) {
        if !self.pending_mount_root_splash_restarts.remove(mount) {
            return;
        }
        if self.mount_root_splash_running(mount) || !self.mount_has_root_splash(mount) {
            if self.mount_has_root_splash(mount) {
                self.pending_mount_root_splash_restarts
                    .insert(mount.to_string());
            }
            return;
        }
        self.start_mount_root_splash_with_reporting(mount);
    }

    fn primary_ui_for_mount(&self, mount: &str) -> Option<ClientId> {
        let client_id = self.primary_ui_by_mount.get(mount).copied()?;
        self.ui_clients
            .contains_key(&client_id)
            .then_some(client_id)
    }

    fn primary_ui_for_build(&self, build_id: QueryId) -> Option<ClientId> {
        let mount = self.build_mount_by_id.get(&build_id)?;
        self.primary_ui_for_mount(mount)
    }

    fn send_runview_message(&self, build_id: QueryId, msg: HubToClient) {
        if let Some(client_id) = self.primary_ui_for_build(build_id) {
            self.send_ui_message(client_id, msg, self.ui_format(client_id));
        } else {
            self.broadcast_ui_message(msg);
        }
    }

    fn send_build_cleanup_message(&self, build_id: QueryId) {
        let msg = HubToClient::BuildCleared { build_id };
        if let Some(client_id) = self.primary_ui_for_build(build_id) {
            self.send_ui_reply(client_id, msg);
        } else {
            self.broadcast_ui_message(msg);
        }
    }

    fn on_run_items_updated(&mut self, mount: String, items: Vec<RunItem>) {
        self.run_items_by_mount.insert(mount.clone(), items.clone());
        self.broadcast_ui_message(HubToClient::RunItems { mount, items });
    }

    fn on_script_run_request(
        &mut self,
        child_build_id: Option<QueryId>,
        mount: String,
        cwd: PathBuf,
        program: String,
        args: Vec<String>,
        env: HashMap<String, String>,
        package: Option<String>,
    ) {
        let build_id = child_build_id.unwrap_or_else(|| self.alloc_build_id());
        let package = package.unwrap_or_else(|| display_name_from_command(&program, &args));
        match self.build_manager.start_command_run(
            build_id,
            mount.clone(),
            package.clone(),
            &cwd,
            program,
            args,
            env,
            false,
            self.studio_addr.clone(),
            self.event_tx.clone(),
        ) {
            Ok(info) => {
                self.build_mount_by_id
                    .insert(info.build_id, info.mount.clone());
                self.broadcast_ui_message(HubToClient::BuildStarted {
                    build_id: info.build_id,
                    mount: info.mount,
                    package: info.package,
                });
            }
            Err(err) => {
                if let Some(client_id) = self.primary_ui_for_mount(&mount) {
                    self.send_ui_error(client_id, err);
                } else {
                    eprintln!(
                        "[studio2-backend] failed to start scripted run for mount {}: {}",
                        mount, err
                    );
                }
            }
        }
    }

    fn set_buildbox_status(&mut self, name: &str, status: BuildBoxStatus) {
        let Some(web_socket_id) = self.buildbox_by_name.get(name).copied() else {
            return;
        };
        let Some(socket) = self.buildbox_sockets.get_mut(&web_socket_id) else {
            return;
        };
        if let Some(info) = socket.info.as_mut() {
            info.status = status;
        }
        self.broadcast_ui_message(HubToClient::BuildBoxes {
            boxes: self.list_buildboxes(),
        });
    }

    fn on_buildbox_disconnected(&mut self, web_socket_id: u64) {
        let Some(socket) = self.buildbox_sockets.remove(&web_socket_id) else {
            return;
        };
        let Some(info) = socket.info else {
            return;
        };

        self.buildbox_by_name.remove(&info.name);
        self.broadcast_ui_message(HubToClient::BuildBoxDisconnected {
            name: info.name.clone(),
        });

        let affected_build_ids: Vec<QueryId> = self
            .remote_build_owner
            .iter()
            .filter_map(|(build_id, owner)| (owner == &info.name).then_some(*build_id))
            .collect();
        for build_id in affected_build_ids {
            self.remote_build_owner.remove(&build_id);
            self.remote_builds.remove(&build_id);
            self.build_mount_by_id.remove(&build_id);
            self.broadcast_ui_message(HubToClient::BuildStopped {
                build_id,
                exit_code: None,
            });
        }

        self.broadcast_ui_message(HubToClient::BuildBoxes {
            boxes: self.list_buildboxes(),
        });
    }

    fn on_buildbox_binary(&mut self, web_socket_id: u64, data: Vec<u8>) {
        let messages = match BuildBoxToHubVec::deserialize_bin(&data) {
            Ok(messages) => messages.0,
            Err(err) => {
                let (index, entry) = self.log_store.append(AppendLogEntry {
                    build_id: None,
                    level: LogLevel::Warning,
                    source: LogSource::BuildBox,
                    message: format!("failed to decode buildbox message: {}", err.msg),
                    file_name: None,
                    line: None,
                    column: None,
                    timestamp: None,
                });
                self.broadcast_live_log_entry(index, entry);
                return;
            }
        };

        for msg in messages {
            self.handle_buildbox_message(web_socket_id, msg);
        }
    }

    fn handle_buildbox_message(&mut self, web_socket_id: u64, msg: BuildBoxToHub) {
        match msg {
            BuildBoxToHub::Hello {
                name,
                platform,
                arch,
                tree_hash,
            } => {
                let info = BuildBoxInfo {
                    name: name.clone(),
                    platform,
                    arch,
                    status: BuildBoxStatus::Idle,
                };
                if let Some(socket) = self.buildbox_sockets.get_mut(&web_socket_id) {
                    socket.info = Some(info.clone());
                    socket.tree_hash = Some(tree_hash);
                }
                self.buildbox_by_name.insert(name.clone(), web_socket_id);
                self.broadcast_ui_message(HubToClient::BuildBoxConnected { info });
                self.broadcast_ui_message(HubToClient::BuildBoxes {
                    boxes: self.list_buildboxes(),
                });
            }
            BuildBoxToHub::BuildOutput { build_id, line } => {
                let (index, entry) = self.log_store.append(AppendLogEntry {
                    build_id: Some(build_id),
                    level: LogLevel::Log,
                    source: LogSource::BuildBox,
                    message: line,
                    file_name: None,
                    line: None,
                    column: None,
                    timestamp: None,
                });
                self.broadcast_live_log_entry(index, entry);
            }
            BuildBoxToHub::BuildStarted { build_id } => {
                if let Some(buildbox_name) = self.remote_build_owner.get(&build_id).cloned() {
                    self.set_buildbox_status(&buildbox_name, BuildBoxStatus::Building { build_id });
                }
            }
            BuildBoxToHub::BuildStopped {
                build_id,
                exit_code,
            } => {
                if let Some(buildbox_name) = self.remote_build_owner.remove(&build_id) {
                    self.remote_builds.remove(&build_id);
                    self.set_buildbox_status(&buildbox_name, BuildBoxStatus::Idle);
                }
                self.build_mount_by_id.remove(&build_id);
                self.broadcast_ui_message(HubToClient::BuildStopped {
                    build_id,
                    exit_code,
                });
            }
            BuildBoxToHub::SyncComplete { tree_hash } => {
                if let Some(socket) = self.buildbox_sockets.get_mut(&web_socket_id) {
                    socket.tree_hash = Some(tree_hash);
                    if let Some(info) = socket.info.as_mut() {
                        info.status = BuildBoxStatus::Idle;
                    }
                }
                self.broadcast_ui_message(HubToClient::BuildBoxes {
                    boxes: self.list_buildboxes(),
                });
            }
            BuildBoxToHub::SyncError { error } => {
                let (index, entry) = self.log_store.append(AppendLogEntry {
                    build_id: None,
                    level: LogLevel::Warning,
                    source: LogSource::BuildBox,
                    message: format!("buildbox sync error: {}", error),
                    file_name: None,
                    line: None,
                    column: None,
                    timestamp: None,
                });
                self.broadcast_live_log_entry(index, entry);
            }
            BuildBoxToHub::Pong => {}
            BuildBoxToHub::FileHashes { .. } => {}
        }
    }

    fn on_app_binary(&mut self, build_id: QueryId, data: Vec<u8>) {
        let messages = match AppToStudioVec::deserialize_bin(&data) {
            Ok(messages) => messages.0,
            Err(err) => {
                let (index, entry) = self.log_store.append(AppendLogEntry {
                    build_id: Some(build_id),
                    level: LogLevel::Warning,
                    source: LogSource::ChildApp,
                    message: format!("failed to decode app message: {}", err.msg),
                    file_name: None,
                    line: None,
                    column: None,
                    timestamp: None,
                });
                self.broadcast_live_log_entry(index, entry);
                return;
            }
        };

        for msg in messages {
            self.handle_app_message(build_id, msg);
        }
    }

    fn on_process_app_message(&mut self, build_id: QueryId, msg: AppToStudio) {
        if studio_hub_debug_enabled() {
            eprintln!(
                "studio hub debug: process app message build {} variant {:?}",
                build_id.0, msg
            );
        }
        self.stdio_ready_builds.insert(build_id);
        self.flush_pending_forward_to_app(build_id);
        self.handle_app_message(build_id, msg);
    }

    fn handle_app_message(&mut self, build_id: QueryId, msg: AppToStudio) {
        match msg {
            AppToStudio::LogItem(item) => {
                let (index, entry) = self.log_store.append(AppendLogEntry {
                    build_id: Some(build_id),
                    level: map_platform_log_level(item.level),
                    source: LogSource::ChildApp,
                    message: item.message,
                    file_name: Some(item.file_name),
                    line: Some((item.line_start as usize).saturating_add(1)),
                    column: Some((item.column_start as usize).saturating_add(1)),
                    timestamp: None,
                });
                self.broadcast_live_log_entry(index, entry);
            }
            AppToStudio::EventSample(sample) => {
                self.profiler_store
                    .append_event(Some(build_id), map_platform_event_sample(sample));
                self.broadcast_live_profiler_queries();
            }
            AppToStudio::GPUSample(sample) => {
                self.profiler_store
                    .append_gpu(Some(build_id), map_platform_gpu_sample(sample));
                self.broadcast_live_profiler_queries();
            }
            AppToStudio::GCSample(sample) => {
                self.profiler_store
                    .append_gc(Some(build_id), map_platform_gc_sample(sample));
                self.broadcast_live_profiler_queries();
            }
            AppToStudio::Screenshot(response) => {
                for request_id in response.request_ids {
                    let query_id = QueryId(request_id);
                    match write_screenshot_png(build_id, 0, request_id, &response.png) {
                        Ok(path) => self.send_to_query_owner(
                            query_id,
                            HubToClient::Screenshot {
                                query_id,
                                build_id,
                                kind_id: 0,
                                path,
                                width: response.width,
                                height: response.height,
                            },
                        ),
                        Err(err) => self.send_to_query_owner(
                            query_id,
                            HubToClient::Error {
                                message: format!("failed to persist screenshot: {}", err),
                            },
                        ),
                    }
                }
            }
            AppToStudio::RunViewFrame(frame) => {
                self.send_runview_message(
                    build_id,
                    HubToClient::RunViewFrame {
                        build_id,
                        window_id: frame.window_id,
                        frame_id: frame.frame_id,
                        width: frame.width,
                        height: frame.height,
                        codec: frame.codec.unwrap_or(backend_proto::FrameCodec::Png),
                        data: frame.data,
                    },
                );
            }
            AppToStudio::RunViewKeyFocusRect(rect) => {
                self.send_runview_message(
                    build_id,
                    HubToClient::RunViewKeyFocusRect {
                        build_id,
                        x: rect.x,
                        y: rect.y,
                        width: rect.width,
                        height: rect.height,
                    },
                );
            }
            AppToStudio::WidgetTreeDump(response) => {
                let query_id = QueryId(response.request_id);
                self.send_to_query_owner(
                    query_id,
                    HubToClient::WidgetTreeDump {
                        query_id,
                        build_id,
                        dump: response.dump,
                    },
                );
            }
            AppToStudio::WidgetQuery(response) => {
                let query_id = QueryId(response.request_id);
                self.send_to_query_owner(
                    query_id,
                    HubToClient::WidgetQuery {
                        query_id,
                        build_id,
                        query: response.query,
                        rects: response.rects,
                    },
                );
            }
            AppToStudio::WidgetSnapshot(response) => {
                let query_id = QueryId(response.request_id);
                self.send_to_query_owner(
                    query_id,
                    HubToClient::WidgetSnapshot {
                        query_id,
                        build_id,
                        widgets: response.widgets,
                    },
                );
            }
            AppToStudio::CreateWindow {
                window_id,
                kind_id: _,
            } => {
                self.send_runview_message(
                    build_id,
                    HubToClient::RunViewCreated {
                        build_id,
                        window_id,
                    },
                );
            }
            AppToStudio::AfterStartup => {
                self.broadcast_ui_message(HubToClient::AppStarted { build_id });
            }
            AppToStudio::SetCursor(cursor) => {
                self.send_runview_message(
                    build_id,
                    HubToClient::RunViewCursor {
                        build_id,
                        cursor: format!("{:?}", cursor),
                    },
                );
            }
            AppToStudio::DrawCompleteAndFlip(presentable_draw) => {
                self.send_runview_message(
                    build_id,
                    HubToClient::RunViewDrawComplete {
                        build_id,
                        window_id: presentable_draw.window_id,
                        presentable_draw,
                    },
                );
            }
            AppToStudio::Custom(message) => {
                let (index, entry) = self.log_store.append(AppendLogEntry {
                    build_id: Some(build_id),
                    level: LogLevel::Log,
                    source: LogSource::ChildApp,
                    message,
                    file_name: None,
                    line: None,
                    column: None,
                    timestamp: None,
                });
                self.broadcast_live_log_entry(index, entry);
            }
            AppToStudio::JumpToFile(_)
            | AppToStudio::SelectInFile(_)
            | AppToStudio::PatchFile(_)
            | AppToStudio::EditFile(_)
            | AppToStudio::SwapSelection(_)
            | AppToStudio::TweakHits(_)
            | AppToStudio::BeforeStartup
            | AppToStudio::RequestAnimationFrame
            | AppToStudio::SetClipboard(_) => {}
        }
    }

    fn on_script_output(
        &mut self,
        _script_id: ScriptId,
        _mount: String,
        is_stderr: bool,
        line: String,
    ) {
        if line.is_empty() {
            return;
        }
        let (index, entry) = self.log_store.append(AppendLogEntry {
            build_id: None,
            level: if is_stderr {
                LogLevel::Error
            } else {
                LogLevel::Log
            },
            source: LogSource::Studio,
            message: line,
            file_name: None,
            line: None,
            column: None,
            timestamp: None,
        });
        self.broadcast_live_log_entry(index, entry);
    }

    fn on_script_exited(&mut self, script_id: ScriptId, mount: String, exit_code: Option<i32>) {
        if self
            .script_manager
            .mark_exited(script_id, exit_code)
            .is_none()
        {
            return;
        }
        self.run_items_by_mount.insert(mount.clone(), Vec::new());
        self.broadcast_ui_message(HubToClient::RunItems {
            mount: mount.clone(),
            items: Vec::new(),
        });
        self.maybe_restart_pending_mount_root_splash(&mount);
    }

    fn on_process_output(&mut self, build_id: QueryId, is_stderr: bool, line: String) {
        if line.is_empty() {
            return;
        }
        match parse_cargo_output_line(&line) {
            ParsedCargoOutputLine::Structured(parsed) => {
                let (index, entry) = self.log_store.append(AppendLogEntry {
                    build_id: Some(build_id),
                    level: parsed.level,
                    source: LogSource::Cargo,
                    message: parsed.message,
                    file_name: parsed.file_name,
                    line: parsed.line,
                    column: parsed.column,
                    timestamp: None,
                });
                self.broadcast_live_log_entry(index, entry);
            }
            ParsedCargoOutputLine::IgnoredStructured => {
                // Ignore non-diagnostic cargo json lines (artifacts, summaries, etc).
            }
            ParsedCargoOutputLine::RawText => {
                let level = classify_cargo_log_line(is_stderr, &line);
                let (index, entry) = self.log_store.append(AppendLogEntry {
                    build_id: Some(build_id),
                    level,
                    source: LogSource::Cargo,
                    message: line,
                    file_name: None,
                    line: None,
                    column: None,
                    timestamp: None,
                });
                self.broadcast_live_log_entry(index, entry);
            }
        }
    }
    fn on_process_exited(&mut self, build_id: QueryId, exit_code: Option<i32>) {
        if self
            .build_manager
            .mark_exited(build_id, exit_code)
            .is_none()
        {
            return;
        };
        self.stdio_ready_builds.remove(&build_id);
        self.build_mount_by_id.remove(&build_id);
        self.broadcast_ui_message(HubToClient::BuildStopped {
            build_id,
            exit_code,
        });
    }

    fn on_terminal_output(&mut self, path: String, data: Vec<u8>) {
        if data.is_empty() {
            return;
        }
        let mount = match self.terminal_manager.mount_for_path(&path) {
            Some(mount) => mount.to_string(),
            None => return,
        };
        // Terminal history is persisted into .makepad/*.term and can trigger file
        // watcher churn. Suppress those self-induced fs events briefly so typing
        // in terminal does not force repeated file-tree reloads.
        self.mount_suppress_fs_until
            .insert(mount, Instant::now() + Duration::from_millis(750));
        let mut force_bottom_for_sticky = true;
        let mut bell_rang = false;
        if let Some(session) = self.terminal_sessions.get_mut(&path) {
            let old_total_rows = {
                let screen = session.terminal.screen();
                screen.scrollback_len() + screen.used_rows()
            };
            session.terminal.process_bytes(&data);
            bell_rang = session.terminal.take_bell();
            let outbound = session.terminal.take_outbound();
            if !outbound.is_empty() {
                let _ = self.terminal_manager.send_input(&path, outbound);
            }
            let new_total_rows = {
                let screen = session.terminal.screen();
                screen.scrollback_len() + screen.used_rows()
            };
            // Only auto-stick to bottom when output actually extends history.
            // TUI redraw bursts mostly rewrite in-place and should not force a
            // viewport jump during rapid resize sequences.
            force_bottom_for_sticky = new_total_rows > old_total_rows;
        }
        if bell_rang {
            self.set_terminal_bell_state(&path, true);
        }
        self.push_terminal_frame_updates(&path, force_bottom_for_sticky);
        self.process_ai_terminal_observation_for_path(&path);
        // Persist terminal history off the dispatch thread so fs I/O cannot
        // block terminal framebuffer delivery.
        let history_vfs = self.vfs.clone_for_search();
        let history_path = path.clone();
        let history_data = data;
        self.io_worker_pool.execute(move || {
            let _ = append_terminal_history_bytes(&history_vfs, &history_path, &history_data);
        });
    }

    fn on_terminal_resized(&mut self, path: String, cols: u16, rows: u16) {
        if let Some(session) = self.terminal_sessions.get_mut(&path) {
            let cols = cols.max(1);
            let rows = rows.max(1);
            if session.applied_cols == cols && session.applied_rows == rows {
                return;
            }
            session.applied_cols = cols;
            session.applied_rows = rows;
            session.terminal.resize(cols as usize, rows as usize);
            Self::adjust_terminal_subscribers_for_resize(session);
            if cols != session.cols || rows != session.rows {
                if self
                    .terminal_manager
                    .resize(&path, session.cols, session.rows)
                    .is_err()
                {
                    // Ignore retry errors here; primary resize request path reports
                    // user-visible errors.
                };
            }
            self.push_terminal_frame_updates(&path, false);
            self.process_ai_terminal_observation_for_path(&path);
        }
    }

    fn on_terminal_exited(&mut self, path: String, exit_code: i32) {
        let mount = self.terminal_manager.remove_terminal(&path);
        self.terminal_sessions.remove(&path);
        self.broadcast_ui_message(HubToClient::TerminalExited {
            path: path.clone(),
            code: exit_code,
        });
        if let Some(mount) = mount {
            if let Some(state) = self
                .ai_manager
                .process_terminal_closed(&mount, &path, exit_code)
            {
                self.broadcast_ui_message(HubToClient::AiMountState { mount, state });
            }
        }
    }

    fn ensure_terminal_session_open(
        &mut self,
        path: &str,
        cols: u16,
        rows: u16,
        env: HashMap<String, String>,
    ) -> Result<bool, String> {
        if self.terminal_sessions.contains_key(path) {
            return Ok(false);
        }
        let Some(mount) = mount_from_virtual_path(path).map(ToOwned::to_owned) else {
            return Err(format!("invalid terminal path (missing mount): {}", path));
        };
        let cwd = self
            .vfs
            .resolve_mount(&mount)
            .map_err(|err| err.to_string())?;
        let history = self
            .vfs
            .resolve_path(path)
            .ok()
            .and_then(|disk_path| fs::read(disk_path).ok())
            .unwrap_or_default();
        self.terminal_manager.open_terminal(
            path.to_string(),
            mount,
            &cwd,
            cols,
            rows,
            env,
            self.event_tx.clone(),
        )?;
        let cols = cols.max(1);
        let rows = rows.max(1);
        let mut terminal = Terminal::new(cols as usize, rows as usize);
        if !history.is_empty() {
            terminal.process_bytes(&history);
            let _ = terminal.take_outbound();
        }
        self.terminal_sessions.insert(
            path.to_string(),
            TerminalSession {
                terminal,
                cols,
                rows,
                applied_cols: cols,
                applied_rows: rows,
                frame_seq: 0,
                bell_pending: false,
                subscribers: HashMap::new(),
            },
        );
        Ok(true)
    }

    fn on_ai_open_terminal_request(
        &mut self,
        mount: String,
        name: Option<String>,
        command: Option<String>,
        cols: u16,
        rows: u16,
        reply_tx: Sender<Result<String, String>>,
    ) {
        let result = self.open_ai_terminal(&mount, name.as_deref(), command.as_deref(), cols, rows);
        let _ = reply_tx.send(result);
    }

    fn on_ai_open_editor_request(
        &mut self,
        mount: String,
        path: String,
        line: Option<usize>,
        column: Option<usize>,
        reply_tx: Sender<Result<String, String>>,
    ) {
        let result = self.open_ai_editor(&mount, &path, line, column);
        let _ = reply_tx.send(result);
    }

    fn on_ai_observe_filesystem_request(
        &mut self,
        mount: String,
        path: Option<String>,
        limit: usize,
        since_secs: u64,
        reply_tx: Sender<Result<String, String>>,
    ) {
        let result = self.observe_ai_filesystem(&mount, path.as_deref(), limit, since_secs);
        let _ = reply_tx.send(result);
    }

    fn open_ai_terminal(
        &mut self,
        mount: &str,
        name: Option<&str>,
        command: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> Result<String, String> {
        let path = self.next_ai_terminal_path(mount, name, command)?;
        self.vfs
            .save_text_file(&path, "")
            .map_err(|err| err.to_string())?;
        self.self_save_suppress_until_by_path
            .insert(path.clone(), Instant::now() + FS_SELF_SAVE_SUPPRESS);
        let _ = self.ensure_terminal_session_open(&path, cols, rows, HashMap::new())?;
        self.broadcast_ui_message(HubToClient::TerminalOpened { path: path.clone() });
        if let Some(command) = command.map(str::trim).filter(|command| !command.is_empty()) {
            self.terminal_manager
                .send_input(&path, format!("{}\n", command).into_bytes())?;
        }
        self.process_ai_terminal_observation_for_path(&path);
        Ok(self.ai_terminal_info(&path)?.serialize_json())
    }

    fn open_ai_editor(
        &mut self,
        mount: &str,
        path: &str,
        line: Option<usize>,
        column: Option<usize>,
    ) -> Result<String, String> {
        if mount_from_virtual_path(path) != Some(mount) {
            return Err(format!(
                "editor path '{}' does not belong to mount '{}'",
                path, mount
            ));
        }
        let Some(client_id) = self.primary_ui_for_mount(mount) else {
            return Err(format!(
                "no primary Studio UI is observing mount '{}'",
                mount
            ));
        };
        let content = self
            .vfs
            .open_text_file(path)
            .map_err(|err| err.to_string())?;
        let line = line.map(|value| value.max(1));
        let column = column.map(|value| value.max(1));
        self.send_ui_reply(
            client_id,
            HubToClient::TextFileOpened {
                path: path.to_string(),
                content,
                git_status: backend_proto::GitStatus::Unknown,
                line,
                column,
            },
        );
        if let Some(line) = line {
            Ok(format!(
                "Opened {} at {}:{} in Studio editor.",
                path,
                line,
                column.unwrap_or(1)
            ))
        } else {
            Ok(format!("Opened {} in Studio editor.", path))
        }
    }

    fn observe_ai_filesystem(
        &mut self,
        mount: &str,
        path_filter: Option<&str>,
        limit: usize,
        since_secs: u64,
    ) -> Result<String, String> {
        let now = Instant::now();
        self.prune_fs_event_history(now);

        let normalized_filter = path_filter
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != ".")
            .map(|value| value.trim_matches('/').to_string());
        let since = Duration::from_secs(since_secs.max(1));
        let mount_prefix = format!("{}/", mount);

        let mut changes = self
            .fs_recent_change_at_by_path
            .iter()
            .filter_map(|(virtual_path, observed_at)| {
                let age = now.saturating_duration_since(*observed_at);
                if age > since {
                    return None;
                }
                let (relative_path, kind) = if virtual_path == mount {
                    (".".to_string(), "mount".to_string())
                } else if let Some(rest) = virtual_path.strip_prefix(&mount_prefix) {
                    (rest.to_string(), "path".to_string())
                } else {
                    return None;
                };
                if let Some(filter) = normalized_filter.as_deref() {
                    if relative_path == "." {
                        return None;
                    }
                    let exact = relative_path == filter;
                    let within = relative_path.starts_with(&format!("{}/", filter));
                    if !exact && !within {
                        return None;
                    }
                }
                Some((
                    *observed_at,
                    AiFilesystemChange {
                        path: relative_path,
                        kind,
                        seconds_ago: age.as_secs_f64(),
                    },
                ))
            })
            .collect::<Vec<_>>();
        changes.sort_by(|a, b| b.0.cmp(&a.0));

        Ok(AiFilesystemObserveResult {
            mount: mount.to_string(),
            path_filter: normalized_filter,
            since_secs,
            changes: changes
                .into_iter()
                .take(limit.max(1))
                .map(|(_, change)| change)
                .collect(),
        }
        .serialize_json())
    }

    fn on_ai_list_terminals_request(
        &mut self,
        mount: String,
        reply_tx: Sender<Result<String, String>>,
    ) {
        let result = self.list_ai_terminals(&mount);
        let _ = reply_tx.send(result);
    }

    fn on_ai_read_terminal_request(
        &mut self,
        mount: String,
        path: String,
        rows: Option<u16>,
        top_row: Option<usize>,
        reply_tx: Sender<Result<String, String>>,
    ) {
        let result = self.read_ai_terminal(&mount, &path, rows, top_row);
        let _ = reply_tx.send(result);
    }

    fn on_ai_send_terminal_text_request(
        &mut self,
        mount: String,
        path: String,
        text: String,
        submit: Option<bool>,
        bracketed_paste: Option<bool>,
        reply_tx: Sender<Result<String, String>>,
    ) {
        let result = self.send_ai_terminal_text(&mount, &path, &text, submit, bracketed_paste);
        let _ = reply_tx.send(result);
    }

    fn on_ai_send_terminal_key_request(
        &mut self,
        mount: String,
        path: String,
        key: String,
        shift: bool,
        control: bool,
        alt: bool,
        reply_tx: Sender<Result<String, String>>,
    ) {
        let result = self.send_ai_terminal_key(&mount, &path, &key, shift, control, alt);
        let _ = reply_tx.send(result);
    }

    fn list_ai_terminals(&self, mount: &str) -> Result<String, String> {
        let mut terminals = self
            .terminal_sessions
            .iter()
            .filter(|(path, _)| self.terminal_manager.mount_for_path(path.as_str()) == Some(mount))
            .map(|(path, _)| self.ai_terminal_info(path))
            .collect::<Result<Vec<_>, String>>()?;
        terminals.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(terminals.serialize_json())
    }

    fn ai_terminal_observation(
        &self,
        path: &str,
        rows: Option<u16>,
        top_row: Option<usize>,
    ) -> Result<AiTerminalObservation, String> {
        let session = self
            .terminal_sessions
            .get(path)
            .ok_or_else(|| format!("unknown terminal: {}", path))?;
        let frame = terminal_framebuffer_from_terminal(
            &session.terminal,
            session.cols.max(1),
            rows.unwrap_or(session.rows).max(1),
            top_row.unwrap_or(usize::MAX),
            session.frame_seq,
        );
        Ok(AiTerminalObservation {
            path: path.to_string(),
            terminal_title: session.terminal.title.clone(),
            cols: frame.cols,
            rows: frame.rows,
            top_row: frame.top_row,
            total_lines: frame.total_lines,
            is_tui: frame.is_tui,
            text: terminal_framebuffer_text(&frame),
        })
    }

    fn process_ai_terminal_observation_for_path(&mut self, path: &str) {
        let Some(mount) = self
            .terminal_manager
            .mount_for_path(path)
            .map(str::to_string)
        else {
            return;
        };
        let Ok(observation) = self.ai_terminal_observation(path, None, Some(usize::MAX)) else {
            return;
        };
        if let Some(state) = self
            .ai_manager
            .process_terminal_observation(&mount, observation)
        {
            self.broadcast_ui_message(HubToClient::AiMountState { mount, state });
        }
    }

    fn process_ai_terminal_input_for_path(&mut self, path: &str) {
        let Some(mount) = self
            .terminal_manager
            .mount_for_path(path)
            .map(str::to_string)
        else {
            return;
        };
        if let Some(state) = self.ai_manager.process_terminal_input(&mount, path) {
            self.broadcast_ui_message(HubToClient::AiMountState { mount, state });
        }
    }

    fn process_ai_path_change(&mut self, mount: &str, virtual_path: &str) {
        if let Some(state) = self.ai_manager.process_path_change(mount, virtual_path) {
            self.broadcast_ui_message(HubToClient::AiMountState {
                mount: mount.to_string(),
                state,
            });
        }
    }

    fn read_ai_terminal(
        &self,
        mount: &str,
        path: &str,
        rows: Option<u16>,
        top_row: Option<usize>,
    ) -> Result<String, String> {
        self.ensure_ai_terminal_access(mount, path)?;
        let observation = self.ai_terminal_observation(path, rows, top_row)?;
        let (mode, is_codex, summary, codex_status) =
            AiManager::terminal_mode_and_summary(&observation.terminal_title, &observation.text);
        let session = self
            .terminal_sessions
            .get(path)
            .ok_or_else(|| format!("unknown terminal: {}", path))?;
        let frame = terminal_framebuffer_from_terminal(
            &session.terminal,
            observation.cols,
            observation.rows,
            observation.top_row,
            session.frame_seq,
        );
        Ok(AiTerminalReadResult {
            path: path.to_string(),
            name: Self::terminal_display_name(path),
            terminal_title: observation.terminal_title,
            cols: observation.cols,
            rows: observation.rows,
            top_row: observation.top_row,
            total_lines: observation.total_lines,
            cursor_col: frame.cursor_col,
            cursor_row: frame.cursor_row,
            cursor_visible: frame.cursor_visible,
            is_tui: observation.is_tui,
            mode: mode.to_string(),
            summary,
            is_codex,
            codex_status,
            bracketed_paste: frame.bracketed_paste,
            cursor_keys_application_mode: frame.cursor_keys_application_mode,
            text: observation.text,
        }
        .serialize_json())
    }

    fn send_ai_terminal_text(
        &mut self,
        mount: &str,
        path: &str,
        text: &str,
        submit: Option<bool>,
        bracketed_paste: Option<bool>,
    ) -> Result<String, String> {
        self.ensure_ai_terminal_access(mount, path)?;
        let (bracketed_paste, submit, submit_bytes) = {
            let session = self
                .terminal_sessions
                .get(path)
                .ok_or_else(|| format!("unknown terminal: {}", path))?;
            let visible_text = {
                let frame = terminal_framebuffer_from_terminal(
                    &session.terminal,
                    session.cols.max(1),
                    session.rows.max(1),
                    usize::MAX,
                    0,
                );
                terminal_framebuffer_text(&frame)
            };
            let bracketed_paste = bracketed_paste
                .unwrap_or(session.terminal.modes.bracketed_paste && text.contains('\n'));
            let submit = submit.unwrap_or(false)
                || Self::terminal_auto_submit_ai_text(
                    path,
                    &session.terminal.title,
                    &visible_text,
                    text,
                );
            let submit_bytes = if submit {
                session
                    .terminal
                    .encode_key(TermKeyCode::Return, "", false, false, false)
                    .or_else(|| Some(vec![b'\n']))
            } else {
                None
            };
            (bracketed_paste, submit, submit_bytes)
        };
        if text.is_empty() && !submit {
            return Err("send_terminal_text requires non-empty text or submit=true".to_string());
        }
        let mut bytes = Vec::with_capacity(text.len() + 16);
        if bracketed_paste {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend_from_slice(text.as_bytes());
        if bracketed_paste {
            bytes.extend_from_slice(b"\x1b[201~");
        }
        if !bytes.is_empty() {
            self.terminal_manager.send_input(path, bytes.clone())?;
        }
        let submit_len = if let Some(submit_bytes) = submit_bytes {
            let len = submit_bytes.len();
            if bytes.is_empty() {
                self.terminal_manager.send_input(path, submit_bytes)?;
            } else {
                self.terminal_manager.send_input_delayed(
                    path,
                    submit_bytes,
                    AI_TERMINAL_SUBMIT_DELAY,
                )?;
            }
            len
        } else {
            0
        };
        self.set_terminal_bell_state(path, false);
        self.process_ai_terminal_input_for_path(path);
        let preview_source = if submit {
            format!("{}<enter>", text)
        } else {
            text.to_string()
        };
        Ok(AiTerminalInputResult {
            path: path.to_string(),
            name: Self::terminal_display_name(path),
            bytes_sent: bytes.len() + submit_len,
            submitted: submit,
            bracketed_paste,
            preview: preview_text(&preview_source),
        }
        .serialize_json())
    }

    fn send_ai_terminal_key(
        &mut self,
        mount: &str,
        path: &str,
        key: &str,
        shift: bool,
        control: bool,
        alt: bool,
    ) -> Result<String, String> {
        self.ensure_ai_terminal_access(mount, path)?;
        let spec = parse_ai_terminal_key_spec(key, shift, control, alt)?;
        let bytes = {
            let session = self
                .terminal_sessions
                .get(path)
                .ok_or_else(|| format!("unknown terminal: {}", path))?;
            encode_ai_terminal_key(&session.terminal, &spec)
                .ok_or_else(|| format!("unsupported terminal key '{}'", key))?
        };
        self.terminal_manager.send_input(path, bytes.clone())?;
        self.set_terminal_bell_state(path, false);
        self.process_ai_terminal_input_for_path(path);
        Ok(AiTerminalInputResult {
            path: path.to_string(),
            name: Self::terminal_display_name(path),
            bytes_sent: bytes.len(),
            submitted: false,
            bracketed_paste: false,
            preview: preview_text(key),
        }
        .serialize_json())
    }

    fn ai_terminal_info(&self, path: &str) -> Result<AiTerminalInfo, String> {
        let session = self
            .terminal_sessions
            .get(path)
            .ok_or_else(|| format!("unknown terminal: {}", path))?;
        let observation = self.ai_terminal_observation(path, None, Some(usize::MAX))?;
        let (mode, is_codex, summary, codex_status) =
            AiManager::terminal_mode_and_summary(&observation.terminal_title, &observation.text);
        Ok(AiTerminalInfo {
            path: path.to_string(),
            name: Self::terminal_display_name(path),
            terminal_title: session.terminal.title.clone(),
            mode: mode.to_string(),
            summary,
            is_codex,
            codex_status,
            cols: session.cols,
            rows: session.rows,
            is_tui: session.terminal.modes.alt_screen
                || session.terminal.screen().scroll_top != 0
                || session.terminal.screen().scroll_bottom != session.terminal.screen().rows(),
            bracketed_paste: session.terminal.modes.bracketed_paste,
            cursor_keys_application_mode: session.terminal.modes.cursor_keys,
            bell_pending: session.bell_pending,
        })
    }

    fn ensure_ai_terminal_access(&self, mount: &str, path: &str) -> Result<(), String> {
        match self.terminal_manager.mount_for_path(path) {
            Some(actual_mount) if actual_mount == mount => Ok(()),
            Some(actual_mount) => Err(format!(
                "terminal '{}' belongs to mount '{}', not '{}'",
                path, actual_mount, mount
            )),
            None => Err(format!("unknown terminal: {}", path)),
        }
    }

    fn terminal_display_name(path: &str) -> String {
        path.rsplit('/').next().unwrap_or(path).to_string()
    }

    fn terminal_auto_submit_ai_text(
        path: &str,
        terminal_title: &str,
        visible_text: &str,
        text: &str,
    ) -> bool {
        if text.trim().is_empty() || text.ends_with('\n') || text.ends_with('\r') {
            return false;
        }
        let haystack = format!("{}\n{}\n{}", path, terminal_title, visible_text).to_lowercase();
        haystack.contains("codex")
            || haystack.contains("claude")
            || haystack.contains("aider")
            || haystack.contains("enter a prompt")
            || haystack.contains("esc to interrupt")
    }

    fn next_ai_terminal_path(
        &self,
        mount: &str,
        name: Option<&str>,
        command: Option<&str>,
    ) -> Result<String, String> {
        self.vfs
            .resolve_mount(mount)
            .map_err(|err| err.to_string())?;
        if let Some(stem) = name
            .and_then(sanitize_terminal_stem)
            .or_else(|| command.and_then(terminal_stem_from_command))
        {
            return self.unique_ai_terminal_path(mount, &stem);
        }
        for index in 0usize.. {
            let stem = if index < 26 {
                ((b'a' + index as u8) as char).to_string()
            } else {
                format!("t{}", index + 1)
            };
            let path = format!("{}/.makepad/{}.term", mount, stem);
            if !self.is_terminal_path_taken(&path) {
                return Ok(path);
            }
        }
        Err("failed to allocate terminal path".to_string())
    }

    fn unique_ai_terminal_path(&self, mount: &str, stem: &str) -> Result<String, String> {
        for index in 0usize.. {
            let file_name = if index == 0 {
                format!("{}.term", stem)
            } else {
                format!("{}-{}.term", stem, index + 1)
            };
            let path = format!("{}/.makepad/{}", mount, file_name);
            if !self.is_terminal_path_taken(&path) {
                return Ok(path);
            }
        }
        Err("failed to allocate named terminal path".to_string())
    }

    fn is_terminal_path_taken(&self, path: &str) -> bool {
        self.terminal_sessions.contains_key(path)
            || self
                .vfs
                .resolve_path(path)
                .map(|disk_path| disk_path.exists())
                .unwrap_or(false)
    }

    fn send_terminal_viewport_for_client(
        &mut self,
        client_id: ClientId,
        path: &str,
        cols: u16,
        rows: u16,
        pty_rows: u16,
        top_row: usize,
    ) {
        if !self.terminal_sessions.contains_key(path) {
            self.send_ui_error(client_id, format!("unknown terminal: {}", path));
            return;
        }
        let cols = cols.max(1);
        let rows = rows.max(1);
        let pty_rows = pty_rows.max(1);
        let mut resize_error = None;
        {
            let session = self
                .terminal_sessions
                .get_mut(path)
                .expect("session presence checked above");
            let needs_resize_request = cols != session.cols
                || pty_rows != session.rows
                || session.applied_cols != cols
                || session.applied_rows != pty_rows;
            session.cols = cols;
            session.rows = pty_rows;
            if needs_resize_request {
                if let Err(err) = self.terminal_manager.resize(path, cols, pty_rows) {
                    resize_error = Some(err);
                }
            }
            let max_top = Self::terminal_max_top_row(&session.terminal, rows);
            let (resolved_top, anchor) = if top_row == usize::MAX {
                (max_top, TerminalViewportAnchor::Bottom)
            } else {
                let clamped = top_row.min(max_top);
                let anchor = if clamped >= max_top.saturating_sub(1) {
                    TerminalViewportAnchor::Bottom
                } else {
                    TerminalViewportAnchor::TopRow
                };
                (clamped, anchor)
            };
            session.subscribers.insert(
                client_id,
                TerminalClientViewport {
                    cols,
                    rows,
                    top_row: resolved_top,
                    anchor,
                },
            );
        }
        if let Some(err) = resize_error {
            self.send_ui_error(client_id, err);
            return;
        }
        self.push_terminal_frame_updates(path, false);
    }

    fn send_terminal_title_to_client(&mut self, client_id: ClientId, path: &str) {
        let Some(session) = self.terminal_sessions.get(path) else {
            return;
        };
        if !session.bell_pending {
            return;
        }
        self.send_ui_reply(
            client_id,
            HubToClient::TerminalTitle {
                path: path.to_string(),
                title: Self::terminal_tab_title(path, true),
            },
        );
    }

    fn set_terminal_bell_state(&mut self, path: &str, bell_pending: bool) {
        let Some(session) = self.terminal_sessions.get_mut(path) else {
            return;
        };
        if session.bell_pending == bell_pending {
            return;
        }
        session.bell_pending = bell_pending;
        self.broadcast_ui_message(HubToClient::TerminalTitle {
            path: path.to_string(),
            title: Self::terminal_tab_title(path, bell_pending),
        });
    }

    fn push_terminal_frame_updates(&mut self, path: &str, force_bottom_for_sticky: bool) {
        let updates = {
            let Some(session) = self.terminal_sessions.get_mut(path) else {
                return;
            };
            for viewport in session.subscribers.values_mut() {
                let max_top = Self::terminal_max_top_row(&session.terminal, viewport.rows);
                if viewport.anchor == TerminalViewportAnchor::Bottom && force_bottom_for_sticky {
                    viewport.top_row = max_top;
                }
                viewport.top_row = viewport.top_row.min(max_top);
            }

            let subscribers: Vec<(ClientId, TerminalClientViewport)> = session
                .subscribers
                .iter()
                .map(|(client_id, viewport)| (*client_id, viewport.clone()))
                .collect();
            let mut updates = Vec::with_capacity(subscribers.len());
            for (client_id, viewport) in subscribers {
                session.frame_seq = session.frame_seq.wrapping_add(1);
                let frame = terminal_framebuffer_from_terminal(
                    &session.terminal,
                    viewport.cols,
                    viewport.rows,
                    viewport.top_row,
                    session.frame_seq,
                );
                updates.push((client_id, frame));
            }
            updates
        };

        let path = path.to_string();
        for (client_id, frame) in updates {
            self.send_ui_reply(
                client_id,
                HubToClient::TerminalFramebuffer {
                    path: path.clone(),
                    frame,
                },
            );
        }
    }

    fn adjust_terminal_subscribers_for_resize(session: &mut TerminalSession) {
        for viewport in session.subscribers.values_mut() {
            viewport.cols = session.cols;
            let max_top = Self::terminal_max_top_row(&session.terminal, viewport.rows);
            if viewport.anchor == TerminalViewportAnchor::Bottom {
                viewport.top_row = max_top;
            }
            viewport.top_row = viewport.top_row.min(max_top);
        }
    }

    fn terminal_max_top_row(terminal: &Terminal, rows: u16) -> usize {
        let screen = terminal.screen();
        let is_tui = screen.scroll_top != 0
            || screen.scroll_bottom != screen.rows()
            || terminal.modes.alt_screen;
        let total_lines = if is_tui {
            screen.scrollback_len() + screen.rows()
        } else {
            screen.scrollback_len() + screen.used_rows()
        };
        total_lines.saturating_sub(rows.max(1) as usize)
    }

    fn broadcast_live_log_entry(&self, index: usize, entry: LogEntry) {
        for (query_id, live) in &self.live_log_queries {
            if !live.query.matches(&entry) {
                continue;
            }
            self.send_ui_reply(
                live.client_id,
                HubToClient::QueryLogResults {
                    query_id: *query_id,
                    entries: vec![(index, entry.clone())],
                    done: false,
                },
            );
        }
    }

    fn broadcast_ui_message(&self, msg: HubToClient) {
        let ids: Vec<ClientId> = self.ui_clients.keys().copied().collect();
        for client_id in ids {
            self.send_ui_message(client_id, msg.clone(), self.ui_format(client_id));
        }
    }

    fn broadcast_ui_message_except(&self, excluded: ClientId, msg: HubToClient) {
        let ids: Vec<ClientId> = self.ui_clients.keys().copied().collect();
        for client_id in ids {
            if client_id == excluded {
                continue;
            }
            self.send_ui_message(client_id, msg.clone(), self.ui_format(client_id));
        }
    }

    fn send_to_query_owner(&self, query_id: QueryId, msg: HubToClient) {
        let client_id = query_id.client_id();
        self.send_ui_reply(client_id, msg);
    }

    fn broadcast_live_profiler_queries(&self) {
        for (query_id, live) in &self.live_profiler_queries {
            let (event_samples, gpu_samples, gc_samples, total_in_window) =
                self.profiler_store.query(&live.query);
            self.send_ui_reply(
                live.client_id,
                HubToClient::QueryProfilerResults {
                    query_id: *query_id,
                    event_samples,
                    gpu_samples,
                    gc_samples,
                    total_in_window,
                    done: false,
                },
            );
        }
    }

    fn ui_format(&self, client_id: ClientId) -> WireFormat {
        self.ui_clients
            .get(&client_id)
            .map(|v| v.format)
            .unwrap_or(WireFormat::Binary)
    }

    fn send_branch_op_result(
        &self,
        client_id: ClientId,
        mount: String,
        before: Option<backend_proto::FileTreeData>,
        result: Result<(), impl std::fmt::Display>,
    ) {
        if let Err(err) = result {
            self.send_ui_error(client_id, err.to_string());
            return;
        }
        match self.vfs.load_file_tree(&mount) {
            Ok(data) => self.send_ui_reply(
                client_id,
                HubToClient::FileTree {
                    mount: mount.clone(),
                    data: data.clone(),
                },
            ),
            Err(err) => self.send_ui_error(client_id, err.to_string()),
        }
        if let Some(before) = before {
            if let Ok(after) = self.vfs.load_file_tree(&mount) {
                self.send_ui_reply(
                    client_id,
                    HubToClient::FileTreeDiff {
                        mount,
                        changes: file_tree_diff(&before, &after),
                    },
                );
            }
        }
    }

    fn terminal_tab_title(path: &str, bell_pending: bool) -> String {
        let title = path.rsplit('/').next().unwrap_or("terminal");
        if bell_pending {
            format!("@ {}", title)
        } else {
            title.to_string()
        }
    }

    fn send_ui_reply(&self, client_id: ClientId, msg: HubToClient) {
        self.send_ui_message(client_id, msg, self.ui_format(client_id));
    }

    fn send_ui_error(&self, client_id: ClientId, message: String) {
        self.send_ui_reply(client_id, HubToClient::Error { message });
    }

    fn send_ui_message(&self, client_id: ClientId, msg: HubToClient, format: WireFormat) {
        let Some(client) = self.ui_clients.get(&client_id) else {
            return;
        };
        if let Some(typed_sender) = &client.typed_sender {
            let _ = typed_sender.send(msg);
            return;
        }
        let payload = match format {
            WireFormat::Binary => msg.serialize_bin(),
            WireFormat::Text => msg.serialize_json().into_bytes(),
        };
        let _ = client.sender.send(payload);
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
