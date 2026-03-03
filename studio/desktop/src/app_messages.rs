use super::*;

macro_rules! ui_file_sync_trace {
    ($($arg:tt)*) => {};
}

impl App {
    fn apply_editor_text_update(
        &mut self,
        cx: &mut Cx,
        path: String,
        content: String,
        allow_create_tab: bool,
    ) {
        self.data.pending_open_paths.remove(&path);
        self.data.pending_reload_paths.remove(&path);

        if !allow_create_tab && !self.data.path_to_tab.contains_key(&path) {
            ui_file_sync_trace!(
                "skip update path={} allow_create_tab={} reason=not-open",
                path,
                allow_create_tab
            );
            return;
        }

        let Some((tab_id, _)) = self.ensure_editor_tab_for_path(cx, &path, false) else {
            ui_file_sync_trace!("skip update path={} reason=no-tab", path);
            return;
        };

        if let Some(session) = self.data.sessions.get_mut(&tab_id) {
            if session.document().as_text().to_string() != content {
                ui_file_sync_trace!("replace session text path={} tab={}", path, tab_id.0);
                session.document().replace(content.into());
            } else {
                ui_file_sync_trace!(
                    "skip replace path={} tab={} reason=identical",
                    path,
                    tab_id.0
                );
            }
        } else {
            ui_file_sync_trace!("create session path={} tab={}", path, tab_id.0);
            self.data.sessions.insert(
                tab_id,
                CodeSession::new(CodeDocument::new(content.into(), DecorationSet::new())),
            );
        }

        self.apply_pending_log_jump(&path, tab_id);
        if let Some(mount) = Self::mount_from_virtual_path(&path) {
            if let Some(dock) = self.mount_workspace_dock(cx, mount) {
                dock.item(tab_id).redraw(cx);
                dock.redraw_tab(cx, tab_id);
            }
        }
    }

    pub(super) fn drain_studio_messages(&mut self, cx: &mut Cx) {
        loop {
            let Some(msg) = self
                .data
                .studio
                .as_ref()
                .and_then(|studio| studio.try_recv())
            else {
                break;
            };
            self.handle_studio_message(cx, msg);
        }
    }

