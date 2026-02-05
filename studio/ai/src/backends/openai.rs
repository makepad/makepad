use crate::backend::*;
use crate::types::*;
use makepad_micro_serde::*;
use makepad_widgets2::*;
use std::collections::HashMap;

// === OpenAI API Response Types ===

#[derive(DeJson, Debug)]
struct OpenAiStreamChunk {
    #[allow(dead_code)]
    id: Option<String>,
    #[allow(dead_code)]
    object: Option<String>,
    #[allow(dead_code)]
    created: Option<u64>,
    #[allow(dead_code)]
    model: Option<String>,
    choices: Vec<OpenAiStreamChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(DeJson, Debug)]
struct OpenAiStreamChoice {
    #[allow(dead_code)]
    index: Option<u32>,
    delta: Option<OpenAiDelta>,
    finish_reason: Option<String>,
}

#[derive(DeJson, Debug)]
struct OpenAiDelta {
    #[allow(dead_code)]
    role: Option<String>,
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCallDelta>>,
}

#[derive(DeJson, Debug)]
struct OpenAiToolCallDelta {
    index: Option<u32>,
    id: Option<String>,
    #[rename(type)]
    #[allow(dead_code)]
    call_type: Option<String>,
    function: Option<OpenAiFunctionDelta>,
}

#[derive(DeJson, Debug)]
struct OpenAiFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(DeJson, Debug, Clone)]
struct OpenAiUsage {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    #[allow(dead_code)]
    total_tokens: Option<u32>,
}

// === In-flight Request Tracking ===

struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

struct InFlightRequest {
    request_id: RequestId,
    accumulated_text: String,
    tool_calls: Vec<ToolCallAccumulator>,
    usage: Option<OpenAiUsage>,
    finish_reason: Option<String>,
}

// === OpenAI Backend Implementation ===

pub struct OpenAiBackend {
    config: BackendConfig,
    in_flight: HashMap<LiveId, InFlightRequest>,
}

impl OpenAiBackend {
    pub fn new(config: BackendConfig) -> Self {
        Self {
            config,
            in_flight: HashMap::new(),
        }
    }

