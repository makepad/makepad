use crate::{makepad_widgets::*, App};
use makepad_studio_protocol::hub_protocol::{
    AiAgentId, AiAgentState, AiMessage, AiMessageRole, AiMountState, ClientToHub,
    TerminalFramebuffer,
};

const AI_CHAT_SCROLL_SETTLE_FRAMES: u8 = 4;
const AI_TASK_EVENT_PREFIX: &str = "TASK EVENT:";
const AI_TERMINAL_EXCERPT_MAX_CHARS: usize = 480;
const AI_TERMINAL_EXCERPT_MAX_LINES: usize = 10;
const AI_CHAT_COMPACT_MAX_CHARS: usize = 220;

struct AiTerminalSnapshot {
    path: String,
    mode: &'static str,
    summary: String,
    visible_text: String,
    is_codex: bool,
    codex_status: Option<String>,
}

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
        self.process_ai_manager_ai_state(cx, mount);
        if self.data.active_mount.as_deref() == Some(mount) {
            self.sync_ai_manager_widgets(cx);
            if should_scroll {
                self.schedule_ai_chat_scroll_to_bottom(cx);
            }
        }
        let _ = self.dispatch_next_ai_manager_followup(cx, mount);
    }

    pub(super) fn refresh_ai_manager_report(&mut self, cx: &mut Cx) {
        self.sync_ai_manager_widgets(cx);
    }

    pub(super) fn refresh_ai_manager_preview(&mut self, cx: &mut Cx) {
        self.sync_ai_manager_widgets(cx);
    }

    pub(super) fn process_ai_manager_task_terminal_update(&mut self, cx: &mut Cx, path: &str) {
        let Some(mount) = Self::mount_from_virtual_path(path).map(str::to_string) else {
            return;
        };

        let snapshot = self.ai_terminal_snapshot(path);
        let mut queue = Vec::new();
        let mut changed = false;

        {
            let mount_state = self.mount_state_mut(&mount);
            for task in mount_state
                .ai_local
                .tasks
                .iter_mut()
                .filter(|task| task.terminal_path.as_deref() == Some(path))
            {
                let previous_mode = task.last_terminal_mode.clone();
                let previous_summary = task.last_terminal_summary.clone();
                let previous_excerpt = task.last_terminal_excerpt.clone();
                let previous_codex_status = task.last_codex_status.clone();

                task.status = if snapshot.mode == "needs-attention" {
                    "needs-attention".to_string()
                } else if snapshot.mode == "done" {
                    "done".to_string()
                } else {
                    "watching".to_string()
                };
                task.last_terminal_mode = snapshot.mode.to_string();
                task.last_terminal_summary = snapshot.summary.clone();
                task.last_terminal_excerpt = Self::truncate_terminal_excerpt(
                    &snapshot.visible_text,
                    AI_TERMINAL_EXCERPT_MAX_CHARS,
                    AI_TERMINAL_EXCERPT_MAX_LINES,
                );
                task.last_codex_status = snapshot.codex_status.clone();

                if previous_mode != task.last_terminal_mode
                    || previous_summary != task.last_terminal_summary
                    || previous_excerpt != task.last_terminal_excerpt
                    || previous_codex_status != task.last_codex_status
                {
                    changed = true;
                }

                if previous_mode != "needs-attention" && snapshot.mode == "needs-attention" {
                    queue.push((
                        task.id,
                        format!("terminal:{}:attention:{}", path, task.last_terminal_summary),
                        "Tracked terminal needs attention".to_string(),
                    ));
                } else if previous_mode != "done" && snapshot.mode == "done" {
                    queue.push((
                        task.id,
                        format!("terminal:{}:done:{}", path, task.last_terminal_summary),
                        "Tracked terminal appears done".to_string(),
                    ));
                }
            }
        }

        for (task_id, signature, reason) in queue {
            self.queue_ai_task_followup(&mount, task_id, signature, &reason);
        }

        if changed && self.data.active_mount.as_deref() == Some(mount.as_str()) {
            self.sync_ai_manager_widgets(cx);
        }

        let _ = self.dispatch_next_ai_manager_followup(cx, &mount);
    }

    pub(super) fn process_ai_manager_path_change(&mut self, cx: &mut Cx, path: &str) {
        if Self::is_terminal_virtual_path(path) || !path.contains('/') {
            return;
        }
        let Some(mount) = Self::mount_from_virtual_path(path).map(str::to_string) else {
            return;
        };
        let Some((_, relative_path)) = path.split_once('/') else {
            return;
        };

        let mut queue = Vec::new();
        let mut changed = false;
        {
            let mount_state = self.mount_state_mut(&mount);
            for task in &mut mount_state.ai_local.tasks {
                if !matches_expected_path(relative_path, &task.expected_paths) {
                    continue;
                }
                if !task
                    .touched_paths
                    .iter()
                    .any(|existing| existing == relative_path)
                {
                    task.touched_paths.push(relative_path.to_string());
                    changed = true;
                }
                if matches!(
                    task.last_terminal_mode.as_str(),
                    "done" | "awaiting-input" | "needs-attention"
                ) {
                    queue.push((
                        task.id,
                        format!(
                            "file:{}:{}:{}",
                            relative_path, task.last_terminal_mode, task.last_terminal_summary
                        ),
                        format!("Observed filesystem change for `{}`", relative_path),
                    ));
                }
            }
        }

        for (task_id, signature, reason) in queue {
            self.queue_ai_task_followup(&mount, task_id, signature, &reason);
        }

        if changed && self.data.active_mount.as_deref() == Some(mount.as_str()) {
            self.sync_ai_manager_widgets(cx);
        }

        let _ = self.dispatch_next_ai_manager_followup(cx, &mount);
    }

    pub(super) fn process_ai_manager_terminal_closed(
        &mut self,
        cx: &mut Cx,
        path: &str,
        code: i32,
    ) {
        let Some(mount) = Self::mount_from_virtual_path(path).map(str::to_string) else {
            return;
        };

        let mut queue = Vec::new();
        let mut changed = false;
        {
            let mount_state = self.mount_state_mut(&mount);
            for task in mount_state
                .ai_local
                .tasks
                .iter_mut()
                .filter(|task| task.terminal_path.as_deref() == Some(path))
            {
                task.status = if code == 0 {
                    "done".to_string()
                } else {
                    "needs-attention".to_string()
                };
                task.last_terminal_mode = "exited".to_string();
                task.last_terminal_summary = format!("terminal exited ({})", code);
                changed = true;
                queue.push((
                    task.id,
                    format!("terminal:{}:exit:{}", path, code),
                    format!("Tracked terminal exited with code {}", code),
                ));
            }
        }

        for (task_id, signature, reason) in queue {
            self.queue_ai_task_followup(&mount, task_id, signature, &reason);
        }

        if changed && self.data.active_mount.as_deref() == Some(mount.as_str()) {
            self.sync_ai_manager_widgets(cx);
        }

        let _ = self.dispatch_next_ai_manager_followup(cx, &mount);
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
        self.note_ai_prompt_task(mount, agent_id, &prompt);
        if self.send_ai_prompt_to_agent(cx, mount, agent_id, &prompt, true) {
            input.set_text(cx, "");
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
        self.mount_state(active_mount)
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

        workspace
            .widget(cx, ids!(ai_live_markdown))
            .set_text(cx, &self.ai_live_markdown(&active_mount));

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
                .widget(cx, ids!(ai_chat_markdown))
                .set_text(cx, "_No AI state yet._");
            workspace
                .label(cx, ids!(ai_status_label))
                .set_text(cx, "Loading AI...");
            workspace
                .button(cx, ids!(ai_send_button))
                .set_enabled(cx, false);
            workspace
                .button(cx, ids!(ai_cancel_button))
                .set_enabled(cx, false);
            return;
        };

        let agent_labels = state
            .agents
            .iter()
            .map(|agent| {
                if agent.pending {
                    format!("{} *", agent.title)
                } else {
                    agent.title.clone()
                }
            })
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

        if let Some(agent) = state.active_agent.as_ref() {
            workspace
                .widget(cx, ids!(ai_chat_markdown))
                .set_text(cx, &ai_chat_markdown(agent));
            workspace
                .label(cx, ids!(ai_status_label))
                .set_text(cx, &agent.status);
            workspace
                .button(cx, ids!(ai_send_button))
                .set_enabled(cx, !agent.pending);
            workspace
                .button(cx, ids!(ai_cancel_button))
                .set_enabled(cx, agent.pending);
        } else {
            workspace
                .widget(cx, ids!(ai_chat_markdown))
                .set_text(cx, "_No AI chats for this mount._");
            workspace
                .label(cx, ids!(ai_status_label))
                .set_text(cx, "No active AI chat");
            workspace
                .button(cx, ids!(ai_send_button))
                .set_enabled(cx, false);
            workspace
                .button(cx, ids!(ai_cancel_button))
                .set_enabled(cx, false);
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
        echo_local: bool,
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

        if echo_local {
            if let Some(state) = self.mount_state_mut(mount).ai_state.as_mut() {
                apply_local_prompt_echo(state, agent_id, prompt);
            }
            if self.data.active_mount.as_deref() == Some(mount) {
                self.sync_ai_manager_widgets(cx);
                if let Some(workspace) = self.mount_workspace_widget(cx, mount) {
                    workspace
                        .text_input(cx, ids!(ai_prompt_input))
                        .set_key_focus(cx);
                }
                self.schedule_ai_chat_scroll_to_bottom(cx);
            }
        }

        let _ = self.send_studio(ClientToHub::AiSendPrompt {
            mount: mount.to_string(),
            agent_id,
            text: prompt.to_string(),
        });
        true
    }

    fn note_ai_prompt_task(&mut self, mount: &str, agent_id: AiAgentId, prompt: &str) {
        if !should_track_ai_terminal_task(prompt) {
            return;
        }
        let mount_state = self.mount_state_mut(mount);
        let task_id = mount_state.ai_local.next_task_id.max(1);
        mount_state.ai_local.next_task_id = task_id.saturating_add(1);
        mount_state
            .ai_local
            .tasks
            .push(crate::app_data::AiTrackedTask {
                id: task_id,
                agent_id,
                goal: prompt.trim().to_string(),
                terminal_path: None,
                expected_paths: extract_expected_paths_from_prompt(prompt),
                touched_paths: Vec::new(),
                status: "waiting-terminal".to_string(),
                last_terminal_mode: "waiting-terminal".to_string(),
                last_terminal_summary: "Waiting for the AI to hand work to a terminal".to_string(),
                last_terminal_excerpt: String::new(),
                last_codex_status: None,
            });
    }

    fn process_ai_manager_ai_state(&mut self, _cx: &mut Cx, mount: &str) {
        let Some(active_agent) = self
            .mount_state(mount)
            .and_then(|state| state.ai_state.as_ref())
            .and_then(|state| state.active_agent.clone())
        else {
            return;
        };

        let start = self
            .mount_state(mount)
            .map(|state| {
                state
                    .ai_local
                    .last_processed_message_count_by_agent
                    .get(&active_agent.agent_id)
                    .copied()
                    .unwrap_or(0)
                    .min(active_agent.messages.len())
            })
            .unwrap_or(0);
        let new_messages = active_agent.messages[start..].to_vec();
        self.mount_state_mut(mount)
            .ai_local
            .last_processed_message_count_by_agent
            .insert(active_agent.agent_id, active_agent.messages.len());

        for message in &new_messages {
            self.process_ai_manager_ai_message(mount, active_agent.agent_id, message);
        }
    }

    fn process_ai_manager_ai_message(
        &mut self,
        mount: &str,
        agent_id: AiAgentId,
        message: &AiMessage,
    ) {
        let tool_name = match message.role {
            AiMessageRole::ToolCall | AiMessageRole::ToolResult => extract_tool_name(&message.text),
            _ => None,
        };
        let Some(tool_name) = tool_name else {
            return;
        };
        let Some(payload) = extract_code_block_body(&message.text) else {
            return;
        };

        let path =
            if tool_name == "open_terminal" && matches!(message.role, AiMessageRole::ToolResult) {
                parse_json_string_field(payload, "path")
            } else if is_terminal_tool_name(&tool_name) {
                parse_json_string_field(payload, "path")
            } else {
                None
            };
        let Some(path) = path else {
            return;
        };
        self.bind_waiting_ai_task_to_terminal(mount, agent_id, &path);
    }

    fn bind_waiting_ai_task_to_terminal(&mut self, mount: &str, agent_id: AiAgentId, path: &str) {
        let title = self.terminal_tab_title(path);
        let snapshot = self.ai_terminal_snapshot(path);
        let mount_state = self.mount_state_mut(mount);
        if let Some(task) = mount_state
            .ai_local
            .tasks
            .iter_mut()
            .find(|task| task.agent_id == agent_id && task.terminal_path.is_none())
        {
            task.terminal_path = Some(path.to_string());
            task.status = "watching".to_string();
            task.last_terminal_mode = snapshot.mode.to_string();
            task.last_terminal_summary = if snapshot.summary.is_empty() {
                format!("Tracking {}", title)
            } else {
                snapshot.summary
            };
            task.last_terminal_excerpt = Self::truncate_terminal_excerpt(
                &snapshot.visible_text,
                AI_TERMINAL_EXCERPT_MAX_CHARS,
                AI_TERMINAL_EXCERPT_MAX_LINES,
            );
            task.last_codex_status = snapshot.codex_status;
        }
    }

    fn queue_ai_task_followup(
        &mut self,
        mount: &str,
        task_id: u64,
        signature: String,
        reason: &str,
    ) {
        let Some((agent_id, text)) = self.ai_task_event_prompt(mount, task_id, reason) else {
            return;
        };
        let mount_state = self.mount_state_mut(mount);
        if mount_state
            .ai_local
            .queued_followups
            .iter()
            .any(|entry| entry.task_id == task_id && entry.signature == signature)
        {
            return;
        }
        mount_state
            .ai_local
            .queued_followups
            .push_back(crate::app_data::AiQueuedFollowup {
                agent_id,
                task_id,
                signature,
                text,
            });
    }

    fn ai_task_event_prompt(
        &self,
        mount: &str,
        task_id: u64,
        reason: &str,
    ) -> Option<(AiAgentId, String)> {
        let task = self
            .mount_state(mount)?
            .ai_local
            .tasks
            .iter()
            .find(|task| task.id == task_id)?;
        let mut prompt = String::new();
        prompt.push_str(AI_TASK_EVENT_PREFIX);
        prompt.push(' ');
        prompt.push_str(&format!("task {} update\n", task.id));
        prompt.push_str(&format!("Reason: {}\n", reason));
        prompt.push_str(&format!("Goal: {}\n", task.goal));
        prompt.push_str(&format!("Task state: {}\n", task.status));
        if let Some(path) = &task.terminal_path {
            prompt.push_str(&format!("Terminal path: {}\n", path));
            prompt.push_str(&format!(
                "Terminal title: {}\n",
                self.terminal_tab_title(path)
            ));
        }
        prompt.push_str(&format!("Terminal mode: {}\n", task.last_terminal_mode));
        if let Some(codex_status) = &task.last_codex_status {
            prompt.push_str(&format!("Codex status: {}\n", codex_status));
        }
        if !task.last_terminal_summary.is_empty() {
            prompt.push_str(&format!("Summary: {}\n", task.last_terminal_summary));
        }
        if !task.expected_paths.is_empty() {
            prompt.push_str(&format!(
                "Expected paths: {}\n",
                task.expected_paths.join(", ")
            ));
        }
        if !task.touched_paths.is_empty() {
            prompt.push_str(&format!(
                "Touched paths: {}\n",
                task.touched_paths.join(", ")
            ));
        }
        if !task.last_terminal_excerpt.is_empty() {
            prompt.push_str("\nLatest output excerpt:\n```text\n");
            prompt.push_str(&task.last_terminal_excerpt);
            prompt.push_str("\n```\n");
        }
        prompt.push_str(
            "\nContinue supervising this delegated terminal task. If it is finished, tell the user briefly. If more work is needed, use terminal tools instead of guessing.",
        );
        Some((task.agent_id, prompt))
    }

    fn dispatch_next_ai_manager_followup(&mut self, cx: &mut Cx, mount: &str) -> bool {
        let Some((queue_index, queued)) = self.mount_state(mount).and_then(|state| {
            let ai_state = state.ai_state.as_ref()?;
            state
                .ai_local
                .queued_followups
                .iter()
                .enumerate()
                .find(|(_, entry)| {
                    ai_state
                        .agents
                        .iter()
                        .find(|agent| agent.agent_id == entry.agent_id)
                        .map(|agent| !agent.pending)
                        .unwrap_or(false)
                })
                .map(|(index, entry)| (index, entry.clone()))
        }) else {
            return false;
        };

        if !self.send_ai_prompt_to_agent(cx, mount, queued.agent_id, &queued.text, true) {
            return false;
        }
        let mount_state = self.mount_state_mut(mount);
        let _ = mount_state.ai_local.queued_followups.remove(queue_index);
        true
    }

    fn ai_live_markdown(&self, mount: &str) -> String {
        let Some(mount_state) = self.mount_state(mount) else {
            return "_No live AI state yet._".to_string();
        };

        let mut markdown = String::new();
        if mount_state.ai_local.tasks.is_empty() {
            markdown.push_str("**Tasks**\n\n_No delegated terminal tasks yet._");
        } else {
            markdown.push_str("**Tasks**\n\n");
            for task in &mount_state.ai_local.tasks {
                markdown.push_str(&format!(
                    "- `T{}` [{}] {}\n",
                    task.id,
                    task.status,
                    truncate_inline(&task.goal, 96)
                ));
                if let Some(path) = &task.terminal_path {
                    markdown.push_str(&format!(
                        "  `{}` [{}]\n",
                        path,
                        truncate_inline(&task.last_terminal_summary, 96)
                    ));
                } else {
                    markdown.push_str("  waiting for terminal assignment\n");
                }
                if !task.touched_paths.is_empty() {
                    markdown.push_str(&format!("  files: {}\n", task.touched_paths.join(", ")));
                } else if !task.expected_paths.is_empty() {
                    markdown.push_str(&format!(
                        "  expecting: {}\n",
                        task.expected_paths.join(", ")
                    ));
                }
            }
        }

        let terminals = self.ai_terminal_snapshots_for_mount(mount);
        markdown.push_str("\n\n**Terminals**\n\n");
        if terminals.is_empty() {
            markdown.push_str("_No terminal activity yet._");
        } else {
            for terminal in terminals {
                markdown.push_str(&format!(
                    "- `{}` [{}{}]\n",
                    terminal.path,
                    terminal.mode,
                    if terminal.is_codex { " / codex" } else { "" }
                ));
                if let Some(codex_status) = &terminal.codex_status {
                    markdown.push_str(&format!("  {}\n", truncate_inline(codex_status, 96)));
                }
                markdown.push_str(&format!("  {}\n", truncate_inline(&terminal.summary, 96)));
            }
        }

        markdown
    }

    fn ai_terminal_snapshots_for_mount(&self, mount: &str) -> Vec<AiTerminalSnapshot> {
        self.collect_mount_terminal_files(mount)
            .into_iter()
            .map(|path| self.ai_terminal_snapshot(&path))
            .collect()
    }

    fn ai_terminal_snapshot(&self, path: &str) -> AiTerminalSnapshot {
        let title = self.terminal_tab_title(path);
        let visible_text = self
            .data
            .terminal_framebuffer_by_path
            .get(path)
            .map(Self::decode_terminal_frame_lines)
            .unwrap_or_default()
            .join("\n");
        let (mode, is_codex, summary, codex_status) =
            Self::terminal_mode_and_summary(&title, &visible_text);
        AiTerminalSnapshot {
            path: path.to_string(),
            mode,
            summary,
            visible_text,
            is_codex,
            codex_status,
        }
    }

    fn decode_terminal_frame_lines(frame: &TerminalFramebuffer) -> Vec<String> {
        let cols = frame.cols as usize;
        let rows = frame.rows as usize;
        let mut lines = Vec::with_capacity(rows);
        for row in 0..rows {
            let mut line = String::with_capacity(cols);
            for col in 0..cols {
                let idx = (row * cols + col) * 10;
                if idx + 3 >= frame.cells.len() {
                    break;
                }
                let codepoint = u32::from_le_bytes([
                    frame.cells[idx],
                    frame.cells[idx + 1],
                    frame.cells[idx + 2],
                    frame.cells[idx + 3],
                ]);
                line.push(char::from_u32(codepoint).unwrap_or(' '));
            }
            lines.push(line.trim_end().to_string());
        }
        lines
    }

    fn is_codex_prompt_line(line: &str) -> bool {
        let trimmed = line.trim_start();
        trimmed.starts_with('›')
            || trimmed.starts_with('>')
            || trimmed.contains("Enter a prompt...")
    }

    fn detect_codex_status_line(lines: &[String]) -> Option<String> {
        lines.iter().rev().take(8).find_map(|line| {
            let trimmed = line.trim();
            if trimmed.contains("Working (") && trimmed.contains("esc to interrupt") {
                Some(trimmed.to_string())
            } else {
                None
            }
        })
    }

    fn terminal_summary_line(
        lines: &[String],
        is_codex: bool,
        codex_status: Option<&str>,
    ) -> String {
        lines
            .iter()
            .rev()
            .map(|line| line.trim())
            .find(|line| {
                !line.is_empty()
                    && Some(*line) != codex_status
                    && !(is_codex
                        && (Self::is_codex_prompt_line(line)
                            || line.contains("esc to interrupt")
                            || line.contains("100% left")
                            || line.contains("left ·")))
            })
            .map(|line| truncate_inline(line, 140))
            .unwrap_or_else(|| "No visible output yet".to_string())
    }

    fn terminal_mode_and_summary(
        title: &str,
        visible_text: &str,
    ) -> (&'static str, bool, String, Option<String>) {
        let lines: Vec<String> = visible_text.lines().map(|line| line.to_string()).collect();
        let lowered = format!("{}\n{}", title, visible_text).to_lowercase();
        let is_codex = lowered.contains("codex")
            || lowered.contains("apply_patch")
            || lowered.contains("exec_command")
            || lowered.contains("functions.exec_command");
        let codex_status = if is_codex {
            Self::detect_codex_status_line(&lines)
        } else {
            None
        };
        let needs_attention = lowered.contains("permission denied")
            || lowered.contains("sandbox")
            || lowered.contains("panic")
            || lowered.contains("error:")
            || lowered.contains("failed")
            || lowered.contains("blocked")
            || lowered.contains("approve")
            || lowered.contains("how would you like to proceed");
        let awaiting_input = lowered.contains("waiting for user")
            || lowered.contains("request user input")
            || lowered.contains("press enter")
            || lowered.contains("press return")
            || lowered.contains("continue?")
            || lowered.contains("type 'continue'")
            || lowered.contains("type \"continue\"");
        let working = lowered.contains("apply_patch")
            || lowered.contains("exec_command")
            || lowered.contains("searching")
            || lowered.contains("reading")
            || lowered.contains("building")
            || lowered.contains("testing")
            || lowered.contains("running")
            || lowered.contains("patching")
            || codex_status.is_some();
        let codex_prompt_visible = is_codex
            && lines
                .iter()
                .rev()
                .take(6)
                .any(|line| Self::is_codex_prompt_line(line));

        let mode = if needs_attention {
            "needs-attention"
        } else if awaiting_input {
            "awaiting-input"
        } else if is_codex && codex_prompt_visible && codex_status.is_none() {
            "done"
        } else if visible_text.trim().is_empty() {
            "starting"
        } else if working {
            "working"
        } else {
            "idle"
        };

        (
            mode,
            is_codex,
            Self::terminal_summary_line(&lines, is_codex, codex_status.as_deref()),
            codex_status,
        )
    }

    fn truncate_terminal_excerpt(text: &str, max_chars: usize, max_lines: usize) -> String {
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.trim().is_empty())
            .collect();
        if lines.is_empty() {
            return String::new();
        }
        let start = lines.len().saturating_sub(max_lines);
        let excerpt = lines[start..].join("\n");
        if excerpt.chars().count() <= max_chars {
            return excerpt;
        }
        let tail: String = excerpt
            .chars()
            .rev()
            .take(max_chars.saturating_sub(1))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("…{}", tail)
    }
}

