//! The `intake` credential's scope on every AI hub (docs/agents/intake.md).
//!
//! The hostile-intake jail on a Linux test box holds one fleet credential, role paired, named
//! `intake` (`fleet-id issue intake paired`). The name is in the root-signed credential, so the
//! jail cannot change it, and this rule is compiled into every hub, so nothing the jail sends can
//! switch it off:
//! - **Routes** ([`route_allowed`]): `/health`, `POST /generate`, the caller's own job polls,
//!   artifacts and cancels, and `POST /v1/messages`. Everything else is a 403: model blobs and
//!   inventories, `/fleet`, `/models`, kernels, realtime sessions, every control route.
//! - **`POST /generate`** ([`check_generate`]): the prefilter's one shape. A pinned Qwen model,
//!   domain `chat`, one user message of text, thinking off, greedy, a bounded token count. No
//!   picture, input payload, peer source, LoRA, download, tool block or any other field: a field
//!   this list does not name is refused, so a new hub feature never widens the scope by itself.
//! - **`POST /v1/messages`** ([`check_messages`], [`route_messages`]): a capability gate in front
//!   of the Anthropic Messages API, served only by a hub that serves the Claude login (the user's
//!   switch). The hub adds the login (the same keychain read as `claude-cli`), so no token is ever
//!   in the jail. Only client-defined tools pass: a tool with a `type` (web search, web fetch,
//!   code execution, bash, text editor, computer use, memory, MCP toolsets) is refused, and so are
//!   `mcp_servers`, `container`, document, file and URL sources, citations, server tool blocks and
//!   every top-level field not listed. The client's headers are dropped (no client betas). The
//!   models are pinned, tokens are capped per request and per item, and the credential is rate
//!   limited. The request that goes upstream is the parsed and checked value written out again,
//!   never the client's bytes.
//!
//! Every intake call names its item (`X-Intake-Item: relax#12`, `makepad#7`, `eval` or `canary`)
//! and is logged with it in `intake-calls.jsonl` in the hub's cache dir and on stderr.

use crate::providers::claude_api::{ANTHROPIC_BETA, ANTHROPIC_VERSION, CLAUDE_MESSAGES_URL};
use makepad_network::blocking_http::{self, Limits, Request};
use makepad_network::{HttpServerResponse, HttpServerResponseSender};
use makepad_strict_json::{self as json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The Qwen models the prefilter may name.
pub const PREFILTER_MODELS: &[&str] = &["qwen3.8-27b"];
/// The Claude models the whitepaper agent may name.
pub const MESSAGE_MODELS: &[&str] = &["claude-opus-5-5", "claude-sonnet-5-5"];
/// Output tokens one prefilter call may ask for (60 + 8 per segment, 24 segments a batch, room).
pub const MAX_PREFILTER_TOKENS: i64 = 1024;
/// Output tokens one Messages call may ask for.
pub const MAX_MESSAGE_TOKENS: i64 = 32_000;
/// Messages calls a minute for the intake credential (a turn of the agent loop is one call).
pub const RATE_PER_MINUTE: usize = 30;
/// Tokens (input, output and cache writes and reads) one item may spend through `/v1/messages`.
pub const ITEM_TOKEN_BUDGET: u64 = 12_000_000;
/// Messages calls one item may make.
pub const ITEM_REQUEST_BUDGET: u32 = 800;
/// Calls waiting for the upstream worker; a full queue is a 429 the jail retries.
const QUEUE: usize = 8;
const MAX_MESSAGES_BODY: usize = 16 * 1024 * 1024;
const MAX_UPSTREAM_BODY: usize = 32 * 1024 * 1024;
const MAX_TEXT: usize = 128 * 1024;
const MAX_SYSTEM: usize = 64 * 1024;

/// Whether a fleet credential name is an intake credential.
pub fn is_intake(name: &str) -> bool {
    name == "intake" || name.starts_with("intake-")
}

/// The routes an intake credential reaches (the fleet front's policy for it, on the LAN and the
/// edge alike).
pub fn route_allowed(method: &str, path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    matches!((method, path), ("GET", "/health") | ("POST", "/generate") | ("POST", "/v1/messages"))
        || (method == "GET" && (path.starts_with("/job/") || path.starts_with("/artifact/")))
        || (method == "POST" && path.starts_with("/job/") && path.ends_with("/cancel"))
}

/// Why a call was refused, as a fixed word for the log and the answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    NotIntake,
    NoItem,
    BadJson,
    Field,
    Model,
    Shape,
    TooLarge,
    ServerTool,
    Block,
    Source,
    Stream,
    ClaudeOff,
    Rate,
    Busy,
    ItemBudget,
}

