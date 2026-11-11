//! Anthropic Messages API chat provider.
//!
//! The real transport is the hub's dependency-free blocking HTTP client on
//! a detached worker. Anthropic's bounded SSE response is parsed without a
//! UI `Cx`; `poll` stays non-blocking and preserves the upstream text-delta
//! order. Production configuration pins `api.anthropic.com`; tests replace
//! only the transport.

use crate::chat_wire::{
    sanitize_public_error, split_delta_text, tool_arguments_too_large, ChatMessage, ChatRole,
    ProviderAvailability, ProviderKind, MAX_MESSAGES, MAX_TOOL_CALL_ID, MAX_TOOL_JSON_BYTES,
    MAX_TOOL_RESULT_BYTES, MAX_TURN_TEXT_BYTES,
};
use crate::providers::provider::{ChatProvider, ProviderEvent, TurnInput};
use makepad_network::blocking_http::{
    request_to_writer_no_redirect, CancelToken, Error as HttpError, Limits, Request,
};
use makepad_strict_json::{self as json, Value};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const CLAUDE_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
pub const ANTHROPIC_BETA: &str =
    "oauth-2025-04-20,interleaved-thinking-2025-05-14,output-128k-2025-02-19";
pub const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
pub const CLAUDE_CODE_OAUTH_TOKEN_ENV: &str = "CLAUDE_CODE_OAUTH_TOKEN";
pub const CLAUDE_MODEL_ENV: &str = "MAKEPAD_CONTENT_CHAT_CLAUDE_MODEL";
pub const DEFAULT_CLAUDE_MODEL: &str = "claude-sonnet-5-5";
pub const SERVER_SIDE_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
pub const DEFAULT_MAX_TOKENS: u32 = 4096;
/// Allow silent model thinking for two minutes; each arriving byte renews the wait.
pub const CLAUDE_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// The API's own request limit, for requests and streamed replies alike: a long tool session's
/// transcript and a reply of the full output allowance both stay well below it.
pub const MAX_CLAUDE_BODY: usize = 32 * 1024 * 1024;

const MAX_MODEL_BYTES: usize = 128;
const MAX_FUNCTION_NAME_BYTES: usize = 64;
// A provider may put the whole escaped argument object in one input delta.
// Preserve the existing SSE envelope allowance; buffers grow with actual data.
const MAX_SSE_LINE_BYTES: usize = 2 * MAX_TOOL_JSON_BYTES + 64 * 1024;
const MAX_SSE_EVENT_BYTES: usize = MAX_SSE_LINE_BYTES;

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
    idle_timeout: Duration,
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
            idle_timeout: CLAUDE_IDLE_TIMEOUT,
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
            idle_timeout: CLAUDE_IDLE_TIMEOUT,
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
            idle_timeout: CLAUDE_IDLE_TIMEOUT,
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


}

fn uses_server_fallback(model: &str) -> bool {
    matches!(model, "claude-fable-5-2" | "claude-opus-5-5" | "claude-sonnet-5-5")
}

/// The `anthropic-beta` header: the OAuth set for a token, the fallback
/// beta for the models that send `fallbacks`; empty sends no header.
fn anthropic_beta(config: &ClaudeApiConfig) -> String {
    let oauth = config.auth.as_ref().is_some_and(ClaudeAuth::is_oauth).then_some(ANTHROPIC_BETA);
    let fallback = uses_server_fallback(&config.model).then_some(SERVER_SIDE_FALLBACK_BETA);
    oauth.into_iter().chain(fallback).collect::<Vec<_>>().join(",")
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
        beta: &str,
        cancel: &CancelToken,
        idle_timeout: Duration,
        on_chunk: &mut dyn FnMut(&[u8]),
    ) -> Result<ClaudeRawHttp, String>;
}

pub struct ClaudeRawHttp {
    pub status: u16,
    pub body: Vec<u8>,
}

fn request_headers(auth: &ClaudeAuth, beta: &str) -> Vec<(&'static str, String)> {
    let mut headers = vec![("anthropic-version", ANTHROPIC_VERSION.to_string())];
    if auth.is_oauth() {
        headers.push(("Authorization", format!("Bearer {}", auth.secret())));
    } else {
        headers.push(("x-api-key", auth.secret().to_string()));
    }
    if !beta.is_empty() {
        headers.push(("anthropic-beta", beta.to_string()));
    }
    headers
}

pub struct BlockingClaudeTransport;

