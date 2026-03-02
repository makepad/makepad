use crate::log_store::{
    query_log_entries, AppendLogEntry, LogQuery, LogStore, ProfilerQuery, ProfilerStore,
};
use crate::process_manager::ProcessManager;
use makepad_studio_protocol::backend_protocol as backend_proto;
use backend_proto::{
    BuildBoxInfo, BuildBoxStatus, BuildBoxToStudio, BuildBoxToStudioVec, BuildInfo, ClientId,
    EventSample as StudioEventSample, GCSample as StudioGCSample, GPUSample as StudioGPUSample,
    LogEntry, LogSource, QueryId, RunViewInputVizKind, RunnableBuild, SaveResult, StudioToBuildBox,
    StudioToBuildBoxVec, StudioToUI, TerminalGrid, UIToStudio, UIToStudioEnvelope,
};
use crate::terminal_manager::TerminalManager;
use crate::virtual_fs::{protocol_search_results, VirtualFs};
use crate::worker_pool::WorkerPool;
use makepad_filesystem_watcher::{FileSystemWatcher, WatchRoot};
use makepad_live_id::LiveId;
use makepad_micro_serde::*;
use makepad_network::ToUISender;
use makepad_studio_protocol::{
    AppToStudio, AppToStudioVec, EventSample, GCSample, GPUSample, KeyCode, KeyEvent, KeyModifiers,
    LogLevel, MouseButton, RemoteKeyModifiers, RemoteMouseDown, RemoteMouseUp, ScreenshotRequest,
    StudioToApp, StudioToAppVec, TextInputEvent, WidgetQueryRequest, WidgetTreeDumpRequest,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireFormat {
    Binary,
    Text,
}

#[derive(Debug)]
pub enum StudioEvent {
    UiConnected {
        web_socket_id: u64,
        sender: ToUISender<Vec<u8>>,
        typed_sender: Option<ToUISender<StudioToUI>>,
    },
    UiDisconnected {
        web_socket_id: u64,
    },
    UiEnvelope {
        web_socket_id: u64,
        envelope: UIToStudioEnvelope,
    },
    UiBinary {
        web_socket_id: u64,
        data: Vec<u8>,
    },
    UiText {
        web_socket_id: u64,
        text: String,
    },
    AppConnected {
        build_id: QueryId,
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
    TerminalOutput {
        path: String,
        data: Vec<u8>,
    },
    TerminalExited {
        path: String,
        exit_code: i32,
    },
    WorkerFindFilesDone {
        client_id: ClientId,
        query_id: QueryId,
        for_search: bool,
        result: Result<Vec<String>, String>,
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
    FlushPendingFileTreeDiffs,
    MountFsChanged {
        mount: String,
        path: PathBuf,
    },
    Shutdown,
}

const FS_EVENT_PATH_DEBOUNCE: Duration = Duration::from_millis(80);
const FS_EVENT_RELOAD_DEBOUNCE: Duration = Duration::from_millis(120);
const FS_EVENT_HISTORY_PRUNE_INTERVAL: Duration = Duration::from_secs(4);
const FS_EVENT_HISTORY_RETENTION: Duration = Duration::from_secs(12);
const FS_DELTA_FLUSH_DELAY: Duration = Duration::from_millis(32);
const FS_DELTA_RELOAD_THRESHOLD: usize = 768;
const FS_SELF_SAVE_SUPPRESS: Duration = Duration::from_millis(300);
const IN_PROCESS_UI_WEB_SOCKET_ID: u64 = 0;
const MAX_UI_CLIENT_IDS: usize = backend_proto::QUERY_ID_CLIENT_LANES as usize;

struct UiClient {
    sender: ToUISender<Vec<u8>>,
    typed_sender: Option<ToUISender<StudioToUI>>,
    format: WireFormat,
}

struct AppSocket {
    build_id: QueryId,
    sender: Sender<Vec<u8>>,
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

pub struct StudioCore {
    rx: Receiver<StudioEvent>,
    event_tx: Sender<StudioEvent>,
    pub vfs: VirtualFs,
    studio_addr: Option<String>,
    client_id_in_use: [bool; MAX_UI_CLIENT_IDS],
    next_build_id: u64,
    client_by_web_socket: HashMap<u64, ClientId>,
    ui_clients: HashMap<ClientId, UiClient>,
    app_sockets: HashMap<u64, AppSocket>,
    buildbox_sockets: HashMap<u64, BuildBoxSocket>,
    buildbox_by_name: HashMap<String, u64>,
    build_mount_by_id: HashMap<QueryId, String>,
    primary_ui_by_mount: HashMap<String, ClientId>,
    remote_builds: HashMap<QueryId, BuildInfo>,
    remote_build_owner: HashMap<QueryId, String>,
    log_store: LogStore,
    profiler_store: ProfilerStore,
    process_manager: ProcessManager,
    terminal_manager: TerminalManager,
    live_log_queries: HashMap<QueryId, LiveLogSubscription>,
    live_profiler_queries: HashMap<QueryId, LiveProfilerSubscription>,
    cancelled_queries: HashSet<QueryId>,
    worker_pool: WorkerPool,
    fs_watcher: Option<FileSystemWatcher>,
    fs_event_last_by_path: HashMap<String, Instant>,
    fs_pending_diffs: HashMap<String, Vec<backend_proto::FileTreeChange>>,
    fs_pending_reload_mounts: HashSet<String>,
    file_tree_load_waiters: HashMap<String, HashSet<ClientId>>,
    fs_diff_flush_scheduled: bool,
    fs_event_last_prune: Instant,
    mount_suppress_fs_until: HashMap<String, Instant>,
    self_save_suppress_until_by_path: HashMap<String, Instant>,
}

impl StudioCore {
    pub fn new(
        rx: Receiver<StudioEvent>,
        event_tx: Sender<StudioEvent>,
        vfs: VirtualFs,
        studio_addr: Option<String>,
    ) -> Self {
        let worker_count = std::thread::available_parallelism()
            .map(|v| v.get())
            .unwrap_or(4)
            .clamp(2, 16);
        let mut this = Self {
            rx,
            event_tx,
            vfs,
            studio_addr,
            client_id_in_use: [false; MAX_UI_CLIENT_IDS],
            next_build_id: 1,
            client_by_web_socket: HashMap::new(),
            ui_clients: HashMap::new(),
            app_sockets: HashMap::new(),
            buildbox_sockets: HashMap::new(),
            buildbox_by_name: HashMap::new(),
            build_mount_by_id: HashMap::new(),
            primary_ui_by_mount: HashMap::new(),
            remote_builds: HashMap::new(),
            remote_build_owner: HashMap::new(),
            log_store: LogStore::default(),
            profiler_store: ProfilerStore::default(),
            process_manager: ProcessManager::default(),
            terminal_manager: TerminalManager::default(),
            live_log_queries: HashMap::new(),
            live_profiler_queries: HashMap::new(),
            cancelled_queries: HashSet::new(),
            worker_pool: WorkerPool::new(worker_count),
            fs_watcher: None,
            fs_event_last_by_path: HashMap::new(),
            fs_pending_diffs: HashMap::new(),
            fs_pending_reload_mounts: HashSet::new(),
            file_tree_load_waiters: HashMap::new(),
            fs_diff_flush_scheduled: false,
            fs_event_last_prune: Instant::now(),
            mount_suppress_fs_until: HashMap::new(),
            self_save_suppress_until_by_path: HashMap::new(),
        };
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

    pub fn handle_event(&mut self, event: StudioEvent) -> bool {
        match event {
            StudioEvent::UiConnected {
                web_socket_id,
                sender,
                typed_sender,
            } => self.on_ui_connected(web_socket_id, sender, typed_sender),
            StudioEvent::UiDisconnected { web_socket_id } => {
                if let Some(client_id) = self.client_by_web_socket.remove(&web_socket_id) {
                    self.ui_clients.remove(&client_id);
                    self.release_client_id(client_id);
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
            StudioEvent::UiEnvelope {
                web_socket_id,
                envelope,
            } => {
                if let Some(&client_id) = self.client_by_web_socket.get(&web_socket_id) {
                    self.on_ui_envelope(client_id, envelope);
                }
            }
            StudioEvent::UiBinary {
                web_socket_id,
                data,
            } => {
                if let Some(&client_id) = self.client_by_web_socket.get(&web_socket_id) {
                    self.on_ui_message(client_id, WireFormat::Binary, &data);
                }
            }
            StudioEvent::UiText {
                web_socket_id,
                text,
            } => {
                if let Some(&client_id) = self.client_by_web_socket.get(&web_socket_id) {
                    self.on_ui_message(client_id, WireFormat::Text, text.as_bytes());
                }
            }
            StudioEvent::AppConnected {
                web_socket_id,
                build_id,
                sender,
            } => {
                self.app_sockets
                    .insert(web_socket_id, AppSocket { build_id, sender });
            }
            StudioEvent::AppDisconnected { web_socket_id } => {
                self.app_sockets.remove(&web_socket_id);
            }
            StudioEvent::AppBinary {
                web_socket_id,
                data,
            } => {
                let build_id = match self.app_sockets.get(&web_socket_id) {
                    Some(socket) => socket.build_id,
                    None => return true,
                };
                self.on_app_binary(build_id, data);
            }
            StudioEvent::BuildBoxConnected {
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
            StudioEvent::BuildBoxDisconnected { web_socket_id } => {
                self.on_buildbox_disconnected(web_socket_id);
            }
            StudioEvent::BuildBoxBinary {
                web_socket_id,
                data,
            } => {
                if self.buildbox_sockets.contains_key(&web_socket_id) {
                    self.on_buildbox_binary(web_socket_id, data);
                }
            }
            StudioEvent::ProcessOutput {
                build_id,
                is_stderr,
                line,
            } => self.on_process_output(build_id, is_stderr, line),
            StudioEvent::ProcessExited {
                build_id,
                exit_code,
            } => self.on_process_exited(build_id, exit_code),
            StudioEvent::TerminalOutput { path, data } => self.on_terminal_output(path, data),
            StudioEvent::TerminalExited { path, exit_code } => {
                self.on_terminal_exited(path, exit_code)
            }
            StudioEvent::WorkerFindFilesDone {
                client_id,
                query_id,
                for_search,
                result,
            } => self.on_worker_find_files_done(client_id, query_id, for_search, result),
            StudioEvent::WorkerQueryLogsDone {
                client_id,
                query_id,
                query,
                live,
                entries,
            } => self.on_worker_query_logs_done(client_id, query_id, query, live, entries),
            StudioEvent::WorkerLoadFileTreeDone {
                mount,
                result,
            } => self.on_worker_load_file_tree_done(mount, result),
            StudioEvent::WorkerFileTreeDeltaDone { mount, change } => {
                self.queue_file_tree_delta_change(mount, change);
            }
            StudioEvent::FlushPendingFileTreeDiffs => self.flush_pending_file_tree_diffs(),
            StudioEvent::MountFsChanged { mount, path } => self.on_mount_fs_changed(mount, path),
            StudioEvent::Shutdown => return false,
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
        typed_sender: Option<ToUISender<StudioToUI>>,
    ) {
        let client_id = if web_socket_id == IN_PROCESS_UI_WEB_SOCKET_ID {
            let reserved = ClientId(0);
            if !self.reserve_client_id(reserved) {
                if let Some(typed_sender) = &typed_sender {
                    let _ = typed_sender.send(StudioToUI::Error {
                        message: "client id 0 already in use".to_string(),
                    });
                } else {
                    let _ = sender.send(
                        StudioToUI::Error {
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
                // Refuse the websocket when we cannot allocate a client lane.
                let _ = sender.send(Vec::new());
                return;
            };
            client_id
        };

        if self.ui_clients.contains_key(&client_id) {
            self.release_client_id(client_id);
            if let Some(typed_sender) = &typed_sender {
                let _ = typed_sender.send(StudioToUI::Error {
                    message: format!("client id {:?} already in use", client_id),
                });
            } else {
                let _ = sender.send(
                    StudioToUI::Error {
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
        self.send_ui_message(
            client_id,
            StudioToUI::Hello { client_id },
            WireFormat::Binary,
        );
    }

    fn on_ui_envelope(&mut self, client_id: ClientId, envelope: UIToStudioEnvelope) {
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
            WireFormat::Binary => UIToStudioEnvelope::deserialize_bin(data).map_err(|e| e.msg),
            WireFormat::Text => std::str::from_utf8(data)
                .map_err(|err| err.to_string())
                .and_then(|text| UIToStudioEnvelope::deserialize_json(text).map_err(|e| e.msg)),
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

    fn handle_ui_message(&mut self, client_id: ClientId, envelope: UIToStudioEnvelope) {
        let query_id = envelope.query_id;
        match envelope.msg {
            UIToStudio::Mount { name, path } => match self.vfs.mount(&name, path) {
                Ok(()) => {
                    self.reset_fs_watcher();
                    match self.vfs.load_file_tree(&name) {
                        Ok(data) => self.send_ui_reply(
                            client_id,
                            StudioToUI::FileTree { mount: name, data }
                        ),
                        Err(err) => self.send_ui_error(client_id, err.to_string()),
                    }
                }
                Err(err) => self.send_ui_error(client_id, err.to_string()),
            },
            UIToStudio::Unmount { name } => {
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
                self.build_mount_by_id.retain(|_, mount| mount != &name);
                self.send_ui_reply(
                    client_id,
                    StudioToUI::FileTree {
                        mount: name.clone(),
                        data: backend_proto::FileTreeData { nodes: Vec::new() },
                    }
                );
                self.send_ui_reply(
                    client_id,
                    StudioToUI::FileTreeDiff {
                        mount: name,
                        changes,
                    }
                );
            }
            UIToStudio::ObserveMount { mount, primary } => {
                if primary.unwrap_or(true) {
                    self.primary_ui_by_mount.insert(mount, client_id);
                } else if self.primary_ui_by_mount.get(&mount) == Some(&client_id) {
                    self.primary_ui_by_mount.remove(&mount);
                }
            }
            UIToStudio::LoadFileTree { mount } => {
                let waiters = self.file_tree_load_waiters.entry(mount.clone()).or_default();
                let first_request = waiters.is_empty();
                waiters.insert(client_id);
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
                    let _ = event_tx.send(StudioEvent::WorkerLoadFileTreeDone {
                        mount: mount_name,
                        result,
                    });
                });
            }
            UIToStudio::OpenTextFile { path } => match self.vfs.open_text_file(&path) {
                Ok(content) => self.send_ui_reply(
                    client_id,
                    StudioToUI::TextFileOpened {
                        path,
                        content,
                        git_status: backend_proto::GitStatus::Unknown,
                    }
                ),
                Err(err) => self.send_ui_error(client_id, err.to_string()),
            },
            UIToStudio::ReadTextFile { path } => match self.vfs.read_text_file(&path) {
                Ok(content) => self.send_ui_reply(
                    client_id,
                    StudioToUI::TextFileRead { path, content }
                ),
                Err(err) => self.send_ui_error(client_id, err.to_string()),
            },
            UIToStudio::SaveTextFile { path, content } => {
                let result = match self.vfs.save_text_file(&path, &content) {
                    Ok(()) => SaveResult::Ok,
                    Err(err) => SaveResult::Err(err.into()),
                };
                let save_ok = matches!(result, SaveResult::Ok);
                self.send_ui_reply(
                    client_id,
                    StudioToUI::TextFileSaved {
                        path: path.clone(),
                        result,
                    }
                );
                if save_ok {
                    self.self_save_suppress_until_by_path
                        .insert(path.clone(), Instant::now() + FS_SELF_SAVE_SUPPRESS);
                    self.broadcast_ui_message_except(
                        client_id,
                        StudioToUI::FileChanged { path: path.clone() },
                    );
                    self.enqueue_file_tree_delta_for_virtual_path(&path);
                }
            }
            UIToStudio::DeleteFile { path } => {
                self.terminal_manager.close_terminal(&path);
                let disk_path = self.vfs.resolve_path(&path).ok();
                if let Err(err) = self.vfs.delete_path(&path) {
                    self.send_ui_error(client_id, err.to_string());
                } else if let Some(disk_path) = disk_path {
                    self.enqueue_file_tree_delta_for_known_path(&path, disk_path);
                }
            }
            UIToStudio::FindFiles {
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
                    let _ = event_tx.send(StudioEvent::WorkerFindFilesDone {
                        client_id,
                        query_id,
                        for_search: false,
                        result,
                    });
                });
            }
            UIToStudio::SearchFiles {
                mount,
                pattern,
                is_regex: _,
                glob: _,
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
                    let _ = event_tx.send(StudioEvent::WorkerFindFilesDone {
                        client_id,
                        query_id,
                        for_search: true,
                        result,
                    });
                });
            }
            UIToStudio::GitLog { mount, max_count } => {
                match self.vfs.git_log(&mount, max_count.unwrap_or(100)) {
                    Ok(log) => self.send_ui_reply(
                        client_id,
                        StudioToUI::GitLog { mount, log }
                    ),
                    Err(err) => self.send_ui_error(client_id, err.to_string()),
                }
            }
            UIToStudio::CreateBranch {
                mount,
                name,
                from_ref,
            } => {
                let before = self.vfs.load_file_tree(&mount).ok();
                let result = self.vfs.create_branch(&mount, &name, from_ref.as_deref());
                self.send_branch_op_result(client_id, mount, before, result);
            }
            UIToStudio::DeleteBranch { mount, name } => {
                let before = self.vfs.load_file_tree(&mount).ok();
                let result = self.vfs.delete_branch(&mount, &name);
                self.send_branch_op_result(client_id, mount, before, result);
            }
            UIToStudio::ListBuilds => {
                self.send_ui_reply(
                    client_id,
                    StudioToUI::Builds {
                        builds: self.list_all_builds(),
                    }
                );
            }
            UIToStudio::LoadRunnableBuilds { mount } => {
                let cwd = match self.vfs.resolve_mount(&mount) {
                    Ok(cwd) => cwd,
                    Err(err) => {
                        self.send_ui_error(client_id, err.to_string());
                        return;
                    }
                };
                match discover_runnable_builds(&cwd) {
                    Ok(builds) => self.send_ui_reply(
                        client_id,
                        StudioToUI::RunnableBuilds { mount, builds }
                    ),
                    Err(err) => self.send_ui_error(client_id, err),
                }
            }
            UIToStudio::Cargo {
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
                    let msg = StudioToBuildBox::CargoBuild {
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
                    self.set_buildbox_status(
                        &buildbox_name,
                        BuildBoxStatus::Building { build_id },
                    );
                    self.broadcast_ui_message(StudioToUI::BuildStarted {
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
                match self.process_manager.start_cargo_run(
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
                        self.broadcast_ui_message(StudioToUI::BuildStarted {
                            build_id: info.build_id,
                            mount: info.mount,
                            package: info.package,
                        });
                    }
                    Err(err) => self.send_ui_error(client_id, err),
                }
            }
            UIToStudio::Run {
                mount,
                process,
                args: app_args,
                standalone,
                env,
                buildbox,
            } => {
                let cargo_args =
                    build_run_cargo_args(&process, app_args, standalone.unwrap_or(false));
                let build_id = self.alloc_build_id();
                if let Some(buildbox_name) = buildbox {
                    let env = env.unwrap_or_default();
                    let msg = StudioToBuildBox::CargoBuild {
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
                    self.set_buildbox_status(
                        &buildbox_name,
                        BuildBoxStatus::Building { build_id },
                    );
                    self.broadcast_ui_message(StudioToUI::BuildStarted {
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
                match self.process_manager.start_cargo_run(
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
                        self.broadcast_ui_message(StudioToUI::BuildStarted {
                            build_id: info.build_id,
                            mount: info.mount,
                            package: info.package,
                        });
                    }
                    Err(err) => self.send_ui_error(client_id, err),
                }
            }
            UIToStudio::StopBuild { build_id } => {
                if self.process_manager.stop_build(build_id).is_ok() {
                    return;
                }
                let Some(buildbox_name) = self.remote_build_owner.get(&build_id).cloned() else {
                    self.send_ui_error(client_id, format!("unknown build: {}", build_id.0));
                    return;
                };
                if let Err(err) = self
                    .send_to_buildbox_name(&buildbox_name, StudioToBuildBox::StopBuild { build_id })
                {
                    self.send_ui_error(client_id, err);
                }
            }
            UIToStudio::ForwardToApp { build_id, msg_bin } => {
                if let Err(err) = self.send_to_app(build_id, msg_bin) {
                    self.send_ui_error(client_id, err);
                }
            }
            UIToStudio::TypeText { build_id, text } => {
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
                        StudioToUI::RunViewInputViz {
                            build_id,
                            kind: RunViewInputVizKind::TypeText,
                            x: None,
                            y: None,
                        },
                    );
                }
            }
            UIToStudio::Return {
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
                        StudioToUI::RunViewInputViz {
                            build_id,
                            kind: RunViewInputVizKind::Return,
                            x: None,
                            y: None,
                        },
                    );
                }
            }
            UIToStudio::Click { build_id, x, y } => {
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
                        StudioToUI::RunViewInputViz {
                            build_id,
                            kind: RunViewInputVizKind::ClickDown,
                            x: Some(x),
                            y: Some(y),
                        },
                    );
                    self.send_runview_message(
                        build_id,
                        StudioToUI::RunViewInputViz {
                            build_id,
                            kind: RunViewInputVizKind::ClickUp,
                            x: Some(x),
                            y: Some(y),
                        },
                    );
                }
            }
            UIToStudio::Screenshot { build_id, kind_id } => {
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
            UIToStudio::WidgetTreeDump { build_id } => {
                if let Err(err) = self.send_app_msg(
                    build_id,
                    StudioToApp::WidgetTreeDump(WidgetTreeDumpRequest {
                        request_id: query_id.0,
                    }),
                ) {
                    self.send_ui_error(client_id, err);
                }
            }
            UIToStudio::WidgetQuery { build_id, query } => {
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
            UIToStudio::RunViewInput {
                build_id,
                window_id,
                msg_bin,
            } => {
                let _ = window_id;
                if let Err(err) = self.send_to_app(build_id, msg_bin) {
                    self.send_ui_error(client_id, err);
                }
            }
            UIToStudio::RunViewResize {
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
            UIToStudio::TerminalOpen {
                path,
                cols,
                rows,
                env,
            } => {
                let Some(mount) = mount_from_virtual_path(&path).map(ToOwned::to_owned) else {
                    self.send_ui_error(
                        client_id,
                        format!("invalid terminal path (missing mount): {}", path),
                    );
                    return;
                };
                let cwd = match self.vfs.resolve_mount(&mount) {
                    Ok(cwd) => cwd,
                    Err(err) => {
                        self.send_ui_error(client_id, err.to_string());
                        return;
                    }
                };
                let history = self
                    .vfs
                    .resolve_path(&path)
                    .ok()
                    .and_then(|disk_path| fs::read(disk_path).ok())
                    .unwrap_or_default();
                match self.terminal_manager.open_terminal(
                    path.clone(),
                    mount,
                    &cwd,
                    cols,
                    rows,
                    env,
                    self.event_tx.clone(),
                ) {
                    Ok(()) => self.send_ui_reply(
                        client_id,
                        StudioToUI::TerminalOpened {
                            path: path.clone(),
                            grid: terminal_grid_from_history(&history, cols, rows),
                            history,
                        }
                    ),
                    Err(err) => self.send_ui_error(client_id, err),
                }
            }
            UIToStudio::TerminalInput { path, data } => {
                if let Err(err) = self.terminal_manager.send_input(&path, data) {
                    self.send_ui_error(client_id, err);
                }
            }
            UIToStudio::TerminalResize { path, cols, rows } => {
                if let Err(err) = self.terminal_manager.resize(&path, cols, rows) {
                    self.send_ui_error(client_id, err);
                }
            }
            UIToStudio::TerminalClose { path } => {
                self.terminal_manager.close_terminal(&path);
            }
            UIToStudio::QueryLogs {
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
                    let _ = event_tx.send(StudioEvent::WorkerQueryLogsDone {
                        client_id,
                        query_id,
                        query,
                        live,
                        entries,
                    });
                });
            }
            UIToStudio::QueryProfiler {
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
                    StudioToUI::QueryProfilerResults {
                        query_id,
                        event_samples,
                        gpu_samples,
                        gc_samples,
                        total_in_window,
                        done: !live,
                    }
                );
                if live {
                    self.live_profiler_queries.insert(
                        query_id,
                        LiveProfilerSubscription {
                            client_id,
                            query,
                        },
                    );
                }
            }
            UIToStudio::CancelQuery { query_id } => {
                self.cancelled_queries.insert(query_id);
                self.live_log_queries.remove(&query_id);
                self.live_profiler_queries.remove(&query_id);
                self.send_ui_reply(
                    client_id,
                    StudioToUI::QueryCancelled { query_id }
                );
            }
            UIToStudio::LogClear => {
                self.log_store.clear();
                self.send_ui_reply(
                    client_id,
                    StudioToUI::LogCleared
                );
            }
            UIToStudio::ListBuildBoxes => {
                self.send_ui_reply(
                    client_id,
                    StudioToUI::BuildBoxes {
                        boxes: self.list_buildboxes(),
                    }
                );
            }
            UIToStudio::BuildBoxSyncNow { name } => {
                if let Err(err) =
                    self.send_to_buildbox_name(&name, StudioToBuildBox::RequestTreeHash)
                {
                    self.send_ui_error(client_id, err);
                    return;
                }
                self.set_buildbox_status(&name, BuildBoxStatus::Syncing);
                self.send_ui_reply(
                    client_id,
                    StudioToUI::BuildBoxes {
                        boxes: self.list_buildboxes(),
                    }
                );
            }
            UIToStudio::ListScriptTasks => {
                self.send_ui_reply(
                    client_id,
                    StudioToUI::ScriptTasks { tasks: Vec::new() }
                );
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
        self.fs_event_last_by_path.clear();
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
        match FileSystemWatcher::start(roots, move |event| {
            let _ = event_tx.send(StudioEvent::MountFsChanged {
                mount: event.mount,
                path: event.path,
            });
        }) {
            Ok(watcher) => {
                self.fs_watcher = Some(watcher);
            }
            Err(err) => {
                eprintln!("[studio2-backend] filesystem watcher unavailable: {}", err);
            }
        }
    }

    fn on_mount_fs_changed(&mut self, mount: String, path: PathBuf) {
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
            // Some watcher implementations only report "mount root changed".
            // Broadcast a mount-level FileChanged so UI can refresh open tabs.
            self.broadcast_ui_message(StudioToUI::FileChanged { path: mount.clone() });
            self.reload_mount_file_tree_broadcast(&mount);
            return;
        }
        if self.should_suppress_self_save_event(&virtual_path, now) {
            return;
        }
        if path_is_file && !self.should_ignore_virtual_path(&mount, &virtual_path) {
            self.broadcast_ui_message(StudioToUI::FileChanged {
                path: virtual_path.clone(),
            });
        }
        if path_is_dir {
            self.reload_mount_file_tree_broadcast(&mount);
            return;
        }
        let (path, virtual_path) =
            self.collapse_removed_path_to_missing_ancestor(&mount, path, virtual_path);
        self.enqueue_file_tree_delta(&mount, &virtual_path, path, now);
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

        let mount = mount.to_string();
        let virtual_path = virtual_path.to_string();
        let event_tx = self.event_tx.clone();
        self.worker_pool.execute(move || {
            let change = compute_filetree_change_for_path(&disk_path, virtual_path);
            let _ = event_tx.send(StudioEvent::WorkerFileTreeDeltaDone { mount, change });
        });
    }

    fn should_ignore_fs_watch_virtual_path(
        &self,
        mount: &str,
        virtual_path: &str,
    ) -> bool {
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
        match self.vfs.load_file_tree(mount) {
            Ok(data) => self.broadcast_ui_message(StudioToUI::FileTree {
                mount: mount.to_string(),
                data,
            }),
            Err(err) => self.broadcast_ui_message(StudioToUI::Error {
                message: format!("file tree reload failed for {}: {}", mount, err),
            }),
        }
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
            let _ = event_tx.send(StudioEvent::FlushPendingFileTreeDiffs);
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
            self.broadcast_ui_message(StudioToUI::FileTreeDiff { mount, changes });
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
        self.self_save_suppress_until_by_path
            .retain(|_, until| *until > now);
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
        for_search: bool,
        result: Result<Vec<String>, String>,
    ) {
        if self.cancelled_queries.remove(&query_id) {
            return;
        }

        match result {
            Ok(paths) => {
                if for_search {
                    self.send_ui_reply(
                        client_id,
                        StudioToUI::SearchFileResults {
                            query_id,
                            results: protocol_search_results(paths),
                            done: true,
                        }
                    );
                } else {
                    self.send_ui_reply(
                        client_id,
                        StudioToUI::FindFileResults {
                            query_id,
                            paths,
                            done: true,
                        }
                    );
                }
            }
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
            StudioToUI::QueryLogResults {
                query_id,
                entries,
                done: !live,
            }
        );

        if live && self.ui_clients.contains_key(&client_id) {
            self.live_log_queries.insert(
                query_id,
                LiveLogSubscription {
                    client_id,
                    query,
                },
            );
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
                        StudioToUI::FileTree {
                            mount: mount.clone(),
                            data: data.clone(),
                        }
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

    fn send_to_app(&self, build_id: QueryId, msg_bin: Vec<u8>) -> Result<(), String> {
        let sender = self
            .app_sockets
            .values()
            .find(|socket| socket.build_id == build_id)
            .map(|socket| socket.sender.clone())
            .ok_or_else(|| format!("no app socket for build {}", build_id.0))?;
        sender
            .send(msg_bin)
            .map_err(|_| format!("failed to send app message for build {}", build_id.0))
    }

    fn send_app_msg(&self, build_id: QueryId, msg: StudioToApp) -> Result<(), String> {
        self.send_to_app(build_id, StudioToAppVec(vec![msg]).serialize_bin())
    }

    fn send_app_msgs(&self, build_id: QueryId, msgs: Vec<StudioToApp>) -> Result<(), String> {
        self.send_to_app(build_id, StudioToAppVec(msgs).serialize_bin())
    }

    fn send_to_buildbox_name(&self, name: &str, msg: StudioToBuildBox) -> Result<(), String> {
        let Some(web_socket_id) = self.buildbox_by_name.get(name).copied() else {
            return Err(format!("buildbox '{}' is not connected", name));
        };
        let Some(socket) = self.buildbox_sockets.get(&web_socket_id) else {
            return Err(format!("buildbox '{}' socket is missing", name));
        };
        socket
            .sender
            .send(StudioToBuildBoxVec(vec![msg]).serialize_bin())
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
        let mut builds = self.process_manager.list_builds();
        builds.extend(self.remote_builds.values().cloned());
        builds.sort_by_key(|build| build.build_id.0);
        builds
    }

    fn primary_ui_for_mount(&self, mount: &str) -> Option<ClientId> {
        let client_id = self.primary_ui_by_mount.get(mount).copied()?;
        self.ui_clients.contains_key(&client_id).then_some(client_id)
    }

    fn primary_ui_for_build(&self, build_id: QueryId) -> Option<ClientId> {
        let mount = self.build_mount_by_id.get(&build_id)?;
        self.primary_ui_for_mount(mount)
    }

    fn send_runview_message(&self, build_id: QueryId, msg: StudioToUI) {
        if let Some(client_id) = self.primary_ui_for_build(build_id) {
            self.send_ui_message(client_id, msg, self.ui_format(client_id));
        } else {
            self.broadcast_ui_message(msg);
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
        self.broadcast_ui_message(StudioToUI::BuildBoxes {
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
        self.broadcast_ui_message(StudioToUI::BuildBoxDisconnected {
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
            self.broadcast_ui_message(StudioToUI::BuildStopped {
                build_id,
                exit_code: None,
            });
        }

        self.broadcast_ui_message(StudioToUI::BuildBoxes {
            boxes: self.list_buildboxes(),
        });
    }

    fn on_buildbox_binary(&mut self, web_socket_id: u64, data: Vec<u8>) {
        let messages = match BuildBoxToStudioVec::deserialize_bin(&data) {
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

    fn handle_buildbox_message(&mut self, web_socket_id: u64, msg: BuildBoxToStudio) {
        match msg {
            BuildBoxToStudio::Hello {
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
                self.broadcast_ui_message(StudioToUI::BuildBoxConnected { info });
                self.broadcast_ui_message(StudioToUI::BuildBoxes {
                    boxes: self.list_buildboxes(),
                });
            }
            BuildBoxToStudio::BuildOutput { build_id, line } => {
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
            BuildBoxToStudio::BuildStarted { build_id } => {
                if let Some(buildbox_name) = self.remote_build_owner.get(&build_id).cloned() {
                    self.set_buildbox_status(&buildbox_name, BuildBoxStatus::Building { build_id });
                }
            }
            BuildBoxToStudio::BuildStopped {
                build_id,
                exit_code,
            } => {
                if let Some(buildbox_name) = self.remote_build_owner.remove(&build_id) {
                    self.remote_builds.remove(&build_id);
                    self.set_buildbox_status(&buildbox_name, BuildBoxStatus::Idle);
                }
                self.build_mount_by_id.remove(&build_id);
                self.broadcast_ui_message(StudioToUI::BuildStopped {
                    build_id,
                    exit_code,
                });
            }
            BuildBoxToStudio::SyncComplete { tree_hash } => {
                if let Some(socket) = self.buildbox_sockets.get_mut(&web_socket_id) {
                    socket.tree_hash = Some(tree_hash);
                    if let Some(info) = socket.info.as_mut() {
                        info.status = BuildBoxStatus::Idle;
                    }
                }
                self.broadcast_ui_message(StudioToUI::BuildBoxes {
                    boxes: self.list_buildboxes(),
                });
            }
            BuildBoxToStudio::SyncError { error } => {
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
            BuildBoxToStudio::Pong => {}
            BuildBoxToStudio::FileHashes { .. } => {}
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
                            StudioToUI::Screenshot {
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
                            StudioToUI::Error {
                                message: format!("failed to persist screenshot: {}", err),
                            },
                        ),
                    }
                }
            }
            AppToStudio::WidgetTreeDump(response) => {
                let query_id = QueryId(response.request_id);
                self.send_to_query_owner(
                    query_id,
                    StudioToUI::WidgetTreeDump {
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
                    StudioToUI::WidgetQuery {
                        query_id,
                        build_id,
                        query: response.query,
                        rects: response.rects,
                    },
                );
            }
            AppToStudio::CreateWindow {
                window_id,
                kind_id: _,
            } => {
                self.send_runview_message(
                    build_id,
                    StudioToUI::RunViewCreated {
                        build_id,
                        window_id,
                    },
                );
            }
            AppToStudio::AfterStartup => {
                self.broadcast_ui_message(StudioToUI::AppStarted { build_id });
            }
            AppToStudio::SetCursor(cursor) => {
                self.send_runview_message(
                    build_id,
                    StudioToUI::RunViewCursor {
                        build_id,
                        cursor: format!("{:?}", cursor),
                    },
                );
            }
            AppToStudio::DrawCompleteAndFlip(presentable_draw) => {
                self.send_runview_message(
                    build_id,
                    StudioToUI::RunViewDrawComplete {
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
            .process_manager
            .mark_exited(build_id, exit_code)
            .is_none()
        {
            return;
        }
        self.build_mount_by_id.remove(&build_id);
        self.broadcast_ui_message(StudioToUI::BuildStopped {
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
        let _ = append_terminal_history_bytes(&self.vfs, &path, &data);
        self.broadcast_ui_message(StudioToUI::TerminalOutput { path, data });
    }

    fn on_terminal_exited(&mut self, path: String, exit_code: i32) {
        self.terminal_manager.remove_terminal(&path);
        self.broadcast_ui_message(StudioToUI::TerminalExited {
            path,
            code: exit_code,
        });
    }

    fn broadcast_live_log_entry(&self, index: usize, entry: LogEntry) {
        for (query_id, live) in &self.live_log_queries {
            if !live.query.matches(&entry) {
                continue;
            }
            self.send_ui_reply(
                live.client_id,
                StudioToUI::QueryLogResults {
                    query_id: *query_id,
                    entries: vec![(index, entry.clone())],
                    done: false,
                },
            );
        }
    }

    fn broadcast_ui_message(&self, msg: StudioToUI) {
        let ids: Vec<ClientId> = self.ui_clients.keys().copied().collect();
        for client_id in ids {
            self.send_ui_message(client_id, msg.clone(), self.ui_format(client_id));
        }
    }

    fn broadcast_ui_message_except(&self, excluded: ClientId, msg: StudioToUI) {
        let ids: Vec<ClientId> = self.ui_clients.keys().copied().collect();
        for client_id in ids {
            if client_id == excluded {
                continue;
            }
            self.send_ui_message(client_id, msg.clone(), self.ui_format(client_id));
        }
    }

    fn send_to_query_owner(&self, query_id: QueryId, msg: StudioToUI) {
        let client_id = query_id.client_id();
        self.send_ui_reply(client_id, msg);
    }

    fn broadcast_live_profiler_queries(&self) {
        for (query_id, live) in &self.live_profiler_queries {
            let (event_samples, gpu_samples, gc_samples, total_in_window) =
                self.profiler_store.query(&live.query);
            self.send_ui_reply(
                live.client_id,
                StudioToUI::QueryProfilerResults {
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
                StudioToUI::FileTree {
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
                    StudioToUI::FileTreeDiff {
                        mount,
                        changes: file_tree_diff(&before, &after),
                    },
                );
            }
        }
    }

    fn send_ui_reply(&self, client_id: ClientId, msg: StudioToUI) {
        self.send_ui_message(client_id, msg, self.ui_format(client_id));
    }

    fn send_ui_error(&self, client_id: ClientId, message: String) {
        self.send_ui_reply(client_id, StudioToUI::Error { message });
    }

    fn send_ui_message(&self, client_id: ClientId, msg: StudioToUI, format: WireFormat) {
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

fn discover_runnable_builds(root_path: &Path) -> Result<Vec<RunnableBuild>, String> {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version=1"])
        .current_dir(root_path)
        .output()
        .map_err(|err| {
            format!(
                "failed to run cargo metadata in {}: {}",
                root_path.display(),
                err
            )
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed in {}\n{}\n{}",
            root_path.display(),
            stderr.trim(),
            stdout.trim()
        ));
    }

    let metadata = CargoMetadata::deserialize_json_lenient(&stdout)
        .map_err(|err| format!("failed to parse cargo metadata json: {err:?}"))?;

    let mut builds = Vec::new();
    let mut seen = HashSet::new();
    for package in metadata.packages {
        let has_bin_target = package
            .targets
            .iter()
            .any(|target| target.kind.iter().any(|kind| kind == "bin"));
        if has_bin_target && seen.insert(package.name.clone()) {
            builds.push(RunnableBuild {
                package: package.name,
            });
        }
    }
    builds.sort_by(|a, b| a.package.cmp(&b.package));
    Ok(builds)
}

fn terminal_grid_from_history(history: &[u8], cols: u16, rows: u16) -> TerminalGrid {
    let mut terminal =
        makepad_terminal_core::Terminal::new(cols.max(1) as usize, rows.max(1) as usize);
    terminal.process_bytes(history);
    let screen = terminal.screen();
    let total_rows = screen.total_rows();
    let start_row = total_rows.saturating_sub(rows as usize);
    let mut text = String::new();
    for row in start_row..total_rows {
        if let Some(row_slice) = screen.row_slice_virtual(row) {
            for col in 0..screen.cols() {
                let ch = row_slice.get(col).map(|cell| cell.codepoint).unwrap_or(' ');
                text.push(ch);
            }
        }
        if row + 1 < total_rows {
            text.push('\n');
        }
    }
    TerminalGrid { cols, rows, text }
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

fn map_platform_event_sample(sample: EventSample) -> StudioEventSample {
    StudioEventSample {
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
            if !arg.contains('=')
                && iter.peek().is_some_and(|next| !next.starts_with('-'))
            {
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
            let git_status = git_status_for_path(abs_path);
            backend_proto::FileTreeChange::Added {
                path: virtual_path,
                node_type,
                git_status,
            }
        }
        Err(_) => backend_proto::FileTreeChange::Removed { path: virtual_path },
    }
}

fn git_status_for_path(path: &Path) -> backend_proto::GitStatus {
    let repo_root = match find_repo_root(path) {
        Some(root) => root,
        None => return backend_proto::GitStatus::Unknown,
    };
    let rel = match path.strip_prefix(&repo_root) {
        Ok(rel) => rel,
        Err(_) => return backend_proto::GitStatus::Unknown,
    };
    let rel = rel.to_string_lossy().replace('\\', "/");
    if rel.is_empty() {
        return backend_proto::GitStatus::Clean;
    }

    let output = match Command::new("git")
        .arg("-C")
        .arg(&repo_root)
        .arg("status")
        .arg("--porcelain")
        .arg("--")
        .arg(&rel)
        .output()
    {
        Ok(output) => output,
        Err(_) => return backend_proto::GitStatus::Unknown,
    };
    if !output.status.success() {
        return backend_proto::GitStatus::Unknown;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().next().unwrap_or("").trim_end();
    if line.is_empty() {
        return backend_proto::GitStatus::Clean;
    }
    if line.starts_with("??") {
        return backend_proto::GitStatus::Untracked;
    }
    let mut chars = line.chars();
    let x = chars.next().unwrap_or(' ');
    let y = chars.next().unwrap_or(' ');
    if x == 'U' || y == 'U' {
        return backend_proto::GitStatus::Conflict;
    }
    if x == 'A' {
        return backend_proto::GitStatus::Added;
    }
    if x == 'D' || y == 'D' {
        return backend_proto::GitStatus::Deleted;
    }
    if x != ' ' && x != '?' {
        return backend_proto::GitStatus::Staged;
    }
    if y != ' ' {
        return backend_proto::GitStatus::Modified;
    }
    backend_proto::GitStatus::Clean
}

fn find_repo_root(path: &Path) -> Option<PathBuf> {
    let mut dir = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()?.to_path_buf()
    };
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
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
    use makepad_network::ToUIReceiver;
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

    fn test_core_with_ui(root: &Path) -> (StudioCore, ToUIReceiver<Vec<u8>>) {
        let (event_tx, event_rx) = mpsc::channel::<StudioEvent>();
        let mut vfs = VirtualFs::new();
        vfs.mount("repo", root.to_path_buf()).expect("mount repo");
        let mut core = StudioCore::new(event_rx, event_tx, vfs, None);

        let ui_rx = ToUIReceiver::<Vec<u8>>::default();
        core.handle_event(StudioEvent::UiConnected {
            web_socket_id: 1,
            sender: ui_rx.sender(),
            typed_sender: None,
        });
        let _ = ui_rx.receiver.recv_timeout(Duration::from_millis(250)); // hello
        (core, ui_rx)
    }

    fn pump_core(core: &mut StudioCore, max_wait: Duration) {
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

    fn recv_ui_messages(rx: &ToUIReceiver<Vec<u8>>, max_wait: Duration) -> Vec<StudioToUI> {
        let deadline = Instant::now() + max_wait;
        let mut out = Vec::new();
        while Instant::now() < deadline {
            match rx.receiver.recv_timeout(Duration::from_millis(25)) {
                Ok(data) => {
                    if let Ok(msg) = StudioToUI::deserialize_bin(&data) {
                        out.push(msg);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        out
    }

    #[test]
    fn ui_envelope_uses_typed_channel_for_in_process_clients() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

        let (event_tx, event_rx) = mpsc::channel::<StudioEvent>();
        let mut vfs = VirtualFs::new();
        vfs.mount("repo", dir.path().to_path_buf()).expect("mount repo");
        let mut core = StudioCore::new(event_rx, event_tx, vfs, None);

        let ui_rx_bin = ToUIReceiver::<Vec<u8>>::default();
        let ui_rx_typed = ToUIReceiver::<StudioToUI>::default();
        core.handle_event(StudioEvent::UiConnected {
            web_socket_id: 1,
            sender: ui_rx_bin.sender(),
            typed_sender: Some(ui_rx_typed.sender()),
        });

        let hello = ui_rx_typed
            .receiver
            .recv_timeout(Duration::from_millis(250))
            .expect("typed hello");
        let client_id = match hello {
            StudioToUI::Hello { client_id } => client_id,
            other => panic!("expected Hello, got {:?}", other),
        };

        let query_id = QueryId::new(client_id, 0);
        core.handle_event(StudioEvent::UiEnvelope {
            web_socket_id: 1,
            envelope: UIToStudioEnvelope {
                query_id,
                msg: UIToStudio::LoadFileTree {
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
            StudioToUI::FileTree { mount, data } => {
                assert_eq!(mount, "repo");
                assert!(data.nodes.iter().any(|node| node.path == "repo/src/lib.rs"));
            }
            other => panic!("expected FileTree, got {:?}", other),
        }

        assert!(ui_rx_bin.receiver.try_recv().is_err());
    }

    #[test]
    fn ui_envelope_rejects_mismatched_client_id() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

        let (event_tx, event_rx) = mpsc::channel::<StudioEvent>();
        let mut vfs = VirtualFs::new();
        vfs.mount("repo", dir.path().to_path_buf()).expect("mount repo");
        let mut core = StudioCore::new(event_rx, event_tx, vfs, None);

        let ui_rx = ToUIReceiver::<Vec<u8>>::default();
        core.handle_event(StudioEvent::UiConnected {
            web_socket_id: 1,
            sender: ui_rx.sender(),
            typed_sender: None,
        });
        let hello_bin = ui_rx
            .receiver
            .recv_timeout(Duration::from_millis(250))
            .expect("hello");
        let client_id = match StudioToUI::deserialize_bin(&hello_bin).expect("deserialize hello") {
            StudioToUI::Hello { client_id } => client_id,
            other => panic!("expected Hello, got {:?}", other),
        };
        let wrong_client_id = if client_id.0 == 0 {
            ClientId(1)
        } else {
            ClientId(0)
        };

        core.handle_event(StudioEvent::UiEnvelope {
            web_socket_id: 1,
            envelope: UIToStudioEnvelope {
                query_id: QueryId::new(wrong_client_id, 0),
                msg: UIToStudio::ListBuilds,
            },
        });

        pump_core(&mut core, Duration::from_millis(250));
        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(300));
        assert!(messages.iter().any(|msg| {
            matches!(
                msg,
                StudioToUI::Error { message }
                    if message.contains("query_id.client_id does not match assigned client")
            )
        }));
    }

    #[test]
    fn ui_binary_rejects_mismatched_client_id() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

        let (event_tx, event_rx) = mpsc::channel::<StudioEvent>();
        let mut vfs = VirtualFs::new();
        vfs.mount("repo", dir.path().to_path_buf()).expect("mount repo");
        let mut core = StudioCore::new(event_rx, event_tx, vfs, None);

        let ui_rx = ToUIReceiver::<Vec<u8>>::default();
        core.handle_event(StudioEvent::UiConnected {
            web_socket_id: 1,
            sender: ui_rx.sender(),
            typed_sender: None,
        });
        let hello_bin = ui_rx
            .receiver
            .recv_timeout(Duration::from_millis(250))
            .expect("hello");
        let client_id = match StudioToUI::deserialize_bin(&hello_bin).expect("deserialize hello") {
            StudioToUI::Hello { client_id } => client_id,
            other => panic!("expected Hello, got {:?}", other),
        };
        let wrong_client_id = if client_id.0 == 0 {
            ClientId(1)
        } else {
            ClientId(0)
        };
        let data = UIToStudioEnvelope {
            query_id: QueryId::new(wrong_client_id, 0),
            msg: UIToStudio::ListBuilds,
        }
        .serialize_bin();

        core.handle_event(StudioEvent::UiBinary {
            web_socket_id: 1,
            data,
        });

        pump_core(&mut core, Duration::from_millis(250));
        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(300));
        assert!(messages.iter().any(|msg| {
            matches!(
                msg,
                StudioToUI::Error { message }
                    if message.contains("query_id.client_id does not match assigned client")
            )
        }));
    }

    #[test]
    fn secondary_ui_click_is_accepted_and_visualized_for_primary_observer() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

        let (event_tx, event_rx) = mpsc::channel::<StudioEvent>();
        let mut vfs = VirtualFs::new();
        vfs.mount("repo", dir.path().to_path_buf()).expect("mount repo");
        let mut core = StudioCore::new(event_rx, event_tx, vfs, None);

        let primary_ui = ToUIReceiver::<Vec<u8>>::default();
        core.handle_event(StudioEvent::UiConnected {
            web_socket_id: 1,
            sender: primary_ui.sender(),
            typed_sender: None,
        });
        let primary_client_id = match StudioToUI::deserialize_bin(
            &primary_ui
                .receiver
                .recv_timeout(Duration::from_millis(250))
                .expect("primary hello"),
        )
        .expect("decode primary hello")
        {
            StudioToUI::Hello { client_id } => client_id,
            other => panic!("expected Hello, got {:?}", other),
        };

        let secondary_ui = ToUIReceiver::<Vec<u8>>::default();
        core.handle_event(StudioEvent::UiConnected {
            web_socket_id: 2,
            sender: secondary_ui.sender(),
            typed_sender: None,
        });
        let secondary_client_id = match StudioToUI::deserialize_bin(
            &secondary_ui
                .receiver
                .recv_timeout(Duration::from_millis(250))
                .expect("secondary hello"),
        )
        .expect("decode secondary hello")
        {
            StudioToUI::Hello { client_id } => client_id,
            other => panic!("expected Hello, got {:?}", other),
        };

        let build_id = QueryId::new(secondary_client_id, 42);
        core.build_mount_by_id.insert(build_id, "repo".to_string());

        let (app_tx, app_rx) = mpsc::channel::<Vec<u8>>();
        core.handle_event(StudioEvent::AppConnected {
            build_id,
            web_socket_id: 77,
            sender: app_tx,
        });

        core.handle_event(StudioEvent::UiEnvelope {
            web_socket_id: 1,
            envelope: UIToStudioEnvelope {
                query_id: QueryId::new(primary_client_id, 0),
                msg: UIToStudio::ObserveMount {
                    mount: "repo".to_string(),
                    primary: Some(true),
                },
            },
        });

        core.handle_event(StudioEvent::UiEnvelope {
            web_socket_id: 2,
            envelope: UIToStudioEnvelope {
                query_id: QueryId::new(secondary_client_id, 0),
                msg: UIToStudio::Click {
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
                StudioToUI::RunViewInputViz {
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
                StudioToUI::RunViewInputViz {
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
            .any(|msg| matches!(msg, StudioToUI::Error { .. })));
    }

    #[test]
    fn mount_fs_changed_file_path_emits_added_diff() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

        let (mut core, ui_rx) = test_core_with_ui(dir.path());
        fs::write(dir.path().join("src/new_file.rs"), "pub fn new_file() {}\n").unwrap();
        core.handle_event(StudioEvent::MountFsChanged {
            mount: "repo".to_string(),
            path: dir.path().join("src/new_file.rs"),
        });

        pump_core(&mut core, Duration::from_millis(400));
        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(300));
        assert!(
            messages.iter().any(|msg| {
                matches!(
                    msg,
                    StudioToUI::FileTreeDiff { mount, changes }
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
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

        let (mut core, ui_rx) = test_core_with_ui(dir.path());
        core.mount_suppress_fs_until
            .insert("repo".to_string(), Instant::now() + Duration::from_secs(2));
        fs::write(dir.path().join("src/new_file.rs"), "pub fn new_file() {}\n").unwrap();
        core.handle_event(StudioEvent::MountFsChanged {
            mount: "repo".to_string(),
            path: dir.path().join("src/new_file.rs"),
        });

        pump_core(&mut core, Duration::from_millis(400));
        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(300));
        assert!(
            messages.iter().any(|msg| {
                matches!(
                    msg,
                    StudioToUI::FileTreeDiff { mount, changes }
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
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

        let (mut core, ui_rx) = test_core_with_ui(dir.path());
        core.mount_suppress_fs_until
            .insert("repo".to_string(), Instant::now() + Duration::from_secs(2));
        core.handle_event(StudioEvent::MountFsChanged {
            mount: "repo".to_string(),
            path: dir.path().to_path_buf(),
        });

        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(350));
        assert!(
            !messages.iter().any(|msg| {
                matches!(
                    msg,
                    StudioToUI::FileTree { mount, .. } | StudioToUI::FileTreeDiff { mount, .. }
                        if mount == "repo"
                ) || matches!(msg, StudioToUI::FileChanged { path } if path == "repo")
            }),
            "expected mount-root fs event to remain suppressed"
        );
    }

    #[test]
    fn mount_fs_changed_directory_path_triggers_full_tree_reload() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

        let (mut core, ui_rx) = test_core_with_ui(dir.path());
        fs::write(dir.path().join("src/from_dir_event.rs"), "pub fn d() {}\n").unwrap();
        core.handle_event(StudioEvent::MountFsChanged {
            mount: "repo".to_string(),
            path: dir.path().join("src"),
        });

        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(350));
        assert!(
            messages.iter().any(|msg| {
                matches!(
                    msg,
                    StudioToUI::FileTree { mount, data }
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
    fn mount_fs_changed_removed_directory_emits_removed_diff() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src/nested")).unwrap();
        fs::write(dir.path().join("src/nested/mod.rs"), "pub fn nested() {}\n").unwrap();

        let (mut core, ui_rx) = test_core_with_ui(dir.path());
        fs::remove_dir_all(dir.path().join("src/nested")).unwrap();
        core.handle_event(StudioEvent::MountFsChanged {
            mount: "repo".to_string(),
            path: dir.path().join("src/nested"),
        });

        pump_core(&mut core, Duration::from_millis(400));
        let messages = recv_ui_messages(&ui_rx, Duration::from_millis(300));
        assert!(
            messages.iter().any(|msg| {
                matches!(
                    msg,
                    StudioToUI::FileTreeDiff { mount, changes }
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
    fn worker_deltas_batch_and_coalesce_removed_descendants() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src/nested")).unwrap();
        let (mut core, ui_rx) = test_core_with_ui(dir.path());

        core.handle_event(StudioEvent::WorkerFileTreeDeltaDone {
            mount: "repo".to_string(),
            change: backend_proto::FileTreeChange::Removed {
                path: "repo/src/nested/a.rs".to_string(),
            },
        });
        core.handle_event(StudioEvent::WorkerFileTreeDeltaDone {
            mount: "repo".to_string(),
            change: backend_proto::FileTreeChange::Removed {
                path: "repo/src/nested/b.rs".to_string(),
            },
        });
        core.handle_event(StudioEvent::WorkerFileTreeDeltaDone {
            mount: "repo".to_string(),
            change: backend_proto::FileTreeChange::Removed {
                path: "repo/src/nested".to_string(),
            },
        });
        core.handle_event(StudioEvent::WorkerFileTreeDeltaDone {
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
                StudioToUI::FileTreeDiff { mount, changes } if mount == "repo" => Some(changes),
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
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        let (mut core, ui_rx) = test_core_with_ui(dir.path());

        core.handle_event(StudioEvent::WorkerFileTreeDeltaDone {
            mount: "repo".to_string(),
            change: backend_proto::FileTreeChange::Removed {
                path: "repo/src/lib.rs".to_string(),
            },
        });
        core.handle_event(StudioEvent::WorkerFileTreeDeltaDone {
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
                StudioToUI::FileTreeDiff { mount, changes } if mount == "repo" => Some(changes),
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
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

        let (mut core, ui_rx) = test_core_with_ui(dir.path());
        for index in 0..(FS_DELTA_RELOAD_THRESHOLD + 16) {
            core.handle_event(StudioEvent::WorkerFileTreeDeltaDone {
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
            .any(|msg| matches!(msg, StudioToUI::FileTree { mount, .. } if mount == "repo"));
        let saw_diff = messages
            .iter()
            .any(|msg| matches!(msg, StudioToUI::FileTreeDiff { mount, .. } if mount == "repo"));
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
    dir.push("makepad_studio_backend");
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
