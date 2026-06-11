use crate::{makepad_widgets::*, App};
use makepad_studio_protocol::ai_format::{
    parse_json_bool_field, parse_json_string_field, AI_TASK_EVENT_PREFIX,
    AI_TERMINAL_OBSERVATION_PREFIX, AI_WAITING_MESSAGE_PREFIX,
};
use makepad_studio_protocol::hub_protocol::{
    AiAgentId, AiAgentState, AiAgentSummary, AiMessage, AiMessageRole, AiMountState, ClientToHub,
};

const AI_CHAT_SCROLL_SETTLE_FRAMES: u8 = 4;
const AI_CHAT_COMPACT_MAX_CHARS: usize = 220;
const AI_CHAT_ACTIVITY_MAX_CHARS: usize = 140;
const AI_TASK_BOARD_WORKFLOW_NAME_MAX_CHARS: usize = 64;
const AI_TASK_BOARD_WORKFLOW_STEP_MAX_CHARS: usize = 96;
const AI_TASK_BOARD_WORKFLOW_MAX_STEPS: usize = 10;

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
                .map(ai_live_activity_markdown)
                .unwrap_or_else(|| "_No live AI state yet._".to_string()),
        );

        workspace.widget(cx, ids!(ai_swarm_markdown)).set_text(
            cx,
            &self
                .mount_state(&active_mount)
                .and_then(|mount| mount.ai_state.as_ref())
                .map(ai_task_board_markdown)
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
            workspace.widget(cx, ids!(ai_run_button)).set_text(cx, "▶");
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

        let active_backend = state.active_backend_id.as_ref().and_then(|active_id| {
            state
                .backends
                .iter()
                .find(|backend| &backend.id == active_id)
        });
        let active_backend_label = active_backend
            .map(|backend| backend.label.clone())
            .unwrap_or_else(|| "local".to_string());
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

        if let Some(agent) = state.active_agent.as_ref() {
            workspace
                .widget(cx, ids!(ai_chat_markdown))
                .set_text(cx, &ai_chat_markdown(agent));
            workspace.label(cx, ids!(ai_status_label)).set_text(
                cx,
                if active_backend_configured {
                    &agent.status
                } else {
                    "Configure backend"
                },
            );
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
                    workspace.fold_header(cx, ids!(ai_swarm_fold)).set_is_open(
                        cx,
                        true,
                        Animate::Yes,
                    );
                    workspace.fold_header(cx, ids!(ai_live_fold)).set_is_open(
                        cx,
                        true,
                        Animate::Yes,
                    );
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
}

fn ai_chat_markdown(agent: &AiAgentState) -> String {
    if agent.messages.is_empty() {
        return "_No messages yet._".to_string();
    }
    let mut markdown = String::new();
    let mut activity = Vec::new();
    for message in &agent.messages {
        if let Some(item) = ai_activity_item(message) {
            if !item.text.is_empty() {
                activity.push(item);
            }
            continue;
        }
        append_activity_markdown(&mut markdown, &activity, false, agent.pending);
        activity.clear();

        let heading = ai_main_message_heading(message);
        let body = ai_main_message_markdown_body(message);
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
    append_activity_markdown(&mut markdown, &activity, true, agent.pending);
    markdown
}

fn ai_main_message_heading(message: &AiMessage) -> &'static str {
    match message.role {
        AiMessageRole::User => "### User",
        AiMessageRole::Assistant => "### Assistant",
        AiMessageRole::System => "### System",
        AiMessageRole::Thinking => "### Thinking",
        AiMessageRole::ToolCall | AiMessageRole::ToolResult => "### Tool",
        AiMessageRole::Error => "### Error",
    }
}

fn ai_main_message_markdown_body(message: &AiMessage) -> String {
    message.text.trim().to_string()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AiActivityKind {
    Thinking,
    Waiting,
    Observation,
    Tool,
    Event,
}

#[derive(Clone, Debug)]
struct AiActivityItem {
    kind: AiActivityKind,
    text: String,
}

#[derive(Clone, Debug)]
struct AiActivityRun {
    kind: AiActivityKind,
    text: String,
    count: usize,
}

fn ai_activity_item(message: &AiMessage) -> Option<AiActivityItem> {
    match message.role {
        AiMessageRole::Thinking => {
            if let Some(waiting) = message.text.strip_prefix(AI_WAITING_MESSAGE_PREFIX) {
                let waiting = normalize_activity_block_text(waiting);
                let text = if waiting.is_empty() {
                    "waiting".to_string()
                } else {
                    waiting
                };
                return Some(AiActivityItem {
                    kind: AiActivityKind::Waiting,
                    text,
                });
            }
            let thinking = normalize_activity_block_text(message.text.trim());
            let text = if thinking.is_empty() {
                "thinking".to_string()
            } else {
                thinking
            };
            Some(AiActivityItem {
                kind: AiActivityKind::Thinking,
                text,
            })
        }
        AiMessageRole::ToolCall => Some(AiActivityItem {
            kind: AiActivityKind::Tool,
            text: clean_activity_text(&summarize_tool_call_message(&message.text)),
        }),
        AiMessageRole::ToolResult => Some(AiActivityItem {
            kind: AiActivityKind::Tool,
            text: clean_activity_text(&summarize_tool_result_message(&message.text)),
        }),
        AiMessageRole::User if message.text.starts_with(AI_TASK_EVENT_PREFIX) => {
            Some(AiActivityItem {
                kind: AiActivityKind::Event,
                text: summarize_task_event_inline(&message.text),
            })
        }
        AiMessageRole::System if message.text.starts_with(AI_TERMINAL_OBSERVATION_PREFIX) => {
            Some(AiActivityItem {
                kind: AiActivityKind::Observation,
                text: summarize_terminal_observation_inline(&message.text),
            })
        }
        _ => None,
    }
}

fn append_activity_markdown(
    markdown: &mut String,
    items: &[AiActivityItem],
    is_trailing: bool,
    agent_pending: bool,
) {
    let runs = compact_activity_runs(items);
    if runs.is_empty() {
        return;
    }

    let mut deferred_blocks = Vec::new();
    let mut inline_runs = Vec::new();
    for run in &runs {
        if activity_kind_uses_scroll_block(run.kind) {
            deferred_blocks.push(run);
        } else {
            inline_runs.push(run);
        }
    }

    if !inline_runs.is_empty() {
        if !markdown.is_empty() {
            markdown.push_str("\n\n");
        }
        markdown.push_str("> **");
        let inline_label = if inline_runs
            .iter()
            .all(|run| run.kind == AiActivityKind::Tool)
        {
            "Tools"
        } else {
            ai_activity_group_label(items, is_trailing, agent_pending)
        };
        markdown.push_str(inline_label);
        markdown.push_str("**");
        for run in inline_runs {
            markdown.push_str(" - ");
            markdown.push_str(&activity_run_text(run));
        }
    }

    for run in deferred_blocks {
        if !markdown.is_empty() {
            markdown.push_str("\n\n");
        }
        markdown.push_str("> **");
        markdown.push_str(ai_activity_kind_label(run.kind));
        markdown.push_str("**");
        markdown.push_str("\n\n```runsplash\n");
        markdown.push_str(&sanitize_fenced_text(&activity_run_text(run)));
        markdown.push_str("\n```");
    }
}

fn compact_activity_runs(items: &[AiActivityItem]) -> Vec<AiActivityRun> {
    let mut runs: Vec<AiActivityRun> = Vec::new();
    for item in items.iter().filter(|item| !item.text.is_empty()) {
        if let Some(last) = runs.last_mut() {
            if last.kind == item.kind && last.text == item.text {
                last.count += 1;
                continue;
            }
        }
        runs.push(AiActivityRun {
            kind: item.kind,
            text: item.text.clone(),
            count: 1,
        });
    }
    runs
}

fn activity_run_text(run: &AiActivityRun) -> String {
    if run.count > 1 {
        format!("{} x{}", run.text, run.count)
    } else {
        run.text.clone()
    }
}

fn activity_kind_uses_scroll_block(kind: AiActivityKind) -> bool {
    matches!(
        kind,
        AiActivityKind::Thinking
            | AiActivityKind::Waiting
            | AiActivityKind::Observation
            | AiActivityKind::Event
    )
}

fn ai_activity_kind_label(kind: AiActivityKind) -> &'static str {
    match kind {
        AiActivityKind::Thinking => "Thinking",
        AiActivityKind::Waiting => "Waiting",
        AiActivityKind::Observation => "Observation",
        AiActivityKind::Tool => "Tool",
        AiActivityKind::Event => "Event",
    }
}

fn ai_activity_group_label(
    items: &[AiActivityItem],
    is_trailing: bool,
    agent_pending: bool,
) -> &'static str {
    if items
        .iter()
        .any(|item| item.kind == AiActivityKind::Waiting)
    {
        return "Waiting";
    }
    if is_trailing
        && agent_pending
        && items
            .iter()
            .any(|item| item.kind == AiActivityKind::Thinking)
    {
        return "Thinking";
    }
    if items
        .iter()
        .all(|item| item.kind == AiActivityKind::Observation)
    {
        return "Observation";
    }
    if items.iter().all(|item| item.kind == AiActivityKind::Tool) {
        return "Tools";
    }
    "Activity"
}

