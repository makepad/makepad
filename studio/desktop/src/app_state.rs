use super::*;
use crate::makepad_micro_serde::*;
use std::collections::{HashMap, HashSet};
use std::fs;

#[derive(SerRon, DeRon)]
struct PersistedMountStateRon {
    mount: String,
    dock_items: HashMap<LiveId, DockItem>,
    editor_tab_to_path: HashMap<LiveId, String>,
    terminal_tab_to_path: HashMap<LiveId, String>,
    file_filter: String,
    log_filter: String,
    log_tail: bool,
}

#[derive(SerRon, DeRon)]
struct AppStateRon {
    active_mount: Option<String>,
    mount_dock_items: HashMap<LiveId, DockItem>,
    mounts: Vec<PersistedMountStateRon>,
}

impl App {
    fn legacy_state_file_path(slot: usize) -> String {
        format!("makepad_state{}.ron", slot)
    }

    fn state_file_path(slot: usize) -> String {
        format!(".makepad/studio_state{}.ron", slot)
    }

    fn persistent_workspace_tab_ids(
        editor_tabs: impl IntoIterator<Item = LiveId>,
        terminal_tabs: impl IntoIterator<Item = LiveId>,
    ) -> HashSet<LiveId> {
        let mut ids = HashSet::from([
            id!(tree_tab),
            id!(run_list_tab),
            id!(editor_first),
            id!(run_first),
            id!(log_first),
            id!(terminal_first),
            id!(terminal_add),
        ]);
        ids.extend(editor_tabs);
        ids.extend(terminal_tabs);
        ids
    }

    fn collect_reachable_dock_items(
        dock_items: &HashMap<LiveId, DockItem>,
        item_id: LiveId,
        reachable: &mut HashSet<LiveId>,
    ) {
        if !reachable.insert(item_id) {
            return;
        }
        match dock_items.get(&item_id) {
            Some(DockItem::Splitter { a, b, .. }) => {
                Self::collect_reachable_dock_items(dock_items, *a, reachable);
                Self::collect_reachable_dock_items(dock_items, *b, reachable);
            }
            Some(DockItem::Tabs { tabs, .. }) => {
                for tab_id in tabs {
                    Self::collect_reachable_dock_items(dock_items, *tab_id, reachable);
                }
            }
            Some(DockItem::Tab { .. }) | None => {}
        }
    }

    fn prune_dock_item<F>(
        dock_items: &mut HashMap<LiveId, DockItem>,
        item_id: LiveId,
        keep_tab: &F,
    ) -> Option<LiveId>
    where
        F: Fn(LiveId, &str, LiveId, LiveId) -> bool,
    {
        let item = dock_items.get(&item_id)?.clone();
        match item {
            DockItem::Tab {
                ref name,
                template,
                kind,
            } => {
                if keep_tab(item_id, name, template, kind) {
                    Some(item_id)
                } else {
                    dock_items.remove(&item_id);
                    None
                }
            }
            DockItem::Tabs {
                tabs,
                selected,
                closable,
                hide_tab_bar,
            } => {
                let mut kept = Vec::new();
                for tab_id in tabs {
                    if let Some(tab_id) = Self::prune_dock_item(dock_items, tab_id, keep_tab) {
                        kept.push(tab_id);
                    }
                }
                if kept.is_empty() {
                    dock_items.remove(&item_id);
                    None
                } else {
                    let selected = selected.min(kept.len().saturating_sub(1));
                    dock_items.insert(
                        item_id,
                        DockItem::Tabs {
                            tabs: kept,
                            selected,
                            closable,
                            hide_tab_bar,
                        },
                    );
                    Some(item_id)
                }
            }
            DockItem::Splitter { axis, align, a, b } => {
                let kept_a = Self::prune_dock_item(dock_items, a, keep_tab);
                let kept_b = Self::prune_dock_item(dock_items, b, keep_tab);
                match (kept_a, kept_b) {
                    (Some(a), Some(b)) => {
                        dock_items.insert(item_id, DockItem::Splitter { axis, align, a, b });
                        Some(item_id)
                    }
                    (Some(only), None) | (None, Some(only)) => {
                        dock_items.remove(&item_id);
                        Some(only)
                    }
                    (None, None) => {
                        dock_items.remove(&item_id);
                        None
                    }
                }
            }
        }
    }

    fn sanitize_dock_items<F>(
        mut dock_items: HashMap<LiveId, DockItem>,
        keep_tab: F,
    ) -> Option<HashMap<LiveId, DockItem>>
    where
        F: Fn(LiveId, &str, LiveId, LiveId) -> bool,
    {
        let root_id = Self::prune_dock_item(&mut dock_items, id!(root), &keep_tab)?;
        if root_id != id!(root) {
            let root_item = dock_items.get(&root_id)?.clone();
            dock_items.insert(id!(root), root_item);
        }

        let mut reachable = HashSet::new();
        Self::collect_reachable_dock_items(&dock_items, id!(root), &mut reachable);
        dock_items.retain(|item_id, _| reachable.contains(item_id));
        Some(dock_items)
    }

