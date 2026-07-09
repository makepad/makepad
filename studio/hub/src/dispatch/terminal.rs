use super::*;
use makepad_studio_protocol::hub_protocol::{ClientId, HubToClient};
use makepad_terminal_core::Terminal;
use std::collections::HashMap;
use std::fs;
use std::time::{Duration, Instant};

impl HubCore {
    pub(super) fn on_terminal_output(&mut self, path: String, data: Vec<u8>) {
        if data.is_empty() {
            return;
        }
        let mount = match self.terminal_manager.mount_for_path(&path) {
            Some(mount) => mount.to_string(),
            None => return,
        };
        // Terminal history is persisted into .makepad/*.term and can trigger file
        // watcher churn. Suppress those self-induced fs events briefly so typing
        // in terminal does not force repeated file-tree reloads.
        self.mount_suppress_fs_until
            .insert(mount, Instant::now() + Duration::from_millis(750));
        let mut force_bottom_for_sticky = true;
        let mut bell_rang = false;
        if let Some(session) = self.terminal_sessions.get_mut(&path) {
            let old_total_rows = {
                let screen = session.terminal.screen();
                screen.scrollback_len() + screen.used_rows()
            };
            session.terminal.process_bytes(&data);
            bell_rang = session.terminal.take_bell();
            let outbound = session.terminal.take_outbound();
            if !outbound.is_empty() {
                let _ = self.terminal_manager.send_input(&path, outbound);
            }
            let new_total_rows = {
                let screen = session.terminal.screen();
                screen.scrollback_len() + screen.used_rows()
            };
            // Only auto-stick to bottom when output actually extends history.
            // TUI redraw bursts mostly rewrite in-place and should not force a
            // viewport jump during rapid resize sequences.
            force_bottom_for_sticky = new_total_rows > old_total_rows;
        }
        if bell_rang {
            self.set_terminal_bell_state(&path, true);
        }
        self.push_terminal_frame_updates(&path, force_bottom_for_sticky);
        // Persist terminal history off the dispatch thread so fs I/O cannot
        // block terminal framebuffer delivery.
        let history_vfs = self.vfs.clone_for_search();
        let history_path = path.clone();
        let history_data = data;
        self.io_worker_pool.execute(move || {
            let _ = append_terminal_history_bytes(&history_vfs, &history_path, &history_data);
        });
    }

    pub(super) fn on_terminal_resized(&mut self, path: String, cols: u16, rows: u16) {
        if let Some(session) = self.terminal_sessions.get_mut(&path) {
            let cols = cols.max(1);
            let rows = rows.max(1);
            if session.applied_cols == cols && session.applied_rows == rows {
                return;
            }
            session.applied_cols = cols;
            session.applied_rows = rows;
            session.terminal.resize(cols as usize, rows as usize);
            Self::adjust_terminal_subscribers_for_resize(session);
            if (cols != session.cols || rows != session.rows)
                && self
                    .terminal_manager
                    .resize(&path, session.cols, session.rows)
                    .is_err()
            {
                // Ignore retry errors here; primary resize request path reports
                // user-visible errors.
            };
            self.push_terminal_frame_updates(&path, false);
        }
    }

    pub(super) fn on_terminal_exited(&mut self, path: String, exit_code: i32) {
        let mount = self.terminal_manager.remove_terminal(&path);
        self.terminal_sessions.remove(&path);
        self.broadcast_ui_message(HubToClient::TerminalExited {
            path: path.clone(),
            code: exit_code,
        });
        let _ = mount;
    }

    pub(super) fn ensure_terminal_session_open(
        &mut self,
        path: &str,
        cols: u16,
        rows: u16,
        env: HashMap<String, String>,
    ) -> Result<bool, String> {
        if self.terminal_sessions.contains_key(path) {
            return Ok(false);
        }
        let Some(mount) = mount_from_virtual_path(path).map(ToOwned::to_owned) else {
            return Err(format!("invalid terminal path (missing mount): {}", path));
        };
        let cwd = self
            .vfs
            .resolve_mount(&mount)
            .map_err(|err| err.to_string())?;
        let history = self
            .vfs
            .resolve_path(path)
            .ok()
            .and_then(|disk_path| fs::read(disk_path).ok())
            .unwrap_or_default();
        self.terminal_manager.open_terminal(
            path.to_string(),
            mount,
            &cwd,
            cols,
            rows,
            env,
            self.event_tx.clone(),
        )?;
        let cols = cols.max(1);
        let rows = rows.max(1);
        let mut terminal = Terminal::new(cols as usize, rows as usize);
        if !history.is_empty() {
            terminal.process_bytes(&history);
            let _ = terminal.take_outbound();
        }
        self.terminal_sessions.insert(
            path.to_string(),
            TerminalSession {
                terminal,
                cols,
                rows,
                applied_cols: cols,
                applied_rows: rows,
                frame_seq: 0,
                bell_pending: false,
                subscribers: HashMap::new(),
            },
        );
        Ok(true)
    }

    #[allow(dead_code)]
    pub(super) fn is_terminal_path_taken(&self, path: &str) -> bool {
        self.terminal_sessions.contains_key(path)
            || self
                .vfs
                .resolve_path(path)
                .map(|disk_path| disk_path.exists())
                .unwrap_or(false)
    }

