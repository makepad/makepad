use makepad_micro_serde::*;
use makepad_studio_protocol::hub_protocol::{AiMessage, AiMessageRole};
use std::collections::HashSet;

#[derive(Clone, Debug, SerJson, DeJson, PartialEq)]
pub struct ToolCallRecord {
    pub id: String,
    pub name: String,
    pub arguments_json: String,
}

#[derive(Clone, Debug, SerJson, DeJson, PartialEq)]
pub enum ConversationItem {
    User {
        text: String,
    },
    Assistant {
        text: String,
        tool_calls: Vec<ToolCallRecord>,
    },
    ToolResult {
        tool_call_id: String,
        content: String,
    },
}

pub fn sanitize_conversation_history(history: Vec<ConversationItem>) -> Vec<ConversationItem> {
    let completed_tool_call_ids = history
        .iter()
        .filter_map(|item| match item {
            ConversationItem::ToolResult { tool_call_id, .. } => Some(tool_call_id.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let mut retained_tool_call_ids = HashSet::new();
    let mut sanitized = Vec::new();

    for item in history {
        match item {
            ConversationItem::Assistant { text, tool_calls } => {
                let tool_calls = tool_calls
                    .into_iter()
                    .filter(|tool_call| completed_tool_call_ids.contains(&tool_call.id))
                    .collect::<Vec<_>>();
                if is_empty_assistant_turn(&text, &tool_calls) {
                    continue;
                }
                for tool_call in &tool_calls {
                    retained_tool_call_ids.insert(tool_call.id.clone());
                }
                sanitized.push(ConversationItem::Assistant { text, tool_calls });
            }
            ConversationItem::ToolResult {
                tool_call_id,
                content,
            } => {
                if retained_tool_call_ids.contains(&tool_call_id) {
                    sanitized.push(ConversationItem::ToolResult {
                        tool_call_id,
                        content,
                    });
                }
            }
            item => sanitized.push(item),
        }
    }

    sanitized
}

pub fn push_assistant_history_dedup(
    history: &mut Vec<ConversationItem>,
    text: String,
    tool_calls: Vec<ToolCallRecord>,
) {
    if tool_calls.is_empty()
        && history.last().is_some_and(|item| {
            matches!(
                item,
                ConversationItem::Assistant {
                    text: previous,
                    tool_calls
                } if tool_calls.is_empty() && same_visible_text(previous, &text)
            )
        })
    {
        return;
    }
    history.push(ConversationItem::Assistant { text, tool_calls });
}

pub fn collapse_repeated_tail_messages(messages: &mut Vec<AiMessage>) {
    loop {
        let len = messages.len();
        if len < 2 {
            return;
        }
        let duplicate = {
            let previous = &messages[len - 2];
            let current = &messages[len - 1];
            matches!(previous.role, AiMessageRole::Assistant)
                && matches!(current.role, AiMessageRole::Assistant)
                && same_visible_text(&previous.text, &current.text)
        };
        if duplicate {
            messages.pop();
        } else {
            return;
        }
    }
}

pub fn same_visible_text(left: &str, right: &str) -> bool {
    left.split_whitespace().collect::<Vec<_>>() == right.split_whitespace().collect::<Vec<_>>()
}

pub fn is_empty_assistant_turn(text: &str, tool_calls: &[ToolCallRecord]) -> bool {
    text.trim().is_empty() && tool_calls.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str) -> ToolCallRecord {
        ToolCallRecord {
            id: id.to_string(),
            name: "read_file".to_string(),
            arguments_json: "{}".to_string(),
        }
    }

    #[test]
    fn sanitize_drops_unresolved_tool_calls_and_orphan_results() {
        let history = vec![
            ConversationItem::User {
                text: "start".to_string(),
            },
            ConversationItem::Assistant {
                text: "".to_string(),
                tool_calls: vec![call("kept"), call("dropped")],
            },
            ConversationItem::ToolResult {
                tool_call_id: "kept".to_string(),
                content: "ok".to_string(),
            },
            ConversationItem::ToolResult {
                tool_call_id: "orphan".to_string(),
                content: "ignored".to_string(),
            },
        ];

        let sanitized = sanitize_conversation_history(history);
        assert_eq!(sanitized.len(), 3);
        assert!(matches!(
            &sanitized[1],
            ConversationItem::Assistant { tool_calls, .. } if tool_calls.len() == 1 && tool_calls[0].id == "kept"
        ));
        assert!(matches!(
            &sanitized[2],
            ConversationItem::ToolResult { tool_call_id, .. } if tool_call_id == "kept"
        ));
    }

    #[test]
    fn push_assistant_history_dedup_ignores_whitespace_only_duplicates() {
        let mut history = vec![ConversationItem::Assistant {
            text: "hello world".to_string(),
            tool_calls: Vec::new(),
        }];

        push_assistant_history_dedup(&mut history, "hello\nworld".to_string(), Vec::new());
        assert_eq!(history.len(), 1);

        push_assistant_history_dedup(&mut history, "changed".to_string(), Vec::new());
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn collapse_repeated_tail_messages_removes_duplicate_assistant_tail() {
        let mut messages = vec![
            AiMessage {
                role: AiMessageRole::Assistant,
                text: "same text".to_string(),
            },
            AiMessage {
                role: AiMessageRole::Assistant,
                text: "same\ntext".to_string(),
            },
        ];

        collapse_repeated_tail_messages(&mut messages);
        assert_eq!(messages.len(), 1);
    }
}
