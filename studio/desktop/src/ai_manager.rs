use crate::{makepad_widgets::*, App};
use makepad_studio_protocol::hub_protocol::{
    AiAgentId, AiAgentState, AiMessage, AiMessageRole, AiMountState, ClientToHub,
};

const AI_CHAT_SCROLL_SETTLE_FRAMES: u8 = 4;
const AI_TASK_EVENT_PREFIX: &str = "TASK EVENT:";
const AI_WAITING_MESSAGE_PREFIX: &str = "WAITING:";
const AI_TERMINAL_OBSERVATION_PREFIX: &str = "TERMINAL OBSERVATION:";
const AI_CHAT_COMPACT_MAX_CHARS: usize = 220;
const AI_CHAT_ACTIVITY_MAX_CHARS: usize = 140;

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
                .map(|state| state.live_markdown.as_str())
                .unwrap_or("_No live AI state yet._"),
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

        if let Some(agent) = state.active_agent.as_ref() {
            workspace
                .widget(cx, ids!(ai_chat_markdown))
                .set_text(cx, &ai_chat_markdown(agent));
            workspace
                .label(cx, ids!(ai_status_label))
                .set_text(cx, &agent.status);
            workspace
                .button(cx, ids!(ai_run_button))
                .set_enabled(cx, true);
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
            live_markdown: String::new(),
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
        let agent = AiAgentState {
            agent_id: AiAgentId(1),
            title: "Chat 1".to_string(),
            backend_id: "local".to_string(),
            status: "thinking...".to_string(),
            pending: true,
            messages: vec![AiMessage {
                role: AiMessageRole::Thinking,
                text: format!(
                    "{}waiting on `makepad/.makepad/hello-world-makepad.term`",
                    AI_WAITING_MESSAGE_PREFIX
                ),
            }],
        };

        let markdown = ai_chat_markdown(&agent);
        assert!(markdown.contains("> **Waiting**"));
        assert!(markdown.contains("```runsplash"));
        assert!(!markdown.contains("### Thinking"));
        assert!(markdown.contains("waiting on `makepad/.makepad/hello-world-makepad.term`"));
    }

    #[test]
    fn ai_chat_markdown_renders_terminal_observation_messages() {
        let agent = AiAgentState {
            agent_id: AiAgentId(1),
            title: "Chat 1".to_string(),
            backend_id: "local".to_string(),
            status: "ready".to_string(),
            pending: false,
            messages: vec![AiMessage {
                role: AiMessageRole::System,
                text: format!(
                    "{} makepad/.makepad/manual-codex.term\nMode: working\nCodex status: Working (3s)",
                    AI_TERMINAL_OBSERVATION_PREFIX
                ),
            }],
        };

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
        let agent = AiAgentState {
            agent_id: AiAgentId(1),
            title: "Chat 1".to_string(),
            backend_id: "local".to_string(),
            status: "ready".to_string(),
            pending: false,
            messages: vec![
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
        };

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
        let agent = AiAgentState {
            agent_id: AiAgentId(1),
            title: "Chat 1".to_string(),
            backend_id: "local".to_string(),
            status: "thinking...".to_string(),
            pending: true,
            messages,
        };

        let markdown = ai_chat_markdown(&agent);
        assert!(!markdown.contains("more"));
        assert_eq!(markdown.matches("```runsplash").count(), 8);
        assert!(markdown.contains("thought line 7"));
    }
}