    pub(super) fn send_terminal_viewport_for_client(
        &mut self,
        client_id: ClientId,
        path: &str,
        cols: u16,
        rows: u16,
        pty_rows: u16,
        top_row: usize,
    ) {
        if !self.terminal_sessions.contains_key(path) {
            self.send_ui_error(client_id, format!("unknown terminal: {}", path));
            return;
        }
        let cols = cols.max(1);
        let rows = rows.max(1);
        let pty_rows = pty_rows.max(1);
        let mut resize_error = None;
        {
            let session = self
                .terminal_sessions
                .get_mut(path)
                .expect("session presence checked above");
            let needs_resize_request = cols != session.cols
                || pty_rows != session.rows
                || session.applied_cols != cols
                || session.applied_rows != pty_rows;
            session.cols = cols;
            session.rows = pty_rows;
            if needs_resize_request {
                if let Err(err) = self.terminal_manager.resize(path, cols, pty_rows) {
                    resize_error = Some(err);
                }
            }
            let max_top = Self::terminal_max_top_row(&session.terminal, rows);
            let (resolved_top, anchor) = if top_row == usize::MAX {
                (max_top, TerminalViewportAnchor::Bottom)
            } else {
                let clamped = top_row.min(max_top);
                let anchor = if clamped >= max_top.saturating_sub(1) {
                    TerminalViewportAnchor::Bottom
                } else {
                    TerminalViewportAnchor::TopRow
                };
                (clamped, anchor)
            };
            session.subscribers.insert(
                client_id,
                TerminalClientViewport {
                    cols,
                    rows,
                    top_row: resolved_top,
                    anchor,
                },
            );
        }
        if let Some(err) = resize_error {
            self.send_ui_error(client_id, err);
            return;
        }
        self.push_terminal_frame_updates(path, false);
    }

    pub(super) fn send_terminal_title_to_client(&mut self, client_id: ClientId, path: &str) {
        let Some(session) = self.terminal_sessions.get(path) else {
            return;
        };
        if !session.bell_pending {
            return;
        }
        self.send_ui_reply(
            client_id,
            HubToClient::TerminalTitle {
                path: path.to_string(),
                title: Self::terminal_tab_title(path, true),
            },
        );
    }

    pub(super) fn set_terminal_bell_state(&mut self, path: &str, bell_pending: bool) {
        let Some(session) = self.terminal_sessions.get_mut(path) else {
            return;
        };
        if session.bell_pending == bell_pending {
            return;
        }
        session.bell_pending = bell_pending;
        self.broadcast_ui_message(HubToClient::TerminalTitle {
            path: path.to_string(),
            title: Self::terminal_tab_title(path, bell_pending),
        });
    }

    pub(super) fn push_terminal_frame_updates(
        &mut self,
        path: &str,
        force_bottom_for_sticky: bool,
    ) {
        let updates = {
            let Some(session) = self.terminal_sessions.get_mut(path) else {
                return;
            };
            for viewport in session.subscribers.values_mut() {
                let max_top = Self::terminal_max_top_row(&session.terminal, viewport.rows);
                if viewport.anchor == TerminalViewportAnchor::Bottom && force_bottom_for_sticky {
                    viewport.top_row = max_top;
                }
                viewport.top_row = viewport.top_row.min(max_top);
            }

            let subscribers: Vec<(ClientId, TerminalClientViewport)> = session
                .subscribers
                .iter()
                .map(|(client_id, viewport)| (*client_id, viewport.clone()))
                .collect();
            let mut updates = Vec::with_capacity(subscribers.len());
            for (client_id, frame) in subscribers {
                session.frame_seq = session.frame_seq.wrapping_add(1);
                let frame = terminal_framebuffer_from_terminal(
                    &session.terminal,
                    frame.cols,
                    frame.rows,
                    frame.top_row,
                    session.frame_seq,
                );
                updates.push((client_id, frame));
            }
            updates
        };

        let path = path.to_string();
        for (client_id, frame) in updates {
            self.send_ui_reply(
                client_id,
                HubToClient::TerminalFramebuffer {
                    path: path.clone(),
                    frame,
                },
            );
        }
    }

    pub(super) fn adjust_terminal_subscribers_for_resize(session: &mut TerminalSession) {
        for viewport in session.subscribers.values_mut() {
            viewport.cols = session.cols;
            let max_top = Self::terminal_max_top_row(&session.terminal, viewport.rows);
            if viewport.anchor == TerminalViewportAnchor::Bottom {
                viewport.top_row = max_top;
            }
            viewport.top_row = viewport.top_row.min(max_top);
        }
    }

    pub(super) fn terminal_max_top_row(terminal: &Terminal, rows: u16) -> usize {
        let screen = terminal.screen();
        let is_tui = screen.scroll_top != 0
            || screen.scroll_bottom != screen.rows()
            || terminal.modes.alt_screen;
        let total_lines = if is_tui {
            screen.scrollback_len() + screen.rows()
        } else {
            screen.scrollback_len() + screen.used_rows()
        };
        total_lines.saturating_sub(rows.max(1) as usize)
    }

    pub(super) fn terminal_tab_title(path: &str, bell_pending: bool) -> String {
        let title = path.rsplit('/').next().unwrap_or("terminal");
        if bell_pending {
            format!("@ {}", title)
        } else {
            title.to_string()
        }
    }
}