impl Refusal {
    pub fn name(self) -> &'static str {
        match self {
            Refusal::NotIntake => "not-intake",
            Refusal::NoItem => "no-item",
            Refusal::BadJson => "bad-json",
            Refusal::Field => "field",
            Refusal::Model => "model",
            Refusal::Shape => "shape",
            Refusal::TooLarge => "too-large",
            Refusal::ServerTool => "server-tool",
            Refusal::Block => "block",
            Refusal::Source => "source",
            Refusal::Stream => "stream",
            Refusal::ClaudeOff => "claude-off",
            Refusal::Rate => "rate",
            Refusal::Busy => "busy",
            Refusal::ItemBudget => "item-budget",
        }
    }

    fn status(self) -> u16 {
        match self {
            Refusal::Rate | Refusal::Busy | Refusal::ItemBudget => 429,
            Refusal::ClaudeOff => 503,
            Refusal::BadJson | Refusal::TooLarge => 400,
            _ => 403,
        }
    }

    pub fn response(self) -> HttpServerResponse {
        let body = json::obj(vec![("error", json::s(format!("intake scope: {}", self.name()))), ("refused", json::s(self.name()))]).to_json();
        let retry = if self.status() == 429 { "Retry-After: 20\r\n" } else { "" };
        let header = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\n{retry}Content-Length: {}\r\nConnection: close\r\n\r\n",
            self.status(),
            if self.status() == 429 { "Too Many Requests" } else if self.status() == 503 { "Service Unavailable" } else if self.status() == 400 { "Bad Request" } else { "Forbidden" },
            body.len()
        );
        HttpServerResponse::new(header, body.into_bytes())
    }
}

/// The item a call is for, from its `X-Intake-Item` header: `relax#<n>`, `makepad#<n>`, `eval` or
/// `canary`.
pub fn item_of(lines: &[String]) -> Result<String, Refusal> {
    let value = lines.iter().find_map(|l| {
        let (n, v) = l.trim_end().split_once(':')?;
        n.trim().eq_ignore_ascii_case("X-Intake-Item").then(|| v.trim().to_string())
    }).ok_or(Refusal::NoItem)?;
    valid_item(&value).then_some(value).ok_or(Refusal::NoItem)
}

pub fn valid_item(item: &str) -> bool {
    if matches!(item, "eval" | "canary") {
        return true;
    }
    let Some((repo, n)) = item.split_once('#') else { return false };
    matches!(repo, "relax" | "makepad") && !n.is_empty() && n.len() <= 7 && !n.starts_with('0') && n.bytes().all(|b| b.is_ascii_digit())
}

fn obj_fields(v: &Value) -> Result<&[(String, Value)], Refusal> {
    match v {
        Value::Obj(fields) => Ok(fields),
        _ => Err(Refusal::Shape),
    }
}

fn only(fields: &[(String, Value)], allowed: &[&str]) -> Result<(), Refusal> {
    fields.iter().all(|(k, _)| allowed.contains(&k.as_str())).then_some(()).ok_or(Refusal::Field)
}

fn text(v: Option<&Value>, max: usize) -> Result<&str, Refusal> {
    let s = v.and_then(Value::as_str).ok_or(Refusal::Shape)?;
    if s.len() > max { Err(Refusal::TooLarge) } else { Ok(s) }
}

fn is_zero(v: &Value) -> bool {
    matches!(v, Value::Int(0)) || matches!(v, Value::F64(f) if *f == 0.0)
}

fn number_in(v: &Value, lo: f64, hi: f64) -> bool {
    match v {
        Value::Int(i) => (*i as f64) >= lo && (*i as f64) <= hi,
        Value::F64(f) => *f >= lo && *f <= hi,
        _ => false,
    }
}

/// The prefilter's one `/generate` shape; Ok with the model.
pub fn check_generate(body: &[u8]) -> Result<String, Refusal> {
    let v = json::parse_depth(body, 8).map_err(|_| Refusal::BadJson)?;
    let fields = obj_fields(&v)?;
    only(fields, &["model", "domain", "chat_system", "chat_messages", "max_tokens", "temperature", "thinking", "seed", "chat_session"])?;
    let model = v.get("model").and_then(Value::as_str).ok_or(Refusal::Shape)?;
    if !PREFILTER_MODELS.contains(&model) {
        return Err(Refusal::Model);
    }
    if v.get("domain").and_then(Value::as_str) != Some("chat") {
        return Err(Refusal::Shape);
    }
    if v.get("chat_system").is_some() {
        text(v.get("chat_system"), MAX_SYSTEM)?;
    }
    let messages = v.get("chat_messages").and_then(Value::as_arr).ok_or(Refusal::Shape)?;
    let [message] = messages else { return Err(Refusal::Shape) };
    only(obj_fields(message)?, &["role", "text"])?;
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return Err(Refusal::Shape);
    }
    text(message.get("text"), MAX_TEXT)?;
    if v.get("thinking").and_then(Value::as_bool) != Some(false) || !v.get("temperature").is_some_and(is_zero) {
        return Err(Refusal::Shape);
    }
    match v.get("max_tokens").and_then(Value::as_i64) {
        Some(n) if (1..=MAX_PREFILTER_TOKENS).contains(&n) => {}
        _ => return Err(Refusal::Shape),
    }
    if let Some(seed) = v.get("seed") {
        seed.as_u64().ok_or(Refusal::Shape)?;
    }
    if let Some(session) = v.get("chat_session") {
        let s = text(Some(session), 64)?;
        if !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return Err(Refusal::Shape);
        }
    }
    Ok(model.to_string())
}

