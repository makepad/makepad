//! Anthropic Messages API chat provider.
//!
//! The real transport is the hub's dependency-free blocking HTTP client on
//! a detached worker. Anthropic's bounded SSE response is parsed without a
//! UI `Cx`; `poll` stays non-blocking and preserves the upstream text-delta
//! order. Production configuration pins `api.anthropic.com`; tests replace
//! only the transport.

use crate::chat_wire::{
    sanitize_public_error, split_delta_text, ChatMessage, ChatRole, ProviderAvailability,
    ProviderKind, MAX_MESSAGES, MAX_TOOL_CALL_ID, MAX_TOOL_JSON_BYTES, MAX_TURN_TEXT_BYTES,
};
use crate::providers::provider::{ChatProvider, ProviderEvent, TurnInput};
use makepad_network::blocking_http::{
    post_json, CancelToken, Error as HttpError, Limits, Request,
};
use makepad_strict_json::{self as json, Value};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Duration;

pub const CLAUDE_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
pub const ANTHROPIC_BETA: &str =
    "oauth-2025-04-20,interleaved-thinking-2025-05-14,output-128k-2025-02-19";
pub const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
pub const CLAUDE_CODE_OAUTH_TOKEN_ENV: &str = "CLAUDE_CODE_OAUTH_TOKEN";
pub const CLAUDE_MODEL_ENV: &str = "MAKEPAD_CONTENT_CHAT_CLAUDE_MODEL";
pub const DEFAULT_CLAUDE_MODEL: &str = "claude-sonnet-5";
pub const DEFAULT_MAX_TOKENS: u32 = 4096;
pub const DEFAULT_CLAUDE_TIMEOUT: Duration = Duration::from_secs(120);
pub const MAX_CLAUDE_BODY: usize = 256 * 1024;

const MAX_MODEL_BYTES: usize = 128;
const MAX_FUNCTION_NAME_BYTES: usize = 64;
const MAX_SSE_LINE_BYTES: usize = 64 * 1024;
const MAX_SSE_EVENT_BYTES: usize = 64 * 1024;

/// Opaque Claude credential. It deliberately implements neither `Debug`
/// nor `Display`.
#[derive(Clone)]
pub struct ClaudeAuth {
    kind: AuthKind,
    secret: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AuthKind {
    ApiKey,
    OAuth,
}

impl ClaudeAuth {
    pub fn api_key(value: impl Into<String>) -> Result<Self, String> {
        Self::new(AuthKind::ApiKey, value.into())
    }

    pub fn oauth_token(value: impl Into<String>) -> Result<Self, String> {
        Self::new(AuthKind::OAuth, value.into())
    }

    fn new(kind: AuthKind, secret: String) -> Result<Self, String> {
        if secret.is_empty()
            || secret.len() > 8 * 1024
            || secret.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
        {
            return Err("credential is empty or invalid".to_string());
        }
        Ok(ClaudeAuth { kind, secret })
    }

    pub fn is_oauth(&self) -> bool {
        self.kind == AuthKind::OAuth
    }

    fn secret(&self) -> &str {
        &self.secret
    }
}

/// Provider configuration. It deliberately implements neither `Debug` nor
/// `Display`, because it owns a credential.
pub struct ClaudeApiConfig {
    kind: ProviderKind,
    auth: Option<ClaudeAuth>,
    model: String,
    endpoint: String,
    max_tokens: u32,
    request_timeout: Duration,
    unavailable_reason: String,
}

impl ClaudeApiConfig {
    /// Production config. The endpoint is always Anthropic's Messages API.
    pub fn new(kind: ProviderKind, auth: ClaudeAuth, model: impl Into<String>) -> Self {
        ClaudeApiConfig {
            kind,
            auth: Some(auth),
            model: bound_model(model.into()),
            endpoint: CLAUDE_MESSAGES_URL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            request_timeout: DEFAULT_CLAUDE_TIMEOUT,
            unavailable_reason: missing_credential_reason(),
        }
    }

    /// Honest no-credential config, useful to callers that resolve secrets
    /// outside the environment.
    pub fn without_credentials(kind: ProviderKind, model: impl Into<String>) -> Self {
        ClaudeApiConfig {
            kind,
            auth: None,
            model: bound_model(model.into()),
            endpoint: CLAUDE_MESSAGES_URL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            request_timeout: DEFAULT_CLAUDE_TIMEOUT,
            unavailable_reason: missing_credential_reason(),
        }
    }