    pub(super) fn handle_studio_message(&mut self, cx: &mut Cx, msg: HubToClient) {
        match msg {
            HubToClient::FileTree { mount, data } => {
                let _ = self.ensure_mount_tab(cx, &mount);
                self.mount_state_mut(&mount).file_tree_data = Some(data);
                self.ensure_mount_terminal_file(cx, &mount);
                if let Some(filter) = self
                    .mount_state(&mount)
                    .map(|mount| mount.file_filter.clone())
                    .filter(|filter| !filter.is_empty())
                {
                    self.set_mount_file_filter(cx, &mount, filter);
                }
                if self.data.active_mount.is_none() {
                    self.select_mount(cx, &mount);
                } else if self.data.active_mount.as_deref() == Some(mount.as_str()) {
                    self.refresh_active_mount_tree(cx);
                    self.set_status(cx, &format!("file tree loaded: {}", mount));
                }
            }
            HubToClient::TextFileOpened { path, content, .. } => {
                if Self::is_terminal_virtual_path(&path) {
                    return;
                }
                self.apply_editor_text_update(cx, path, content, true);
                self.set_status(cx, "opened file");
            }
            HubToClient::FileTreeDiff { mount, changes } => {
                self.apply_mount_file_tree_diff(cx, &mount, changes);
            }
            HubToClient::TextFileRead { path, content } => {
                if Self::is_terminal_virtual_path(&path) {
                    return;
                }
                let allow_create_tab = self.data.pending_open_paths.contains(&path);
                ui_file_sync_trace!(
                    "TextFileRead path={} allow_create_tab={} pending_reload={}",
                    path,
                    allow_create_tab,
                    self.data.pending_reload_paths.contains(&path)
                );
                self.apply_editor_text_update(cx, path, content, allow_create_tab);
            }
            HubToClient::TextFileSaved { path, result } => {
                if Self::is_terminal_virtual_path(&path) {
                    return;
                }
                self.set_status(cx, &format!("saved {} ({:?})", path, result));
            }
            HubToClient::FileChanged { path } => {
                if Self::is_terminal_virtual_path(&path) {
                    ui_file_sync_trace!("ignore FileChanged path={} reason=terminal", path);
                    return;
                }

                // Root-level watcher fallback: backend can emit mount names when
                // it only knows "something changed under this mount".
                if !path.contains('/') {
                    let mount = path;
                    let open_paths: Vec<String> = self
                        .data
                        .path_to_tab
                        .keys()
                        .filter(|open_path| {
                            Self::mount_from_virtual_path(open_path.as_str())
                                == Some(mount.as_str())
                        })
                        .cloned()
                        .collect();
                    if open_paths.is_empty() {
                        ui_file_sync_trace!(
                            "ignore mount-level FileChanged mount={} reason=no-open-tabs",
                            mount
                        );
                        return;
                    }
                    for open_path in open_paths {
                        if self.data.pending_reload_paths.insert(open_path.clone()) {
                            let _ = self.send_studio(ClientToHub::ReadTextFile {
                                path: open_path.clone(),
                            });
                        }
                    }
                    return;
                }

                if !self.data.path_to_tab.contains_key(&path) {
                    return;
                }

                if self.data.pending_reload_paths.insert(path.clone()) {
                    ui_file_sync_trace!("queue file reload path={}", path);
                    let _ = self.send_studio(ClientToHub::ReadTextFile { path });
                } else {
                    ui_file_sync_trace!("coalesce file reload path={}", path);
                }
            }
            HubToClient::FindFileResults {
                query_id,
                paths,
                done,
            } => {
                let Some(mount) = self.data.file_filter_mount_by_query.get(&query_id).cloned()
                else {
                    return;
                };
                if self
                    .mount_state(&mount)
                    .and_then(|mount| mount.file_filter_query)
                    != Some(query_id)
                {
                    return;
                }
                self.mount_state_mut(&mount).file_filter_results = paths;
                self.mount_state_mut(&mount).file_filter_pending = !done;
                if done {
                    self.mount_state_mut(&mount).file_filter_query = None;
                    self.data.file_filter_mount_by_query.remove(&query_id);
                }
                if self.data.active_mount.as_deref() == Some(mount.as_str()) {
                    self.refresh_active_mount_tree(cx);
                }
            }
            HubToClient::Builds { builds } => {
                for build in &builds {
                    self.data
                        .build_to_mount
                        .insert(build.build_id, build.mount.clone());
                }
                let Some(mount) = self.data.pending_stop_all_mount.take() else {
                    return;
                };
                let mut stop_count = 0usize;
                for build in builds {
                    if build.active && build.mount == mount {
                        if let Some(tab_id) =
                            self.data.run_tab_by_build.get(&build.build_id).copied()
                        {
                            if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                                dock.item(tab_id)
                                    .desktop_run_view(cx, ids!(run_view))
                                    .clear_run_target(cx);
                                dock.redraw_tab(cx, tab_id);
                            }
                        }
                        let _ = self.send_studio(ClientToHub::StopBuild {
                            build_id: build.build_id,
                        });
                        stop_count += 1;
                    }
                }
                self.set_status(
                    cx,
                    &format!("stop-all {}: {} running build(s)", mount, stop_count),
                );
            }
            HubToClient::RunnableBuilds { mount, builds } => {
                self.mount_state_mut(&mount).runnable_builds = builds;
                if self.data.active_mount.as_deref() == Some(mount.as_str()) {
                    self.refresh_active_mount_run_list(cx);
                    self.set_status(cx, &format!("run targets loaded: {}", mount));
                }
            }
            HubToClient::BuildStarted {
                build_id,
                mount,
                package,
            } => {
                let _ = self.ensure_mount_tab(cx, &mount);
                if self.data.active_mount.as_deref() != Some(mount.as_str()) {
                    self.select_mount(cx, &mount);
                }
                self.data
                    .profiler_running_by_build
                    .entry(build_id)
                    .or_insert(true);
                self.data.build_to_mount.insert(build_id, mount.clone());
                self.data.build_package.insert(build_id, package.clone());
                self.data
                    .active_log_build_by_mount
                    .insert(mount.clone(), build_id);
                let run_tab_id =
                    self.ensure_run_tab_for_build(cx, build_id, &mount, &package, true);
                let _ = self.ensure_log_tab_for_build(cx, build_id, &mount, &package, true);
                if let Some(tab_id) =
                    run_tab_id.or_else(|| self.data.run_tab_by_build.get(&build_id).copied())
                {
                    let mut window_id = None;
                    if let Some(state) = self.data.run_tab_state.get_mut(&tab_id) {
                        state.mount = mount.clone();
                        state.package = package.clone();
                        state.status = "running".to_string();
                        window_id = state.window_id;
                    }
                    if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                        dock.item(tab_id)
                            .desktop_run_view(cx, ids!(run_view))
                            .set_run_target(cx, build_id, window_id);
                        dock.redraw_tab(cx, tab_id);
                    }
                }
                if let Some(log_tab_id) = self.data.log_tab_by_build.get(&build_id).copied() {
                    if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                        dock.redraw_tab(cx, log_tab_id);
                    }
                }
                self.set_status(cx, &format!("build started: {}", package));
            }
            HubToClient::BuildStopped {
                build_id,
                exit_code,
            } => {
                self.data.build_to_mount.remove(&build_id);
                self.stop_profiler_query_for_build(build_id);
                self.data.profiler_running_by_build.insert(build_id, false);
                if let Some(tab_id) = self.data.run_tab_by_build.get(&build_id).copied() {
                    let mount = self
                        .data
                        .run_tab_state
                        .get(&tab_id)
                        .map(|state| state.mount.clone())
                        .unwrap_or_default();
                    if let Some(state) = self.data.run_tab_state.get_mut(&tab_id) {
                        state.status = match exit_code {
                            Some(code) => format!("stopped (exit code {})", code),
                            None => "stopped".to_string(),
                        };
                        state.window_id = None;
                    }
                    if !mount.is_empty() {
                        if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                            dock.item(tab_id)
                                .desktop_run_view(cx, ids!(run_view))
                                .clear_run_target(cx);
                            dock.redraw_tab(cx, tab_id);
                        }
                    }
                }
            }
            HubToClient::RunViewCreated {
                build_id,
                window_id,
            } => {
                let tab_id =
                    if let Some(tab_id) = self.data.run_tab_by_build.get(&build_id).copied() {
                        tab_id
                    } else {
                        let Some(mount) = self.data.build_to_mount.get(&build_id).cloned() else {
                            return;
                        };
                        let package = self
                            .data
                            .build_package
                            .get(&build_id)
                            .cloned()
                            .unwrap_or_else(|| format!("build {}", build_id.0));
                        if self.data.active_mount.as_deref() != Some(mount.as_str()) {
                            self.select_mount(cx, &mount);
                        }
                        let Some(tab_id) =
                            self.ensure_run_tab_for_build(cx, build_id, &mount, &package, true)
                        else {
                            return;
                        };
                        tab_id
                    };
                let mut mount = None;
                if let Some(state) = self.data.run_tab_state.get_mut(&tab_id) {
                    mount = Some(state.mount.clone());
                    if state.window_id.is_none() {
                        state.window_id = Some(window_id);
                    }
                }
                if let Some(mount) = mount {
                    if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                        let run_view = dock.item(tab_id).desktop_run_view(cx, ids!(run_view));
                        run_view.set_run_target(cx, build_id, Some(window_id));
                        run_view.rebootstrap_after_app_ready(cx, build_id, window_id);
                        dock.redraw_tab(cx, tab_id);
                    }
                }
            }
            HubToClient::RunViewDrawComplete {
                build_id,
                window_id,
                presentable_draw,
            } => {
                let Some(tab_id) = self.data.run_tab_by_build.get(&build_id).copied() else {
                    return;
                };
                let mut mount = None;
                let mut accepted_window = false;
                if let Some(state) = self.data.run_tab_state.get_mut(&tab_id) {
                    mount = Some(state.mount.clone());
                    if state.window_id.is_none() {
                        state.window_id = Some(window_id);
                    }
                    accepted_window = state.window_id == Some(window_id);
                }
                if !accepted_window {
                    return;
                }
                let Some(mount) = mount else {
                    return;
                };
                if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                    let run_view = dock.item(tab_id).desktop_run_view(cx, ids!(run_view));
                    run_view.set_run_target(cx, build_id, Some(window_id));
                    run_view.set_presentable_draw(cx, presentable_draw);
                    dock.redraw_tab(cx, tab_id);
                }
            }
            HubToClient::RunViewCursor { build_id, cursor } => {
                let Some(tab_id) = self.data.run_tab_by_build.get(&build_id).copied() else {
                    return;
                };
                let Some(mount) = self
                    .data
                    .run_tab_state
                    .get(&tab_id)
                    .map(|state| state.mount.clone())
                else {
                    return;
                };
                if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                    dock.item(tab_id)
                        .desktop_run_view(cx, ids!(run_view))
                        .set_remote_cursor(cx, Self::parse_run_view_cursor(&cursor));
                }
            }
            HubToClient::RunViewInputViz {
                build_id,
                kind,
                x,
                y,
            } => {
                let Some(tab_id) = self.data.run_tab_by_build.get(&build_id).copied() else {
                    return;
                };
                let Some(mount) = self
                    .data
                    .run_tab_state
                    .get(&tab_id)
                    .map(|state| state.mount.clone())
                else {
                    return;
                };
                if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                    dock.item(tab_id)
                        .desktop_run_view(cx, ids!(run_view))
                        .show_input_viz(cx, kind, x, y);
                }
            }
            HubToClient::QueryLogResults {
                query_id,
                entries,
                done: _,
            } => {
                if self.data.live_log_query != Some(query_id) {
                    return;
                }
                let mut touched_mounts = HashSet::new();
                for (_index, entry) in entries {
                    let Some(build_id) = entry.build_id else {
                        continue;
                    };
                    let Some(mount) = self.data.build_to_mount.get(&build_id).cloned() else {
                        continue;
                    };
                    let location = self.extract_log_location(&mount, &entry);
                    let log_entry = UiLogEntry {
                        level: entry.level,
                        source: entry.source,
                        message: entry.message,
                        location,
                    };
                    push_capped_deque(
                        self.data.build_log_entries.entry(build_id).or_default(),
                        log_entry.clone(),
                        2_000,
                    );
                    push_capped_deque(
                        &mut self.mount_state_mut(&mount).log_entries,
                        log_entry,
                        3_000,
                    );
                    touched_mounts.insert(mount);

                    if let Some(log_tab_id) = self.data.log_tab_by_build.get(&build_id).copied() {
                        if let Some(log_mount) = self.data.build_to_mount.get(&build_id).cloned() {
                            if let Some(dock) = self.mount_workspace_dock(cx, &log_mount) {
                                dock.redraw_tab(cx, log_tab_id);
                            }
                        }
                    }
                }
                touched_mounts.retain(|mount| !mount.is_empty());
                self.refresh_active_mount_log_panels(cx);
            }
            HubToClient::QueryProfilerResults {
                query_id,
                event_samples,
                gpu_samples,
                gc_samples,
                total_in_window,
                done,
            } => {
                let Some(build_id) = self
                    .data
                    .profiler_query_build_by_query
                    .get(&query_id)
                    .copied()
                else {
                    return;
                };
                if self.data.profiler_running_by_build.get(&build_id).copied() == Some(false) {
                    return;
                }
                self.data.profiler_samples_by_build.insert(
                    build_id,
                    UiProfilerSamples {
                        event_samples,
                        gpu_samples,
                        gc_samples,
                        total_in_window,
                    },
                );
                if done {
                    self.data.profiler_query_build_by_query.remove(&query_id);
                    if self.data.live_profiler_query_by_build.get(&build_id) == Some(&query_id) {
                        self.data.live_profiler_query_by_build.remove(&build_id);
                    }
                }
                if let Some(tab_id) = self.data.profiler_tab_by_build.get(&build_id).copied() {
                    let mount = self
                        .data
                        .profiler_tab_state
                        .get(&tab_id)
                        .map(|state| state.mount.clone());
                    if let Some(mount) = mount {
                        if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                            dock.redraw_tab(cx, tab_id);
                        }
                    }
                }
            }
            HubToClient::QueryCancelled { query_id } => {
                if let Some(build_id) = self.data.profiler_query_build_by_query.remove(&query_id) {
                    if self.data.live_profiler_query_by_build.get(&build_id) == Some(&query_id) {
                        self.data.live_profiler_query_by_build.remove(&build_id);
                    }
                }
            }
            HubToClient::TerminalOpened { path } => {
                self.data.terminal_open_paths.insert(path.clone());
                self.data
                    .terminal_framebuffer_by_path
                    .entry(path)
                    .or_default();
                self.refresh_active_mount_log_panels(cx);
            }
            HubToClient::TerminalFramebuffer { path, frame } => {
                self.data.terminal_framebuffer_by_path.insert(path, frame);
                self.refresh_active_mount_log_panels(cx);
            }
            HubToClient::TerminalExited { path, code } => {
                self.data.terminal_open_paths.remove(&path);
                self.data.terminal_framebuffer_by_path.remove(&path);
                self.set_status(cx, &format!("terminal exited ({})", code));
            }
            HubToClient::Error { message } => {
                self.data.pending_reload_paths.clear();
                self.set_status(cx, &format!("error: {}", message));
            }
            _ => {}
        }
    }

    fn parse_run_view_cursor(cursor: &str) -> MouseCursor {
        match cursor {
            "Hidden" => MouseCursor::Hidden,
            "Default" => MouseCursor::Default,
            "Crosshair" => MouseCursor::Crosshair,
            "Hand" => MouseCursor::Hand,
            "Arrow" => MouseCursor::Arrow,
            "Move" => MouseCursor::Move,
            "Text" => MouseCursor::Text,
            "Wait" => MouseCursor::Wait,
            "Help" => MouseCursor::Help,
            "NotAllowed" => MouseCursor::NotAllowed,
            "Grab" => MouseCursor::Grab,
            "Grabbing" => MouseCursor::Grabbing,
            "NResize" => MouseCursor::NResize,
            "NeResize" => MouseCursor::NeResize,
            "EResize" => MouseCursor::EResize,
            "SeResize" => MouseCursor::SeResize,
            "SResize" => MouseCursor::SResize,
            "SwResize" => MouseCursor::SwResize,
            "WResize" => MouseCursor::WResize,
            "NwResize" => MouseCursor::NwResize,
            "NsResize" => MouseCursor::NsResize,
            "NeswResize" => MouseCursor::NeswResize,
            "EwResize" => MouseCursor::EwResize,
            "NwseResize" => MouseCursor::NwseResize,
            "ColResize" => MouseCursor::ColResize,
            "RowResize" => MouseCursor::RowResize,
            _ => MouseCursor::Default,
        }
    }
}
