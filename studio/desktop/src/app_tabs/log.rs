use super::*;

impl App {
    pub(crate) fn ensure_log_tab_for_build(
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
            &dock,
            id!(log_first),
            self.data
                .log_tab_state
                .iter()
                .map(|(id, s)| (id, s.mount.as_str())),
            mount,
        )?;
        let tab_id = dock.unique_id(LiveId::from_str(&format!("log/{}/{}", mount, build_id.0)).0);
        Self::create_dock_tab(
            &dock,
            cx,
            anchor,
            tab_id,
            id!(LogPane),
            title.to_string(),
            select,
        )?;

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

    pub(crate) fn close_log_tab(&mut self, cx: &mut Cx, tab_id: LiveId) {
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

    pub(crate) fn handle_log_view_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            let Some(widget_action) = action.as_widget_action() else {
                continue;
            };
            let Some(log_action) = widget_action.action.downcast_ref::<DesktopLogViewAction>()
            else {
                continue;
            };
            match log_action {
                DesktopLogViewAction::OpenLocation { path, line, column } => {
                    self.open_log_location(cx, path, *line, *column);
                }
                DesktopLogViewAction::None => {}
            }
        }
    }

    pub(crate) fn log_jump_position(session: &CodeSession, line: usize, column: usize) -> Position {
        let text = session.document().as_text();
        let lines = text.as_lines();
        let line_index = line.saturating_sub(1).min(lines.len().saturating_sub(1));
        let byte_index = lines
            .get(line_index)
            .map(|line_text| {
                line_text
                    .char_indices()
                    .nth(column.saturating_sub(1))
                    .map(|(byte_index, _)| byte_index)
                    .unwrap_or_else(|| line_text.len())
            })
            .unwrap_or(0);
        Position {
            line_index,
            byte_index,
        }
    }

    pub(crate) fn try_apply_log_jump(
        &mut self,
        cx: &mut Cx,
        tab_id: LiveId,
        line: usize,
        column: usize,
    ) -> bool {
        let Some(path) = self.data.tab_to_path.get(&tab_id).cloned() else {
            return false;
        };
        let Some(mount) = Self::mount_from_virtual_path(&path) else {
            return false;
        };
        let Some(dock) = self.mount_workspace_dock(cx, mount) else {
            return false;
        };
        let editor = dock.item(tab_id).desktop_code_editor(cx, ids!(code_editor));
        let Some(session) = self.data.sessions.get_mut(&tab_id) else {
            return false;
        };
        let pos = Self::log_jump_position(session, line, column);
        if !editor.set_cursor_and_scroll(cx, pos, session) {
            return false;
        }
        dock.redraw_tab(cx, tab_id);
        true
    }

    pub(crate) fn apply_pending_log_jump(&mut self, cx: &mut Cx, path: &str, tab_id: LiveId) {
        let Some((line, column)) = self.data.pending_log_jumps.remove(path) else {
            return;
        };
        if !self.try_apply_log_jump(cx, tab_id, line, column) {
            self.data
                .pending_log_jumps
                .insert(path.to_string(), (line, column));
        };
    }

    pub(crate) fn open_log_location(
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

        if self.try_apply_log_jump(cx, tab_id, line, column) {
            self.set_status(cx, &format!("opened {}:{}:{}", path, line, column));
            return;
        }

        self.data
            .pending_log_jumps
            .insert(path.to_string(), (line, column));
        if !self.data.pending_open_paths.contains(path) {
            self.data.pending_open_paths.insert(path.to_string());
            let _ = self.send_studio(ClientToHub::OpenTextFile {
                path: path.to_string(),
            });
        }
        self.set_status(cx, &format!("opening {}:{}:{}", path, line, column));
    }

    pub(crate) fn extract_log_location(
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

    pub(crate) fn virtualize_log_path(&self, mount: &str, raw_path: &str) -> Option<String> {
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

    pub(crate) fn absolute_to_virtual_path(&self, mount: &str, abs_path: &Path) -> Option<String> {
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
}
