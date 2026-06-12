use super::*;
use makepad_studio_protocol::hub_protocol::{FileNode, FileTreeChange};
use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

#[path = "app_backend/cli.rs"]
pub mod cli;
use cli::*;
#[path = "app_backend/panel_anim.rs"]
pub mod panel_anim;
use panel_anim::*;
#[path = "app_backend/terminal_sync.rs"]
pub mod terminal_sync;
use terminal_sync::*;

const FILE_FILTER_DEBOUNCE_SECONDS: f64 = 0.14;
const FILE_FILTER_MAX_RESULTS: usize = 600;

impl App {




    pub(super) fn start_backend(&mut self, cx: &mut Cx) {
        let current_path = match env::current_dir().and_then(|p| p.canonicalize()) {
            Ok(path) => path,
            Err(err) => {
                self.set_status(cx, &format!("failed to resolve current dir: {}", err));
                return;
            }
        };

        let mut mounts = if let Some(spec) = parse_cli_mounts_spec() {
            parse_mounts_spec(&spec, ',', ':')
        } else if let Ok(spec) = env::var("STUDIO2_MOUNTS") {
            parse_mounts_spec(&spec, ';', '=')
        } else {
            Vec::new()
        };
        if mounts.is_empty() {
            mounts.push(MountConfig {
                name: "makepad".to_string(),
                path: current_path,
            });
        }

        let listen_address = match parse_cli_bind_address(parse_cli_bind_spec()) {
            Ok(addr) => addr,
            Err(err) => {
                self.set_status(cx, &err);
                return;
            }
        };

        let config = HubConfig {
            listen_address,
            mounts: mounts.clone(),
            enable_in_process_gateway: true,
            ..Default::default()
        };

        match StudioHub::start_in_process(config) {
            Ok(studio) => {
                self.data.studio = Some(studio);
                for mount in &mounts {
                    self.data.mounts.entry(mount.name.clone()).or_default().root =
                        mount.path.clone();
                    let _ = self.ensure_mount_tab(cx, &mount.name);
                    let _ = self.send_studio(ClientToHub::LoadFileTree {
                        mount: mount.name.clone(),
                    });
                    let _ = self.send_studio(ClientToHub::ObserveMount {
                        mount: mount.name.clone(),
                        primary: Some(true),
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

    pub(super) fn send_studio(&mut self, msg: ClientToHub) -> Option<QueryId> {
        self.data.studio.as_mut().map(|studio| studio.send(msg))
    }

    pub(super) fn studio_addr(&self) -> Option<String> {
        self.data.studio.as_ref().and_then(|s| s.studio_addr())
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
                self.sync_mount_tab_bar_visibility(cx);
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
            let (tab_bar, pos) = Self::reachable_tab_bar_of_tab(&dock, anchor)?;
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
        self.sync_mount_tab_bar_visibility(cx);
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

    pub(super) fn mount_terminal_dock(&mut self, cx: &mut Cx, mount: &str) -> Option<DockRef> {
        let dock = self.mount_workspace_dock(cx, mount)?;
        if dock.find_tab_bar_of_tab(id!(bottom_terminal_tab)).is_some() {
            dock.close_tab(cx, id!(bottom_terminal_tab));
        }
        if dock.find_tab_bar_of_tab(id!(terminal_first)).is_none() {
            let (tab_bar, pos) = Self::reachable_tab_bar_of_tab(&dock, id!(log_first))?;
            if dock
                .create_tab(
                    cx,
                    tab_bar,
                    id!(terminal_first),
                    id!(TerminalFirstPane),
                    "Terminal".to_string(),
                    id!(TerminalTab),
                    Some(pos),
                )
                .is_none()
            {
                return None;
            }
        }
        dock.set_tab_title(cx, id!(terminal_first), "Terminal".to_string());
        Some(dock)
    }

    pub(super) fn refresh_active_mount_tree(&mut self, cx: &mut Cx) {
        let Some(active_mount) = self.data.active_mount.clone() else {
            self.data.file_tree = FlatFileTree::default();
            return;
        };
        let Some(workspace) = self.mount_workspace_widget(cx, &active_mount) else {
            return;
        };
        // Take the data out temporarily to avoid cloning the entire FileTreeData.
        let Some(tree_data) = self.mount_state_mut(&active_mount).file_tree_data.take() else {
            self.data.file_tree = FlatFileTree::default();
            workspace.widget(cx, ids!(file_tree)).redraw(cx);
            return;
        };
        self.data.file_tree.rebuild(&tree_data);
        // Put it back.
        self.mount_state_mut(&active_mount).file_tree_data = Some(tree_data);
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

        let terminal_tabs: Vec<LiveId> = self
            .mount_state(&active_mount)
            .map(|state| state.terminal_tab_to_path.keys().copied().collect())
            .unwrap_or_default();

        if let Some(workspace) = self.mount_workspace_widget(cx, &active_mount) {
            workspace.widget(cx, ids!(log_view)).redraw(cx);
        }

        if let Some(dock) = self.mount_terminal_dock(cx, &active_mount) {
            for tab_id in terminal_tabs {
                dock.item(tab_id).redraw(cx);
                dock.redraw_tab(cx, tab_id);
            }
        }
    }

    pub(super) fn refresh_active_mount_terminal_panel(&mut self, cx: &mut Cx, path: &str) {
        let Some(mount) = Self::mount_from_virtual_path(path) else {
            return;
        };
        if self.data.active_mount.as_deref() != Some(mount) {
            return;
        }
        let Some(tab_id) = self
            .mount_state(mount)
            .and_then(|state| state.terminal_path_to_tab.get(path).copied())
        else {
            return;
        };
        if let Some(dock) = self.mount_terminal_dock(cx, mount) {
            dock.item(tab_id).redraw(cx);
        }
    }

    pub(super) fn default_terminal_tab_title(path: &str) -> String {
        path.rsplit('/').next().unwrap_or("terminal").to_string()
    }

    pub(super) fn terminal_tab_title(&self, path: &str) -> String {
        self.data
            .terminal_title_by_path
            .get(path)
            .cloned()
            .unwrap_or_else(|| Self::default_terminal_tab_title(path))
    }

    pub(super) fn apply_terminal_tab_title(&mut self, cx: &mut Cx, path: &str, title: String) {
        self.data
            .terminal_title_by_path
            .insert(path.to_string(), title.clone());
        let Some((mount, tab_id)) = Self::mount_from_virtual_path(path).and_then(|mount| {
            self.mount_state(mount)
                .and_then(|state| state.terminal_path_to_tab.get(path).copied())
                .map(|tab_id| (mount.to_string(), tab_id))
        }) else {
            return;
        };
        if let Some(dock) = self.mount_terminal_dock(cx, &mount) {
            dock.set_tab_title(cx, tab_id, title);
            dock.redraw_tab(cx, tab_id);
        }
    }

    pub(super) fn reset_terminal_tab_title(&mut self, cx: &mut Cx, path: &str) {
        self.data.terminal_title_by_path.remove(path);
        let title = Self::default_terminal_tab_title(path);
        let Some((mount, tab_id)) = Self::mount_from_virtual_path(path).and_then(|mount| {
            self.mount_state(mount)
                .and_then(|state| state.terminal_path_to_tab.get(path).copied())
                .map(|tab_id| (mount.to_string(), tab_id))
        }) else {
            return;
        };
        if let Some(dock) = self.mount_terminal_dock(cx, &mount) {
            dock.set_tab_title(cx, tab_id, title);
            dock.redraw_tab(cx, tab_id);
        }
    }

    pub(super) fn terminal_tab_mount_path(&self, tab_id: LiveId) -> Option<(String, String)> {
        for (mount, state) in &self.data.mounts {
            if let Some(path) = state.terminal_tab_to_path.get(&tab_id) {
                return Some((mount.clone(), path.clone()));
            }
        }
        None
    }

    fn cancel_file_filter_query(&mut self, cx: &mut Cx, mount: &str, filter: &str) {
        self.pending_file_filter = None;
        if !self.file_filter_debounce_timer.is_empty() {
            cx.stop_timer(self.file_filter_debounce_timer);
        }
        let old_query = {
            let mount_state = self.mount_state_mut(mount);
            mount_state.file_filter = filter.to_string();
            mount_state.file_filter_results.clear();
            mount_state.file_filter_pending = !filter.is_empty();
            mount_state.file_filter_query.take()
        };
        if let Some(query_id) = old_query {
            self.data.file_filter_mount_by_query.remove(&query_id);
            let _ = self.send_studio(ClientToHub::CancelQuery { query_id });
        }
    }

    fn redraw_file_tree_if_active(&mut self, cx: &mut Cx, mount: &str) {
        if self.data.active_mount.as_deref() == Some(mount) {
            if let Some(workspace) = self.mount_workspace_widget(cx, mount) {
                workspace.widget(cx, ids!(file_tree)).redraw(cx);
            }
        }
    }

    pub(super) fn set_mount_file_filter(&mut self, cx: &mut Cx, mount: &str, filter: String) {
        let filter = filter.trim().to_string();
        self.cancel_file_filter_query(cx, mount, &filter);

        if !filter.is_empty() {
            if let Some(query_id) = self.send_studio(ClientToHub::FindFiles {
                mount: Some(mount.to_string()),
                pattern: filter,
                is_regex: Some(false),
                max_results: Some(FILE_FILTER_MAX_RESULTS),
            }) {
                self.mount_state_mut(mount).file_filter_query = Some(query_id);
                self.data
                    .file_filter_mount_by_query
                    .insert(query_id, mount.to_string());
            } else {
                self.mount_state_mut(mount).file_filter_pending = false;
            }
        } else {
            self.mount_state_mut(mount).file_filter_pending = false;
        }
        self.redraw_file_tree_if_active(cx, mount);
    }

    pub(super) fn queue_mount_file_filter(&mut self, cx: &mut Cx, mount: &str, filter: String) {
        let filter = filter.trim().to_string();
        self.cancel_file_filter_query(cx, mount, &filter);

        if !filter.is_empty() {
            self.pending_file_filter = Some((mount.to_string(), filter));
            self.file_filter_debounce_timer = cx.start_timeout(FILE_FILTER_DEBOUNCE_SECONDS);
        }
        self.redraw_file_tree_if_active(cx, mount);
    }

    pub(super) fn flush_queued_mount_file_filter(&mut self, cx: &mut Cx) {
        let Some((mount, filter)) = self.pending_file_filter.take() else {
            return;
        };
        self.set_mount_file_filter(cx, &mount, filter);
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
        let pattern = self
            .mount_state(mount)
            .map(|mount_state| mount_state.log_filter.trim().to_string())
            .unwrap_or_default();
        if let Some(query_id) = self.data.live_log_query.take() {
            let _ = self.send_studio(ClientToHub::CancelQuery { query_id });
        }
        self.data.build_log_entries.clear();
        for mount_state in self.data.mounts.values_mut() {
            mount_state.log_entries.clear();
        }
        self.data.live_log_query = self.send_studio(ClientToHub::QueryLogs {
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
            live: Some(true),
        });
        self.refresh_active_mount_log_panels(cx);
    }

    pub(super) fn clear_ui_log_entries(&mut self, cx: &mut Cx) {
        self.data.build_log_entries.clear();
        for mount_state in self.data.mounts.values_mut() {
            mount_state.log_entries.clear();
        }
        self.refresh_active_mount_log_panels(cx);
    }

    pub(super) fn request_log_clear(&mut self, cx: &mut Cx) {
        let _ = self.send_studio(ClientToHub::LogClear);
        self.set_status(cx, "clearing logs...");
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
                .set_active(cx, log_tail, Animate::Yes);
            workspace
                .desktop_log_view(cx, ids!(log_view))
                .set_tail(cx, log_tail);
        }
    }

    pub(super) fn request_stop_all_builds_for_mount(&mut self, cx: &mut Cx, mount: &str) {
        self.data.pending_stop_all_mount = Some(mount.to_string());
        let _ = self.send_studio(ClientToHub::ListBuilds);
        self.set_status(cx, &format!("requesting stop-all for {}", mount));
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
            let _ = self.send_studio(ClientToHub::LoadFileTree {
                mount: mount.to_string(),
            });
            self.set_status(cx, &format!("loading mount: {}", mount));
        }
        self.ensure_mount_terminal_file(cx, mount);
        self.apply_mount_toolbar_state(cx, mount);
        self.restart_log_query_for_mount(cx, mount);
        self.refresh_active_mount_run_list(cx);
        self.refresh_active_mount_log_panels(cx);
        self.request_ai_mount_state(mount);
        self.refresh_ai_manager_report(cx);
    }
}
