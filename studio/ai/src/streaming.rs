use crate::ToolCallRecord;
use makepad_micro_serde::*;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolCallAccumulator {
    pub id: String,
    pub name: String,
    pub arguments_json: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StreamVisibleState {
    pub thinking_message_index: Option<usize>,
    pub assistant_message_index: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamingTurnState {
    pub buffer: String,
    pub raw_event_sample: String,
    pub saw_text_delta: bool,
    pub thinking_text: String,
    pub assistant_text: String,
    pub tool_calls: Vec<ToolCallAccumulator>,
    pub finish_reason: Option<String>,
    pub done_received: bool,
    pub visible: StreamVisibleState,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamUpdate {
    pub changed: bool,
    pub done: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AssistantTurn {
    pub text: String,
    pub thinking_text: String,
    pub tool_calls: Vec<ToolCallRecord>,
    pub raw_event_sample: String,
}

#[derive(DeJson)]
pub struct OpenAiStreamChunk {
    pub choices: Vec<OpenAiStreamChoice>,
    pub error: Option<OpenAiErrorEnvelope>,
}

#[derive(DeJson)]
pub struct OpenAiStreamChoice {
    pub delta: Option<OpenAiStreamDelta>,
    pub finish_reason: Option<String>,
}

#[derive(DeJson)]
pub struct OpenAiStreamDelta {
    pub content: Option<String>,
    pub reasoning_content: Option<String>,
    pub reasoning: Option<String>,
    pub reasoning_text: Option<String>,
    pub tool_calls: Option<Vec<OpenAiStreamToolCallDelta>>,
}

#[derive(DeJson)]
pub struct OpenAiStreamToolCallDelta {
    pub index: Option<u32>,
    pub id: Option<String>,
    #[rename(type)]
    pub kind: Option<String>,
    pub function: Option<OpenAiStreamFunctionDelta>,
}

#[derive(DeJson)]
pub struct OpenAiStreamFunctionDelta {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[derive(DeJson)]
pub struct OpenAiErrorEnvelope {
    pub message: Option<String>,
}

pub fn drain_sse_events(buffer: &mut String, flush: bool) -> Vec<String> {
    let mut events = Vec::new();
    while let Some(index) = buffer.find("\n\n") {
        let event = buffer[..index].to_string();
        buffer.drain(..index + 2);
        events.push(event);
    }
    if flush {
        let trailing = buffer.trim();
        if !trailing.is_empty() {
            events.push(trailing.to_string());
        }
        buffer.clear();
    }
    events
}

pub fn extract_sse_event_data(event: &str) -> Option<String> {
    let mut out = String::new();
    for line in event.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.strip_prefix(' ').unwrap_or(data);
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(data);
    }
    (!out.is_empty()).then_some(out)
}

pub fn first_non_empty_stream_reasoning(delta: &OpenAiStreamDelta) -> Option<String> {
    delta
        .reasoning_content
        .as_deref()
        .or(delta.reasoning.as_deref())
        .or(delta.reasoning_text.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn apply_tool_call_delta(
    tool_calls: &mut Vec<ToolCallAccumulator>,
    delta: OpenAiStreamToolCallDelta,
) -> Result<(), String> {
    if let Some(kind) = &delta.kind {
        if kind != "function" {
            return Err(format!("unsupported streamed tool call type '{}'", kind));
        }
    }
    let index = delta.index.unwrap_or(0) as usize;
    while tool_calls.len() <= index {
        tool_calls.push(ToolCallAccumulator::default());
    }
    let tool_call = &mut tool_calls[index];
    if let Some(id) = delta.id {
        tool_call.id = id;
    }
    if let Some(function) = delta.function {
        if let Some(name) = function.name {
            tool_call.name = name;
        }
        if let Some(arguments) = function.arguments {
            tool_call.arguments_json.push_str(&arguments);
        }
    }
    Ok(())
}

pub fn finalize_stream_turn(
    stream: StreamingTurnState,
) -> Result<(AssistantTurn, StreamVisibleState), String> {
    let tool_calls = stream
        .tool_calls
        .into_iter()
        .filter(|tool_call| {
            !tool_call.id.is_empty()
                || !tool_call.name.is_empty()
                || !tool_call.arguments_json.is_empty()
        })
        .map(|tool_call| {
            if tool_call.id.is_empty() {
                return Err("AI backend streamed a tool call without an id".to_string());
            }
            if tool_call.name.is_empty() {
                return Err("AI backend streamed a tool call without a name".to_string());
            }
            Ok(ToolCallRecord {
                id: tool_call.id,
                name: tool_call.name,
                arguments_json: tool_call.arguments_json,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((
        AssistantTurn {
            text: stream.assistant_text,
            thinking_text: stream.thinking_text,
            tool_calls,
            raw_event_sample: stream.raw_event_sample,
        },
        stream.visible,
    ))
}

pub fn append_raw_event_sample(sample: &mut String, event: &str) {
    const MAX_RAW_EVENT_SAMPLE: usize = 6000;
    if sample.len() >= MAX_RAW_EVENT_SAMPLE {
        return;
    }
    if !sample.is_empty() {
        sample.push_str("\n\n");
    }
    let remaining = MAX_RAW_EVENT_SAMPLE.saturating_sub(sample.len());
    if event.len() <= remaining {
        sample.push_str(event);
    } else {
        sample.push_str(&event[..remaining]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_sse_events_keeps_partial_event_until_flush() {
        let mut buffer = "data: one\n\ndata: two".to_string();
        let events = drain_sse_events(&mut buffer, false);
        assert_eq!(events, vec!["data: one"]);
        assert_eq!(buffer, "data: two");

        let events = drain_sse_events(&mut buffer, true);
        assert_eq!(events, vec!["data: two"]);
        assert!(buffer.is_empty());
    }

    #[test]
    fn extract_sse_event_data_concatenates_data_lines() {
        let event = "event: message\ndata: {\"a\":1}\ndata: {\"b\":2}";
        assert_eq!(
            extract_sse_event_data(event).as_deref(),
            Some("{\"a\":1}\n{\"b\":2}")
        );
    }

    #[test]
    fn streamed_tool_call_deltas_finalize_into_records() {
        let mut stream = StreamingTurnState::default();
        apply_tool_call_delta(
            &mut stream.tool_calls,
            OpenAiStreamToolCallDelta {
                index: Some(0),
                id: Some("call_1".to_string()),
                kind: Some("function".to_string()),
                function: Some(OpenAiStreamFunctionDelta {
                    name: Some("read_file".to_string()),
                    arguments: Some("{\"path\"".to_string()),
                }),
            },
        )
        .unwrap();
        apply_tool_call_delta(
            &mut stream.tool_calls,
            OpenAiStreamToolCallDelta {
                index: Some(0),
                id: None,
                kind: None,
                function: Some(OpenAiStreamFunctionDelta {
                    name: None,
                    arguments: Some(":\"Cargo.toml\"}".to_string()),
                }),
            },
        )
        .unwrap();

        let (turn, _) = finalize_stream_turn(stream).unwrap();
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].id, "call_1");
        assert_eq!(turn.tool_calls[0].name, "read_file");
        assert_eq!(
            turn.tool_calls[0].arguments_json,
            "{\"path\":\"Cargo.toml\"}"
        );
    }
}