fn ai_chat_markdown(agent: &AiAgentState) -> String {
    if agent.messages.is_empty() {
        return "_No messages yet._".to_string();
    }
    let mut markdown = String::new();
    for message in &agent.messages {
        let heading = ai_message_heading(message);
        let body = ai_message_markdown_body(message);
        if body.is_empty() {
            continue;
        }
        if !markdown.is_empty() {
            markdown.push_str("\n\n");
        }
        markdown.push_str(heading);
        markdown.push_str("\n\n");
        markdown.push_str(&body);
    }
    markdown
}

fn ai_message_heading(message: &AiMessage) -> &'static str {
    match message.role {
        AiMessageRole::User if message.text.starts_with(AI_TASK_EVENT_PREFIX) => "### Event",
        AiMessageRole::User => "### User",
        AiMessageRole::Assistant => "### Assistant",
        AiMessageRole::Thinking => "### Thinking",
        AiMessageRole::System => "### System",
        AiMessageRole::ToolCall => "### Tool",
        AiMessageRole::ToolResult => "### Tool",
        AiMessageRole::Error => "### Error",
    }
}

fn ai_message_markdown_body(message: &AiMessage) -> String {
    match message.role {
        AiMessageRole::Thinking => {
            if message.text.trim().is_empty() {
                "_thinking..._".to_string()
            } else {
                truncate_inline(message.text.trim(), AI_CHAT_COMPACT_MAX_CHARS)
            }
        }
        AiMessageRole::ToolCall => summarize_tool_call_message(&message.text),
        AiMessageRole::ToolResult => summarize_tool_result_message(&message.text),
        AiMessageRole::User if message.text.starts_with(AI_TASK_EVENT_PREFIX) => {
            summarize_task_event_message(&message.text)
        }
        _ => message.text.trim().to_string(),
    }
}

