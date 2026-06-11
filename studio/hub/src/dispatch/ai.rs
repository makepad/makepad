use super::*;
use crate::ai_manager::{AiTerminalObservation, AiToolExecutionResult};
use makepad_studio_protocol::hub_protocol::{ClientId, HubToClient, QueryId};
use makepad_terminal_core::TermKeyCode;
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::Instant;

impl HubCore {
    pub(super) fn on_ai_open_terminal_request(
        &mut self,
        mount: String,
        name: Option<String>,
        command: Option<String>,
        cols: u16,
        rows: u16,
        reply_tx: Sender<Result<String, String>>,
    ) {
        let result = self.open_ai_terminal(&mount, name.as_deref(), command.as_deref(), cols, rows);
        let _ = reply_tx.send(result);
    }

    pub(super) fn on_ai_open_editor_request(
        &mut self,
        mount: String,
        path: String,
        line: Option<usize>,
        column: Option<usize>,
        reply_tx: Sender<Result<String, String>>,
    ) {
        let result = self.open_ai_editor(&mount, &path, line, column);
        let _ = reply_tx.send(result);
    }

    pub(super) fn on_ai_observe_filesystem_request(
        &mut self,
        mount: String,
        path: Option<String>,
        limit: usize,
        since_secs: u64,
        reply_tx: Sender<Result<String, String>>,
    ) {
        let result = self.observe_ai_filesystem(&mount, path.as_deref(), limit, since_secs);
        let _ = reply_tx.send(result);
    }

    pub(super) fn open_ai_terminal(
        &mut self,
        mount: &str,
        name: Option<&str>,
        command: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> Result<String, String> {
        let path = self.next_ai_terminal_path(mount, name, command)?;
        self.vfs
            .save_text_file(&path, "")
            .map_err(|err| err.to_string())?;
        self.self_save_suppress_until_by_path
            .insert(path.clone(), Instant::now() + FS_SELF_SAVE_SUPPRESS);
        let _ = self.ensure_terminal_session_open(&path, cols, rows, HashMap::new())?;
        self.broadcast_ui_message(HubToClient::TerminalOpened { path: path.clone() });
        if let Some(command) = command.map(str::trim).filter(|command| !command.is_empty()) {
            self.terminal_manager
                .send_input(&path, format!("{}\n", command).into_bytes())?;
        }
        self.process_ai_terminal_observation_for_path(&path);
        Ok(self.ai_terminal_info(&path)?.serialize_json())
    }

    pub(super) fn open_ai_editor(
        &mut self,
        mount: &str,
        path: &str,
        line: Option<usize>,
        column: Option<usize>,
    ) -> Result<String, String> {
        if mount_from_virtual_path(path) != Some(mount) {
            return Err(format!(
                "editor path '{}' does not belong to mount '{}'",
                path, mount
            ));
        }
        let Some(client_id) = self.primary_ui_for_mount(mount) else {
            return Err(format!(
                "no primary Studio UI is observing mount '{}'",
                mount
            ));
        };
        let content = self
            .vfs
            .open_text_file(path)
            .map_err(|err| err.to_string())?;
        let line = line.map(|value| value.max(1));
        let column = column.map(|value| value.max(1));
        self.send_ui_reply(
            client_id,
            HubToClient::TextFileOpened {
                path: path.to_string(),
                content,
                git_status: backend_proto::GitStatus::Unknown,
                line,
                column,
            },
        );
        if let Some(line) = line {
            Ok(format!(
                "Opened {} at {}:{} in Studio editor.",
                path,
                line,
                column.unwrap_or(1)
            ))
        } else {
            Ok(format!("Opened {} in Studio editor.", path))
        }
    }

    pub(super) fn observe_ai_filesystem(
        &mut self,
        mount: &str,
        path_filter: Option<&str>,
        limit: usize,
        since_secs: u64,
    ) -> Result<String, String> {
        let now = Instant::now();
        self.prune_fs_event_history(now);

        let normalized_filter = path_filter
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != ".")
            .map(|value| value.trim_matches('/').to_string());
        let since = Duration::from_secs(since_secs.max(1));
        let mount_prefix = format!("{}/", mount);

        let mut changes = self
            .fs_recent_change_at_by_path
            .iter()
            .filter_map(|(virtual_path, observed_at)| {
                let age = now.saturating_duration_since(*observed_at);
                if age > since {
                    return None;
                }
                let (relative_path, kind) = if virtual_path == mount {
                    (".".to_string(), "mount".to_string())
                } else if let Some(rest) = virtual_path.strip_prefix(&mount_prefix) {
                    (rest.to_string(), "path".to_string())
                } else {
                    return None;
                };
                if let Some(filter) = normalized_filter.as_deref() {
                    if relative_path == "." {
                        return None;
                    }
                    let exact = relative_path == filter;
                    let within = relative_path.starts_with(&format!("{}/", filter));
                    if !exact && !within {
                        return None;
                    }
                }
                Some((
                    *observed_at,
                    AiFilesystemChange {
                        path: relative_path,
                        kind,
                        seconds_ago: age.as_secs_f64(),
                    },
                ))
            })
            .collect::<Vec<_>>();
        changes.sort_by(|a, b| b.0.cmp(&a.0));

        Ok(AiFilesystemObserveResult {
            mount: mount.to_string(),
            path_filter: normalized_filter,
            since_secs,
            changes: changes
                .into_iter()
                .take(limit.max(1))
                .map(|(_, change)| change)
                .collect(),
        }
        .serialize_json())
    }

