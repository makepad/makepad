use super::*;
use makepad_studio_protocol::hub_protocol::{
    ClientId, ClientToHub, ClientToHubEnvelope, HubToClient, QueryId, SaveResult, BuildInfo, BuildBoxStatus,
};
use makepad_studio_protocol::{
    KeyEvent, KeyModifiers, KeyCode, TextInputEvent, RemoteMouseDown, RemoteMouseUp, RemoteKeyModifiers, MouseButton,
    ScreenshotRequest, WidgetTreeDumpRequest, WidgetQueryRequest, WidgetSnapshotRequest, LogLevel,
};
use makepad_network::ToUISender;
use std::sync::Arc;
use std::time::Instant;

impl HubCore {
    pub(super) fn reserve_client_id(&mut self, client_id: ClientId) -> bool {
        let Some(slot) = self.client_id_in_use.get_mut(client_id.0 as usize) else {
            return false;
        };
        if *slot {
            return false;
        }
        *slot = true;
        true
    }

    pub(super) fn alloc_client_id(&mut self) -> Option<ClientId> {
        for client_id in 1..(MAX_UI_CLIENT_IDS as u16) {
            if self.reserve_client_id(ClientId(client_id)) {
                return Some(ClientId(client_id));
            }
        }
        None
    }

    pub(super) fn release_client_id(&mut self, client_id: ClientId) {
        if let Some(slot) = self.client_id_in_use.get_mut(client_id.0 as usize) {
            *slot = false;
        }
    }

    pub(super) fn alloc_build_id(&mut self) -> QueryId {
        let build_id = QueryId(self.next_build_id);
        self.next_build_id = self.next_build_id.wrapping_add(1);
        if self.next_build_id == 0 {
            self.next_build_id = 1;
        }
        build_id
    }

    pub(super) fn on_ui_connected(
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

    pub(super) fn on_ui_envelope(&mut self, client_id: ClientId, envelope: ClientToHubEnvelope) {
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

    pub(super) fn on_ui_message(&mut self, client_id: ClientId, format: WireFormat, data: &[u8]) {
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

    pub(super) fn handle_ui_message(&mut self, client_id: ClientId, envelope: ClientToHubEnvelope) {
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
            ClientToHub::AiConfigureBackend { mount, backend_id } => {
                let state = self.ai_manager.configure_backend(&mount, &backend_id);
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

    pub(super) fn broadcast_live_log_entry(&self, index: usize, entry: LogEntry) {
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

    pub(super) fn broadcast_ui_message(&self, msg: HubToClient) {
        let ids: Vec<ClientId> = self.ui_clients.keys().copied().collect();
        for client_id in ids {
            self.send_ui_message(client_id, msg.clone(), self.ui_format(client_id));
        }
    }

    pub(super) fn broadcast_ui_message_except(&self, excluded: ClientId, msg: HubToClient) {
        let ids: Vec<ClientId> = self.ui_clients.keys().copied().collect();
        for client_id in ids {
            if client_id == excluded {
                continue;
            }
            self.send_ui_message(client_id, msg.clone(), self.ui_format(client_id));
        }
    }

    pub(super) fn send_to_query_owner(&self, query_id: QueryId, msg: HubToClient) {
        let client_id = query_id.client_id();
        self.send_ui_reply(client_id, msg);
    }

    pub(super) fn broadcast_live_profiler_queries(&self) {
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

    pub(super) fn ui_format(&self, client_id: ClientId) -> WireFormat {
        self.ui_clients
            .get(&client_id)
            .map(|v| v.format)
            .unwrap_or(WireFormat::Binary)
    }

    pub(super) fn send_ui_reply(&self, client_id: ClientId, msg: HubToClient) {
        self.send_ui_message(client_id, msg, self.ui_format(client_id));
    }

    pub(super) fn send_ui_error(&self, client_id: ClientId, message: String) {
        self.send_ui_reply(client_id, HubToClient::Error { message });
    }

    pub(super) fn send_ui_message(&self, client_id: ClientId, msg: HubToClient, format: WireFormat) {
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
