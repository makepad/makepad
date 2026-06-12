use super::*;

pub(crate) fn editor_tab_titles_for_paths(tab_to_path: &HashMap<LiveId, String>) -> HashMap<LiveId, String> {
    let mut parts_by_tab: HashMap<LiveId, Vec<String>> = HashMap::new();
    let mut depth_by_tab: HashMap<LiveId, usize> = HashMap::new();
    for (tab_id, path) in tab_to_path {
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
                .entry(App::title_suffix(parts, depth))
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

    parts_by_tab
        .iter()
        .map(|(tab_id, parts)| {
            let depth = depth_by_tab.get(tab_id).copied().unwrap_or(1);
            (*tab_id, App::title_suffix(parts, depth))
        })
        .collect()
}

impl App {
    pub(crate) fn tab_id_from_widget_uid(cx: &Cx, widget_uid: WidgetUid) -> LiveId {
        let path = cx.widget_tree().path_to(widget_uid);
        path.get(path.len().wrapping_sub(2))
            .copied()
            .unwrap_or(id!(editor_first))
    }

    pub(crate) fn set_active_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
        if let Some(path) = self.data.tab_to_path.get(&tab_id).cloned() {
            self.data.current_file_path = Some(path.clone());
            self.set_current_file_label(cx, Some(&path));
        } else {
            self.data.current_file_path = None;
            self.set_current_file_label(cx, None);
        }
    }

    pub(crate) fn ensure_editor_tab_for_path(
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
        let (tab_bar, pos) = Self::reachable_tab_bar_of_tab(&dock, anchor_tab_id)?;
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

    pub(crate) fn find_editor_anchor_tab(&self, dock: &DockRef, mount: &str) -> Option<LiveId> {
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

    pub(crate) fn close_editor_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
        if !self.data.tab_to_path.contains_key(&tab_id) {
            return;
        }
        if let Some(path) = self.data.tab_to_path.get(&tab_id).cloned() {
            if Self::is_terminal_virtual_path(&path) {
                self.delete_terminal_path(cx, &path);
                return;
            }
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

    pub(crate) fn update_editor_tab_titles(&mut self, cx: &mut Cx) {
        if self.data.tab_to_path.is_empty() {
            let mounts: Vec<String> = self.data.mounts.keys().cloned().collect();
            for mount in mounts {
                if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                    dock.set_tab_title(cx, id!(editor_first), String::new());
                }
            }
            return;
        }

        for (tab_id, title) in editor_tab_titles_for_paths(&self.data.tab_to_path) {
            let mount = self
                .data
                .tab_to_path
                .get(&tab_id)
                .and_then(|path| Self::mount_from_virtual_path(path))
                .map(ToOwned::to_owned);
            if let Some(mount) = mount {
                if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
                    dock.set_tab_title(cx, tab_id, title);
                }
            }
        }
    }

    pub(crate) fn title_suffix(parts: &[String], depth: usize) -> String {
        let count = parts.len();
        let take = depth.min(count);
        parts[count - take..].join("/")
    }

    pub(crate) fn open_path_in_editor(&mut self, cx: &mut Cx, path: &str) {
        if Self::is_terminal_virtual_path(path) {
            self.reveal_terminal_path(cx, path);
            self.set_status(
                cx,
                &format!("opened terminal {}", Self::default_terminal_tab_title(path)),
            );
            return;
        }
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
        let _ = self.send_studio(ClientToHub::OpenTextFile { path });
    }

    pub(crate) fn open_node_in_editor(&mut self, cx: &mut Cx, node_id: LiveId) {
        if !self.data.file_tree.is_file(node_id) {
            return;
        }
        let Some(path) = self.data.file_tree.path_for(node_id).map(ToOwned::to_owned) else {
            return;
        };
        self.open_path_in_editor(cx, &path);
    }

    pub(crate) fn save_tab_file(&mut self, cx: &mut Cx, tab_id: LiveId) {
        let Some(path) = self.data.tab_to_path.get(&tab_id).cloned() else {
            return;
        };
        let Some(session) = self.data.sessions.get(&tab_id) else {
            return;
        };
        let content = session.document().as_text().to_string();
        let _ = self.send_studio(ClientToHub::SaveTextFile { path, content });
        self.set_status(cx, "saving...");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(name: &str) -> LiveId {
        LiveId::from_str(name)
    }

    #[test]
    fn editor_tab_titles_use_file_names_when_unique() {
        let paths = HashMap::from([
            (tab("a"), "makepad/studio/desktop/src/main.rs".to_string()),
            (tab("b"), "makepad/studio/hub/src/lib.rs".to_string()),
        ]);

        let titles = editor_tab_titles_for_paths(&paths);

        assert_eq!(titles.get(&tab("a")).map(String::as_str), Some("main.rs"));
        assert_eq!(titles.get(&tab("b")).map(String::as_str), Some("lib.rs"));
    }

    #[test]
    fn editor_tab_titles_expand_only_colliding_suffixes() {
        let paths = HashMap::from([
            (
                tab("desktop"),
                "makepad/studio/desktop/src/main.rs".to_string(),
            ),
            (tab("hub"), "makepad/studio/hub/src/main.rs".to_string()),
            (
                tab("protocol"),
                "makepad/platform/studio/src/lib.rs".to_string(),
            ),
        ]);

        let titles = editor_tab_titles_for_paths(&paths);

        assert_eq!(
            titles.get(&tab("desktop")).map(String::as_str),
            Some("desktop/src/main.rs")
        );
        assert_eq!(
            titles.get(&tab("hub")).map(String::as_str),
            Some("hub/src/main.rs")
        );
        assert_eq!(
            titles.get(&tab("protocol")).map(String::as_str),
            Some("lib.rs")
        );
    }

    #[test]
    fn editor_tab_titles_fall_back_to_full_paths_for_identical_paths() {
        let paths = HashMap::from([
            (tab("one"), "makepad/studio/src/main.rs".to_string()),
            (tab("two"), "makepad/studio/src/main.rs".to_string()),
        ]);

        let titles = editor_tab_titles_for_paths(&paths);

        assert_eq!(
            titles.get(&tab("one")).map(String::as_str),
            Some("makepad/studio/src/main.rs")
        );
        assert_eq!(
            titles.get(&tab("two")).map(String::as_str),
            Some("makepad/studio/src/main.rs")
        );
    }
}
