use crate::{makepad_widgets::*, App};
use makepad_studio_protocol::hub_protocol::{ClientToHub, FileNode, FileNodeType, FileTreeChange};
use std::collections::{HashMap, HashSet};

impl App {
    pub(crate) fn apply_mount_file_tree_diff(
        &mut self,
        cx: &mut Cx,
        mount: &str,
        changes: Vec<FileTreeChange>,
    ) {
        if changes.is_empty() {
            return;
        }
        let mut changed = false;
        let Some(tree) = self.mount_state_mut(mount).file_tree_data.as_mut() else {
            let _ = self.send_studio(ClientToHub::LoadFileTree {
                mount: mount.to_string(),
            });
            return;
        };

        for change in changes {
            match change {
                FileTreeChange::Added {
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
                        tree.nodes.push(FileNode {
                            path,
                            name,
                            node_type,
                            git_status,
                        });
                        changed = true;
                    }
                }
                FileTreeChange::Removed { path } => {
                    let prefix = format!("{}/", path);
                    let before = tree.nodes.len();
                    tree.nodes
                        .retain(|node| node.path != path && !node.path.starts_with(&prefix));
                    if tree.nodes.len() != before {
                        changed = true;
                    }
                }
                FileTreeChange::Modified { path, git_status } => {
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

    pub(crate) fn collect_mount_terminal_files(&self, mount: &str) -> Vec<String> {
        let tree = self
            .mount_state(mount)
            .and_then(|mount| mount.file_tree_data.as_ref());
        let prefix = format!("{}/.makepad/", mount);
        let mut files: HashSet<String> = HashSet::new();

        if let Some(tree) = tree {
            for node in &tree.nodes {
                if !matches!(node.node_type, FileNodeType::File) {
                    continue;
                }
                if !node.path.starts_with(&prefix) || !node.path.ends_with(".term") {
                    continue;
                }
                let tail = &node.path[prefix.len()..];
                if tail.contains('/') {
                    continue;
                }
                files.insert(node.path.clone());
            }
        }

        if let Some(state) = self.mount_state(mount) {
            for path in &state.terminal_files {
                if Self::is_terminal_virtual_path(path)
                    && Self::mount_from_virtual_path(path.as_str()) == Some(mount)
                {
                    files.insert(path.clone());
                }
            }
        }
        for path in self.data.terminal_framebuffer_by_path.keys() {
            if Self::is_terminal_virtual_path(path)
                && Self::mount_from_virtual_path(path.as_str()) == Some(mount)
            {
                files.insert(path.clone());
            }
        }

        let mut files: Vec<String> = files.into_iter().collect();
        files.sort();
        files
    }

    pub(crate) fn sync_mount_terminal_tabs(&mut self, cx: &mut Cx, mount: &str, select_last: bool) {
        let files = self
            .mount_state(mount)
            .map(|mount| mount.terminal_files.clone())
            .unwrap_or_default();
        let titles: HashMap<String, String> = files
            .iter()
            .map(|path| (path.clone(), self.terminal_tab_title(path)))
            .collect();

        let Some(dock) = self.mount_terminal_dock(cx, mount) else {
            return;
        };

        let mount_state = self.mount_state_mut(mount);
        let path_to_tab = &mut mount_state.terminal_path_to_tab;
        let tab_to_path = &mut mount_state.terminal_tab_to_path;

        // Keep terminal_first as a persistent anchor tab.
        if let Some(old_path) = tab_to_path.remove(&id!(terminal_first)) {
            path_to_tab.remove(&old_path);
        }
        path_to_tab.retain(|_, tab_id| *tab_id != id!(terminal_first));
        dock.set_tab_title(cx, id!(terminal_first), "Terminal".to_string());

        for path in files.iter() {
            let title = titles
                .get(path)
                .cloned()
                .unwrap_or_else(|| Self::default_terminal_tab_title(path));
            // If a valid tab already exists for this path, just update its title.
            if let Some(existing) = path_to_tab.get(path).copied() {
                if dock.find_tab_bar_of_tab(existing).is_some() {
                    dock.set_tab_title(cx, existing, title);
                    continue;
                }
                path_to_tab.remove(path);
                tab_to_path.remove(&existing);
            }
            let Some((tab_bar, anchor_pos)) =
                Self::reachable_tab_bar_of_tab(&dock, id!(terminal_first))
            else {
                continue;
            };
            let insert_after = path_to_tab
                .values()
                .copied()
                .chain(std::iter::once(id!(terminal_first)))
                .filter_map(|tab_id| Self::reachable_tab_bar_of_tab(&dock, tab_id))
                .filter(|(candidate_bar, _)| *candidate_bar == tab_bar)
                .map(|(_, pos)| pos)
                .max()
                .unwrap_or(anchor_pos);
            let tab_id = dock.unique_id(LiveId::from_str(path).0);
            if dock
                .create_tab(
                    cx,
                    tab_bar,
                    tab_id,
                    id!(TerminalPane),
                    title,
                    id!(TerminalCloseableTab),
                    Some(insert_after),
                )
                .is_none()
            {
                continue;
            }
            path_to_tab.insert(path.clone(), tab_id);
            tab_to_path.insert(tab_id, path.clone());
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

    pub(crate) fn select_bottom_terminal_panel(&mut self, cx: &mut Cx, mount: &str) {
        let Some(dock) = self.mount_workspace_dock(cx, mount) else {
            return;
        };
        dock.select_tab(cx, id!(terminal_first));
    }

    pub(crate) fn reveal_bottom_terminal_panel(&mut self, cx: &mut Cx, mount: &str) {
        let Some(current_height) = self.workspace_main_splitter_height(cx, mount) else {
            return;
        };
        if current_height > 1.0 {
            return;
        }
        let restore_height = self
            .mount_state(mount)
            .and_then(|state| state.bottom_panel_restore_height)
            .unwrap_or(220.0);
        self.start_bottom_panel_animation(cx, mount, restore_height);
    }

    pub(crate) fn select_terminal_path(&mut self, cx: &mut Cx, mount: &str, path: &str) {
        let Some(tab_id) = self
            .mount_state(mount)
            .and_then(|state| state.terminal_path_to_tab.get(path).copied())
        else {
            return;
        };
        let Some(dock) = self.mount_terminal_dock(cx, mount) else {
            return;
        };
        dock.select_tab(cx, tab_id);
    }

    pub(crate) fn ensure_terminal_session_open(&mut self, path: &str) {
        if self.data.terminal_open_paths.contains(path) {
            return;
        }
        let (cols, rows) = (120u16, 40u16);
        let _ = self.send_studio(ClientToHub::TerminalOpen {
            path: path.to_string(),
            cols,
            rows,
            env: HashMap::new(),
        });
    }

    pub(crate) fn ensure_mount_terminal_file(&mut self, cx: &mut Cx, mount: &str) {
        let known_before = self
            .mount_state(mount)
            .map(|mount| mount.terminals_initialized)
            .unwrap_or(false);
        let files = self.collect_mount_terminal_files(mount);
        let keep_paths: HashSet<String> = files.iter().cloned().collect();
        let stale_paths: Vec<String> = self
            .data
            .terminal_framebuffer_by_path
            .keys()
            .filter(|path| {
                Self::mount_from_virtual_path(path.as_str()) == Some(mount)
                    && !keep_paths.contains(path.as_str())
            })
            .cloned()
            .collect();
        for stale in stale_paths {
            self.data.terminal_framebuffer_by_path.remove(&stale);
            if self.data.terminal_open_paths.remove(&stale) {
                let _ = self.send_studio(ClientToHub::TerminalClose { path: stale });
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
                .terminal_framebuffer_by_path
                .entry(path.clone())
                .or_default();
            self.ensure_terminal_session_open(path);
        }

        if !known_before && files.is_empty() {
            let path = Self::terminal_virtual_path(mount);
            {
                let mount_state = self.mount_state_mut(mount);
                mount_state.select_last_terminal_once = true;
                if !mount_state
                    .terminal_files
                    .iter()
                    .any(|existing| existing == &path)
                {
                    mount_state.terminal_files.push(path.clone());
                    mount_state.terminal_files.sort();
                }
            }
            self.data
                .terminal_framebuffer_by_path
                .entry(path.clone())
                .or_default();
            self.sync_mount_terminal_tabs(cx, mount, true);
            self.ensure_terminal_session_open(&path);
            let _ = self.send_studio(ClientToHub::SaveTextFile {
                path,
                content: String::new(),
            });
            return;
        }

        if known_before {}
    }

    pub(crate) fn reveal_terminal_path(&mut self, cx: &mut Cx, path: &str) {
        let Some(mount) = Self::mount_from_virtual_path(path).map(str::to_string) else {
            return;
        };
        {
            let mount_state = self.mount_state_mut(&mount);
            mount_state.select_last_terminal_once = true;
            if !mount_state
                .terminal_files
                .iter()
                .any(|existing| existing == path)
            {
                mount_state.terminal_files.push(path.to_string());
                mount_state.terminal_files.sort();
            }
        }
        self.reveal_bottom_terminal_panel(cx, &mount);
        self.ensure_mount_terminal_file(cx, &mount);
        self.select_terminal_path(cx, &mount, path);
    }

    pub(crate) fn next_terminal_path(&mut self, mount: &str) -> String {
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

    pub(crate) fn create_new_terminal_tab(&mut self, _cx: &mut Cx, mount: &str) {
        let path = self.next_terminal_path(mount);
        let name = path.rsplit('/').next().unwrap_or("terminal").to_string();
        {
            let mount_state = self.mount_state_mut(mount);
            mount_state.select_last_terminal_once = true;
            if !mount_state
                .terminal_files
                .iter()
                .any(|existing| existing == &path)
            {
                mount_state.terminal_files.push(path.clone());
                mount_state.terminal_files.sort();
            }
        }

        let _ = self.send_studio(ClientToHub::SaveTextFile {
            path: path.clone(),
            content: String::new(),
        });
        self.data
            .terminal_framebuffer_by_path
            .entry(path.clone())
            .or_default();
        self.sync_mount_terminal_tabs(_cx, mount, true);
        self.select_terminal_path(_cx, mount, &path);
        self.ensure_terminal_session_open(&path);
        self.set_status(_cx, &format!("created terminal {}", name));
        self.refresh_ai_manager_report(_cx);
    }

    pub(crate) fn remove_terminal_path_local_state(
        &mut self,
        cx: &mut Cx,
        path: &str,
    ) -> Option<(String, bool)> {
        if !Self::is_terminal_virtual_path(path) {
            return None;
        }
        let mount = Self::mount_from_virtual_path(path).map(str::to_string)?;
        let mut removed_any = false;

        if let Some(editor_tab_id) = self.data.path_to_tab.remove(path) {
            removed_any = true;
            self.data.tab_to_path.remove(&editor_tab_id);
            self.data.sessions.remove(&editor_tab_id);
            if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                if dock.find_tab_bar_of_tab(editor_tab_id).is_some() {
                    dock.close_tab(cx, editor_tab_id);
                }
            }
        }
        self.data.pending_open_paths.remove(path);
        self.data.pending_reload_paths.remove(path);
        if self.data.current_file_path.as_deref() == Some(path) {
            self.data.current_file_path = None;
            self.set_current_file_label(cx, None);
        }
        self.update_editor_tab_titles(cx);

        let terminal_tab_id = self
            .mount_state(&mount)
            .and_then(|mount| mount.terminal_path_to_tab.get(path).copied());
        {
            let mount_state = self.mount_state_mut(&mount);
            if let Some(tab_id) = terminal_tab_id {
                removed_any = true;
                mount_state.terminal_tab_to_path.remove(&tab_id);
            }
            if mount_state.terminal_path_to_tab.remove(path).is_some() {
                removed_any = true;
            }
            let before = mount_state.terminal_files.len();
            mount_state.terminal_files.retain(|file| file != path);
            if mount_state.terminal_files.len() != before {
                removed_any = true;
            }
        }
        if let Some(tab_id) = terminal_tab_id {
            if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                if dock.find_tab_bar_of_tab(tab_id).is_some() {
                    dock.close_tab(cx, tab_id);
                }
            }
        }

        removed_any |= self.data.terminal_open_paths.remove(path);
        removed_any |= self.data.terminal_frame_id_by_path.remove(path).is_some();
        removed_any |= self
            .data
            .terminal_framebuffer_by_path
            .remove(path)
            .is_some();
        removed_any |= self.data.terminal_title_by_path.remove(path).is_some();

        Some((mount, removed_any))
    }

    pub(crate) fn delete_terminal_path(&mut self, cx: &mut Cx, path: &str) {
        let Some((_mount, _removed_any)) = self.remove_terminal_path_local_state(cx, path) else {
            return;
        };

        let path = path.to_string();
        let _ = self.send_studio(ClientToHub::TerminalClose { path: path.clone() });
        let _ = self.send_studio(ClientToHub::DeleteFile { path });
        self.refresh_ai_manager_report(cx);
    }

    pub(crate) fn handle_terminal_exit_cleanup(&mut self, cx: &mut Cx, path: &str) {
        let Some((_mount, removed_any)) = self.remove_terminal_path_local_state(cx, path) else {
            return;
        };
        if removed_any {
            let _ = self.send_studio(ClientToHub::DeleteFile {
                path: path.to_string(),
            });
            self.refresh_ai_manager_report(cx);
        }
    }

    pub(crate) fn delete_terminal_tab_file(&mut self, cx: &mut Cx, mount: &str, tab_id: LiveId) {
        let Some(path) = self
            .mount_state(mount)
            .and_then(|mount| mount.terminal_tab_to_path.get(&tab_id))
            .cloned()
        else {
            return;
        };
        let _ = mount;
        self.delete_terminal_path(cx, &path);
    }
}
