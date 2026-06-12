use makepad_studio_protocol::ai_format::{
    parse_json_bool_field, parse_json_string_field, AI_TASK_EVENT_PREFIX,
    AI_TERMINAL_OBSERVATION_PREFIX, AI_WAITING_MESSAGE_PREFIX,
};
use makepad_studio_protocol::hub_protocol::{
    AiAgentId, AiAgentState, AiAgentSummary, AiMessage, AiMessageRole, AiMountState,
};

pub const AI_CHAT_SCROLL_SETTLE_FRAMES: u8 = 4;
pub const AI_CHAT_COMPACT_MAX_CHARS: usize = 220;
pub const AI_CHAT_ACTIVITY_MAX_CHARS: usize = 140;
pub const AI_TASK_BOARD_WORKFLOW_NAME_MAX_CHARS: usize = 64;
pub const AI_TASK_BOARD_WORKFLOW_STEP_MAX_CHARS: usize = 96;
pub const AI_TASK_BOARD_WORKFLOW_MAX_STEPS: usize = 10;
pub const AI_SUBAGENT_ROLES: &[&str] = &["coder", "planner", "explorer", "reviewer", "verifier"];

pub fn ai_chat_markdown(agent: &AiAgentState) -> String {
    if agent.messages.is_empty() {
        return "_No messages yet._".to_string();
    }
    let mut markdown = String::new();
    let mut activity = Vec::new();
    for message in &agent.messages {
        if let Some(item) = ai_activity_item(message) {
            if matches!(
                item.kind,
                AiActivityKind::Observation | AiActivityKind::Event
            ) {
                continue;
            }
            if !item.text.is_empty() {
                activity.push(item);
            }
            continue;
        }
        append_activity_markdown(&mut markdown, &activity, false, agent.pending);
        activity.clear();

        let body = ai_main_message_markdown_body(message);
        if body.is_empty() {
            continue;
        }
        append_main_message_markdown(&mut markdown, message, &body);
    }
    append_activity_markdown(&mut markdown, &activity, true, agent.pending);
    markdown
}

pub fn ai_agent_picker_label(agent: &AiAgentSummary, state: &AiMountState) -> String {
    let mut label = String::new();
    let depth = agent_depth(agent, state);
    for _ in 0..depth {
        label.push_str("  ");
    }
    if depth > 0 {
        label.push_str("↳ ");
    }
    label.push_str(&truncate_inline(&agent.title, 42));
    if let Some(role) = agent.role.as_deref().filter(|role| !role.trim().is_empty()) {
        label.push_str(" · ");
        label.push_str(&truncate_inline(role, 18));
    }
    if agent.pending {
        label.push_str(" · running");
    } else if agent.status == "completed" {
        label.push_str(" · done");
    } else if agent.status.contains("error") {
        label.push_str(" · error");
    }
    label
}

fn agent_depth(agent: &AiAgentSummary, state: &AiMountState) -> usize {
    let mut depth = 0;
    let mut parent_id = agent.parent_agent_id;
    while let Some(id) = parent_id {
        depth += 1;
        if depth >= 4 {
            break;
        }
        parent_id = state
            .agents
            .iter()
            .find(|candidate| candidate.agent_id == id)
            .and_then(|candidate| candidate.parent_agent_id);
    }
    depth
}

fn ai_main_message_label(message: &AiMessage) -> &'static str {
    match message.role {
        AiMessageRole::User => "User",
        AiMessageRole::Assistant => "Assistant",
        AiMessageRole::System => "System",
        AiMessageRole::Thinking => "Thinking",
        AiMessageRole::ToolCall | AiMessageRole::ToolResult => "Tool",
        AiMessageRole::Error => "Error",
    }
}

