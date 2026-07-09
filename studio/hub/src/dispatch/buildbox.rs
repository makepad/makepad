use super::*;
use makepad_studio_protocol::hub_protocol::{
    BuildBoxInfo, BuildBoxStatus, BuildBoxToHub, BuildBoxToHubVec, HubToClient, QueryId,
};
use makepad_studio_protocol::LogLevel;

impl HubCore {
    pub(super) fn set_buildbox_status(&mut self, name: &str, status: BuildBoxStatus) {
        let Some(web_socket_id) = self.buildbox_by_name.get(name).copied() else {
            return;
        };
        let Some(socket) = self.buildbox_sockets.get_mut(&web_socket_id) else {
            return;
        };
        if let Some(info) = socket.info.as_mut() {
            info.status = status;
        }
        self.broadcast_ui_message(HubToClient::BuildBoxes {
            boxes: self.list_buildboxes(),
        });
    }

    pub(super) fn on_buildbox_disconnected(&mut self, web_socket_id: u64) {
        let Some(socket) = self.buildbox_sockets.remove(&web_socket_id) else {
            return;
        };
        let Some(info) = socket.info else {
            return;
        };

        self.buildbox_by_name.remove(&info.name);
        self.broadcast_ui_message(HubToClient::BuildBoxDisconnected {
            name: info.name.clone(),
        });

        let affected_build_ids: Vec<QueryId> = self
            .remote_build_owner
            .iter()
            .filter_map(|(build_id, owner)| (owner == &info.name).then_some(*build_id))
            .collect();
        for build_id in affected_build_ids {
            self.remote_build_owner.remove(&build_id);
            self.remote_builds.remove(&build_id);
            self.build_mount_by_id.remove(&build_id);
            self.broadcast_ui_message(HubToClient::BuildStopped {
                build_id,
                exit_code: None,
            });
        }

        self.broadcast_ui_message(HubToClient::BuildBoxes {
            boxes: self.list_buildboxes(),
        });
    }

    pub(super) fn on_buildbox_binary(&mut self, web_socket_id: u64, data: Vec<u8>) {
        let messages = match BuildBoxToHubVec::deserialize_bin(&data) {
            Ok(messages) => messages.0,
            Err(err) => {
                let (index, entry) = self.log_store.append(AppendLogEntry {
                    build_id: None,
                    level: LogLevel::Warning,
                    source: LogSource::BuildBox,
                    message: format!("failed to decode buildbox message: {}", err.msg),
                    file_name: None,
                    line: None,
                    column: None,
                    timestamp: None,
                });
                self.broadcast_live_log_entry(index, entry);
                return;
            }
        };

        for msg in messages {
            self.handle_buildbox_message(web_socket_id, msg);
        }
    }

    pub(super) fn handle_buildbox_message(&mut self, web_socket_id: u64, msg: BuildBoxToHub) {
        match msg {
            BuildBoxToHub::Hello {
                name,
                platform,
                arch,
                tree_hash,
            } => {
                let info = BuildBoxInfo {
                    name: name.clone(),
                    platform,
                    arch,
                    status: BuildBoxStatus::Idle,
                };
                if let Some(socket) = self.buildbox_sockets.get_mut(&web_socket_id) {
                    socket.info = Some(info.clone());
                    socket.tree_hash = Some(tree_hash);
                }
                self.buildbox_by_name.insert(name.clone(), web_socket_id);
                self.broadcast_ui_message(HubToClient::BuildBoxConnected { info });
                self.broadcast_ui_message(HubToClient::BuildBoxes {
                    boxes: self.list_buildboxes(),
                });
            }
            BuildBoxToHub::BuildOutput { build_id, line } => {
                let (index, entry) = self.log_store.append(AppendLogEntry {
                    build_id: Some(build_id),
                    level: LogLevel::Log,
                    source: LogSource::BuildBox,
                    message: line,
                    file_name: None,
                    line: None,
                    column: None,
                    timestamp: None,
                });
                self.broadcast_live_log_entry(index, entry);
            }
            BuildBoxToHub::BuildStarted { build_id } => {
                if let Some(buildbox_name) = self.remote_build_owner.get(&build_id).cloned() {
                    self.set_buildbox_status(&buildbox_name, BuildBoxStatus::Building { build_id });
                }
            }
            BuildBoxToHub::BuildStopped {
                build_id,
                exit_code,
            } => {
                if let Some(buildbox_name) = self.remote_build_owner.remove(&build_id) {
                    self.remote_builds.remove(&build_id);
                    self.set_buildbox_status(&buildbox_name, BuildBoxStatus::Idle);
                }
                self.build_mount_by_id.remove(&build_id);
                self.broadcast_ui_message(HubToClient::BuildStopped {
                    build_id,
                    exit_code,
                });
            }
            BuildBoxToHub::SyncComplete { tree_hash } => {
                if let Some(socket) = self.buildbox_sockets.get_mut(&web_socket_id) {
                    socket.tree_hash = Some(tree_hash);
                    if let Some(info) = socket.info.as_mut() {
                        info.status = BuildBoxStatus::Idle;
                    }
                }
                self.broadcast_ui_message(HubToClient::BuildBoxes {
                    boxes: self.list_buildboxes(),
                });
            }
            BuildBoxToHub::SyncError { error } => {
                let (index, entry) = self.log_store.append(AppendLogEntry {
                    build_id: None,
                    level: LogLevel::Warning,
                    source: LogSource::BuildBox,
                    message: format!("buildbox sync error: {}", error),
                    file_name: None,
                    line: None,
                    column: None,
                    timestamp: None,
                });
                self.broadcast_live_log_entry(index, entry);
            }
            BuildBoxToHub::Pong => {}
            BuildBoxToHub::FileHashes { .. } => {}
        }
    }
}
