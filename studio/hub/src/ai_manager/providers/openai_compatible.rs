use super::super::{
    build_request_body, extract_openai_assistant_turn, extract_sse_event_data,
    first_non_empty_stream_reasoning, AiBackendConfig, AssistantTurn, ConversationItem,
    OpenAiStreamChunk, ParsedSkill, ParsedWorkflow, StreamingTurnState,
};
use super::{AiProviderBackend, ProviderStreamDeltas};
use makepad_micro_serde::*;
use makepad_network::{HttpMethod, HttpRequest};
use makepad_studio_protocol::hub_protocol::ActiveWorkflowState;

pub(super) struct OpenAiCompatibleBackend;

pub(super) static OPENAI_COMPATIBLE_BACKEND: OpenAiCompatibleBackend = OpenAiCompatibleBackend;

impl AiProviderBackend for OpenAiCompatibleBackend {
    fn build_http_request(
        &self,
        backend: &AiBackendConfig,
        mount: &str,
        root_path: &str,
        history: &[ConversationItem],
        role: Option<&str>,
        task: Option<&str>,
        active_terminals: &[String],
        skills: &[ParsedSkill],
        workflows: &[ParsedWorkflow],
        active_workflow: Option<&ActiveWorkflowState>,
    ) -> Result<HttpRequest, String> {
        let body = build_request_body(
            backend,
            mount,
            root_path,
            history,
            role,
            task,
            active_terminals,
            skills,
            workflows,
            active_workflow,
        );

        let mut request = HttpRequest::new(backend.url.clone(), HttpMethod::POST);
        request.set_is_streaming();
        request.set_header("Content-Type".to_string(), "application/json".to_string());
        request.set_header("Accept".to_string(), "text/event-stream".to_string());
        if let Some(api_key) = &backend.api_key {
            request.set_header("Authorization".to_string(), format!("Bearer {}", api_key));
        }
        request.set_string_body(body);
        Ok(request)
    }

    fn extract_assistant_turn(&self, body: &str) -> Result<AssistantTurn, String> {
        extract_openai_assistant_turn(body)
    }

    fn process_stream_events(
        &self,
        events: Vec<String>,
        _stream: &mut StreamingTurnState,
    ) -> Result<ProviderStreamDeltas, String> {
        let mut deltas = ProviderStreamDeltas::empty();
        for event in events {
            let Some(json_data) = extract_sse_event_data(&event) else {
                continue;
            };
            if json_data == "[DONE]" {
                deltas.saw_done = true;
                continue;
            }
            let chunk = OpenAiStreamChunk::deserialize_json_lenient(&json_data)
                .map_err(|err| format!("invalid AI stream chunk: {:?}", err))?;
            if let Some(error) = chunk.error {
                return Err(error
                    .message
                    .unwrap_or_else(|| "AI backend returned a stream error".to_string()));
            }
            for choice in chunk.choices {
                if let Some(reason) = choice.finish_reason {
                    deltas.finish_reason = Some(reason);
                }
                if let Some(delta) = choice.delta {
                    if let Some(reasoning) = first_non_empty_stream_reasoning(&delta) {
                        deltas.thinking_delta.push_str(&reasoning);
                    }
                    if let Some(text) = delta.content {
                        deltas.assistant_delta.push_str(&text);
                    }
                    if let Some(tool_calls) = delta.tool_calls {
                        deltas.tool_call_deltas.extend(tool_calls);
                    }
                }
            }
        }
        Ok(deltas)
    }
}