fn summarize_task_event_inline(text: &str) -> String {
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            !line.starts_with("Continue supervising this delegated terminal task")
                && !line.starts_with("Continue supervising this observed terminal task")
                && !line.starts_with("Latest output excerpt:")
                && *line != "```text"
                && *line != "```"
        })
        .take(3)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return truncate_inline(text.trim(), AI_CHAT_COMPACT_MAX_CHARS);
    }
    let parts = lines
        .into_iter()
        .map(|line| {
            line.strip_prefix(AI_TASK_EVENT_PREFIX)
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .unwrap_or(line)
        })
        .collect::<Vec<_>>()
        .join(" - ");
    truncate_inline(&clean_activity_text(&parts), AI_CHAT_ACTIVITY_MAX_CHARS)
}

fn summarize_terminal_observation_inline(text: &str) -> String {
    let mut parts = Vec::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            !line.starts_with("Latest output excerpt:") && *line != "```text" && *line != "```"
        })
    {
        if let Some(path) = line.strip_prefix(AI_TERMINAL_OBSERVATION_PREFIX) {
            let path = path.trim();
            if !path.is_empty() {
                parts.push(format!("`{}`", truncate_inline(path, 80)));
            }
        } else if let Some(mode) = line.strip_prefix("Mode:") {
            parts.push(format!("mode {}", mode.trim()));
        } else if let Some(status) = line.strip_prefix("Codex status:") {
            parts.push(status.trim().to_string());
        } else if parts.len() < 2 {
            parts.push(line.to_string());
        }
        if parts.len() >= 3 {
            break;
        }
    }
    if parts.is_empty() {
        return truncate_inline(text.trim(), AI_CHAT_COMPACT_MAX_CHARS);
    }
    truncate_inline(
        &clean_activity_text(&parts.join(" - ")),
        AI_CHAT_ACTIVITY_MAX_CHARS,
    )
}

