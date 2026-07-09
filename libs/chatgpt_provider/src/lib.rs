use makepad_micro_serde::*;
use makepad_network::{HttpMethod, HttpRequest};
use rand::{rngs::OsRng, RngCore};
use std::convert::TryFrom;
use std::fmt;

const DEFAULT_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const DEFAULT_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const DEFAULT_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const DEFAULT_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEFAULT_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const DEFAULT_SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
const DEFAULT_ORIGINATOR: &str = "makepad-studio";
const DEFAULT_BETA_HEADER: &str = "responses=experimental";
const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 4096;
const PKCE_VERIFIER_BYTES: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatGptModel {
    Gpt54Mini,
    CodexMiniLatest,
    O4Mini,
    O3,
    Custom(String),
}

impl Default for ChatGptModel {
    fn default() -> Self {
        Self::Gpt54Mini
    }
}

impl ChatGptModel {
    pub fn built_in_models() -> &'static [&'static str] {
        &["gpt-5.4-mini", "codex-mini-latest", "o4-mini", "o3"]
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Gpt54Mini => "gpt-5.4-mini",
            Self::CodexMiniLatest => "codex-mini-latest",
            Self::O4Mini => "o4-mini",
            Self::O3 => "o3",
            Self::Custom(value) => value.as_str(),
        }
    }

    pub fn supports_parallel_tool_calls(&self) -> bool {
        !matches!(self, Self::O3)
    }

    pub fn max_output_tokens(&self) -> u32 {
        match self {
            Self::O3 => 64_000,
            Self::O4Mini => 16_384,
            Self::Gpt54Mini | Self::CodexMiniLatest | Self::Custom(_) => DEFAULT_MAX_OUTPUT_TOKENS,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatGptOAuthConfig {
    pub client_id: String,
    pub authorize_url: String,
    pub token_url: String,
    pub redirect_uri: String,
    pub scope: String,
}

impl ChatGptOAuthConfig {
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            authorize_url: DEFAULT_AUTHORIZE_URL.to_string(),
            token_url: DEFAULT_TOKEN_URL.to_string(),
            redirect_uri: DEFAULT_REDIRECT_URI.to_string(),
            scope: DEFAULT_SCOPE.to_string(),
        }
    }
}