fn summarize_task_event_message(text: &str) -> String {
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            !line.starts_with("Continue supervising this delegated terminal task")
                && !line.starts_with("Latest output excerpt:")
                && *line != "```text"
                && *line != "```"
        })
        .take(7)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return truncate_inline(text.trim(), AI_CHAT_COMPACT_MAX_CHARS);
    }
    lines
        .into_iter()
        .map(|line| format!("> {}", truncate_inline(line, 120)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn summarize_tool_call_message(text: &str) -> String {
    let Some(tool_name) = extract_tool_name(text) else {
        return truncate_inline(text.trim(), AI_CHAT_COMPACT_MAX_CHARS);
    };
    let summary = extract_code_block_body(text)
        .and_then(|payload| parse_json_string_field(payload, "path"))
        .map(|path| format!("`{}` `{}`", tool_name, path))
        .unwrap_or_else(|| format!("`{}`", tool_name));
    summary
}

fn summarize_tool_result_message(text: &str) -> String {
    let Some(tool_name) = extract_tool_name(text) else {
        return truncate_inline(text.trim(), AI_CHAT_COMPACT_MAX_CHARS);
    };
    let payload = extract_code_block_body(text).unwrap_or_default();
    match tool_name.as_str() {
        "open_terminal" => parse_json_string_field(payload, "path")
            .map(|path| format!("opened terminal `{}`", path))
            .unwrap_or_else(|| "opened terminal".to_string()),
        "send_terminal_text" => {
            let path = parse_json_string_field(payload, "path").unwrap_or_default();
            let submitted = parse_json_bool_field(payload, "submitted").unwrap_or(false);
            if path.is_empty() {
                if submitted {
                    "sent text and pressed Enter in terminal".to_string()
                } else {
                    "sent text to terminal".to_string()
                }
            } else if submitted {
                format!("sent text and pressed Enter in `{}`", path)
            } else {
                format!("sent text to `{}`", path)
            }
        }
        "send_terminal_key" => parse_json_string_field(payload, "path")
            .map(|path| format!("sent key to `{}`", path))
            .unwrap_or_else(|| "sent key to terminal".to_string()),
        "read_terminal" => {
            let path = parse_json_string_field(payload, "path").unwrap_or_default();
            let mode = parse_json_string_field(payload, "mode").unwrap_or_default();
            let summary = parse_json_string_field(payload, "summary").unwrap_or_default();
            if path.is_empty() {
                "read terminal".to_string()
            } else if summary.is_empty() {
                format!("read `{}` [{}]", path, mode)
            } else {
                format!(
                    "read `{}` [{}] {}",
                    path,
                    if mode.is_empty() { "unknown" } else { &mode },
                    truncate_inline(&summary, 110)
                )
            }
        }
        "observe_filesystem" => {
            let count = payload.matches("\"seconds_ago\":").count();
            if count == 0 {
                "checked recent filesystem changes".to_string()
            } else {
                format!("checked recent filesystem changes ({})", count)
            }
        }
        "open_editor" => truncate_inline(payload.trim(), 120),
        _ => truncate_inline(payload.trim(), 120),
    }
}

fn extract_tool_name(text: &str) -> Option<String> {
    let rest = text.strip_prefix('`')?;
    let (tool_name, _) = rest.split_once('`')?;
    Some(tool_name.to_string())
}

fn extract_code_block_body(text: &str) -> Option<&str> {
    let start = text.find("```")?;
    let after_start = &text[start + 3..];
    let newline = after_start.find('\n')?;
    let body = &after_start[newline + 1..];
    let end = body.rfind("\n```")?;
    Some(&body[..end])
}

fn parse_json_string_field<'a>(json: &'a str, field: &str) -> Option<String> {
    let needle = format!("\"{}\":\"", field);
    let start = json.find(&needle)? + needle.len();
    let mut out = String::new();
    let mut escaped = false;
    for ch in json[start..].chars() {
        if escaped {
            out.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '"' => '"',
                '\\' => '\\',
                other => other,
            });
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(out),
            other => out.push(other),
        }
    }
    None
}

