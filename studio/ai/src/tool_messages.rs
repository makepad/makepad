use crate::{truncate_inline, ConversationItem, ToolCallRecord};
use makepad_micro_serde::*;
use makepad_studio_protocol::ai_format::{
    parse_json_string_field, AI_TERMINAL_OBSERVATION_PREFIX, AI_WAITING_MESSAGE_PREFIX,
};
use makepad_studio_protocol::hub_protocol::{AiMessage, AiMessageRole};

const MAX_RESULT_CHARS: usize = 16_000;

#[derive(Clone, Debug)]
pub struct AiToolExecutionResult {
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: String,
    pub is_error: bool,
}

pub fn format_tool_call_message(tool_call: &ToolCallRecord) -> String {
    format!(
        "`{}`\n```json\n{}\n```",
        tool_call.name,
        tool_call.arguments_json.trim()
    )
}

pub fn format_tool_result_message(result: &AiToolExecutionResult) -> String {
    let label = if result.is_error {
        format!("`{}` failed", result.tool_name)
    } else {
        format!("`{}` result", result.tool_name)
    };
    format!(
        "{}\n```text\n{}\n```",
        label,
        truncate_text(result.content.trim(), MAX_RESULT_CHARS)
    )
}

pub fn tool_calls_action_summary(tool_calls: &[ToolCallRecord]) -> String {
    if tool_calls.len() == 1 {
        return format!("Running {}", tool_call_activity_label(&tool_calls[0]));
    }
    let names = tool_calls
        .iter()
        .take(4)
        .map(tool_call_activity_label)
        .collect::<Vec<_>>()
        .join(", ");
    let remaining = tool_calls.len().saturating_sub(4);
    if remaining == 0 {
        format!("Running {} tools: {}", tool_calls.len(), names)
    } else {
        format!(
            "Running {} tools: {}, +{} more",
            tool_calls.len(),
            names,
            remaining
        )
    }
}

pub fn tool_call_activity_label(tool_call: &ToolCallRecord) -> String {
    let args = tool_call.arguments_json.as_str();
    match tool_call.name.as_str() {
        "read_file" | "write_file" | "replace_in_file" | "open_editor" => {
            if let Some(path) = parse_json_string_field(args, "path") {
                return format!("`{}` on `{}`", tool_call.name, truncate_inline(&path, 80));
            }
        }
        "list_files" | "observe_filesystem" => {
            let path = parse_json_string_field(args, "path").unwrap_or_else(|| ".".to_string());
            return format!("`{}` in `{}`", tool_call.name, truncate_inline(&path, 80));
        }
        "search_text" => {
            if let Some(pattern) = parse_json_string_field(args, "pattern") {
                let path = parse_json_string_field(args, "path").unwrap_or_else(|| ".".to_string());
                return format!(
                    "`search_text` for `{}` in `{}`",
                    truncate_inline(&pattern, 48),
                    truncate_inline(&path, 80)
                );
            }
        }
        "bash" => {
            if let Some(command) = parse_json_string_field(args, "command") {
                return format!("`bash` `{}`", truncate_inline(&command, 96));
            }
        }
        "read_terminal" | "send_terminal_text" | "send_terminal_key" => {
            if let Some(path) = parse_json_string_field(args, "path") {
                return format!("`{}` for `{}`", tool_call.name, truncate_inline(&path, 80));
            }
        }
        "open_terminal" => {
            if let Some(command) = parse_json_string_field(args, "command") {
                return format!("`open_terminal` `{}`", truncate_inline(&command, 96));
            }
            if let Some(name) = parse_json_string_field(args, "name") {
                return format!("`open_terminal` `{}`", truncate_inline(&name, 80));
            }
        }
        "spawn_subagent" => {
            let role =
                parse_json_string_field(args, "role").unwrap_or_else(|| "subagent".to_string());
            if let Some(task) = parse_json_string_field(args, "task") {
                return format!(
                    "`spawn_subagent` {}: {}",
                    truncate_inline(&role, 32),
                    truncate_inline(&task, 96)
                );
            }
            return format!("`spawn_subagent` {}", truncate_inline(&role, 32));
        }
        _ => {}
    }
    format!("`{}`", tool_call.name)
}