    pub(super) fn on_ai_list_terminals_request(
        &mut self,
        mount: String,
        reply_tx: Sender<Result<String, String>>,
    ) {
        let result = self.list_ai_terminals(&mount);
        let _ = reply_tx.send(result);
    }

    pub(super) fn on_ai_read_terminal_request(
        &mut self,
        mount: String,
        path: String,
        rows: Option<u16>,
        top_row: Option<usize>,
        reply_tx: Sender<Result<String, String>>,
    ) {
        let result = self.read_ai_terminal(&mount, &path, rows, top_row);
        let _ = reply_tx.send(result);
    }

    pub(super) fn on_ai_send_terminal_text_request(
        &mut self,
        mount: String,
        path: String,
        text: String,
        submit: Option<bool>,
        bracketed_paste: Option<bool>,
        reply_tx: Sender<Result<String, String>>,
    ) {
        let result = self.send_ai_terminal_text(&mount, &path, &text, submit, bracketed_paste);
        let _ = reply_tx.send(result);
    }

    pub(super) fn on_ai_send_terminal_key_request(
        &mut self,
        mount: String,
        path: String,
        key: String,
        shift: bool,
        control: bool,
        alt: bool,
        reply_tx: Sender<Result<String, String>>,
    ) {
        let result = self.send_ai_terminal_key(&mount, &path, &key, shift, control, alt);
        let _ = reply_tx.send(result);
    }

    pub(super) fn list_ai_terminals(&self, mount: &str) -> Result<String, String> {
        let mut terminals = self
            .terminal_sessions
            .iter()
            .filter(|(path, _)| self.terminal_manager.mount_for_path(path.as_str()) == Some(mount))
            .map(|(path, _)| self.ai_terminal_info(path))
            .collect::<Result<Vec<_>, String>>()?;
        terminals.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(terminals.serialize_json())
    }

    pub(super) fn ai_terminal_observation(
        &self,
        path: &str,
        rows: Option<u16>,
        top_row: Option<usize>,
    ) -> Result<AiTerminalObservation, String> {
        let session = self
            .terminal_sessions
            .get(path)
            .ok_or_else(|| format!("unknown terminal: {}", path))?;
        let frame = terminal_framebuffer_from_terminal(
            &session.terminal,
            session.cols.max(1),
            rows.unwrap_or(session.rows).max(1),
            top_row.unwrap_or(usize::MAX),
            session.frame_seq,
        );
        Ok(AiTerminalObservation {
            path: path.to_string(),
            terminal_title: session.terminal.title.clone(),
            cols: frame.cols,
            rows: frame.rows,
            top_row: frame.top_row,
            total_lines: frame.total_lines,
            is_tui: frame.is_tui,
            text: terminal_framebuffer_text(&frame),
        })
    }

    pub(super) fn process_ai_terminal_observation_for_path(&mut self, path: &str) {
        let Some(mount) = self
            .terminal_manager
            .mount_for_path(path)
            .map(str::to_string)
        else {
            return;
        };
        let Ok(observation) = self.ai_terminal_observation(path, None, Some(usize::MAX)) else {
            return;
        };
        if let Some(state) = self
            .ai_manager
            .process_terminal_observation(&mount, observation)
        {
            self.broadcast_ui_message(HubToClient::AiMountState { mount, state });
        }
    }

    pub(super) fn process_ai_terminal_input_for_path(&mut self, path: &str) {
        let Some(mount) = self
            .terminal_manager
            .mount_for_path(path)
            .map(str::to_string)
        else {
            return;
        };
        if let Some(state) = self.ai_manager.process_terminal_input(&mount, path) {
            self.broadcast_ui_message(HubToClient::AiMountState { mount, state });
        }
    }

