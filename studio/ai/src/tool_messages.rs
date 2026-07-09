use crate::{truncate_inline, ToolCallRecord};
use makepad_micro_serde::*;
use makepad_studio_protocol::ai_format::parse_json_string_field;

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
}