impl ClaudeApiTransport for BlockingClaudeTransport {
    fn post_messages(
        &self,
        url: &str,
        auth: &ClaudeAuth,
        body: &[u8],
        beta: &str,
        cancel: &CancelToken,
        idle_timeout: Duration,
        on_chunk: &mut dyn FnMut(&[u8]),
    ) -> Result<ClaudeRawHttp, String> {
        let mut request = Request::post(url);
        for (name, value) in request_headers(auth, beta) {
            request = request.header(name, &value).map_err(safe_http_err)?;
        }
        let request = request
            .json_body(body.to_vec())
            .map_err(safe_http_err)?
            .cancel_token(cancel.clone())
            .limits(Limits {
                max_body_bytes: MAX_CLAUDE_BODY,
                idle_timeout,
                ..Limits::default()
            });
        let mut writer = ClaudeBody { body: Vec::new(), on_chunk };
        match request_to_writer_no_redirect(request, &mut writer) {
            Ok(response) => Ok(ClaudeRawHttp {
                status: response.status,
                body: writer.body,
            }),
            Err(error) => Err(safe_http_err(error)),
        }
    }
}

/// Decode SSE as each HTTP body chunk arrives, including chunked transfer
/// encoding. Keep the bounded body for the existing HTTP error categories.
struct ClaudeBody<'a> {
    body: Vec<u8>,
    on_chunk: &'a mut dyn FnMut(&[u8]),
}

impl std::io::Write for ClaudeBody<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.body.extend_from_slice(bytes);
        (self.on_chunk)(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
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
    pub content: Vec<Value>,
}

#[derive(Clone, Debug)]
pub struct ParsedClaudeToolUse {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

enum BlockState {
    Text,
    Thinking,
    Tool {
        call_id: String,
        name: String,
        partial_json: String,
        initial_input: String,
        last_partial: Option<Instant>,
    },
    Ignored,
}

#[derive(Default)]
struct ClaudeStreamParser {
    text: String,
    deltas: Vec<String>,
    block: Option<BlockState>,
    current_content: Option<Value>,
    content: Vec<Value>,
    function_call: Option<ParsedClaudeToolUse>,
    stop_reason: Option<String>,
    saw_message_stop: bool,
    partial_events: Vec<ProviderEvent>,
}

impl ClaudeStreamParser {
    fn event(&mut self, event: SseEvent) -> Result<(), String> {
        self.event_at(event, Instant::now())
    }

    fn event_at(&mut self, event: SseEvent, now: Instant) -> Result<(), String> {
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
            "content_block_delta" => self.delta_block(&value, now),
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
        self.current_content = Some(block.clone());
        self.block = Some(match block.get("type").and_then(Value::as_str) {
            Some("thinking") => {
                if block.get("thinking").is_none() {
                    set_field(self.current_content.as_mut().unwrap(), "thinking", json::s(""));
                }
                BlockState::Thinking
            }
            Some("redacted_thinking") => BlockState::Ignored,
            Some("fallback") => {
                // Calls from the abandoned model must not reach the tool loop.
                self.function_call = None;
                BlockState::Ignored
            }
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
                    last_partial: None,
                }
            }
            Some(_) => {
                self.current_content = None;
                BlockState::Ignored
            },
            None => return Err("content block type missing".to_string()),
        });
        Ok(())
    }

    fn delta_block(&mut self, value: &Value, now: Instant) -> Result<(), String> {
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
                append_block_text(self.current_content.as_mut().unwrap(), "text", &text)?;
                self.append_text(&text)
            }
            Some(BlockState::Thinking)
                if matches!(delta_type, Some("thinking_delta" | "signature_delta")) =>
            {
                let field = if delta_type == Some("thinking_delta") { "thinking" } else { "signature" };
                let fragment = delta.get(field).and_then(Value::as_str)
                    .ok_or_else(|| "thinking delta missing".to_string())?;
                append_block_text(self.current_content.as_mut().unwrap(), field, fragment)
            }
            Some(BlockState::Tool { call_id, name, partial_json, last_partial, .. })
                if delta_type == Some("input_json_delta") =>
            {
                let fragment = delta
                    .get("partial_json")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "tool input delta missing".to_string())?;
                if partial_json.len().saturating_add(fragment.len()) > MAX_TOOL_JSON_BYTES {
                    return Err(tool_arguments_too_large(partial_json.len().saturating_add(fragment.len())));
                }
                partial_json.push_str(fragment);
                if last_partial.is_none_or(|last| now.duration_since(last) >= Duration::from_millis(50)) {
                    self.partial_events.push(ProviderEvent::FunctionCallPartial {
                        call_id: call_id.clone(), name: name.clone(), partial_json: partial_json.clone(),
                    });
                    *last_partial = Some(now);
                }
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
            BlockState::Text | BlockState::Thinking | BlockState::Ignored => {},
            BlockState::Tool {
                call_id,
                name,
                partial_json,
                initial_input,
                ..
            } => {
                if self.function_call.is_some() {
                    return Err("multiple tool uses are not allowed".to_string());
                }
                let arguments = if partial_json.is_empty() {
                    initial_input
                } else {
                    partial_json
                };
                // Always flush the final prefix, even inside the throttle window.
                self.partial_events.push(ProviderEvent::FunctionCallPartial {
                    call_id: call_id.clone(), name: name.clone(), partial_json: arguments.clone(),
                });
                validate_tool_arguments(&arguments)?;
                set_field(self.current_content.as_mut().unwrap(), "input",
                    json::parse(arguments.as_bytes()).map_err(|_| "invalid tool input".to_string())?);
                self.function_call = Some(ParsedClaudeToolUse {
                    call_id,
                    name,
                    arguments,
                });
            }
        }
        if let Some(content) = self.current_content.take() {
            self.content.push(content);
        }
        Ok(())
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

    fn finish(mut self) -> Result<ParsedClaudeStream, String> {
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
            (Some("refusal"), _) => {
                // A declined reply is shown, never sent back: its partial blocks leave the transcript.
                self.function_call = None;
                self.content.clear();
                let notice = if self.text.is_empty() {
                    "The model declined this request."
                } else {
                    "\n\nThe model declined this request."
                };
                self.append_text(notice)?;
            }
            (Some(_), _) => return Err("unsupported claude stop reason".to_string()),
            (None, _) => return Err("claude stop reason missing".to_string()),
        }
        if let Some(final_fallback) = self.content.iter().rposition(|block| block_type(block) == "fallback") {
            self.content = self.content.into_iter().enumerate()
                .filter(|(index, block)| *index >= final_fallback || !matches!(block_type(block), "thinking" | "redacted_thinking" | "tool_use"))
                .map(|(_, block)| block).collect();
        }
        Ok(ParsedClaudeStream {
            text: self.text,
            deltas: self.deltas,
            function_call: self.function_call,
            content: self.content,
        })
    }
}