fn check_cache_control(v: Option<&Value>) -> Result<(), Refusal> {
    let Some(v) = v else { return Ok(()) };
    only(obj_fields(v)?, &["type", "ttl"])?;
    if v.get("type").and_then(Value::as_str) != Some("ephemeral") {
        return Err(Refusal::Shape);
    }
    match v.get("ttl").map(Value::as_str) {
        None | Some(Some("5m" | "1h")) => Ok(()),
        _ => Err(Refusal::Shape),
    }
}

fn check_text_block(b: &Value) -> Result<(), Refusal> {
    only(obj_fields(b)?, &["type", "text", "cache_control"])?;
    text(b.get("text"), MAX_TEXT * 8)?;
    check_cache_control(b.get("cache_control"))
}

fn check_image_block(b: &Value) -> Result<(), Refusal> {
    only(obj_fields(b)?, &["type", "source", "cache_control"])?;
    let source = b.get("source").ok_or(Refusal::Shape)?;
    only(obj_fields(source)?, &["type", "media_type", "data"]).map_err(|_| Refusal::Source)?;
    if source.get("type").and_then(Value::as_str) != Some("base64") {
        return Err(Refusal::Source);
    }
    match source.get("media_type").and_then(Value::as_str) {
        Some("image/png" | "image/jpeg" | "image/gif" | "image/webp") => {}
        _ => return Err(Refusal::Source),
    }
    text(source.get("data"), MAX_MESSAGES_BODY)?;
    check_cache_control(b.get("cache_control"))
}

