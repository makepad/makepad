use crate::{
    desktop_file_tree::*,
    desktop_log_view::*,
    desktop_run_list::*,
    desktop_terminal_view::*,
    makepad_code_editor::{
        code_editor::CodeEditorAction, decoration::DecorationSet, history::NewGroup,
        selection::Affinity, session::SelectionMode, text::Position, CodeDocument, CodeSession,
    },
    makepad_studio_backend::{
        BackendConfig, FileNodeType, FileTreeData, GitStatus, LogEntry, LogLevel, LogSource,
        MountConfig, QueryId, RunnableBuild, StudioBackend, StudioConnection, StudioToUI,
        UIToStudio,
    },
    makepad_widgets::{file_tree::GitStatusDotKind, *},
};
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Component, Path, PathBuf};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    load_all_resources() do #(App::script_component(vm)) {
        ui: Root {
            AppUI {}
        }
    }
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);
        crate::makepad_code_editor::script_mod(vm);
        crate::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }

    fn start_backend(&mut self, cx: &mut Cx) {
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
                    self.data
                        .mount_roots
                        .insert(mount.name.clone(), mount.path.clone());
                    let _ = self.ensure_mount_tab(cx, &mount.name);
                    let _ = self.send_studio(UIToStudio::LoadFileTree {
                        mount: mount.name.clone(),
                    });
                    let _ = self.send_studio(UIToStudio::LoadRunnableBuilds {
                        mount: mount.name.clone(),
                    });
                }
                self.data.live_log_query = self.send_studio(UIToStudio::QueryLogs {
                    build_id: None,
                    level: None,
                    source: None,
                    file: None,
                    pattern: None,
                    is_regex: None,
                    since_index: None,
                    live: Some(true),
                });
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

    fn set_status(&self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(status_label)).set_text(cx, text);
    }

    fn set_current_file_label(&self, cx: &mut Cx, path: Option<&str>) {
        let label = path.unwrap_or("No file");
        self.ui
            .label(cx, ids!(current_file_label))
            .set_text(cx, label);
    }

    fn send_studio(&mut self, msg: UIToStudio) -> Option<QueryId> {
        self.data.studio.as_mut().map(|studio| studio.send(msg))
    }

    fn ensure_mount_tab(&mut self, cx: &mut Cx, mount: &str) -> Option<LiveId> {
        let dock = self.ui.dock(cx, ids!(mount_dock));
        if let Some(tab_id) = self.data.mount_to_tab.get(mount).copied() {
            if dock.find_tab_bar_of_tab(tab_id).is_some() {
                return Some(tab_id);
            }
            self.data.mount_to_tab.remove(mount);
            self.data.tab_to_mount.remove(&tab_id);
        }

        let tab_id = if self.data.mount_to_tab.is_empty() {
            id!(mount_first)
        } else {
            let anchor = self
                .data
                .mount_to_tab
                .values()
                .copied()
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
        self.data.mount_to_tab.insert(mount.to_string(), tab_id);
        self.data.tab_to_mount.insert(tab_id, mount.to_string());
        Some(tab_id)
    }

    fn mount_from_virtual_path(path: &str) -> Option<&str> {
        path.split('/').next().filter(|part| !part.is_empty())
    }

    fn terminal_virtual_path(mount: &str) -> String {
        format!("{}/.makepad/a.term", mount)
    }

    fn is_terminal_virtual_path(path: &str) -> bool {
        path.contains("/.makepad/") && path.ends_with(".term")
    }

    fn mount_workspace_widget(&mut self, cx: &mut Cx, mount: &str) -> Option<WidgetRef> {
        let tab_id = self.ensure_mount_tab(cx, mount)?;
        let mount_dock = self.ui.dock(cx, ids!(mount_dock));
        if mount_dock.find_tab_bar_of_tab(tab_id).is_none() {
            return None;
        }
        Some(mount_dock.item(tab_id))
    }

    fn mount_workspace_dock(&mut self, cx: &mut Cx, mount: &str) -> Option<DockRef> {
        let workspace = self.mount_workspace_widget(cx, mount)?;
        Some(workspace.dock(cx, ids!(dock)))
    }

    fn refresh_active_mount_tree(&mut self, cx: &mut Cx) {
        let Some(active_mount) = self.data.active_mount.clone() else {
            self.data.file_tree = FlatFileTree::default();
            return;
        };
        let Some(workspace) = self.mount_workspace_widget(cx, &active_mount) else {
            return;
        };
        let Some(tree_data) = self.data.mount_file_trees.get(&active_mount).cloned() else {
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

    fn refresh_active_mount_run_list(&mut self, cx: &mut Cx) {
        let Some(active_mount) = self.data.active_mount.clone() else {
            return;
        };
        if let Some(workspace) = self.mount_workspace_widget(cx, &active_mount) {
            workspace.widget(cx, ids!(run_list)).redraw(cx);
        }
    }

    fn refresh_active_mount_log_panels(&mut self, cx: &mut Cx) {
        let Some(active_mount) = self.data.active_mount.clone() else {
            return;
        };
        if let Some(workspace) = self.mount_workspace_widget(cx, &active_mount) {
            workspace.widget(cx, ids!(log_view)).redraw(cx);
            workspace.widget(cx, ids!(terminal_view)).redraw(cx);
        }
    }

    fn terminal_tab_title(path: &str) -> String {
        path.rsplit('/').next().unwrap_or("terminal").to_string()
    }

    fn collect_mount_terminal_files(&self, mount: &str) -> Vec<String> {
        let Some(tree) = self.data.mount_file_trees.get(mount) else {
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

    fn sync_mount_terminal_tabs(&mut self, cx: &mut Cx, mount: &str) {
        let files = self
            .data
            .mount_terminal_files
            .get(mount)
            .cloned()
            .unwrap_or_default();

        let Some(dock) = self.mount_workspace_dock(cx, mount) else {
            return;
        };

        let path_to_tab = self
            .data
            .mount_terminal_path_to_tab
            .entry(mount.to_string())
            .or_default();
        let tab_to_path = self
            .data
            .mount_terminal_tab_to_path
            .entry(mount.to_string())
            .or_default();

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
    }

    fn ensure_terminal_session_open(&mut self, path: &str) {
        if self.data.terminal_open_paths.contains(path) {
            return;
        }
        let _ = self.send_studio(UIToStudio::TerminalOpen {
            path: path.to_string(),
            cols: 120,
            rows: 40,
            env: HashMap::new(),
        });
    }

    fn ensure_mount_terminal_file(&mut self, cx: &mut Cx, mount: &str) {
        let known_before = self.data.mount_terminal_files.contains_key(mount);
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
            if self.data.terminal_open_paths.remove(&stale) {
                let _ = self.send_studio(UIToStudio::TerminalClose { path: stale });
            }
        }
        self.data
            .mount_terminal_files
            .insert(mount.to_string(), files.clone());
        self.sync_mount_terminal_tabs(cx, mount);

        for path in &files {
            self.data
                .terminal_stream_by_path
                .entry(path.clone())
                .or_default();
            self.ensure_terminal_session_open(path);
        }

        if !known_before && files.is_empty() {
            let path = Self::terminal_virtual_path(mount);
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

    fn next_terminal_path(&mut self, mount: &str) -> String {
        let files = self
            .data
            .mount_terminal_files
            .entry(mount.to_string())
            .or_default()
            .clone();
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

    fn create_new_terminal_tab(&mut self, _cx: &mut Cx, mount: &str) {
        let path = self.next_terminal_path(mount);
        let name = path.rsplit('/').next().unwrap_or("terminal").to_string();

        let _ = self.send_studio(UIToStudio::SaveTextFile {
            path: path.clone(),
            content: String::new(),
        });
        let _ = self.send_studio(UIToStudio::LoadFileTree {
            mount: mount.to_string(),
        });
        self.set_status(_cx, &format!("created terminal {}", name));
    }

    fn delete_terminal_tab_file(&mut self, cx: &mut Cx, mount: &str, tab_id: LiveId) {
        if tab_id == id!(terminal_add) {
            return;
        }
        let Some(path) = self
            .data
            .mount_terminal_tab_to_path
            .get(mount)
            .and_then(|tabs| tabs.get(&tab_id))
            .cloned()
        else {
            return;
        };

        if let Some(tab_to_path) = self.data.mount_terminal_tab_to_path.get_mut(mount) {
            tab_to_path.remove(&tab_id);
        }
        if let Some(path_to_tab) = self.data.mount_terminal_path_to_tab.get_mut(mount) {
            path_to_tab.remove(&path);
        }
        if let Some(files) = self.data.mount_terminal_files.get_mut(mount) {
            files.retain(|file| file != &path);
        }
        if let Some(dock) = self.mount_workspace_dock(cx, mount) {
            if tab_id != id!(terminal_first) {
                dock.close_tab(cx, tab_id);
            } else {
                dock.set_tab_title(cx, id!(terminal_first), String::new());
            }
        }

        self.data.terminal_open_paths.remove(&path);
        self.data.terminal_stream_by_path.remove(&path);
        let _ = self.send_studio(UIToStudio::TerminalClose { path: path.clone() });
        let _ = self.send_studio(UIToStudio::DeleteFile { path });
        let _ = self.send_studio(UIToStudio::LoadFileTree {
            mount: mount.to_string(),
        });
    }

    fn select_mount(&mut self, cx: &mut Cx, mount: &str) {
        self.data.active_mount = Some(mount.to_string());
        if let Some(tab_id) = self.ensure_mount_tab(cx, mount) {
            self.ui.dock(cx, ids!(mount_dock)).select_tab(cx, tab_id);
        }
        if self.data.mount_file_trees.contains_key(mount) {
            self.refresh_active_mount_tree(cx);
            self.set_status(cx, &format!("mount ready: {}", mount));
        } else {
            let _ = self.send_studio(UIToStudio::LoadFileTree {
                mount: mount.to_string(),
            });
            self.set_status(cx, &format!("loading mount: {}", mount));
        }
        self.ensure_mount_terminal_file(cx, mount);
        if !self.data.mount_runnable_builds.contains_key(mount) {
            let _ = self.send_studio(UIToStudio::LoadRunnableBuilds {
                mount: mount.to_string(),
            });
        }
        self.refresh_active_mount_run_list(cx);
        self.refresh_active_mount_log_panels(cx);
    }

    fn drain_studio_messages(&mut self, cx: &mut Cx) {
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

    fn handle_studio_message(&mut self, cx: &mut Cx, msg: StudioToUI) {
        match msg {
            StudioToUI::FileTree { mount, data } => {
                let _ = self.ensure_mount_tab(cx, &mount);
                self.data.mount_file_trees.insert(mount.clone(), data);
                self.ensure_mount_terminal_file(cx, &mount);
                if self.data.active_mount.is_none() {
                    self.select_mount(cx, &mount);
                } else if self.data.active_mount.as_deref() == Some(mount.as_str()) {
                    self.refresh_active_mount_tree(cx);
                    self.set_status(cx, &format!("file tree loaded: {}", mount));
                }
            }
            StudioToUI::TextFileOpened { path, content, .. } => {
                if Self::is_terminal_virtual_path(&path) {
                    self.data
                        .terminal_stream_by_path
                        .insert(path, content.into_bytes());
                    self.refresh_active_mount_log_panels(cx);
                    return;
                }
                self.data.pending_open_paths.remove(&path);
                if let Some((tab_id, _)) = self.ensure_editor_tab_for_path(cx, &path, false) {
                    self.data.sessions.insert(
                        tab_id,
                        CodeSession::new(CodeDocument::new(content.into(), DecorationSet::new())),
                    );
                    self.apply_pending_log_jump(&path, tab_id);
                    if let Some(mount) = Self::mount_from_virtual_path(&path) {
                        if let Some(dock) = self.mount_workspace_dock(cx, mount) {
                            dock.redraw_tab(cx, tab_id);
                        }
                    }
                }
                self.set_status(cx, "opened file");
            }
            StudioToUI::FileTreeDiff { mount, .. } => {
                if self.data.active_mount.as_deref() == Some(mount.as_str()) {
                    let _ = self.send_studio(UIToStudio::LoadFileTree { mount });
                }
            }
            StudioToUI::TextFileRead { path, content } => {
                if Self::is_terminal_virtual_path(&path) {
                    self.data
                        .terminal_stream_by_path
                        .insert(path, content.into_bytes());
                    self.refresh_active_mount_log_panels(cx);
                    return;
                }
                self.data.pending_open_paths.remove(&path);
                if let Some((tab_id, _)) = self.ensure_editor_tab_for_path(cx, &path, false) {
                    self.data.sessions.insert(
                        tab_id,
                        CodeSession::new(CodeDocument::new(content.into(), DecorationSet::new())),
                    );
                    self.apply_pending_log_jump(&path, tab_id);
                    if let Some(mount) = Self::mount_from_virtual_path(&path) {
                        if let Some(dock) = self.mount_workspace_dock(cx, mount) {
                            dock.redraw_tab(cx, tab_id);
                        }
                    }
                }
            }
            StudioToUI::TextFileSaved { path, result } => {
                if Self::is_terminal_virtual_path(&path) {
                    return;
                }
                self.set_status(cx, &format!("saved {} ({:?})", path, result));
            }
            StudioToUI::RunnableBuilds { mount, builds } => {
                self.data
                    .mount_runnable_builds
                    .insert(mount.clone(), builds);
                if self.data.active_mount.as_deref() == Some(mount.as_str()) {
                    self.refresh_active_mount_run_list(cx);
                    self.set_status(cx, &format!("run targets loaded: {}", mount));
                }
            }
            StudioToUI::BuildStarted {
                build_id,
                mount,
                package,
            } => {
                self.data.build_to_mount.insert(build_id, mount.clone());
                if !self.data.log_tab_by_build.contains_key(&build_id) {
                    let _ = self.ensure_log_tab_for_build(cx, build_id, &mount, &package, false);
                }
                if let Some(tab_id) = self.data.run_tab_by_build.get(&build_id).copied() {
                    if let Some(state) = self.data.run_tab_state.get_mut(&tab_id) {
                        state.mount = mount.clone();
                        state.package = package.clone();
                        state.status = "running".to_string();
                    }
                    if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
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
            StudioToUI::BuildStopped {
                build_id,
                exit_code,
            } => {
                self.data.build_to_mount.remove(&build_id);
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
                    }
                    if !mount.is_empty() {
                        if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                            dock.redraw_tab(cx, tab_id);
                        }
                    }
                }
            }
            StudioToUI::QueryLogResults {
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
                    push_capped_vec(
                        self.data.build_log_entries.entry(build_id).or_default(),
                        log_entry.clone(),
                        2_000,
                    );
                    push_capped_vec(
                        self.data
                            .mount_log_entries
                            .entry(mount.clone())
                            .or_default(),
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
            StudioToUI::TerminalOpened {
                path,
                history,
                grid,
            } => {
                self.data.terminal_open_paths.insert(path.clone());
                let stream = if history.is_empty() {
                    grid.text.into_bytes()
                } else {
                    history
                };
                self.data.terminal_stream_by_path.insert(path, stream);
                self.refresh_active_mount_log_panels(cx);
            }
            StudioToUI::TerminalOutput { path, data } => {
                self.data
                    .terminal_stream_by_path
                    .entry(path)
                    .or_default()
                    .extend_from_slice(&data);
                self.refresh_active_mount_log_panels(cx);
            }
            StudioToUI::TerminalExited { path, code } => {
                self.data.terminal_open_paths.remove(&path);
                self.set_status(cx, &format!("terminal exited ({})", code));
            }
            StudioToUI::Error { message } => {
                self.set_status(cx, &format!("error: {}", message));
            }
            _ => {}
        }
    }

    fn tab_id_from_widget_uid(cx: &Cx, widget_uid: WidgetUid) -> LiveId {
        let path = cx.widget_tree().path_to(widget_uid);
        path.get(path.len().wrapping_sub(2))
            .copied()
            .unwrap_or(id!(editor_first))
    }

    fn set_active_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
        if let Some(path) = self.data.tab_to_path.get(&tab_id).cloned() {
            self.data.current_file_path = Some(path.clone());
            self.set_current_file_label(cx, Some(&path));
        } else {
            self.data.current_file_path = None;
            self.set_current_file_label(cx, None);
        }
    }

    fn ensure_editor_tab_for_path(
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

    fn find_editor_anchor_tab(&self, dock: &DockRef, mount: &str) -> Option<LiveId> {
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

    fn close_editor_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
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

    fn update_editor_tab_titles(&mut self, cx: &mut Cx) {
        if self.data.tab_to_path.is_empty() {
            let mounts: Vec<String> = self.data.mount_to_tab.keys().cloned().collect();
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

    fn title_suffix(parts: &[String], depth: usize) -> String {
        let count = parts.len();
        let take = depth.min(count);
        parts[count - take..].join("/")
    }

    fn open_node_in_editor(&mut self, cx: &mut Cx, node_id: LiveId) {
        if !self.data.file_tree.is_file(node_id) {
            return;
        }
        let Some(path) = self.data.file_tree.path_for(node_id).map(ToOwned::to_owned) else {
            return;
        };
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

    fn save_tab_file(&mut self, cx: &mut Cx, tab_id: LiveId) {
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

    fn find_run_anchor_tab(&self, dock: &DockRef, mount: &str) -> Option<LiveId> {
        if dock.find_tab_bar_of_tab(id!(run_first)).is_some() {
            return Some(id!(run_first));
        }
        for (tab_id, state) in &self.data.run_tab_state {
            if state.mount == mount && dock.find_tab_bar_of_tab(*tab_id).is_some() {
                return Some(*tab_id);
            }
        }
        None
    }

    fn ensure_run_tab_for_build(
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
                if select {
                    dock.select_tab(cx, tab_id);
                }
                return Some(tab_id);
            }
            self.data.run_tab_by_build.remove(&build_id);
            self.data.run_tab_state.remove(&tab_id);
        }

        let anchor = self.find_run_anchor_tab(&dock, mount)?;
        let (tab_bar, pos) = dock.find_tab_bar_of_tab(anchor)?;
        let tab_id = dock.unique_id(LiveId::from_str(&format!("run/{}/{}", mount, build_id.0)).0);
        let created = if select {
            dock.create_and_select_tab(
                cx,
                tab_bar,
                tab_id,
                id!(RunningAppPane),
                package.to_string(),
                id!(CloseableTab),
                Some(pos),
            )
        } else {
            dock.create_tab(
                cx,
                tab_bar,
                tab_id,
                id!(RunningAppPane),
                package.to_string(),
                id!(CloseableTab),
                Some(pos),
            )
        };
        if created.is_none() {
            return None;
        }

        self.data.run_tab_by_build.insert(build_id, tab_id);
        self.data.run_tab_state.insert(
            tab_id,
            RunTabState {
                mount: mount.to_string(),
                package: package.to_string(),
                build_id,
                status: "starting".to_string(),
            },
        );
        dock.set_tab_title(cx, tab_id, package.to_string());
        Some(tab_id)
    }

    fn find_log_anchor_tab(&self, dock: &DockRef, mount: &str) -> Option<LiveId> {
        if dock.find_tab_bar_of_tab(id!(log_first)).is_some() {
            return Some(id!(log_first));
        }
        for (tab_id, state) in &self.data.log_tab_state {
            if state.mount == mount && dock.find_tab_bar_of_tab(*tab_id).is_some() {
                return Some(*tab_id);
            }
        }
        None
    }

    fn ensure_log_tab_for_build(
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
                }
                return Some(tab_id);
            }
            self.data.log_tab_by_build.remove(&build_id);
            self.data.log_tab_state.remove(&tab_id);
        }

        let anchor = self.find_log_anchor_tab(&dock, mount)?;
        let (tab_bar, pos) = dock.find_tab_bar_of_tab(anchor)?;
        let tab_id = dock.unique_id(LiveId::from_str(&format!("log/{}/{}", mount, build_id.0)).0);
        let created = if select {
            dock.create_and_select_tab(
                cx,
                tab_bar,
                tab_id,
                id!(LogPane),
                title.to_string(),
                id!(CloseableTab),
                Some(pos),
            )
        } else {
            dock.create_tab(
                cx,
                tab_bar,
                tab_id,
                id!(LogPane),
                title.to_string(),
                id!(CloseableTab),
                Some(pos),
            )
        };
        if created.is_none() {
            return None;
        }
        self.data.log_tab_by_build.insert(build_id, tab_id);
        self.data.log_tab_state.insert(
            tab_id,
            LogTabState {
                mount: mount.to_string(),
                build_id,
            },
        );
        dock.set_tab_title(cx, tab_id, title.to_string());
        Some(tab_id)
    }

    fn send_terminal_input(&mut self, path: &str, data: Vec<u8>) {
        self.ensure_terminal_session_open(path);
        let _ = self.send_studio(UIToStudio::TerminalInput {
            path: path.to_string(),
            data,
        });
    }

    fn send_terminal_resize(&mut self, path: &str, cols: u16, rows: u16) {
        self.ensure_terminal_session_open(path);
        let _ = self.send_studio(UIToStudio::TerminalResize {
            path: path.to_string(),
            cols,
            rows,
        });
    }

    fn apply_cursor_jump(session: &CodeSession, line: usize, column: usize) {
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

    fn apply_pending_log_jump(&mut self, path: &str, tab_id: LiveId) {
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

    fn open_log_location(&mut self, cx: &mut Cx, path: &str, line: usize, column: usize) {
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

    fn extract_log_location(&self, mount: &str, entry: &LogEntry) -> Option<UiLogLocation> {
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

    fn virtualize_log_path(&self, mount: &str, raw_path: &str) -> Option<String> {
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

    fn absolute_to_virtual_path(&self, mount: &str, abs_path: &Path) -> Option<String> {
        if let Some(root) = self.data.mount_roots.get(mount) {
            if let Ok(rel) = abs_path.strip_prefix(root) {
                return Some(format!("{}/{}", mount, path_to_virtual(rel)));
            }
        }

        for (other_mount, root) in &self.data.mount_roots {
            if let Ok(rel) = abs_path.strip_prefix(root) {
                return Some(format!("{}/{}", other_mount, path_to_virtual(rel)));
            }
        }
        None
    }

    fn run_package(&mut self, cx: &mut Cx, mount: &str, package: &str) {
        if self.data.active_mount.as_deref() != Some(mount) {
            self.select_mount(cx, mount);
        }
        let Some(build_id) = self.send_studio(UIToStudio::CargoRun {
            mount: mount.to_string(),
            args: vec![
                "run".to_string(),
                "-p".to_string(),
                package.to_string(),
                "--message-format=json".to_string(),
                "--".to_string(),
                "--message-format=json".to_string(),
                "--stdin-loop".to_string(),
            ],
            startup_query: None,
            env: None,
            buildbox: None,
        }) else {
            self.set_status(cx, "backend not connected");
            return;
        };
        self.data.build_to_mount.insert(build_id, mount.to_string());
        if self
            .ensure_run_tab_for_build(cx, build_id, mount, package, true)
            .is_none()
        {
            self.set_status(cx, "failed to create run tab");
            return;
        }
        let _ = self.ensure_log_tab_for_build(cx, build_id, mount, package, true);
        if let Some(tab_id) = self.data.run_tab_by_build.get(&build_id).copied() {
            if let Some(dock) = self.mount_workspace_dock(cx, mount) {
                dock.redraw_tab(cx, tab_id);
            }
        }
        self.set_status(cx, &format!("starting {} on {}", package, mount));
    }

    fn handle_terminal_actions(&mut self, actions: &Actions) {
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

    fn close_run_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
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

    fn close_log_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
        let Some(state) = self.data.log_tab_state.remove(&tab_id) else {
            return;
        };
        self.data.log_tab_by_build.remove(&state.build_id);
        if let Some(dock) = self.mount_workspace_dock(cx, &state.mount) {
            dock.close_tab(cx, tab_id);
        }
    }
}

fn push_capped_vec<T>(entries: &mut Vec<T>, entry: T, max_len: usize) {
    entries.push(entry);
    if entries.len() > max_len {
        let remove = entries.len() - max_len;
        entries.drain(..remove);
    }
}

fn parse_path_line_column_token(token: &str) -> Option<(String, usize, usize)> {
    let cleaned = token.trim_matches(|c| matches!(c, '"' | '\'' | '(' | ')' | ',' | ';'));
    let (path_and_line, column_str) = cleaned.rsplit_once(':')?;
    let (path, line_str) = path_and_line.rsplit_once(':')?;
    let line = line_str.parse::<usize>().ok()?.max(1);
    let column = column_str.parse::<usize>().ok()?.max(1);
    if path.is_empty() {
        return None;
    }
    Some((path.to_string(), line, column))
}

fn path_to_virtual(path: &Path) -> String {
    let parts: Vec<String> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

#[derive(Clone)]
pub struct RunTabState {
    pub mount: String,
    pub package: String,
    pub build_id: QueryId,
    pub status: String,
}

#[derive(Clone)]
pub struct LogTabState {
    pub mount: String,
    pub build_id: QueryId,
}

#[derive(Clone, Debug)]
pub struct UiLogLocation {
    pub path: String,
    pub line: usize,
    pub column: usize,
}

impl UiLogLocation {
    pub fn display_label(&self) -> String {
        format!("{}:{}:{}", self.path, self.line, self.column)
    }
}

#[derive(Clone, Debug)]
pub struct UiLogEntry {
    pub level: LogLevel,
    pub source: LogSource,
    pub message: String,
    pub location: Option<UiLogLocation>,
}

#[derive(Default)]
pub struct AppData {
    pub studio: Option<StudioConnection>,
    pub mount_roots: HashMap<String, PathBuf>,
    pub mount_file_trees: HashMap<String, FileTreeData>,
    pub mount_runnable_builds: HashMap<String, Vec<RunnableBuild>>,
    pub mount_to_tab: HashMap<String, LiveId>,
    pub tab_to_mount: HashMap<LiveId, String>,
    pub active_mount: Option<String>,
    pub file_tree: FlatFileTree,
    pub sessions: HashMap<LiveId, CodeSession>,
    pub path_to_tab: HashMap<String, LiveId>,
    pub tab_to_path: HashMap<LiveId, String>,
    pub pending_open_paths: HashSet<String>,
    pub current_file_path: Option<String>,
    pub run_tab_state: HashMap<LiveId, RunTabState>,
    pub run_tab_by_build: HashMap<QueryId, LiveId>,
    pub log_tab_state: HashMap<LiveId, LogTabState>,
    pub log_tab_by_build: HashMap<QueryId, LiveId>,
    pub build_log_entries: HashMap<QueryId, Vec<UiLogEntry>>,
    pub build_to_mount: HashMap<QueryId, String>,
    pub live_log_query: Option<QueryId>,
    pub mount_log_entries: HashMap<String, Vec<UiLogEntry>>,
    pub terminal_stream_by_path: HashMap<String, Vec<u8>>,
    pub terminal_open_paths: HashSet<String>,
    pub mount_terminal_files: HashMap<String, Vec<String>>,
    pub mount_terminal_path_to_tab: HashMap<String, HashMap<String, LiveId>>,
    pub mount_terminal_tab_to_path: HashMap<String, HashMap<LiveId, String>>,
    pub pending_log_jumps: HashMap<String, (usize, usize)>,
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    pub ui: WidgetRef,
    #[rust]
    pub data: AppData,
    #[rust]
    pub poll_timer: Timer,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.poll_timer = cx.start_interval(0.05);
        self.start_backend(cx);
        self.set_current_file_label(cx, None);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if let Some(active_mount) = self.data.active_mount.clone() {
            if let Some(workspace) = self.mount_workspace_widget(cx, &active_mount) {
                if let Some(node_id) = workspace
                    .desktop_file_tree(cx, ids!(file_tree))
                    .file_clicked(actions)
                {
                    self.open_node_in_editor(cx, node_id);
                }
                if let Some((mount, package)) = workspace
                    .desktop_run_list(cx, ids!(run_list))
                    .run_requested(actions)
                {
                    self.run_package(cx, &mount, &package);
                }
                if let Some((path, line, column)) = workspace
                    .desktop_log_view(cx, ids!(log_view))
                    .open_location_requested(actions)
                {
                    self.open_log_location(cx, &path, line, column);
                }
            }
        }

        self.handle_terminal_actions(actions);

        for action in actions {
            if let Some(action) = action.as_widget_action() {
                match action.cast() {
                    DockAction::TabWasPressed(tab_id) => {
                        if let Some(mount) = self.data.tab_to_mount.get(&tab_id).cloned() {
                            self.select_mount(cx, &mount);
                        } else if tab_id == id!(terminal_add) {
                            if let Some(mount) = self.data.active_mount.clone() {
                                self.create_new_terminal_tab(cx, &mount);
                            }
                        } else {
                            if let Some(mount) = self.data.active_mount.clone() {
                                if let Some(path) = self
                                    .data
                                    .mount_terminal_tab_to_path
                                    .get(&mount)
                                    .and_then(|tabs| tabs.get(&tab_id))
                                    .cloned()
                                {
                                    self.ensure_terminal_session_open(&path);
                                }
                            }
                            self.set_active_tab(cx, tab_id);
                        }
                    }
                    DockAction::TabCloseWasPressed(tab_id) => {
                        if self.data.tab_to_mount.contains_key(&tab_id) {
                            continue;
                        }
                        if self.data.run_tab_state.contains_key(&tab_id) {
                            self.close_run_tab(cx, tab_id);
                        } else if self.data.log_tab_state.contains_key(&tab_id) {
                            self.close_log_tab(cx, tab_id);
                        } else if self.data.tab_to_path.contains_key(&tab_id) {
                            self.close_editor_tab(cx, tab_id);
                        } else if tab_id != id!(terminal_add) {
                            if let Some(mount) = self.data.active_mount.clone() {
                                let is_terminal_tab = self
                                    .data
                                    .mount_terminal_tab_to_path
                                    .get(&mount)
                                    .is_some_and(|tabs| tabs.contains_key(&tab_id));
                                if is_terminal_tab {
                                    self.delete_terminal_tab_file(cx, &mount, tab_id);
                                } else if tab_id != id!(terminal_first) {
                                    if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                                        dock.close_tab(cx, tab_id);
                                    }
                                }
                            }
                        }
                    }
                    DockAction::SplitPanelChanged { .. }
                    | DockAction::ShouldTabStartDrag(_)
                    | DockAction::Drag(_)
                    | DockAction::Drop(_)
                    | DockAction::None => {}
                }

                match action.cast() {
                    CodeEditorAction::TextDidChange => {
                        let tab_id = Self::tab_id_from_widget_uid(cx, action.widget_uid);
                        self.save_tab_file(cx, tab_id);
                    }
                    CodeEditorAction::UnhandledKeyDown(_) => {}
                    CodeEditorAction::None => {}
                }
            }
        }
    }

    fn handle_timer(&mut self, cx: &mut Cx, event: &TimerEvent) {
        if self.poll_timer.is_timer(event).is_some() {
            self.drain_studio_messages(cx);
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui
            .handle_event(cx, event, &mut Scope::with_data(&mut self.data));

        self.drain_studio_messages(cx);
    }
}

#[derive(Clone)]
struct FlatNode {
    id: LiveId,
    path: String,
    name: String,
    node_type: FileNodeType,
    git_status: GitStatus,
    children: Vec<LiveId>,
}

#[derive(Default)]
pub struct FlatFileTree {
    nodes: HashMap<LiveId, FlatNode>,
    roots: Vec<LiveId>,
    path_to_id: HashMap<String, LiveId>,
}

impl FlatFileTree {
    pub fn rebuild(&mut self, data: FileTreeData) {
        self.nodes.clear();
        self.roots.clear();
        self.path_to_id.clear();

        for node in data.nodes {
            let id = LiveId::from_str(&node.path);
            self.path_to_id.insert(node.path.clone(), id);
            self.nodes.insert(
                id,
                FlatNode {
                    id,
                    path: node.path,
                    name: node.name,
                    node_type: node.node_type,
                    git_status: node.git_status,
                    children: Vec::new(),
                },
            );
        }

        let ids: Vec<LiveId> = self.nodes.keys().copied().collect();
        for id in &ids {
            let Some(path) = self.nodes.get(id).map(|node| node.path.clone()) else {
                continue;
            };
            let Some((parent_path, _)) = path.rsplit_once('/') else {
                self.roots.push(*id);
                continue;
            };
            if let Some(parent_id) = self.path_to_id.get(parent_path).copied() {
                if let Some(parent) = self.nodes.get_mut(&parent_id) {
                    parent.children.push(*id);
                }
            } else {
                self.roots.push(*id);
            }
        }

        let mut sort_meta = HashMap::<LiveId, (bool, String)>::new();
        for (id, node) in &self.nodes {
            sort_meta.insert(
                *id,
                (
                    matches!(node.node_type, FileNodeType::Dir),
                    node.name.clone(),
                ),
            );
        }

        for node in self.nodes.values_mut() {
            node.children.sort_by(|a, b| {
                let Some((a_dir, a_name)) = sort_meta.get(a) else {
                    return std::cmp::Ordering::Equal;
                };
                let Some((b_dir, b_name)) = sort_meta.get(b) else {
                    return std::cmp::Ordering::Equal;
                };
                match b_dir.cmp(a_dir) {
                    std::cmp::Ordering::Equal => a_name.cmp(b_name),
                    other => other,
                }
            });
        }

        self.roots.sort_by(|a, b| {
            let Some((a_dir, a_name)) = sort_meta.get(a) else {
                return std::cmp::Ordering::Equal;
            };
            let Some((b_dir, b_name)) = sort_meta.get(b) else {
                return std::cmp::Ordering::Equal;
            };
            match b_dir.cmp(a_dir) {
                std::cmp::Ordering::Equal => a_name.cmp(b_name),
                other => other,
            }
        });
    }

    pub fn draw(&self, cx: &mut Cx2d, file_tree: &mut FileTree) {
        for root_id in &self.roots {
            self.draw_node(cx, file_tree, *root_id);
        }
    }

    pub fn is_file(&self, node_id: LiveId) -> bool {
        self.nodes
            .get(&node_id)
            .is_some_and(|node| matches!(node.node_type, FileNodeType::File))
    }

    pub fn path_for(&self, node_id: LiveId) -> Option<&str> {
        self.nodes.get(&node_id).map(|node| node.path.as_str())
    }

    fn draw_node(&self, cx: &mut Cx2d, file_tree: &mut FileTree, node_id: LiveId) {
        let Some(node) = self.nodes.get(&node_id) else {
            return;
        };
        let status = git_status_dot(node.git_status);

        if matches!(node.node_type, FileNodeType::Dir) {
            if file_tree
                .begin_folder_with_status(cx, node.id, &node.name, status)
                .is_ok()
            {
                for child_id in &node.children {
                    self.draw_node(cx, file_tree, *child_id);
                }
                file_tree.end_folder();
            }
        } else {
            file_tree.file_with_status(cx, node.id, &node.name, status);
        }
    }
}

fn git_status_dot(status: GitStatus) -> GitStatusDotKind {
    match status {
        GitStatus::Added | GitStatus::Untracked => GitStatusDotKind::New,
        GitStatus::Modified | GitStatus::Staged => GitStatusDotKind::Modified,
        GitStatus::Deleted => GitStatusDotKind::Deleted,
        GitStatus::Conflict => GitStatusDotKind::Mixed,
        GitStatus::Clean | GitStatus::Ignored | GitStatus::Unknown => GitStatusDotKind::None,
    }
}