    /// OAuth has the same precedence as the old Cx backend: use it when
    /// present, otherwise use `ANTHROPIC_API_KEY`.
    pub fn from_env(kind: ProviderKind) -> Self {
        let model = std::env::var(CLAUDE_MODEL_ENV)
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_CLAUDE_MODEL.to_string());
        let oauth = std::env::var(CLAUDE_CODE_OAUTH_TOKEN_ENV)
            .ok()
            .filter(|value| !value.is_empty());
        let api_key = std::env::var(ANTHROPIC_API_KEY_ENV)
            .ok()
            .filter(|value| !value.is_empty());
        let (auth, unavailable_reason) = if let Some(token) = oauth {
            match ClaudeAuth::oauth_token(token) {
                Ok(auth) => (Some(auth), missing_credential_reason()),
                Err(_) => (
                    None,
                    format!("{CLAUDE_CODE_OAUTH_TOKEN_ENV} is invalid"),
                ),
            }
        } else if let Some(key) = api_key {
            match ClaudeAuth::api_key(key) {
                Ok(auth) => (Some(auth), missing_credential_reason()),
                Err(_) => (None, format!("{ANTHROPIC_API_KEY_ENV} is invalid")),
            }
        } else {
            (None, missing_credential_reason())
        };
        ClaudeApiConfig {
            kind,
            auth,
            model: bound_model(model),
            endpoint: CLAUDE_MESSAGES_URL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            request_timeout: DEFAULT_CLAUDE_TIMEOUT,
            unavailable_reason,
        }
    }

    pub fn kind(&self) -> ProviderKind {
        self.kind
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn with_max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = tokens.clamp(1, 128 * 1024);
        self
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout
            .max(Duration::from_secs(5))
            .min(Duration::from_secs(600));
        self
    }
}

fn missing_credential_reason() -> String {
    format!("{ANTHROPIC_API_KEY_ENV} or {CLAUDE_CODE_OAUTH_TOKEN_ENV} is not set")
}

fn bound_model(model: String) -> String {
    if model.is_empty()
        || model.len() > MAX_MODEL_BYTES
        || model.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
    {
        DEFAULT_CLAUDE_MODEL.to_string()
    } else {
        model
    }
}

/// HTTP seam for deterministic tests. Implementations must not include the
/// credential, request headers, request body, or response body in errors.
pub trait ClaudeApiTransport: Send + Sync + 'static {
    fn post_messages(
        &self,
        url: &str,
        auth: &ClaudeAuth,
        body: &[u8],
        cancel: &CancelToken,
        timeout: Duration,
    ) -> Result<ClaudeRawHttp, String>;
}

pub struct ClaudeRawHttp {
    pub status: u16,
    pub body: Vec<u8>,
}

pub struct BlockingClaudeTransport;

impl ClaudeApiTransport for BlockingClaudeTransport {
    fn post_messages(
        &self,
        url: &str,
        auth: &ClaudeAuth,
        body: &[u8],
        cancel: &CancelToken,
        timeout: Duration,
    ) -> Result<ClaudeRawHttp, String> {
        let request = Request::post(url)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .map_err(safe_http_err)?;
        let request = if auth.is_oauth() {
            request
                .bearer(auth.secret())
                .map_err(safe_http_err)?
                .header("anthropic-beta", ANTHROPIC_BETA)
                .map_err(safe_http_err)?
        } else {
            request
                .header("x-api-key", auth.secret())
                .map_err(safe_http_err)?
        };
        let request = request
            .json_body(body.to_vec())
            .map_err(safe_http_err)?
            .cancel_token(cancel.clone())
            .limits(Limits {
                max_body_bytes: MAX_CLAUDE_BODY,
                total_timeout: timeout,
                ..Limits::default()
            });
        match post_json(request) {
            Ok(response) => Ok(ClaudeRawHttp {
                status: response.status,
                body: response.body,
            }),
            Err(error) => Err(safe_http_err(error)),
        }
    }
}

fn safe_http_err(error: HttpError) -> String {
    error.to_string()
}

#[derive(Debug, PartialEq, Eq)]
struct SseEvent {
    event_type: String,
    data: String,
}

/// Incremental, bounded SSE line decoder. Bytes can end anywhere, including
/// inside CRLF or a multi-byte UTF-8 scalar.
#[derive(Default)]
struct SseDecoder {
    pending_line: Vec<u8>,
    event_type: String,
    data: String,
}

impl SseDecoder {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, String> {
        let mut events = Vec::new();
        let mut start = 0;
        while let Some(offset) = chunk[start..].iter().position(|byte| *byte == b'\n') {
            let end = start + offset;
            self.extend_line(&chunk[start..end])?;
            let line = std::mem::take(&mut self.pending_line);
            self.process_line(&line, &mut events)?;
            start = end + 1;
        }
        self.extend_line(&chunk[start..])?;
        Ok(events)
    }

    fn finish(&mut self) -> Result<Vec<SseEvent>, String> {
        let mut events = Vec::new();
        if !self.pending_line.is_empty() {
            let line = std::mem::take(&mut self.pending_line);
            self.process_line(&line, &mut events)?;
        }
        self.dispatch(&mut events);
        Ok(events)
    }

