use super::*;
use makepad_studio_protocol::hub_protocol::{FileNode, FileTreeChange};
use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

fn parse_mounts_spec(spec: &str, item_sep: char, pair_sep: char) -> Vec<MountConfig> {
    spec.split(item_sep)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .filter_map(|token| {
            let (name, path_str) = token.split_once(pair_sep)?;
            let name = name.trim();
            let path_str = path_str.trim();
            if name.is_empty() || path_str.is_empty() {
                return None;
            }
            let path = std::path::PathBuf::from(path_str).canonicalize().ok()?;
            Some(MountConfig {
                name: name.to_string(),
                path,
            })
        })
        .collect()
}

fn parse_cli_arg_value(name: &str) -> Option<String> {
    let mut value = None;
    let prefixed = format!("--{name}=");
    let plain = format!("--{name}");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if let Some(parsed) = arg.strip_prefix(&prefixed) {
            value = Some(parsed.to_string());
            continue;
        }
        if arg == plain {
            value = Some(args.next().unwrap_or_default());
        }
    }
    value
}

fn parse_cli_mounts_spec() -> Option<String> {
    parse_cli_arg_value("mounts")
}

fn parse_cli_bind_spec() -> Option<String> {
    let mut value = None;
    let prefixed = "--bind=";
    for arg in std::env::args().skip(1) {
        if let Some(parsed) = arg.strip_prefix(prefixed) {
            value = Some(parsed.to_string());
            continue;
        }
        if arg == "--bind" {
            value = Some("0.0.0.0".to_string());
        }
    }
    value
}