    fn sanitize_mount_dock_items(
        dock_items: HashMap<LiveId, DockItem>,
        valid_mounts: &HashSet<String>,
    ) -> Option<HashMap<LiveId, DockItem>> {
        Self::sanitize_dock_items(dock_items, |_, name, _, kind| {
            kind == id!(MountWorkspace) && valid_mounts.contains(name)
        })
    }

    fn sanitize_workspace_dock_items(
        dock_items: HashMap<LiveId, DockItem>,
        allowed_tab_ids: &HashSet<LiveId>,
    ) -> Option<HashMap<LiveId, DockItem>> {
        Self::sanitize_dock_items(dock_items, |tab_id, _, _, _| {
            allowed_tab_ids.contains(&tab_id)
        })
    }

    fn rebuild_mount_tab_bindings(&mut self, cx: &Cx) {
        self.data.tab_to_mount.clear();
        for mount_state in self.data.mounts.values_mut() {
            mount_state.tab_id = None;
        }

        let Some(dock_items) = self.ui.dock(cx, ids!(mount_dock)).clone_state() else {
            return;
        };
        for (tab_id, item) in dock_items {
            let DockItem::Tab { name, kind, .. } = item else {
                continue;
            };
            if kind != id!(MountWorkspace) {
                continue;
            }
            if let Some(mount_state) = self.data.mounts.get_mut(&name) {
                mount_state.tab_id = Some(tab_id);
                self.data.tab_to_mount.insert(tab_id, name);
            }
        }
    }

    fn reset_persisted_ui_state(&mut self) {
        self.data.tab_to_mount.clear();
        self.data.path_to_tab.clear();
        self.data.tab_to_path.clear();
        self.data.sessions.clear();
        self.data.pending_open_paths.clear();
        self.data.pending_reload_paths.clear();
        self.data.current_file_path = None;
        self.data.run_tab_state.clear();
        self.data.run_tab_by_build.clear();
        self.data.log_tab_state.clear();
        self.data.log_tab_by_build.clear();
        self.data.profiler_tab_state.clear();
        self.data.profiler_tab_by_build.clear();
        self.data.build_log_entries.clear();
        self.data.profiler_samples_by_build.clear();
        self.data.profiler_running_by_build.clear();
        self.data.profiler_time_start_by_build.clear();
        self.data.build_to_mount.clear();
        self.data.build_package.clear();
        self.data.active_log_build_by_mount.clear();
        self.data.live_log_query = None;
        self.data.live_profiler_query_by_build.clear();
        self.data.profiler_query_build_by_query.clear();
        self.data.pending_log_jumps.clear();

        for mount_state in self.data.mounts.values_mut() {
            mount_state.tab_id = None;
            mount_state.terminals_initialized = false;
            mount_state.select_last_terminal_once = false;
            mount_state.terminal_files.clear();
            mount_state.terminal_path_to_tab.clear();
            mount_state.terminal_tab_to_path.clear();
            mount_state.file_filter_query = None;
            mount_state.file_filter_pending = false;
            mount_state.file_filter_results.clear();
        }
    }

    pub(super) fn load_state(&mut self, cx: &mut Cx, slot: usize) {
        let contents = fs::read_to_string(Self::state_file_path(slot))
            .or_else(|_| fs::read_to_string(Self::legacy_state_file_path(slot)));
        let Ok(contents) = contents else {
            return;
        };
        let Ok(state) = AppStateRon::deserialize_ron(&contents) else {
            return;
        };

        self.reset_persisted_ui_state();

        let mount_names: Vec<String> = self.data.mounts.keys().cloned().collect();
        let valid_mounts: HashSet<String> = mount_names.iter().cloned().collect();

        if let Some(dock_items) = Self::sanitize_mount_dock_items(state.mount_dock_items, &valid_mounts)
        {
            self.ui.dock(cx, ids!(mount_dock)).load_state(cx, dock_items);
        }
        self.rebuild_mount_tab_bindings(cx);
        for mount in &mount_names {
            let _ = self.ensure_mount_tab(cx, mount);
        }
        self.rebuild_mount_tab_bindings(cx);

        let saved_mounts: HashMap<String, PersistedMountStateRon> = state
            .mounts
            .into_iter()
            .map(|saved| (saved.mount.clone(), saved))
            .collect();
        let mut reopen_paths: Vec<String> = Vec::new();

        for mount in &mount_names {
            let Some(saved) = saved_mounts.get(mount) else {
                continue;
            };
            let Some(dock) = self.mount_workspace_dock(cx, mount) else {
                continue;
            };

            let mut editor_tab_to_path = saved.editor_tab_to_path.clone();
            let mut terminal_tab_to_path = saved.terminal_tab_to_path.clone();
            let allowed_tab_ids = Self::persistent_workspace_tab_ids(
                editor_tab_to_path.keys().copied(),
                terminal_tab_to_path.keys().copied(),
            );

            if let Some(dock_items) =
                Self::sanitize_workspace_dock_items(saved.dock_items.clone(), &allowed_tab_ids)
            {
                dock.load_state(cx, dock_items);
            }

            editor_tab_to_path.retain(|tab_id, _| dock.find_tab_bar_of_tab(*tab_id).is_some());
            terminal_tab_to_path.retain(|tab_id, _| dock.find_tab_bar_of_tab(*tab_id).is_some());

            {
                let mount_state = self.mount_state_mut(mount);
                mount_state.file_filter = saved.file_filter.clone();
                mount_state.log_filter = saved.log_filter.clone();
                mount_state.log_tail = saved.log_tail;
                mount_state.terminals_initialized = true;
                mount_state.terminal_files = terminal_tab_to_path.values().cloned().collect();
                mount_state.terminal_files.sort();
                mount_state.terminal_path_to_tab = terminal_tab_to_path
                    .iter()
                    .map(|(tab_id, path): (&LiveId, &String)| (path.clone(), *tab_id))
                    .collect::<HashMap<String, LiveId>>();
                mount_state.terminal_tab_to_path = terminal_tab_to_path.clone();
            }

            for (tab_id, path) in editor_tab_to_path {
                self.data.path_to_tab.insert(path.clone(), tab_id);
                self.data.tab_to_path.insert(tab_id, path.clone());
                reopen_paths.push(path);
            }
        }

        self.update_editor_tab_titles(cx);

        let active_mount = match state.active_mount {
            Some(mount) if self.data.mounts.contains_key(&mount) => Some(mount),
            _ => mount_names.first().cloned(),
        };
        if let Some(active_mount) = active_mount {
            self.select_mount(cx, &active_mount);
        }

        reopen_paths.sort();
        reopen_paths.dedup();
        for path in reopen_paths {
            let _ = self.send_studio(ClientToHub::OpenTextFile { path });
        }
    }

