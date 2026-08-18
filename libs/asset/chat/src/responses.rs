//! Common OpenAI-compatible Responses API provider.
//!
//! Non-streaming `POST {endpoint}` on a detached worker thread. `poll` is
//! non-blocking. One active request; `cancel` flips the token, idles
//! immediately, ignores late events, and never joins.
//!
//! Request fields (every hop): `model`, `instructions`, `input`, native
//! `tools`, `tool_choice: auto`, `parallel_tool_calls: false`,
//! `max_output_tokens`. New user turns send `previous_response_id` plus the
//! unsent message tail. Tool continuation sends `previous_response_id` plus
//! one `function_call_output` item. Production constructors pin the provider
//! origin; tests inject a transport, not a public endpoint override.

use crate::provider::{ChatProvider, ProviderEvent, TurnInput};
use crate::tools;
use crate::wire::{
    sanitize_public_error, split_delta_text, ChatMessage, ProviderAvailability, ProviderKind,
    MAX_MESSAGES, MAX_TOOL_CALL_ID, MAX_TOOL_JSON_BYTES, MAX_TURN_TEXT_BYTES,
};
use makepad_asset_client::json::{self, Value};
use makepad_network::blocking_http::{
    post_json, CancelToken, Error as HttpError, Limits, Request,
};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 4096;
pub const MAX_RESPONSES_BODY: usize = 256 * 1024;
/// OpenAI non-reasoning default.
pub const DEFAULT_OPENAI_TIMEOUT: Duration = Duration::from_secs(60);
/// Grok reasoning can take minutes; still hard-capped.
pub const DEFAULT_GROK_TIMEOUT: Duration = Duration::from_secs(180);
pub const MAX_GROK_TIMEOUT: Duration = Duration::from_secs(600);
pub const GROK_TIMEOUT_ENV: &str = "MAKEPAD_CONTENT_CHAT_GROK_TIMEOUT_SECS";
const MAX_RESPONSE_ID: usize = 128;
const MAX_CALL_ID: usize = MAX_TOOL_CALL_ID;
const MAX_FN_NAME: usize = 64;
const SAFETY_REFUSAL_TEXT: &str = "The request was refused by the model's safety policy.";

/// Opaque API key. Does not implement `Debug` or `Display`.
#[derive(Clone)]
pub struct ApiKey(String);

impl ApiKey {
    pub fn new(value: impl Into<String>) -> Result<ApiKey, String> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 512
            || value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
        {
            return Err("api key is empty or invalid".to_string());
        }
        Ok(ApiKey(value))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

/// Provider configuration. Does not implement `Debug`.
pub struct ResponsesConfig {
    kind: ProviderKind,
    api_key: Option<ApiKey>,
    model: String,
    endpoint: String,
    max_output_tokens: u32,
    request_timeout: Duration,
    missing_key_reason: String,
}

impl ResponsesConfig {
    /// Production OpenAI config. Origin is always `api.openai.com`.
    pub fn openai(api_key: ApiKey, model: impl Into<String>) -> Self {
        ResponsesConfig {
            kind: ProviderKind::OpenAi,
            api_key: Some(api_key),
            model: bound_model(model.into(), crate::openai::DEFAULT_OPENAI_MODEL),
            endpoint: crate::openai::OPENAI_RESPONSES_URL.to_string(),
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            request_timeout: DEFAULT_OPENAI_TIMEOUT,
            missing_key_reason: format!("{} is not set", crate::openai::OPENAI_API_KEY_ENV),
        }
    }

    /// Production Grok config. Origin is always `api.x.ai`.
    pub fn grok(api_key: ApiKey, model: impl Into<String>) -> Self {
        ResponsesConfig {
            kind: ProviderKind::Grok,
            api_key: Some(api_key),
            model: bound_model(model.into(), crate::grok::DEFAULT_GROK_MODEL),
            endpoint: crate::grok::GROK_RESPONSES_URL.to_string(),
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            request_timeout: grok_timeout_from_env(),
            missing_key_reason: format!("{} is not set", crate::grok::GROK_API_KEY_ENV),
        }
    }

