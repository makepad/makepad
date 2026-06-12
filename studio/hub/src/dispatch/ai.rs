use super::*;
use makepad_studio_protocol::hub_protocol::HubToClient;
use std::sync::mpsc::Sender;

impl HubCore {
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

    pub(super) fn reveal_ai_touched_file(&mut self, mount: &str, path: &str) {
        let _ = self.open_ai_editor(mount, path, None, None);
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

    pub(super) fn process_ai_path_change(&mut self, mount: &str, virtual_path: &str) {
        if let Some(state) = self.ai_manager.process_path_change(mount, virtual_path) {
            self.broadcast_ui_message(HubToClient::AiMountState {
                mount: mount.to_string(),
                state,
            });
        }
    }
}
