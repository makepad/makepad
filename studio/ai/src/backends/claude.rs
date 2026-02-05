use crate::backend::*;
use crate::types::*;
use makepad_micro_serde::*;
use makepad_widgets2::*;
use std::collections::HashMap;

// === Claude API Response Types ===

#[derive(DeJson, Debug)]
struct ClaudeStreamMessage {
    #[allow(dead_code)]
    message: Option<ClaudeMessageObj>,
    #[allow(dead_code)]
    index: Option<u32>,
    content_block: Option<ClaudeResponseBlock>,
    delta: Option<ClaudeDelta>,
    usage: Option<ClaudeUsage>,
    error: Option<ClaudeError>,
}

#[derive(DeJson, Debug)]
struct ClaudeMessageObj {
    #[allow(dead_code)]
    id: Option<String>,
    #[allow(dead_code)]
    model: Option<String>,
    usage: Option<ClaudeUsage>,
}

#[derive(DeJson, Debug)]
struct ClaudeResponseBlock {
    #[rename(type)]
    #[allow(dead_code)]
    block_type: Option<String>,
    text: Option<String>,
    id: Option<String>,
    name: Option<String>,
}

#[derive(DeJson, Debug)]
struct ClaudeDelta {
    #[rename(type)]
    delta_type: Option<String>,
    text: Option<String>,
    partial_json: Option<String>,
    stop_reason: Option<String>,
}

#[derive(DeJson, Debug, Clone)]
struct ClaudeUsage {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
}

#[derive(DeJson, Debug)]
struct ClaudeError {
    #[rename(type)]
    #[allow(dead_code)]
    error_type: Option<String>,
    message: Option<String>,
}

// === In-flight Request Tracking ===

struct InFlightRequest {
    request_id: RequestId,
    accumulated_text: String,
    current_tool_id: Option<String>,
    current_tool_name: Option<String>,
    current_tool_json: String,
    content_blocks: Vec<ContentBlock>,
    usage: Option<ClaudeUsage>,
    stop_reason: Option<String>,
}

// === Claude Backend Implementation ===

pub struct ClaudeBackend {
    config: BackendConfig,
    in_flight: HashMap<LiveId, InFlightRequest>,
}

impl ClaudeBackend {
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

    fn build_request_json(request: &AiRequest, model: &str) -> String {
        let mut json = String::new();
        json.push_str("{");

        // Model
        json.push_str(&format!("\"model\":\"{}\",", model));

        // Max tokens
        json.push_str(&format!("\"max_tokens\":{},", request.max_tokens));

        // Stream
        json.push_str("\"stream\":true,");

        // System prompt
        if let Some(system) = &request.system_prompt {
            json.push_str(&format!(
                "\"system\":\"{}\",",
                Self::escape_json_string(system)
            ));
        }

        // Temperature
        if let Some(temp) = request.temperature {
            json.push_str(&format!("\"temperature\":{},", temp));
        }

        // Messages
        json.push_str("\"messages\":[");
        let mut first = true;
        for msg in &request.messages {
            if msg.role == MessageRole::System {
                continue; // System handled above
            }

            if !first {
                json.push(',');
            }
            first = false;

            let role = match msg.role {
                MessageRole::User | MessageRole::Tool => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::System => continue,
            };

            json.push_str("{");
            json.push_str(&format!("\"role\":\"{}\",", role));

            // Check if simple text
            let has_only_text = msg
                .content
                .iter()
                .all(|c| matches!(c, ContentBlock::Text { .. }));

            if has_only_text && msg.content.len() == 1 {
                if let ContentBlock::Text { text } = &msg.content[0] {
                    json.push_str(&format!(
                        "\"content\":\"{}\"",
                        Self::escape_json_string(text)
                    ));
                }
            } else {
                json.push_str("\"content\":[");
                let mut first_block = true;
                for block in &msg.content {
                    if !first_block {
                        json.push(',');
                    }
                    first_block = false;

                    match block {
                        ContentBlock::Text { text } => {
                            json.push_str(&format!(
                                "{{\"type\":\"text\",\"text\":\"{}\"}}",
                                Self::escape_json_string(text)
                            ));
                        }
                        ContentBlock::ToolUse { id, name, input } => {
                            json.push_str(&format!(
                                "{{\"type\":\"tool_use\",\"id\":\"{}\",\"name\":\"{}\",\"input\":{}}}",
                                Self::escape_json_string(id),
                                Self::escape_json_string(name),
                                input // Already JSON
                            ));
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            json.push_str(&format!(
                                "{{\"type\":\"tool_result\",\"tool_use_id\":\"{}\",\"content\":\"{}\",\"is_error\":{}}}",
                                Self::escape_json_string(tool_use_id),
                                Self::escape_json_string(content),
                                is_error
                            ));
                        }
                        ContentBlock::Image { media_type, data } => {
                            json.push_str(&format!(
                                "{{\"type\":\"image\",\"source\":{{\"type\":\"base64\",\"media_type\":\"{}\",\"data\":\"{}\"}}}}",
                                media_type,
                                data
                            ));
                        }
                    }
                }
                json.push(']');
            }

            json.push('}');
        }
        json.push(']');

        json.push('}');
        json
    }

