use crate::log_store::{
    query_log_entries, AppendLogEntry, LogQuery, LogStore, ProfilerQuery, ProfilerStore,
};
use crate::process_manager::ProcessManager;
use crate::protocol::{
    BuildBoxInfo, BuildBoxStatus, BuildBoxToStudio, BuildBoxToStudioVec, BuildInfo, ClientId,
    EventSample as StudioEventSample, GCSample as StudioGCSample, GPUSample as StudioGPUSample,
    LogEntry, LogLevel, LogSource, QueryId, RunnableBuild, SaveResult, StudioToBuildBox,
    StudioToBuildBoxVec, StudioToUI, TerminalGrid, UIToStudio, UIToStudioEnvelope,
};
use makepad_live_id::LiveId;
use makepad_network::ToUISender;
use crate::terminal_manager::TerminalManager;
use crate::virtual_fs::{protocol_search_results, VirtualFs};
use crate::worker_pool::WorkerPool;
use makepad_studio_protocol::{
    AppToStudio, AppToStudioVec, EventSample, GCSample, GPUSample, KeyCode, KeyEvent,
    KeyModifiers, LogLevel as StudioProtocolLogLevel, MouseButton, RemoteKeyModifiers,
    RemoteMouseDown, RemoteMouseUp, ScreenshotRequest, StudioToApp, StudioToAppVec, TextInputEvent,
    WidgetTreeDumpRequest,
};
use makepad_micro_serde::*;
use makepad_filesystem_watcher::{FileSystemWatcher, WatchRoot};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
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
    },
    UiDisconnected {
        web_socket_id: u64,
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
        web_socket_id: u64,
        query_id: QueryId,
        for_search: bool,
        result: Result<Vec<String>, String>,
    },
    WorkerQueryLogsDone {
        web_socket_id: u64,
        query_id: QueryId,
        query: LogQuery,
        live: bool,
        entries: Vec<(usize, LogEntry)>,
    },
    MountFsChanged {
        mount: String,
    },
    Shutdown,
}

struct UiClient {
    client_id: ClientId,
    sender: ToUISender<Vec<u8>>,
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
    web_socket_id: u64,
    query: LogQuery,
}

struct LiveProfilerSubscription {
    web_socket_id: u64,
    query: ProfilerQuery,
}