fn tool_name_ok(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn check_block(b: &Value, role: &str) -> Result<(), Refusal> {
    let kind = b.get("type").and_then(Value::as_str).ok_or(Refusal::Shape)?;
    match (kind, role) {
        ("text", _) => check_text_block(b),
        ("image", "user") => check_image_block(b),
        ("tool_use", "assistant") => {
            only(obj_fields(b)?, &["type", "id", "name", "input", "cache_control"])?;
            text(b.get("id"), 128)?;
            if !tool_name_ok(text(b.get("name"), 64)?) {
                return Err(Refusal::Shape);
            }
            obj_fields(b.get("input").ok_or(Refusal::Shape)?)?;
            check_cache_control(b.get("cache_control"))
        }
        ("tool_result", "user") => {
            only(obj_fields(b)?, &["type", "tool_use_id", "content", "is_error", "cache_control"])?;
            text(b.get("tool_use_id"), 128)?;
            match b.get("content") {
                None | Some(Value::Str(_)) => {}
                Some(Value::Arr(parts)) => {
                    for part in parts {
                        match part.get("type").and_then(Value::as_str) {
                            Some("text") => check_text_block(part)?,
                            Some("image") => check_image_block(part)?,
                            _ => return Err(Refusal::Block),
                        }
                    }
                }
                Some(_) => return Err(Refusal::Shape),
            }
            if let Some(e) = b.get("is_error") {
                e.as_bool().ok_or(Refusal::Shape)?;
            }
            check_cache_control(b.get("cache_control"))
        }
        ("thinking", "assistant") => {
            only(obj_fields(b)?, &["type", "thinking", "signature"])?;
            text(b.get("thinking"), MAX_MESSAGES_BODY)?;
            text(b.get("signature"), MAX_MESSAGES_BODY)?;
            Ok(())
        }
        ("redacted_thinking", "assistant") => {
            only(obj_fields(b)?, &["type", "data"])?;
            text(b.get("data"), MAX_MESSAGES_BODY)?;
            Ok(())
        }
        // server tool blocks, documents, search results, MCP and container blocks, and a block
        // in the wrong role's turn
        _ => Err(Refusal::Block),
    }
}

/// The checked `/v1/messages` request: Ok with the value to send upstream and its model.
pub fn check_messages(body: &[u8]) -> Result<(Value, String), Refusal> {
    if body.len() > MAX_MESSAGES_BODY {
        return Err(Refusal::TooLarge);
    }
    let v = json::parse_depth(body, 64).map_err(|_| Refusal::BadJson)?;
    let fields = obj_fields(&v)?;
    if fields.iter().any(|(k, _)| matches!(k.as_str(), "mcp_servers" | "container")) {
        return Err(Refusal::ServerTool);
    }
    only(fields, &["model", "max_tokens", "messages", "system", "temperature", "top_p", "top_k", "stop_sequences", "stream", "tools", "tool_choice", "thinking", "metadata"])?;
    let model = v.get("model").and_then(Value::as_str).ok_or(Refusal::Shape)?;
    if !MESSAGE_MODELS.contains(&model) {
        return Err(Refusal::Model);
    }
    match v.get("max_tokens").and_then(Value::as_i64) {
        Some(n) if (1..=MAX_MESSAGE_TOKENS).contains(&n) => {}
        _ => return Err(Refusal::Shape),
    }
    match v.get("stream") {
        None | Some(Value::Bool(false)) => {}
        _ => return Err(Refusal::Stream),
    }
    for (key, lo, hi) in [("temperature", 0.0, 1.0), ("top_p", 0.0, 1.0), ("top_k", 0.0, 500.0)] {
        if let Some(x) = v.get(key) {
            if !number_in(x, lo, hi) {
                return Err(Refusal::Shape);
            }
        }
    }
    if let Some(stops) = v.get("stop_sequences") {
        let stops = stops.as_arr().ok_or(Refusal::Shape)?;
        if stops.len() > 8 || stops.iter().any(|s| s.as_str().is_none_or(|s| s.len() > 256)) {
            return Err(Refusal::Shape);
        }
    }
    match v.get("system") {
        None | Some(Value::Str(_)) => {
            if let Some(s) = v.get("system") {
                text(Some(s), MAX_SYSTEM)?;
            }
        }
        Some(Value::Arr(blocks)) => {
            for b in blocks {
                if b.get("type").and_then(Value::as_str) != Some("text") {
                    return Err(Refusal::Block);
                }
                check_text_block(b)?;
            }
        }
        Some(_) => return Err(Refusal::Shape),
    }
    if let Some(tools) = v.get("tools") {
        let tools = tools.as_arr().ok_or(Refusal::Shape)?;
        if tools.len() > 32 {
            return Err(Refusal::Shape);
        }
        for tool in tools {
            let tf = obj_fields(tool)?;
            // a tool with a type is Anthropic's (web search, web fetch, code execution, bash,
            // text editor, computer, memory, an MCP toolset): only client-defined tools pass
            if tf.iter().any(|(k, val)| k == "type" && val.as_str() != Some("custom")) {
                return Err(Refusal::ServerTool);
            }
            only(tf, &["type", "name", "description", "input_schema", "cache_control"]).map_err(|_| Refusal::ServerTool)?;
            if !tool_name_ok(text(tool.get("name"), 64)?) {
                return Err(Refusal::Shape);
            }
            if let Some(d) = tool.get("description") {
                text(Some(d), 16 * 1024)?;
            }
            obj_fields(tool.get("input_schema").ok_or(Refusal::ServerTool)?)?;
            check_cache_control(tool.get("cache_control"))?;
        }
    }
    if let Some(choice) = v.get("tool_choice") {
        only(obj_fields(choice)?, &["type", "name", "disable_parallel_tool_use"])?;
        match choice.get("type").and_then(Value::as_str) {
            Some("auto" | "any" | "none") => {}
            Some("tool") => {
                tool_name_ok(text(choice.get("name"), 64)?).then_some(()).ok_or(Refusal::Shape)?;
            }
            _ => return Err(Refusal::Shape),
        }
    }
    if let Some(thinking) = v.get("thinking") {
        only(obj_fields(thinking)?, &["type", "budget_tokens"])?;
        match thinking.get("type").and_then(Value::as_str) {
            Some("enabled" | "disabled" | "adaptive") => {}
            _ => return Err(Refusal::Shape),
        }
        if let Some(b) = thinking.get("budget_tokens") {
            b.as_u64().filter(|b| *b <= MAX_MESSAGE_TOKENS as u64).ok_or(Refusal::Shape)?;
        }
    }
    if let Some(meta) = v.get("metadata") {
        only(obj_fields(meta)?, &["user_id"])?;
        text(meta.get("user_id"), 256)?;
    }
    let messages = v.get("messages").and_then(Value::as_arr).ok_or(Refusal::Shape)?;
    if messages.is_empty() || messages.len() > 2000 {
        return Err(Refusal::Shape);
    }
    for m in messages {
        only(obj_fields(m)?, &["role", "content"])?;
        let role = match m.get("role").and_then(Value::as_str) {
            Some(r @ ("user" | "assistant")) => r,
            _ => return Err(Refusal::Shape),
        };
        match m.get("content") {
            Some(Value::Str(s)) if s.len() <= MAX_TEXT * 8 => {}
            Some(Value::Arr(blocks)) => {
                for b in blocks {
                    check_block(b, role)?;
                }
            }
            _ => return Err(Refusal::Shape),
        }
    }
    let model = model.to_string();
    Ok((v, model))
}

#[derive(Default)]
struct ItemUse {
    requests: u32,
    tokens: u64,
}

/// Rate and per-item budgets for the intake credential (in memory: a hub restart starts them
/// again, and the log keeps every call).
#[derive(Default)]
pub struct Ledger {
    items: HashMap<String, ItemUse>,
    recent: VecDeque<Instant>,
}

impl Ledger {
    pub fn admit(&mut self, item: &str, now: Instant) -> Result<(), Refusal> {
        while self.recent.front().is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60)) {
            self.recent.pop_front();
        }
        if self.recent.len() >= RATE_PER_MINUTE {
            return Err(Refusal::Rate);
        }
        let used = self.items.entry(item.to_string()).or_default();
        if used.requests >= ITEM_REQUEST_BUDGET || used.tokens >= ITEM_TOKEN_BUDGET {
            return Err(Refusal::ItemBudget);
        }
        used.requests += 1;
        self.recent.push_back(now);
        Ok(())
    }

    pub fn spent(&mut self, item: &str, tokens: u64) {
        self.items.entry(item.to_string()).or_default().tokens += tokens;
    }
}