fn parse_json_bool_field(json: &str, field: &str) -> Option<bool> {
    let true_needle = format!("\"{}\":true", field);
    if json.contains(&true_needle) {
        return Some(true);
    }
    let false_needle = format!("\"{}\":false", field);
    if json.contains(&false_needle) {
        return Some(false);
    }
    None
}

fn is_terminal_tool_name(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "read_terminal" | "send_terminal_text" | "send_terminal_key" | "open_terminal"
    )
}

fn should_track_ai_terminal_task(prompt: &str) -> bool {
    let lowered = prompt.to_lowercase();
    lowered.contains("codex")
        || lowered.contains("terminal")
        || lowered.contains("other agent")
        || lowered.contains("tell ") && lowered.contains(" to ")
}

fn extract_expected_paths_from_prompt(prompt: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in prompt.split_whitespace() {
        let token = raw.trim_matches(|ch: char| {
            matches!(
                ch,
                '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | ':'
            )
        });
        if token.is_empty() || token.starts_with('-') {
            continue;
        }
        let looks_like_path = token.contains('/')
            || token.contains('\\')
            || token.rsplit_once('.').is_some_and(|(_, ext)| {
                !ext.is_empty()
                    && ext.len() <= 8
                    && ext.chars().all(|ch| ch.is_ascii_alphanumeric())
            });
        if !looks_like_path {
            continue;
        }
        let normalized = token.replace('\\', "/");
        if !out.iter().any(|existing| existing == &normalized) {
            out.push(normalized);
        }
    }
    out
}

