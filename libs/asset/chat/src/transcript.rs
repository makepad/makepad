//! The conversation as PEOPLE read it, rendered from the history a session
//! feeds its provider.
//!
//! The provider-facing history is deliberately not the transcript: user
//! rows carry the tool reminder, assistant rows carry the trained-template
//! call text that keeps the model calling tools, and tool rows are the raw
//! outcome JSON. A client that stored that would re-render prompt
//! plumbing. This module folds each of those back into what was shown on
//! screen: user text, assistant text (thinking stripped), and ONE `tool`
//! chip per executed round with a short title. It is pure — an owner
//! persists [`crate::session::Session::history`] and renders on read.

use crate::session::TOOL_REMINDER;
use crate::toolcall;
use crate::tools;
use crate::wire::{ChatMessage, ChatRole};
use makepad_asset_client::json::{self, Value};

/// Tool rows carry their parts next to the rendered title so a client can
/// draw a chip without parsing the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptRow {
    pub role: ChatRole,
    pub text: String,
    /// `Tool` rows: the dotted tool name of the round, when known.
    pub tool: Option<String>,
    /// `Tool` rows: `ok | unavailable | denied | refused | failed`.
    pub outcome: Option<&'static str>,
}

/// The assistant history placeholder for a tool-only reply (see
/// `Session::tool_round`); never shown.
const TOOL_ONLY_PLACEHOLDER: &str = "(tool call)";

/// Render `history` (oldest first) into screen rows, oldest first.
pub fn render(history: &[ChatMessage]) -> Vec<TranscriptRow> {
    let mut out = Vec::with_capacity(history.len());
    // The tool name of the round in flight: the assistant row that called
    // it comes first, the tool row that answered it next.
    let mut pending_tool: Option<String> = None;
    for m in history {
        match m.role {
            ChatRole::User => {
                let text = m.text.strip_suffix(TOOL_REMINDER).unwrap_or(&m.text);
                if !text.is_empty() {
                    out.push(TranscriptRow {
                        role: ChatRole::User,
                        text: text.to_string(),
                        tool: None,
                        outcome: None,
                    });
                }
            }
            ChatRole::Assistant => {
                let (visible, call) = split_trained_call(&m.text);
                if let Some(name) = call {
                    pending_tool = Some(name);
                }
                let visible = toolcall::split_thinking(&visible).visible;
                let visible = visible.trim();
                if !visible.is_empty() && visible != TOOL_ONLY_PLACEHOLDER {
                    out.push(TranscriptRow {
                        role: ChatRole::Assistant,
                        text: visible.to_string(),
                        tool: None,
                        outcome: None,
                    });
                }
            }
            ChatRole::Tool => {
                let row = match json::parse(m.text.as_bytes()) {
                    Ok(v) => match outcome_slug(&v) {
                        Some(outcome) => {
                            let tool = pending_tool.take();
                            let title = match &tool {
                                Some(name) => format!("{name} · {outcome}"),
                                None => format!("tool · {outcome}"),
                            };
                            TranscriptRow { role: ChatRole::Tool, text: title, tool, outcome: Some(outcome) }
                        }
                        // A broker note (the tool-budget nudge) rides the
                        // tool role without an outcome.
                        None => TranscriptRow {
                            role: ChatRole::Tool,
                            text: v
                                .get("note")
                                .and_then(Value::as_str)
                                .unwrap_or("(note)")
                                .to_string(),
                            tool: None,
                            outcome: None,
                        },
                    },
                    Err(_) => TranscriptRow {
                        role: ChatRole::Tool,
                        text: "(tool)".to_string(),
                        tool: pending_tool.take(),
                        outcome: None,
                    },
                };
                out.push(row);
            }
            ChatRole::System => out.push(TranscriptRow {
                role: ChatRole::System,
                text: m.text.clone(),
                tool: None,
                outcome: None,
            }),
        }
    }
    out
}