fn ledger() -> &'static Mutex<Ledger> {
    static LEDGER: OnceLock<Mutex<Ledger>> = OnceLock::new();
    LEDGER.get_or_init(|| Mutex::new(Ledger::default()))
}

/// One line of the intake call log.
pub struct CallLog<'a> {
    pub client: &'a str,
    pub item: &'a str,
    pub route: &'a str,
    pub model: &'a str,
    pub outcome: &'a str,
    pub tokens: u64,
}

pub fn log_call(cache_dir: &Path, call: &CallLog) {
    let line = json::obj(vec![
        ("t", Value::Int(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64))),
        ("client", json::s(call.client)),
        ("item", json::s(call.item)),
        ("route", json::s(call.route)),
        ("model", json::s(call.model)),
        ("outcome", json::s(call.outcome)),
        ("tokens", Value::Int(call.tokens as i64)),
    ]).to_json();
    eprintln!("intake-call: {line}");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(cache_dir.join("intake-calls.jsonl")) {
        let _ = writeln!(f, "{line}");
    }
}

/// Tokens a Messages answer reports (input, output, cache writes and reads).
pub fn usage_tokens(answer: &[u8]) -> u64 {
    let Ok(v) = json::parse_depth(answer, 64) else { return 0 };
    let Some(usage) = v.get("usage") else { return 0 };
    ["input_tokens", "output_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"]
        .iter()
        .filter_map(|k| usage.get(k).and_then(Value::as_u64))
        .sum()
}

struct Call {
    body: Vec<u8>,
    client: String,
    item: String,
    model: String,
    cache_dir: PathBuf,
    response: HttpServerResponseSender,
}

/// The one long-lived upstream worker (the hub's route loop never waits on Anthropic).
fn worker() -> &'static SyncSender<Call> {
    static WORKER: OnceLock<SyncSender<Call>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Call>(QUEUE);
        std::thread::Builder::new()
            .name("intake-messages".into())
            .spawn(move || {
                while let Ok(call) = rx.recv() {
                    let (response, outcome, tokens) = upstream(&call.body);
                    ledger().lock().unwrap().spent(&call.item, tokens);
                    log_call(&call.cache_dir, &CallLog { client: &call.client, item: &call.item, route: "/v1/messages", model: &call.model, outcome: &outcome, tokens });
                    let _ = call.response.send(response);
                }
            })
            .expect("intake-messages worker");
        tx
    })
}

fn upstream(body: &[u8]) -> (HttpServerResponse, String, u64) {
    let login = match crate::claude_cli::keychain_login() {
        Ok(login) => login,
        Err(e) => {
            eprintln!("intake-call: no Claude login: {e}");
            return (Refusal::ClaudeOff.response(), "no-login".into(), 0);
        }
    };
    let request = Request::post(CLAUDE_MESSAGES_URL)
        .limits(Limits { max_body_bytes: MAX_UPSTREAM_BODY, idle_timeout: Duration::from_secs(600), ..Limits::default() })
        .header("anthropic-version", ANTHROPIC_VERSION)
        .and_then(|r| r.header("anthropic-beta", ANTHROPIC_BETA))
        .and_then(|r| r.bearer(login.expose()))
        .and_then(|r| r.json_body(body.to_vec()));
    let request = match request {
        Ok(r) => r,
        Err(e) => return (error_response(502, &format!("upstream request: {e}")), "request-error".into(), 0),
    };
    match blocking_http::request_no_redirect(request) {
        Ok(answer) => {
            let tokens = usage_tokens(&answer.body);
            let header = format!(
                "HTTP/1.1 {} Upstream\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                answer.status,
                answer.body.len()
            );
            (HttpServerResponse::new(header, answer.body), format!("upstream-{}", answer.status), tokens)
        }
        Err(e) => (error_response(502, &format!("upstream: {e}")), "upstream-error".into(), 0),
    }
}