    fn extend_line(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.pending_line.len().saturating_add(bytes.len()) > MAX_SSE_LINE_BYTES {
            return Err("sse line too large".to_string());
        }
        self.pending_line.extend_from_slice(bytes);
        Ok(())
    }

    fn process_line(&mut self, raw: &[u8], events: &mut Vec<SseEvent>) -> Result<(), String> {
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        let line = std::str::from_utf8(raw).map_err(|_| "malformed sse utf-8".to_string())?;
        if line.is_empty() {
            self.dispatch(events);
            return Ok(());
        }
        if line.starts_with(':') {
            return Ok(());
        }
        let (field, mut value) = line.split_once(':').unwrap_or((line, ""));
        if let Some(rest) = value.strip_prefix(' ') {
            value = rest;
        }
        match field {
            "event" => {
                if value.len() > 64 {
                    return Err("sse event name too large".to_string());
                }
                self.event_type.clear();
                self.event_type.push_str(value);
            }
            "data" => {
                let separator = usize::from(!self.data.is_empty());
                if self.data.len().saturating_add(separator).saturating_add(value.len())
                    > MAX_SSE_EVENT_BYTES
                {
                    return Err("sse event data too large".to_string());
                }
                if separator != 0 {
                    self.data.push('\n');
                }
                self.data.push_str(value);
            }
            _ => {}
        }
        Ok(())
    }

    fn dispatch(&mut self, events: &mut Vec<SseEvent>) {
        if !self.data.is_empty() {
            events.push(SseEvent {
                event_type: std::mem::take(&mut self.event_type),
                data: std::mem::take(&mut self.data),
            });
        } else {
            self.event_type.clear();
        }
    }
}

#[derive(Debug)]
pub struct ParsedClaudeStream {
    pub text: String,
    pub deltas: Vec<String>,
    pub function_call: Option<ParsedClaudeToolUse>,
}

#[derive(Clone, Debug)]
pub struct ParsedClaudeToolUse {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

enum BlockState {
    Text,
    Tool {
        call_id: String,
        name: String,
        partial_json: String,
        initial_input: String,
    },
    Ignored,
}

#[derive(Default)]
struct ClaudeStreamParser {
    text: String,
    deltas: Vec<String>,
    block: Option<BlockState>,
    function_call: Option<ParsedClaudeToolUse>,
    stop_reason: Option<String>,
    saw_message_stop: bool,
}

impl ClaudeStreamParser {
    fn event(&mut self, event: SseEvent) -> Result<(), String> {
        match event.event_type.as_str() {
            "ping" => return Ok(()),
            // Forward-compatible Anthropic events are deliberately ignored,
            // including their data shape.
            "message_start" | "content_block_start" | "content_block_delta"
            | "content_block_stop" | "message_delta" | "message_stop" | "error" => {}
            _ => return Ok(()),
        }
        let value = json::parse(event.data.as_bytes())
            .map_err(|_| "malformed claude stream json".to_string())?;
        match event.event_type.as_str() {
            "message_start" => Ok(()),
            "content_block_start" => self.start_block(&value),
            "content_block_delta" => self.delta_block(&value),
            "content_block_stop" => self.stop_block(),
            "message_delta" => self.message_delta(&value),
            "message_stop" => {
                if self.block.is_some() {
                    return Err("message stopped inside a content block".to_string());
                }
                self.saw_message_stop = true;
                Ok(())
            }
            "error" => Err(claude_error_category(value.get("error"))),
            _ => Ok(()),
        }
    }