    pub(super) fn process_ai_path_change(&mut self, mount: &str, virtual_path: &str) {
        if let Some(state) = self.ai_manager.process_path_change(mount, virtual_path) {
            self.broadcast_ui_message(HubToClient::AiMountState {
                mount: mount.to_string(),
                state,
            });
        }
    }

    pub(super) fn read_ai_terminal(
        &self,
        mount: &str,
        path: &str,
        rows: Option<u16>,
        top_row: Option<usize>,
    ) -> Result<String, String> {
        self.ensure_ai_terminal_access(mount, path)?;
        let observation = self.ai_terminal_observation(path, rows, top_row)?;
        let (mode, is_codex, summary, codex_status) =
            AiManager::terminal_mode_and_summary(&observation.terminal_title, &observation.text);
        let session = self
            .terminal_sessions
            .get(path)
            .ok_or_else(|| format!("unknown terminal: {}", path))?;
        let frame = terminal_framebuffer_from_terminal(
            &session.terminal,
            observation.cols,
            observation.rows,
            observation.top_row,
            session.frame_seq,
        );
        Ok(AiTerminalReadResult {
            path: path.to_string(),
            name: Self::terminal_display_name(path),
            terminal_title: observation.terminal_title,
            cols: observation.cols,
            rows: observation.rows,
            top_row: observation.top_row,
            total_lines: observation.total_lines,
            cursor_col: frame.cursor_col,
            cursor_row: frame.cursor_row,
            cursor_visible: frame.cursor_visible,
            is_tui: observation.is_tui,
            mode: mode.to_string(),
            summary,
            is_codex,
            codex_status,
            bracketed_paste: frame.bracketed_paste,
            cursor_keys_application_mode: frame.cursor_keys_application_mode,
            text: observation.text,
        }
        .serialize_json())
    }