fn error_response(status: u16, message: &str) -> HttpServerResponse {
    let body = json::obj(vec![("error", json::s(message))]).to_json();
    let header = format!("HTTP/1.1 {status} Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    HttpServerResponse::new(header, body.into_bytes())
}

/// `POST /v1/messages`, from the route loop: checks at once, then queues the call for the
/// upstream worker. `client` is the caller's credential name (None: no caller at all);
/// `claude_serving`: this hub serves the Claude login now.
pub fn route_messages(cache_dir: &Path, client: Option<&str>, claude_serving: bool, lines: &[String], body: Vec<u8>, response: HttpServerResponseSender) {
    let refuse = |client: &str, item: &str, why: Refusal| {
        log_call(cache_dir, &CallLog { client, item, route: "/v1/messages", model: "", outcome: &format!("refused-{}", why.name()), tokens: 0 });
        let _ = response.send(why.response());
    };
    let Some(client) = client.filter(|c| is_intake(c)) else {
        return refuse(client.unwrap_or("-"), "-", Refusal::NotIntake);
    };
    let item = match item_of(lines) {
        Ok(item) => item,
        Err(why) => return refuse(client, "-", why),
    };
    if !claude_serving {
        return refuse(client, &item, Refusal::ClaudeOff);
    }
    let (value, model) = match check_messages(&body) {
        Ok(ok) => ok,
        Err(why) => return refuse(client, &item, why),
    };
    if let Err(why) = ledger().lock().unwrap().admit(&item, Instant::now()) {
        return refuse(client, &item, why);
    }
    let call = Call { body: value.to_json().into_bytes(), client: client.to_string(), item: item.clone(), model, cache_dir: cache_dir.to_path_buf(), response: response.clone() };
    match worker().try_send(call) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => refuse(client, &item, Refusal::Busy),
    }
}