    fn start_block(&mut self, value: &Value) -> Result<(), String> {
        if self.block.is_some() {
            return Err("overlapping content blocks".to_string());
        }
        let block = value
            .get("content_block")
            .ok_or_else(|| "content block missing".to_string())?;
        self.block = Some(match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text") {
                    let text = text
                        .as_str()
                        .ok_or_else(|| "text block has the wrong shape".to_string())?;
                    self.append_text(text)?;
                }
                BlockState::Text
            }
            Some("tool_use") => {
                let call_id = block.get("id").and_then(Value::as_str).unwrap_or("");
                validate_call_id(call_id)?;
                let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                validate_function_name(name)?;
                let initial_input = block
                    .get("input")
                    .cloned()
                    .unwrap_or_else(|| Value::Obj(Vec::new()))
                    .to_json();
                BlockState::Tool {
                    call_id: call_id.to_string(),
                    name: name.to_string(),
                    partial_json: String::new(),
                    initial_input,
                }
            }
            Some(_) => BlockState::Ignored,
            None => return Err("content block type missing".to_string()),
        });
        Ok(())
    }

    fn delta_block(&mut self, value: &Value) -> Result<(), String> {
        let delta = value
            .get("delta")
            .ok_or_else(|| "content block delta missing".to_string())?;
        let delta_type = delta.get("type").and_then(Value::as_str);
        match self.block.as_mut() {
            Some(BlockState::Text) if delta_type == Some("text_delta") => {
                let text = delta
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "text delta missing".to_string())?
                    .to_string();
                self.append_text(&text)
            }
            Some(BlockState::Tool { partial_json, .. })
                if delta_type == Some("input_json_delta") =>
            {
                let fragment = delta
                    .get("partial_json")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "tool input delta missing".to_string())?;
                if partial_json.len().saturating_add(fragment.len()) > MAX_TOOL_JSON_BYTES {
                    return Err("tool arguments too large".to_string());
                }
                partial_json.push_str(fragment);
                Ok(())
            }
            Some(_) => Ok(()),
            None => Err("content block delta without a block".to_string()),
        }
    }

    fn stop_block(&mut self) -> Result<(), String> {
        match self
            .block
            .take()
            .ok_or_else(|| "content block stop without a block".to_string())?
        {
            BlockState::Text | BlockState::Ignored => Ok(()),
            BlockState::Tool {
                call_id,
                name,
                partial_json,
                initial_input,
            } => {
                if self.function_call.is_some() {
                    return Err("multiple tool uses are not allowed".to_string());
                }
                let arguments = if partial_json.is_empty() {
                    initial_input
                } else {
                    partial_json
                };
                validate_tool_arguments(&arguments)?;
                self.function_call = Some(ParsedClaudeToolUse {
                    call_id,
                    name,
                    arguments,
                });
                Ok(())
            }
        }
    }

    fn message_delta(&mut self, value: &Value) -> Result<(), String> {
        let Some(delta) = value.get("delta") else {
            return Err("message delta missing".to_string());
        };
        match delta.get("stop_reason") {
            Some(Value::Str(reason)) => {
                self.stop_reason = Some(reason.clone());
                Ok(())
            }
            None | Some(Value::Null) => Ok(()),
            Some(_) => Err("stop reason has the wrong type".to_string()),
        }
    }

    fn append_text(&mut self, text: &str) -> Result<(), String> {
        if self.text.len().saturating_add(text.len()) > MAX_TURN_TEXT_BYTES {
            return Err("assistant text too large".to_string());
        }
        self.text.push_str(text);
        if !text.is_empty() {
            self.deltas.push(text.to_string());
        }
        Ok(())
    }

    fn finish(self) -> Result<ParsedClaudeStream, String> {
        if self.block.is_some() {
            return Err("truncated content block".to_string());
        }
        if !self.saw_message_stop {
            return Err("claude stream ended before message_stop".to_string());
        }
        match (self.stop_reason.as_deref(), self.function_call.is_some()) {
            (Some("tool_use"), true) => {}
            (Some("end_turn" | "stop_sequence"), false) => {}
            (Some("tool_use"), false) | (Some("end_turn" | "stop_sequence"), true) => {
                return Err("claude tool state is inconsistent".to_string())
            }
            (Some("max_tokens"), _) => {
                return Err("response incomplete: output limit".to_string())
            }
            (Some("refusal"), _) => return Err("response refused by safety policy".to_string()),
            (Some(_), _) => return Err("unsupported claude stop reason".to_string()),
            (None, _) => return Err("claude stop reason missing".to_string()),
        }
        Ok(ParsedClaudeStream {
            text: self.text,
            deltas: self.deltas,
            function_call: self.function_call,
        })
    }
}

/// Parse one complete, bounded Anthropic SSE body.
pub fn parse_claude_stream(bytes: &[u8]) -> Result<ParsedClaudeStream, String> {
    if bytes.len() > MAX_CLAUDE_BODY {
        return Err("response body too large".to_string());
    }
    let mut decoder = SseDecoder::default();
    let mut parser = ClaudeStreamParser::default();
    for event in decoder.push(bytes)? {
        parser.event(event)?;
    }
    for event in decoder.finish()? {
        parser.event(event)?;
    }
    parser.finish()
}

fn validate_call_id(call_id: &str) -> Result<(), String> {
    if call_id.is_empty()
        || call_id.len() > MAX_TOOL_CALL_ID
        || !call_id.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err("tool call id missing or invalid".to_string());
    }
    Ok(())
}

fn validate_function_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > MAX_FUNCTION_NAME_BYTES
        || name.bytes().any(|byte| byte == b'\r' || byte == b'\n' || byte == 0)
    {
        return Err("tool name missing or invalid".to_string());
    }
    Ok(())
}

fn validate_tool_arguments(arguments: &str) -> Result<(), String> {
    if arguments.len() > MAX_TOOL_JSON_BYTES {
        return Err("tool arguments too large".to_string());
    }
    match json::parse(arguments.as_bytes()) {
        Ok(Value::Obj(_)) => Ok(()),
        _ => Err("tool arguments are not a json object".to_string()),
    }
}