    pub fn openai_from_env() -> Self {
        let model = std::env::var(crate::openai::OPENAI_MODEL_ENV)
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| crate::openai::DEFAULT_OPENAI_MODEL.to_string());
        let api_key = std::env::var(crate::openai::OPENAI_API_KEY_ENV)
            .ok()
            .and_then(|s| ApiKey::new(s).ok());
        ResponsesConfig {
            kind: ProviderKind::OpenAi,
            api_key,
            model: bound_model(model, crate::openai::DEFAULT_OPENAI_MODEL),
            endpoint: crate::openai::OPENAI_RESPONSES_URL.to_string(),
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            request_timeout: DEFAULT_OPENAI_TIMEOUT,
            missing_key_reason: format!("{} is not set", crate::openai::OPENAI_API_KEY_ENV),
        }
    }

    pub fn grok_from_env() -> Self {
        let model = std::env::var(crate::grok::GROK_MODEL_ENV)
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| crate::grok::DEFAULT_GROK_MODEL.to_string());
        let api_key = std::env::var(crate::grok::GROK_API_KEY_ENV)
            .ok()
            .and_then(|s| ApiKey::new(s).ok());
        ResponsesConfig {
            kind: ProviderKind::Grok,
            api_key,
            model: bound_model(model, crate::grok::DEFAULT_GROK_MODEL),
            endpoint: crate::grok::GROK_RESPONSES_URL.to_string(),
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            request_timeout: grok_timeout_from_env(),
            missing_key_reason: format!("{} is not set", crate::grok::GROK_API_KEY_ENV),
        }
    }

    pub fn kind(&self) -> ProviderKind {
        self.kind
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn request_timeout(&self) -> Duration {
        self.request_timeout
    }

    pub fn with_max_output_tokens(mut self, tokens: u32) -> Self {
        self.max_output_tokens = tokens.max(1).min(32_768);
        self
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        let max = if self.kind == ProviderKind::Grok {
            MAX_GROK_TIMEOUT
        } else {
            Duration::from_secs(120)
        };
        self.request_timeout = timeout.max(Duration::from_secs(5)).min(max);
        self
    }
}

fn grok_timeout_from_env() -> Duration {
    let secs = std::env::var(GROK_TIMEOUT_ENV)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_GROK_TIMEOUT.as_secs());
    Duration::from_secs(secs.clamp(30, MAX_GROK_TIMEOUT.as_secs()))
}

fn bound_model(model: String, fallback: &str) -> String {
    if model.is_empty() || model.len() > 128 || model.bytes().any(|b| b == b'\r' || b == b'\n') {
        fallback.to_string()
    } else {
        model
    }
}

/// Fail-closed Responses body parser. Errors never include the raw body
/// or upstream `error.message`. Success requires top-level `status` exactly
/// `completed`. Incomplete function calls and partial text are never
/// treated as success. A safety refusal is a bounded assistant Done.
pub fn parse_responses_body(bytes: &[u8]) -> Result<ParsedResponses, String> {
    if bytes.len() > MAX_RESPONSES_BODY {
        return Err("response body too large".to_string());
    }
    let v = json::parse(bytes).map_err(|_| "malformed responses json".to_string())?;
    if let Some(err) = v.get("error") {
        if !err.is_null() {
            return Err(api_error_category(err));
        }
    }
    let id = match v.get("id") {
        Some(Value::Str(s)) => s.as_str(),
        _ => return Err("response id missing or invalid".to_string()),
    };
    if id.is_empty() || id.len() > MAX_RESPONSE_ID {
        return Err("response id missing or invalid".to_string());
    }
    let status = match v.get("status") {
        Some(Value::Str(s)) => s.as_str(),
        Some(_) => return Err("response status has the wrong type".to_string()),
        None => return Err("response status missing".to_string()),
    };
    let output = match v.get("output") {
        Some(Value::Arr(items)) => items,
        Some(_) => return Err("response output has the wrong shape".to_string()),
        None => return Err("response output missing".to_string()),
    };
    let incomplete_reason = match v.get("incomplete_details") {
        None | Some(Value::Null) => None,
        Some(Value::Obj(details)) => match details.iter().find(|(k, _)| k == "reason") {
            None | Some((_, Value::Null)) => None,
            Some((_, Value::Str(s))) => Some(s.as_str()),
            Some((_, _)) => return Err("incomplete_details.reason has the wrong type".to_string()),
        },
        Some(_) => return Err("incomplete_details has the wrong shape".to_string()),
    };
    if is_safety_refusal(incomplete_reason, output) {
        return Ok(ParsedResponses {
            id: id.to_string(),
            text: SAFETY_REFUSAL_TEXT.to_string(),
            function_call: None,
        });
    }
    if status != "completed" {
        return Err(status_error(status, incomplete_reason));
    }
    let mut text = String::new();
    let mut function_call = None;
    for item in output {
        require_item_completed(item)?;
        match item.get("type").and_then(Value::as_str) {
            Some("message") => append_message_text(&mut text, item)?,
            Some("output_text") => {
                text.push_str(need_output_text(item)?);
            }
            Some("function_call") => {
                if function_call.is_some() {
                    return Err("multiple function calls are not allowed".to_string());
                }
                function_call = Some(parse_function_call(item)?);
            }
            Some("reasoning") => {}
            Some(_) => return Err("unsupported output item type".to_string()),
            None => return Err("output item missing type".to_string()),
        }
        if text.len() > MAX_TURN_TEXT_BYTES {
            return Err("assistant text too large".to_string());
        }
    }
    Ok(ParsedResponses { id: id.to_string(), text, function_call })
}