fn outcome_slug(v: &Value) -> Option<&'static str> {
    match v.get("outcome").and_then(Value::as_str)? {
        "ok" => Some("ok"),
        "unavailable" => Some("unavailable"),
        "denied" => Some("denied"),
        "refused" => Some("refused"),
        "failed" => Some("failed"),
        _ => None,
    }
}

/// Split an assistant history entry into its visible text and the dotted
/// name of the trained-template call recorded after it, if any.
fn split_trained_call(text: &str) -> (String, Option<String>) {
    let Some(at) = text.find("<tool_call>") else {
        return (text.to_string(), None);
    };
    let visible = text[..at].trim_end().to_string();
    let call = &text[at..];
    let name = call
        .find("<function=")
        .map(|i| &call[i + "<function=".len()..])
        .and_then(|rest| rest.split('>').next())
        .map(str::trim)
        .filter(|n| !n.is_empty() && n.len() <= 64)
        .map(tools::canonicalize_tool_name);
    (visible, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: ChatRole, text: &str) -> ChatMessage {
        ChatMessage::new(role, text)
    }

    #[test]
    fn a_tool_round_folds_into_text_plus_one_chip() {
        let history = vec![
            msg(ChatRole::User, "make a level"),
            msg(
                ChatRole::Assistant,
                "Building it.\n<tool_call>\n<function=world_set_source>\n<parameter=source>\ngame.sky({})\n</parameter>\n</function>\n</tool_call>",
            ),
            msg(ChatRole::Tool, r#"{"outcome":"ok","value":{"eval":"ok"}}"#),
            msg(ChatRole::User, &format!("thanks{TOOL_REMINDER}")),
            msg(ChatRole::Assistant, "<think>hm</think>Done — the level is live."),
        ];
        let rows = render(&history);
        assert_eq!(rows.len(), 5, "{rows:?}");
        assert_eq!(rows[0].role, ChatRole::User);
        assert_eq!(rows[0].text, "make a level");
        assert_eq!(rows[1].role, ChatRole::Assistant);
        assert_eq!(rows[1].text, "Building it.");
        assert_eq!(
            rows[2],
            TranscriptRow {
                role: ChatRole::Tool,
                text: "world.set_source · ok".into(),
                tool: Some("world.set_source".into()),
                outcome: Some("ok"),
            }
        );
        // The reminder is prompt plumbing, not what the user typed.
        assert_eq!(rows[3].text, "thanks");
        assert!(!rows[3].text.contains("tools are live"));
        // Thinking never reaches the screen.
        assert_eq!(rows[4].text, "Done — the level is live.");
    }

    #[test]
    fn tool_only_replies_notes_and_native_rounds_render_honestly() {
        let history = vec![
            // A tool-only assistant entry: the placeholder is not a row.
            msg(
                ChatRole::Assistant,
                "(tool call)\n<tool_call>\n<function=assets.query>\n<parameter=sql>\nSELECT 1\n</parameter>\n</function>\n</tool_call>",
            ),
            msg(ChatRole::Tool, r#"{"outcome":"refused","what":"bad sql"}"#),
            // The budget nudge.
            msg(ChatRole::Tool, r#"{"note":"tool budget reached"}"#),
            // A native-lane round: no trained call in the assistant text.
            msg(ChatRole::Assistant, "(tool call)"),
            msg(ChatRole::Tool, r#"{"outcome":"failed","message":"boom"}"#),
            // Garbage in the tool slot stays a chip, never a crash.
            msg(ChatRole::Tool, "not json"),
        ];
        let rows = render(&history);
        assert_eq!(rows.len(), 4, "{rows:?}");
        assert_eq!(rows[0].text, "assets.query · refused");
        assert_eq!(rows[0].outcome, Some("refused"));
        assert_eq!(rows[1].text, "tool budget reached");
        assert_eq!(rows[1].tool, None);
        assert_eq!(rows[2].text, "tool · failed");
        assert_eq!(rows[2].tool, None);
        assert_eq!(rows[3].text, "(tool)");
        assert_eq!(rows[3].outcome, None);
    }

    #[test]
    fn an_empty_history_is_an_empty_transcript() {
        assert!(render(&[]).is_empty());
    }
}