fn claude_error_category(error: Option<&Value>) -> String {
    let error_type = error
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    match error_type {
        "authentication_error" => "api error: authentication".to_string(),
        "permission_error" => "api error: permission".to_string(),
        "rate_limit_error" => "api error: rate limited".to_string(),
        "invalid_request_error" => "api error: invalid request".to_string(),
        "overloaded_error" | "api_error" => "api error: server".to_string(),
        _ => "api error".to_string(),
    }
}

fn http_status_error(status: u16, body: &[u8]) -> String {
    if body.len() <= MAX_CLAUDE_BODY {
        if let Ok(value) = json::parse(body) {
            if let Some(error) = value.get("error") {
                return claude_error_category(Some(error));
            }
        }
    }
    match status {
        401 | 403 => "http error: authentication".to_string(),
        429 => "http error: rate limited".to_string(),
        400 | 404 | 422 => "http error: invalid request".to_string(),
        500..=599 => "http error: server".to_string(),
        _ => format!("http {status}"),
    }
}

fn validate_turn_messages(messages: &[ChatMessage]) -> Result<(), String> {
    if messages.len() > MAX_MESSAGES {
        return Err("too many messages".to_string());
    }
    for message in messages {
        message
            .validate()
            .map_err(|_| "message empty or too large".to_string())?;
    }
    Ok(())
}

fn encode_messages(messages: &[ChatMessage]) -> Vec<Value> {
    messages
        .iter()
        .filter_map(|message| match message.role {
            ChatRole::System => None,
            ChatRole::Tool => Some(json::obj(vec![
                ("role", json::s("user")),
                (
                    "content",
                    json::s(format!("[tool result]\n{}", message.text)),
                ),
            ])),
            ChatRole::User | ChatRole::Assistant => Some(json::obj(vec![
                ("role", json::s(message.role.slug())),
                ("content", json::s(message.text.clone())),
            ])),
        })
        .collect()
}

fn build_request_body(
    config: &ClaudeApiConfig,
    system: &str,
    messages: Vec<Value>,
    native_tools: Option<&Value>,
    tools_enabled: bool,
) -> Value {
    let mut pairs = vec![
        ("model", json::s(config.model.clone())),
        ("max_tokens", Value::Int(config.max_tokens as i64)),
        ("stream", Value::Bool(true)),
    ];
    if !system.is_empty() {
        pairs.push(("system", json::s(system)));
    }
    if tools_enabled {
        if let Some(tools) = native_tools {
            pairs.push(("tools", tools.clone()));
        }
    }
    pairs.push(("messages", Value::Arr(messages)));
    json::obj(pairs)
}

fn assistant_tool_message(tool: &PendingTool) -> Result<Value, String> {
    let mut content = Vec::new();
    if !tool.text.is_empty() {
        content.push(json::obj(vec![
            ("type", json::s("text")),
            ("text", json::s(tool.text.clone())),
        ]));
    }
    let input = json::parse(tool.arguments.as_bytes())
        .map_err(|_| "tool arguments are not valid json".to_string())?;
    content.push(json::obj(vec![
        ("type", json::s("tool_use")),
        ("id", json::s(tool.call_id.clone())),
        ("name", json::s(tool.name.clone())),
        ("input", input),
    ]));
    Ok(json::obj(vec![
        ("role", json::s("assistant")),
        ("content", Value::Arr(content)),
    ]))
}

fn tool_result_message(call_id: &str, output: &str) -> Value {
    json::obj(vec![
        ("role", json::s("user")),
        (
            "content",
            Value::Arr(vec![json::obj(vec![
                ("type", json::s("tool_result")),
                ("tool_use_id", json::s(call_id)),
                ("content", json::s(output)),
            ])]),
        ),
    ])
}

fn endpoint_detail(url: &str) -> String {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    rest.split('/').next().unwrap_or("configured-endpoint").to_string()
}

#[derive(Clone)]
struct PendingTool {
    call_id: String,
    name: String,
    arguments: String,
    text: String,
}

enum WorkerOut {
    Ok(ParsedClaudeStream),
    Err(String),
}

struct Active {
    rx: Receiver<WorkerOut>,
    request_messages: Vec<Value>,
    restore_tool: Option<PendingTool>,
}

pub struct ClaudeApiChatProvider<T: ClaudeApiTransport = BlockingClaudeTransport> {
    config: ClaudeApiConfig,
    native_tools: Option<Value>,
    transport: Arc<T>,
    cancel: CancelToken,
    active: Option<Active>,
    messages: Vec<Value>,
    system: String,
    tools_enabled: bool,
    pending_tool: Option<PendingTool>,
}

impl<T: ClaudeApiTransport> ClaudeApiChatProvider<T> {
    pub fn with_transport(
        config: ClaudeApiConfig,
        transport: T,
        native_tools: Option<Value>,
    ) -> Self {
        ClaudeApiChatProvider {
            config,
            native_tools,
            transport: Arc::new(transport),
            cancel: CancelToken::new(),
            active: None,
            messages: Vec::new(),
            system: String::new(),
            tools_enabled: true,
            pending_tool: None,
        }
    }