fn parse_cli_bind_address(spec: Option<String>) -> Result<SocketAddr, String> {
    let Some(spec) = spec.map(|spec| spec.trim().to_string()) else {
        return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8001));
    };
    if spec.is_empty() {
        return Err("invalid --bind value '', expected ip or ip:port".to_string());
    }
    if let Ok(addr) = spec.parse::<SocketAddr>() {
        return Ok(addr);
    }
    if let Ok(ip) = spec.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, 8001));
    }
    Err(format!(
        "invalid --bind value '{}', expected ip or ip:port",
        spec
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cli_bind_address_defaults_to_localhost() {
        assert_eq!(
            parse_cli_bind_address(None).unwrap(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8001)
        );
    }

    #[test]
    fn parse_cli_bind_address_accepts_ip_without_port() {
        assert_eq!(
            parse_cli_bind_address(Some("0.0.0.0".to_string())).unwrap(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8001)
        );
    }

    #[test]
    fn parse_cli_bind_address_accepts_ip_with_port() {
        assert_eq!(
            parse_cli_bind_address(Some("127.0.0.1:9001".to_string())).unwrap(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9001)
        );
    }
}

const FILE_FILTER_DEBOUNCE_SECONDS: f64 = 0.14;
const FILE_FILTER_MAX_RESULTS: usize = 600;

impl App {
    fn panel_animation_progress(time: f64, start_time: &mut Option<f64>) -> f64 {
        let start_time = start_time.get_or_insert(time);
        let elapsed = (time - *start_time).max(0.0);
        let duration = 0.16;
        let progress = (elapsed / duration).min(1.0);
        1.0 - (1.0 - progress).powi(3)
    }

    fn workspace_root_splitter_position(&mut self, cx: &mut Cx, mount: &str) -> Option<f64> {
        let dock = self.mount_workspace_dock(cx, mount)?;
        dock.splitter_position(id!(root))
    }

    fn set_workspace_root_splitter_width(&mut self, cx: &mut Cx, mount: &str, width: f64) -> bool {
        let Some(dock) = self.mount_workspace_dock(cx, mount) else {
            return false;
        };
        dock.set_splitter_align(cx, id!(root), SplitterAlign::FromA(width.max(0.0)), false)
    }

    fn start_sidebar_animation(&mut self, cx: &mut Cx, mount: &str, to_width: f64) {
        let from_width = self
            .workspace_root_splitter_position(cx, mount)
            .unwrap_or(to_width);
        self.sidebar_animation = Some(SidebarAnimation {
            mount: mount.to_string(),
            from_width,
            to_width: to_width.max(0.0),
            start_time: None,
        });
        self.sidebar_animation_next_frame = cx.new_next_frame();
    }

    pub(super) fn step_sidebar_animation(&mut self, cx: &mut Cx, time: f64) {
        let Some(animation) = self.sidebar_animation.as_mut() else {
            return;
        };
        let eased = Self::panel_animation_progress(time, &mut animation.start_time);
        let progress = eased;
        let mount = animation.mount.clone();
        let target = animation.to_width;
        let width = animation.from_width + (target - animation.from_width) * eased;

        if !self.set_workspace_root_splitter_width(cx, &mount, width) {
            self.sidebar_animation = None;
            return;
        }

        if progress >= 1.0 {
            self.sidebar_animation = None;
            self.set_workspace_root_splitter_width(cx, &mount, target);
            self.save_state(cx, 0);
        } else {
            self.sidebar_animation_next_frame = cx.new_next_frame();
        }
    }

    fn workspace_main_splitter_height(&mut self, cx: &mut Cx, mount: &str) -> Option<f64> {
        let dock = self.mount_workspace_dock(cx, mount)?;
        let dock_height = dock.area().rect(cx).size.y.max(0.0);
        let splitter_position = dock.splitter_position(id!(main_split))?;
        Some((dock_height - splitter_position).max(0.0))
    }

    fn set_workspace_main_splitter_height(
        &mut self,
        cx: &mut Cx,
        mount: &str,
        height: f64,
    ) -> bool {
        let Some(dock) = self.mount_workspace_dock(cx, mount) else {
            return false;
        };
        dock.set_splitter_align(
            cx,
            id!(main_split),
            SplitterAlign::FromB(height.max(0.0)),
            false,
        )
    }

    fn start_bottom_panel_animation(&mut self, cx: &mut Cx, mount: &str, to_height: f64) {
        let from_height = self
            .workspace_main_splitter_height(cx, mount)
            .unwrap_or(to_height);
        self.bottom_panel_animation = Some(BottomPanelAnimation {
            mount: mount.to_string(),
            from_height,
            to_height: to_height.max(0.0),
            start_time: None,
        });
        self.bottom_panel_animation_next_frame = cx.new_next_frame();
    }

    pub(super) fn step_bottom_panel_animation(&mut self, cx: &mut Cx, time: f64) {
        let Some(animation) = self.bottom_panel_animation.as_mut() else {
            return;
        };
        let eased = Self::panel_animation_progress(time, &mut animation.start_time);
        let progress = eased;
        let mount = animation.mount.clone();
        let target = animation.to_height;
        let height = animation.from_height + (target - animation.from_height) * eased;

        if !self.set_workspace_main_splitter_height(cx, &mount, height) {
            self.bottom_panel_animation = None;
            return;
        }

        if progress >= 1.0 {
            self.bottom_panel_animation = None;
            self.set_workspace_main_splitter_height(cx, &mount, target);
            self.save_state(cx, 0);
        } else {
            self.bottom_panel_animation_next_frame = cx.new_next_frame();
        }
    }

    fn sync_mount_tab_bar_visibility(&mut self, cx: &mut Cx) {
        let dock = self.ui.dock(cx, ids!(mount_dock));
        let Some(mut dock_items) = dock.clone_state() else {
            return;
        };

        let mut changed = false;
        for item in dock_items.values_mut() {
            let DockItem::Tabs {
                tabs,
                selected,
                closable,
                hide_tab_bar,
            } = item
            else {
                continue;
            };
            let should_hide = tabs.len() <= 1;
            if *hide_tab_bar == should_hide {
                continue;
            }
            *item = DockItem::Tabs {
                tabs: tabs.clone(),
                selected: *selected,
                closable: *closable,
                hide_tab_bar: should_hide,
            };
            changed = true;
        }

        if changed {
            dock.load_state(cx, dock_items);
        }
    }

    pub(super) fn toggle_mount_sidebar(&mut self, cx: &mut Cx, mount: &str) {
        let Some(current_width) = self.workspace_root_splitter_position(cx, mount) else {
            return;
        };
        let restore_width = self
            .mount_state(mount)
            .and_then(|state| state.sidebar_restore_width)
            .unwrap_or(310.0);

        if current_width <= 1.0 {
            self.start_sidebar_animation(cx, mount, restore_width);
        } else {
            self.mount_state_mut(mount).sidebar_restore_width = Some(current_width);
            self.start_sidebar_animation(cx, mount, 0.0);
        }
    }

    pub(super) fn apply_mount_file_tree_diff(
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

        if let Some(dock) = self.mount_workspace_dock(cx, &active_mount) {
            for tab_id in terminal_tabs {
                dock.item(tab_id).redraw(cx);
                dock.redraw_tab(cx, tab_id);
            }
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
        if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
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
        if let Some(dock) = self.mount_workspace_dock(cx, &mount) {
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
                .set_active(cx, log_tail);
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

    pub(super) fn collect_mount_terminal_files(&self, mount: &str) -> Vec<String> {
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

    pub(super) fn sync_mount_terminal_tabs(&mut self, cx: &mut Cx, mount: &str, select_last: bool) {
        let files = self
            .mount_state(mount)
            .map(|mount| mount.terminal_files.clone())
            .unwrap_or_default();
        let titles: HashMap<String, String> = files
            .iter()
            .map(|path| (path.clone(), self.terminal_tab_title(path)))
            .collect();

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
            // Create a new terminal tab before the "+" button.
            let Some((tab_bar, pos)) = Self::reachable_tab_bar_of_tab(&dock, id!(terminal_add))
            else {
                continue;
            };
            let tab_id = dock.unique_id(LiveId::from_str(path).0);
            if dock
                .create_tab(
                    cx,
                    tab_bar,
                    tab_id,
                    id!(TerminalPane),
                    title,
                    id!(TerminalCloseableTab),
                    Some(pos.saturating_sub(1)),
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
            dock.select_tab(cx, id!(bottom_terminal_tab));
        }
    }

    pub(super) fn select_bottom_terminal_panel(&mut self, cx: &mut Cx, mount: &str) {
        let Some(dock) = self.mount_workspace_dock(cx, mount) else {
            return;
        };
        dock.select_tab(cx, id!(bottom_terminal_tab));
    }

    pub(super) fn toggle_bottom_panel(&mut self, cx: &mut Cx, mount: &str) {
        let Some(current_height) = self.workspace_main_splitter_height(cx, mount) else {
            return;
        };
        let restore_height = self
            .mount_state(mount)
            .and_then(|state| state.bottom_panel_restore_height)
            .unwrap_or(220.0);

        if current_height <= 1.0 {
            self.start_bottom_panel_animation(cx, mount, restore_height);
        } else {
            self.mount_state_mut(mount).bottom_panel_restore_height = Some(current_height);
            self.start_bottom_panel_animation(cx, mount, 0.0);
        }
    }

    pub(super) fn ensure_terminal_session_open(&mut self, path: &str) {
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

    pub(super) fn ensure_mount_terminal_file(&mut self, cx: &mut Cx, mount: &str) {
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
        self.ensure_terminal_session_open(&path);
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
        self.data.terminal_framebuffer_by_path.remove(&path);
        let _ = self.send_studio(ClientToHub::TerminalClose { path: path.clone() });
        let _ = self.send_studio(ClientToHub::DeleteFile { path });
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
    }
}
