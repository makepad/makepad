use super::super::{
    append_raw_event_sample, build_chatgpt_request, extract_openai_assistant_turn, AiBackendConfig,
    AssistantTurn, ConversationItem, OpenAiStreamFunctionDelta, OpenAiStreamToolCallDelta,
    StreamingTurnState,
};
use super::{AiProviderBackend, ProviderStreamDeltas};
use makepad_chatgpt_provider::{ChatGptProvider, ChatGptStreamEvent};
use makepad_network::HttpRequest;

pub(super) struct ChatGptBackend;

pub(super) static CHATGPT_BACKEND: ChatGptBackend = ChatGptBackend;

impl AiProviderBackend for ChatGptBackend {
    fn build_http_request(
        &self,
        backend: &AiBackendConfig,
        mount: &str,
        root_path: &str,
        history: &[ConversationItem],
        role: Option<&str>,
        task: Option<&str>,
        active_terminals: &[String],
    ) -> Result<HttpRequest, String> {
        let Some(provider) = &backend.chatgpt else {
            return Err("ChatGPT backend is missing provider state".to_string());
        };
        let request = build_chatgpt_request(
            backend,
            mount,
            root_path,
            history,
            role,
            task,
            active_terminals,
        )?;
        provider
            .build_responses_request(&request)
            .map_err(|err| err.to_string())
    }

    fn response_is_stream(&self) -> bool {
        true
    }

    fn extract_assistant_turn(&self, body: &str) -> Result<AssistantTurn, String> {
        extract_openai_assistant_turn(body)
    }

    fn process_stream_events(
        &self,
        events: Vec<String>,
        stream: &mut StreamingTurnState,
    ) -> Result<ProviderStreamDeltas, String> {
        let mut deltas = ProviderStreamDeltas::empty();
        for event in events {
            append_raw_event_sample(&mut stream.raw_event_sample, &event);
            for delta in
                ChatGptProvider::parse_stream_chunk(&event).map_err(|err| err.to_string())?
            {
                match delta {
                    ChatGptStreamEvent::TextDelta { text } => {
                        if deltas.saw_done {
                            if !stream.saw_text_delta {
                                deltas.assistant_delta.push_str(&text);
                            }
                        } else {
                            deltas.assistant_delta.push_str(&text);
                            stream.saw_text_delta = true;
                        }
                    }
                    ChatGptStreamEvent::ToolCallStart { id, name } => {
                        deltas.tool_call_deltas.push(OpenAiStreamToolCallDelta {
                            index: Some(deltas.tool_call_deltas.len() as u32),
                            id: Some(id),
                            kind: Some("function".to_string()),
                            function: Some(OpenAiStreamFunctionDelta {
                                name: Some(name),
                                arguments: None,
                            }),
                        });
                    }
                    ChatGptStreamEvent::ToolCallArgumentsDelta { partial_json } => {
                        if let Some(last) = deltas.tool_call_deltas.last_mut() {
                            let mut args = last
                                .function
                                .as_ref()
                                .and_then(|function| function.arguments.clone())
                                .unwrap_or_default();
                            args.push_str(&partial_json);
                            if let Some(function) = last.function.as_mut() {
                                function.arguments = Some(args);
                            }
                        }
                    }
                    ChatGptStreamEvent::Completed {
                        finish_reason: stream_finish_reason,
                        ..
                    } => {
                        if let Some(reason) = stream_finish_reason {
                            deltas.finish_reason = Some(reason);
                        }
                        deltas.saw_done = true;
                    }
                    ChatGptStreamEvent::Error { message } => return Err(message),
                }
            }
        }
        Ok(deltas)
    }
}