    fn spawn_request(
        &mut self,
        body: Value,
        request_messages: Vec<Value>,
        restore_tool: Option<PendingTool>,
    ) -> Result<(), String> {
        if self.active.is_some() {
            return Err("a turn is already in flight".to_string());
        }
        let auth = self
            .config
            .auth
            .as_ref()
            .ok_or_else(|| self.config.unavailable_reason.clone())?
            .clone();
        let bytes = body.to_json().into_bytes();
        if bytes.len() > MAX_CLAUDE_BODY {
            return Err("request body too large".to_string());
        }
        let endpoint = self.config.endpoint.clone();
        let timeout = self.config.request_timeout;
        let cancel = self.cancel.clone();
        let transport = self.transport.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("content-chat-claude-api".into())
            .spawn(move || {
                let response =
                    transport.post_messages(&endpoint, &auth, &bytes, &cancel, timeout);
                if cancel.is_cancelled() {
                    return;
                }
                let result = match response {
                    Err(error) => {
                        let public = if error.contains(auth.secret()) {
                            "provider error".to_string()
                        } else {
                            sanitize_public_error(&error)
                        };
                        WorkerOut::Err(public)
                    }
                    Ok(raw) if raw.body.len() > MAX_CLAUDE_BODY => {
                        WorkerOut::Err("response body too large".to_string())
                    }
                    Ok(raw) if !(200..300).contains(&raw.status) => {
                        WorkerOut::Err(http_status_error(raw.status, &raw.body))
                    }
                    Ok(raw) => match parse_claude_stream(&raw.body) {
                        Ok(parsed) => WorkerOut::Ok(parsed),
                        Err(error) => WorkerOut::Err(sanitize_public_error(&error)),
                    },
                };
                if !cancel.is_cancelled() {
                    let _ = tx.send(result);
                }
            })
            .map_err(|_| "failed to start provider worker".to_string())?;
        self.active = Some(Active {
            rx,
            request_messages,
            restore_tool,
        });
        Ok(())
    }
}

impl ClaudeApiChatProvider<BlockingClaudeTransport> {
    pub fn new(
        config: ClaudeApiConfig,
        native_tools: Option<Value>,
    ) -> ClaudeApiChatProvider<BlockingClaudeTransport> {
        ClaudeApiChatProvider::with_transport(config, BlockingClaudeTransport, native_tools)
    }

    pub fn from_env(
        kind: ProviderKind,
        native_tools: Option<Value>,
    ) -> ClaudeApiChatProvider<BlockingClaudeTransport> {
        Self::new(ClaudeApiConfig::from_env(kind), native_tools)
    }
}

impl<T: ClaudeApiTransport> ChatProvider for ClaudeApiChatProvider<T> {
    fn kind(&self) -> ProviderKind {
        self.config.kind
    }

    fn availability(&mut self) -> ProviderAvailability {
        match self.config.auth {
            Some(_) => ProviderAvailability::Available {
                model: self.config.model.clone(),
                detail: endpoint_detail(&self.config.endpoint),
            },
            None => ProviderAvailability::Unavailable {
                reason: self.config.unavailable_reason.clone(),
            },
        }
    }

    fn begin_turn(&mut self, input: &TurnInput) -> Result<(), String> {
        if self.active.is_some() {
            return Err("a turn is already in flight".to_string());
        }
        if self.pending_tool.is_some() {
            return Err("unresolved function call".to_string());
        }
        if self.config.auth.is_none() {
            return Err(self.config.unavailable_reason.clone());
        }
        validate_turn_messages(&input.messages)?;
        let messages = encode_messages(&input.messages);
        if messages.is_empty() {
            return Err("no input to send".to_string());
        }
        self.system = input.system_with_dynamic();
        self.tools_enabled = input.tools_enabled;
        let body = build_request_body(
            &self.config,
            &self.system,
            messages.clone(),
            self.native_tools.as_ref(),
            self.tools_enabled,
        );
        self.spawn_request(body, messages, None)
    }

    fn continue_function(&mut self, call_id: &str, output: &str) -> Result<(), String> {
        if self.active.is_some() {
            return Err("a turn is already in flight".to_string());
        }
        validate_call_id(call_id)?;
        if output.len() > MAX_TOOL_JSON_BYTES {
            return Err("function output too large".to_string());
        }
        let tool = match self.pending_tool.as_ref() {
            None => return Err("no function call is awaiting output".to_string()),
            Some(tool) if tool.call_id != call_id => {
                return Err("mismatched function call id".to_string())
            }
            Some(tool) => tool.clone(),
        };
        let mut messages = self.messages.clone();
        messages.push(assistant_tool_message(&tool)?);
        messages.push(tool_result_message(call_id, output));
        let body = build_request_body(
            &self.config,
            &self.system,
            messages.clone(),
            self.native_tools.as_ref(),
            self.tools_enabled,
        );
        self.pending_tool = None;
        if let Err(error) = self.spawn_request(body, messages, Some(tool.clone())) {
            self.pending_tool = Some(tool);
            return Err(error);
        }
        Ok(())
    }