pub fn tool_results_action_summary(results: &[AiToolExecutionResult]) -> String {
    if results.len() == 1 {
        let result = &results[0];
        return if result.is_error {
            format!("`{}` failed", result.tool_name)
        } else {
            format!("`{}` completed", result.tool_name)
        };
    }
    let failed = results.iter().filter(|result| result.is_error).count();
    if failed == 0 {
        format!("Completed {} tools", results.len())
    } else {
        format!("Completed {} tools, {} failed", results.len(), failed)
    }
}

pub fn format_terminal_waiting_message(result: &AiToolExecutionResult) -> Option<String> {
    if result.is_error || result.tool_name != "read_terminal" {
        return None;
    }
    let mode = parse_json_string_field(&result.content, "mode").unwrap_or_default();
    let path = parse_json_string_field(&result.content, "path").unwrap_or_default();
    let detail = parse_json_string_field(&result.content, "codex_status")
        .or_else(|| parse_json_string_field(&result.content, "summary"))
        .unwrap_or_default();
    let detail_lowered = detail.to_ascii_lowercase();
    if mode != "working"
        && !detail_lowered.contains("working")
        && !detail_lowered.contains("esc to interrupt")
    {
        return None;
    }

    let mut message = format!("{}waiting", AI_WAITING_MESSAGE_PREFIX);
    if !path.is_empty() {
        message.push_str(" on `");
        message.push_str(&path);
        message.push('`');
    }
    if !detail.trim().is_empty() {
        message.push_str(" - ");
        message.push_str(&truncate_waiting_detail(&detail));
    }
    Some(message)
}

pub fn last_terminal_waiting_message_from_history(history: &[ConversationItem]) -> Option<String> {
    let ConversationItem::ToolResult {
        tool_call_id,
        content,
    } = history.last()?
    else {
        return None;
    };

    let tool_call = history.iter().rev().find_map(|item| {
        let ConversationItem::Assistant { tool_calls, .. } = item else {
            return None;
        };
        tool_calls
            .iter()
            .find(|tool_call| tool_call.id == *tool_call_id && tool_call.name == "read_terminal")
    })?;

    format_terminal_waiting_message(&AiToolExecutionResult {
        tool_call_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        content: content.clone(),
        is_error: false,
    })
}

pub fn format_terminal_observation_message(
    path: &str,
    mode: &str,
    summary: &str,
    codex_status: Option<&str>,
    excerpt: &str,
) -> String {
    let mut message = format!("{} {}\n", AI_TERMINAL_OBSERVATION_PREFIX, path);
    message.push_str(&format!("Mode: {}\n", mode));
    if let Some(codex_status) = codex_status.filter(|value| !value.trim().is_empty()) {
        message.push_str(&format!("Codex status: {}\n", codex_status.trim()));
    }
    if !summary.trim().is_empty() {
        message.push_str(&format!("Summary: {}\n", summary.trim()));
    }
    if !excerpt.trim().is_empty() {
        message.push_str("\nLatest output excerpt:\n```text\n");
        message.push_str(excerpt.trim());
        message.push_str("\n```");
    }
    message
}

pub fn upsert_terminal_observation_message(messages: &mut Vec<AiMessage>, text: &str) {
    let path = terminal_observation_path(text);
    if let Some(path) = path {
        if let Some(last) = messages.last_mut() {
            if matches!(last.role, AiMessageRole::System)
                && terminal_observation_path(&last.text) == Some(path)
            {
                last.text = text.to_string();
                return;
            }
        }
    }
    messages.push(AiMessage {
        role: AiMessageRole::System,
        text: text.to_string(),
    });
}