    fn escape_json_string(s: &str) -> String {
        let mut result = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                '"' => result.push_str("\\\""),
                '\\' => result.push_str("\\\\"),
                '\n' => result.push_str("\\n"),
                '\r' => result.push_str("\\r"),
                '\t' => result.push_str("\\t"),
                c if c.is_control() => {
                    result.push_str(&format!("\\u{:04x}", c as u32));
                }
                c => result.push(c),
            }
        }
        result
    }

    fn build_request_json(
        request: &AiRequest,
        model: &str,
        reasoning_effort: &Option<String>,
    ) -> String {
        let mut json = String::new();
        json.push_str("{");

        // Model
        json.push_str(&format!("\"model\":\"{}\",", model));

        // Stream
        json.push_str("\"stream\":true,");

        // Max tokens
        json.push_str(&format!("\"max_tokens\":{},", request.max_tokens));

        // Temperature
        if let Some(temp) = request.temperature {
            json.push_str(&format!("\"temperature\":{},", temp));
        }

        // Reasoning effort (for o-series models)
        if let Some(effort) = reasoning_effort {
            json.push_str(&format!("\"reasoning_effort\":\"{}\",", effort));
        }

        // Messages
        json.push_str("\"messages\":[");
        let mut first = true;

        // Add system prompt as first message if present
        if let Some(system) = &request.system_prompt {
            json.push_str(&format!(
                "{{\"role\":\"system\",\"content\":\"{}\"}}",
                Self::escape_json_string(system)
            ));
            first = false;
        }

        for msg in &request.messages {
            if !first {
                json.push(',');
            }
            first = false;

            let role = msg.role.as_str();
            let content = msg.text();

            json.push_str("{");
            json.push_str(&format!("\"role\":\"{}\"", role));

            if !content.is_empty() {
                json.push_str(&format!(
                    ",\"content\":\"{}\"",
                    Self::escape_json_string(&content)
                ));
            }

            // Handle tool calls for assistant messages
            let tool_uses: Vec<_> = msg
                .content
                .iter()
                .filter_map(|c| {
                    if let ContentBlock::ToolUse { id, name, input } = c {
                        Some((id, name, input))
                    } else {
                        None
                    }
                })
                .collect();

            if !tool_uses.is_empty() {
                json.push_str(",\"tool_calls\":[");
                for (i, (id, name, input)) in tool_uses.iter().enumerate() {
                    if i > 0 {
                        json.push(',');
                    }
                    json.push_str(&format!(
                        "{{\"id\":\"{}\",\"type\":\"function\",\"function\":{{\"name\":\"{}\",\"arguments\":{}}}}}",
                        id, name, input
                    ));
                }
                json.push(']');
            }

            // Handle tool results
            for block in &msg.content {
                if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                    json.push_str(&format!(",\"tool_call_id\":\"{}\"", tool_use_id));
                    break;
                }
            }

            json.push('}');
        }
        json.push(']');

        json.push('}');
        json
    }

    fn build_http_request(&self, request: &AiRequest) -> HttpRequest {
        let BackendConfig::OpenAI {
            api_key,
            model,
            base_url,
            reasoning_effort,
        } = &self.config
        else {
            panic!("OpenAiBackend requires OpenAI config");
        };

        let url = base_url
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1/chat/completions".to_string());

        let mut http = HttpRequest::new(url, HttpMethod::POST);
        http.set_is_streaming();
        http.set_header("Content-Type".to_string(), "application/json".to_string());
        http.set_header("Authorization".to_string(), format!("Bearer {}", api_key));

        let body = Self::build_request_json(request, model, reasoning_effort);
        http.set_string_body(body);
        http
    }

    fn process_stream_data(&mut self, live_id: LiveId, data: &str) -> Vec<AiEvent> {
        let mut events = vec![];

        let Some(in_flight) = self.in_flight.get_mut(&live_id) else {
            return events;
        };

        let request_id = in_flight.request_id;

        // OpenAI uses "data: <json>\n\n" format with "data: [DONE]" at end
        for chunk in data.split("\n\n") {
            let chunk = chunk.trim();
            if chunk.is_empty() {
                continue;
            }

            let Some(json_data) = chunk.strip_prefix("data: ") else {
                continue;
            };

            if json_data == "[DONE]" {
                continue;
            }

            match OpenAiStreamChunk::deserialize_json(json_data) {
                Ok(chunk) => {
                    if let Some(usage) = chunk.usage {
                        in_flight.usage = Some(usage);
                    }

                    for choice in &chunk.choices {
                        if let Some(finish) = &choice.finish_reason {
                            in_flight.finish_reason = Some(finish.clone());
                        }

                        if let Some(delta) = &choice.delta {
                            // Handle text content
                            if let Some(text) = &delta.content {
                                in_flight.accumulated_text.push_str(text);
                                events.push(AiEvent::StreamDelta {
                                    request_id,
                                    delta: StreamDelta::TextDelta { text: text.clone() },
                                });
                            }

                            // Handle tool calls
                            if let Some(tool_calls) = &delta.tool_calls {
                                for tc_delta in tool_calls {
                                    let idx = tc_delta.index.unwrap_or(0) as usize;

                                    // Ensure we have an accumulator for this index
                                    while in_flight.tool_calls.len() <= idx {
                                        in_flight.tool_calls.push(ToolCallAccumulator {
                                            id: String::new(),
                                            name: String::new(),
                                            arguments: String::new(),
                                        });
                                    }

                                    let acc = &mut in_flight.tool_calls[idx];

                                    // Update ID if provided
                                    if let Some(id) = &tc_delta.id {
                                        acc.id = id.clone();
                                    }

                                    // Update function info
                                    if let Some(func) = &tc_delta.function {
                                        if let Some(name) = &func.name {
                                            if acc.name.is_empty() {
                                                acc.name = name.clone();
                                                events.push(AiEvent::StreamDelta {
                                                    request_id,
                                                    delta: StreamDelta::ToolUseStart {
                                                        id: acc.id.clone(),
                                                        name: name.clone(),
                                                    },
                                                });
                                            }
                                        }
                                        if let Some(args) = &func.arguments {
                                            acc.arguments.push_str(args);
                                            events.push(AiEvent::StreamDelta {
                                                request_id,
                                                delta: StreamDelta::ToolUseDelta {
                                                    partial_json: args.clone(),
                                                },
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    log!("OpenAI JSON parse error: {:?} for data: {}", e, json_data);
                }
            }
        }

        events
    }
}

impl AiBackend for OpenAiBackend {
    fn send_request(&mut self, cx: &mut Cx, request: AiRequest) -> RequestId {
        let request_id = RequestId::new();
        let http = self.build_http_request(&request);

        self.in_flight.insert(
            request_id.0,
            InFlightRequest {
                request_id,
                accumulated_text: String::new(),
                tool_calls: vec![],
                usage: None,
                finish_reason: None,
            },
        );

        cx.http_request(request_id.0, http);
        request_id
    }

    fn cancel_request(&mut self, cx: &mut Cx, request_id: RequestId) {
        if self.in_flight.remove(&request_id.0).is_some() {
            cx.cancel_http_request(request_id.0);
        }
    }

    fn handle_event(&mut self, _cx: &mut Cx, event: &Event) -> Vec<AiEvent> {
        let mut ai_events = vec![];

        if let Event::NetworkResponses(responses) = event {
            for response in responses {
                if !self.in_flight.contains_key(&response.request_id) {
                    continue;
                }

                match &response.response {
                    NetworkResponse::HttpStreamResponse(res) => {
                        if let Some(data) = res.get_string_body() {
                            ai_events.extend(self.process_stream_data(response.request_id, &data));
                        }
                    }
                    NetworkResponse::HttpStreamComplete(_) => {
                        if let Some(in_flight) = self.in_flight.remove(&response.request_id) {
                            // Build content blocks
                            let mut content_blocks = vec![];

                            if !in_flight.accumulated_text.is_empty() {
                                content_blocks.push(ContentBlock::Text {
                                    text: in_flight.accumulated_text,
                                });
                            }

                            for tc in in_flight.tool_calls {
                                if !tc.id.is_empty() && !tc.name.is_empty() {
                                    content_blocks.push(ContentBlock::ToolUse {
                                        id: tc.id,
                                        name: tc.name,
                                        input: tc.arguments,
                                    });
                                    ai_events.push(AiEvent::StreamDelta {
                                        request_id: in_flight.request_id,
                                        delta: StreamDelta::ToolUseEnd,
                                    });
                                }
                            }

                            let stop_reason = match in_flight.finish_reason.as_deref() {
                                Some("stop") => StopReason::EndTurn,
                                Some("length") => StopReason::MaxTokens,
                                Some("tool_calls") => StopReason::ToolUse,
                                _ => StopReason::EndTurn,
                            };

                            let usage = in_flight
                                .usage
                                .map(|u| Usage {
                                    input_tokens: u.prompt_tokens.unwrap_or(0),
                                    output_tokens: u.completion_tokens.unwrap_or(0),
                                })
                                .unwrap_or_default();

                            ai_events.push(AiEvent::Complete {
                                request_id: in_flight.request_id,
                                response: AiResponse {
                                    message: Message {
                                        role: MessageRole::Assistant,
                                        content: content_blocks,
                                    },
                                    stop_reason,
                                    usage,
                                },
                            });
                        }
                    }
                    NetworkResponse::HttpRequestError(err) => {
                        if let Some(in_flight) = self.in_flight.remove(&response.request_id) {
                            ai_events.push(AiEvent::Error {
                                request_id: in_flight.request_id,
                                error: err.message.clone(),
                            });
                        }
                    }
                    _ => {}
                }
            }
        }

        ai_events
    }

    fn config(&self) -> &BackendConfig {
        &self.config
    }
}