    fn build_http_request(&self, request: &AiRequest) -> HttpRequest {
        let BackendConfig::Claude {
            api_key,
            oauth_token,
            model,
        } = &self.config
        else {
            panic!("ClaudeBackend requires Claude config");
        };

        let mut http = HttpRequest::new(
            "https://api.anthropic.com/v1/messages".to_string(),
            HttpMethod::POST,
        );
        http.set_is_streaming();
        http.set_header("Content-Type".to_string(), "application/json".to_string());
        http.set_header("anthropic-version".to_string(), "2023-06-01".to_string());

        // Use OAuth token if available, otherwise API key
        if let Some(token) = oauth_token {
            http.set_header("Authorization".to_string(), format!("Bearer {}", token));
        } else if let Some(key) = api_key {
            http.set_header("x-api-key".to_string(), key.clone());
        }

        let body = Self::build_request_json(request, model);
        http.set_string_body(body);
        http
    }

    fn parse_sse_events(data: &str) -> Vec<(String, String)> {
        let mut events = vec![];
        for chunk in data.split("\n\n") {
            let chunk = chunk.trim();
            if chunk.is_empty() {
                continue;
            }
            let mut event_type = String::new();
            let mut event_data = String::new();
            for line in chunk.lines() {
                if let Some(t) = line.strip_prefix("event: ") {
                    event_type = t.to_string();
                } else if let Some(d) = line.strip_prefix("data: ") {
                    event_data = d.to_string();
                }
            }
            if !event_data.is_empty() {
                events.push((event_type, event_data));
            }
        }
        events
    }