fn append_main_message_markdown(markdown: &mut String, message: &AiMessage, body: &str) {
    if !markdown.is_empty() {
        markdown.push_str("\n\n");
    }
    markdown.push_str("> **");
    markdown.push_str(ai_main_message_label(message));
    markdown.push_str("**");
    for line in body.lines() {
        markdown.push_str("\n> ");
        markdown.push_str(line);
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
            let text = normalize_activity_block_text(message.text.trim());
            if text.is_empty() {
                return None;
            }
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
        "open_editor" => "opened editor".to_string(),
        "list_terminals" => "listed terminals".to_string(),
        "bash" => {
            if payload.contains("failed") || payload.contains("error") || payload.contains("Error")
            {
                "ran bash command (failed)".to_string()
            } else {
                "ran bash command".to_string()
            }
        }
        "read_file" | "read" => "read file".to_string(),
        "write_file" | "write" => "wrote file".to_string(),
        "search_text" | "search" => "searched codebase".to_string(),
        "ast_grep" => "searched syntax tree".to_string(),
        "lsp" => "queried code intelligence".to_string(),
        "edit" => "edited file".to_string(),
        "ast_edit" => "edited syntax tree".to_string(),
        _ => truncate_inline(payload.trim(), 80),
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

pub fn apply_local_prompt_echo(state: &mut AiMountState, agent_id: AiAgentId, prompt: &str) {
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

pub fn native_delegation_echo(role: &str, prompt: &str) -> String {
    native_delegation_prompt(role, prompt)
}

pub fn native_delegation_prompt(role: &str, task: &str) -> String {
    let role = role.trim();
    let role = if role.is_empty() { "agent" } else { role };
    format!("Native {} subagent\n\n{}", role, task.trim())
}

pub fn subagent_kickoff_prompt(role: &str, task: &str) -> String {
    format!(
        "You are the `{}` subagent.\n\nTask:\n{}\n\nWork only on this task. Use tools as needed. When finished, call `complete_task` with success and a concise summary so the parent agent can continue.",
        role.trim(),
        task.trim()
    )
}

pub fn ai_file_link_path_from_href(href: &str) -> Option<String> {
    href.strip_prefix("makepad-studio-file:")
        .map(|path| {
            path.replace("%20", " ")
                .replace("%29", ")")
                .replace("%28", "(")
        })
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty() && !path.contains(".."))
}

pub fn ai_subagent_role_label(role: &str) -> String {
    format!("{} agent", role)
}

pub fn ai_status_label(agent: &AiAgentState, role_index: usize) -> String {
    if agent.pending {
        return agent.status.clone();
    }
    let role = AI_SUBAGENT_ROLES
        .get(role_index)
        .copied()
        .unwrap_or("coder");
    format!("{} · send starts {} agent", agent.status, role)
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

pub fn summarize_title(prompt: &str) -> String {
    let single_line = prompt
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .replace('\t', " ");
    if single_line.is_empty() {
        return String::new();
    }
    let mut title = single_line.chars().take(40).collect::<String>();
    if single_line.chars().count() > 40 {
        title.push_str("...");
    }
    title
}

pub fn parse_direct_subagent_command(prompt: &str) -> Option<(String, String)> {
    let trimmed = prompt.trim();
    let rest = trimmed
        .strip_prefix("/subagent")
        .or_else(|| trimmed.strip_prefix("/agent"))?
        .trim();
    if rest.is_empty() {
        return None;
    }

    let (role, task) = if let Some((role, task)) = rest.split_once(':') {
        (role.trim(), task.trim())
    } else {
        let mut parts = rest.splitn(2, char::is_whitespace);
        (parts.next()?.trim(), parts.next().unwrap_or("").trim())
    };

    if role.is_empty() || task.is_empty() {
        return None;
    }

    Some((role.to_string(), task.to_string()))
}

pub fn extract_expected_paths_from_prompt(prompt: &str) -> Vec<String> {
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

pub fn matches_expected_path(path: &str, expected_paths: &[String]) -> bool {
    expected_paths.iter().any(|expected| {
        path == expected || path.ends_with(&format!("/{}", expected)) || expected.ends_with(path)
    })
}

pub fn terminal_display_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

pub fn non_empty_labels(mut labels: Vec<String>, fallback: &str) -> Vec<String> {
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
    fn native_delegation_echo_hides_subagent_slash_command() {
        let echo = native_delegation_echo("coder", "implement the native activity board");

        assert_eq!(
            echo,
            "Native coder subagent\n\nimplement the native activity board"
        );
        assert!(!echo.contains("/subagent"));
    }

    #[test]
    fn subagent_prompts_are_shared_between_hub_and_desktop() {
        assert_eq!(
            native_delegation_prompt("coder", "implement native runs"),
            "Native coder subagent\n\nimplement native runs"
        );
        assert!(subagent_kickoff_prompt("coder", "implement native runs")
            .contains("You are the `coder` subagent."));
    }

    #[test]
    fn direct_subagent_command_accepts_agent_and_colon_forms() {
        assert_eq!(
            parse_direct_subagent_command("/subagent coder implement native runs"),
            Some(("coder".to_string(), "implement native runs".to_string()))
        );
        assert_eq!(
            parse_direct_subagent_command("/agent reviewer: inspect the hub diff"),
            Some(("reviewer".to_string(), "inspect the hub diff".to_string()))
        );
        assert_eq!(parse_direct_subagent_command("/subagent coder"), None);
        assert_eq!(
            parse_direct_subagent_command("subagent coder do work"),
            None
        );
    }

    #[test]
    fn title_summary_trims_and_truncates() {
        assert_eq!(summarize_title("  hello world  "), "hello world");
        assert_eq!(
            summarize_title("01234567890123456789012345678901234567890"),
            "0123456789012345678901234567890123456789..."
        );
    }

    #[test]
    fn expected_path_helpers_match_relative_targets() {
        assert_eq!(
            extract_expected_paths_from_prompt("tell codex to write a poem into `poem.txt`"),
            vec!["poem.txt".to_string()]
        );
        assert!(matches_expected_path(
            "src/poem.txt",
            &[String::from("poem.txt")]
        ));
        assert!(matches_expected_path(
            "poem.txt",
            &[String::from("poem.txt")]
        ));
    }

    #[test]
    fn subagent_role_labels_make_native_agent_mode_visible() {
        assert_eq!(ai_subagent_role_label("coder"), "coder agent");
        assert_eq!(ai_subagent_role_label("reviewer"), "reviewer agent");
    }

    #[test]
    fn idle_status_explains_native_send_target() {
        let mut agent = test_agent_state(AiAgentId(1), "ready", false, Vec::new());
        assert_eq!(
            ai_status_label(&agent, 1),
            "ready · send starts planner agent"
        );
        agent.pending = true;
        agent.status = "thinking...".to_string();
        assert_eq!(ai_status_label(&agent, 1), "thinking...");
    }

    #[test]
    fn ai_file_links_round_trip_to_editor_paths() {
        let markdown = ai_file_markdown_link("makepad", "studio/src/file with space.rs", 80);
        assert_eq!(
            markdown,
            "[studio/src/file with space.rs](makepad-studio-file:makepad/studio/src/file%20with%20space.rs)"
        );
        assert_eq!(
            ai_file_link_path_from_href(
                "makepad-studio-file:makepad/studio/src/file%20with%20space.rs"
            ),
            Some("makepad/studio/src/file with space.rs".to_string())
        );
        assert_eq!(ai_file_link_path_from_href("https://example.com"), None);
        assert_eq!(
            ai_file_link_path_from_href("makepad-studio-file:../secret"),
            None
        );
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
    fn ai_chat_markdown_renders_user_and_assistant_as_cards() {
        let agent = test_agent_state(
            AiAgentId(1),
            "ready",
            false,
            vec![
                AiMessage {
                    role: AiMessageRole::User,
                    text: "What enhancement should we make?".to_string(),
                },
                AiMessage {
                    role: AiMessageRole::Assistant,
                    text: "Use transcript cards for clearer separation.".to_string(),
                },
            ],
        );

        let markdown = ai_chat_markdown(&agent);
        assert!(markdown.contains("> **User**"));
        assert!(markdown.contains("> What enhancement should we make?"));
        assert!(markdown.contains("> **Assistant**"));
        assert!(markdown.contains("> Use transcript cards for clearer separation."));
        assert!(!markdown.contains("### User"));
        assert!(!markdown.contains("### Assistant"));
    }

    #[test]
    fn ai_chat_markdown_hides_placeholder_thinking_messages() {
        let agent = test_agent_state(
            AiAgentId(1),
            "ready",
            false,
            vec![
                AiMessage {
                    role: AiMessageRole::Assistant,
                    text: "First answer.".to_string(),
                },
                AiMessage {
                    role: AiMessageRole::Thinking,
                    text: String::new(),
                },
                AiMessage {
                    role: AiMessageRole::Thinking,
                    text: String::new(),
                },
                AiMessage {
                    role: AiMessageRole::Assistant,
                    text: "Second answer.".to_string(),
                },
            ],
        );

        let markdown = ai_chat_markdown(&agent);
        assert!(!markdown.contains("> **Thinking**"));
        assert!(!markdown.contains("thinking x2"));
        assert!(!markdown.contains("```runsplash"));
        assert!(markdown.contains("First answer."));
        assert!(markdown.contains("Second answer."));
    }

    #[test]
    fn ai_chat_markdown_hides_background_terminal_observations() {
        let agent = test_agent_state(
            AiAgentId(1),
            "ready",
            false,
            vec![
                AiMessage {
                    role: AiMessageRole::User,
                    text: "review the PR".to_string(),
                },
                AiMessage {
                    role: AiMessageRole::System,
                    text: format!(
                        "{} makepad/.makepad/manual-codex.term\nMode: working\nCodex status: Working (3s)",
                        AI_TERMINAL_OBSERVATION_PREFIX
                    ),
                },
                AiMessage {
                    role: AiMessageRole::Assistant,
                    text: "Still working.".to_string(),
                },
            ],
        );

        let markdown = ai_chat_markdown(&agent);
        assert!(!markdown.contains("> **Observation**"));
        assert!(!markdown.contains("```runsplash"));
        assert!(!markdown.contains("makepad/.makepad/manual-codex.term"));
        assert!(!markdown.contains("mode working"));
        assert!(markdown.contains("> **User**"));
        assert!(markdown.contains("> **Assistant**"));
    }

    #[test]
    fn ai_chat_markdown_keeps_tool_activity_between_cards() {
        let agent = test_agent_state(
            AiAgentId(1),
            "ready",
            false,
            vec![
                AiMessage {
                    role: AiMessageRole::User,
                    text: "Inspect the terminal.".to_string(),
                },
                AiMessage {
                    role: AiMessageRole::ToolResult,
                    text: "`read_terminal` result\n```text\n{}\n```".to_string(),
                },
                AiMessage {
                    role: AiMessageRole::Assistant,
                    text: "Terminal is idle.".to_string(),
                },
            ],
        );

        let markdown = ai_chat_markdown(&agent);
        let user = markdown.find("> **User**").unwrap();
        let tools = markdown.find("> **Tools**").unwrap();
        let assistant = markdown.find("> **Assistant**").unwrap();
        assert!(user < tools);
        assert!(tools < assistant);
        assert!(markdown.contains("Read terminal"));
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
    fn noisy_tool_results_are_summarized_without_payloads() {
        let list_terminals = "`list_terminals` result\n```text\n[{\"path\":\"makepad/.makepad/a.term\",\"name\":\"a.term\",\"mode\":\"idle\",\"summary\":\"wheregmis@Sahins-MacBook...\"}]\n```";
        let bash_failed = "`bash` result\n```text\ninvalid bash arguments: JSON Deserialize error: Key not found command, line:1 col:3\n```";
        let search = "`search` result\n```text\nhuge matched file output\n```";

        assert_eq!(
            summarize_tool_result_message(list_terminals),
            "listed terminals"
        );
        assert_eq!(
            summarize_tool_result_message(bash_failed),
            "ran bash command (failed)"
        );
        assert_eq!(summarize_tool_result_message(search), "searched codebase");
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
        assert!(markdown.contains("> **User**"));
        assert!(markdown.contains("> **Thinking**"));
        assert!(markdown.contains("```runsplash"));
        assert!(markdown.contains("> **Tools**"));
        assert!(markdown.contains("Read terminal x2"));
        assert!(markdown.contains("> **Assistant**"));
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

pub fn ai_task_board_markdown(mount: &str, state: &AiMountState) -> String {
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
            if step.status.eq_ignore_ascii_case("active")
                || step.status.eq_ignore_ascii_case("running")
                || step.status.eq_ignore_ascii_case("current")
            {
                for agent in &state.agents {
                    if agent.workflow_step_name.as_deref() == Some(&step.name) {
                        render_workflow_owner_agent(&mut markdown, mount, agent, state, 1);
                    }
                }
            }
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
        .filter(|agent| agent.parent_agent_id.is_none() && agent.workflow_step_name.is_none())
        .collect();

    for root in roots {
        render_task_board_agent(&mut markdown, mount, root, state, 0);
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
    mount: &str,
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
    let state_label = live_agent_state_label(agent);
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
    append_task_board_agent_timeline(markdown, mount, agent, depth);

    let children: Vec<_> = state
        .agents
        .iter()
        .filter(|a| a.parent_agent_id == Some(agent.agent_id))
        .collect();

    for child in children {
        render_task_board_agent(markdown, mount, child, state, depth + 1);
    }
}

fn render_workflow_owner_agent(
    markdown: &mut String,
    mount: &str,
    agent: &AiAgentSummary,
    state: &AiMountState,
    depth: usize,
) {
    let indent = format!("{}└─ ", "  ".repeat(depth - 1));
    let is_active = state.active_agent_id == Some(agent.agent_id);
    let state_label = live_agent_state_label(agent);

    let status_chips = if is_active {
        format!("`selected` `{}`", state_label)
    } else {
        format!("`{}`", state_label)
    };

    let action_str = if let Some(action) = agent.current_action.as_ref() {
        truncate_inline(action, 48)
    } else {
        truncate_inline(&agent.status, 48)
    };

    let step_info = if let Some(step_name) = agent.workflow_step_name.as_ref() {
        let step_status = agent.workflow_step_status.as_deref().unwrap_or("active");
        format!(
            " (Step: {} [{}])",
            truncate_inline(step_name, 20),
            step_status
        )
    } else {
        "".to_string()
    };

    markdown.push_str(&format!(
        "{}- **{}** - {} - {}{}\n",
        indent,
        truncate_inline(&agent.title, 44),
        status_chips,
        action_str,
        step_info
    ));
    append_task_board_agent_timeline(markdown, mount, agent, depth);

    let children: Vec<_> = state
        .agents
        .iter()
        .filter(|a| a.parent_agent_id == Some(agent.agent_id))
        .collect();

    for child in children {
        render_task_board_agent(markdown, mount, child, state, depth + 1);
    }
}

fn append_task_board_agent_timeline(
    markdown: &mut String,
    mount: &str,
    agent: &AiAgentSummary,
    depth: usize,
) {
    let detail_indent = format!("{}  ", "  ".repeat(depth + 1));
    let mut wrote_timeline = false;
    let mut append_line = |markdown: &mut String, label: &str, value: String| {
        let value = value.trim();
        if value.is_empty() {
            return;
        }
        if !wrote_timeline {
            markdown.push_str(&format!("{}timeline:\n", detail_indent));
            wrote_timeline = true;
        }
        markdown.push_str(&format!("{}- {}: {}\n", detail_indent, label, value));
    };

    if let Some(step_name) = agent.workflow_step_name.as_deref() {
        let step_status = agent.workflow_step_status.as_deref().unwrap_or("active");
        append_line(
            markdown,
            "step",
            format!(
                "{} [{}]",
                truncate_inline(step_name, 64),
                truncate_inline(step_status, 24)
            ),
        );
    }

    if let Some(reason) = agent.blocked_reason.as_deref() {
        append_line(markdown, "blocked", truncate_inline(reason, 96));
    }

    if let Some(action) = agent.current_action.as_deref() {
        append_line(markdown, "now", truncate_inline(action, 96));
    }

    if let Some(path) = agent.active_terminal_path.as_deref() {
        let title = agent.active_terminal_title.as_deref().unwrap_or("terminal");
        append_line(
            markdown,
            "terminal",
            format!(
                "`{}` ({})",
                truncate_inline(path, 80),
                truncate_inline(title, 40)
            ),
        );
    } else if let Some(excerpt) = agent.last_terminal_excerpt.as_deref() {
        if let Some(line) = excerpt.lines().map(str::trim).find(|line| !line.is_empty()) {
            append_line(markdown, "terminal", truncate_inline(line, 120));
        }
    }

    if !agent.files_touched.is_empty() {
        let mut files = String::new();
        for (index, path) in agent.files_touched.iter().take(4).enumerate() {
            if index > 0 {
                files.push_str(", ");
            }
            files.push_str(&ai_file_markdown_link(mount, path, 72));
        }
        let remaining = agent.files_touched.len().saturating_sub(4);
        if remaining > 0 {
            files.push_str(&format!(", +{} more", remaining));
        }
        append_line(markdown, "files", files);
    }
}

pub fn ai_live_activity_markdown(mount: &str, state: &AiMountState) -> String {
    let live = state.live_markdown.trim();
    let mut markdown = if live.is_empty() {
        String::new()
    } else {
        live.lines()
            .map(polish_live_activity_line)
            .collect::<Vec<_>>()
            .join("\n")
    };

    append_live_agent_details(&mut markdown, mount, state);
    append_recent_activity(&mut markdown, mount, state);
    if markdown.is_empty() {
        "_No live AI activity yet._".to_string()
    } else {
        markdown
    }
}

fn append_live_agent_details(markdown: &mut String, mount: &str, state: &AiMountState) {
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
        let has_terminal =
            agent.active_terminal_path.is_some() || agent.last_terminal_excerpt.is_some();
        let has_files = !agent.files_touched.is_empty();
        let has_blocked = agent.blocked_reason.is_some();
        if !has_action && !has_terminal && !has_files && !has_blocked {
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
        if let Some(reason) = agent.blocked_reason.as_deref() {
            let reason = reason.trim();
            if !reason.is_empty() {
                markdown.push_str(&format!("  blocked: {}\n", truncate_inline(reason, 96)));
            }
        }
        if let Some(action) = agent.current_action.as_deref() {
            let action = action.trim();
            if !action.is_empty() {
                markdown.push_str(&format!("  action: {}\n", truncate_inline(action, 96)));
            }
        }
        if let Some(path) = agent.active_terminal_path.as_deref() {
            let path = path.trim();
            if !path.is_empty() {
                let title = agent.active_terminal_title.as_deref().unwrap_or("Codex");
                markdown.push_str(&format!(
                    "  terminal: `{}` ({})\n",
                    truncate_inline(path, 80),
                    truncate_inline(title, 40)
                ));
            }
        } else if let Some(excerpt) = agent.last_terminal_excerpt.as_deref() {
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
                markdown.push_str(&ai_file_markdown_link(mount, path, 80));
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

fn append_recent_activity(markdown: &mut String, mount: &str, state: &AiMountState) {
    if state.visibility_events.is_empty() {
        return;
    }
    if !markdown.is_empty() {
        markdown.push_str("\n\n");
    }
    markdown.push_str("**Recent Activity**\n\n");
    for event in state.visibility_events.iter().rev().take(10) {
        let kind_label = match event.kind.as_str() {
            "step_activated" => "Step Active",
            "step_completed" => "Step Completed",
            "subagent_spawned" => "Subagent Started",
            "subagent_completed" => "Subagent Done",
            "native_tools_started" => "Native Tools Started",
            "native_tools_finished" => "Native Tools Finished",
            "terminal_attached" => "Terminal Open",
            "terminal_needs_input" => "Awaiting Input",
            "terminal_done" => "Terminal Done",
            "file_touched" => "File Touched",
            "agent_done" => "Agent Done",
            "agent_failed" => "Agent Failed",
            "workflow_failed" => "Workflow Failed",
            other => other,
        };
        let agent_label = event
            .agent_id
            .and_then(|agent_id| {
                state
                    .agents
                    .iter()
                    .find(|agent| agent.agent_id == agent_id)
                    .map(|agent| truncate_inline(&agent.title, 32))
            })
            .map(|title| format!(" - {}", title))
            .unwrap_or_default();
        let detail = if event.kind == "file_touched" {
            ai_file_markdown_link(mount, event.detail.trim(), 96)
        } else {
            truncate_inline(&event.detail, 96)
        };
        markdown.push_str(&format!(
            "- `[{}]` **{}**{} - {}\n",
            kind_label,
            truncate_inline(&event.title, 44),
            agent_label,
            detail
        ));
    }
    if markdown.ends_with('\n') {
        markdown.pop();
    }
}

pub fn ai_changed_files_markdown(mount: &str, state: &AiMountState) -> String {
    let mut files = Vec::<String>::new();
    for agent in &state.agents {
        for path in &agent.files_touched {
            push_unique_changed_file(&mut files, path);
        }
    }
    if let Some(agent) = state.active_agent.as_ref() {
        for path in &agent.files_touched {
            push_unique_changed_file(&mut files, path);
        }
    }
    for event in state.visibility_events.iter().rev() {
        if event.kind == "file_touched" {
            push_unique_changed_file(&mut files, &event.detail);
        }
    }

    if files.is_empty() {
        return "_No files changed yet._".to_string();
    }

    let mut markdown = String::new();
    markdown.push_str(&format!("**{} changed**\n\n", files.len()));
    for path in files.iter().take(8) {
        markdown.push_str("- ");
        markdown.push_str(&ai_file_markdown_link(mount, path, 88));
        markdown.push('\n');
    }
    let remaining = files.len().saturating_sub(8);
    if remaining > 0 {
        markdown.push_str(&format!("- +{} more", remaining));
    } else if markdown.ends_with('\n') {
        markdown.pop();
    }
    markdown
}

fn push_unique_changed_file(files: &mut Vec<String>, path: &str) {
    let path = path.trim();
    if path.is_empty() {
        return;
    }
    if !files.iter().any(|existing| existing == path) {
        files.push(path.to_string());
    }
}

pub fn ai_file_markdown_link(mount: &str, path: &str, max_chars: usize) -> String {
    let path = path.trim();
    if path.is_empty() {
        return String::new();
    }
    let virtual_path = if path.starts_with(&format!("{}/", mount)) {
        path.to_string()
    } else {
        format!("{}/{}", mount, path.trim_start_matches('/'))
    };
    let label = escape_markdown_link_label(&truncate_inline(path, max_chars));
    let href = escape_markdown_link_destination(&format!("makepad-studio-file:{}", virtual_path));
    format!("[{}]({})", label, href)
}

fn escape_markdown_link_label(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

fn escape_markdown_link_destination(text: &str) -> String {
    text.replace(' ', "%20")
        .replace(')', "%29")
        .replace('(', "%28")
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
        ActiveWorkflowState, AiBackendInfo, AiVisibilityEvent, WorkflowStepState,
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

        let markdown = ai_task_board_markdown("makepad", &state);
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

        let markdown = ai_task_board_markdown("makepad", &state);
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

        let markdown = ai_task_board_markdown("makepad", &state);
        assert!(markdown.contains("**2 chats**"));
        assert!(markdown.contains("**1 active**"));
        assert!(markdown.contains("`selected` `running`"));
        assert!(markdown.contains("Plan Studio tasks"));
        assert!(markdown.contains("Review task UI"));
        assert!(markdown.contains("6 msgs"));
    }

    #[test]
    fn task_board_keeps_completed_subagent_result_visible() {
        let root = summary(1, "Plan Studio tasks", "thinking...", true, None);
        let mut child = summary(2, "Coder Subagent", "completed", false, Some(AiAgentId(1)));
        child.role = Some("coder".to_string());
        child.current_action = Some("Completed: Updated the native activity board.".to_string());
        child.files_touched = vec!["studio/hub/src/ai_manager.rs".to_string()];

        let state = mount_state(vec![root, child], "");
        let markdown = ai_task_board_markdown("makepad", &state);

        assert!(markdown.contains("└─ - **Coder Subagent**"));
        assert!(markdown.contains("`done`"));
        assert!(markdown.contains("Completed: Updated the native activity board."));
        assert!(markdown.contains(
            "files: [studio/hub/src/ai_manager.rs](makepad-studio-file:makepad/studio/hub/src/ai_manager.rs)"
        ));
    }

    #[test]
    fn task_board_renders_failed_subagent_result_as_error() {
        let root = summary(1, "Plan Studio tasks", "thinking...", true, None);
        let mut child = summary(2, "Verifier Subagent", "error", false, Some(AiAgentId(1)));
        child.role = Some("verifier".to_string());
        child.current_action = Some("Failed: Regression test is failing.".to_string());
        child.blocked_reason = Some("Subagent reported task failure".to_string());

        let state = mount_state(vec![root, child], "");
        let markdown = ai_task_board_markdown("makepad", &state);

        assert!(markdown.contains("└─ - **Verifier Subagent**"));
        assert!(markdown.contains("`error`"));
        assert!(markdown.contains("blocked: Subagent reported task failure"));
        assert!(markdown.contains("Failed: Regression test is failing."));
    }

    #[test]
    fn task_board_renders_agent_timeline_state_details() {
        let mut root = summary(1, "Plan Studio tasks", "thinking...", true, None);
        root.current_action = Some("Editing Agent panel timeline".to_string());
        root.blocked_reason = Some("Waiting for CI logs".to_string());
        root.active_terminal_path = Some("makepad/.makepad/codex.term".to_string());
        root.active_terminal_title = Some("Codex Terminal".to_string());
        root.files_touched = vec![
            "studio/desktop/src/ai_manager.rs".to_string(),
            "platform/studio/src/hub_protocol.rs".to_string(),
        ];

        let mut child = summary(2, "Review task UI", "ready", false, Some(AiAgentId(1)));
        child.last_terminal_excerpt =
            Some("cargo test -p makepad-studio ai_task_board_tests\nok".to_string());

        let state = mount_state(vec![root, child], "");

        let markdown = ai_task_board_markdown("makepad", &state);
        assert!(markdown.contains("timeline:"));
        assert!(markdown.contains("- blocked: Waiting for CI logs"));
        assert!(markdown.contains("- now: Editing Agent panel timeline"));
        assert!(markdown.contains("- terminal: `makepad/.makepad/codex.term` (Codex Terminal)"));
        assert!(markdown.contains(
            "- files: [studio/desktop/src/ai_manager.rs](makepad-studio-file:makepad/studio/desktop/src/ai_manager.rs), [platform/studio/src/hub_protocol.rs](makepad-studio-file:makepad/platform/studio/src/hub_protocol.rs)"
        ));
        assert!(markdown.contains("cargo test -p makepad-studio ai_task_board_tests"));
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

        let markdown = ai_live_activity_markdown("makepad", &state);
        assert!(markdown.contains("**Todo**"));
        assert!(markdown.contains("**Agents**"));
        assert!(markdown.contains("**Task 4**"));
        assert!(markdown.contains("action: Editing ai_manager.rs"));
        assert!(markdown.contains("terminal: cargo test -p makepad-studio ai_task_board_tests"));
        assert!(markdown.contains(
            "files touched: [studio/desktop/src/ai_manager.rs](makepad-studio-file:makepad/studio/desktop/src/ai_manager.rs), [studio/desktop/src/app.rs](makepad-studio-file:makepad/studio/desktop/src/app.rs)"
        ));
    }
    #[test]
    fn live_activity_polishes_task_and_terminal_labels() {
        let state = mount_state(
            Vec::new(),
            "**Tasks**\n\n- `T1` [working] Build task UI\n  `makepad/.makepad/task.term` [Working]\n  expecting: studio/desktop/src/ai_manager.rs\n\n**Terminals**\n\n- `makepad/.makepad/task.term` [working / codex]\n  Working (3s)",
        );

        let markdown = ai_live_activity_markdown("makepad", &state);
        assert!(markdown.contains("**Todo**"));
        assert!(markdown.contains("- Task `T1` - **working** - Build task UI"));
        assert!(markdown.contains("terminal: `makepad/.makepad/task.term` - Working"));
        assert!(markdown.contains("expected files: studio/desktop/src/ai_manager.rs"));
        assert!(markdown.contains("- Terminal `makepad/.makepad/task.term` - working / codex"));
    }

    #[test]
    fn task_board_renders_active_workflow_owner_and_nested_subagents() {
        let mut owner = summary(1, "Workflow Owner", "running", true, None);
        owner.workflow_step_name = Some("Review implementation".to_string());
        owner.workflow_step_status = Some("active".to_string());
        owner.current_action = Some("Resolving PR comments".to_string());

        let child = summary(2, "Subagent Reviewer", "ready", false, Some(AiAgentId(1)));

        let state = mount_state_with_workflow(vec![owner, child], "", Some(workflow_state()));

        let markdown = ai_task_board_markdown("makepad", &state);
        assert!(markdown.contains("**Workflow:** review-prs"));
        assert!(markdown.contains("▶ Review implementation"));
        assert!(markdown.contains("└─ - **Workflow Owner**"));
        assert!(markdown.contains("`running`"));
        assert!(markdown.contains("Resolving PR comments"));
        assert!(markdown.contains("  └─ - **Subagent Reviewer**"));

        // Verify total chat count is shown, but owner is not listed at the bottom
        assert!(markdown.contains("**2 chats**"));
        assert!(!markdown.contains("\n- **Workflow Owner**")); // Only indented as └─
    }

    #[test]
    fn agent_picker_labels_show_subagent_hierarchy_and_status() {
        let root = summary(
            1,
            "Implement agent observability",
            "thinking...",
            true,
            None,
        );
        let mut child = summary(
            2,
            "Review instrumentation",
            "ready",
            false,
            Some(AiAgentId(1)),
        );
        child.role = Some("reviewer".to_string());
        let state = mount_state(vec![root.clone(), child.clone()], "");

        assert_eq!(
            ai_agent_picker_label(&root, &state),
            "Implement agent observability · running"
        );
        assert_eq!(
            ai_agent_picker_label(&child, &state),
            "  ↳ Review instrumentation · reviewer"
        );
    }

    #[test]
    fn live_activity_renders_recent_activity_events() {
        let state_agent = summary(1, "Workflow Owner", "running", true, None);
        let mut state = mount_state(vec![state_agent], "");
        state.visibility_events = vec![
            AiVisibilityEvent {
                kind: "step_activated".to_string(),
                agent_id: Some(AiAgentId(1)),
                title: "Resolve PR Set".to_string(),
                detail: "Step active".to_string(),
                timestamp: 100.0,
            },
            AiVisibilityEvent {
                kind: "subagent_spawned".to_string(),
                agent_id: Some(AiAgentId(1)),
                title: "Reviewer Subagent".to_string(),
                detail: "Workflow Owner delegated work to Reviewer Subagent".to_string(),
                timestamp: 101.0,
            },
            AiVisibilityEvent {
                kind: "native_tools_started".to_string(),
                agent_id: Some(AiAgentId(1)),
                title: "Reviewer Subagent".to_string(),
                detail: "Running `read_file`".to_string(),
                timestamp: 102.0,
            },
            AiVisibilityEvent {
                kind: "native_tools_finished".to_string(),
                agent_id: Some(AiAgentId(1)),
                title: "Reviewer Subagent".to_string(),
                detail: "`read_file` completed".to_string(),
                timestamp: 103.0,
            },
            AiVisibilityEvent {
                kind: "file_touched".to_string(),
                agent_id: Some(AiAgentId(1)),
                title: "File updated".to_string(),
                detail: "studio/desktop/src/ai_manager.rs".to_string(),
                timestamp: 104.0,
            },
        ];

        let markdown = ai_live_activity_markdown("makepad", &state);
        assert!(markdown.contains("**Recent Activity**"));
        // Newest-first check: order is reverse
        assert!(markdown.contains(
            "- `[File Touched]` **File updated** - Workflow Owner - [studio/desktop/src/ai_manager.rs](makepad-studio-file:makepad/studio/desktop/src/ai_manager.rs)"
        ));
        assert!(markdown.contains(
            "- `[Native Tools Finished]` **Reviewer Subagent** - Workflow Owner - `read_file` completed"
        ));
        assert!(markdown.contains(
            "- `[Native Tools Started]` **Reviewer Subagent** - Workflow Owner - Running `read_file`"
        ));
        assert!(markdown.contains(
            "- `[Subagent Started]` **Reviewer Subagent** - Workflow Owner - Workflow Owner delegated work to Reviewer Subagent"
        ));
        assert!(markdown
            .contains("- `[Step Active]` **Resolve PR Set** - Workflow Owner - Step active"));
    }

    #[test]
    fn live_activity_renders_agent_blocked_reason_and_active_terminal() {
        let mut agent = summary(1, "Task 4", "running", true, None);
        agent.blocked_reason = Some("Waiting for user to clarify PR bounds".to_string());
        agent.active_terminal_path = Some("repo/.makepad/codex.term".to_string());
        agent.active_terminal_title = Some("Codex Terminal".to_string());
        let state = mount_state(vec![agent], "");

        let markdown = ai_live_activity_markdown("makepad", &state);
        assert!(markdown.contains("**Agents**"));
        assert!(markdown.contains("blocked: Waiting for user to clarify PR bounds"));
        assert!(markdown.contains("terminal: `repo/.makepad/codex.term` (Codex Terminal)"));
    }

    #[test]
    fn changed_files_markdown_collects_native_agent_files_and_events() {
        let mut root = summary(1, "Root", "ready", false, None);
        root.files_touched = vec![
            "studio/desktop/src/ai_manager.rs".to_string(),
            "studio/hub/src/ai_manager.rs".to_string(),
        ];
        let mut child = summary(2, "Child", "completed", false, Some(AiAgentId(1)));
        child.files_touched = vec![
            "studio/hub/src/ai_manager.rs".to_string(),
            "platform/studio/src/hub_protocol.rs".to_string(),
        ];
        let mut state = mount_state(vec![root, child], "");
        state.visibility_events = vec![AiVisibilityEvent {
            kind: "file_touched".to_string(),
            agent_id: Some(AiAgentId(2)),
            title: "File updated".to_string(),
            detail: "studio/desktop/src/main.rs".to_string(),
            timestamp: 42.0,
        }];

        let markdown = ai_changed_files_markdown("makepad", &state);
        assert!(markdown.contains("**4 changed**"));
        assert!(markdown.contains(
            "- [studio/desktop/src/ai_manager.rs](makepad-studio-file:makepad/studio/desktop/src/ai_manager.rs)"
        ));
        assert_eq!(markdown.matches("studio/hub/src/ai_manager.rs").count(), 2);
        assert!(markdown.contains(
            "- [platform/studio/src/hub_protocol.rs](makepad-studio-file:makepad/platform/studio/src/hub_protocol.rs)"
        ));
        assert!(markdown.contains(
            "- [studio/desktop/src/main.rs](makepad-studio-file:makepad/studio/desktop/src/main.rs)"
        ));
    }

    #[test]
    fn changed_files_markdown_shows_empty_state() {
        let state = mount_state(Vec::new(), "");
        assert_eq!(
            ai_changed_files_markdown("makepad", &state),
            "_No files changed yet._"
        );
    }
}