/// An intake caller's `POST /generate`: Ok to go on (logged), or the refusal to answer with.
pub fn gate_generate(cache_dir: &Path, client: &str, lines: &[String], body: &[u8]) -> Result<(), HttpServerResponse> {
    let item = match item_of(lines) {
        Ok(item) => item,
        Err(why) => {
            log_call(cache_dir, &CallLog { client, item: "-", route: "/generate", model: "", outcome: &format!("refused-{}", why.name()), tokens: 0 });
            return Err(why.response());
        }
    };
    match check_generate(body) {
        Ok(model) => {
            log_call(cache_dir, &CallLog { client, item: &item, route: "/generate", model: &model, outcome: "admitted", tokens: 0 });
            Ok(())
        }
        Err(why) => {
            log_call(cache_dir, &CallLog { client, item: &item, route: "/generate", model: "", outcome: &format!("refused-{}", why.name()), tokens: 0 });
            Err(why.response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFILTER: &str = r#"{"model":"qwen3.8-27b","domain":"chat","chat_system":"label the segments","chat_messages":[{"role":"user","text":"segments"}],"max_tokens":260,"temperature":0,"thinking":false,"seed":7}"#;

    fn with(base: &str, key: &str, value: &str) -> String {
        let mut v = json::parse_depth(base.as_bytes(), 64).unwrap();
        if let Value::Obj(fields) = &mut v {
            fields.retain(|(k, _)| k != key);
            if !value.is_empty() {
                fields.push((key.to_string(), json::parse_depth(value.as_bytes(), 64).unwrap()));
            }
        }
        v.to_json()
    }

    #[test]
    fn the_scope_is_compiled_in_and_names_only_intake_credentials() {
        assert!(is_intake("intake") && is_intake("intake-2"), "intake names");
        assert!(!is_intake("intaker") && !is_intake("n5") && !is_intake("local"), "other names stay outside the scope");
        for (m, p) in [("GET", "/health"), ("POST", "/generate"), ("GET", "/job/j1"), ("GET", "/artifact/j1-0"), ("POST", "/job/j1/cancel"), ("POST", "/v1/messages")] {
            assert!(route_allowed(m, p), "{m} {p} must be reachable");
        }
        for (m, p) in [("GET", "/fleet"), ("GET", "/models"), ("GET", "/v1/model_inventory"), ("GET", "/v1/model_blob/00"), ("POST", "/realtime"), ("GET", "/control"),
                       ("POST", "/control/model"), ("POST", "/llm/context"), ("GET", "/kernel/x"), ("POST", "/jobs"), ("POST", "/job/j1/keepalive"), ("POST", "/v1/messages/count_tokens"),
                       ("POST", "/v1/files"), ("GET", "/v1/models"), ("POST", "/bye")] {
            assert!(!route_allowed(m, p), "{m} {p} must be refused to intake");
        }
        assert!(valid_item("relax#12") && valid_item("makepad#1") && valid_item("eval") && valid_item("canary"));
        for bad in ["relax#", "relax#012", "other#1", "relax#12345678", "relax#1a", "", "canary2"] {
            assert!(!valid_item(bad), "{bad:?} is not an item");
        }
        assert_eq!(item_of(&["X-Intake-Item: relax#4".to_string()]), Ok("relax#4".to_string()));
        assert_eq!(item_of(&["Host: x".to_string()]), Err(Refusal::NoItem));
    }

    #[test]
    fn generate_takes_only_the_prefilter_shape() {
        assert_eq!(check_generate(PREFILTER.as_bytes()), Ok("qwen3.8-27b".to_string()));
        let cases = [
            ("model", r#""qwen3.5-9b""#, Refusal::Model),
            ("model", r#""claude-cli""#, Refusal::Model),
            ("domain", r#""text""#, Refusal::Shape),
            ("thinking", "true", Refusal::Shape),
            ("thinking", "", Refusal::Shape),
            ("temperature", "0.7", Refusal::Shape),
            ("max_tokens", "5000", Refusal::Shape),
            ("chat_messages", r#"[{"role":"user","text":"a","images":["AAAA"]}]"#, Refusal::Field),
            ("chat_messages", r#"[{"role":"assistant","text":"a"}]"#, Refusal::Shape),
            ("chat_messages", r#"[{"role":"user","text":"a"},{"role":"user","text":"b"}]"#, Refusal::Shape),
            ("peer_sources", r#"["https://example.org/x"]"#, Refusal::Field),
            ("pull_only", "true", Refusal::Field),
            ("loras", "[]", Refusal::Field),
            ("input_b64", r#""AAAA""#, Refusal::Field),
            ("tools", "[]", Refusal::Field),
            ("claude_model", r#""claude-opus-5-5""#, Refusal::Field),
        ];
        for (key, value, why) in cases {
            assert_eq!(check_generate(with(PREFILTER, key, value).as_bytes()), Err(why), "{key}={value}");
        }
        assert_eq!(check_generate(b"{\"model\":\"qwen3.8-27b\",\"model\":\"x\"}"), Err(Refusal::BadJson), "duplicate keys are refused");
        assert_eq!(check_generate(b"[]"), Err(Refusal::Shape));
    }

    const MESSAGES: &str = r#"{"model":"claude-opus-5-5","max_tokens":4096,"system":"write a whitepaper","tools":[{"name":"bash","description":"run a command in the jail","input_schema":{"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}}],
        "messages":[{"role":"user","content":"item text"},{"role":"assistant","content":[{"type":"text","text":"ok"},{"type":"tool_use","id":"t1","name":"bash","input":{"command":"ls"}}]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"src"}]}]}"#;

    #[test]
    fn messages_pass_client_tools_and_refuse_every_server_capability() {
        let (value, model) = check_messages(MESSAGES.as_bytes()).expect("a client-tool loop passes");
        assert_eq!(model, "claude-opus-5-5");
        assert!(value.get("tools").is_some(), "the checked value goes upstream whole");
        // the canary: a server-side tool of every kind is refused
        for tool in [
            r#"{"type":"web_search_20250305","name":"web_search","max_uses":5}"#,
            r#"{"type":"web_fetch_20250910","name":"web_fetch"}"#,
            r#"{"type":"code_execution_20250825","name":"code_execution"}"#,
            r#"{"type":"bash_20250124","name":"bash"}"#,
            r#"{"type":"text_editor_20250728","name":"str_replace_based_edit_tool"}"#,
            r#"{"type":"computer_20250124","name":"computer","display_width_px":1024,"display_height_px":768}"#,
            r#"{"type":"memory_20250818","name":"memory"}"#,
            r#"{"type":"mcp_toolset","mcp_server_name":"x"}"#,
            r#"{"name":"bash","input_schema":{"type":"object"},"allowed_callers":["code_execution_20250825"]}"#,
        ] {
            let body = with(MESSAGES, "tools", &format!("[{tool}]"));
            assert_eq!(check_messages(body.as_bytes()).map(|_| ()), Err(Refusal::ServerTool), "{tool}");
        }
        let refused = [
            ("mcp_servers", r#"[{"type":"url","url":"https://example.org/mcp","name":"x"}]"#, Refusal::ServerTool),
            ("container", r#""c1""#, Refusal::ServerTool),
            ("model", r#""claude-fable-5-2""#, Refusal::Model),
            ("model", r#""claude-haiku-4-5""#, Refusal::Model),
            ("max_tokens", "64000", Refusal::Shape),
            ("stream", "true", Refusal::Stream),
            ("betas", r#"["files-api-2025-04-14"]"#, Refusal::Field),
            ("context_management", "{}", Refusal::Field),
            ("service_tier", r#""priority""#, Refusal::Field),
            ("tool_choice", r#"{"type":"web_search"}"#, Refusal::Shape),
        ];
        for (key, value, why) in refused {
            assert_eq!(check_messages(with(MESSAGES, key, value).as_bytes()).map(|_| ()), Err(why), "{key}={value}");
        }
        let blocks = [
            (r#"{"role":"user","content":[{"type":"document","source":{"type":"url","url":"https://example.org/a.pdf"}}]}"#, Refusal::Block),
            (r#"{"role":"user","content":[{"type":"image","source":{"type":"url","url":"https://example.org/a.png"}}]}"#, Refusal::Source),
            (r#"{"role":"user","content":[{"type":"image","source":{"type":"file","file_id":"f1"}}]}"#, Refusal::Source),
            (r#"{"role":"user","content":[{"type":"container_upload","file_id":"f1"}]}"#, Refusal::Block),
            (r#"{"role":"assistant","content":[{"type":"server_tool_use","id":"s1","name":"web_search","input":{}}]}"#, Refusal::Block),
            (r#"{"role":"user","content":[{"type":"web_search_tool_result","tool_use_id":"s1","content":[]}]}"#, Refusal::Block),
            (r#"{"role":"user","content":[{"type":"search_result","source":"https://example.org","title":"t","content":[]}]}"#, Refusal::Block),
            (r#"{"role":"user","content":[{"type":"text","text":"x","citations":[]}]}"#, Refusal::Field),
            (r#"{"role":"user","content":[{"type":"tool_use","id":"t","name":"bash","input":{}}]}"#, Refusal::Block),
            (r#"{"role":"system","content":"x"}"#, Refusal::Shape),
        ];
        for (message, why) in blocks {
            let body = with(MESSAGES, "messages", &format!("[{message}]"));
            assert_eq!(check_messages(body.as_bytes()).map(|_| ()), Err(why), "{message}");
        }
        let image = r#"[{"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0K"}}]}]"#;
        assert!(check_messages(with(MESSAGES, "messages", image).as_bytes()).is_ok(), "an inline picture passes");
    }

    #[test]
    fn the_ledger_rate_limits_and_spends_item_budgets() {
        let mut ledger = Ledger::default();
        let t0 = Instant::now();
        for _ in 0..RATE_PER_MINUTE {
            assert_eq!(ledger.admit("relax#1", t0), Ok(()));
        }
        assert_eq!(ledger.admit("relax#2", t0), Err(Refusal::Rate), "a full minute refuses with a retry");
        assert_eq!(ledger.admit("relax#2", t0 + Duration::from_secs(61)), Ok(()), "the window moves on");
        ledger.spent("relax#2", ITEM_TOKEN_BUDGET);
        assert_eq!(ledger.admit("relax#2", t0 + Duration::from_secs(200)), Err(Refusal::ItemBudget), "a spent item stops");
        assert_eq!(ledger.admit("relax#3", t0 + Duration::from_secs(200)), Ok(()), "other items go on");
        assert_eq!(usage_tokens(br#"{"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":100}}"#), 115);
        assert_eq!(usage_tokens(b"not json"), 0);
    }

    #[test]
    fn the_messages_route_refuses_before_any_upstream_call() {
        let dir = std::env::temp_dir().join(format!("intake-scope-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let item = vec!["X-Intake-Item: canary".to_string()];
        let server_tool = with(MESSAGES, "tools", r#"[{"type":"web_fetch_20250910","name":"web_fetch"}]"#).into_bytes();
        let cases: [(Option<&str>, bool, &[String], Vec<u8>, &str); 5] = [
            (Some("n5"), true, &item, MESSAGES.as_bytes().to_vec(), "not-intake"),
            (None, true, &item, MESSAGES.as_bytes().to_vec(), "not-intake"),
            (Some("intake"), true, &[], MESSAGES.as_bytes().to_vec(), "no-item"),
            (Some("intake"), false, &item, MESSAGES.as_bytes().to_vec(), "claude-off"),
            (Some("intake"), true, &item, server_tool, "server-tool"),
        ];
        for (client, serving, lines, body, why) in cases {
            let (tx, rx) = mpsc::channel();
            route_messages(&dir, client, serving, lines, body, tx);
            let answer = rx.try_recv().expect("a refusal answers at once, before the worker");
            let text = String::from_utf8_lossy(&answer.body).to_string();
            assert!(text.contains(&format!("\"refused\":\"{why}\"")), "{client:?} {why}: {text}");
        }
        let log = std::fs::read_to_string(dir.join("intake-calls.jsonl")).unwrap();
        assert_eq!(log.lines().count(), 5, "every refused call is logged");
        assert!(log.contains("\"outcome\":\"refused-server-tool\"") && log.contains("\"item\":\"canary\""), "{log}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