fn block_type(block: &Value) -> &str {
    block.get("type").and_then(Value::as_str).unwrap_or("")
}

fn set_field(value: &mut Value, key: &str, replacement: Value) {
    if let Value::Obj(fields) = value {
        if let Some((_, field)) = fields.iter_mut().find(|(name, _)| name == key) {
            *field = replacement;
        } else {
            fields.push((key.to_string(), replacement));
        }
    }
}

fn append_block_text(block: &mut Value, field: &str, fragment: &str) -> Result<(), String> {
    if block.get(field).is_none() {
        set_field(block, field, json::s(""));
    }
    if let Value::Obj(fields) = block {
        if let Some((_, Value::Str(text))) = fields.iter_mut().find(|(name, _)| name == field) {
            text.push_str(fragment);
            return Ok(());
        }
    }
    Err("content block has the wrong shape".to_string())
}

fn strip_thinking(messages: &mut [Value]) {
    for message in messages {
        if let Value::Obj(fields) = message {
            if let Some((_, Value::Arr(blocks))) = fields.iter_mut().find(|(name, _)| name == "content") {
                blocks.retain(|block| !matches!(block_type(block), "thinking" | "redacted_thinking"));
            }
        }
    }
}

// Only the newest user message receives this turn's volatile context.
fn encode_turn_messages(messages: &[ChatMessage], dynamic_context: &str) -> Vec<Value> {
    let mut encoded = encode_messages(messages);
    if !dynamic_context.is_empty() {
        if let Some(index) = messages.iter().filter(|message| message.role != ChatRole::System)
            .enumerate().filter(|(_, message)| message.role == ChatRole::User).map(|(index, _)| index).last() {
            let text = encoded[index].get("content").cloned().unwrap();
            set_field(&mut encoded[index], "content", Value::Arr(vec![
                json::obj(vec![("type", json::s("text")), ("text", json::s(dynamic_context))]),
                json::obj(vec![("type", json::s("text")), ("text", text)]),
            ]));
        }
    }
    encoded
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
        return Err(tool_arguments_too_large(arguments.len()));
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
) -> Result<Value, String> {
    if messages.last().and_then(|message| message.get("role")).and_then(Value::as_str) == Some("assistant")
    {
        return Err("this model requires a user message after the assistant reply".into());
    }
    let mut pairs = vec![
        ("model", json::s(config.model.clone())),
        ("max_tokens", Value::Int(config.max_tokens as i64)),
        ("stream", Value::Bool(true)),
    ];
    if uses_server_fallback(&config.model) {
        pairs.push(("fallbacks", json::s("default")));
    }
    // Fable rejects any thinking field, including an explicit disabled value.
    if config.model == "claude-fable-5-2" {
        pairs.push(("output_config", json::obj(vec![("effort", json::s("high"))])));
    }
    if !system.is_empty() {
        pairs.push(("system", json::s(system)));
    }
    if tools_enabled {
        if let Some(tools) = native_tools {
            pairs.push(("tools", tools.clone()));
            pairs.push(("tool_choice", json::obj(vec![
                ("type", json::s("auto")), ("disable_parallel_tool_use", Value::Bool(true)),
            ])));
        }
    }
    pairs.push(("messages", Value::Arr(messages)));
    Ok(json::obj(pairs))
}