impl Default for ChatGptOAuthConfig {
    fn default() -> Self {
        Self::new(DEFAULT_CLIENT_ID)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChatGptCredentials {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub account_id: Option<String>,
    pub expires_at_unix: Option<u64>,
}

impl ChatGptCredentials {
    pub fn is_expiring_soon(&self, now_unix: u64, lead_secs: u64) -> bool {
        self.expires_at_unix
            .map(|expires_at| expires_at <= now_unix.saturating_add(lead_secs))
            .unwrap_or(false)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatGptTool {
    pub name: String,
    pub description: String,
    pub parameters_json: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatGptMessageRole {
    System,
    User,
    Assistant,
    Tool,
}

impl ChatGptMessageRole {
    fn as_str(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatGptContentBlock {
    Text {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments_json: String,
    },
    ToolResult {
        tool_call_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatGptMessage {
    pub role: ChatGptMessageRole,
    pub content: Vec<ChatGptContentBlock>,
}

impl ChatGptMessage {
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: ChatGptMessageRole::System,
            content: vec![ChatGptContentBlock::Text { text: text.into() }],
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: ChatGptMessageRole::User,
            content: vec![ChatGptContentBlock::Text { text: text.into() }],
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: ChatGptMessageRole::Assistant,
            content: vec![ChatGptContentBlock::Text { text: text.into() }],
        }
    }

    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ChatGptContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

#[derive(Clone, Debug)]
pub struct ChatGptRequest {
    pub messages: Vec<ChatGptMessage>,
    pub model: ChatGptModel,
    pub max_output_tokens: u32,
    pub temperature: Option<f32>,
    pub tools: Vec<ChatGptTool>,
    pub stream: bool,
}

impl Default for ChatGptRequest {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            model: ChatGptModel::default(),
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            temperature: None,
            tools: Vec::new(),
            stream: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PkcePair {
    pub verifier: String,
    pub challenge: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatGptProvider {
    pub oauth: ChatGptOAuthConfig,
    pub credentials: ChatGptCredentials,
    pub model: ChatGptModel,
    pub originator: String,
    pub beta_header: String,
    pub responses_url: String,
}

impl ChatGptProvider {
    pub fn new(
        oauth: ChatGptOAuthConfig,
        credentials: ChatGptCredentials,
        model: ChatGptModel,
    ) -> Self {
        Self {
            oauth,
            credentials,
            model,
            originator: DEFAULT_ORIGINATOR.to_string(),
            beta_header: DEFAULT_BETA_HEADER.to_string(),
            responses_url: DEFAULT_RESPONSES_URL.to_string(),
        }
    }

    pub fn pkce_pair() -> PkcePair {
        let mut verifier_bytes = [0u8; PKCE_VERIFIER_BYTES];
        OsRng.fill_bytes(&mut verifier_bytes);
        let verifier = base64_url_encode(&verifier_bytes);
        let challenge = pkce_code_challenge(&verifier);
        PkcePair {
            verifier,
            challenge,
        }
    }

    pub fn authorize_url(&self, state: &str, pkce: &PkcePair) -> String {
        let mut url = String::new();
        url.push_str(&self.oauth.authorize_url);
        url.push('?');
        append_query_param(&mut url, "client_id", &self.oauth.client_id);
        append_query_param(&mut url, "redirect_uri", &self.oauth.redirect_uri);
        append_query_param(&mut url, "response_type", "code");
        append_query_param(&mut url, "scope", &self.oauth.scope);
        append_query_param(&mut url, "state", state);
        append_query_param(&mut url, "code_challenge", &pkce.challenge);
        append_query_param(&mut url, "code_challenge_method", "S256");
        url.trim_end_matches('&').to_string()
    }

    pub fn authorization_request_body(
        &self,
        code: &str,
        code_verifier: &str,
    ) -> Result<String, ChatGptError> {
        if self.oauth.client_id.trim().is_empty() {
            return Err(ChatGptError::MissingClientId);
        }
        if code.trim().is_empty() {
            return Err(ChatGptError::MissingAuthorizationCode);
        }
        if code_verifier.trim().is_empty() {
            return Err(ChatGptError::MissingPkceVerifier);
        }

        Ok(form_encode(&[
            ("grant_type", "authorization_code"),
            ("client_id", self.oauth.client_id.as_str()),
            ("code", code),
            ("code_verifier", code_verifier),
            ("redirect_uri", self.oauth.redirect_uri.as_str()),
        ]))
    }

    pub fn refresh_request_body(&self) -> Result<String, ChatGptError> {
        let refresh_token = self
            .credentials
            .refresh_token
            .as_deref()
            .ok_or(ChatGptError::MissingRefreshToken)?;

        if self.oauth.client_id.trim().is_empty() {
            return Err(ChatGptError::MissingClientId);
        }

        Ok(form_encode(&[
            ("grant_type", "refresh_token"),
            ("client_id", self.oauth.client_id.as_str()),
            ("refresh_token", refresh_token),
        ]))
    }

    pub fn build_authorization_request(&self, state: &str, pkce: &PkcePair) -> HttpRequest {
        let mut request = HttpRequest::new(
            format_authorization_url(&self.oauth, state, pkce),
            HttpMethod::GET,
        );
        request.set_header(
            "Accept".to_string(),
            "text/html,application/xhtml+xml".to_string(),
        );
        request
    }

    pub fn build_token_request(&self, body: String) -> HttpRequest {
        let mut request = HttpRequest::new(self.oauth.token_url.clone(), HttpMethod::POST);
        request.set_header(
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        );
        request.set_header("Accept".to_string(), "application/json".to_string());
        request.set_body_string(&body);
        request
    }

    pub fn build_responses_request(
        &self,
        request: &ChatGptRequest,
    ) -> Result<HttpRequest, ChatGptError> {
        let body = self.build_responses_body(request)?;
        let mut http = HttpRequest::new(self.responses_url.clone(), HttpMethod::POST);
        http.set_is_streaming();
        http.set_header("Content-Type".to_string(), "application/json".to_string());
        http.set_header("Accept".to_string(), "text/event-stream".to_string());
        http.set_header("originator".to_string(), self.originator.clone());
        http.set_header("OpenAI-Beta".to_string(), self.beta_header.clone());
        if self.credentials.access_token.trim().is_empty() {
            return Err(ChatGptError::MissingAccessToken);
        }
        let account_id = self
            .credentials
            .account_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .or_else(|| Self::extract_account_id_from_jwt(&self.credentials.access_token))
            .ok_or(ChatGptError::MissingAccountId)?;
        http.set_header("ChatGPT-Account-Id".to_string(), account_id);
        http.set_header(
            "Authorization".to_string(),
            format!("Bearer {}", self.credentials.access_token),
        );
        http.set_string_body(body);
        Ok(http)
    }

    pub fn build_responses_body(&self, request: &ChatGptRequest) -> Result<String, ChatGptError> {
        if request.model.as_str().trim().is_empty() {
            return Err(ChatGptError::MissingModel);
        }

        let mut instructions = Vec::new();
        let mut input_messages = Vec::new();

        for message in &request.messages {
            if matches!(message.role, ChatGptMessageRole::System) {
                let text = message.text();
                if !text.trim().is_empty() {
                    instructions.push(text);
                }
                continue;
            }
            input_messages.push(message);
        }

        let mut out = String::new();
        out.push('{');
        push_json_string_field(&mut out, "model", request.model.as_str());
        out.push(',');
        if !instructions.is_empty() {
            push_json_string_field(&mut out, "instructions", &instructions.join("\n\n"));
            out.push(',');
        }
        out.push_str("\"input\":[");
        let mut first_message = true;
        for message in input_messages {
            append_input_items_json(&mut out, &mut first_message, message)?;
        }
        out.push(']');
        out.push(',');

        if !request.tools.is_empty() {
            out.push_str("\"tools\":[");
            for (index, tool) in request.tools.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str("{\"type\":\"function\",\"name\":");
                out.push_str(&json_string(&tool.name));
                out.push_str(",\"description\":");
                out.push_str(&json_string(&tool.description));
                out.push_str(",\"parameters\":");
                out.push_str(&tool.parameters_json);
                out.push('}');
            }
            out.push_str("],\"tool_choice\":\"auto\",");
        }

        if let Some(temperature) = request.temperature {
            out.push_str("\"temperature\":");
            out.push_str(&temperature.to_string());
            out.push(',');
        }
        out.push_str("\"stream\":");
        out.push_str(if request.stream { "true" } else { "false" });
        out.push(',');
        out.push_str("\"store\":false");
        out.push('}');
        Ok(out)
    }

    pub fn extract_account_id_from_jwt(jwt: &str) -> Option<String> {
        let mut parts = jwt.split('.');
        let _header = parts.next()?;
        let payload = parts.next()?;
        let payload = base64_url_decode(payload)?;
        let text = String::from_utf8(payload).ok()?;
        let value = JsonValue::deserialize_json_lenient(&text).ok()?;
        extract_account_id_from_json(&value)
    }

    pub fn parse_stream_chunk(chunk: &str) -> Result<Vec<ChatGptStreamEvent>, ChatGptError> {
        let mut events = Vec::new();
        for event in chunk.split("\n\n") {
            let event = event.trim();
            if event.is_empty() {
                continue;
            }
            let (event_name, data) = parse_sse_event(event);
            let Some(data) = data else {
                continue;
            };
            if data == "[DONE]" {
                events.push(ChatGptStreamEvent::Completed {
                    finish_reason: None,
                    usage: None,
                });
                continue;
            }
            let value = JsonValue::deserialize_json_lenient(&data)
                .map_err(|err| ChatGptError::InvalidResponse(format!("{:?}", err)))?;
            if let Some(message) = extract_error_message(&value) {
                events.push(ChatGptStreamEvent::Error { message });
                continue;
            }
            let event_name = event_name
                .or_else(|| json_string_field(&value, "type").map(str::to_string))
                .unwrap_or_default();

            if let Some(text) = extract_delta_text(&value, &event_name) {
                events.push(ChatGptStreamEvent::TextDelta { text });
            }
            if let Some(text) = extract_completed_text(&value, &event_name) {
                events.push(ChatGptStreamEvent::TextSnapshot { text });
            }

            if let Some(tool_call) = extract_tool_call(&value, &event_name) {
                events.push(ChatGptStreamEvent::ToolCallStart {
                    id: tool_call.id,
                    name: tool_call.name,
                });
                if !tool_call.arguments_json.is_empty() {
                    events.push(ChatGptStreamEvent::ToolCallArgumentsDelta {
                        partial_json: tool_call.arguments_json,
                    });
                }
            }

            if is_completion_event(&event_name, &value) {
                events.push(ChatGptStreamEvent::Completed {
                    finish_reason: extract_finish_reason(&value),
                    usage: extract_usage(&value),
                });
            }
        }
        Ok(events)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatGptToolCall {
    pub id: String,
    pub name: String,
    pub arguments_json: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatGptUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatGptFinishReason {
    EndTurn,
    MaxTokens,
    ToolUse,
    StopSequence,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatGptAssistantTurn {
    pub text: String,
    pub tool_calls: Vec<ChatGptToolCall>,
    pub finish_reason: Option<ChatGptFinishReason>,
    pub usage: Option<ChatGptUsage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatGptStreamEvent {
    TextDelta {
        text: String,
    },
    TextSnapshot {
        text: String,
    },
    ToolCallStart {
        id: String,
        name: String,
    },
    ToolCallArgumentsDelta {
        partial_json: String,
    },
    Completed {
        finish_reason: Option<String>,
        usage: Option<ChatGptUsage>,
    },
    Error {
        message: String,
    },
}

#[derive(Default)]
pub struct ChatGptStreamState {
    text: String,
    tool_calls: Vec<ToolCallAccumulator>,
    finish_reason: Option<String>,
    usage: Option<ChatGptUsage>,
    done: bool,
}

impl ChatGptStreamState {
    pub fn apply(&mut self, event: ChatGptStreamEvent) {
        match event {
            ChatGptStreamEvent::TextDelta { text } => self.text.push_str(&text),
            ChatGptStreamEvent::TextSnapshot { text } => {
                if self.text.is_empty() {
                    self.text = text;
                }
            }
            ChatGptStreamEvent::ToolCallStart { id, name } => {
                self.tool_calls.push(ToolCallAccumulator {
                    id,
                    name,
                    arguments_json: String::new(),
                });
            }
            ChatGptStreamEvent::ToolCallArgumentsDelta { partial_json } => {
                if let Some(last) = self.tool_calls.last_mut() {
                    last.arguments_json.push_str(&partial_json);
                }
            }
            ChatGptStreamEvent::Completed {
                finish_reason,
                usage,
            } => {
                if finish_reason.is_some() {
                    self.finish_reason = finish_reason;
                }
                if usage.is_some() {
                    self.usage = usage;
                }
                self.done = true;
            }
            ChatGptStreamEvent::Error { .. } => {}
        }
    }

    pub fn finalize(self) -> Result<ChatGptAssistantTurn, ChatGptError> {
        let mut tool_calls = Vec::new();
        for call in self.tool_calls {
            if call.id.trim().is_empty() || call.name.trim().is_empty() {
                return Err(ChatGptError::InvalidResponse(
                    "streamed tool call missing id or name".to_string(),
                ));
            }
            tool_calls.push(ChatGptToolCall {
                id: call.id,
                name: call.name,
                arguments_json: call.arguments_json,
            });
        }
        Ok(ChatGptAssistantTurn {
            text: self.text,
            tool_calls,
            finish_reason: self.finish_reason.map(|reason| match reason.as_str() {
                "stop" | "end_turn" => ChatGptFinishReason::EndTurn,
                "length" | "max_tokens" => ChatGptFinishReason::MaxTokens,
                "tool_calls" | "tool_use" => ChatGptFinishReason::ToolUse,
                "stop_sequence" => ChatGptFinishReason::StopSequence,
                other => ChatGptFinishReason::Other(other.to_string()),
            }),
            usage: self.usage,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatGptError {
    MissingClientId,
    MissingAuthorizationCode,
    MissingPkceVerifier,
    MissingRefreshToken,
    MissingAccountId,
    MissingAccessToken,
    MissingModel,
    InvalidResponse(String),
}

impl fmt::Display for ChatGptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingClientId => write!(f, "missing OAuth client_id"),
            Self::MissingAuthorizationCode => write!(f, "missing authorization code"),
            Self::MissingPkceVerifier => write!(f, "missing PKCE verifier"),
            Self::MissingRefreshToken => write!(f, "missing refresh token"),
            Self::MissingAccountId => write!(f, "missing ChatGPT account id"),
            Self::MissingAccessToken => write!(f, "missing access token"),
            Self::MissingModel => write!(f, "missing model"),
            Self::InvalidResponse(message) => write!(f, "invalid response: {}", message),
        }
    }
}

impl std::error::Error for ChatGptError {}

#[derive(DeJson, Debug, Clone)]
pub struct ChatGptTokenResponse {
    #[allow(dead_code)]
    token_type: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    #[allow(dead_code)]
    scope: Option<String>,
    #[allow(dead_code)]
    id_token: Option<String>,
}

impl ChatGptTokenResponse {
    pub fn into_credentials(self, account_id: Option<String>, now_unix: u64) -> ChatGptCredentials {
        ChatGptCredentials {
            access_token: self.access_token.unwrap_or_default(),
            refresh_token: self.refresh_token,
            account_id,
            expires_at_unix: self.expires_in.map(|delta| now_unix.saturating_add(delta)),
        }
    }
}

#[derive(DeJson, Debug, Clone)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments_json: String,
}

fn format_authorization_url(oauth: &ChatGptOAuthConfig, state: &str, pkce: &PkcePair) -> String {
    let mut url = String::new();
    url.push_str(&oauth.authorize_url);
    url.push('?');
    append_query_param(&mut url, "client_id", &oauth.client_id);
    append_query_param(&mut url, "redirect_uri", &oauth.redirect_uri);
    append_query_param(&mut url, "response_type", "code");
    append_query_param(&mut url, "scope", &oauth.scope);
    append_query_param(&mut url, "state", state);
    append_query_param(&mut url, "code_challenge", &pkce.challenge);
    append_query_param(&mut url, "code_challenge_method", "S256");
    if url.ends_with('&') {
        url.pop();
    }
    url
}

fn append_query_param(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push('=');
    out.push_str(&percent_encode(value));
    out.push('&');
}

fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

fn form_encode(pairs: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (index, (key, value)) in pairs.iter().enumerate() {
        if index > 0 {
            out.push('&');
        }
        out.push_str(&percent_encode(key));
        out.push('=');
        out.push_str(&percent_encode(value));
    }
    out
}

fn base64_url_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((bytes.len() * 4 + 2) / 3);
    let mut index = 0;
    while index < bytes.len() {
        let b0 = bytes[index];
        let b1 = bytes.get(index + 1).copied().unwrap_or(0);
        let b2 = bytes.get(index + 2).copied().unwrap_or(0);

        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if index + 1 < bytes.len() {
            out.push(TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        }
        if index + 2 < bytes.len() {
            out.push(TABLE[(b2 & 0x3f) as usize] as char);
        }
        index += 3;
    }
    out
}

fn base64_url_decode(text: &str) -> Option<Vec<u8>> {
    let mut bits = 0u32;
    let mut bit_count = 0u8;
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            b'=' => continue,
            _ => return None,
        } as u32;
        bits = (bits << 6) | value;
        bit_count += 6;
        if bit_count >= 8 {
            bit_count -= 8;
            out.push(((bits >> bit_count) & 0xff) as u8);
        }
    }
    Some(out)
}

fn pkce_code_challenge(verifier: &str) -> String {
    let hash = makepad_network::digest::sha256_hash(verifier.as_bytes());
    base64_url_encode(&hash)
}

fn push_json_string_field(out: &mut String, key: &str, value: &str) {
    out.push('"');
    out.push_str(key);
    out.push_str("\":");
    out.push_str(&json_string(value));
}

fn json_string(value: &str) -> String {
    value.to_string().serialize_json()
}

fn append_input_items_json(
    out: &mut String,
    first_item: &mut bool,
    message: &ChatGptMessage,
) -> Result<(), ChatGptError> {
    let mut text_blocks = Vec::new();
    for block in &message.content {
        match block {
            ChatGptContentBlock::Text { text } => {
                if !text.trim().is_empty() {
                    text_blocks.push(text.as_str());
                }
            }
            ChatGptContentBlock::ToolCall {
                id,
                name,
                arguments_json,
            } => {
                append_pending_text_message(out, first_item, &message.role, &mut text_blocks);
                append_comma_if_needed(out, first_item);
                out.push_str("{\"type\":\"function_call\",\"call_id\":");
                out.push_str(&json_string(id));
                out.push_str(",\"name\":");
                out.push_str(&json_string(name));
                out.push_str(",\"arguments\":");
                out.push_str(&json_string(&normalized_tool_arguments(arguments_json)));
                out.push('}');
            }
            ChatGptContentBlock::ToolResult {
                tool_call_id,
                content,
                is_error: _,
            } => {
                append_pending_text_message(out, first_item, &message.role, &mut text_blocks);
                append_comma_if_needed(out, first_item);
                out.push_str("{\"type\":\"function_call_output\",\"call_id\":");
                out.push_str(&json_string(tool_call_id));
                out.push_str(",\"output\":");
                out.push_str(&json_string(content));
                out.push('}');
            }
        }
    }

    append_pending_text_message(out, first_item, &message.role, &mut text_blocks);
    Ok(())
}

fn append_pending_text_message(
    out: &mut String,
    first_item: &mut bool,
    role: &ChatGptMessageRole,
    text_blocks: &mut Vec<&str>,
) {
    if !text_blocks.is_empty() {
        append_comma_if_needed(out, first_item);
        append_text_message_json(out, role, text_blocks);
        text_blocks.clear();
    }
}

fn append_text_message_json(out: &mut String, role: &ChatGptMessageRole, text_blocks: &[&str]) {
    let text_type = match role {
        ChatGptMessageRole::Assistant => "output_text",
        _ => "input_text",
    };
    out.push('{');
    out.push_str("\"role\":");
    out.push_str(&json_string(role.as_str()));
    out.push_str(",\"content\":[");
    let mut first_block = true;
    for text in text_blocks {
        if !first_block {
            out.push(',');
        }
        first_block = false;
        out.push_str("{\"type\":\"");
        out.push_str(text_type);
        out.push_str("\",\"text\":");
        out.push_str(&json_string(text));
        out.push('}');
    }
    out.push(']');
    out.push('}');
}

fn append_comma_if_needed(out: &mut String, first_item: &mut bool) {
    if !*first_item {
        out.push(',');
    }
    *first_item = false;
}

fn normalized_tool_arguments(arguments_json: &str) -> String {
    let trimmed = arguments_json.trim();
    if trimmed.is_empty() {
        "{}".to_string()
    } else {
        trimmed.to_string()
    }
}

fn parse_sse_event(event: &str) -> (Option<String>, Option<String>) {
    let mut event_name = None;
    let mut data = String::new();
    for line in event.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            event_name = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            let value = value.strip_prefix(' ').unwrap_or(value);
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value);
        }
    }
    let data = if data.is_empty() { None } else { Some(data) };
    (event_name, data)
}

fn json_string_field<'a>(value: &'a JsonValue, key: &str) -> Option<&'a str> {
    value
        .key(key)
        .and_then(JsonValue::string)
        .map(String::as_str)
}

fn extract_error_message(value: &JsonValue) -> Option<String> {
    if let Some(error) = value.key("error") {
        if let Some(message) = json_string_field(error, "message") {
            return Some(message.to_string());
        }
    }
    if let Some(message) = json_string_field(value, "message") {
        let kind = json_string_field(value, "type");
        let status = json_string_field(value, "status");
        if kind.map(|kind| kind.contains("error")).unwrap_or(true)
            || status.map(|status| status == "error").unwrap_or(false)
        {
            return Some(message.to_string());
        }
    }
    None
}

fn extract_delta_text(value: &JsonValue, event_name: &str) -> Option<String> {
    let event_name = event_name.to_ascii_lowercase();
    if event_name.contains("text.delta") || event_name.contains("output_text.delta") {
        if let Some(text) = json_string_field(value, "delta") {
            return Some(text.to_string());
        }
        if let Some(text) = json_string_field(value, "text") {
            return Some(text.to_string());
        }
        if let Some(delta) = value.key("delta") {
            if let Some(text) =
                json_string_field(delta, "text").or_else(|| json_string_field(delta, "content"))
            {
                return Some(text.to_string());
            }
        }
    }
    if let Some(text) = json_string_field(value, "text") {
        if event_name.contains("message") || event_name.contains("output") {
            return Some(text.to_string());
        }
    }
    None
}

fn extract_completed_text(value: &JsonValue, event_name: &str) -> Option<String> {
    let event_name = event_name.to_ascii_lowercase();
    if !(event_name.contains("completed")
        || event_name.contains("done")
        || event_name.contains("output_item"))
    {
        return None;
    }
    let mut text = String::new();
    for candidate in [
        value.key("item"),
        value.key("output_item"),
        value.key("message"),
        value.key("response"),
    ]
    .into_iter()
    .flatten()
    {
        collect_completed_text(candidate, &mut text);
    }
    if text.is_empty() {
        collect_completed_text(value, &mut text);
    }
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn collect_completed_text(value: &JsonValue, out: &mut String) {
    if let Some(output) = value.key("output") {
        collect_completed_text(output, out);
    }
    if let Some(content) = value.key("content") {
        collect_completed_text(content, out);
    }
    if let JsonValue::Array(items) = value {
        for item in items {
            collect_completed_text(item, out);
        }
        return;
    }
    let item_type = json_string_field(value, "type").unwrap_or_default();
    let is_text_item = matches!(
        item_type,
        "output_text" | "text" | "message" | "assistant_message"
    ) || item_type.contains("output_text")
        || item_type.contains("text.done");
    if is_text_item {
        if let Some(text) = json_string_field(value, "text")
            .or_else(|| json_string_field(value, "output_text"))
            .or_else(|| json_string_field(value, "value"))
        {
            out.push_str(text);
            return;
        }
    }
    if let Some(text_value) = value.key("text") {
        if let Some(text) = text_value.string() {
            if is_text_item {
                out.push_str(text);
            }
        } else {
            collect_completed_text(text_value, out);
        }
    }
}

fn extract_tool_call(value: &JsonValue, event_name: &str) -> Option<ChatGptToolCall> {
    let event_name = event_name.to_ascii_lowercase();
    let candidates = [
        value.key("item"),
        value.key("output_item"),
        value.key("function_call"),
        value.key("tool_call"),
    ];
    for candidate in candidates.into_iter().flatten() {
        let id = json_string_field(candidate, "id")
            .or_else(|| json_string_field(candidate, "call_id"))
            .or_else(|| json_string_field(candidate, "tool_call_id"))?;
        let function = candidate.key("function");
        let name = json_string_field(candidate, "name")
            .or_else(|| function.and_then(|function| json_string_field(function, "name")))
            .or_else(|| json_string_field(candidate, "function"))?;
        let arguments_json = json_string_field(candidate, "arguments")
            .or_else(|| json_string_field(candidate, "input"))
            .or_else(|| function.and_then(|function| json_string_field(function, "arguments")))
            .or_else(|| json_string_field(candidate, "partial_json"))
            .map(str::to_string)
            .unwrap_or_default();
        return Some(ChatGptToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments_json,
        });
    }
    if event_name.contains("function_call.arguments.delta") {
        let id = json_string_field(value, "call_id")
            .or_else(|| json_string_field(value, "id"))
            .unwrap_or_default()
            .to_string();
        let name = json_string_field(value, "name")
            .unwrap_or_default()
            .to_string();
        let arguments_json = json_string_field(value, "delta")
            .or_else(|| json_string_field(value, "partial_json"))
            .unwrap_or_default()
            .to_string();
        return Some(ChatGptToolCall {
            id,
            name,
            arguments_json,
        });
    }
    None
}

fn extract_finish_reason(value: &JsonValue) -> Option<String> {
    json_string_field(value, "finish_reason")
        .or_else(|| json_string_field(value, "finishReason"))
        .or_else(|| json_string_field(value, "stop_reason"))
        .or_else(|| json_string_field(value, "reason"))
        .map(str::to_string)
}

fn extract_usage(value: &JsonValue) -> Option<ChatGptUsage> {
    let usage = value.key("usage").or_else(|| value.key("usage_metadata"))?;
    let input_tokens = number_field(usage, &["input_tokens", "prompt_tokens", "inputTokens"])?;
    let output_tokens = number_field(
        usage,
        &[
            "output_tokens",
            "completion_tokens",
            "outputTokens",
            "candidates_token_count",
        ],
    )?;
    let total_tokens = number_field(usage, &["total_tokens", "totalTokens", "total_token_count"])
        .unwrap_or(input_tokens.saturating_add(output_tokens));
    Some(ChatGptUsage {
        input_tokens,
        output_tokens,
        total_tokens,
    })
}

fn number_field(value: &JsonValue, keys: &[&str]) -> Option<u32> {
    for key in keys {
        if let Some(found) = value.key(key) {
            match found {
                JsonValue::U64(number) => return u32::try_from(*number).ok(),
                JsonValue::U128(number) => return u32::try_from(*number).ok(),
                JsonValue::I64(number) => return u32::try_from(*number).ok(),
                JsonValue::I128(number) => return u32::try_from(*number).ok(),
                JsonValue::F64(number) if *number >= 0.0 => return Some(*number as u32),
                _ => {}
            }
        }
    }
    None
}

fn is_completion_event(event_name: &str, value: &JsonValue) -> bool {
    let lowered = event_name.to_ascii_lowercase();
    lowered.contains("completed")
        || lowered.contains("done")
        || lowered.contains("response.completed")
        || json_string_field(value, "status").is_some_and(|status| status == "completed")
}

fn extract_account_id_from_json(value: &JsonValue) -> Option<String> {
    if let Some(id) = json_string_field(value, "account_id") {
        return Some(id.to_string());
    }
    if let Some(id) = json_string_field(value, "chatgpt_account_id") {
        return Some(id.to_string());
    }
    if let Some(id) = json_string_field(value, "chatgptAccountId") {
        return Some(id.to_string());
    }
    if let Some(id) = json_string_field(value, "sub") {
        return Some(id.to_string());
    }
    if let Some(object) = value.object() {
        for child in object.values() {
            if let Some(found) = extract_account_id_from_json(child) {
                return Some(found);
            }
        }
    }
    if let JsonValue::Array(items) = value {
        for item in items {
            if let Some(found) = extract_account_id_from_json(item) {
                return Some(found);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_rfc_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            pkce_code_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn authorize_url_includes_expected_query_params() {
        let oauth = ChatGptOAuthConfig {
            client_id: "client".to_string(),
            authorize_url: DEFAULT_AUTHORIZE_URL.to_string(),
            token_url: DEFAULT_TOKEN_URL.to_string(),
            redirect_uri: DEFAULT_REDIRECT_URI.to_string(),
            scope: "openid profile".to_string(),
        };
        let provider = ChatGptProvider::new(
            oauth,
            ChatGptCredentials::default(),
            ChatGptModel::default(),
        );
        let pkce = PkcePair {
            verifier: "verifier".to_string(),
            challenge: "challenge".to_string(),
        };
        let url = provider.authorize_url("state", &pkce);
        assert!(url.contains("client_id=client"));
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("code_challenge=challenge"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=state"));
    }

    #[test]
    fn responses_request_carries_required_headers_and_body() {
        let provider = ChatGptProvider::new(
            ChatGptOAuthConfig::new("client"),
            ChatGptCredentials {
                access_token: "token".to_string(),
                refresh_token: Some("refresh".to_string()),
                account_id: Some("acct_123".to_string()),
                expires_at_unix: Some(100),
            },
            ChatGptModel::CodexMiniLatest,
        );
        let request = ChatGptRequest {
            messages: vec![
                ChatGptMessage::system("system one"),
                ChatGptMessage::user("hello"),
                ChatGptMessage {
                    role: ChatGptMessageRole::Assistant,
                    content: vec![
                        ChatGptContentBlock::Text {
                            text: "working".to_string(),
                        },
                        ChatGptContentBlock::ToolCall {
                            id: "call_1".to_string(),
                            name: "read_file".to_string(),
                            arguments_json: "{\"path\":\"src/lib.rs\"}".to_string(),
                        },
                    ],
                },
                ChatGptMessage {
                    role: ChatGptMessageRole::Assistant,
                    content: vec![ChatGptContentBlock::ToolCall {
                        id: "call_2".to_string(),
                        name: "list_files".to_string(),
                        arguments_json: String::new(),
                    }],
                },
                ChatGptMessage {
                    role: ChatGptMessageRole::Tool,
                    content: vec![ChatGptContentBlock::ToolResult {
                        tool_call_id: "call_2".to_string(),
                        content: "[]".to_string(),
                        is_error: false,
                    }],
                },
            ],
            model: ChatGptModel::O4Mini,
            max_output_tokens: 1234,
            temperature: Some(0.2),
            tools: vec![ChatGptTool {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters_json: "{\"type\":\"object\"}".to_string(),
            }],
            stream: true,
        };
        let http = provider.build_responses_request(&request).unwrap();
        assert!(http.url.contains("/backend-api/codex/responses"));
        assert_eq!(
            http.headers
                .get("originator")
                .and_then(|values| values.first())
                .map(String::as_str),
            Some("makepad-studio")
        );
        assert_eq!(
            http.headers
                .get("OpenAI-Beta")
                .and_then(|values| values.first())
                .map(String::as_str),
            Some("responses=experimental")
        );
        assert_eq!(
            http.headers
                .get("ChatGPT-Account-Id")
                .and_then(|values| values.first())
                .map(String::as_str),
            Some("acct_123")
        );
        let body = http
            .body
            .as_ref()
            .map(|body| String::from_utf8_lossy(body).to_string())
            .unwrap();
        assert!(body.contains("\"store\":false"));
        assert!(body.contains("\"model\":\"o4-mini\""));
        assert!(body.contains("\"instructions\":\"system one\""));
        assert!(body.contains("\"tool_choice\":\"auto\""));
        assert!(!body.contains("\"max_output_tokens\""));
        assert!(body.contains("\"temperature\":0.2"));
        assert!(body.contains(
            "\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"hello\"}]"
        ));
        assert!(body.contains(
            "\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"working\"}]"
        ));
        assert!(!body.contains("\"role\":\"assistant\",\"content\":[{\"type\":\"input_text\""));
        assert!(body.contains("\"type\":\"function_call\",\"call_id\":\"call_1\""));
        assert!(body.contains("\"name\":\"read_file\""));
        assert!(body.contains("\"arguments\":\"{\\\"path\\\":\\\"src/lib.rs\\\"}\""));
        assert!(body.contains("\"type\":\"function_call\",\"call_id\":\"call_2\""));
        assert!(body.contains("\"arguments\":\"{}\""));
        assert!(body.contains("\"type\":\"function_call_output\",\"call_id\":\"call_2\""));
        assert!(!body.contains("\"is_error\""));
        assert!(!body.contains("\"type\":\"tool_call\""));
    }

    #[test]
    fn token_request_bodies_are_form_encoded() {
        let provider = ChatGptProvider::new(
            ChatGptOAuthConfig::new("client"),
            ChatGptCredentials {
                access_token: String::new(),
                refresh_token: Some("refresh".to_string()),
                account_id: Some("acct".to_string()),
                expires_at_unix: None,
            },
            ChatGptModel::default(),
        );
        let body = provider
            .authorization_request_body("code", "verifier")
            .unwrap();
        assert!(body.contains("grant_type=authorization_code"));
        assert!(body.contains("code=code"));
        assert!(body.contains("code_verifier=verifier"));
        let refresh_body = provider.refresh_request_body().unwrap();
        assert!(refresh_body.contains("grant_type=refresh_token"));
        assert!(refresh_body.contains("refresh_token=refresh"));
    }

    #[test]
    fn jwt_payload_account_id_can_be_extracted() {
        let payload = base64_url_encode(br#"{"account_id":"acct_123"}"#);
        let jwt = format!("header.{}.sig", payload);
        assert_eq!(
            ChatGptProvider::extract_account_id_from_jwt(&jwt),
            Some("acct_123".to_string())
        );
    }

    #[test]
    fn parses_basic_stream_chunks() {
        let events = ChatGptProvider::parse_stream_chunk(
            "event: response.output_text.delta\ndata: {\"delta\":\"hello\"}\n\n\
             event: response.completed\ndata: {\"type\":\"response.completed\",\"finish_reason\":\"stop\",\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}\n\n",
        )
        .unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            ChatGptStreamEvent::TextDelta { text } if text == "hello"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            ChatGptStreamEvent::Completed { finish_reason, .. }
            if finish_reason.as_deref() == Some("stop")
        )));
    }

    #[test]
    fn stream_state_finalizes_tool_calls() {
        let mut state = ChatGptStreamState::default();
        state.apply(ChatGptStreamEvent::TextDelta {
            text: "hello".to_string(),
        });
        state.apply(ChatGptStreamEvent::TextSnapshot {
            text: "hello".to_string(),
        });
        state.apply(ChatGptStreamEvent::ToolCallStart {
            id: "call_1".to_string(),
            name: "read_file".to_string(),
        });
        state.apply(ChatGptStreamEvent::ToolCallArgumentsDelta {
            partial_json: "{\"path\":\"src/lib.rs\"}".to_string(),
        });
        state.apply(ChatGptStreamEvent::Completed {
            finish_reason: Some("tool_calls".to_string()),
            usage: Some(ChatGptUsage {
                input_tokens: 1,
                output_tokens: 2,
                total_tokens: 3,
            }),
        });
        let turn = state.finalize().unwrap();
        assert_eq!(turn.text, "hello");
        assert_eq!(turn.tool_calls.len(), 1);
        assert!(matches!(
            turn.finish_reason,
            Some(ChatGptFinishReason::ToolUse)
        ));
        assert_eq!(turn.usage.unwrap().total_tokens, 3);
    }

    #[test]
    fn final_text_snapshot_does_not_duplicate_streamed_delta() {
        let events = ChatGptProvider::parse_stream_chunk(
            "event: response.output_text.delta\ndata: {\"delta\":\"hello\"}\n\n\
             event: response.output_item.done\ndata: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}}\n\n\
             event: response.completed\ndata: {\"type\":\"response.completed\",\"status\":\"completed\",\"finish_reason\":\"stop\"}\n\n",
        )
        .unwrap();
        assert!(events.iter().any(
            |event| matches!(event, ChatGptStreamEvent::TextDelta { text } if text == "hello")
        ));
        assert!(events.iter().any(
            |event| matches!(event, ChatGptStreamEvent::TextSnapshot { text } if text == "hello")
        ));

        let mut state = ChatGptStreamState::default();
        for event in events {
            state.apply(event);
        }
        let turn = state.finalize().unwrap();
        assert_eq!(turn.text, "hello");
    }
}
