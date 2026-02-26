use crate::log_store::{
    AppendLogEntry, LogQuery, LogStore, ProfilerQuery, ProfilerStore,
};
use crate::process_manager::ProcessManager;
use crate::protocol::{
    AppToStudioMsg, AppToStudioVec, BuildBoxInfo, BuildBoxStatus, BuildBoxToStudio,
    BuildBoxToStudioVec, BuildInfo, ClientId, LogEntry, LogLevel, LogSource, QueryId,
    SaveResult, StudioToAppMsg, StudioToAppVec, StudioToBuildBox, StudioToBuildBoxVec,
    StudioToUI, UIToStudio, UIToStudioEnvelope,
};
use crate::virtual_fs::{protocol_search_results, VirtualFs};
use makepad_micro_serde::*;
use std::collections::HashMap;
use std::fs;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireFormat {
    Binary,
    Text,
}

#[derive(Debug)]
pub enum StudioEvent {
    UiConnected {
        web_socket_id: u64,
        sender: Sender<Vec<u8>>,
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
    Shutdown,
}

struct UiClient {
    client_id: ClientId,
    sender: Sender<Vec<u8>>,
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
    live_log_queries: HashMap<QueryId, LiveLogSubscription>,
    live_profiler_queries: HashMap<QueryId, LiveProfilerSubscription>,
}

impl StudioCore {
    pub fn new(rx: Receiver<StudioEvent>, event_tx: Sender<StudioEvent>, vfs: VirtualFs) -> Self {
        Self {
            rx,
            event_tx,
            vfs,
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
            live_log_queries: HashMap::new(),
            live_profiler_queries: HashMap::new(),
        }
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

    fn on_ui_connected(&mut self, web_socket_id: u64, sender: Sender<Vec<u8>>) {
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
                Ok(()) => match self.vfs.load_file_tree(&name) {
                    Ok(data) => self.send_ui_message(
                        web_socket_id,
                        StudioToUI::FileTree { root: name, data },
                        self.ui_format(web_socket_id),
                    ),
                    Err(err) => self.send_ui_error(web_socket_id, err.to_string()),
                },
                Err(err) => self.send_ui_error(web_socket_id, err.to_string()),
            },
            UIToStudio::Unmount { name } => {
                self.vfs.unmount(&name);
                self.send_ui_message(
                    web_socket_id,
                    StudioToUI::FileTree {
                        root: name,
                        data: crate::protocol::FileTreeData { nodes: Vec::new() },
                    },
                    self.ui_format(web_socket_id),
                );
            }
            UIToStudio::LoadFileTree { root } => match self.vfs.load_file_tree(&root) {
                Ok(data) => self.send_ui_message(
                    web_socket_id,
                    StudioToUI::FileTree { root, data },
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
            UIToStudio::FindFiles {
                root,
                pattern,
                is_regex: _,
                max_results,
            } => match self.vfs.find_files(root.as_deref(), &pattern, max_results) {
                Ok(paths) => self.send_ui_message(
                    web_socket_id,
                    StudioToUI::FindFileResults {
                        query_id,
                        paths,
                        done: true,
                    },
                    self.ui_format(web_socket_id),
                ),
                Err(err) => self.send_ui_error(web_socket_id, err.to_string()),
            },
            UIToStudio::SearchFiles {
                root,
                pattern,
                is_regex: _,
                glob: _,
                max_results,
            } => match self.vfs.find_files(root.as_deref(), &pattern, max_results) {
                Ok(paths) => self.send_ui_message(
                    web_socket_id,
                    StudioToUI::SearchFileResults {
                        query_id,
                        results: protocol_search_results(paths),
                        done: true,
                    },
                    self.ui_format(web_socket_id),
                ),
                Err(err) => self.send_ui_error(web_socket_id, err.to_string()),
            },
            UIToStudio::GitLog { root, max_count } => {
                match self.vfs.git_log(&root, max_count.unwrap_or(100)) {
                    Ok(log) => self.send_ui_message(
                        web_socket_id,
                        StudioToUI::GitLog { root, log },
                        self.ui_format(web_socket_id),
                    ),
                    Err(err) => self.send_ui_error(web_socket_id, err.to_string()),
                }
            }
            UIToStudio::CreateBranch {
                root,
                name,
                from_ref,
            } => {
                if let Err(err) = self.vfs.create_branch(&root, &name, from_ref.as_deref()) {
                    self.send_ui_error(web_socket_id, err.to_string());
                    return;
                }
                match self.vfs.load_file_tree(&root) {
                    Ok(data) => self.send_ui_message(
                        web_socket_id,
                        StudioToUI::FileTree { root, data },
                        self.ui_format(web_socket_id),
                    ),
                    Err(err) => self.send_ui_error(web_socket_id, err.to_string()),
                }
            }
            UIToStudio::DeleteBranch { root, name } => {
                if let Err(err) = self.vfs.delete_branch(&root, &name) {
                    self.send_ui_error(web_socket_id, err.to_string());
                    return;
                }
                match self.vfs.load_file_tree(&root) {
                    Ok(data) => self.send_ui_message(
                        web_socket_id,
                        StudioToUI::FileTree { root, data },
                        self.ui_format(web_socket_id),
                    ),
                    Err(err) => self.send_ui_error(web_socket_id, err.to_string()),
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
            UIToStudio::CargoRun {
                root,
                args,
                startup_query: _,
                env,
                buildbox,
            } => {
                if let Some(buildbox_name) = buildbox {
                    let package = parse_package_name(&args).unwrap_or_else(|| "unknown".to_string());
                    let env = env.unwrap_or_default();
                    let msg = StudioToBuildBox::CargoBuild {
                        build_id: query_id,
                        root: root.clone(),
                        args,
                        env,
                    };
                    if let Err(err) = self.send_to_buildbox_name(&buildbox_name, msg) {
                        self.send_ui_error(web_socket_id, err);
                        return;
                    }

                    let info = BuildInfo {
                        build_id: query_id,
                        root: root.clone(),
                        package,
                        active: true,
                    };
                    self.remote_build_owner
                        .insert(query_id, buildbox_name.clone());
                    self.remote_builds.insert(query_id, info.clone());
                    self.set_buildbox_status(&buildbox_name, BuildBoxStatus::Building { build_id: query_id });
                    self.send_ui_message(
                        web_socket_id,
                        StudioToUI::BuildStarted {
                            build_id: info.build_id,
                            root: info.root,
                            package: info.package,
                        },
                        self.ui_format(web_socket_id),
                    );
                    return;
                }

                let cwd = match self.vfs.resolve_root(&root) {
                    Ok(cwd) => cwd,
                    Err(err) => {
                        self.send_ui_error(web_socket_id, err.to_string());
                        return;
                    }
                };
                match self.process_manager.start_cargo_run(
                    query_id,
                    root.clone(),
                    &cwd,
                    args,
                    env.unwrap_or_default(),
                    self.event_tx.clone(),
                ) {
                    Ok(info) => self.send_ui_message(
                        web_socket_id,
                        StudioToUI::BuildStarted {
                            build_id: info.build_id,
                            root: info.root,
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
                if let Err(err) = self.send_to_buildbox_name(
                    &buildbox_name,
                    StudioToBuildBox::StopBuild { build_id },
                ) {
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
                    StudioToAppMsg::TypeText {
                        text,
                        replace_last: false,
                        was_paste: false,
                    },
                ) {
                    self.send_ui_error(web_socket_id, err);
                }
            }
            UIToStudio::Return {
                build_id,
                auto_dump: _,
            } => {
                if let Err(err) = self.send_app_msg(build_id, StudioToAppMsg::Return) {
                    self.send_ui_error(web_socket_id, err);
                }
            }
            UIToStudio::Click { build_id, x, y } => {
                if let Err(err) = self.send_app_msg(
                    build_id,
                    StudioToAppMsg::Click { x, y, button: 1 },
                ) {
                    self.send_ui_error(web_socket_id, err);
                }
            }
            UIToStudio::Screenshot { build_id, kind_id } => {
                if let Err(err) = self.send_app_msg(
                    build_id,
                    StudioToAppMsg::ScreenshotRequest {
                        request_id: query_id.0,
                        kind_id: kind_id.unwrap_or(0),
                    },
                ) {
                    self.send_ui_error(web_socket_id, err);
                }
            }
            UIToStudio::WidgetTreeDump { build_id } => {
                if let Err(err) = self.send_app_msg(
                    build_id,
                    StudioToAppMsg::WidgetTreeDumpRequest {
                        request_id: query_id.0,
                    },
                ) {
                    self.send_ui_error(web_socket_id, err);
                }
            }
            UIToStudio::WidgetQuery { build_id, query } => {
                if let Err(err) = self.send_app_msg(
                    build_id,
                    StudioToAppMsg::WidgetQueryRequest {
                        request_id: query_id.0,
                        query,
                    },
                ) {
                    self.send_ui_error(web_socket_id, err);
                }
            }
            UIToStudio::RunViewInput {
                build_id,
                window_id,
                msg_bin,
            } => {
                if let Err(err) = self.send_app_msg(
                    build_id,
                    StudioToAppMsg::RunViewInput { window_id, msg_bin },
                ) {
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
                    StudioToAppMsg::RunViewResize {
                        window_id,
                        width,
                        height,
                        dpi,
                    },
                ) {
                    self.send_ui_error(web_socket_id, err);
                }
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
                let entries = self.log_store.query(&query);
                self.send_ui_message(
                    web_socket_id,
                    StudioToUI::QueryLogResults {
                        query_id,
                        entries,
                        done: !live,
                    },
                    self.ui_format(web_socket_id),
                );
                if live {
                    self.live_log_queries.insert(
                        query_id,
                        LiveLogSubscription {
                            web_socket_id,
                            query,
                        },
                    );
                }
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
                self.send_ui_message(web_socket_id, StudioToUI::LogCleared, self.ui_format(web_socket_id));
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

    fn send_app_msg(&self, build_id: QueryId, msg: StudioToAppMsg) -> Result<(), String> {
        self.send_to_app(build_id, StudioToAppVec(vec![msg]).serialize_bin())
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
                    self.set_buildbox_status(
                        &buildbox_name,
                        BuildBoxStatus::Building { build_id },
                    );
                }
            }
            BuildBoxToStudio::BuildStopped { build_id, exit_code } => {
                if let Some(buildbox_name) = self.remote_build_owner.remove(&build_id) {
                    self.remote_builds.remove(&build_id);
                    self.set_buildbox_status(&buildbox_name, BuildBoxStatus::Idle);
                }
                self.broadcast_ui_message(StudioToUI::BuildStopped { build_id, exit_code });
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

    fn handle_app_message(&mut self, build_id: QueryId, msg: AppToStudioMsg) {
        match msg {
            AppToStudioMsg::Log {
                level,
                message,
                file_name,
                line,
                column,
            } => {
                let (index, entry) = self.log_store.append(AppendLogEntry {
                    build_id: Some(build_id),
                    level,
                    source: LogSource::ChildApp,
                    message,
                    file_name,
                    line,
                    column,
                    timestamp: None,
                });
                self.broadcast_live_log_entry(index, entry);
            }
            AppToStudioMsg::EventSample(sample) => {
                self.profiler_store.append_event(Some(build_id), sample);
                self.broadcast_live_profiler_queries();
            }
            AppToStudioMsg::GPUSample(sample) => {
                self.profiler_store.append_gpu(Some(build_id), sample);
                self.broadcast_live_profiler_queries();
            }
            AppToStudioMsg::GCSample(sample) => {
                self.profiler_store.append_gc(Some(build_id), sample);
                self.broadcast_live_profiler_queries();
            }
            AppToStudioMsg::Screenshot {
                request_id,
                kind_id,
                png,
                width,
                height,
            } => {
                let query_id = QueryId(request_id);
                match write_screenshot_png(build_id, kind_id, request_id, &png) {
                    Ok(path) => self.send_to_query_owner(
                        query_id,
                        StudioToUI::Screenshot {
                            query_id,
                            build_id,
                            kind_id,
                            path,
                            width,
                            height,
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
            AppToStudioMsg::WidgetTreeDump { request_id, dump } => {
                let query_id = QueryId(request_id);
                self.send_to_query_owner(
                    query_id,
                    StudioToUI::WidgetTreeDump {
                        query_id,
                        build_id,
                        dump,
                    },
                );
            }
            AppToStudioMsg::WidgetQuery {
                request_id,
                query,
                rects,
            } => {
                let query_id = QueryId(request_id);
                self.send_to_query_owner(
                    query_id,
                    StudioToUI::WidgetQuery {
                        query_id,
                        build_id,
                        query,
                        rects,
                    },
                );
            }
            AppToStudioMsg::RunViewFrame {
                window_id,
                frame_id,
                width,
                height,
                codec,
                data,
            } => self.broadcast_ui_message(StudioToUI::RunViewFrame {
                build_id,
                window_id,
                frame_id,
                width,
                height,
                codec,
                data,
            }),
            AppToStudioMsg::RunViewDrawComplete {
                window_id,
                presented_image_id,
            } => self.broadcast_ui_message(StudioToUI::RunViewDrawComplete {
                build_id,
                window_id,
                presented_image_id,
            }),
            AppToStudioMsg::RunViewCursor { cursor } => {
                self.broadcast_ui_message(StudioToUI::RunViewCursor { build_id, cursor })
            }
        }
    }

    fn on_process_output(&mut self, build_id: QueryId, is_stderr: bool, line: String) {
        if line.is_empty() {
            return;
        }
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

    fn on_process_exited(&mut self, build_id: QueryId, exit_code: Option<i32>) {
        if self.process_manager.mark_exited(build_id, exit_code).is_none() {
            return;
        }
        self.broadcast_ui_message(StudioToUI::BuildStopped { build_id, exit_code });
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

fn classify_cargo_log_line(is_stderr: bool, line: &str) -> LogLevel {
    let lower = line.to_ascii_lowercase();
    if lower.contains("error") {
        return LogLevel::Error;
    }
    if lower.contains("warning") {
        return LogLevel::Warning;
    }
    if is_stderr {
        return LogLevel::Warning;
    }
    LogLevel::Log
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