pub fn upsert_terminal_observation_history(history: &mut Vec<ConversationItem>, text: &str) {
    let path = terminal_observation_path(text);
    if let Some(path) = path {
        if let Some(ConversationItem::User { text: last }) = history.last_mut() {
            if terminal_observation_path(last) == Some(path) {
                *last = text.to_string();
                return;
            }
        }
    }
    history.push(ConversationItem::User {
        text: text.to_string(),
    });
}

pub fn terminal_observation_path(text: &str) -> Option<&str> {
    text.lines()
        .next()?
        .strip_prefix(AI_TERMINAL_OBSERVATION_PREFIX)?
        .trim()
        .split_whitespace()
        .next()
}

pub fn upsert_terminal_waiting_message(messages: &mut Vec<AiMessage>, waiting_message: String) {
    if let Some(last) = messages.last_mut() {
        if matches!(last.role, AiMessageRole::Thinking)
            && last.text.starts_with(AI_WAITING_MESSAGE_PREFIX)
        {
            last.text = waiting_message;
            return;
        }
    }
    messages.push(AiMessage {
        role: AiMessageRole::Thinking,
        text: waiting_message,
    });
}

pub fn trim_terminal_waiting_tail(messages: &mut Vec<AiMessage>) {
    if messages
        .last()
        .is_some_and(|message| message_tool_name(message) == Some("read_terminal"))
    {
        messages.pop();
    }
    while messages.last().is_some_and(|message| {
        matches!(
            message.role,
            AiMessageRole::Thinking | AiMessageRole::Assistant
        ) && looks_like_terminal_waiting_text(&message.text)
    }) {
        messages.pop();
    }
}

pub fn message_tool_name(message: &AiMessage) -> Option<&str> {
    if !matches!(message.role, AiMessageRole::ToolCall) {
        return None;
    }
    let rest = message.text.strip_prefix('`')?;
    let (tool_name, _) = rest.split_once('`')?;
    Some(tool_name)
}

pub fn looks_like_terminal_waiting_text(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    lowered.contains("working on the task")
        || lowered.contains("still working")
        || (lowered.contains("codex") && lowered.contains("working"))
        || ((lowered.contains("wait") || lowered.contains("check again"))
            && lowered.contains("progress"))
}

pub fn truncate_waiting_detail(text: &str) -> String {
    let single_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = single_line.chars().take(160).collect::<String>();
    if single_line.chars().count() > 160 {
        out.push_str("...");
    }
    out
}

pub fn truncate_text(text: &str, max_chars: usize) -> String {
    let mut out = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        out.push_str("\n\n[output truncated]");
    }
    out
}

pub fn json_string(value: &str) -> String {
    value.to_string().serialize_json()
}

pub fn is_terminal_tool_name(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "read_terminal" | "send_terminal_text" | "send_terminal_key" | "open_terminal"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_activity_labels_are_compact() {
        let call = ToolCallRecord {
            id: "call_1".to_string(),
            name: "write_file".to_string(),
            arguments_json: r#"{"path":"src/main.rs"}"#.to_string(),
        };
        assert_eq!(
            tool_call_activity_label(&call),
            "`write_file` on `src/main.rs`"
        );
    }

    #[test]
    fn terminal_waiting_message_formats_working_reads() {
        let result = AiToolExecutionResult {
            tool_call_id: "call_1".to_string(),
            tool_name: "read_terminal".to_string(),
            content: r#"{"mode":"working","path":"term","summary":"Still working on the task"}"#
                .to_string(),
            is_error: false,
        };
        let message = format_terminal_waiting_message(&result).unwrap();
        assert!(message.contains("waiting on `term`"));
        assert!(message.contains("Still working"));
    }

    #[test]
    fn terminal_observation_upsert_replaces_same_path() {
        let mut messages = vec![AiMessage {
            role: AiMessageRole::System,
            text: format_terminal_observation_message("term", "working", "old", None, ""),
        }];
        upsert_terminal_observation_message(
            &mut messages,
            &format_terminal_observation_message("term", "done", "new", None, ""),
        );
        assert_eq!(messages.len(), 1);
        assert!(messages[0].text.contains("Mode: done"));
    }
}