fn matches_expected_path(path: &str, expected_paths: &[String]) -> bool {
    expected_paths.iter().any(|expected| {
        path == expected || path.ends_with(&format!("/{}", expected)) || expected.ends_with(path)
    })
}

fn truncate_inline(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out = trimmed
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

fn apply_local_prompt_echo(state: &mut AiMountState, agent_id: AiAgentId, prompt: &str) {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return;
    }

    let title = summarized_chat_title(state, agent_id, prompt);

    if let Some(agent) = state
        .active_agent
        .as_mut()
        .filter(|agent| agent.agent_id == agent_id)
    {
        if let Some(title) = title.as_ref() {
            agent.title = title.clone();
        }
        agent.messages.push(AiMessage {
            role: AiMessageRole::User,
            text: prompt.to_string(),
        });
        agent.messages.push(AiMessage {
            role: AiMessageRole::Thinking,
            text: String::new(),
        });
        agent.pending = true;
        agent.status = "thinking...".to_string();
    }

    if let Some(summary) = state
        .agents
        .iter_mut()
        .find(|agent| agent.agent_id == agent_id)
    {
        if let Some(title) = title.as_ref() {
            summary.title = title.clone();
        }
        summary.pending = true;
        summary.status = "thinking...".to_string();
        summary.message_count += 2;
    }
}

