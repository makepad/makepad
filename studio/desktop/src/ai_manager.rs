use crate::{makepad_widgets::*, App};
use makepad_studio_ai::*;
use makepad_studio_protocol::hub_protocol::{AiAgentId, AiMountState, ClientToHub};

impl App {
    pub(super) fn init_ai_manager(&mut self, cx: &mut Cx) {
        for mount in self.data.mounts.keys().cloned().collect::<Vec<_>>() {
            let _ = self.send_studio(ClientToHub::AiGetState { mount });
        }
        self.sync_ai_manager_widgets(cx);
    }

    pub(super) fn receive_ai_state(&mut self, cx: &mut Cx, mount: &str, state: AiMountState) {
        let should_scroll = state
            .active_agent
            .as_ref()
            .map(|agent| !agent.messages.is_empty())
            .unwrap_or(false);
        self.mount_state_mut(mount).ai_state = Some(state);
        if self.data.active_mount.as_deref() == Some(mount) {
            self.sync_ai_manager_widgets(cx);
            if should_scroll {
                self.schedule_ai_chat_scroll_to_bottom(cx);
            }
        }
    }

    pub(super) fn refresh_ai_manager_report(&mut self, cx: &mut Cx) {
        self.sync_ai_manager_widgets(cx);
    }

    pub(super) fn refresh_ai_manager_preview(&mut self, cx: &mut Cx) {
        self.sync_ai_manager_widgets(cx);
    }

    pub(super) fn request_ai_mount_state(&mut self, mount: &str) {
        let _ = self.send_studio(ClientToHub::AiGetState {
            mount: mount.to_string(),
        });
    }

    pub(super) fn create_ai_manager_agent(&mut self, mount: &str) {
        let _ = self.send_studio(ClientToHub::AiCreateAgent {
            mount: mount.to_string(),
            title: None,
        });
    }

    pub(super) fn delete_ai_manager_agent(&mut self, mount: &str) {
        let Some(agent_id) = self
            .mount_state(mount)
            .and_then(|state| state.ai_state.as_ref())
            .and_then(|state| state.active_agent_id)
        else {
            return;
        };
        let _ = self.send_studio(ClientToHub::AiDeleteAgent {
            mount: mount.to_string(),
            agent_id,
        });
    }

    pub(super) fn select_ai_manager_agent(&mut self, mount: &str, index: usize) {
        let Some(agent_id) = self
            .mount_state(mount)
            .and_then(|state| state.ai_state.as_ref())
            .and_then(|state| state.agents.get(index))
            .map(|agent| agent.agent_id)
        else {
            return;
        };
        let _ = self.send_studio(ClientToHub::AiSelectAgent {
            mount: mount.to_string(),
            agent_id,
        });
    }

    pub(super) fn select_ai_manager_backend(&mut self, mount: &str, index: usize) {
        let Some(backend_id) = self
            .mount_state(mount)
            .and_then(|state| state.ai_state.as_ref())
            .and_then(|state| state.backends.get(index))
            .map(|backend| backend.id.clone())
        else {
            return;
        };
        let _ = self.send_studio(ClientToHub::AiSetBackend {
            mount: mount.to_string(),
            backend_id,
        });
    }

    pub(super) fn configure_ai_manager_backend(&mut self, cx: &mut Cx, mount: &str) {
        let Some((backend_id, configuration_url)) = self
            .mount_state(mount)
            .and_then(|state| state.ai_state.as_ref())
            .and_then(|state| {
                let backend_id = state
                    .active_backend_id
                    .clone()
                    .or_else(|| state.backends.first().map(|backend| backend.id.clone()))?;
                let configuration_url = state
                    .backends
                    .iter()
                    .find(|backend| backend.id == backend_id)
                    .and_then(|backend| backend.configuration_url.clone());
                Some((backend_id, configuration_url))
            })
        else {
            return;
        };
        let _ = self.send_studio(ClientToHub::AiConfigureBackend {
            mount: mount.to_string(),
            backend_id,
        });
        if let Some(url) = configuration_url {
            cx.open_url(&url, OpenUrlInPlace::No);
        }
    }