    fn collect_persisted_mount_state(
        &self,
        cx: &Cx,
        mount: &str,
    ) -> Option<PersistedMountStateRon> {
        let tab_id = self.mount_state(mount)?.tab_id?;
        let workspace = self.ui.dock(cx, ids!(mount_dock)).item(tab_id);
        let dock = workspace.dock(cx, ids!(dock));

        let mut editor_tab_to_path = HashMap::new();
        for (editor_tab_id, path) in &self.data.tab_to_path {
            if Self::mount_from_virtual_path(path) == Some(mount)
                && dock.find_tab_bar_of_tab(*editor_tab_id).is_some()
            {
                editor_tab_to_path.insert(*editor_tab_id, path.clone());
            }
        }

        let mut terminal_tab_to_path = HashMap::new();
        if let Some(mount_state) = self.mount_state(mount) {
            for (terminal_tab_id, path) in &mount_state.terminal_tab_to_path {
                if dock.find_tab_bar_of_tab(*terminal_tab_id).is_some() {
                    terminal_tab_to_path.insert(*terminal_tab_id, path.clone());
                }
            }
        }

        let allowed_tab_ids = Self::persistent_workspace_tab_ids(
            editor_tab_to_path.keys().copied(),
            terminal_tab_to_path.keys().copied(),
        );
        let dock_items = dock
            .clone_state()
            .and_then(|dock_items| Self::sanitize_workspace_dock_items(dock_items, &allowed_tab_ids))
            .unwrap_or_default();

        let mount_state = self.mount_state(mount)?;
        Some(PersistedMountStateRon {
            mount: mount.to_string(),
            dock_items,
            editor_tab_to_path,
            terminal_tab_to_path,
            file_filter: mount_state.file_filter.clone(),
            log_filter: mount_state.log_filter.clone(),
            log_tail: mount_state.log_tail,
        })
    }

    fn save_state(&self, cx: &Cx, slot: usize) {
        let valid_mounts: HashSet<String> = self.data.mounts.keys().cloned().collect();
        let mount_dock_items = self
            .ui
            .dock(cx, ids!(mount_dock))
            .clone_state()
            .and_then(|dock_items| Self::sanitize_mount_dock_items(dock_items, &valid_mounts))
            .unwrap_or_default();

        let mut mounts = Vec::new();
        let mut mount_names: Vec<String> = self.data.mounts.keys().cloned().collect();
        mount_names.sort();
        for mount in mount_names {
            if let Some(saved) = self.collect_persisted_mount_state(cx, &mount) {
                mounts.push(saved);
            }
        }

        let state = AppStateRon {
            active_mount: self.data.active_mount.clone(),
            mount_dock_items,
            mounts,
        };
        let _ = fs::create_dir_all(".makepad");
        let _ = fs::write(Self::state_file_path(slot), state.serialize_ron());
    }

    pub(super) fn save_state_if_needed(&mut self, cx: &mut Cx) {
        let mut needs_save = self.ui.dock(cx, ids!(mount_dock)).check_and_clear_need_save();
        let mount_names: Vec<String> = self.data.mounts.keys().cloned().collect();
        for mount in mount_names {
            let Some(tab_id) = self.mount_state(&mount).and_then(|state| state.tab_id) else {
                continue;
            };
            let workspace = self.ui.dock(cx, ids!(mount_dock)).item(tab_id);
            let dock = workspace.dock(cx, ids!(dock));
            needs_save |= dock.check_and_clear_need_save();
        }
        if needs_save {
            self.save_state(cx, 0);
        }
    }
}
