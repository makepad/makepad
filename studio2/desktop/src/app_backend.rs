use super::*;

impl App {
    pub(super) fn apply_mount_file_tree_diff(
        &mut self,
        cx: &mut Cx,
        mount: &str,
        changes: Vec<crate::makepad_studio_backend::FileTreeChange>,
    ) {
        if changes.is_empty() {
            return;
        }
        let mut changed = false;
        let Some(tree) = self.mount_state_mut(mount).file_tree_data.as_mut() else {
            let _ = self.send_studio(UIToStudio::LoadFileTree {
                mount: mount.to_string(),
            });
            return;
        };

        for change in changes {
            match change {
                crate::makepad_studio_backend::FileTreeChange::Added {
                    path,
                    node_type,
                    git_status,
                } => {
                    let name = path.rsplit('/').next().unwrap_or("").to_string();
                    if let Some(node) = tree.nodes.iter_mut().find(|node| node.path == path) {
                        node.node_type = node_type;
                        node.git_status = git_status;
                        if !name.is_empty() {
                            node.name = name;
                        }
                        changed = true;
                    } else if !name.is_empty() {
                        tree.nodes.push(crate::makepad_studio_backend::FileNode {
                            path,
                            name,
                            node_type,
                            git_status,
                        });
                        changed = true;
                    }
                }
                crate::makepad_studio_backend::FileTreeChange::Removed { path } => {
                    let prefix = format!("{}/", path);
                    let before = tree.nodes.len();
                    tree.nodes
                        .retain(|node| node.path != path && !node.path.starts_with(&prefix));
                    if tree.nodes.len() != before {
                        changed = true;
                    }
                }
                crate::makepad_studio_backend::FileTreeChange::Modified { path, git_status } => {
                    if let Some(node) = tree.nodes.iter_mut().find(|node| node.path == path) {
                        node.git_status = git_status;
                        changed = true;
                    }
                }
            }
        }

        if !changed {
            return;
        }
        self.ensure_mount_terminal_file(cx, mount);
        if self.data.active_mount.as_deref() == Some(mount) {
            self.refresh_active_mount_tree(cx);
            self.refresh_active_mount_log_panels(cx);
        }
    }