    pub(super) fn send_ai_manager_prompt(&mut self, cx: &mut Cx, mount: &str) {
        let Some(workspace) = self.mount_workspace_widget(cx, mount) else {
            return;
        };
        let input = workspace.text_input(cx, ids!(ai_prompt_input));
        let prompt = input.text().trim().to_string();
        if prompt.is_empty() {
            return;
        }
        let Some(agent_id) = self
            .mount_state(mount)
            .and_then(|state| state.ai_state.as_ref())
            .and_then(|state| {
                state
                    .active_agent
                    .as_ref()
                    .map(|agent| (agent.agent_id, agent.pending))
                    .or_else(|| state.active_agent_id.map(|agent_id| (agent_id, false)))
            })
        else {
            return;
        };
        if agent_id.1 {
            return;
        }
        let agent_id = agent_id.0;
        if prompt.starts_with('/') {
            if self.send_ai_prompt_to_agent(cx, mount, agent_id, &prompt, Some(&prompt)) {
                input.set_text(cx, "");
            }
        } else {
            let role_index = workspace
                .drop_down(cx, ids!(ai_subagent_role_picker))
                .selected_item();
            let role = AI_SUBAGENT_ROLES
                .get(role_index)
                .copied()
                .unwrap_or("coder");
            if self.spawn_ai_subagent(cx, mount, agent_id, role, &prompt) {
                input.set_text(cx, "");
            }
        }
    }

    pub(super) fn cancel_ai_manager_prompt(&mut self, mount: &str) {
        let Some(agent_id) = self
            .mount_state(mount)
            .and_then(|state| state.ai_state.as_ref())
            .and_then(|state| state.active_agent_id)
        else {
            return;
        };
        let _ = self.send_studio(ClientToHub::AiCancelPrompt {
            mount: mount.to_string(),
            agent_id,
        });
    }

    pub(super) fn active_ai_agent_is_pending(&self) -> bool {
        let Some(active_mount) = self.data.active_mount.as_deref() else {
            return false;
        };
        self.active_ai_agent_is_pending_for_mount(active_mount)
    }

    pub(super) fn active_ai_agent_is_pending_for_mount(&self, mount: &str) -> bool {
        self.mount_state(mount)
            .and_then(|state| state.ai_state.as_ref())
            .and_then(|state| state.active_agent.as_ref())
            .map(|agent| agent.pending)
            .unwrap_or(false)
    }

