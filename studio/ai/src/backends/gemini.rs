use crate::backend::*;
use crate::types::*;
use makepad_micro_serde::*;
use makepad_widgets::*;
use std::collections::HashMap;

// === Gemini API Response Types ===

#[derive(DeJson, Debug)]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    #[rename(usageMetadata)]
    usage_metadata: Option<GeminiUsageMetadata>,
    error: Option<GeminiError>,
    #[rename(modelVersion)]
    #[allow(dead_code)]
    model_version: Option<String>,
    #[rename(responseId)]
    #[allow(dead_code)]
    response_id: Option<String>,
}

#[derive(DeJson, Debug)]
struct GeminiCandidate {
    content: Option<GeminiContent>,
    #[rename(finishReason)]
    finish_reason: Option<String>,
    #[allow(dead_code)]
    index: Option<u32>,
    #[rename(citationMetadata)]
    #[allow(dead_code)]
    citation_metadata: Option<GeminiCitationMetadata>,
}

#[derive(DeJson, Debug)]
struct GeminiCitationMetadata {
    #[rename(citationSources)]
    #[allow(dead_code)]
    citation_sources: Option<Vec<GeminiCitationSource>>,
}

#[derive(DeJson, Debug)]
struct GeminiCitationSource {
    #[rename(startIndex)]
    #[allow(dead_code)]
    start_index: Option<u32>,
    #[rename(endIndex)]
    #[allow(dead_code)]
    end_index: Option<u32>,
    #[allow(dead_code)]
    uri: Option<String>,
}

#[derive(DeJson, Debug, Clone)]
struct GeminiContent {
    #[allow(dead_code)]
    role: Option<String>,
    parts: Vec<GeminiPart>,
}

#[derive(DeJson, Debug, Clone)]
struct GeminiPart {
    text: Option<String>,
    #[rename(functionCall)]
    function_call: Option<GeminiFunctionCall>,
}

#[derive(DeJson, Debug, Clone)]
struct GeminiFunctionCall {
    name: String,
    args: Option<String>,
}

#[derive(DeJson, Debug, Clone)]
struct GeminiTokenDetails {
    #[allow(dead_code)]
    modality: Option<String>,
    #[rename(tokenCount)]
    #[allow(dead_code)]
    token_count: Option<u32>,
}

#[derive(DeJson, Debug, Clone)]
struct GeminiUsageMetadata {
    #[rename(promptTokenCount)]
    prompt_token_count: Option<u32>,
    #[rename(candidatesTokenCount)]
    candidates_token_count: Option<u32>,
    #[rename(totalTokenCount)]
    #[allow(dead_code)]
    total_token_count: Option<u32>,
    #[rename(promptTokensDetails)]
    #[allow(dead_code)]
    prompt_tokens_details: Option<Vec<GeminiTokenDetails>>,
    #[rename(candidatesTokensDetails)]
    #[allow(dead_code)]
    candidates_tokens_details: Option<Vec<GeminiTokenDetails>>,
    #[rename(thoughtsTokenCount)]
    #[allow(dead_code)]
    thoughts_token_count: Option<u32>,
}

#[derive(DeJson, Debug)]
struct GeminiError {
    #[allow(dead_code)]
    code: Option<u32>,
    message: Option<String>,
    #[allow(dead_code)]
    status: Option<String>,
}

// === In-flight Request Tracking ===

struct InFlightRequest {
    request_id: RequestId,
    accumulated_text: String,
    content_blocks: Vec<ContentBlock>,
    usage: Option<GeminiUsageMetadata>,
    finish_reason: Option<String>,
}

// === Gemini Backend Implementation ===

pub struct GeminiBackend {
    config: BackendConfig,
    in_flight: HashMap<LiveId, InFlightRequest>,
}

impl GeminiBackend {
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

    fn build_request_json(request: &AiRequest) -> String {
        let mut json = String::new();
        json.push_str("{");

        // Contents (messages)
        json.push_str("\"contents\":[");
        let mut first = true;
        for msg in &request.messages {
            if msg.role == MessageRole::System {
                continue; // Handled separately
            }

            if !first {
                json.push(',');
            }
            first = false;

            let role = match msg.role {
                MessageRole::User | MessageRole::Tool => "user",
                MessageRole::Assistant => "model",
                MessageRole::System => continue,
            };

            json.push_str("{");
            json.push_str(&format!("\"role\":\"{}\",", role));
            json.push_str("\"parts\":[");

            let mut first_part = true;
            for block in &msg.content {
                if !first_part {
                    json.push(',');
                }
                first_part = false;

                match block {
                    ContentBlock::Text { text } => {
                        json.push_str(&format!(
                            "{{\"text\":\"{}\"}}",
                            Self::escape_json_string(text)
                        ));
                    }
                    ContentBlock::Image { media_type, data } => {
                        json.push_str(&format!(
                            "{{\"inlineData\":{{\"mimeType\":\"{}\",\"data\":\"{}\"}}}}",
                            media_type, data
                        ));
                    }
                    ContentBlock::ToolUse { name, input, .. } => {
                        json.push_str(&format!(
                            "{{\"functionCall\":{{\"name\":\"{}\",\"args\":{}}}}}",
                            name, input
                        ));
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        json.push_str(&format!(
                            "{{\"functionResponse\":{{\"name\":\"function\",\"response\":{{\"result\":\"{}\"}}}}}}",
                            Self::escape_json_string(content)
                        ));
                    }
                }
            }

            json.push_str("]}");
        }
        json.push(']');

        // System instruction
        if let Some(system) = &request.system_prompt {
            json.push_str(&format!(
                ",\"systemInstruction\":{{\"parts\":[{{\"text\":\"{}\"}}]}}",
                Self::escape_json_string(system)
            ));
        }

        // Generation config
        json.push_str(",\"generationConfig\":{");
        json.push_str(&format!("\"maxOutputTokens\":{}", request.max_tokens));
        if let Some(temp) = request.temperature {
            json.push_str(&format!(",\"temperature\":{}", temp));
        }
        json.push('}');

        json.push('}');
        json
    }