    fn process_stream_data(&mut self, live_id: LiveId, data: &str) -> Vec<AiEvent> {
        let mut events = vec![];

        let Some(in_flight) = self.in_flight.get_mut(&live_id) else {
            return events;
        };

        let request_id = in_flight.request_id;

        for (event_type, event_data) in Self::parse_sse_events(data) {
            // Handle by event type first
            match event_type.as_str() {
                "ping" => continue,
                "message_stop" => continue,
                _ => {}
            }

            match ClaudeStreamMessage::deserialize_json(&event_data) {
                Ok(msg) => {
                    // Check for error
                    if let Some(err) = &msg.error {
                        let message = err
                            .message
                            .clone()
                            .unwrap_or_else(|| "Unknown error".to_string());
                        events.push(AiEvent::StreamDelta {
                            request_id,
                            delta: StreamDelta::Error { message },
                        });
                        continue;
                    }

                    match event_type.as_str() {
                        "message_start" => {
                            if let Some(message) = &msg.message {
                                if let Some(usage) = &message.usage {
                                    in_flight.usage = Some(usage.clone());
                                }
                            }
                        }
                        "content_block_start" => {
                            if let Some(block) = &msg.content_block {
                                if block.id.is_some() && block.name.is_some() {
                                    // Tool use block
                                    in_flight.current_tool_id = block.id.clone();
                                    in_flight.current_tool_name = block.name.clone();
                                    in_flight.current_tool_json.clear();

                                    if let (Some(id), Some(name)) = (&block.id, &block.name) {
                                        events.push(AiEvent::StreamDelta {
                                            request_id,
                                            delta: StreamDelta::ToolUseStart {
                                                id: id.clone(),
                                                name: name.clone(),
                                            },
                                        });
                                    }
                                }
                            }
                        }
                        "content_block_delta" => {
                            if let Some(delta) = &msg.delta {
                                match delta.delta_type.as_deref() {
                                    Some("text_delta") => {
                                        if let Some(text) = &delta.text {
                                            in_flight.accumulated_text.push_str(text);
                                            events.push(AiEvent::StreamDelta {
                                                request_id,
                                                delta: StreamDelta::TextDelta {
                                                    text: text.clone(),
                                                },
                                            });
                                        }
                                    }
                                    Some("input_json_delta") => {
                                        if let Some(json) = &delta.partial_json {
                                            in_flight.current_tool_json.push_str(json);
                                            events.push(AiEvent::StreamDelta {
                                                request_id,
                                                delta: StreamDelta::ToolUseDelta {
                                                    partial_json: json.clone(),
                                                },
                                            });
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        "content_block_stop" => {
                            if in_flight.current_tool_id.is_some() {
                                if let (Some(id), Some(name)) = (
                                    in_flight.current_tool_id.take(),
                                    in_flight.current_tool_name.take(),
                                ) {
                                    let input = std::mem::take(&mut in_flight.current_tool_json);
                                    in_flight.content_blocks.push(ContentBlock::ToolUse {
                                        id,
                                        name,
                                        input,
                                    });
                                }
                                events.push(AiEvent::StreamDelta {
                                    request_id,
                                    delta: StreamDelta::ToolUseEnd,
                                });
                            } else if !in_flight.accumulated_text.is_empty() {
                                let text = std::mem::take(&mut in_flight.accumulated_text);
                                in_flight.content_blocks.push(ContentBlock::Text { text });
                            }
                        }
                        "message_delta" => {
                            if let Some(delta) = &msg.delta {
                                in_flight.stop_reason = delta.stop_reason.clone();
                            }
                            if let Some(usage) = &msg.usage {
                                in_flight.usage = Some(usage.clone());
                            }
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    log!("Claude JSON parse error: {:?} for data: {}", e, event_data);
                }
            }
        }

        events
    }
}

impl AiBackend for ClaudeBackend {
    fn send_request(&mut self, cx: &mut Cx, request: AiRequest) -> RequestId {
        let request_id = RequestId::new();
        let http = self.build_http_request(&request);

        self.in_flight.insert(
            request_id.0,
            InFlightRequest {
                request_id,
                accumulated_text: String::new(),
                current_tool_id: None,
                current_tool_name: None,
                current_tool_json: String::new(),
                content_blocks: vec![],
                usage: None,
                stop_reason: None,
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
                        if let Some(mut in_flight) = self.in_flight.remove(&response.request_id) {
                            if !in_flight.accumulated_text.is_empty() {
                                let text = std::mem::take(&mut in_flight.accumulated_text);
                                in_flight.content_blocks.push(ContentBlock::Text { text });
                            }

                            let stop_reason = match in_flight.stop_reason.as_deref() {
                                Some("end_turn") => StopReason::EndTurn,
                                Some("max_tokens") => StopReason::MaxTokens,
                                Some("stop_sequence") => StopReason::StopSequence,
                                Some("tool_use") => StopReason::ToolUse,
                                _ => StopReason::EndTurn,
                            };

                            let usage = in_flight
                                .usage
                                .map(|u| Usage {
                                    input_tokens: u.input_tokens.unwrap_or(0),
                                    output_tokens: u.output_tokens.unwrap_or(0),
                                })
                                .unwrap_or_default();

                            ai_events.push(AiEvent::Complete {
                                request_id: in_flight.request_id,
                                response: AiResponse {
                                    message: Message {
                                        role: MessageRole::Assistant,
                                        content: in_flight.content_blocks,
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