    fn poll(&mut self) -> Vec<ProviderEvent> {
        let received = match self.active.as_ref() {
            Some(active) => active.rx.try_recv(),
            None => return Vec::new(),
        };
        match received {
            Ok(WorkerOut::Ok(parsed)) => {
                let Some(active) = self.active.take() else {
                    return vec![ProviderEvent::Error("provider state error".to_string())];
                };
                self.messages = active.request_messages;
                let mut events = Vec::new();
                for delta in &parsed.deltas {
                    for chunk in split_delta_text(delta) {
                        events.push(ProviderEvent::Delta(chunk));
                    }
                }
                if let Some(tool) = parsed.function_call {
                    self.pending_tool = Some(PendingTool {
                        call_id: tool.call_id.clone(),
                        name: tool.name.clone(),
                        arguments: tool.arguments.clone(),
                        text: parsed.text,
                    });
                    events.push(ProviderEvent::FunctionCall {
                        call_id: tool.call_id,
                        name: tool.name,
                        arguments: tool.arguments,
                    });
                } else {
                    self.pending_tool = None;
                    events.push(ProviderEvent::Done { text: parsed.text });
                }
                events
            }
            Ok(WorkerOut::Err(error)) => {
                let restore = self.active.take().and_then(|active| active.restore_tool);
                if let Some(tool) = restore {
                    self.pending_tool = Some(tool);
                }
                vec![ProviderEvent::Error(sanitize_public_error(&error))]
            }
            Err(mpsc::TryRecvError::Empty) => Vec::new(),
            Err(mpsc::TryRecvError::Disconnected) => {
                let restore = self.active.take().and_then(|active| active.restore_tool);
                if let Some(tool) = restore {
                    self.pending_tool = Some(tool);
                }
                vec![ProviderEvent::Error("provider worker ended".to_string())]
            }
        }
    }

    fn cancel(&mut self) {
        self.cancel.cancel();
        let restore = self.active.take().and_then(|active| active.restore_tool);
        if let Some(tool) = restore {
            self.pending_tool = Some(tool);
        }
        self.cancel = CancelToken::new();
    }

    fn reset_conversation(&mut self) {
        self.cancel.cancel();
        self.active = None;
        self.messages.clear();
        self.system.clear();
        self.pending_tool = None;
        self.cancel = CancelToken::new();
    }
}

