use super::*;

impl App {
    pub(super) fn tab_id_from_widget_uid(cx: &Cx, widget_uid: WidgetUid) -> LiveId {
        let path = cx.widget_tree().path_to(widget_uid);
        path.get(path.len().wrapping_sub(2))
            .copied()
            .unwrap_or(id!(editor_first))
    }

    pub(super) fn set_active_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
        if let Some(path) = self.data.tab_to_path.get(&tab_id).cloned() {
            self.data.current_file_path = Some(path.clone());
            self.set_current_file_label(cx, Some(&path));
        } else {
            self.data.current_file_path = None;
            self.set_current_file_label(cx, None);
        }
    }

    pub(super) fn ensure_editor_tab_for_path(
        &mut self,
        cx: &mut Cx,
        path: &str,
        select: bool,
    ) -> Option<(LiveId, bool)> {
        let mount = Self::mount_from_virtual_path(path)?;
        if select && self.data.active_mount.as_deref() != Some(mount) {
            self.select_mount(cx, mount);
        }
        let dock = self.mount_workspace_dock(cx, mount)?;

        if let Some(tab_id) = self.data.path_to_tab.get(path).copied() {
            if dock.find_tab_bar_of_tab(tab_id).is_some() {
                if select {
                    dock.select_tab(cx, tab_id);
                    self.set_active_tab(cx, tab_id);
                }
                return Some((tab_id, true));
            }
            self.data.path_to_tab.remove(path);
            self.data.tab_to_path.remove(&tab_id);
            self.data.sessions.remove(&tab_id);
        }

        let Some(anchor_tab_id) = self.find_editor_anchor_tab(&dock, mount) else {
            return None;
        };
        let (tab_bar, pos) = dock.find_tab_bar_of_tab(anchor_tab_id)?;
        let tab_id = dock.unique_id(LiveId::from_str(path).0);
        let created = if select {
            dock.create_and_select_tab(
                cx,
                tab_bar,
                tab_id,
                id!(CodeEditorPane),
                String::new(),
                id!(CloseableTab),
                Some(pos),
            )
        } else {
            dock.create_tab(
                cx,
                tab_bar,
                tab_id,
                id!(CodeEditorPane),
                String::new(),
                id!(CloseableTab),
                Some(pos),
            )
        };
        if created.is_none() {
            return None;
        }

        self.data.path_to_tab.insert(path.to_string(), tab_id);
        self.data.tab_to_path.insert(tab_id, path.to_string());
        self.update_editor_tab_titles(cx);

        if select {
            dock.select_tab(cx, tab_id);
            self.set_active_tab(cx, tab_id);
        }

        Some((tab_id, false))
    }

    pub(super) fn find_editor_anchor_tab(&self, dock: &DockRef, mount: &str) -> Option<LiveId> {
        if dock.find_tab_bar_of_tab(id!(editor_first)).is_some() {
            return Some(id!(editor_first));
        }
        for (tab_id, path) in &self.data.tab_to_path {
            if Self::mount_from_virtual_path(path) == Some(mount)
                && dock.find_tab_bar_of_tab(*tab_id).is_some()
            {
                return Some(*tab_id);
            }
        }
        None
    }

    pub(super) fn close_editor_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
        if !self.data.tab_to_path.contains_key(&tab_id) {
            return;
        }
        let mount = self
            .data
            .tab_to_path
            .get(&tab_id)
            .and_then(|path| Self::mount_from_virtual_path(path))
            .map(ToOwned::to_owned);
        if let Some(path) = self.data.tab_to_path.remove(&tab_id) {
            self.data.path_to_tab.remove(&path);
            self.data.sessions.remove(&tab_id);
            self.data.pending_open_paths.remove(&path);
            self.data.pending_reload_paths.remove(&path);
            if self.data.current_file_path.as_deref() == Some(path.as_str()) {
                self.data.current_file_path = None;
                self.set_current_file_label(cx, None);
            }
        }
        if let Some(mount) = mount {
            if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                dock.close_tab(cx, tab_id);
            }
        }
        self.update_editor_tab_titles(cx);
    }

    pub(super) fn update_editor_tab_titles(&mut self, cx: &mut Cx) {
        if self.data.tab_to_path.is_empty() {
            let mounts: Vec<String> = self.data.mounts.keys().cloned().collect();
            for mount in mounts {
                if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                    dock.set_tab_title(cx, id!(editor_first), String::new());
                }
            }
            return;
        }

        let mut parts_by_tab: HashMap<LiveId, Vec<String>> = HashMap::new();
        let mut depth_by_tab: HashMap<LiveId, usize> = HashMap::new();
        for (tab_id, path) in &self.data.tab_to_path {
            let mut parts: Vec<String> = path
                .split('/')
                .filter(|segment| !segment.is_empty())
                .map(|segment| segment.to_string())
                .collect();
            if parts.is_empty() {
                parts.push(path.clone());
            }
            parts_by_tab.insert(*tab_id, parts);
            depth_by_tab.insert(*tab_id, 1);
        }

        loop {
            let mut title_to_tabs: HashMap<String, Vec<LiveId>> = HashMap::new();
            for (tab_id, parts) in &parts_by_tab {
                let depth = depth_by_tab.get(tab_id).copied().unwrap_or(1);
                title_to_tabs
                    .entry(Self::title_suffix(parts, depth))
                    .or_default()
                    .push(*tab_id);
            }

            let mut changed = false;
            for tabs in title_to_tabs.values() {
                if tabs.len() <= 1 {
                    continue;
                }
                let expandable = tabs.iter().any(|tab_id| {
                    let depth = depth_by_tab.get(tab_id).copied().unwrap_or(1);
                    let part_count = parts_by_tab.get(tab_id).map_or(1, |parts| parts.len());
                    depth < part_count
                });
                if !expandable {
                    continue;
                }
                for tab_id in tabs {
                    let depth = depth_by_tab.get(tab_id).copied().unwrap_or(1);
                    let part_count = parts_by_tab.get(tab_id).map_or(1, |parts| parts.len());
                    let next = (depth + 1).min(part_count);
                    if next != depth {
                        depth_by_tab.insert(*tab_id, next);
                        changed = true;
                    }
                }
            }

            if !changed {
                break;
            }
        }

        for (tab_id, parts) in &parts_by_tab {
            let depth = depth_by_tab.get(tab_id).copied().unwrap_or(1);
            let mount = self
                .data
                .tab_to_path
                .get(tab_id)
                .and_then(|path| Self::mount_from_virtual_path(path))
                .map(ToOwned::to_owned);
            if let Some(mount) = mount {
                if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                    dock.set_tab_title(cx, *tab_id, Self::title_suffix(parts, depth));
                }
            }
        }
    }

    pub(super) fn title_suffix(parts: &[String], depth: usize) -> String {
        let count = parts.len();
        let take = depth.min(count);
        parts[count - take..].join("/")
    }

    pub(super) fn open_path_in_editor(&mut self, cx: &mut Cx, path: &str) {
        let path = path.to_string();
        let Some((tab_id, already_open)) = self.ensure_editor_tab_for_path(cx, &path, true) else {
            self.set_status(cx, "failed to create editor tab");
            return;
        };
        if already_open && self.data.sessions.contains_key(&tab_id) {
            self.set_status(cx, "focused open file");
            return;
        }
        if self.data.pending_open_paths.contains(&path) {
            self.set_status(cx, "opening...");
            return;
        }
        self.set_status(cx, &format!("opening {}", path));
        self.data.pending_open_paths.insert(path.clone());
        let _ = self.send_studio(UIToStudio::OpenTextFile { path });
    }

    pub(super) fn open_node_in_editor(&mut self, cx: &mut Cx, node_id: LiveId) {
        if !self.data.file_tree.is_file(node_id) {
            return;
        }
        let Some(path) = self.data.file_tree.path_for(node_id).map(ToOwned::to_owned) else {
            return;
        };
        self.open_path_in_editor(cx, &path);
    }

    pub(super) fn save_tab_file(&mut self, cx: &mut Cx, tab_id: LiveId) {
        let Some(path) = self.data.tab_to_path.get(&tab_id).cloned() else {
            return;
        };
        let Some(session) = self.data.sessions.get(&tab_id) else {
            return;
        };
        let content = session.document().as_text().to_string();
        let _ = self.send_studio(UIToStudio::SaveTextFile { path, content });
        self.set_status(cx, "saving...");
    }

    fn create_dock_tab(
        dock: &DockRef,
        cx: &mut Cx,
        anchor: LiveId,
        tab_id: LiveId,
        pane_id: LiveId,
        title: String,
        select: bool,
    ) -> Option<()> {
        let (tab_bar, pos) = dock.find_tab_bar_of_tab(anchor)?;
        let created = if select {
            dock.create_and_select_tab(
                cx, tab_bar, tab_id, pane_id, title, id!(CloseableTab), Some(pos),
            )
        } else {
            dock.create_tab(
                cx, tab_bar, tab_id, pane_id, title, id!(CloseableTab), Some(pos),
            )
        };
        created.map(|_| ())
    }

    fn find_anchor_tab_in<'a>(
        dock: &DockRef,
        default_id: LiveId,
        iter: impl Iterator<Item = (&'a LiveId, &'a str)>,
        mount: &str,
    ) -> Option<LiveId> {
        if dock.find_tab_bar_of_tab(default_id).is_some() {
            return Some(default_id);
        }
        for (tab_id, tab_mount) in iter {
            if tab_mount == mount && dock.find_tab_bar_of_tab(*tab_id).is_some() {
                return Some(*tab_id);
            }
        }
        None
    }

    pub(super) fn ensure_run_tab_for_build(
        &mut self,
        cx: &mut Cx,
        build_id: QueryId,
        mount: &str,
        package: &str,
        select: bool,
    ) -> Option<LiveId> {
        let dock = self.mount_workspace_dock(cx, mount)?;
        if let Some(tab_id) = self.data.run_tab_by_build.get(&build_id).copied() {
            if dock.find_tab_bar_of_tab(tab_id).is_some() {
                let window_id = self
                    .data
                    .run_tab_state
                    .get(&tab_id)
                    .and_then(|state| state.window_id);
                dock.item(tab_id)
                    .desktop_run_view(cx, ids!(run_view))
                    .set_run_target(cx, build_id, window_id);
                if select {
                    dock.select_tab(cx, tab_id);
                }
                return Some(tab_id);
            }
            self.data.run_tab_by_build.remove(&build_id);
            self.data.run_tab_state.remove(&tab_id);
        }

        let anchor = Self::find_anchor_tab_in(
            &dock, id!(run_first),
            self.data.run_tab_state.iter().map(|(id, s)| (id, s.mount.as_str())),
            mount,
        )?;
        let tab_id = dock.unique_id(LiveId::from_str(&format!("run/{}/{}", mount, build_id.0)).0);
        Self::create_dock_tab(&dock, cx, anchor, tab_id, id!(RunningAppPane), package.to_string(), select)?;

        self.data.run_tab_by_build.insert(build_id, tab_id);
        self.data.run_tab_state.insert(
            tab_id,
            RunTabState {
                mount: mount.to_string(),
                package: package.to_string(),
                build_id,
                status: "starting".to_string(),
                window_id: None,
            },
        );
        dock.set_tab_title(cx, tab_id, package.to_string());
        dock.item(tab_id)
            .desktop_run_view(cx, ids!(run_view))
            .set_run_target(cx, build_id, None);
        Some(tab_id)
    }

    pub(super) fn refresh_run_view_targets(&mut self, cx: &mut Cx) {
        let targets: Vec<(LiveId, String, QueryId, Option<usize>)> = self
            .data
            .run_tab_state
            .iter()
            .filter_map(|(tab_id, state)| {
                let active_mount = self.data.build_to_mount.get(&state.build_id)?;
                if active_mount != &state.mount {
                    return None;
                }
                Some((*tab_id, state.mount.clone(), state.build_id, state.window_id))
            })
            .collect();

        for (tab_id, mount, build_id, window_id) in targets {
            if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                dock.item(tab_id)
                    .desktop_run_view(cx, ids!(run_view))
                    .set_run_target(cx, build_id, window_id);
            }
        }
    }

    pub(super) fn ensure_log_tab_for_build(
        &mut self,
        cx: &mut Cx,
        build_id: QueryId,
        mount: &str,
        title: &str,
        select: bool,
    ) -> Option<LiveId> {
        let dock = self.mount_workspace_dock(cx, mount)?;
        if let Some(tab_id) = self.data.log_tab_by_build.get(&build_id).copied() {
            if dock.find_tab_bar_of_tab(tab_id).is_some() {
                if select {
                    dock.select_tab(cx, tab_id);
                    self.data
                        .active_log_build_by_mount
                        .insert(mount.to_string(), build_id);
                }
                return Some(tab_id);
            }
            self.data.log_tab_by_build.remove(&build_id);
            self.data.log_tab_state.remove(&tab_id);
        }

        let anchor = Self::find_anchor_tab_in(
            &dock, id!(log_first),
            self.data.log_tab_state.iter().map(|(id, s)| (id, s.mount.as_str())),
            mount,
        )?;
        let tab_id = dock.unique_id(LiveId::from_str(&format!("log/{}/{}", mount, build_id.0)).0);
        Self::create_dock_tab(&dock, cx, anchor, tab_id, id!(LogPane), title.to_string(), select)?;

        self.data.log_tab_by_build.insert(build_id, tab_id);
        self.data.log_tab_state.insert(
            tab_id,
            LogTabState {
                mount: mount.to_string(),
                build_id,
            },
        );
        dock.set_tab_title(cx, tab_id, title.to_string());
        if select {
            self.data
                .active_log_build_by_mount
                .insert(mount.to_string(), build_id);
        }
        Some(tab_id)
    }

    pub(super) fn find_profiler_anchor_tab(&self, dock: &DockRef, mount: &str) -> Option<LiveId> {
        if dock.find_tab_bar_of_tab(id!(log_first)).is_some() {
            return Some(id!(log_first));
        }
        for (tab_id, state) in &self.data.profiler_tab_state {
            if state.mount == mount && dock.find_tab_bar_of_tab(*tab_id).is_some() {
                return Some(*tab_id);
            }
        }
        for (tab_id, state) in &self.data.log_tab_state {
            if state.mount == mount && dock.find_tab_bar_of_tab(*tab_id).is_some() {
                return Some(*tab_id);
            }
        }
        None
    }

    pub(super) fn ensure_profiler_tab_for_build(
        &mut self,
        cx: &mut Cx,
        build_id: QueryId,
        mount: &str,
        title: &str,
        select: bool,
    ) -> Option<LiveId> {
        let dock = self.mount_workspace_dock(cx, mount)?;
        if let Some(tab_id) = self.data.profiler_tab_by_build.get(&build_id).copied() {
            if dock.find_tab_bar_of_tab(tab_id).is_some() {
                if select {
                    dock.select_tab(cx, tab_id);
                    self.data
                        .active_log_build_by_mount
                        .insert(mount.to_string(), build_id);
                }
                return Some(tab_id);
            }
            self.data.profiler_tab_by_build.remove(&build_id);
            self.data.profiler_tab_state.remove(&tab_id);
        }

        let anchor = self.find_profiler_anchor_tab(&dock, mount)?;
        let tab_id = dock.unique_id(LiveId::from_str(&format!("prof/{}/{}", mount, build_id.0)).0);
        Self::create_dock_tab(&dock, cx, anchor, tab_id, id!(ProfilerPane), title.to_string(), select)?;

        self.data.profiler_tab_by_build.insert(build_id, tab_id);
        self.data.profiler_tab_state.insert(
            tab_id,
            ProfilerTabState {
                mount: mount.to_string(),
                build_id,
                title: title.to_string(),
            },
        );
        dock.set_tab_title(cx, tab_id, title.to_string());
        Some(tab_id)
    }

    pub(super) fn start_profiler_query_for_build(&mut self, build_id: QueryId) {
        if let Some(prev_query_id) = self.data.live_profiler_query_by_build.remove(&build_id) {
            self.data
                .profiler_query_build_by_query
                .remove(&prev_query_id);
            let _ = self.send_studio(UIToStudio::CancelQuery {
                query_id: prev_query_id,
            });
        }
        let time_start = self
            .data
            .profiler_time_start_by_build
            .get(&build_id)
            .copied();
        let Some(query_id) = self.send_studio(UIToStudio::QueryProfiler {
            build_id: Some(build_id),
            sample_type: None,
            time_start,
            time_end: None,
            max_samples: Some(4096),
            live: Some(true),
        }) else {
            return;
        };
        self.data
            .live_profiler_query_by_build
            .insert(build_id, query_id);
        self.data
            .profiler_query_build_by_query
            .insert(query_id, build_id);
    }

    fn latest_profiler_sample_end(&self, build_id: QueryId) -> Option<f64> {
        let samples = self.data.profiler_samples_by_build.get(&build_id)?;
        [
            samples.event_samples.last().map(|sample| sample.end),
            samples.gpu_samples.last().map(|sample| sample.end),
            samples.gc_samples.last().map(|sample| sample.end),
        ]
        .into_iter()
        .flatten()
        .max_by(|a, b| a.total_cmp(b))
    }

    pub(super) fn stop_profiler_query_for_build(&mut self, build_id: QueryId) {
        if let Some(query_id) = self.data.live_profiler_query_by_build.remove(&build_id) {
            self.data.profiler_query_build_by_query.remove(&query_id);
            let _ = self.send_studio(UIToStudio::CancelQuery { query_id });
        }
    }

    pub(super) fn profiler_target_for_mount(&self, mount: &str) -> Option<(QueryId, String)> {
        if let Some(build_id) = self.data.active_log_build_by_mount.get(mount).copied() {
            let title = self
                .data
                .build_package
                .get(&build_id)
                .cloned()
                .unwrap_or_else(|| format!("build {}", build_id.0));
            return Some((build_id, title));
        }
        let build_id = self
            .data
            .log_tab_state
            .values()
            .find(|state| state.mount == mount)
            .map(|state| state.build_id)
            .or_else(|| {
                self.data
                    .run_tab_state
                    .values()
                    .find(|state| state.mount == mount)
                    .map(|state| state.build_id)
            })?;
        let title = self
            .data
            .build_package
            .get(&build_id)
            .cloned()
            .unwrap_or_else(|| format!("build {}", build_id.0));
        Some((build_id, title))
    }

    pub(super) fn open_profiler_for_mount(&mut self, cx: &mut Cx, mount: &str) {
        let Some((build_id, title)) = self.profiler_target_for_mount(mount) else {
            self.set_status(cx, "no build selected for profiler");
            return;
        };
        let tab_title = format!("{} profile", title);
        let Some(tab_id) =
            self.ensure_profiler_tab_for_build(cx, build_id, mount, &tab_title, true)
        else {
            self.set_status(cx, "failed to create profiler tab");
            return;
        };
        let running = *self
            .data
            .profiler_running_by_build
            .entry(build_id)
            .or_insert(true);
        if running {
            self.start_profiler_query_for_build(build_id);
        } else {
            self.stop_profiler_query_for_build(build_id);
        }
        if let Some(dock) = self.mount_workspace_dock(cx, mount) {
            dock.redraw_tab(cx, tab_id);
        }
        self.set_status(cx, &format!("opened profiler for {}", title));
    }

    pub(super) fn send_terminal_input(&mut self, path: &str, data: Vec<u8>) {
        self.ensure_terminal_session_open(path);
        let _ = self.send_studio(UIToStudio::TerminalInput {
            path: path.to_string(),
            data,
        });
    }

    pub(super) fn send_terminal_resize(&mut self, path: &str, cols: u16, rows: u16) {
        self.data
            .terminal_desired_size_by_path
            .insert(path.to_string(), (cols, rows));
        self.ensure_terminal_session_open(path);
        if !self.data.terminal_open_paths.contains(path) {
            return;
        }
        let _ = self.send_studio(UIToStudio::TerminalResize {
            path: path.to_string(),
            cols,
            rows,
        });
    }

    pub(super) fn apply_cursor_jump(session: &CodeSession, line: usize, column: usize) {
        session.set_selection(
            Position {
                line_index: line.saturating_sub(1),
                byte_index: column.saturating_sub(1),
            },
            Affinity::Before,
            SelectionMode::Simple,
            NewGroup::Yes,
        );
    }

    pub(super) fn apply_pending_log_jump(&mut self, path: &str, tab_id: LiveId) {
        let Some((line, column)) = self.data.pending_log_jumps.remove(path) else {
            return;
        };
        let Some(session) = self.data.sessions.get(&tab_id) else {
            self.data
                .pending_log_jumps
                .insert(path.to_string(), (line, column));
            return;
        };
        Self::apply_cursor_jump(session, line, column);
    }

    pub(super) fn open_log_location(
        &mut self,
        cx: &mut Cx,
        path: &str,
        line: usize,
        column: usize,
    ) {
        let Some((tab_id, _already_open)) = self.ensure_editor_tab_for_path(cx, path, true) else {
            self.set_status(cx, &format!("could not open log location {}", path));
            return;
        };

        if let Some(session) = self.data.sessions.get(&tab_id) {
            Self::apply_cursor_jump(session, line, column);
            if let Some(mount) = Self::mount_from_virtual_path(path) {
                if let Some(dock) = self.mount_workspace_dock(cx, mount) {
                    dock.redraw_tab(cx, tab_id);
                }
            }
            self.set_status(cx, &format!("opened {}:{}:{}", path, line, column));
            return;
        }

        self.data
            .pending_log_jumps
            .insert(path.to_string(), (line, column));
        if !self.data.pending_open_paths.contains(path) {
            self.data.pending_open_paths.insert(path.to_string());
            let _ = self.send_studio(UIToStudio::OpenTextFile {
                path: path.to_string(),
            });
        }
        self.set_status(cx, &format!("opening {}:{}:{}", path, line, column));
    }

    pub(super) fn extract_log_location(
        &self,
        mount: &str,
        entry: &LogEntry,
    ) -> Option<UiLogLocation> {
        if let Some(file_name) = entry.file_name.as_deref() {
            let path = self.virtualize_log_path(mount, file_name)?;
            let line = entry.line.unwrap_or(1).max(1);
            let column = entry.column.unwrap_or(1).max(1);
            return Some(UiLogLocation { path, line, column });
        }

        for token in entry.message.split_whitespace() {
            if let Some((raw_path, line, column)) = parse_path_line_column_token(token) {
                if let Some(path) = self.virtualize_log_path(mount, &raw_path) {
                    return Some(UiLogLocation { path, line, column });
                }
            }
        }
        None
    }

    pub(super) fn virtualize_log_path(&self, mount: &str, raw_path: &str) -> Option<String> {
        let mut path = raw_path
            .trim()
            .trim_matches(|c| matches!(c, '"' | '\'' | '(' | ')' | ',' | ';'))
            .to_string();
        if path.is_empty() {
            return None;
        }
        if path.starts_with(mount)
            && path
                .as_bytes()
                .get(mount.len())
                .copied()
                .is_some_and(|b| b == b'/')
        {
            return Some(path);
        }

        path = path.replace('\\', "/");
        if path.starts_with('/') {
            return self.absolute_to_virtual_path(mount, Path::new(&path));
        }

        let relative = path.trim_start_matches("./");
        if relative.is_empty() || relative.starts_with("../") {
            return None;
        }
        Some(format!("{}/{}", mount, relative))
    }

    pub(super) fn absolute_to_virtual_path(&self, mount: &str, abs_path: &Path) -> Option<String> {
        if let Some(root) = self.mount_state(mount).map(|mount| &mount.root) {
            if let Ok(rel) = abs_path.strip_prefix(root) {
                return Some(format!("{}/{}", mount, path_to_virtual(rel)));
            }
        }

        for (other_mount, mount_state) in &self.data.mounts {
            if let Ok(rel) = abs_path.strip_prefix(&mount_state.root) {
                return Some(format!("{}/{}", other_mount, path_to_virtual(rel)));
            }
        }
        None
    }

    pub(super) fn close_mount_run_and_log_tabs(&mut self, cx: &mut Cx, mount: &str) {
        let run_tabs: Vec<LiveId> = self
            .data
            .run_tab_state
            .iter()
            .filter_map(|(tab_id, state)| (state.mount == mount).then_some(*tab_id))
            .collect();
        let log_tabs: Vec<LiveId> = self
            .data
            .log_tab_state
            .iter()
            .filter_map(|(tab_id, state)| (state.mount == mount).then_some(*tab_id))
            .collect();

        for tab_id in log_tabs {
            self.close_log_tab(cx, tab_id);
        }
        for tab_id in run_tabs {
            self.close_run_tab(cx, tab_id);
        }
    }

    pub(super) fn run_package(
        &mut self,
        cx: &mut Cx,
        mount: &str,
        package: &str,
        outside_studio: bool,
    ) {
        if self.data.active_mount.as_deref() != Some(mount) {
            self.select_mount(cx, mount);
        }
        if !outside_studio {
            self.close_mount_run_and_log_tabs(cx, mount);
        }
        let Some(build_id) = self.send_studio(UIToStudio::Run {
            mount: mount.to_string(),
            process: package.to_string(),
            args: Vec::new(),
            standalone: Some(outside_studio),
            env: None,
            buildbox: None,
        }) else {
            self.set_status(cx, "backend not connected");
            return;
        };
        self.data.build_to_mount.insert(build_id, mount.to_string());
        self.data
            .build_package
            .insert(build_id, package.to_string());
        self.data
            .active_log_build_by_mount
            .insert(mount.to_string(), build_id);
        if outside_studio {
            self.set_status(
                cx,
                &format!("starting {} on {} (external window)", package, mount),
            );
        } else {
            self.set_status(cx, &format!("starting {} on {}", package, mount));
        }
    }

    pub(super) fn handle_run_view_actions(&mut self, actions: &Actions) {
        for action in actions {
            let Some(widget_action) = action.as_widget_action() else {
                continue;
            };
            let Some(run_action) = widget_action.action.downcast_ref::<DesktopRunViewAction>()
            else {
                continue;
            };
            match run_action {
                DesktopRunViewAction::ForwardToApp { build_id, msg_bin } => {
                    let _ = self.send_studio(UIToStudio::ForwardToApp {
                        build_id: *build_id,
                        msg_bin: msg_bin.clone(),
                    });
                }
                DesktopRunViewAction::None => {}
            }
        }
    }

    pub(super) fn handle_profiler_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            let Some(widget_action) = action.as_widget_action() else {
                continue;
            };
            let Some(profiler_action) = widget_action
                .action
                .downcast_ref::<DesktopProfilerViewAction>()
            else {
                continue;
            };
            match profiler_action {
                DesktopProfilerViewAction::SetRunning { build_id, running } => {
                    self.data
                        .profiler_running_by_build
                        .insert(*build_id, *running);
                    if *running {
                        if let Some(last_end) = self.latest_profiler_sample_end(*build_id) {
                            self.data
                                .profiler_time_start_by_build
                                .insert(*build_id, last_end + 0.000_001);
                        }
                        self.data.profiler_samples_by_build.remove(build_id);
                        self.start_profiler_query_for_build(*build_id);
                    } else {
                        self.stop_profiler_query_for_build(*build_id);
                    }
                    if let Some(tab_id) = self.data.profiler_tab_by_build.get(build_id).copied() {
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
                DesktopProfilerViewAction::Clear { build_id } => {
                    if let Some(last_end) = self.latest_profiler_sample_end(*build_id) {
                        self.data
                            .profiler_time_start_by_build
                            .insert(*build_id, last_end + 0.000_001);
                    }
                    self.data.profiler_samples_by_build.remove(build_id);
                    if self
                        .data
                        .profiler_running_by_build
                        .get(build_id)
                        .copied()
                        .unwrap_or(true)
                    {
                        self.start_profiler_query_for_build(*build_id);
                    }
                    if let Some(tab_id) = self.data.profiler_tab_by_build.get(build_id).copied() {
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
                DesktopProfilerViewAction::None => {}
            }
        }
    }

    pub(super) fn handle_terminal_actions(&mut self, actions: &Actions) {
        for action in actions {
            let Some(widget_action) = action.as_widget_action() else {
                continue;
            };
            let Some(term_action) = widget_action
                .action
                .downcast_ref::<DesktopTerminalViewAction>()
            else {
                continue;
            };
            match term_action {
                DesktopTerminalViewAction::Input { path, data } => {
                    self.send_terminal_input(path, data.clone());
                }
                DesktopTerminalViewAction::Resize { path, cols, rows } => {
                    self.send_terminal_resize(path, *cols, *rows);
                }
                DesktopTerminalViewAction::None => {}
            }
        }
    }

    pub(super) fn close_run_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
        let Some(state) = self.data.run_tab_state.remove(&tab_id) else {
            return;
        };
        self.data.run_tab_by_build.remove(&state.build_id);
        self.data.build_to_mount.remove(&state.build_id);
        let _ = self.send_studio(UIToStudio::StopBuild {
            build_id: state.build_id,
        });
        if let Some(dock) = self.mount_workspace_dock(cx, &state.mount) {
            dock.close_tab(cx, tab_id);
        }
    }

    pub(super) fn close_log_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
        let Some(state) = self.data.log_tab_state.remove(&tab_id) else {
            return;
        };
        self.data.log_tab_by_build.remove(&state.build_id);
        if self.data.active_log_build_by_mount.get(&state.mount) == Some(&state.build_id) {
            self.data.active_log_build_by_mount.remove(&state.mount);
        }
        if let Some(dock) = self.mount_workspace_dock(cx, &state.mount) {
            dock.close_tab(cx, tab_id);
        }
    }

    pub(super) fn close_profiler_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
        let Some(state) = self.data.profiler_tab_state.remove(&tab_id) else {
            return;
        };
        self.data.profiler_tab_by_build.remove(&state.build_id);
        self.data.profiler_running_by_build.remove(&state.build_id);
        self.data
            .profiler_time_start_by_build
            .remove(&state.build_id);
        if self.data.active_log_build_by_mount.get(&state.mount) == Some(&state.build_id) {
            self.data.active_log_build_by_mount.remove(&state.mount);
        }
        self.stop_profiler_query_for_build(state.build_id);
        if let Some(dock) = self.mount_workspace_dock(cx, &state.mount) {
            dock.close_tab(cx, tab_id);
        }
    }
}