    pub(super) fn sync_ai_manager_widgets(&mut self, cx: &mut Cx) {
        let Some(active_mount) = self.data.active_mount.clone() else {
            return;
        };
        let Some(workspace) = self.mount_workspace_widget(cx, &active_mount) else {
            return;
        };

        workspace.widget(cx, ids!(ai_live_markdown)).set_text(
            cx,
            &self
                .mount_state(&active_mount)
                .and_then(|mount| mount.ai_state.as_ref())
                .map(|state| ai_live_activity_markdown(&active_mount, state))
                .unwrap_or_else(|| "_No live AI state yet._".to_string()),
        );

        workspace.widget(cx, ids!(ai_files_markdown)).set_text(
            cx,
            &self
                .mount_state(&active_mount)
                .and_then(|mount| mount.ai_state.as_ref())
                .map(|state| ai_changed_files_markdown(&active_mount, state))
                .unwrap_or_else(|| "_No files changed yet._".to_string()),
        );

        workspace.widget(cx, ids!(ai_swarm_markdown)).set_text(
            cx,
            &self
                .mount_state(&active_mount)
                .and_then(|mount| mount.ai_state.as_ref())
                .map(|state| ai_task_board_markdown(&active_mount, state))
                .unwrap_or_else(|| "_No active tasks._".to_string()),
        );

        let Some(state) = self
            .mount_state(&active_mount)
            .and_then(|mount| mount.ai_state.as_ref())
        else {
            workspace
                .drop_down(cx, ids!(ai_agent_dropdown))
                .set_labels(cx, vec!["Loading AI...".to_string()]);
            workspace
                .drop_down(cx, ids!(ai_agent_dropdown))
                .set_selected_item(cx, 0);
            workspace
                .drop_down(cx, ids!(ai_model_picker))
                .set_labels(cx, vec!["Loading...".to_string()]);
            workspace
                .drop_down(cx, ids!(ai_model_picker))
                .set_selected_item(cx, 0);
            workspace
                .widget(cx, ids!(ai_chat_markdown))
                .set_text(cx, "_No AI state yet._");
            workspace
                .label(cx, ids!(ai_status_label))
                .set_text(cx, "Loading AI...");
            workspace
                .button(cx, ids!(ai_run_button))
                .set_enabled(cx, false);
            workspace
                .drop_down(cx, ids!(ai_subagent_role_picker))
                .set_labels(
                    cx,
                    AI_SUBAGENT_ROLES
                        .iter()
                        .map(|role| ai_subagent_role_label(role))
                        .collect(),
                );
            workspace
                .drop_down(cx, ids!(ai_subagent_role_picker))
                .set_selected_item(cx, 0);
            workspace.widget(cx, ids!(ai_run_button)).set_text(cx, "▶");
            return;
        };

        let agent_labels = state
            .agents
            .iter()
            .map(|agent| ai_agent_picker_label(agent, state))
            .collect::<Vec<_>>();
        let agent_selected = state
            .active_agent_id
            .and_then(|selected| {
                state
                    .agents
                    .iter()
                    .position(|agent| agent.agent_id == selected)
            })
            .unwrap_or(0);
        workspace
            .drop_down(cx, ids!(ai_agent_dropdown))
            .set_labels(cx, non_empty_labels(agent_labels, "Chat 1"));
        workspace
            .drop_down(cx, ids!(ai_agent_dropdown))
            .set_selected_item(cx, agent_selected);

        let active_backend = state.active_backend_id.as_ref().and_then(|active_id| {
            state
                .backends
                .iter()
                .find(|backend| &backend.id == active_id)
        });
        let active_backend_configured = active_backend
            .map(|backend| backend.configured)
            .unwrap_or(true);

        let backend_labels = state
            .backends
            .iter()
            .map(|backend| backend.label.clone())
            .collect::<Vec<_>>();
        let backend_selected = state
            .active_backend_id
            .as_ref()
            .and_then(|active_id| {
                state
                    .backends
                    .iter()
                    .position(|backend| &backend.id == active_id)
            })
            .unwrap_or(0);

        workspace
            .drop_down(cx, ids!(ai_model_picker))
            .set_labels(cx, non_empty_labels(backend_labels, "local"));
        workspace
            .drop_down(cx, ids!(ai_model_picker))
            .set_selected_item(cx, backend_selected);
        workspace
            .drop_down(cx, ids!(ai_subagent_role_picker))
            .set_labels(
                cx,
                AI_SUBAGENT_ROLES
                    .iter()
                    .map(|role| ai_subagent_role_label(role))
                    .collect(),
            );

        if let Some(agent) = state.active_agent.as_ref() {
            let selected_role_index = workspace
                .drop_down(cx, ids!(ai_subagent_role_picker))
                .selected_item();
            workspace
                .widget(cx, ids!(ai_chat_markdown))
                .set_text(cx, &ai_chat_markdown(agent));
            let status_text = if active_backend_configured {
                ai_status_label(agent, selected_role_index)
            } else {
                "Configure backend".to_string()
            };
            workspace
                .label(cx, ids!(ai_status_label))
                .set_text(cx, &status_text);
            workspace
                .button(cx, ids!(ai_run_button))
                .set_enabled(cx, active_backend_configured);
            workspace
                .widget(cx, ids!(ai_run_button))
                .set_text(cx, if agent.pending { "■" } else { "▶" });
        } else {
            workspace
                .widget(cx, ids!(ai_chat_markdown))
                .set_text(cx, "_No AI chats for this mount._");
            workspace
                .label(cx, ids!(ai_status_label))
                .set_text(cx, "No active AI chat");
            workspace
                .button(cx, ids!(ai_run_button))
                .set_enabled(cx, false);
            workspace
                .drop_down(cx, ids!(ai_subagent_role_picker))
                .set_selected_item(cx, 0);
            workspace.widget(cx, ids!(ai_run_button)).set_text(cx, "▶");
        }
    }

    pub(super) fn schedule_ai_chat_scroll_to_bottom(&mut self, cx: &mut Cx) {
        self.ai_chat_scroll_pending = true;
        self.ai_chat_scroll_frames_remaining = AI_CHAT_SCROLL_SETTLE_FRAMES;
        self.ai_chat_scroll_next_frame = cx.new_next_frame();
        self.scroll_ai_chat_to_bottom(cx);
    }