    pub(super) fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);
        crate::makepad_code_editor::script_mod(vm);
        crate::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }

    pub(super) fn start_backend(&mut self, cx: &mut Cx) {
        let current_path = match env::current_dir().and_then(|p| p.canonicalize()) {
            Ok(path) => path,
            Err(err) => {
                self.set_status(cx, &format!("failed to resolve current dir: {}", err));
                return;
            }
        };

        let mut mounts = Vec::new();
        if let Ok(spec) = env::var("STUDIO2_MOUNTS") {
            for token in spec.split(';').map(str::trim).filter(|t| !t.is_empty()) {
                let Some((name, path_str)) = token.split_once('=') else {
                    continue;
                };
                let name = name.trim();
                let path_str = path_str.trim();
                if name.is_empty() || path_str.is_empty() {
                    continue;
                }
                if let Ok(path) = std::path::PathBuf::from(path_str).canonicalize() {
                    mounts.push(MountConfig {
                        name: name.to_string(),
                        path,
                    });
                }
            }
        }
        if mounts.is_empty() {
            mounts.push(MountConfig {
                name: "makepad".to_string(),
                path: current_path,
            });
        }

        let config = BackendConfig {
            mounts: mounts.clone(),
            enable_in_process_gateway: true,
            ..Default::default()
        };

        match StudioBackend::start_in_process(config) {
            Ok(studio) => {
                self.data.studio = Some(studio);
                for mount in &mounts {
                    self.data.mounts.entry(mount.name.clone()).or_default().root = mount.path.clone();
                    let _ = self.ensure_mount_tab(cx, &mount.name);
                    let _ = self.send_studio(UIToStudio::LoadFileTree {
                        mount: mount.name.clone(),
                    });
                    let _ = self.send_studio(UIToStudio::LoadRunnableBuilds {
                        mount: mount.name.clone(),
                    });
                }
                if let Some(first_mount) = mounts.first() {
                    self.select_mount(cx, &first_mount.name);
                }
                self.set_status(cx, "connected to backend");
            }
            Err(err) => {
                self.set_status(cx, &format!("backend startup failed: {}", err));
            }
        }
    }

    pub(super) fn set_status(&self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(status_label)).set_text(cx, text);
    }

    pub(super) fn set_current_file_label(&self, cx: &mut Cx, path: Option<&str>) {
        let label = path.unwrap_or("No file");
        self.ui
            .label(cx, ids!(current_file_label))
            .set_text(cx, label);
    }

    pub(super) fn send_studio(&mut self, msg: UIToStudio) -> Option<QueryId> {
        self.data.studio.as_mut().map(|studio| studio.send(msg))
    }

    pub(super) fn mount_state(&self, mount: &str) -> Option<&MountState> {
        self.data.mounts.get(mount)
    }

    pub(super) fn mount_state_mut(&mut self, mount: &str) -> &mut MountState {
        self.data.mounts.entry(mount.to_string()).or_default()
    }

    pub(super) fn ensure_mount_tab(&mut self, cx: &mut Cx, mount: &str) -> Option<LiveId> {
        let dock = self.ui.dock(cx, ids!(mount_dock));
        if let Some(tab_id) = self.mount_state(mount).and_then(|state| state.tab_id) {
            if dock.find_tab_bar_of_tab(tab_id).is_some() {
                return Some(tab_id);
            }
            self.data.tab_to_mount.remove(&tab_id);
            self.mount_state_mut(mount).tab_id = None;
        }

        let has_any_mount_tab = self
            .data
            .mounts
            .values()
            .any(|state| state.tab_id.is_some());
        let tab_id = if !has_any_mount_tab {
            id!(mount_first)
        } else {
            let anchor = self
                .data
                .mounts
                .values()
                .filter_map(|state| state.tab_id)
                .next()
                .unwrap_or(id!(mount_first));
            let (tab_bar, pos) = dock.find_tab_bar_of_tab(anchor)?;
            let tab_id = dock.unique_id(LiveId::from_str(&format!("mount/{}", mount)).0);
            if dock
                .create_tab(
                    cx,
                    tab_bar,
                    tab_id,
                    id!(MountWorkspace),
                    mount.to_string(),
                    id!(MountTab),
                    Some(pos),
                )
                .is_none()
            {
                return None;
            }
            tab_id
        };

        dock.set_tab_title(cx, tab_id, mount.to_string());
        self.mount_state_mut(mount).tab_id = Some(tab_id);
        self.data.tab_to_mount.insert(tab_id, mount.to_string());
        Some(tab_id)
    }

    pub(super) fn mount_from_virtual_path(path: &str) -> Option<&str> {
        path.split('/').next().filter(|part| !part.is_empty())
    }

    pub(super) fn terminal_virtual_path(mount: &str) -> String {
        format!("{}/.makepad/a.term", mount)
    }

    pub(super) fn is_terminal_virtual_path(path: &str) -> bool {
        path.contains("/.makepad/") && path.ends_with(".term")
    }

    pub(super) fn mount_workspace_widget(&mut self, cx: &mut Cx, mount: &str) -> Option<WidgetRef> {
        let tab_id = self.ensure_mount_tab(cx, mount)?;
        let mount_dock = self.ui.dock(cx, ids!(mount_dock));
        if mount_dock.find_tab_bar_of_tab(tab_id).is_none() {
            return None;
        }
        Some(mount_dock.item(tab_id))
    }

    pub(super) fn mount_workspace_dock(&mut self, cx: &mut Cx, mount: &str) -> Option<DockRef> {
        let workspace = self.mount_workspace_widget(cx, mount)?;
        Some(workspace.dock(cx, ids!(dock)))
    }

    pub(super) fn refresh_active_mount_tree(&mut self, cx: &mut Cx) {
        let Some(active_mount) = self.data.active_mount.clone() else {
            self.data.file_tree = FlatFileTree::default();
            return;
        };
        let Some(workspace) = self.mount_workspace_widget(cx, &active_mount) else {
            return;
        };
        let Some(tree_data) = self
            .mount_state(&active_mount)
            .and_then(|mount| mount.file_tree_data.clone())
        else {
            self.data.file_tree = FlatFileTree::default();
            workspace.widget(cx, ids!(file_tree)).redraw(cx);
            return;
        };
        self.data.file_tree.rebuild(tree_data);
        workspace.widget(cx, ids!(file_tree)).redraw(cx);
        workspace
            .desktop_file_tree(cx, ids!(file_tree))
            .set_folder_is_open(cx, LiveId::from_str(&active_mount), true, Animate::No);
    }

    pub(super) fn refresh_active_mount_run_list(&mut self, cx: &mut Cx) {
        let Some(active_mount) = self.data.active_mount.clone() else {
            return;
        };
        if let Some(workspace) = self.mount_workspace_widget(cx, &active_mount) {
            workspace.widget(cx, ids!(run_list)).redraw(cx);
        }
    }

    pub(super) fn refresh_active_mount_log_panels(&mut self, cx: &mut Cx) {
        let Some(active_mount) = self.data.active_mount.clone() else {
            return;
        };
        if let Some(workspace) = self.mount_workspace_widget(cx, &active_mount) {
            workspace.widget(cx, ids!(log_view)).redraw(cx);
            workspace.widget(cx, ids!(terminal_view)).redraw(cx);
        }
    }

    pub(super) fn terminal_tab_title(path: &str) -> String {
        path.rsplit('/').next().unwrap_or("terminal").to_string()
    }

    pub(super) fn terminal_tab_mount_path(&self, tab_id: LiveId) -> Option<(String, String)> {
        for (mount, state) in &self.data.mounts {
            if let Some(path) = state.terminal_tab_to_path.get(&tab_id) {
                return Some((mount.clone(), path.clone()));
            }
        }
        None
    }

    pub(super) fn set_mount_file_filter(&mut self, cx: &mut Cx, mount: &str, filter: String) {
        let filter = filter.trim().to_string();
        let old_query = {
            let mount_state = self.mount_state_mut(mount);
            mount_state.file_filter = filter.clone();
            mount_state.file_filter_results.clear();
            mount_state.file_filter_query.take()
        };
        if let Some(query_id) = old_query {
            self.data.file_filter_mount_by_query.remove(&query_id);
            let _ = self.send_studio(UIToStudio::CancelQuery { query_id });
        }

        if !filter.is_empty() {
            if let Some(query_id) = self.send_studio(UIToStudio::FindFiles {
                mount: Some(mount.to_string()),
                pattern: filter,
                is_regex: Some(false),
                max_results: Some(2000),
            }) {
                self.mount_state_mut(mount).file_filter_query = Some(query_id);
                self.data
                    .file_filter_mount_by_query
                    .insert(query_id, mount.to_string());
            }
        }
        if self.data.active_mount.as_deref() == Some(mount) {
            if let Some(workspace) = self.mount_workspace_widget(cx, mount) {
                workspace.widget(cx, ids!(file_tree)).redraw(cx);
            }
        }
    }

    pub(super) fn set_mount_log_tail(&mut self, cx: &mut Cx, mount: &str, tail: bool) {
        self.mount_state_mut(mount).log_tail = tail;
        if self.data.active_mount.as_deref() == Some(mount) {
            if let Some(workspace) = self.mount_workspace_widget(cx, mount) {
                workspace
                    .desktop_log_view(cx, ids!(log_view))
                    .set_tail(cx, tail);
            }
        }
    }

    pub(super) fn set_mount_log_filter(&mut self, mount: &str, filter: String) {
        self.mount_state_mut(mount).log_filter = filter.trim().to_string();
    }

    pub(super) fn restart_log_query_for_mount(&mut self, cx: &mut Cx, mount: &str) {
        let (pattern, live) = self
            .mount_state(mount)
            .map(|mount_state| {
                (
                    mount_state.log_filter.trim().to_string(),
                    mount_state.log_tail,
                )
            })
            .unwrap_or_else(|| (String::new(), true));
        if let Some(query_id) = self.data.live_log_query.take() {
            let _ = self.send_studio(UIToStudio::CancelQuery { query_id });
        }
        self.data.build_log_entries.clear();
        for mount_state in self.data.mounts.values_mut() {
            mount_state.log_entries.clear();
        }
        self.data.live_log_query = self.send_studio(UIToStudio::QueryLogs {
            build_id: None,
            level: None,
            source: None,
            file: None,
            pattern: if pattern.is_empty() {
                None
            } else {
                Some(pattern)
            },
            is_regex: Some(false),
            since_index: None,
            live: Some(live),
        });
        self.refresh_active_mount_log_panels(cx);
    }

    pub(super) fn apply_mount_toolbar_state(&mut self, cx: &mut Cx, mount: &str) {
        let (file_filter, log_filter, log_tail) = self
            .mount_state(mount)
            .map(|state| {
                (
                    state.file_filter.clone(),
                    state.log_filter.clone(),
                    state.log_tail,
                )
            })
            .unwrap_or_else(|| (String::new(), String::new(), true));
        if let Some(workspace) = self.mount_workspace_widget(cx, mount) {
            workspace
                .text_input(cx, ids!(file_tree_filter))
                .set_text(cx, &file_filter);
            workspace
                .text_input(cx, ids!(log_filter))
                .set_text(cx, &log_filter);
            workspace
                .check_box(cx, ids!(log_tail_toggle))
                .set_active(cx, log_tail);
            workspace
                .desktop_log_view(cx, ids!(log_view))
                .set_tail(cx, log_tail);
        }
    }

    pub(super) fn request_stop_all_builds_for_mount(&mut self, cx: &mut Cx, mount: &str) {
        self.data.pending_stop_all_mount = Some(mount.to_string());
        let _ = self.send_studio(UIToStudio::ListBuilds);
        self.set_status(cx, &format!("requesting stop-all for {}", mount));
    }

    pub(super) fn collect_mount_terminal_files(&self, mount: &str) -> Vec<String> {
        let Some(tree) = self
            .mount_state(mount)
            .and_then(|mount| mount.file_tree_data.as_ref())
        else {
            return Vec::new();
        };
        let prefix = format!("{}/.makepad/", mount);
        let mut files: Vec<String> = tree
            .nodes
            .iter()
            .filter_map(|node| {
                if !matches!(node.node_type, FileNodeType::File) {
                    return None;
                }
                if !node.path.starts_with(&prefix) || !node.path.ends_with(".term") {
                    return None;
                }
                let tail = &node.path[prefix.len()..];
                if tail.contains('/') {
                    return None;
                }
                Some(node.path.clone())
            })
            .collect();
        files.sort();
        files
    }

    pub(super) fn sync_mount_terminal_tabs(&mut self, cx: &mut Cx, mount: &str, select_last: bool) {
        let files = self
            .mount_state(mount)
            .map(|mount| mount.terminal_files.clone())
            .unwrap_or_default();

        let Some(dock) = self.mount_workspace_dock(cx, mount) else {
            return;
        };

        let mount_state = self.mount_state_mut(mount);
        let path_to_tab = &mut mount_state.terminal_path_to_tab;
        let tab_to_path = &mut mount_state.terminal_tab_to_path;

        // Keep terminal_first as a persistent icon-only anchor tab.
        if let Some(old_path) = tab_to_path.remove(&id!(terminal_first)) {
            path_to_tab.remove(&old_path);
        }
        path_to_tab.retain(|_, tab_id| *tab_id != id!(terminal_first));
        dock.set_tab_title(cx, id!(terminal_first), String::new());

        for path in files.iter() {
            let tab_id = if let Some(existing_tab_id) = path_to_tab.get(path).copied() {
                if dock.find_tab_bar_of_tab(existing_tab_id).is_some() {
                    existing_tab_id
                } else {
                    path_to_tab.remove(path);
                    tab_to_path.remove(&existing_tab_id);
                    let Some((tab_bar, pos)) = dock.find_tab_bar_of_tab(id!(terminal_add)) else {
                        continue;
                    };
                    let tab_id = dock.unique_id(LiveId::from_str(path).0);
                    let insert_after = Some(pos.saturating_sub(1));
                    if dock
                        .create_tab(
                            cx,
                            tab_bar,
                            tab_id,
                            id!(TerminalPane),
                            Self::terminal_tab_title(path),
                            id!(CloseableTab),
                            insert_after,
                        )
                        .is_none()
                    {
                        continue;
                    }
                    path_to_tab.insert(path.clone(), tab_id);
                    tab_to_path.insert(tab_id, path.clone());
                    tab_id
                }
            } else {
                let Some((tab_bar, pos)) = dock.find_tab_bar_of_tab(id!(terminal_add)) else {
                    continue;
                };
                let tab_id = dock.unique_id(LiveId::from_str(path).0);
                let insert_after = Some(pos.saturating_sub(1));
                if dock
                    .create_tab(
                        cx,
                        tab_bar,
                        tab_id,
                        id!(TerminalPane),
                        Self::terminal_tab_title(path),
                        id!(CloseableTab),
                        insert_after,
                    )
                    .is_none()
                {
                    continue;
                }
                path_to_tab.insert(path.clone(), tab_id);
                tab_to_path.insert(tab_id, path.clone());
                tab_id
            };
            dock.set_tab_title(cx, tab_id, Self::terminal_tab_title(path));
        }

        let keep_paths: HashSet<String> = files.iter().cloned().collect();
        let stale: Vec<(String, LiveId)> = path_to_tab
            .iter()
            .filter_map(|(path, tab_id)| {
                if keep_paths.contains(path) {
                    None
                } else {
                    Some((path.clone(), *tab_id))
                }
            })
            .collect();
        for (path, tab_id) in stale {
            path_to_tab.remove(&path);
            tab_to_path.remove(&tab_id);
            if tab_id != id!(terminal_first) {
                dock.close_tab(cx, tab_id);
            }
        }

        if select_last {
            if let Some(last_path) = files.last() {
                if let Some(last_tab_id) = path_to_tab.get(last_path).copied() {
                    dock.select_tab(cx, last_tab_id);
                } else {
                    dock.select_tab(cx, id!(terminal_first));
                }
            } else {
                dock.select_tab(cx, id!(terminal_first));
            }
        }
    }

    pub(super) fn ensure_terminal_session_open(&mut self, path: &str) {
        if self.data.terminal_open_paths.contains(path) {
            return;
        }
        let (cols, rows) = self
            .data
            .terminal_desired_size_by_path
            .get(path)
            .copied()
            .unwrap_or((120, 40));
        let _ = self.send_studio(UIToStudio::TerminalOpen {
            path: path.to_string(),
            cols,
            rows,
            env: HashMap::new(),
        });
    }

    pub(super) fn ensure_mount_terminal_file(&mut self, cx: &mut Cx, mount: &str) {
        let known_before = self
            .mount_state(mount)
            .map(|mount| mount.terminals_initialized)
            .unwrap_or(false);
        let files = self.collect_mount_terminal_files(mount);
        let keep_paths: HashSet<String> = files.iter().cloned().collect();
        let stale_paths: Vec<String> = self
            .data
            .terminal_stream_by_path
            .keys()
            .filter(|path| {
                Self::mount_from_virtual_path(path.as_str()) == Some(mount)
                    && !keep_paths.contains(path.as_str())
            })
            .cloned()
            .collect();
        for stale in stale_paths {
            self.data.terminal_stream_by_path.remove(&stale);
            self.data.terminal_history_len_by_path.remove(&stale);
            self.data.terminal_desired_size_by_path.remove(&stale);
            if self.data.terminal_open_paths.remove(&stale) {
                let _ = self.send_studio(UIToStudio::TerminalClose { path: stale });
            }
        }
        let select_last = {
            let mount_state = self.mount_state_mut(mount);
            let select_last = !known_before || mount_state.select_last_terminal_once;
            mount_state.select_last_terminal_once = false;
            mount_state.terminals_initialized = true;
            mount_state.terminal_files = files.clone();
            select_last
        };
        self.sync_mount_terminal_tabs(cx, mount, select_last);

        for path in &files {
            self.data
                .terminal_stream_by_path
                .entry(path.clone())
                .or_default();
            self.ensure_terminal_session_open(path);
        }

        if !known_before && files.is_empty() {
            let path = Self::terminal_virtual_path(mount);
            self.mount_state_mut(mount).select_last_terminal_once = true;
            self.data
                .terminal_stream_by_path
                .entry(path.clone())
                .or_default();
            let _ = self.send_studio(UIToStudio::SaveTextFile {
                path,
                content: String::new(),
            });
            let _ = self.send_studio(UIToStudio::LoadFileTree {
                mount: mount.to_string(),
            });
            return;
        }

        if known_before {
            return;
        }
    }

    pub(super) fn next_terminal_path(&mut self, mount: &str) -> String {
        let files = self
            .mount_state(mount)
            .map(|mount| mount.terminal_files.clone())
            .unwrap_or_default();
        let mut index = 0usize;
        loop {
            let name = if index < 26 {
                let ch = (b'a' + index as u8) as char;
                format!("{}.term", ch)
            } else {
                format!("t{}.term", index + 1)
            };
            let path = format!("{}/.makepad/{}", mount, name);
            if !files.iter().any(|existing| existing == &path) {
                return path;
            }
            index += 1;
        }
    }

    pub(super) fn create_new_terminal_tab(&mut self, _cx: &mut Cx, mount: &str) {
        let path = self.next_terminal_path(mount);
        let name = path.rsplit('/').next().unwrap_or("terminal").to_string();
        self.mount_state_mut(mount).select_last_terminal_once = true;

        let _ = self.send_studio(UIToStudio::SaveTextFile {
            path: path.clone(),
            content: String::new(),
        });
        let _ = self.send_studio(UIToStudio::LoadFileTree {
            mount: mount.to_string(),
        });
        self.set_status(_cx, &format!("created terminal {}", name));
    }

    pub(super) fn delete_terminal_tab_file(&mut self, cx: &mut Cx, mount: &str, tab_id: LiveId) {
        if tab_id == id!(terminal_add) {
            return;
        }
        let Some(path) = self
            .mount_state(mount)
            .and_then(|mount| mount.terminal_tab_to_path.get(&tab_id))
            .cloned()
        else {
            return;
        };

        let mount_state = self.mount_state_mut(mount);
        mount_state.terminal_tab_to_path.remove(&tab_id);
        mount_state.terminal_path_to_tab.remove(&path);
        mount_state.terminal_files.retain(|file| file != &path);
        if let Some(dock) = self.mount_workspace_dock(cx, mount) {
            if tab_id != id!(terminal_first) {
                dock.close_tab(cx, tab_id);
            } else {
                dock.set_tab_title(cx, id!(terminal_first), String::new());
            }
        }

        self.data.terminal_open_paths.remove(&path);
        self.data.terminal_stream_by_path.remove(&path);
        self.data.terminal_history_len_by_path.remove(&path);
        self.data.terminal_desired_size_by_path.remove(&path);
        let _ = self.send_studio(UIToStudio::TerminalClose { path: path.clone() });
        let _ = self.send_studio(UIToStudio::DeleteFile { path });
        let _ = self.send_studio(UIToStudio::LoadFileTree {
            mount: mount.to_string(),
        });
    }

    pub(super) fn select_mount(&mut self, cx: &mut Cx, mount: &str) {
        self.data.active_mount = Some(mount.to_string());
        if let Some(tab_id) = self.ensure_mount_tab(cx, mount) {
            self.ui.dock(cx, ids!(mount_dock)).select_tab(cx, tab_id);
        }
        if self
            .mount_state(mount)
            .and_then(|mount| mount.file_tree_data.as_ref())
            .is_some()
        {
            self.refresh_active_mount_tree(cx);
            self.set_status(cx, &format!("mount ready: {}", mount));
        } else {
            let _ = self.send_studio(UIToStudio::LoadFileTree {
                mount: mount.to_string(),
            });
            self.set_status(cx, &format!("loading mount: {}", mount));
        }
        self.ensure_mount_terminal_file(cx, mount);
        if self
            .mount_state(mount)
            .map(|mount| mount.runnable_builds.is_empty())
            .unwrap_or(true)
        {
            let _ = self.send_studio(UIToStudio::LoadRunnableBuilds {
                mount: mount.to_string(),
            });
        }
        self.apply_mount_toolbar_state(cx, mount);
        self.restart_log_query_for_mount(cx, mount);
        self.refresh_active_mount_run_list(cx);
        self.refresh_active_mount_log_panels(cx);
    }
}