impl<T: ClaudeApiTransport> Drop for ClaudeApiChatProvider<T> {
    fn drop(&mut self) {
        if self.active.is_some() {
            self.cancel.cancel();
            self.active = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Instant;

    #[test]
    fn sse_chunk_reassembly_handles_crlf_utf8_and_multiline_data() {
        let wire = "event: first\r\ndata: {\"text\":\"hé\"}\r\n\r\nevent: second\ndata: one\ndata: two\n\n";
        let bytes = wire.as_bytes();
        let mut decoder = SseDecoder::default();
        let mut events = Vec::new();
        for chunk in bytes.chunks(3) {
            events.extend(decoder.push(chunk).expect("chunk"));
        }
        events.extend(decoder.finish().expect("finish"));
        assert_eq!(
            events,
            vec![
                SseEvent {
                    event_type: "first".into(),
                    data: "{\"text\":\"hé\"}".into(),
                },
                SseEvent {
                    event_type: "second".into(),
                    data: "one\ntwo".into(),
                },
            ]
        );
    }

    #[test]
    fn sse_parser_is_bounded() {
        let mut decoder = SseDecoder::default();
        let oversized = vec![b'x'; MAX_SSE_LINE_BYTES + 1];
        assert_eq!(decoder.push(&oversized).unwrap_err(), "sse line too large");
    }

    #[test]
    fn parses_text_and_tool_use_while_ignoring_unknown_events() {
        let body = br#"event: message_start
data: {"type":"message_start","message":{}}

event: future_event
data: this need not be json

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Checking. "}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"lookup","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"city\":"}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"Amsterdam\"}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":1}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":12}}

event: message_stop
data: {"type":"message_stop"}

"#;
        let parsed = parse_claude_stream(body).expect("stream");
        assert_eq!(parsed.text, "Checking. ");
        assert_eq!(parsed.deltas, vec!["Checking. "]);
        let tool = parsed.function_call.expect("tool use");
        assert_eq!(tool.call_id, "toolu_1");
        assert_eq!(tool.name, "lookup");
        assert_eq!(tool.arguments, r#"{"city":"Amsterdam"}"#);
    }

    #[test]
    fn truncated_and_upstream_error_streams_fail_closed() {
        let truncated = b"event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n";
        assert_eq!(
            parse_claude_stream(truncated).unwrap_err(),
            "claude stream ended before message_stop"
        );
        let error = b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"secret upstream detail\"}}\n\n";
        assert_eq!(
            parse_claude_stream(error).unwrap_err(),
            "api error: rate limited"
        );
    }

    struct UnusedTransport;

    impl ClaudeApiTransport for UnusedTransport {
        fn post_messages(
            &self,
            _url: &str,
            _auth: &ClaudeAuth,
            _body: &[u8],
            _cancel: &CancelToken,
            _timeout: Duration,
        ) -> Result<ClaudeRawHttp, String> {
            panic!("transport must not be called without credentials")
        }
    }

    #[test]
    fn availability_without_credentials_is_typed_and_begin_refuses() {
        let config = ClaudeApiConfig::without_credentials(
            ProviderKind::OpenAi,
            DEFAULT_CLAUDE_MODEL,
        );
        let mut provider = ClaudeApiChatProvider::with_transport(config, UnusedTransport, None);
        assert_eq!(
            provider.availability(),
            ProviderAvailability::Unavailable {
                reason: missing_credential_reason()
            }
        );
        let error = provider
            .begin_turn(&TurnInput::new(
                "system",
                vec![ChatMessage::new(ChatRole::User, "hello")],
            ))
            .unwrap_err();
        assert_eq!(error, missing_credential_reason());
    }

    struct SeenRequest {
        url: String,
        oauth: bool,
        secret_len: usize,
        body: Vec<u8>,
    }

    struct FakeTransport {
        seen: Arc<Mutex<Option<SeenRequest>>>,
        response: Mutex<Option<ClaudeRawHttp>>,
    }

    impl ClaudeApiTransport for FakeTransport {
        fn post_messages(
            &self,
            url: &str,
            auth: &ClaudeAuth,
            body: &[u8],
            _cancel: &CancelToken,
            _timeout: Duration,
        ) -> Result<ClaudeRawHttp, String> {
            *self.seen.lock().unwrap() = Some(SeenRequest {
                url: url.to_string(),
                oauth: auth.is_oauth(),
                secret_len: auth.secret().len(),
                body: body.to_vec(),
            });
            Ok(self.response.lock().unwrap().take().expect("one response"))
        }
    }

    #[test]
    fn transport_faked_round_trip_preserves_deltas_and_request_shape() {
        let response = br#"event: message_start
data: {"type":"message_start","message":{}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hel"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}

event: message_stop
data: {"type":"message_stop"}

"#;
        let seen = Arc::new(Mutex::new(None));
        let transport = FakeTransport {
            seen: seen.clone(),
            response: Mutex::new(Some(ClaudeRawHttp {
                status: 200,
                body: response.to_vec(),
            })),
        };
        let auth = ClaudeAuth::oauth_token("opaque-test-token").unwrap();
        let config = ClaudeApiConfig::new(ProviderKind::OpenAi, auth, DEFAULT_CLAUDE_MODEL);
        let tools = Value::Arr(vec![json::obj(vec![
            ("name", json::s("lookup")),
            (
                "input_schema",
                json::obj(vec![("type", json::s("object"))]),
            ),
        ])]);
        let mut provider =
            ClaudeApiChatProvider::with_transport(config, transport, Some(tools.clone()));
        provider
            .begin_turn(&TurnInput::new(
                "be concise",
                vec![ChatMessage::new(ChatRole::User, "hello")],
            ))
            .expect("begin");

        let deadline = Instant::now() + Duration::from_secs(2);
        let events = loop {
            let events = provider.poll();
            if !events.is_empty() {
                break events;
            }
            assert!(Instant::now() < deadline, "provider worker timed out");
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(
            events,
            vec![
                ProviderEvent::Delta("hel".into()),
                ProviderEvent::Delta("lo".into()),
                ProviderEvent::Done {
                    text: "hello".into()
                },
            ]
        );

        let seen = seen.lock().unwrap().take().expect("request captured");
        assert_eq!(seen.url, CLAUDE_MESSAGES_URL);
        assert!(seen.oauth);
        assert_eq!(seen.secret_len, "opaque-test-token".len());
        let body = json::parse(&seen.body).expect("request json");
        assert_eq!(body.get("model").and_then(Value::as_str), Some(DEFAULT_CLAUDE_MODEL));
        assert_eq!(body.get("stream").and_then(Value::as_bool), Some(true));
        assert_eq!(body.get("system").and_then(Value::as_str), Some("be concise"));
        assert_eq!(body.get("tools"), Some(&tools));
        let messages = body.get("messages").and_then(Value::as_arr).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].get("role").and_then(Value::as_str), Some("user"));
        assert_eq!(messages[0].get("content").and_then(Value::as_str), Some("hello"));
    }
}