    pub(super) fn send_ai_terminal_text(
        &mut self,
        mount: &str,
        path: &str,
        text: &str,
        submit: Option<bool>,
        bracketed_paste: Option<bool>,
    ) -> Result<String, String> {
        self.ensure_ai_terminal_access(mount, path)?;
        let (bracketed_paste, submit, submit_bytes) = {
            let session = self
                .terminal_sessions
                .get(path)
                .ok_or_else(|| format!("unknown terminal: {}", path))?;
            let visible_text = {
                let frame = terminal_framebuffer_from_terminal(
                    &session.terminal,
                    session.cols.max(1),
                    session.rows.max(1),
                    usize::MAX,
                    0,
                );
                terminal_framebuffer_text(&frame)
            };
            let bracketed_paste = bracketed_paste
                .unwrap_or(session.terminal.modes.bracketed_paste && text.contains('\n'));
            let submit = submit.unwrap_or(false)
                || Self::terminal_auto_submit_ai_text(
                    path,
                    &session.terminal.title,
                    &visible_text,
                    text,
                );
            let submit_bytes = if submit {
                session
                    .terminal
                    .encode_key(TermKeyCode::Return, "", false, false, false)
                    .or_else(|| Some(vec![b'\n']))
            } else {
                None
            };
            (bracketed_paste, submit, submit_bytes)
        };
        if text.is_empty() && !submit {
            return Err("send_terminal_text requires non-empty text or submit=true".to_string());
        }
        let mut bytes = Vec::with_capacity(text.len() + 16);
        if bracketed_paste {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend_from_slice(text.as_bytes());
        if bracketed_paste {
            bytes.extend_from_slice(b"\x1b[201~");
        }
        if !bytes.is_empty() {
            self.terminal_manager.send_input(path, bytes.clone())?;
        }
        let submit_len = if let Some(submit_bytes) = submit_bytes {
            let len = submit_bytes.len();
            if bytes.is_empty() {
                self.terminal_manager.send_input(path, submit_bytes)?;
            } else {
                self.terminal_manager.send_input_delayed(
                    path,
                    submit_bytes,
                    AI_TERMINAL_SUBMIT_DELAY,
                )?;
            }
            len
        } else {
            0
        };
        self.set_terminal_bell_state(path, false);
        self.process_ai_terminal_input_for_path(path);
        let preview_source = if submit {
            format!("{}<enter>", text)
        } else {
            text.to_string()
        };
        Ok(AiTerminalInputResult {
            path: path.to_string(),
            name: Self::terminal_display_name(path),
            bytes_sent: bytes.len() + submit_len,
            submitted: submit,
            bracketed_paste,
            preview: preview_text(&preview_source),
        }
        .serialize_json())
    }

    pub(super) fn send_ai_terminal_key(
        &mut self,
        mount: &str,
        path: &str,
        key: &str,
        shift: bool,
        control: bool,
        alt: bool,
    ) -> Result<String, String> {
        self.ensure_ai_terminal_access(mount, path)?;
        let spec = parse_ai_terminal_key_spec(key, shift, control, alt)?;
        let bytes = {
            let session = self
                .terminal_sessions
                .get(path)
                .ok_or_else(|| format!("unknown terminal: {}", path))?;
            encode_ai_terminal_key(&session.terminal, &spec)
                .ok_or_else(|| format!("unsupported terminal key '{}'", key))?
        };
        self.terminal_manager.send_input(path, bytes.clone())?;
        self.set_terminal_bell_state(path, false);
        self.process_ai_terminal_input_for_path(path);
        Ok(AiTerminalInputResult {
            path: path.to_string(),
            name: Self::terminal_display_name(path),
            bytes_sent: bytes.len(),
            submitted: false,
            bracketed_paste: false,
            preview: preview_text(key),
        }
        .serialize_json())
    }

    pub(super) fn ai_terminal_info(&self, path: &str) -> Result<AiTerminalInfo, String> {
        let session = self
            .terminal_sessions
            .get(path)
            .ok_or_else(|| format!("unknown terminal: {}", path))?;
        let observation = self.ai_terminal_observation(path, None, Some(usize::MAX))?;
        let (mode, is_codex, summary, codex_status) =
            AiManager::terminal_mode_and_summary(&observation.terminal_title, &observation.text);
        Ok(AiTerminalInfo {
            path: path.to_string(),
            name: Self::terminal_display_name(path),
            terminal_title: session.terminal.title.clone(),
            mode: mode.to_string(),
            summary,
            is_codex,
            codex_status,
            cols: session.cols,
            rows: session.rows,
            is_tui: session.terminal.modes.alt_screen
                || session.terminal.screen().scroll_top != 0
                || session.terminal.screen().scroll_bottom != session.terminal.screen().rows(),
            bracketed_paste: session.terminal.modes.bracketed_paste,
            cursor_keys_application_mode: session.terminal.modes.cursor_keys,
            bell_pending: session.bell_pending,
        })
    }

    pub(super) fn ensure_ai_terminal_access(&self, mount: &str, path: &str) -> Result<(), String> {
        match self.terminal_manager.mount_for_path(path) {
            Some(actual_mount) if actual_mount == mount => Ok(()),
            Some(actual_mount) => Err(format!(
                "terminal '{}' belongs to mount '{}', not '{}'",
                path, actual_mount, mount
            )),
            None => Err(format!("unknown terminal: {}", path)),
        }
    }

    pub(super) fn terminal_display_name(path: &str) -> String {
        path.rsplit('/').next().unwrap_or(path).to_string()
    }

    pub(super) fn terminal_auto_submit_ai_text(
        path: &str,
        terminal_title: &str,
        visible_text: &str,
        text: &str,
    ) -> bool {
        if text.trim().is_empty() || text.ends_with('\n') || text.ends_with('\r') {
            return false;
        }
        let haystack = format!("{}\n{}\n{}", path, terminal_title, visible_text).to_lowercase();
        haystack.contains("codex")
            || haystack.contains("claude")
            || haystack.contains("aider")
            || haystack.contains("enter a prompt")
            || haystack.contains("esc to interrupt")
    }

    pub(super) fn next_ai_terminal_path(
        &self,
        mount: &str,
        name: Option<&str>,
        command: Option<&str>,
    ) -> Result<String, String> {
        self.vfs
            .resolve_mount(mount)
            .map_err(|err| err.to_string())?;
        if let Some(stem) = name
            .and_then(sanitize_terminal_stem)
            .or_else(|| command.and_then(terminal_stem_from_command))
        {
            return self.unique_ai_terminal_path(mount, &stem);
        }
        for index in 0usize.. {
            let stem = if index < 26 {
                ((b'a' + index as u8) as char).to_string()
            } else {
                format!("t{}", index + 1)
            };
            let path = format!("{}/.makepad/{}.term", mount, stem);
            if !self.is_terminal_path_taken(&path) {
                return Ok(path);
            }
        }
        Err("failed to allocate terminal path".to_string())
    }

    pub(super) fn unique_ai_terminal_path(
        &self,
        mount: &str,
        stem: &str,
    ) -> Result<String, String> {
        for index in 0usize.. {
            let file_name = if index == 0 {
                format!("{}.term", stem)
            } else {
                format!("{}-{}.term", stem, index + 1)
            };
            let path = format!("{}/.makepad/{}", mount, file_name);
            if !self.is_terminal_path_taken(&path) {
                return Ok(path);
            }
        }
        Err("failed to allocate named terminal path".to_string())
    }
}