pub struct StudioCore {
    rx: Receiver<StudioEvent>,
    event_tx: Sender<StudioEvent>,
    pub vfs: VirtualFs,
    studio_addr: Option<String>,
    next_client_id: u16,
    ui_clients: HashMap<u64, UiClient>,
    app_sockets: HashMap<u64, AppSocket>,
    buildbox_sockets: HashMap<u64, BuildBoxSocket>,
    buildbox_by_name: HashMap<String, u64>,
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
    mount_last_fs_event: HashMap<String, Instant>,
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
            next_client_id: 0,
            ui_clients: HashMap::new(),
            app_sockets: HashMap::new(),
            buildbox_sockets: HashMap::new(),
            buildbox_by_name: HashMap::new(),
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
            mount_last_fs_event: HashMap::new(),
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
            } => self.on_ui_connected(web_socket_id, sender),
            StudioEvent::UiDisconnected { web_socket_id } => {
                self.ui_clients.remove(&web_socket_id);
                self.live_log_queries
                    .retain(|_, query| query.web_socket_id != web_socket_id);
                self.live_profiler_queries
                    .retain(|_, query| query.web_socket_id != web_socket_id);
            }
            StudioEvent::UiBinary {
                web_socket_id,
                data,
            } => self.on_ui_message(web_socket_id, WireFormat::Binary, &data),
            StudioEvent::UiText {
                web_socket_id,
                text,
            } => self.on_ui_message(web_socket_id, WireFormat::Text, text.as_bytes()),
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
                web_socket_id,
                query_id,
                for_search,
                result,
            } => self.on_worker_find_files_done(web_socket_id, query_id, for_search, result),
            StudioEvent::WorkerQueryLogsDone {
                web_socket_id,
                query_id,
                query,
                live,
                entries,
            } => self.on_worker_query_logs_done(web_socket_id, query_id, query, live, entries),
            StudioEvent::MountFsChanged { mount } => self.on_mount_fs_changed(mount),
            StudioEvent::Shutdown => return false,
        }
        true
    }

    fn alloc_client_id(&mut self) -> Option<ClientId> {
        if self.next_client_id == u16::MAX {
            return None;
        }
        let id = ClientId(self.next_client_id);
        self.next_client_id = self.next_client_id.wrapping_add(1);
        Some(id)
    }

    fn on_ui_connected(&mut self, web_socket_id: u64, sender: ToUISender<Vec<u8>>) {
        let Some(client_id) = self.alloc_client_id() else {
            let _ = sender.send(
                StudioToUI::Error {
                    message: "client id space exhausted".to_string(),
                }
                .serialize_bin(),
            );
            return;
        };

        self.ui_clients.insert(
            web_socket_id,
            UiClient {
                client_id,
                sender,
                format: WireFormat::Binary,
            },
        );
        self.send_ui_message(
            web_socket_id,
            StudioToUI::Hello { client_id },
            WireFormat::Binary,
        );
    }

    fn on_ui_message(&mut self, web_socket_id: u64, format: WireFormat, data: &[u8]) {
        let Some(client) = self.ui_clients.get_mut(&web_socket_id) else {
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
                self.send_ui_error(web_socket_id, format!("invalid UI envelope: {}", err));
                return;
            }
        };

        if envelope.query_id.client_id() != client.client_id {
            self.send_ui_error(
                web_socket_id,
                "query_id.client_id does not match assigned client".to_string(),
            );
            return;
        }

        self.handle_ui_message(web_socket_id, envelope);
    }

    fn handle_ui_message(&mut self, web_socket_id: u64, envelope: UIToStudioEnvelope) {
        let query_id = envelope.query_id;
        match envelope.msg {
            UIToStudio::Mount { name, path } => match self.vfs.mount(&name, path) {
                Ok(()) => {
                    self.reset_fs_watcher();
                    match self.vfs.load_file_tree(&name) {
                    Ok(data) => self.send_ui_message(
                        web_socket_id,
                        StudioToUI::FileTree { mount: name, data },
                        self.ui_format(web_socket_id),
                    ),
                    Err(err) => self.send_ui_error(web_socket_id, err.to_string()),
                }
                }
                Err(err) => self.send_ui_error(web_socket_id, err.to_string()),
            },
            UIToStudio::Unmount { name } => {
                let changes = match self.vfs.load_file_tree(&name) {
                    Ok(tree) => tree
                        .nodes
                        .into_iter()
                        .map(|node| crate::protocol::FileTreeChange::Removed { path: node.path })
                        .collect(),
                    Err(_) => Vec::new(),
                };
                self.vfs.unmount(&name);
                self.reset_fs_watcher();
                self.send_ui_message(
                    web_socket_id,
                    StudioToUI::FileTree {
                        mount: name.clone(),
                        data: crate::protocol::FileTreeData { nodes: Vec::new() },
                    },
                    self.ui_format(web_socket_id),
                );
                self.send_ui_message(
                    web_socket_id,
                    StudioToUI::FileTreeDiff {
                        mount: name,
                        changes,
                    },
                    self.ui_format(web_socket_id),
                );
            }
            UIToStudio::LoadFileTree { mount } => match self.vfs.load_file_tree(&mount) {
                Ok(data) => self.send_ui_message(
                    web_socket_id,
                    StudioToUI::FileTree { mount, data },
                    self.ui_format(web_socket_id),
                ),
                Err(err) => self.send_ui_error(web_socket_id, err.to_string()),
            },
            UIToStudio::OpenTextFile { path } => match self.vfs.open_text_file(&path) {
                Ok(content) => self.send_ui_message(
                    web_socket_id,
                    StudioToUI::TextFileOpened {
                        path,
                        content,
                        git_status: crate::protocol::GitStatus::Unknown,
                    },
                    self.ui_format(web_socket_id),
                ),
                Err(err) => self.send_ui_error(web_socket_id, err.to_string()),
            },
            UIToStudio::ReadTextFile { path } => match self.vfs.read_text_file(&path) {
                Ok(content) => self.send_ui_message(
                    web_socket_id,
                    StudioToUI::TextFileRead { path, content },
                    self.ui_format(web_socket_id),
                ),
                Err(err) => self.send_ui_error(web_socket_id, err.to_string()),
            },
            UIToStudio::SaveTextFile { path, content } => {
                let result = match self.vfs.save_text_file(&path, &content) {
                    Ok(()) => SaveResult::Ok,
                    Err(err) => SaveResult::Err(err.into()),
                };
                self.send_ui_message(
                    web_socket_id,
                    StudioToUI::TextFileSaved { path, result },
                    self.ui_format(web_socket_id),
                );
            }
            UIToStudio::DeleteFile { path } => {
                self.terminal_manager.close_terminal(&path);
                if let Err(err) = self.vfs.delete_path(&path) {
                    self.send_ui_error(web_socket_id, err.to_string());
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
                        web_socket_id,
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
                        web_socket_id,
                        query_id,
                        for_search: true,
                        result,
                    });
                });
            }
            UIToStudio::GitLog { mount, max_count } => {
                match self.vfs.git_log(&mount, max_count.unwrap_or(100)) {
                    Ok(log) => self.send_ui_message(
                        web_socket_id,
                        StudioToUI::GitLog { mount, log },
                        self.ui_format(web_socket_id),
                    ),
                    Err(err) => self.send_ui_error(web_socket_id, err.to_string()),
                }
            }
            UIToStudio::CreateBranch {
                mount,
                name,
                from_ref,
            } => {
                let before = self.vfs.load_file_tree(&mount).ok();
                if let Err(err) = self.vfs.create_branch(&mount, &name, from_ref.as_deref()) {
                    self.send_ui_error(web_socket_id, err.to_string());
                    return;
                }
                match self.vfs.load_file_tree(&mount) {
                    Ok(data) => self.send_ui_message(
                        web_socket_id,
                        StudioToUI::FileTree {
                            mount: mount.clone(),
                            data: data.clone(),
                        },
                        self.ui_format(web_socket_id),
                    ),
                    Err(err) => self.send_ui_error(web_socket_id, err.to_string()),
                }
                if let Some(before) = before {
                    if let Ok(after) = self.vfs.load_file_tree(&mount) {
                        self.send_ui_message(
                            web_socket_id,
                            StudioToUI::FileTreeDiff {
                                mount,
                                changes: file_tree_diff(&before, &after),
                            },
                            self.ui_format(web_socket_id),
                        );
                    }
                }
            }
            UIToStudio::DeleteBranch { mount, name } => {
                let before = self.vfs.load_file_tree(&mount).ok();
                if let Err(err) = self.vfs.delete_branch(&mount, &name) {
                    self.send_ui_error(web_socket_id, err.to_string());
                    return;
                }
                match self.vfs.load_file_tree(&mount) {
                    Ok(data) => self.send_ui_message(
                        web_socket_id,
                        StudioToUI::FileTree {
                            mount: mount.clone(),
                            data: data.clone(),
                        },
                        self.ui_format(web_socket_id),
                    ),
                    Err(err) => self.send_ui_error(web_socket_id, err.to_string()),
                }
                if let Some(before) = before {
                    if let Ok(after) = self.vfs.load_file_tree(&mount) {
                        self.send_ui_message(
                            web_socket_id,
                            StudioToUI::FileTreeDiff {
                                mount,
                                changes: file_tree_diff(&before, &after),
                            },
                            self.ui_format(web_socket_id),
                        );
                    }
                }
            }
            UIToStudio::ListBuilds => {
                self.send_ui_message(
                    web_socket_id,
                    StudioToUI::Builds {
                        builds: self.list_all_builds(),
                    },
                    self.ui_format(web_socket_id),
                );
            }
            UIToStudio::LoadRunnableBuilds { mount } => {
                let cwd = match self.vfs.resolve_mount(&mount) {
                    Ok(cwd) => cwd,
                    Err(err) => {
                        self.send_ui_error(web_socket_id, err.to_string());
                        return;
                    }
                };
                match discover_runnable_builds(&cwd) {
                    Ok(builds) => self.send_ui_message(
                        web_socket_id,
                        StudioToUI::RunnableBuilds { mount, builds },
                        self.ui_format(web_socket_id),
                    ),
                    Err(err) => self.send_ui_error(web_socket_id, err),
                }
            }
            UIToStudio::CargoRun {
                mount,
                args,
                startup_query: _,
                env,
                buildbox,
            } => {
                if let Some(buildbox_name) = buildbox {
                    let package =
                        parse_package_name(&args).unwrap_or_else(|| "unknown".to_string());
                    let env = env.unwrap_or_default();
                    let msg = StudioToBuildBox::CargoBuild {
                        build_id: query_id,
                        mount: mount.clone(),
                        args,
                        env,
                    };
                    if let Err(err) = self.send_to_buildbox_name(&buildbox_name, msg) {
                        self.send_ui_error(web_socket_id, err);
                        return;
                    }

                    let info = BuildInfo {
                        build_id: query_id,
                        mount: mount.clone(),
                        package,
                        active: true,
                    };
                    self.remote_build_owner
                        .insert(query_id, buildbox_name.clone());
                    self.remote_builds.insert(query_id, info.clone());
                    self.set_buildbox_status(
                        &buildbox_name,
                        BuildBoxStatus::Building { build_id: query_id },
                    );
                    self.send_ui_message(
                        web_socket_id,
                        StudioToUI::BuildStarted {
                            build_id: info.build_id,
                            mount: info.mount,
                            package: info.package,
                        },
                        self.ui_format(web_socket_id),
                    );
                    return;
                }

                let cwd = match self.vfs.resolve_mount(&mount) {
                    Ok(cwd) => cwd,
                    Err(err) => {
                        self.send_ui_error(web_socket_id, err.to_string());
                        return;
                    }
                };
                match self.process_manager.start_cargo_run(
                    query_id,
                    mount.clone(),
                    &cwd,
                    args,
                    env.unwrap_or_default(),
                    self.studio_addr.clone(),
                    self.event_tx.clone(),
                ) {
                    Ok(info) => self.send_ui_message(
                        web_socket_id,
                        StudioToUI::BuildStarted {
                            build_id: info.build_id,
                            mount: info.mount,
                            package: info.package,
                        },
                        self.ui_format(web_socket_id),
                    ),
                    Err(err) => self.send_ui_error(web_socket_id, err),
                }
            }
            UIToStudio::StopBuild { build_id } => {
                if self.process_manager.stop_build(build_id).is_ok() {
                    return;
                }
                let Some(buildbox_name) = self.remote_build_owner.get(&build_id).cloned() else {
                    self.send_ui_error(web_socket_id, format!("unknown build: {}", build_id.0));
                    return;
                };
                if let Err(err) = self
                    .send_to_buildbox_name(&buildbox_name, StudioToBuildBox::StopBuild { build_id })
                {
                    self.send_ui_error(web_socket_id, err);
                }
            }
            UIToStudio::ForwardToApp { build_id, msg_bin } => {
                if let Err(err) = self.send_to_app(build_id, msg_bin) {
                    self.send_ui_error(web_socket_id, err);
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
                    self.send_ui_error(web_socket_id, err);
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
                    vec![
                        StudioToApp::KeyDown(key),
                        StudioToApp::KeyUp(key),
                    ],
                ) {
                    self.send_ui_error(web_socket_id, err);
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
                    self.send_ui_error(web_socket_id, err);
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
                    self.send_ui_error(web_socket_id, err);
                }
            }
            UIToStudio::WidgetTreeDump { build_id } => {
                if let Err(err) = self.send_app_msg(
                    build_id,
                    StudioToApp::WidgetTreeDump(WidgetTreeDumpRequest {
                        request_id: query_id.0,
                    }),
                ) {
                    self.send_ui_error(web_socket_id, err);
                }
            }
            UIToStudio::WidgetQuery { build_id, query } => {
                let _ = build_id;
                let _ = query;
                self.send_ui_error(
                    web_socket_id,
                    "WidgetQuery is not part of platform StudioToApp protocol".to_string(),
                );
            }
            UIToStudio::RunViewInput {
                build_id,
                window_id,
                msg_bin,
            } => {
                let _ = window_id;
                if let Err(err) = self.send_to_app(build_id, msg_bin) {
                    self.send_ui_error(web_socket_id, err);
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
                    self.send_ui_error(web_socket_id, err);
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
                        web_socket_id,
                        format!("invalid terminal path (missing mount): {}", path),
                    );
                    return;
                };
                let cwd = match self.vfs.resolve_mount(&mount) {
                    Ok(cwd) => cwd,
                    Err(err) => {
                        self.send_ui_error(web_socket_id, err.to_string());
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
                    Ok(()) => self.send_ui_message(
                        web_socket_id,
                        StudioToUI::TerminalOpened {
                            path: path.clone(),
                            grid: terminal_grid_from_history(&history, cols, rows),
                            history,
                        },
                        self.ui_format(web_socket_id),
                    ),
                    Err(err) => self.send_ui_error(web_socket_id, err),
                }
            }
            UIToStudio::TerminalInput { path, data } => {
                if let Err(err) = self.terminal_manager.send_input(&path, data) {
                    self.send_ui_error(web_socket_id, err);
                }
            }
            UIToStudio::TerminalResize { path, cols, rows } => {
                if let Err(err) = self.terminal_manager.resize(&path, cols, rows) {
                    self.send_ui_error(web_socket_id, err);
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
                        web_socket_id,
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
                self.send_ui_message(
                    web_socket_id,
                    StudioToUI::QueryProfilerResults {
                        query_id,
                        event_samples,
                        gpu_samples,
                        gc_samples,
                        total_in_window,
                        done: !live,
                    },
                    self.ui_format(web_socket_id),
                );
                if live {
                    self.live_profiler_queries.insert(
                        query_id,
                        LiveProfilerSubscription {
                            web_socket_id,
                            query,
                        },
                    );
                }
            }
            UIToStudio::CancelQuery { query_id } => {
                self.cancelled_queries.insert(query_id);
                self.live_log_queries.remove(&query_id);
                self.live_profiler_queries.remove(&query_id);
                self.send_ui_message(
                    web_socket_id,
                    StudioToUI::QueryCancelled { query_id },
                    self.ui_format(web_socket_id),
                );
            }
            UIToStudio::LogClear => {
                self.log_store.clear();
                self.send_ui_message(
                    web_socket_id,
                    StudioToUI::LogCleared,
                    self.ui_format(web_socket_id),
                );
            }
            UIToStudio::ListBuildBoxes => {
                self.send_ui_message(
                    web_socket_id,
                    StudioToUI::BuildBoxes {
                        boxes: self.list_buildboxes(),
                    },
                    self.ui_format(web_socket_id),
                );
            }
            UIToStudio::BuildBoxSyncNow { name } => {
                if let Err(err) =
                    self.send_to_buildbox_name(&name, StudioToBuildBox::RequestTreeHash)
                {
                    self.send_ui_error(web_socket_id, err);
                    return;
                }
                self.set_buildbox_status(&name, BuildBoxStatus::Syncing);
                self.send_ui_message(
                    web_socket_id,
                    StudioToUI::BuildBoxes {
                        boxes: self.list_buildboxes(),
                    },
                    self.ui_format(web_socket_id),
                );
            }
            UIToStudio::ListScriptTasks => {
                self.send_ui_message(
                    web_socket_id,
                    StudioToUI::ScriptTasks { tasks: Vec::new() },
                    self.ui_format(web_socket_id),
                );
            }
            other => {
                self.send_ui_error(
                    web_socket_id,
                    format!("message not implemented yet: {:?}", other),
                );
            }
        }
    }

    fn reset_fs_watcher(&mut self) {
        self.fs_watcher.take();
        self.mount_last_fs_event.clear();

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
            let _ = event_tx.send(StudioEvent::MountFsChanged { mount: event.mount });
        }) {
            Ok(watcher) => {
                self.fs_watcher = Some(watcher);
            }
            Err(err) => {
                eprintln!("[studio2-backend] filesystem watcher unavailable: {}", err);
            }
        }
    }

    fn on_mount_fs_changed(&mut self, mount: String) {
        let now = Instant::now();
        if let Some(last) = self.mount_last_fs_event.get(&mount) {
            if now.saturating_duration_since(*last) < Duration::from_millis(120) {
                return;
            }
        }
        self.mount_last_fs_event.insert(mount.clone(), now);
        self.broadcast_ui_message(StudioToUI::FileTreeDiff {
            mount,
            changes: Vec::new(),
        });
    }

    fn on_worker_find_files_done(
        &mut self,
        web_socket_id: u64,
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
                    self.send_ui_message(
                        web_socket_id,
                        StudioToUI::SearchFileResults {
                            query_id,
                            results: protocol_search_results(paths),
                            done: true,
                        },
                        self.ui_format(web_socket_id),
                    );
                } else {
                    self.send_ui_message(
                        web_socket_id,
                        StudioToUI::FindFileResults {
                            query_id,
                            paths,
                            done: true,
                        },
                        self.ui_format(web_socket_id),
                    );
                }
            }
            Err(err) => self.send_ui_error(web_socket_id, err),
        }
    }

    fn on_worker_query_logs_done(
        &mut self,
        web_socket_id: u64,
        query_id: QueryId,
        query: LogQuery,
        live: bool,
        entries: Vec<(usize, LogEntry)>,
    ) {
        if self.cancelled_queries.remove(&query_id) {
            return;
        }

        self.send_ui_message(
            web_socket_id,
            StudioToUI::QueryLogResults {
                query_id,
                entries,
                done: !live,
            },
            self.ui_format(web_socket_id),
        );

        if live && self.ui_clients.contains_key(&web_socket_id) {
            self.live_log_queries
                .insert(query_id, LiveLogSubscription { web_socket_id, query });
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

    fn send_app_msg(
        &self,
        build_id: QueryId,
        msg: StudioToApp,
    ) -> Result<(), String> {
        self.send_to_app(build_id, StudioToAppVec(vec![msg]).serialize_bin())
    }

    fn send_app_msgs(
        &self,
        build_id: QueryId,
        msgs: Vec<StudioToApp>,
    ) -> Result<(), String> {
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
            AppToStudio::CreateWindow { window_id, kind_id: _ } => {
                self.broadcast_ui_message(StudioToUI::RunViewCreated { build_id, window_id });
            }
            AppToStudio::SetCursor(cursor) => {
                self.broadcast_ui_message(StudioToUI::RunViewCursor {
                    build_id,
                    cursor: format!("{:?}", cursor),
                });
            }
            AppToStudio::DrawCompleteAndFlip(presentable_draw) => {
                self.broadcast_ui_message(StudioToUI::RunViewDrawComplete {
                    build_id,
                    window_id: presentable_draw.window_id,
                    presentable_draw,
                });
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
            | AppToStudio::ReadyToStart
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
        self.broadcast_ui_message(StudioToUI::BuildStopped {
            build_id,
            exit_code,
        });
    }

    fn on_terminal_output(&mut self, path: String, data: Vec<u8>) {
        if data.is_empty() {
            return;
        }
        if self.terminal_manager.mount_for_path(&path).is_none() {
            return;
        }
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
            self.send_ui_message(
                live.web_socket_id,
                StudioToUI::QueryLogResults {
                    query_id: *query_id,
                    entries: vec![(index, entry.clone())],
                    done: false,
                },
                self.ui_format(live.web_socket_id),
            );
        }
    }

    fn broadcast_ui_message(&self, msg: StudioToUI) {
        let ids: Vec<u64> = self.ui_clients.keys().copied().collect();
        for web_socket_id in ids {
            self.send_ui_message(web_socket_id, msg.clone(), self.ui_format(web_socket_id));
        }
    }

    fn send_to_query_owner(&self, query_id: QueryId, msg: StudioToUI) {
        let owner = query_id.client_id();
        let web_socket_id = self
            .ui_clients
            .iter()
            .find_map(|(socket_id, client)| (client.client_id == owner).then_some(*socket_id));
        let Some(web_socket_id) = web_socket_id else {
            return;
        };
        self.send_ui_message(web_socket_id, msg, self.ui_format(web_socket_id));
    }

    fn broadcast_live_profiler_queries(&self) {
        for (query_id, live) in &self.live_profiler_queries {
            let (event_samples, gpu_samples, gc_samples, total_in_window) =
                self.profiler_store.query(&live.query);
            self.send_ui_message(
                live.web_socket_id,
                StudioToUI::QueryProfilerResults {
                    query_id: *query_id,
                    event_samples,
                    gpu_samples,
                    gc_samples,
                    total_in_window,
                    done: false,
                },
                self.ui_format(live.web_socket_id),
            );
        }
    }

    fn ui_format(&self, web_socket_id: u64) -> WireFormat {
        self.ui_clients
            .get(&web_socket_id)
            .map(|v| v.format)
            .unwrap_or(WireFormat::Binary)
    }

    fn send_ui_error(&self, web_socket_id: u64, message: String) {
        self.send_ui_message(
            web_socket_id,
            StudioToUI::Error { message },
            self.ui_format(web_socket_id),
        );
    }

    fn send_ui_message(&self, web_socket_id: u64, msg: StudioToUI, format: WireFormat) {
        let Some(client) = self.ui_clients.get(&web_socket_id) else {
            return;
        };
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

fn map_platform_log_level(level: StudioProtocolLogLevel) -> LogLevel {
    match level {
        StudioProtocolLogLevel::Error | StudioProtocolLogLevel::Panic => {
            LogLevel::Error
        }
        StudioProtocolLogLevel::Warning | StudioProtocolLogLevel::Wait => {
            LogLevel::Warning
        }
        StudioProtocolLogLevel::Log => LogLevel::Log,
    }
}

fn map_platform_event_sample(sample: EventSample) -> StudioEventSample {
    StudioEventSample {
        at: sample.start,
        label: LiveId(sample.event_u32 as u64),
    }
}

fn map_platform_gpu_sample(sample: GPUSample) -> StudioGPUSample {
    StudioGPUSample {
        at: sample.end,
        label: LiveId(0),
    }
}

fn map_platform_gc_sample(sample: GCSample) -> StudioGCSample {
    StudioGCSample {
        at: sample.end,
        label: LiveId(0),
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

fn file_tree_diff(
    before: &crate::protocol::FileTreeData,
    after: &crate::protocol::FileTreeData,
) -> Vec<crate::protocol::FileTreeChange> {
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
            changes.push(crate::protocol::FileTreeChange::Removed {
                path: node.path.clone(),
            });
        }
    }
    for node in &after.nodes {
        match before_by_path.get(node.path.as_str()) {
            None => changes.push(crate::protocol::FileTreeChange::Added {
                path: node.path.clone(),
                node_type: node.node_type.clone(),
                git_status: node.git_status,
            }),
            Some((_, before_status)) if *before_status != node.git_status => {
                changes.push(crate::protocol::FileTreeChange::Modified {
                    path: node.path.clone(),
                    git_status: node.git_status,
                });
            }
            Some(_) => {}
        }
    }

    changes.sort_by(|a, b| {
        let a_path = match a {
            crate::protocol::FileTreeChange::Added { path, .. } => path,
            crate::protocol::FileTreeChange::Removed { path } => path,
            crate::protocol::FileTreeChange::Modified { path, .. } => path,
        };
        let b_path = match b {
            crate::protocol::FileTreeChange::Added { path, .. } => path,
            crate::protocol::FileTreeChange::Removed { path } => path,
            crate::protocol::FileTreeChange::Modified { path, .. } => path,
        };
        a_path.cmp(b_path)
    });
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

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