    pub(super) fn flush_ai_chat_scroll_to_bottom(&mut self, cx: &mut Cx) {
        self.scroll_ai_chat_to_bottom(cx);
        if self.ai_chat_scroll_frames_remaining > 1 {
            self.ai_chat_scroll_frames_remaining -= 1;
            self.ai_chat_scroll_next_frame = cx.new_next_frame();
            self.ai_chat_scroll_pending = true;
        } else {
            self.ai_chat_scroll_frames_remaining = 0;
            self.ai_chat_scroll_pending = false;
        }
    }

    fn scroll_ai_chat_to_bottom(&mut self, cx: &mut Cx) {
        let Some(active_mount) = self.data.active_mount.clone() else {
            return;
        };
        let Some(workspace) = self.mount_workspace_widget(cx, &active_mount) else {
            return;
        };
        workspace.view(cx, ids!(chat_scroll)).set_scroll_pos(
            cx,
            Vec2d {
                x: 0.0,
                y: 1_000_000.0,
            },
        );
    }

    fn send_ai_prompt_to_agent(
        &mut self,
        cx: &mut Cx,
        mount: &str,
        agent_id: AiAgentId,
        prompt: &str,
        local_echo: Option<&str>,
    ) -> bool {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return false;
        }

        let is_pending = self
            .mount_state(mount)
            .and_then(|state| state.ai_state.as_ref())
            .and_then(|state| {
                state
                    .agents
                    .iter()
                    .find(|agent| agent.agent_id == agent_id)
                    .map(|agent| agent.pending)
            })
            .unwrap_or(false);
        if is_pending {
            return false;
        }

        if let Some(local_echo) = local_echo {
            if let Some(state) = self.mount_state_mut(mount).ai_state.as_mut() {
                apply_local_prompt_echo(state, agent_id, local_echo);
            }
            self.focus_ai_prompt_after_local_send(cx, mount);
        }

        let _ = self.send_studio(ClientToHub::AiSendPrompt {
            mount: mount.to_string(),
            agent_id,
            text: prompt.to_string(),
        });
        true
    }

    fn spawn_ai_subagent(
        &mut self,
        cx: &mut Cx,
        mount: &str,
        parent_agent_id: AiAgentId,
        role: &str,
        task: &str,
    ) -> bool {
        let task = task.trim();
        if task.is_empty() {
            return false;
        }

        let is_pending = self
            .mount_state(mount)
            .and_then(|state| state.ai_state.as_ref())
            .and_then(|state| {
                state
                    .agents
                    .iter()
                    .find(|agent| agent.agent_id == parent_agent_id)
                    .map(|agent| agent.pending)
            })
            .unwrap_or(false);
        if is_pending {
            return false;
        }

        let echo = native_delegation_echo(role, task);
        if let Some(state) = self.mount_state_mut(mount).ai_state.as_mut() {
            apply_local_prompt_echo(state, parent_agent_id, &echo);
        }
        self.focus_ai_prompt_after_local_send(cx, mount);
        let _ = self.send_studio(ClientToHub::AiSpawnSubagent {
            mount: mount.to_string(),
            parent_agent_id,
            role: role.to_string(),
            task: task.to_string(),
        });
        true
    }

    fn focus_ai_prompt_after_local_send(&mut self, cx: &mut Cx, mount: &str) {
        if self.data.active_mount.as_deref() != Some(mount) {
            return;
        }
        self.sync_ai_manager_widgets(cx);
        if let Some(workspace) = self.mount_workspace_widget(cx, mount) {
            workspace
                .text_input(cx, ids!(ai_prompt_input))
                .set_key_focus(cx);
            workspace
                .fold_header(cx, ids!(ai_swarm_fold))
                .set_is_open(cx, true, Animate::Yes);
            workspace
                .fold_header(cx, ids!(ai_live_fold))
                .set_is_open(cx, true, Animate::Yes);
            workspace
                .fold_header(cx, ids!(ai_files_fold))
                .set_is_open(cx, true, Animate::Yes);
        }
        self.schedule_ai_chat_scroll_to_bottom(cx);
    }
}