fn clean_activity_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_activity_block_text(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn sanitize_fenced_text(text: &str) -> String {
    text.replace("```", "'''")
}

fn summarize_tool_call_message(text: &str) -> String {
    let Some(tool_name) = extract_tool_name(text) else {
        return truncate_inline(text.trim(), AI_CHAT_COMPACT_MAX_CHARS);
    };
    if tool_name == "read_terminal" {
        return String::new();
    }
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
            if text.starts_with("`read_terminal` failed") {
                "Read terminal failed".to_string()
            } else {
                "Read terminal".to_string()
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

    fn test_agent_summary(
        agent_id: AiAgentId,
        title: &str,
        message_count: usize,
    ) -> AiAgentSummary {
        AiAgentSummary {
            agent_id,
            title: title.to_string(),
            backend_id: "local".to_string(),
            status: "idle".to_string(),
            pending: false,
            updated_at: 0.0,
            message_count,
            parent_agent_id: None,
            role: None,
            current_action: None,
            last_terminal_excerpt: None,
            files_touched: Vec::new(),
            active_terminal_path: None,
            active_terminal_title: None,
            state_changed_at: 0.0,
            workflow_step_name: None,
            workflow_step_status: None,
            blocked_reason: None,
        }
    }

    fn test_agent_state(
        agent_id: AiAgentId,
        status: &str,
        pending: bool,
        messages: Vec<AiMessage>,
    ) -> AiAgentState {
        AiAgentState {
            agent_id,
            title: "Chat 1".to_string(),
            backend_id: "local".to_string(),
            status: status.to_string(),
            pending,
            messages,
            parent_agent_id: None,
            role: None,
            subagents: Vec::new(),
            current_action: None,
            last_terminal_excerpt: None,
            files_touched: Vec::new(),
            active_terminal_path: None,
            active_terminal_title: None,
            state_changed_at: 0.0,
            workflow_step_name: None,
            workflow_step_status: None,
            blocked_reason: None,
        }
    }

    #[test]
    fn apply_local_prompt_echo_updates_visible_agent_immediately() {
        let agent_id = AiAgentId(7);
        let mut state = AiMountState {
            backends: vec![AiBackendInfo {
                id: "local".to_string(),
                label: "Local".to_string(),
                detail: String::new(),
                configured: true,
                configuration_url: None,
                configuration_hint: None,
            }],
            active_backend_id: Some("local".to_string()),
            active_agent_id: Some(agent_id),
            agents: vec![test_agent_summary(agent_id, "Chat 1", 0)],
            active_agent: Some(test_agent_state(agent_id, "idle", false, Vec::new())),
            live_markdown: String::new(),
            active_workflow: None,
            visibility_events: Vec::new(),
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
    fn ai_chat_markdown_renders_waiting_messages_as_waiting() {
        let agent = test_agent_state(
            AiAgentId(1),
            "thinking...",
            true,
            vec![AiMessage {
                role: AiMessageRole::Thinking,
                text: format!(
                    "{}waiting on `makepad/.makepad/hello-world-makepad.term`",
                    AI_WAITING_MESSAGE_PREFIX
                ),
            }],
        );

        let markdown = ai_chat_markdown(&agent);
        assert!(markdown.contains("> **Waiting**"));
        assert!(markdown.contains("```runsplash"));
        assert!(!markdown.contains("### Thinking"));
        assert!(markdown.contains("waiting on `makepad/.makepad/hello-world-makepad.term`"));
    }

    #[test]
    fn ai_chat_markdown_renders_terminal_observation_messages() {
        let agent = test_agent_state(
            AiAgentId(1),
            "ready",
            false,
            vec![AiMessage {
                role: AiMessageRole::System,
                text: format!(
                    "{} makepad/.makepad/manual-codex.term\nMode: working\nCodex status: Working (3s)",
                    AI_TERMINAL_OBSERVATION_PREFIX
                ),
            }],
        );

        let markdown = ai_chat_markdown(&agent);
        assert!(markdown.contains("> **Observation**"));
        assert!(markdown.contains("```runsplash"));
        assert!(markdown.contains("`makepad/.makepad/manual-codex.term`"));
        assert!(markdown.contains("mode working"));
        assert!(markdown.contains("Working (3s)"));
    }

    #[test]
    fn read_terminal_tool_messages_are_compact() {
        let call = "`read_terminal`\n```json\n{\"path\":\"makepad/.makepad/hello-world-makepad.term\"}\n```";
        let result = "`read_terminal` result\n```text\n{\"path\":\"makepad/.makepad/hello-world-makepad.term\",\"mode\":\"done\",\"summary\":\"finished\"}\n```";
        let failed = "`read_terminal` failed\n```text\nunknown terminal\n```";

        assert_eq!(summarize_tool_call_message(call), "");
        assert_eq!(summarize_tool_result_message(result), "Read terminal");
        assert_eq!(
            summarize_tool_result_message(failed),
            "Read terminal failed"
        );
    }

    #[test]
    fn ai_chat_markdown_groups_activity_before_assistant() {
        let agent = test_agent_state(
            AiAgentId(1),
            "ready",
            false,
            vec![
                AiMessage {
                    role: AiMessageRole::User,
                    text: "add a button".to_string(),
                },
                AiMessage {
                    role: AiMessageRole::Thinking,
                    text: "I should inspect the example first.".to_string(),
                },
                AiMessage {
                    role: AiMessageRole::ToolResult,
                    text: "`read_terminal` result\n```text\n{}\n```".to_string(),
                },
                AiMessage {
                    role: AiMessageRole::ToolResult,
                    text: "`read_terminal` result\n```text\n{}\n```".to_string(),
                },
                AiMessage {
                    role: AiMessageRole::Assistant,
                    text: "Done.".to_string(),
                },
            ],
        );

        let markdown = ai_chat_markdown(&agent);
        assert!(markdown.contains("### User"));
        assert!(markdown.contains("> **Thinking**"));
        assert!(markdown.contains("```runsplash"));
        assert!(markdown.contains("> **Tools**"));
        assert!(markdown.contains("Read terminal x2"));
        assert!(markdown.contains("### Assistant"));
        assert_eq!(markdown.matches("### Tool").count(), 0);
    }

    #[test]
    fn ai_chat_markdown_does_not_hide_activity_behind_more_count() {
        let mut messages = Vec::new();
        for index in 0..8 {
            messages.push(AiMessage {
                role: AiMessageRole::Thinking,
                text: format!("thought line {}", index),
            });
        }
        let agent = test_agent_state(AiAgentId(1), "thinking...", true, messages);

        let markdown = ai_chat_markdown(&agent);
        assert!(!markdown.contains("more"));
        assert_eq!(markdown.matches("```runsplash").count(), 8);
        assert!(markdown.contains("thought line 7"));
    }
}

fn ai_task_board_markdown(state: &AiMountState) -> String {
    let has_workflow = state.active_workflow.is_some();
    if state.agents.is_empty() && !has_workflow {
        return "_No active tasks._".to_string();
    }

    let mut markdown = String::new();
    if let Some(workflow) = state.active_workflow.as_ref() {
        let total_steps = workflow.steps.len();
        let current_step = if total_steps == 0 {
            0
        } else {
            workflow.current_step.saturating_add(1).min(total_steps)
        };
        markdown.push_str(&format!(
            "**Workflow:** {}\n\nStep {}/{}\n",
            truncate_inline(&workflow.name, AI_TASK_BOARD_WORKFLOW_NAME_MAX_CHARS),
            current_step,
            total_steps
        ));
        let current_step_index = workflow.current_step.min(total_steps.saturating_sub(1));
        let visible_start = if total_steps > AI_TASK_BOARD_WORKFLOW_MAX_STEPS {
            current_step_index
                .saturating_add(1)
                .saturating_sub(AI_TASK_BOARD_WORKFLOW_MAX_STEPS)
        } else {
            0
        };
        let visible_end = total_steps.min(visible_start + AI_TASK_BOARD_WORKFLOW_MAX_STEPS);
        if visible_start > 0 {
            markdown.push_str(&format!("... {} more steps\n", visible_start));
        }
        for step in &workflow.steps[visible_start..visible_end] {
            markdown.push_str(&format!(
                "- {} {}\n",
                workflow_step_marker(&step.status),
                truncate_inline(&step.name, AI_TASK_BOARD_WORKFLOW_STEP_MAX_CHARS)
            ));
        }
        let omitted_after = total_steps.saturating_sub(visible_end);
        if omitted_after > 0 {
            markdown.push_str(&format!("... {} more steps\n", omitted_after));
        }
        markdown.push('\n');
    }

    if state.agents.is_empty() {
        markdown.push_str("_No active tasks._");
        return markdown;
    }

    let active_count = state.agents.iter().filter(|agent| agent.pending).count();
    let idle_count = state.agents.len().saturating_sub(active_count);
    let message_count: usize = state.agents.iter().map(|agent| agent.message_count).sum();
    markdown.push_str(&format!(
        "**{} chats** - **{} active** - {} idle - {} msgs\n\n",
        state.agents.len(),
        active_count,
        idle_count,
        message_count
    ));

    let roots: Vec<_> = state
        .agents
        .iter()
        .filter(|agent| agent.parent_agent_id.is_none())
        .collect();

    for root in roots {
        render_task_board_agent(&mut markdown, root, state, 0);
    }

    markdown
}

fn workflow_step_marker(status: &str) -> &'static str {
    let status = status.trim();
    if status.eq_ignore_ascii_case("done") || status.eq_ignore_ascii_case("completed") {
        "✓"
    } else if status.eq_ignore_ascii_case("active")
        || status.eq_ignore_ascii_case("running")
        || status.eq_ignore_ascii_case("current")
    {
        "▶"
    } else if status.eq_ignore_ascii_case("failed") || status.eq_ignore_ascii_case("error") {
        "✗"
    } else {
        "○"
    }
}

fn render_task_board_agent(
    markdown: &mut String,
    agent: &AiAgentSummary,
    state: &AiMountState,
    depth: usize,
) {
    let indent = if depth == 0 {
        "".to_string()
    } else {
        format!("{}└─ ", "  ".repeat(depth - 1))
    };

    let is_active = state.active_agent_id == Some(agent.agent_id);
    let state_label = if agent.pending {
        "running"
    } else if agent.status == "completed" {
        "done"
    } else if agent.status == "cancelled" {
        "cancelled"
    } else if agent.status.contains("error") {
        "error"
    } else {
        "idle"
    };
    let role = agent
        .role
        .as_deref()
        .map(|role| format!(" - {}", role))
        .unwrap_or_default();
    let title = truncate_inline(&agent.title, 44);
    let status_chips = if is_active {
        format!("`selected` `{}`", state_label)
    } else {
        format!("`{}`", state_label)
    };

    markdown.push_str(&format!(
        "{}- **{}** - {} - {}{}\n",
        indent,
        title,
        status_chips,
        truncate_inline(&agent.status, 48),
        role
    ));

    let children: Vec<_> = state
        .agents
        .iter()
        .filter(|a| a.parent_agent_id == Some(agent.agent_id))
        .collect();

    for child in children {
        render_task_board_agent(markdown, child, state, depth + 1);
    }
}

fn ai_live_activity_markdown(state: &AiMountState) -> String {
    let live = state.live_markdown.trim();
    let mut markdown = if live.is_empty() {
        String::new()
    } else {
        live.lines()
            .map(polish_live_activity_line)
            .collect::<Vec<_>>()
            .join("\n")
    };

    append_live_agent_details(&mut markdown, state);
    if markdown.is_empty() {
        "_No live AI activity yet._".to_string()
    } else {
        markdown
    }
}

fn append_live_agent_details(markdown: &mut String, state: &AiMountState) {
    let mut wrote_header = false;
    for agent in state
        .agents
        .iter()
        .filter(|agent| agent.pending || state.active_agent_id == Some(agent.agent_id))
    {
        let has_action = agent
            .current_action
            .as_deref()
            .map(|action| !action.trim().is_empty())
            .unwrap_or(false);
        let has_terminal = agent
            .last_terminal_excerpt
            .as_deref()
            .map(|excerpt| !excerpt.trim().is_empty())
            .unwrap_or(false);
        let has_files = !agent.files_touched.is_empty();
        if !has_action && !has_terminal && !has_files {
            continue;
        }
        if !wrote_header {
            if !markdown.is_empty() {
                markdown.push_str("\n\n");
            }
            markdown.push_str("**Agents**\n\n");
            wrote_header = true;
        }
        markdown.push_str(&format!(
            "- **{}** - `{}`\n",
            truncate_inline(&agent.title, 44),
            live_agent_state_label(agent)
        ));
        if let Some(action) = agent.current_action.as_deref() {
            let action = action.trim();
            if !action.is_empty() {
                markdown.push_str(&format!("  action: {}\n", truncate_inline(action, 96)));
            }
        }
        if let Some(excerpt) = agent.last_terminal_excerpt.as_deref() {
            if let Some(line) = excerpt.lines().map(str::trim).find(|line| !line.is_empty()) {
                markdown.push_str(&format!("  terminal: {}\n", truncate_inline(line, 120)));
            }
        }
        if !agent.files_touched.is_empty() {
            markdown.push_str("  files touched: ");
            for (index, path) in agent.files_touched.iter().take(5).enumerate() {
                if index > 0 {
                    markdown.push_str(", ");
                }
                markdown.push_str(&format!("`{}`", truncate_inline(path, 80)));
            }
            let remaining = agent.files_touched.len().saturating_sub(5);
            if remaining > 0 {
                markdown.push_str(&format!(", +{} more", remaining));
            }
            markdown.push('\n');
        }
    }
    if wrote_header && markdown.ends_with('\n') {
        markdown.pop();
    }
}

fn live_agent_state_label(agent: &AiAgentSummary) -> &'static str {
    if agent.pending {
        "running"
    } else if agent.status == "completed" {
        "done"
    } else if agent.status == "cancelled" {
        "cancelled"
    } else if agent.status.contains("error") {
        "error"
    } else {
        "idle"
    }
}

fn polish_live_activity_line(line: &str) -> String {
    let trimmed = line.trim_end();
    if trimmed == "**Tasks**" || trimmed == "**Todo**" {
        return "**Todo**".to_string();
    }
    if trimmed == "**Terminals**" {
        return "**Active Terminals**".to_string();
    }
    if trimmed == "_No delegated terminal tasks yet._" {
        return "_No open AI todos._".to_string();
    }
    if trimmed == "_No terminal activity yet._" {
        return "_No active terminal activity._".to_string();
    }
    if let Some(rest) = trimmed.strip_prefix("- `T") {
        if let Some((id_and_status, goal)) = rest.split_once(']') {
            if let Some((id, status)) = id_and_status.split_once("` [") {
                return format!("- Task `T{}` - **{}** - {}", id, status, goal.trim());
            }
        }
    }
    if trimmed
        .trim_start()
        .starts_with("waiting for terminal assignment")
    {
        return "  terminal: waiting for assignment".to_string();
    }
    if let Some(path_line) = trimmed.trim_start().strip_prefix('`') {
        if let Some((path, rest)) = path_line.split_once("` [") {
            return format!("  terminal: `{}` - {}", path, rest.trim_end_matches(']'));
        }
    }
    if let Some(path_line) = trimmed.strip_prefix("- `") {
        if let Some((path, rest)) = path_line.split_once("` [") {
            return format!("- Terminal `{}` - {}", path, rest.trim_end_matches(']'));
        }
    }
    if let Some(files) = trimmed.trim_start().strip_prefix("files:") {
        return format!("  files touched: {}", files.trim());
    }
    if let Some(files) = trimmed.trim_start().strip_prefix("expecting:") {
        return format!("  expected files: {}", files.trim());
    }
    trimmed.to_string()
}

#[cfg(test)]
mod ai_task_board_tests {
    use super::*;
    use makepad_studio_protocol::hub_protocol::{
        ActiveWorkflowState, AiBackendInfo, WorkflowStepState,
    };

    fn summary(
        id: u64,
        title: &str,
        status: &str,
        pending: bool,
        parent: Option<AiAgentId>,
    ) -> AiAgentSummary {
        AiAgentSummary {
            agent_id: AiAgentId(id),
            title: title.to_string(),
            backend_id: "chatgpt".to_string(),
            status: status.to_string(),
            pending,
            updated_at: 0.0,
            message_count: 3,
            parent_agent_id: parent,
            role: None,
            current_action: None,
            last_terminal_excerpt: None,
            files_touched: Vec::new(),
            active_terminal_path: None,
            active_terminal_title: None,
            state_changed_at: 0.0,
            workflow_step_name: None,
            workflow_step_status: None,
            blocked_reason: None,
        }
    }

    fn mount_state(agents: Vec<AiAgentSummary>, live_markdown: &str) -> AiMountState {
        AiMountState {
            backends: vec![AiBackendInfo {
                id: "chatgpt".to_string(),
                label: "ChatGPT".to_string(),
                detail: "gpt".to_string(),
                configured: true,
                configuration_url: None,
                configuration_hint: None,
            }],
            active_backend_id: Some("chatgpt".to_string()),
            active_agent_id: Some(AiAgentId(1)),
            agents,
            active_agent: None,
            live_markdown: live_markdown.to_string(),
            active_workflow: None,
            visibility_events: Vec::new(),
        }
    }

    fn workflow_state() -> ActiveWorkflowState {
        ActiveWorkflowState {
            name: "review-prs".to_string(),
            current_step: 1,
            steps: vec![
                WorkflowStepState {
                    name: "Collect context".to_string(),
                    status: "done".to_string(),
                },
                WorkflowStepState {
                    name: "Review implementation".to_string(),
                    status: "active".to_string(),
                },
                WorkflowStepState {
                    name: "Summarize findings".to_string(),
                    status: "pending".to_string(),
                },
                WorkflowStepState {
                    name: "Post review".to_string(),
                    status: "failed".to_string(),
                },
            ],
        }
    }

    fn mount_state_with_workflow(
        agents: Vec<AiAgentSummary>,
        live_markdown: &str,
        active_workflow: Option<ActiveWorkflowState>,
    ) -> AiMountState {
        AiMountState {
            active_workflow,
            ..mount_state(agents, live_markdown)
        }
    }

    #[test]
    fn task_board_renders_active_workflow_and_preserves_agents() {
        let state = mount_state_with_workflow(
            vec![
                summary(1, "Review pull request", "thinking...", true, None),
                summary(2, "Check tests", "ready", false, Some(AiAgentId(1))),
            ],
            "",
            Some(workflow_state()),
        );

        let markdown = ai_task_board_markdown(&state);
        assert!(markdown.contains("**Workflow:** review-prs"));
        assert!(markdown.contains("Step 2/4"));
        assert!(markdown.contains("✓ Collect context"));
        assert!(markdown.contains("▶ Review implementation"));
        assert!(markdown.contains("○ Summarize findings"));
        assert!(markdown.contains("✗ Post review"));
        assert!(markdown.contains("**2 chats**"));
        assert!(markdown.contains("Review pull request"));
        assert!(markdown.contains("└─ - **Check tests**"));
    }

    #[test]
    fn task_board_bounds_and_truncates_active_workflow_steps() {
        let long_workflow_name = format!("{}{}", "workflow-", "w".repeat(120));
        let long_step_name = format!("{}{}", "step-", "s".repeat(120));
        let active_step_name = format!("{}{}", "active-step-", "a".repeat(120));
        let mut steps = (0..12)
            .map(|index| WorkflowStepState {
                name: if index == 11 {
                    active_step_name.clone()
                } else {
                    format!("{long_step_name}-{index}")
                },
                status: if index == 11 { "active" } else { "pending" }.to_string(),
            })
            .collect::<Vec<_>>();
        steps[10].name = "penultimate bounded step".to_string();

        let state = mount_state_with_workflow(
            Vec::new(),
            "",
            Some(ActiveWorkflowState {
                name: long_workflow_name.clone(),
                current_step: 11,
                steps,
            }),
        );

        let markdown = ai_task_board_markdown(&state);
        assert!(markdown.contains(&format!(
            "**Workflow:** {}",
            truncate_inline(&long_workflow_name, 64)
        )));
        assert!(!markdown.contains(&long_workflow_name));
        assert!(markdown.contains("Step 12/12"));
        assert!(markdown.contains("... 2 more steps"));
        assert_eq!(markdown.matches("\n- ").count(), 10);
        assert!(markdown.contains(&truncate_inline(&active_step_name, 96)));
        assert!(!markdown.contains(&active_step_name));
        assert!(markdown.contains("▶ active-step-"));
    }

    #[test]
    fn task_board_marks_active_running_and_children() {
        let state = mount_state(
            vec![
                summary(1, "Plan Studio tasks", "thinking...", true, None),
                summary(2, "Review task UI", "ready", false, Some(AiAgentId(1))),
            ],
            "",
        );

        let markdown = ai_task_board_markdown(&state);
        assert!(markdown.contains("**2 chats**"));
        assert!(markdown.contains("**1 active**"));
        assert!(markdown.contains("`selected` `running`"));
        assert!(markdown.contains("Plan Studio tasks"));
        assert!(markdown.contains("Review task UI"));
        assert!(markdown.contains("6 msgs"));
    }

    #[test]
    fn live_activity_renders_agent_action_terminal_excerpt_and_files() {
        let mut agent = summary(1, "Task 4", "running", true, None);
        agent.current_action = Some("Editing ai_manager.rs".to_string());
        agent.last_terminal_excerpt =
            Some("cargo test -p makepad-studio ai_task_board_tests\nok".to_string());
        agent.files_touched = vec![
            "studio/desktop/src/ai_manager.rs".to_string(),
            "studio/desktop/src/app.rs".to_string(),
        ];
        let state = mount_state(vec![agent], "**Tasks**\n\n- `T1` [working] Upgrade UI");

        let markdown = ai_live_activity_markdown(&state);
        assert!(markdown.contains("**Todo**"));
        assert!(markdown.contains("**Agents**"));
        assert!(markdown.contains("**Task 4**"));
        assert!(markdown.contains("action: Editing ai_manager.rs"));
        assert!(markdown.contains("terminal: cargo test -p makepad-studio ai_task_board_tests"));
        assert!(markdown.contains(
            "files touched: `studio/desktop/src/ai_manager.rs`, `studio/desktop/src/app.rs`"
        ));
    }
    #[test]
    fn live_activity_polishes_task_and_terminal_labels() {
        let state = mount_state(
            Vec::new(),
            "**Tasks**\n\n- `T1` [working] Build task UI\n  `makepad/.makepad/task.term` [Working]\n  expecting: studio/desktop/src/ai_manager.rs\n\n**Terminals**\n\n- `makepad/.makepad/task.term` [working / codex]\n  Working (3s)",
        );

        let markdown = ai_live_activity_markdown(&state);
        assert!(markdown.contains("**Todo**"));
        assert!(markdown.contains("- Task `T1` - **working** - Build task UI"));
        assert!(markdown.contains("terminal: `makepad/.makepad/task.term` - Working"));
        assert!(markdown.contains("expected files: studio/desktop/src/ai_manager.rs"));
        assert!(markdown.contains("- Terminal `makepad/.makepad/task.term` - working / codex"));
    }
}