/// HTTP seam so tests can script the Responses hop. Implementations must
/// not put the bearer, headers, or body into returned error strings.
pub trait ResponsesTransport: Send + Sync + 'static {
    fn post_json(
        &self,
        url: &str,
        api_key: &str,
        body: &[u8],
        cancel: &CancelToken,
        timeout: Duration,
    ) -> Result<RawHttp, String>;
}

pub struct RawHttp {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Real transport over [`makepad_network::blocking_http`].
pub struct BlockingTransport;

impl ResponsesTransport for BlockingTransport {
    fn post_json(
        &self,
        url: &str,
        api_key: &str,
        body: &[u8],
        cancel: &CancelToken,
        timeout: Duration,
    ) -> Result<RawHttp, String> {
        let req = Request::post(url)
            .bearer(api_key)
            .map_err(safe_http_err)?
            .json_body(body.to_vec())
            .map_err(safe_http_err)?
            .cancel_token(cancel.clone())
            .limits(Limits {
                max_body_bytes: MAX_RESPONSES_BODY,
                total_timeout: timeout,
                ..Limits::default()
            });
        match post_json(req) {
            Ok(resp) => Ok(RawHttp { status: resp.status, body: resp.body }),
            Err(e) => Err(safe_http_err(e)),
        }
    }
}

fn safe_http_err(err: HttpError) -> String {
    err.to_string()
}

#[derive(Debug)]
pub struct ParsedResponses {
    pub id: String,
    pub text: String,
    pub function_call: Option<ParsedFunctionCall>,
}

#[derive(Debug)]
pub struct ParsedFunctionCall {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

fn is_safety_refusal(incomplete_reason: Option<&str>, output: &[Value]) -> bool {
    if incomplete_reason == Some("content_filter") {
        return true;
    }
    for item in output {
        match item.get("type").and_then(Value::as_str) {
            Some("refusal") => return true,
            Some("message") => {
                if let Some(Value::Arr(content)) = item.get("content") {
                    if content.iter().any(|c| c.get("type").and_then(Value::as_str) == Some("refusal"))
                    {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

fn need_output_text(item: &Value) -> Result<&str, String> {
    match item.get("text") {
        Some(Value::Str(s)) => Ok(s.as_str()),
        Some(_) => Err("output_text.text has the wrong type".to_string()),
        None => Err("output_text.text missing".to_string()),
    }
}

fn require_item_completed(item: &Value) -> Result<(), String> {
    match item.get("status") {
        None => Ok(()),
        Some(Value::Str(s)) if s == "completed" => Ok(()),
        Some(Value::Str(_)) => Err("output item is not completed".to_string()),
        Some(_) => Err("output item status has the wrong type".to_string()),
    }
}

fn status_error(status: &str, reason: Option<&str>) -> String {
    match status {
        "incomplete" => match reason {
            Some("max_output_tokens") => "response incomplete: output limit".to_string(),
            Some("content_filter") => "response incomplete: safety".to_string(),
            _ => "response incomplete".to_string(),
        },
        "failed" => "response status failed".to_string(),
        "cancelled" => "response status cancelled".to_string(),
        "in_progress" => "response status in_progress".to_string(),
        _ => "unsupported response status".to_string(),
    }
}

fn append_message_text(text: &mut String, item: &Value) -> Result<(), String> {
    let content = match item.get("content") {
        Some(Value::Arr(items)) => items,
        Some(Value::Str(s)) => {
            text.push_str(s);
            return Ok(());
        }
        Some(_) => return Err("message content has the wrong shape".to_string()),
        None => return Err("message content missing".to_string()),
    };
    for c in content {
        match c.get("type").and_then(Value::as_str) {
            Some("output_text") => {
                text.push_str(need_output_text(c)?);
            }
            Some("reasoning") => {}
            Some(_) => return Err("unsupported message content type".to_string()),
            None => return Err("message content item missing type".to_string()),
        }
    }
    Ok(())
}

fn parse_function_call(item: &Value) -> Result<ParsedFunctionCall, String> {
    let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
    if call_id.is_empty()
        || call_id.len() > MAX_CALL_ID
        || !call_id.bytes().all(|b| b.is_ascii_graphic())
    {
        return Err("function call_id missing or invalid".to_string());
    }
    let name = item.get("name").and_then(Value::as_str).unwrap_or("");
    if name.is_empty() || name.len() > MAX_FN_NAME {
        return Err("function name missing or invalid".to_string());
    }
    let arguments = item
        .get("arguments")
        .and_then(Value::as_str)
        .ok_or_else(|| "function arguments missing".to_string())?;
    if arguments.len() > MAX_TOOL_JSON_BYTES {
        return Err("function arguments too large".to_string());
    }
    Ok(ParsedFunctionCall {
        call_id: call_id.to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
    })
}

/// Map an upstream error object to a static category. Never copies
/// `error.message` (it can contain keys, headers, or request ids).
fn api_error_category(err: &Value) -> String {
    let typ = match err {
        Value::Str(_) => "",
        Value::Obj(_) => err.get("type").and_then(Value::as_str).unwrap_or(""),
        _ => return "api error".to_string(),
    };
    let code = err.get("code").and_then(Value::as_str).unwrap_or("");
    match (typ, code) {
        ("invalid_request_error", _) => "api error: invalid request".to_string(),
        ("authentication_error", _) | (_, "invalid_api_key") => {
            "api error: authentication".to_string()
        }
        ("permission_error", _) => "api error: permission".to_string(),
        ("rate_limit_error", _) | (_, "rate_limit_exceeded") => {
            "api error: rate limited".to_string()
        }
        ("server_error", _) => "api error: server".to_string(),
        _ => "api error".to_string(),
    }
}

enum WorkerOut {
    Ok { parsed: ParsedResponses, on_success_sent: usize },
    Err(String),
}

struct Active {
    rx: Receiver<WorkerOut>,
    restore_call_id: Option<String>,
}

pub struct ResponsesChatProvider<T: ResponsesTransport> {
    config: ResponsesConfig,
    transport: Arc<T>,
    cancel: CancelToken,
    active: Option<Active>,
    previous_response_id: Option<String>,
    sent_messages: usize,
    pending_call_id: Option<String>,
    last_instructions: String,
    last_tools_enabled: bool,
}

impl<T: ResponsesTransport> ResponsesChatProvider<T> {
    pub fn new(config: ResponsesConfig, transport: T) -> ResponsesChatProvider<T> {
        ResponsesChatProvider {
            config,
            transport: Arc::new(transport),
            cancel: CancelToken::new(),
            active: None,
            previous_response_id: None,
            sent_messages: 0,
            pending_call_id: None,
            last_instructions: String::new(),
            last_tools_enabled: true,
        }
    }

    fn spawn_request(
        &mut self,
        body: Value,
        on_success_sent: usize,
        restore_call_id: Option<String>,
    ) -> Result<(), String> {
        if self.active.is_some() {
            return Err("a turn is already in flight".to_string());
        }
        let key = self
            .config
            .api_key
            .as_ref()
            .ok_or_else(|| self.config.missing_key_reason.clone())?
            .as_str()
            .to_string();
        let url = self.config.endpoint.clone();
        let bytes = body.to_json().into_bytes();
        if bytes.len() > MAX_RESPONSES_BODY {
            return Err("request body too large".to_string());
        }
        let (tx, rx) = mpsc::channel();
        let transport = self.transport.clone();
        let cancel = self.cancel.clone();
        let timeout = self.config.request_timeout;
        std::thread::Builder::new()
            .name("content-chat-responses".into())
            .spawn(move || {
                let result = transport.post_json(&url, &key, &bytes, &cancel, timeout);
                if cancel.is_cancelled() {
                    return;
                }
                let out = match result {
                    Err(e) => WorkerOut::Err(sanitize_public_error(&e)),
                    Ok(raw) => {
                        if raw.body.len() > MAX_RESPONSES_BODY {
                            WorkerOut::Err("response body too large".to_string())
                        } else if !(200..300).contains(&raw.status) {
                            WorkerOut::Err(http_status_error(raw.status, &raw.body))
                        } else {
                            match parse_responses_body(&raw.body) {
                                Ok(parsed) => WorkerOut::Ok { parsed, on_success_sent },
                                Err(e) => WorkerOut::Err(sanitize_public_error(&e)),
                            }
                        }
                    }
                };
                if !cancel.is_cancelled() {
                    let _ = tx.send(out);
                }
            })
            .map_err(|_| "failed to start provider worker".to_string())?;
        self.active = Some(Active { rx, restore_call_id });
        Ok(())
    }

    fn build_common(&self, input: Value) -> Value {
        let mut pairs = vec![("model", json::s(self.config.model.clone()))];
        if let Some(id) = &self.previous_response_id {
            pairs.push(("previous_response_id", json::s(id.clone())));
        }
        pairs.push(("instructions", json::s(self.last_instructions.clone())));
        pairs.push(("input", input));
        if self.last_tools_enabled {
            pairs.push(("tools", tools::native_tools_payload()));
            pairs.push(("tool_choice", json::s("auto")));
            pairs.push(("parallel_tool_calls", Value::Bool(false)));
        } else {
            pairs.push(("tools", Value::Arr(Vec::new())));
            pairs.push(("tool_choice", json::s("none")));
        }
        pairs.push((
            "max_output_tokens",
            Value::Int(self.config.max_output_tokens as i64),
        ));
        json::obj(pairs)
    }
}

fn http_status_error(status: u16, body: &[u8]) -> String {
    if let Ok(v) = json::parse(body) {
        if let Some(err) = v.get("error") {
            if !err.is_null() {
                return api_error_category(err);
            }
        }
    }
    match status {
        401 | 403 => "http error: authentication".to_string(),
        429 => "http error: rate limited".to_string(),
        400 | 422 => "http error: invalid request".to_string(),
        500..=599 => "http error: server".to_string(),
        _ => format!("http {status}"),
    }
}

fn validate_turn_messages(messages: &[ChatMessage]) -> Result<(), String> {
    if messages.len() > MAX_MESSAGES {
        return Err("too many messages".to_string());
    }
    for m in messages {
        m.validate().map_err(|_| "message empty or too large".to_string())?;
    }
    Ok(())
}

fn encode_input_messages(messages: &[ChatMessage]) -> Value {
    Value::Arr(
        messages
            .iter()
            .map(|m| {
                json::obj(vec![
                    ("type", json::s("message")),
                    ("role", json::s(m.role.slug())),
                    ("content", json::s(m.text.clone())),
                ])
            })
            .collect(),
    )
}

fn endpoint_detail(url: &str) -> String {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    rest.split('/').next().unwrap_or("configured-endpoint").to_string()
}

impl<T: ResponsesTransport> ChatProvider for ResponsesChatProvider<T> {
    fn kind(&self) -> ProviderKind {
        self.config.kind
    }

    fn availability(&mut self) -> ProviderAvailability {
        match &self.config.api_key {
            Some(_) => ProviderAvailability::Available {
                model: self.config.model.clone(),
                detail: endpoint_detail(&self.config.endpoint),
            },
            None => ProviderAvailability::Unavailable {
                reason: self.config.missing_key_reason.clone(),
            },
        }
    }

    fn begin_turn(&mut self, input: &TurnInput) -> Result<(), String> {
        if self.active.is_some() {
            return Err("a turn is already in flight".to_string());
        }
        if self.pending_call_id.is_some() {
            // Fail closed: an unresolved function call cannot be turned into
            // a new user turn. The session must end rather than replay.
            return Err("unresolved function call".to_string());
        }
        if self.config.api_key.is_none() {
            return Err(self.config.missing_key_reason.clone());
        }
        validate_turn_messages(&input.messages)?;
        if self.sent_messages > input.messages.len() {
            return Err("conversation cursor is ahead of history".to_string());
        }
        let tail = &input.messages[self.sent_messages..];
        let encoded = encode_input_messages(tail);
        if encoded.as_arr().map(|a| a.is_empty()).unwrap_or(true) {
            return Err("no new input to send".to_string());
        }
        self.last_instructions = input.system.clone();
        self.last_tools_enabled = input.tools_enabled;
        let body = self.build_common(encoded);
        let on_success_sent = input.messages.len().saturating_add(1);
        self.spawn_request(body, on_success_sent, None)
    }

    fn continue_function(&mut self, call_id: &str, output: &str) -> Result<(), String> {
        if self.active.is_some() {
            return Err("a turn is already in flight".to_string());
        }
        match self.pending_call_id.as_deref() {
            None => return Err("no function call is awaiting output".to_string()),
            Some(pending) if pending != call_id => {
                return Err("mismatched function call id".to_string());
            }
            Some(_) => {}
        }
        if self.previous_response_id.is_none() {
            return Err("no previous response to continue".to_string());
        }
        if call_id.len() > MAX_CALL_ID {
            return Err("function call_id missing or invalid".to_string());
        }
        if output.len() > MAX_TOOL_JSON_BYTES {
            return Err("function output too large".to_string());
        }
        let pending = self.pending_call_id.take().unwrap();
        let input = Value::Arr(vec![json::obj(vec![
            ("type", json::s("function_call_output")),
            ("call_id", json::s(pending)),
            ("output", json::s(output)),
        ])]);
        let body = self.build_common(input);
        let on_success_sent = self.sent_messages.saturating_add(2);
        if let Err(e) = self.spawn_request(body, on_success_sent, Some(call_id.to_string())) {
            self.pending_call_id = Some(call_id.to_string());
            return Err(e);
        }
        Ok(())
    }

    fn poll(&mut self) -> Vec<ProviderEvent> {
        let Some(active) = &self.active else {
            return Vec::new();
        };
        match active.rx.try_recv() {
            Ok(WorkerOut::Ok { parsed, on_success_sent }) => {
                self.active = None;
                self.previous_response_id = Some(parsed.id);
                self.sent_messages = on_success_sent;
                let mut events = Vec::new();
                for chunk in split_delta_text(&parsed.text) {
                    events.push(ProviderEvent::Delta(chunk));
                }
                if let Some(fc) = parsed.function_call {
                    self.pending_call_id = Some(fc.call_id.clone());
                    events.push(ProviderEvent::FunctionCall {
                        call_id: fc.call_id,
                        name: fc.name,
                        arguments: fc.arguments,
                    });
                } else {
                    self.pending_call_id = None;
                    events.push(ProviderEvent::Done { text: parsed.text });
                }
                events
            }
            Ok(WorkerOut::Err(message)) => {
                let restore = self.active.take().and_then(|a| a.restore_call_id);
                if let Some(id) = restore {
                    self.pending_call_id = Some(id);
                }
                vec![ProviderEvent::Error(sanitize_public_error(&message))]
            }
            Err(mpsc::TryRecvError::Empty) => Vec::new(),
            Err(mpsc::TryRecvError::Disconnected) => {
                let restore = self.active.take().and_then(|a| a.restore_call_id);
                if let Some(id) = restore {
                    self.pending_call_id = Some(id);
                }
                vec![ProviderEvent::Error("provider worker ended".to_string())]
            }
        }
    }

    fn cancel(&mut self) {
        self.cancel.cancel();
        let restore = self.active.take().and_then(|a| a.restore_call_id);
        if let Some(id) = restore {
            self.pending_call_id = Some(id);
        }
        // Idle cancel and in-flight cancel both retain the conversation
        // chain. An unresolved continuation is fail-closed by begin_turn.
        self.cancel = CancelToken::new();
    }
}

impl<T: ResponsesTransport> Drop for ResponsesChatProvider<T> {
    fn drop(&mut self) {
        if self.active.is_some() {
            self.cancel.cancel();
            self.active = None;
        }
    }
}
