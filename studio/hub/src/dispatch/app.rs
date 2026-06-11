use super::*;
use makepad_studio_protocol::hub_protocol::{
    AppSocketInfo, BuildBoxInfo, BuildBoxStatus, BuildInfo, ClientId, HubToBuildBox,
    HubToBuildBoxVec, HubToClient, LogSource, QueryId, RunItem,
};
use makepad_studio_protocol::{
    AppToStudio, KeyCode, KeyEvent, KeyModifiers, LogLevel, MouseButton, RemoteKeyModifiers,
    RemoteMouseDown, RemoteMouseUp, ScreenshotRequest, StudioToApp, StudioToAppVec, TextInputEvent,
    WidgetQueryRequest, WidgetSnapshotRequest, WidgetTreeDumpRequest,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

impl HubCore {
    pub(super) fn send_to_app_with_socket(
        &self,
        build_id: QueryId,
        msg_bin: Vec<u8>,
    ) -> Result<u64, String> {
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

    pub(super) fn send_to_process_stdin(
        &self,
        build_id: QueryId,
        msg_bin: Vec<u8>,
    ) -> Result<(), String> {
        let msgs = StudioToAppVec::deserialize_bin(&msg_bin)
            .map_err(|err| format!("failed to decode app payload: {}", err.msg))?;
        for msg in msgs.0 {
            let mut line = msg.serialize_json();
            line.push('\n');
            self.build_manager.send_stdin(build_id, &line)?;
        }
        Ok(())
    }

    pub(super) fn queue_pending_forward_to_app(
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

    pub(super) fn merge_pending_bootstrap_msgs(
        existing: &[u8],
        incoming: &[u8],
    ) -> Option<Vec<u8>> {
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

    pub(super) fn flush_pending_forward_to_app(&mut self, build_id: QueryId) {
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

    pub(super) fn send_to_app(&self, build_id: QueryId, msg_bin: Vec<u8>) -> Result<(), String> {
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

    pub(super) fn build_ids_for_virtual_path(&self, virtual_path: &str) -> Vec<QueryId> {
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

    pub(super) fn virtual_path_matches_build_mount(virtual_path: &str, build_mount: &str) -> bool {
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

    pub(super) fn forward_live_change_to_builds(
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

    pub(super) fn send_app_msg(&self, build_id: QueryId, msg: StudioToApp) -> Result<(), String> {
        self.send_to_app(build_id, StudioToAppVec(vec![msg]).serialize_bin())
    }

    pub(super) fn send_app_msgs(
        &self,
        build_id: QueryId,
        msgs: Vec<StudioToApp>,
    ) -> Result<(), String> {
        self.send_to_app(build_id, StudioToAppVec(msgs).serialize_bin())
    }

    pub(super) fn send_to_buildbox_name(
        &self,
        name: &str,
        msg: HubToBuildBox,
    ) -> Result<(), String> {
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

    pub(super) fn list_buildboxes(&self) -> Vec<BuildBoxInfo> {
        let mut boxes: Vec<BuildBoxInfo> = self
            .buildbox_sockets
            .values()
            .filter_map(|socket| socket.info.clone())
            .collect();
        boxes.sort_by(|a, b| a.name.cmp(&b.name));
        boxes
    }

    pub(super) fn list_all_builds(&self) -> Vec<BuildInfo> {
        let mut builds = self.build_manager.list_builds();
        builds.extend(self.remote_builds.values().cloned());
        builds.sort_by_key(|build| build.build_id.0);
        builds
    }

    pub(super) fn build_info_for_id(&self, build_id: QueryId) -> Option<BuildInfo> {
        self.build_manager
            .list_builds()
            .into_iter()
            .find(|build| build.build_id == build_id)
            .or_else(|| self.remote_builds.get(&build_id).cloned())
    }

    pub(super) fn list_app_sockets(&self) -> Vec<AppSocketInfo> {
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

    pub(super) fn mount_has_root_splash(&self, mount: &str) -> bool {
        self.vfs
            .resolve_mount(mount)
            .map(|cwd| cwd.join(MAKEPAD_SPLASH_RUNNABLE).is_file())
            .unwrap_or(false)
    }

    pub(super) fn is_mount_root_splash_virtual_path(mount: &str, virtual_path: &str) -> bool {
        virtual_path == format!("{}/{}", mount, MAKEPAD_SPLASH_RUNNABLE)
    }

    pub(super) fn mount_root_splash_running(&self, mount: &str) -> bool {
        self.script_manager.is_running_for_mount(mount)
    }

    pub(super) fn ensure_mount_root_splash_running(&mut self, mount: &str) -> Result<bool, String> {
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

    pub(super) fn start_mount_root_splash_with_reporting(&mut self, mount: &str) {
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

    pub(super) fn maybe_revive_mount_root_splash_from_fs_fallback(&mut self, mount: &str) {
        if self.mount_root_splash_running(mount) {
            return;
        }
        if self.primary_ui_for_mount(mount).is_none() || !self.mount_has_root_splash(mount) {
            return;
        }
        self.start_mount_root_splash_with_reporting(mount);
    }

    pub(super) fn request_mount_root_splash_reload(&mut self, mount: &str) {
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

    pub(super) fn maybe_restart_pending_mount_root_splash(&mut self, mount: &str) {
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

    pub(super) fn primary_ui_for_mount(&self, mount: &str) -> Option<ClientId> {
        let client_id = self.primary_ui_by_mount.get(mount).copied()?;
        self.ui_clients
            .contains_key(&client_id)
            .then_some(client_id)
    }

    pub(super) fn primary_ui_for_build(&self, build_id: QueryId) -> Option<ClientId> {
        let mount = self.build_mount_by_id.get(&build_id)?;
        self.primary_ui_for_mount(mount)
    }

    pub(super) fn send_runview_message(&self, build_id: QueryId, msg: HubToClient) {
        if let Some(client_id) = self.primary_ui_for_build(build_id) {
            self.send_ui_message(client_id, msg, self.ui_format(client_id));
        } else {
            self.broadcast_ui_message(msg);
        }
    }

    pub(super) fn send_build_cleanup_message(&self, build_id: QueryId) {
        let msg = HubToClient::BuildCleared { build_id };
        if let Some(client_id) = self.primary_ui_for_build(build_id) {
            self.send_ui_reply(client_id, msg);
        } else {
            self.broadcast_ui_message(msg);
        }
    }

    pub(super) fn on_run_items_updated(&mut self, mount: String, items: Vec<RunItem>) {
        self.run_items_by_mount.insert(mount.clone(), items.clone());
        self.broadcast_ui_message(HubToClient::RunItems { mount, items });
    }

    pub(super) fn on_script_run_request(
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

    pub(super) fn on_app_binary(&mut self, build_id: QueryId, data: Vec<u8>) {
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

    pub(super) fn on_process_app_message(&mut self, build_id: QueryId, msg: AppToStudio) {
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

    pub(super) fn handle_app_message(&mut self, build_id: QueryId, msg: AppToStudio) {
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

    pub(super) fn on_script_output(
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

    pub(super) fn on_script_exited(
        &mut self,
        script_id: ScriptId,
        mount: String,
        exit_code: Option<i32>,
    ) {
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

    pub(super) fn on_process_output(&mut self, build_id: QueryId, is_stderr: bool, line: String) {
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

    pub(super) fn on_process_exited(&mut self, build_id: QueryId, exit_code: Option<i32>) {
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
}