fn summarized_chat_title(
    state: &AiMountState,
    agent_id: AiAgentId,
    prompt: &str,
) -> Option<String> {
    let should_summarize = state
        .active_agent
        .as_ref()
        .filter(|agent| agent.agent_id == agent_id)
        .map(|agent| agent.messages.is_empty() && agent.title.starts_with("Chat "))
        .or_else(|| {
            state
                .agents
                .iter()
                .find(|agent| agent.agent_id == agent_id)
                .map(|agent| agent.message_count == 0 && agent.title.starts_with("Chat "))
        })
        .unwrap_or(false);

    if !should_summarize {
        return None;
    }

    let single_line = prompt.replace('\n', " ").trim().to_string();
    if single_line.is_empty() {
        return None;
    }
    let mut title = single_line.chars().take(40).collect::<String>();
    if single_line.chars().count() > 40 {
        title.push_str("...");
    }
    Some(title)
}

fn non_empty_labels(mut labels: Vec<String>, fallback: &str) -> Vec<String> {
    if labels.is_empty() {
        labels.push(fallback.to_string());
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_studio_protocol::hub_protocol::{AiAgentSummary, AiBackendInfo};

    #[test]
    fn apply_local_prompt_echo_updates_visible_agent_immediately() {
        let agent_id = AiAgentId(7);
        let mut state = AiMountState {
            backends: vec![AiBackendInfo {
                id: "local".to_string(),
                label: "Local".to_string(),
                detail: String::new(),
                configured: true,
            }],
            active_backend_id: Some("local".to_string()),
            active_agent_id: Some(agent_id),
            agents: vec![AiAgentSummary {
                agent_id,
                title: "Chat 1".to_string(),
                backend_id: "local".to_string(),
                status: "idle".to_string(),
                pending: false,
                updated_at: 0.0,
                message_count: 0,
            }],
            active_agent: Some(AiAgentState {
                agent_id,
                title: "Chat 1".to_string(),
                backend_id: "local".to_string(),
                status: "idle".to_string(),
                pending: false,
                messages: Vec::new(),
            }),
        };

        apply_local_prompt_echo(&mut state, agent_id, "say hi");

        let agent = state.active_agent.as_ref().unwrap();
        assert_eq!(agent.title, "say hi");
        assert_eq!(agent.status, "thinking...");
        assert!(agent.pending);
        assert_eq!(agent.messages.len(), 2);
        assert!(matches!(agent.messages[0].role, AiMessageRole::User));
        assert_eq!(agent.messages[0].text, "say hi");
        assert!(matches!(agent.messages[1].role, AiMessageRole::Thinking));
        assert_eq!(agent.messages[1].text, "");

        let summary = &state.agents[0];
        assert_eq!(summary.title, "say hi");
        assert_eq!(summary.status, "thinking...");
        assert!(summary.pending);
        assert_eq!(summary.message_count, 2);
    }

    #[test]
    fn extracts_expected_paths_from_prompt() {
        assert_eq!(
            extract_expected_paths_from_prompt("tell codex to write a poem into `poem.txt`"),
            vec!["poem.txt".to_string()]
        );
    }

    #[test]
    fn matches_expected_path_handles_relative_targets() {
        assert!(matches_expected_path(
            "src/poem.txt",
            &[String::from("poem.txt")]
        ));
        assert!(matches_expected_path(
            "poem.txt",
            &[String::from("poem.txt")]
        ));
    }
}