fn tool_result_message(call_id: &str, output: &str, is_error: bool) -> Value {
    json::obj(vec![
        ("role", json::s("user")),
        (
            "content",
            Value::Arr(vec![json::obj(vec![
                ("type", json::s("tool_result")),
                ("tool_use_id", json::s(call_id)),
                ("content", json::s(output)),
                ("is_error", Value::Bool(is_error)),
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
}

enum WorkerOut {
    Partial(ProviderEvent),
    Ok(ParsedClaudeStream),
    Err(String),
}

struct Active {
    rx: Receiver<WorkerOut>,
}

pub struct ClaudeApiChatProvider<T: ClaudeApiTransport = BlockingClaudeTransport> {
    config: ClaudeApiConfig,
    native_tools: Option<Value>,
    transport: Arc<T>,
    cancel: CancelToken,
    active: Option<Active>,
    messages: Vec<Value>,
    input_messages: Option<Vec<ChatMessage>>,
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
            input_messages: None,
            system: String::new(),
            tools_enabled: true,
            pending_tool: None,
        }
    }

    fn spawn_request(
        &mut self,
        body: Value,
        request_messages: Vec<Value>,
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
        let beta = anthropic_beta(&self.config);
        let bytes = body.to_json().into_bytes();
        if bytes.len() > MAX_CLAUDE_BODY {
            return Err("request body too large".to_string());
        }
        let endpoint = self.config.endpoint.clone();
        let idle_timeout = self.config.idle_timeout;
        let cancel = self.cancel.clone();
        let transport = self.transport.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("content-chat-claude-api".into())
            .spawn(move || {
                let mut decoder = SseDecoder::default();
                let mut parser = ClaudeStreamParser::default();
                let mut parse_error = None;
                let response = transport.post_messages(&endpoint, &auth, &bytes, &beta, &cancel, idle_timeout, &mut |chunk| {
                    if parse_error.is_some() || cancel.is_cancelled() { return; }
                    let result = decoder.push(chunk).and_then(|events| {
                        for event in events {
                            parser.event(event)?;
                            for partial in parser.partial_events.drain(..) {
                                let _ = tx.send(WorkerOut::Partial(partial));
                            }
                        }
                        Ok(())
                    });
                    parse_error = result.err();
                });
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
                    Ok(_) => match (|| {
                        if let Some(error) = parse_error { return Err(error); }
                        for event in decoder.finish()? {
                            parser.event(event)?;
                            for partial in parser.partial_events.drain(..) {
                                let _ = tx.send(WorkerOut::Partial(partial));
                            }
                        }
                        parser.finish()
                    })() {
                        Ok(parsed) => WorkerOut::Ok(parsed),
                        Err(error) => WorkerOut::Err(sanitize_public_error(&error)),
                    },
                };
                if !cancel.is_cancelled() {
                    let _ = tx.send(result);
                }
            })
            .map_err(|_| "failed to start provider worker".to_string())?;
        // Once dispatched, this exact prefix belongs to the conversation,
        // including when the request is later cancelled or fails.
        self.messages = request_messages;
        self.active = Some(Active { rx });
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
        if self.config.auth.is_none() {
            return Err(self.config.unavailable_reason.clone());
        }
        validate_turn_messages(&input.messages)?;
        let extends = self.input_messages.as_ref()
            .is_some_and(|previous| input.messages.starts_with(previous));
        let mut messages;
        if extends {
            messages = self.messages.clone();
            if self.system != input.system || self.tools_enabled != input.tools_enabled {
                strip_thinking(&mut messages);
            }
            if let Some(tool) = &self.pending_tool {
                messages.push(tool_result_message(&tool.call_id, "cancelled", true));
            }
            let mut new_messages = &input.messages[self.input_messages.as_ref().unwrap().len()..];
            if new_messages.first().is_some_and(|message| message.role == ChatRole::Assistant) {
                new_messages = &new_messages[1..];
            }
            let new_messages: Vec<_> = new_messages.iter()
                .filter(|message| matches!(message.role, ChatRole::User | ChatRole::Tool))
                .cloned().collect();
            messages.extend(encode_turn_messages(&new_messages, &input.dynamic_context));
        } else {
            if self.input_messages.is_some() {
                eprintln!("[chat] input history changed; restarting the transcript without thinking blocks");
            }
            messages = encode_turn_messages(&input.messages, &input.dynamic_context);
        }
        if messages.is_empty() {
            return Err("no input to send".to_string());
        }
        let body = build_request_body(
            &self.config, &input.system, messages.clone(),
            self.native_tools.as_ref(), input.tools_enabled,
        )?;
        self.spawn_request(body, messages)?;
        self.system = input.system.clone();
        self.tools_enabled = input.tools_enabled;
        self.input_messages = Some(input.messages.clone());
        self.pending_tool = None;
        Ok(())
    }

    fn continue_function(&mut self, call_id: &str, output: &str) -> Result<(), String> {
        if self.active.is_some() {
            return Err("a turn is already in flight".to_string());
        }
        validate_call_id(call_id)?;
        if output.len() > MAX_TOOL_RESULT_BYTES {
            return Err("function output too large".to_string());
        }
        match self.pending_tool.as_ref() {
            None => return Err("no function call is awaiting output".to_string()),
            Some(tool) if tool.call_id != call_id => {
                return Err("mismatched function call id".to_string())
            }
            Some(_) => {},
        };
        let mut messages = self.messages.clone();
        messages.push(tool_result_message(call_id, output, false));
        let body = build_request_body(
            &self.config,
            &self.system,
            messages.clone(),
            self.native_tools.as_ref(),
            self.tools_enabled,
        )?;
        self.spawn_request(body, messages)?;
        self.pending_tool = None;
        Ok(())
    }

    fn poll(&mut self) -> Vec<ProviderEvent> {
        let received = match self.active.as_ref() {
            Some(active) => active.rx.try_recv(),
            None => return Vec::new(),
        };
        match received {
            Ok(WorkerOut::Partial(event)) => vec![event],
            Ok(WorkerOut::Ok(parsed)) => {
                let Some(_) = self.active.take() else {
                    return vec![ProviderEvent::Error("provider state error".to_string())];
                };
                if !parsed.content.is_empty() {
                    self.messages.push(json::obj(vec![
                        ("role", json::s("assistant")),
                        ("content", Value::Arr(parsed.content)),
                    ]));
                }
                let mut events = Vec::new();
                for delta in &parsed.deltas {
                    for chunk in split_delta_text(delta) {
                        events.push(ProviderEvent::Delta(chunk));
                    }
                }
                if let Some(tool) = parsed.function_call {
                    self.pending_tool = Some(PendingTool {
                        call_id: tool.call_id.clone(),
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
                self.active = None;
                vec![ProviderEvent::Error(sanitize_public_error(&error))]
            }
            Err(mpsc::TryRecvError::Empty) => Vec::new(),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.active = None;
                vec![ProviderEvent::Error("provider worker ended".to_string())]
            }
        }
    }

    fn cancel(&mut self) {
        self.cancel.cancel();
        self.active = None;
        self.cancel = CancelToken::new();
    }

    fn reset_conversation(&mut self) {
        self.cancel.cancel();
        self.active = None;
        self.messages.clear();
        self.input_messages = None;
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
    use std::collections::VecDeque;
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
        assert_eq!(parsed.content, vec![text_block("Checking. "), json::obj(vec![
            ("type", json::s("tool_use")), ("id", json::s("toolu_1")), ("name", json::s("lookup")),
            ("input", json::obj(vec![("city", json::s("Amsterdam"))])),
        ])]);
        // The accumulated argument object may exceed 16 KiB, and one delta
        // may contain it all. Exercise both shapes through the SSE decoder.
        let arguments = json::obj(vec![("source", json::s("\"\n".repeat(50 * 1024)))]).to_json();
        for fragment_bytes in [16 * 1024, arguments.len()] {
            let mut stream = String::from("event: message_start\ndata: {\"type\":\"message_start\",\"message\":{}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_large\",\"name\":\"replace_source\",\"input\":{}}}\n\n");
            for fragment in arguments.as_bytes().chunks(fragment_bytes) {
                let delta = json::obj(vec![("type", json::s("content_block_delta")), ("index", Value::Int(0)),
                    ("delta", json::obj(vec![("type", json::s("input_json_delta")),
                        ("partial_json", json::s(std::str::from_utf8(fragment).unwrap()))]))]);
                stream.push_str(&format!("event: content_block_delta\ndata: {}\n\n", delta.to_json()));
            }
            stream.push_str("event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
            let parsed = parse_claude_stream(stream.as_bytes()).unwrap();
            assert_eq!(parsed.function_call.unwrap().arguments, arguments);
        }
        let fallback = json::parse(br#"{"type":"fallback","from":{"model":"first"},"to":{"model":"second"}}"#).unwrap();
        let redacted = json::parse(br#"{"type":"redacted_thinking","data":"opaque+/="}"#).unwrap();
        let blocks = vec![thinking("old"), redacted.clone(), text_block("before"), tool_block("old_call"),
            fallback.clone(), thinking("also_old"), redacted.clone(), text_block("between"), tool_block("middle_call"),
            fallback.clone(), thinking("kept"), redacted.clone(), text_block("after"), tool_block("new_call")];
        let parsed = parse_claude_stream(&stream_blocks(&blocks, "tool_use").body).unwrap();
        assert_eq!(parsed.content, vec![text_block("before"), fallback.clone(), text_block("between"), fallback,
            thinking("kept"), redacted, text_block("after"), tool_block("new_call")]);
        assert_eq!(parsed.function_call.unwrap().call_id, "new_call");

        let wire = br#"event: content_block_start
data: {"content_block":{"type":"thinking","thinking":"initial ","signature":"s"}}

event: content_block_delta
data: {"delta":{"type":"thinking_delta","thinking":"thought\n"}}

event: content_block_delta
data: {"delta":{"type":"signature_delta","signature":"+/="}}

event: content_block_stop
data: {}

event: message_delta
data: {"delta":{"stop_reason":"end_turn"}}

event: message_stop
data: {}

"#;
        let parsed = parse_claude_stream(wire).unwrap();
        assert_eq!(parsed.content, vec![json::obj(vec![("type", json::s("thinking")),
            ("thinking", json::s("initial thought\n")), ("signature", json::s("s+/="))])]);
        assert!(parsed.text.is_empty());
        assert!(parsed.deltas.is_empty());
        let default_thinking = json::obj(vec![("type", json::s("thinking")), ("signature", json::s("opaque"))]);
        let parsed = parse_claude_stream(&stream_blocks(&[default_thinking], "end_turn").body).unwrap();
        assert_eq!(parsed.content[0].get("thinking").and_then(Value::as_str), Some(""));
        assert_eq!(parsed.content[0].get("signature").and_then(Value::as_str), Some("opaque"));
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
            _beta: &str,
            _cancel: &CancelToken,
            _timeout: Duration,
            _on_chunk: &mut dyn FnMut(&[u8]),
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
        beta: String,
    }

    struct FakeTransport {
        seen: Arc<Mutex<Vec<SeenRequest>>>,
        responses: Mutex<VecDeque<ClaudeRawHttp>>,
    }

    impl ClaudeApiTransport for FakeTransport {
        fn post_messages(
            &self,
            url: &str,
            auth: &ClaudeAuth,
            body: &[u8],
            beta: &str,
            _cancel: &CancelToken,
            _timeout: Duration,
            on_chunk: &mut dyn FnMut(&[u8]),
        ) -> Result<ClaudeRawHttp, String> {
            self.seen.lock().unwrap().push(SeenRequest {
                url: url.to_string(),
                oauth: auth.is_oauth(),
                secret_len: auth.secret().len(),
                body: body.to_vec(),
                beta: beta.to_string(),
            });
            let response = self.responses.lock().unwrap().pop_front().expect("queued response");
            for chunk in response.body.chunks(17) { on_chunk(chunk); }
            Ok(response)
        }
    }

    fn automatic_tool_choice() -> Value {
        json::obj(vec![("type", json::s("auto")), ("disable_parallel_tool_use", Value::Bool(true))])
    }

    fn thinking(signature: &str) -> Value {
        json::obj(vec![("type", json::s("thinking")), ("thinking", json::s("")), ("signature", json::s(signature))])
    }

    fn text_block(text: &str) -> Value {
        json::obj(vec![("type", json::s("text")), ("text", json::s(text))])
    }

    fn tool_block(id: &str) -> Value {
        json::obj(vec![("type", json::s("tool_use")), ("id", json::s(id)), ("name", json::s("lookup")), ("input", json::obj(vec![]))])
    }

    fn stream_blocks(blocks: &[Value], stop: &str) -> ClaudeRawHttp {
        let mut body = String::new();
        for block in blocks {
            body.push_str(&format!("event: content_block_start\ndata: {}\n\n", json::obj(vec![("content_block", block.clone())]).to_json()));
            body.push_str("event: content_block_stop\ndata: {}\n\n");
        }
        body.push_str(&format!("event: message_delta\ndata: {{\"delta\":{{\"stop_reason\":\"{stop}\"}}}}\n\nevent: message_stop\ndata: {{}}\n\n"));
        ClaudeRawHttp { status: 200, body: body.into_bytes() }
    }

    fn finish_response(provider: &mut ClaudeApiChatProvider<FakeTransport>) -> Vec<ProviderEvent> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut events = Vec::new();
        while provider.active.is_some() {
            events.extend(provider.poll());
            assert!(Instant::now() < deadline, "provider worker timed out");
            std::thread::yield_now();
        }
        assert!(!events.iter().any(|event| matches!(event, ProviderEvent::Error(_))), "{events:?}");
        events
    }

    fn assert_message_prefix(previous: &Value, next: &Value) {
        let previous = previous.get("messages").unwrap().to_json();
        let next = next.get("messages").unwrap().to_json();
        // The closing array delimiter moves after the appended messages.
        assert!(next.starts_with(previous.strip_suffix(']').unwrap()), "request history changed");
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
        for model in ["claude-fable-5-2", "claude-opus-5-5", DEFAULT_CLAUDE_MODEL] {
            for (oauth, stop_reason, reply) in [(true, "end_turn", "hello"), (false, "refusal", "hello"), (true, "refusal", "")] {
                let response = std::str::from_utf8(response).unwrap().replace("end_turn", stop_reason);
                let response = if reply.is_empty() { response.replace(r#""text":"hel""#, r#""text":"""#).replace(r#""text":"lo""#, r#""text":"""#) } else { response };
                let seen = Arc::new(Mutex::new(Vec::new()));
                let transport = FakeTransport {
                    seen: seen.clone(),
                    responses: Mutex::new(VecDeque::from([ClaudeRawHttp {
                        status: 200,
                        body: response.into_bytes(),
                    }])),
                };
                let auth = if oauth { ClaudeAuth::oauth_token("opaque-test-token") } else { ClaudeAuth::api_key("opaque-test-token") }.unwrap();
                let config = ClaudeApiConfig::new(ProviderKind::OpenAi, auth, model);
                let headers = request_headers(config.auth.as_ref().unwrap(), &anthropic_beta(&config));
                let header = |name| headers.iter().find(|(key, _)| *key == name).map(|(_, value)| value.as_str());
                assert_eq!(header("anthropic-version"), Some(ANTHROPIC_VERSION));
                if oauth {
                    assert_eq!(header("Authorization"), Some("Bearer opaque-test-token"));
                    assert!(header("x-api-key").is_none());
                } else {
                    assert_eq!(header("x-api-key"), Some("opaque-test-token"));
                    assert!(header("Authorization").is_none());
                }
                assert_eq!(header("anthropic-beta").unwrap().contains("oauth-2025-04-20"), oauth);
                assert!(header("anthropic-beta").unwrap().contains(SERVER_SIDE_FALLBACK_BETA));
                let prefill = encode_messages(&[ChatMessage::new(ChatRole::Assistant, "unfinished")]);
                assert!(build_request_body(&config, "", prefill, None, true).is_err(), "{model}: no assistant prefill");
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
                let mut expected = Vec::new();
                let mut full = reply.to_string();
                if !reply.is_empty() {
                    expected.extend([ProviderEvent::Delta("hel".into()), ProviderEvent::Delta("lo".into())]);
                }
                if stop_reason == "refusal" {
                    let notice = if reply.is_empty() { "The model declined this request." } else { "\n\nThe model declined this request." };
                    full.push_str(notice);
                    expected.push(ProviderEvent::Delta(notice.into()));
                }
                expected.push(ProviderEvent::Done { text: full });
                assert_eq!(events, expected, "{model}: {stop_reason}");
                assert_eq!(provider.messages.len(), if stop_reason == "refusal" { 1 } else { 2 }, "{model}: a declined reply stays out of the transcript");

                let seen = seen.lock().unwrap().remove(0);
                assert_eq!(seen.url, CLAUDE_MESSAGES_URL);
                assert_eq!(seen.oauth, oauth);
                let beta = if oauth { format!("{ANTHROPIC_BETA},{SERVER_SIDE_FALLBACK_BETA}") } else { SERVER_SIDE_FALLBACK_BETA.to_string() };
                assert_eq!(seen.beta, beta, "{model}: oauth {oauth}");
                assert_eq!(seen.secret_len, "opaque-test-token".len());
                let body = json::parse(&seen.body).expect("request json");
                assert_eq!(body.get("model").and_then(Value::as_str), Some(model));
                assert_eq!(body.get("fallbacks").and_then(Value::as_str), Some("default"));
                assert_eq!(body.get("tool_choice"), Some(&automatic_tool_choice()));
                if model == "claude-fable-5-2" {
                    assert!(body.get("thinking").is_none());
                    assert_eq!(body.get("output_config").and_then(|v| v.get("effort")).and_then(Value::as_str), Some("high"));
                }
                assert_eq!(body.get("stream").and_then(Value::as_bool), Some(true));
                assert_eq!(body.get("system").and_then(Value::as_str), Some("be concise"));
                assert_eq!(body.get("tools"), Some(&tools));
                let messages = body.get("messages").and_then(Value::as_arr).unwrap();
                assert_eq!(messages.len(), 1);
                assert_eq!(messages[0].get("role").and_then(Value::as_str), Some("user"));
                assert_eq!(messages[0].get("content").and_then(Value::as_str), Some("hello"));
            }
        }

        // One conversation: two tool continuations, a final reply, then a new turn.
        // Also run the same prefix with cancellation while the second call is pending.
        for cancel_second_tool in [false, true] {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let mut responses = VecDeque::from([
                stream_blocks(&[thinking("s1"), tool_block("A")], "tool_use"),
                stream_blocks(&[thinking("s2"), tool_block("B")], "tool_use"),
                stream_blocks(&[thinking("s3"), text_block("first reply")], "end_turn"),
                stream_blocks(&[thinking("s4"), text_block("second reply")], "end_turn"),
            ]);
            // More replies exercise re-freezing, an edited caller history, and reset.
            for _ in 0..4 {
                responses.push_back(stream_blocks(&[thinking("later"), text_block("reply")], "end_turn"));
            }
            let transport = FakeTransport { seen: seen.clone(), responses: Mutex::new(responses) };
            let tools = json::parse(br#"[{"name":"lookup","input_schema":{"type":"object"}}]"#).unwrap();
            let config = ClaudeApiConfig::new(ProviderKind::OpenAi, ClaudeAuth::oauth_token("test-token").unwrap(), DEFAULT_CLAUDE_MODEL);
            let mut provider = ClaudeApiChatProvider::with_transport(config, transport, Some(tools.clone()));
            let mut input = TurnInput::new("fixed system", vec![ChatMessage::new(ChatRole::User, "first question")]);
            input.dynamic_context = "first context".into();
            provider.begin_turn(&input).unwrap();
            finish_response(&mut provider);
            provider.continue_function("A", "result A").unwrap();
            finish_response(&mut provider);
            if cancel_second_tool {
                provider.cancel();
            } else {
                provider.continue_function("B", "result B").unwrap();
                finish_response(&mut provider);
                input.messages.push(ChatMessage::new(ChatRole::Assistant, "first reply"));
            }
            input.messages.push(ChatMessage::new(ChatRole::User, "second question"));
            input.dynamic_context = "second context".into();
            provider.begin_turn(&input).unwrap();
            finish_response(&mut provider);
            let requests: Vec<_> = seen.lock().unwrap().iter().map(|request| json::parse(&request.body).unwrap()).collect();
            assert_eq!(requests.len(), if cancel_second_tool { 3 } else { 4 });
            for request in &requests {
                assert_eq!(request.get("system").unwrap().to_json(), requests[0].get("system").unwrap().to_json());
                assert_eq!(request.get("system").and_then(Value::as_str), Some("fixed system"));
                assert_eq!(request.get("tools").unwrap().to_json(), tools.to_json());
                assert_eq!(request.get("tool_choice"), Some(&automatic_tool_choice()));
            }
            for pair in requests.windows(2) { assert_message_prefix(&pair[0], &pair[1]); }
            let messages = requests[1].get("messages").unwrap().as_arr().unwrap();
            assert_eq!(messages[1].get("content"), Some(&Value::Arr(vec![thinking("s1"), tool_block("A")])));
            assert_eq!(messages[2], tool_result_message("A", "result A", false));
            assert_eq!(messages[0].get("content"), Some(&Value::Arr(vec![text_block("first context"), text_block("first question")])));
            let messages = requests.last().unwrap().get("messages").unwrap().as_arr().unwrap();
            let previous = &messages[messages.len() - 2];
            if cancel_second_tool {
                assert_eq!(*previous, tool_result_message("B", "cancelled", true));
                assert_eq!(messages[messages.len() - 3].get("content"), Some(&Value::Arr(vec![thinking("s2"), tool_block("B")])));
            } else {
                assert_eq!(previous.get("content"), Some(&Value::Arr(vec![thinking("s3"), text_block("first reply")])));
            }
            assert_eq!(messages.last().unwrap().get("content"), Some(&Value::Arr(vec![text_block("second context"), text_block("second question")])));

            let before = provider.messages.clone();
            input.system = "changed system".into();
            input.messages.push(ChatMessage::new(ChatRole::Assistant, "reply"));
            input.messages.push(ChatMessage::new(ChatRole::User, "third question"));
            provider.begin_turn(&input).unwrap();
            finish_response(&mut provider);
            let mut stripped = before;
            strip_thinking(&mut stripped);
            assert_eq!(&provider.messages[..stripped.len()], stripped.as_slice());
            input.tools_enabled = false;
            input.messages.push(ChatMessage::new(ChatRole::Assistant, "reply"));
            input.messages.push(ChatMessage::new(ChatRole::User, "fourth question"));
            provider.begin_turn(&input).unwrap();
            finish_response(&mut provider);
            let request = json::parse(&seen.lock().unwrap().last().unwrap().body).unwrap();
            assert!(request.get("tools").is_none());
            assert!(request.get("tool_choice").is_none());
            assert!(!request.get("messages").unwrap().to_json().contains("\"thinking\""));

            input.messages = vec![ChatMessage::new(ChatRole::User, "edited history")];
            provider.begin_turn(&input).unwrap();
            finish_response(&mut provider);
            assert_eq!(provider.messages.len(), 2);
            provider.reset_conversation();
            input.dynamic_context = "x".repeat(512 * 1024);
            provider.begin_turn(&input).unwrap();
            finish_response(&mut provider);
            assert_eq!(provider.messages.len(), 2);
            assert!(seen.lock().unwrap().last().unwrap().body.len() > 512 * 1024,
                "a long session's request goes out");
        }
    }
}