    fn build_http_request(&self, request: &AiRequest) -> HttpRequest {
        let BackendConfig::Gemini { api_key, model } = &self.config else {
            panic!("GeminiBackend requires Gemini config");
        };

        // Use streaming endpoint with SSE
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?alt=sse&key={}",
            model, api_key
        );

        let mut http = HttpRequest::new(url, HttpMethod::POST);
        http.set_is_streaming();
        http.set_header("Content-Type".to_string(), "application/json".to_string());

        let body = Self::build_request_json(request);
        http.set_string_body(body);
        http
    }

    fn process_stream_data(&mut self, live_id: LiveId, data: &str) -> Vec<AiEvent> {
        let mut events = vec![];

        let Some(in_flight) = self.in_flight.get_mut(&live_id) else {
            return events;
        };

        let request_id = in_flight.request_id;

        // Gemini uses "data: <json>\r\n\r\n" format (note: \r\n\r\n separator)
        // But may also use \n\n in some cases
        for chunk in data.split("\r\n\r\n").flat_map(|s| s.split("\n\n")) {
            let chunk = chunk.trim();
            if chunk.is_empty() {
                continue;
            }

            let json_data = if let Some(d) = chunk.strip_prefix("data: ") {
                d
            } else if chunk.starts_with('{') {
                chunk
            } else {
                continue;
            };

            match GeminiResponse::deserialize_json_lenient(json_data) {
                Ok(response) => {
                    // Check for error
                    if let Some(error) = response.error {
                        events.push(AiEvent::StreamDelta {
                            request_id,
                            delta: StreamDelta::Error {
                                message: error
                                    .message
                                    .unwrap_or_else(|| "Unknown error".to_string()),
                            },
                        });
                        continue;
                    }

                    // Update usage
                    if let Some(usage) = response.usage_metadata {
                        in_flight.usage = Some(usage);
                    }

                    // Process candidates
                    if let Some(candidates) = response.candidates {
                        for candidate in candidates {
                            if let Some(finish) = candidate.finish_reason {
                                in_flight.finish_reason = Some(finish);
                            }

                            if let Some(content) = candidate.content {
                                for part in content.parts {
                                    if let Some(text) = part.text {
                                        in_flight.accumulated_text.push_str(&text);
                                        events.push(AiEvent::StreamDelta {
                                            request_id,
                                            delta: StreamDelta::TextDelta { text },
                                        });
                                    }

                                    if let Some(func_call) = part.function_call {
                                        let id = format!("call_{}", in_flight.content_blocks.len());

                                        events.push(AiEvent::StreamDelta {
                                            request_id,
                                            delta: StreamDelta::ToolUseStart {
                                                id: id.clone(),
                                                name: func_call.name.clone(),
                                            },
                                        });

                                        if let Some(args) = &func_call.args {
                                            events.push(AiEvent::StreamDelta {
                                                request_id,
                                                delta: StreamDelta::ToolUseDelta {
                                                    partial_json: args.clone(),
                                                },
                                            });
                                        }

                                        in_flight.content_blocks.push(ContentBlock::ToolUse {
                                            id,
                                            name: func_call.name,
                                            input: func_call.args.unwrap_or_default(),
                                        });

                                        events.push(AiEvent::StreamDelta {
                                            request_id,
                                            delta: StreamDelta::ToolUseEnd,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    log!("Gemini JSON parse error: {:?} for data: {}", e, json_data);
                }
            }
        }

        events
    }
}

impl AiBackend for GeminiBackend {
    fn send_request(&mut self, cx: &mut Cx, request: AiRequest) -> RequestId {
        let request_id = RequestId::new();
        let http = self.build_http_request(&request);

        self.in_flight.insert(
            request_id.0,
            InFlightRequest {
                request_id,
                accumulated_text: String::new(),
                content_blocks: vec![],
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
                        if let Some(mut in_flight) = self.in_flight.remove(&response.request_id) {
                            // Add accumulated text as content block
                            if !in_flight.accumulated_text.is_empty() {
                                in_flight.content_blocks.insert(
                                    0,
                                    ContentBlock::Text {
                                        text: in_flight.accumulated_text,
                                    },
                                );
                            }

                            let stop_reason = match in_flight.finish_reason.as_deref() {
                                Some("STOP") => StopReason::EndTurn,
                                Some("MAX_TOKENS") => StopReason::MaxTokens,
                                Some("SAFETY") => StopReason::EndTurn,
                                _ => StopReason::EndTurn,
                            };

                            let usage = in_flight
                                .usage
                                .map(|u| Usage {
                                    input_tokens: u.prompt_token_count.unwrap_or(0),
                                    output_tokens: u.candidates_token_count.unwrap_or(0),
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
