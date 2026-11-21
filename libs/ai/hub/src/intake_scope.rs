//! The intake scope: what a hub admits from an intake credential ([`is_intake`]). The rule is
//! compiled into every hub, so no request can switch it off.
//! - **Routes** ([`route_allowed`]): `/health`, `POST /generate`, the caller's own job polls,
//!   artifacts and cancels, and `POST /v1/messages`. Every other route is a 403.
//! - **`POST /generate`** ([`check_generate`]): one shape. A pinned model, domain `chat`, one user
//!   message of text, thinking off, greedy, a bounded token count. A field this list does not name
//!   is refused, so a new hub feature never widens the scope by itself.
//! - **`POST /v1/messages`** ([`check_messages`], [`cli_request`], [`route_messages`]): served only
//!   by a hub that serves `claude-cli`. The checked request runs as one claude-cli turn
//!   ([`crate::claude_cli::run_intake_turn`]): no tools, no MCP server, no settings, hooks,
//!   plugins, CLAUDE.md or memory, no saved session, a fresh empty directory under the OS sandbox,
//!   the user message over stdin. Only a system string and one user message of text reach it, and
//!   the answer goes back in the Messages shape ([`cli_answer`]). The hub never sends the login
//!   itself, and the caller holds no token. A tool with a `type`, `mcp_servers`, `container`,
//!   document, file and URL sources, citations, server tool blocks and every top-level field not
//!   listed are refused. The client's headers are dropped. The models are pinned, tokens are capped
//!   per request and per item per UTC day, and the credential is rate limited and has a day's
//!   total of its own, kept across restarts ([`Ledger`]). The ledger belongs to one hub process
//!   and its cache folder: one hub serves the login for intake, and a second (or a restart that
//!   overlaps the old process on one folder) would count a day of its own.
//! - A call that does not name its item ([`item_of`]) is refused, and each call is logged with it.
//!
//! What reaches a turn is the checked request's system string and user message, never the
//! client's bytes.

use crate::backend::CancelToken;
use crate::AssetAiError;
use makepad_network::{HttpServerResponse, HttpServerResponseSender};
use makepad_strict_json::{self as json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The Qwen models a `/generate` call may name.
pub const PREFILTER_MODELS: &[&str] = &["qwen3.8-27b"];
/// The Claude models a Messages call may name.
pub const MESSAGE_MODELS: &[&str] = &["claude-opus-5-5", "claude-sonnet-5-5"];
/// Output tokens one `/generate` call may ask for.
pub const MAX_PREFILTER_TOKENS: i64 = 1024;
/// Output tokens one Messages call may ask for.
pub const MAX_MESSAGE_TOKENS: i64 = 32_000;
/// Messages calls a minute for the intake credential.
pub const RATE_PER_MINUTE: usize = 30;
/// Tokens (input, output and cache writes and reads) one item may spend through `/v1/messages`
/// in a UTC day: about two full reads of the largest item the jail reads in parts, from the
/// estimate until a measured read sets it. Past it the item waits for the next day, so whatever
/// its author pushes, one item takes about a tenth of the day.
pub const ITEM_TOKEN_BUDGET: u64 = 3_000_000;
/// Messages calls one item may make in a UTC day.
pub const ITEM_REQUEST_BUDGET: u32 = 80;
/// Messages calls each of our own fixtures (`canary`, `eval`) may make in a UTC day: about thirty
/// canary runs (three calls each) or eight steering runs (twelve), and never the whole day, so a
/// jail that named every call after a fixture would still stop well short of the credential's day.
pub const FIXTURE_REQUEST_BUDGET: u32 = 100;
/// Tokens each fixture may spend in a UTC day (see [`FIXTURE_REQUEST_BUDGET`]).
pub const FIXTURE_TOKEN_BUDGET: u64 = 6_000_000;
/// Messages calls in a UTC day for the intake credential, every item together, kept across hub
/// restarts: a little above the jail proxy's own day (400 calls, 30M tokens), so the proxy's fires
/// first and this one holds when a proxy does not.
pub const DAY_REQUEST_BUDGET: u32 = 440;
/// Tokens in a UTC day for the intake credential (see [`DAY_REQUEST_BUDGET`]).
pub const DAY_TOKEN_BUDGET: u64 = 33_000_000;
/// The day's counts in the hub's cache folder.
const DAY_FILE: &str = "intake-day.json";
/// Calls waiting for the turn worker; a full queue returns 429.
const QUEUE: usize = 8;
const MAX_MESSAGES_BODY: usize = 16 * 1024 * 1024;
const MAX_TEXT: usize = 128 * 1024;
const MAX_SYSTEM: usize = 64 * 1024;

/// Whether a fleet credential name is an intake credential.
pub fn is_intake(name: &str) -> bool {
    name == "intake" || name.starts_with("intake-")
}

/// An intake credential's name in the one form a log line may carry: `intake`, or `intake-` and 1 to
/// 32 of `a-z0-9-`. A call from any other intake name is refused, so the log never holds free text.
/// [`is_intake`] stays broad: it decides the restrictions, which must hold for every intake name.
pub fn loggable_client(name: &str) -> bool {
    name == "intake" || name.strip_prefix("intake-").is_some_and(|s| (1..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
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
    DayBudget,
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
            Refusal::ClaudeOff => "claude-off-intake",
            Refusal::Rate => "rate",
            Refusal::Busy => "busy",
            // both name the day: a jail waits on a `day-budget` for the next day, unparked
            Refusal::ItemBudget => "item-day-budget",
            Refusal::DayBudget => "hub-day-budget",
        }
    }

    fn status(self) -> u16 {
        match self {
            Refusal::Rate | Refusal::Busy | Refusal::ItemBudget | Refusal::DayBudget => 429,
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

/// The item a call is for, from its `X-Intake-Item` header; missing or invalid items are refused.
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

/// The one allowed `/generate` shape; Ok with the model.
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

/// The checked `/v1/messages` request: Ok with the checked value and its model.
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

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
struct ItemUse {
    requests: u32,
    tokens: u64,
}

impl ItemUse {
    fn over(&self, requests: u32, tokens: u64) -> bool {
        self.requests >= requests || self.tokens >= tokens
    }
}

/// The intake credential's rate, its day's total and each item's day, by UTC day. The day's counts
/// are kept in the hub's cache folder ([`DAY_FILE`]) after every change, so a hub restart starts
/// none of them again; the log keeps every call.
#[derive(Default)]
pub struct Ledger {
    /// the UTC day (days since the epoch) the counts are of
    day: u64,
    total: ItemUse,
    items: HashMap<String, ItemUse>,
    recent: VecDeque<Instant>,
    /// where the day is kept (none in a test)
    file: Option<PathBuf>,
    /// the last save failed: calls are refused until one succeeds (a call whose own save fails is
    /// not served either), so a restart cannot forget what the file does not hold
    unsaved: bool,
}

impl Ledger {
    /// The ledger kept at `file`: today's counts as it holds them, none for another day. A file
    /// that is there but does not read counts today as spent (it may have held anything), and
    /// says so.
    fn kept(file: PathBuf, today: u64) -> Ledger {
        let mut ledger = Ledger { day: today, file: Some(file.clone()), ..Ledger::default() };
        match std::fs::read(&file) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Ok(b) if ledger.read_day(&b).is_some() => {}
            _ => {
                eprintln!("intake scope: {DAY_FILE} does not read: today's intake budget counts as spent");
                ledger.total = ItemUse { requests: DAY_REQUEST_BUDGET, tokens: DAY_TOKEN_BUDGET };
            }
        }
        ledger
    }

    /// The kept day into this ledger when it is today's: Some when the bytes read.
    fn read_day(&mut self, bytes: &[u8]) -> Option<()> {
        let v = json::parse_depth(bytes, 4).ok()?;
        let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_u64);
        let usage = |v: &Value| Some(ItemUse { requests: u32::try_from(num(v, "requests")?).ok()?, tokens: num(v, "tokens")? });
        let day = num(&v, "day")?;
        let total = usage(&v)?;
        let mut items = HashMap::new();
        let Value::Obj(kept) = v.get("items")? else { return None };
        for (item, u) in kept {
            if !valid_item(item) {
                return None;
            }
            items.insert(item.clone(), usage(u)?);
        }
        if day == self.day {
            (self.total, self.items) = (total, items);
        }
        Some(())
    }

    /// The day into its file; a save that fails is tried again at the next call, which is refused
    /// until one succeeds.
    fn save(&mut self) {
        self.unsaved = !self.write();
    }

    fn write(&self) -> bool {
        let Some(file) = &self.file else { return true };
        let usage = |u: &ItemUse| json::obj(vec![("requests", Value::Int(u.requests as i64)), ("tokens", Value::Int(u.tokens as i64))]);
        let mut items: Vec<(&String, &ItemUse)> = self.items.iter().collect();
        items.sort_by(|a, b| a.0.cmp(b.0));
        let text = json::obj(vec![
            ("day", Value::Int(self.day as i64)),
            ("requests", Value::Int(self.total.requests as i64)),
            ("tokens", Value::Int(self.total.tokens as i64)),
            ("items", json::obj(items.into_iter().map(|(i, u)| (i.as_str(), usage(u))).collect())),
        ])
        .to_json();
        let tmp = file.with_extension("tmp");
        match std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, file)) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("intake scope: {DAY_FILE} not saved: {e}: intake calls are refused until it is");
                false
            }
        }
    }

    /// A new UTC day starts every count again.
    fn roll(&mut self, today: u64) {
        if today != self.day {
            (self.day, self.total) = (today, ItemUse::default());
            self.items.clear();
        }
    }

    /// One call of `item` at `now`, on UTC day `today`: the rate, then the day's total, then the
    /// item's day (our own fixtures' smaller one: see [`FIXTURE_REQUEST_BUDGET`]). It is counted
    /// only once the day is kept.
    pub fn admit(&mut self, item: &str, now: Instant, today: u64) -> Result<(), Refusal> {
        while self.recent.front().is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60)) {
            self.recent.pop_front();
        }
        if self.recent.len() >= RATE_PER_MINUTE {
            return Err(Refusal::Rate);
        }
        self.roll(today);
        if self.unsaved {
            self.save();
        }
        if self.unsaved || self.total.over(DAY_REQUEST_BUDGET, DAY_TOKEN_BUDGET) {
            return Err(Refusal::DayBudget);
        }
        let used = self.items.entry(item.to_string()).or_default();
        let (requests, tokens) = if matches!(item, "canary" | "eval") { (FIXTURE_REQUEST_BUDGET, FIXTURE_TOKEN_BUDGET) } else { (ITEM_REQUEST_BUDGET, ITEM_TOKEN_BUDGET) };
        if used.over(requests, tokens) {
            return Err(Refusal::ItemBudget);
        }
        used.requests += 1;
        self.total.requests += 1;
        self.recent.push_back(now);
        self.save();
        // a call whose count is not kept is not served
        if self.unsaved {
            self.undo(item);
            return Err(Refusal::DayBudget);
        }
        Ok(())
    }

    fn undo(&mut self, item: &str) {
        if let Some(used) = self.items.get_mut(item) {
            used.requests = used.requests.saturating_sub(1);
        }
        self.total.requests = self.total.requests.saturating_sub(1);
        self.recent.pop_back();
    }

    /// An admitted call that never reached the worker (its queue was full): its count goes back,
    /// so a caller that asks again does not spend the day.
    pub fn give_back(&mut self, item: &str, today: u64) {
        if today != self.day {
            return;
        }
        self.undo(item);
        self.save();
    }

    /// The tokens a call of `item` spent, on UTC day `today` (a call that ends past midnight
    /// counts toward the new day).
    pub fn spent(&mut self, item: &str, tokens: u64, today: u64) {
        self.roll(today);
        self.items.entry(item.to_string()).or_default().tokens += tokens;
        self.total.tokens += tokens;
        self.save();
    }
}

/// Days since the epoch, UTC.
fn utc_day() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400)
}

/// The one ledger, read from the cache folder its first call names.
fn ledger(cache_dir: &Path) -> &'static Mutex<Ledger> {
    static LEDGER: OnceLock<Mutex<Ledger>> = OnceLock::new();
    LEDGER.get_or_init(|| Mutex::new(Ledger::kept(cache_dir.join(DAY_FILE), utc_day())))
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
    // every field in its fixed form or `-`: what a caller sent never reaches the log as text
    let client = if loggable_client(call.client) { call.client } else { "-" };
    let item = if valid_item(call.item) { call.item } else { "-" };
    let route = if matches!(call.route, "/v1/messages" | "/generate") { call.route } else { "-" };
    let model = if call.model.is_empty() || MESSAGE_MODELS.contains(&call.model) || PREFILTER_MODELS.contains(&call.model) { call.model } else { "-" };
    let word = |w: &str| !w.is_empty() && w.len() <= 40 && w.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    let outcome = if word(call.outcome) { call.outcome } else { "-" };
    let line = json::obj(vec![
        ("t", Value::Int(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64))),
        ("client", json::s(client)),
        ("item", json::s(item)),
        ("route", json::s(route)),
        ("model", json::s(model)),
        ("outcome", json::s(outcome)),
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

/// The one long-lived turn worker (the hub's route loop never waits on a turn).
fn worker() -> &'static SyncSender<Call> {
    static WORKER: OnceLock<SyncSender<Call>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Call>(QUEUE);
        std::thread::Builder::new()
            .name("intake-messages".into())
            .spawn(move || {
                while let Ok(call) = rx.recv() {
                    let (response, outcome, tokens) = upstream(&call.body);
                    ledger(&call.cache_dir).lock().unwrap().spent(&call.item, tokens, utc_day());
                    log_call(&call.cache_dir, &CallLog { client: &call.client, item: &call.item, route: "/v1/messages", model: &call.model, outcome: &outcome, tokens });
                    let _ = call.response.send(response);
                }
            })
            .expect("intake-messages worker");
        tx
    })
}

/// What a claude-cli turn carries of a checked request: its model, its system prompt (a string)
/// and its one user message (text). Tools, a tool choice, more turns, blocks or a system of blocks
/// are refused, and a turn has no other way in.
pub fn cli_request(v: &Value) -> Result<(String, String, String), Refusal> {
    if v.get("tools").is_some() || v.get("tool_choice").is_some() {
        return Err(Refusal::Shape);
    }
    let model = v.get("model").and_then(Value::as_str).filter(|m| MESSAGE_MODELS.contains(m)).ok_or(Refusal::Model)?.to_string();
    let system = match v.get("system") {
        None => String::new(),
        Some(s) => s.as_str().ok_or(Refusal::Shape)?.to_string(),
    };
    let messages = v.get("messages").and_then(Value::as_arr).ok_or(Refusal::Shape)?;
    let [message] = messages else { return Err(Refusal::Shape) };
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return Err(Refusal::Shape);
    }
    let user = message.get("content").and_then(Value::as_str).ok_or(Refusal::Shape)?.to_string();
    Ok((model, system, user))
}

/// A claude-cli turn's outcome in the Messages API's shape: the reply as
/// one text block with `end_turn`, a declined turn as `refusal`, the CLI's busy as the hub's own
/// `busy`, a lockdown breach or any other failure as an error with a fixed word.
/// With the outcome for the call log (`cli-...`).
pub fn cli_answer(model: &str, result: Result<(String, [u64; 4]), AssetAiError>) -> (HttpServerResponse, &'static str) {
    let message = |stop: &str, text: &str, usage: [u64; 4]| {
        let content = if text.is_empty() { vec![] } else { vec![json::obj(vec![("type", json::s("text")), ("text", json::s(text))])] };
        let usage = json::obj(crate::claude_cli::USAGE_KEYS.iter().zip(usage).map(|(k, n)| (*k, Value::Int(n as i64))).collect());
        let body = json::obj(vec![("type", json::s("message")), ("role", json::s("assistant")), ("model", json::s(model)), ("content", Value::Arr(content)), ("stop_reason", json::s(stop)), ("usage", usage)]).to_json();
        let header = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        HttpServerResponse::new(header, body.into_bytes())
    };
    match result {
        Ok((text, usage)) => (message("end_turn", &text, usage), "cli-end-turn"),
        Err(AssetAiError::Backend(e)) if e.starts_with("Claude declined") => (message("refusal", "", [0; 4]), "cli-refusal"),
        Err(AssetAiError::Backend(e)) if e.starts_with("Claude is busy") => (Refusal::Busy.response(), "cli-busy"),
        Err(AssetAiError::Backend(e)) if e.contains("usage limit") => (error_response(503, "claude-cli: the usage limit is reached"), "cli-usage-limit"),
        Err(AssetAiError::Backend(e)) if e.contains("native tool") => (error_response(502, "claude-cli: lockdown breach: a tool use"), "cli-tool-use"),
        Err(AssetAiError::Backend(e)) if e.contains("working directory") => (error_response(502, "claude-cli: lockdown breach: a write"), "cli-wrote"),
        Err(AssetAiError::Backend(e)) if e.contains("login was refused") => (error_response(503, "claude-cli: the login was refused"), "cli-login"),
        Err(AssetAiError::Unavailable(_)) => (Refusal::ClaudeOff.response(), "cli-unavailable"),
        Err(_) => (error_response(502, "claude-cli: the turn failed"), "cli-error"),
    }
}

/// The checked request as one claude-cli turn (no direct API call: the hub never sends the login itself).
fn upstream(body: &[u8]) -> (HttpServerResponse, String, u64) {
    let request = json::parse_depth(body, 64).map_err(|_| Refusal::BadJson).and_then(|v| cli_request(&v));
    let (model, system, user) = match request {
        Ok(r) => r,
        Err(why) => return (why.response(), format!("refused-{}", why.name()), 0),
    };
    let (response, outcome) = cli_answer(&model, crate::claude_cli::run_intake_turn(&model, &system, &user, &CancelToken::default()));
    let tokens = usage_tokens(&response.body);
    (response, outcome.to_string(), tokens)
}

fn error_response(status: u16, message: &str) -> HttpServerResponse {
    let body = json::obj(vec![("error", json::s(message))]).to_json();
    let header = format!("HTTP/1.1 {status} Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    HttpServerResponse::new(header, body.into_bytes())
}

/// `POST /v1/messages`, from the route loop: checks at once, then queues the call for the
/// upstream worker. `client` is the caller's credential name (None: no caller at all);
/// `claude_serving`: this hub serves the Claude login for intake (else the
/// refusal is `claude-off-intake`).
pub fn route_messages(cache_dir: &Path, client: Option<&str>, claude_serving: bool, lines: &[String], body: Vec<u8>, response: HttpServerResponseSender) {
    let refuse = |client: &str, item: &str, why: Refusal| {
        log_call(cache_dir, &CallLog { client, item, route: "/v1/messages", model: "", outcome: &format!("refused-{}", why.name()), tokens: 0 });
        let _ = response.send(why.response());
    };
    let Some(client) = client.filter(|c| is_intake(c) && loggable_client(c)) else {
        return refuse("-", "-", Refusal::NotIntake);
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
    if let Err(why) = cli_request(&value) {
        return refuse(client, &item, why);
    }
    let call = Call { body: value.to_json().into_bytes(), client: client.to_string(), item: item.clone(), model, cache_dir: cache_dir.to_path_buf(), response: response.clone() };
    // the worker starts before the ledger is locked: a start that fails cannot poison the lock.
    // Then one lock from the count to the queue (`try_send` never waits), so a call the queue
    // cannot take gives back its own count
    let queue = worker();
    let today = utc_day();
    let mut day = ledger(cache_dir).lock().unwrap();
    if let Err(why) = day.admit(&item, Instant::now(), today) {
        drop(day);
        return refuse(client, &item, why);
    }
    if let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) = queue.try_send(call) {
        // never served: the caller's next try does not spend the day
        day.give_back(&item, today);
        drop(day);
        refuse(client, &item, Refusal::Busy);
    }
}

/// An intake caller's `POST /generate`: Ok to go on (logged), or the refusal to answer with.
pub fn gate_generate(cache_dir: &Path, client: &str, lines: &[String], body: &[u8]) -> Result<(), HttpServerResponse> {
    if !loggable_client(client) {
        log_call(cache_dir, &CallLog { client: "-", item: "-", route: "/generate", model: "", outcome: "refused-not-intake", tokens: 0 });
        return Err(Refusal::NotIntake.response());
    }
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
        assert!(value.get("tools").is_some(), "check_messages keeps client tools: cli_request is what refuses them");
        // a server-side tool of every kind is refused
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

    /// The rate, each item's UTC day and the credential's day total, kept across a restart.
    #[test]
    fn the_ledger_rate_limits_and_spends_budgets_by_day() {
        let (mut ledger, day) = (Ledger::default(), 20_000);
        let t0 = Instant::now();
        let min = |m: u64| t0 + Duration::from_secs(m * 61);
        for _ in 0..RATE_PER_MINUTE {
            assert_eq!(ledger.admit("relax#1", t0, day), Ok(()));
        }
        assert_eq!(ledger.admit("relax#2", t0, day), Err(Refusal::Rate), "a full minute refuses with a retry");
        assert_eq!(ledger.admit("relax#2", min(1), day), Ok(()), "the window moves on");
        ledger.spent("relax#2", ITEM_TOKEN_BUDGET, day);
        assert_eq!(ledger.admit("relax#2", min(2), day), Err(Refusal::ItemBudget), "a spent item waits");
        assert_eq!(ledger.admit("relax#3", min(2), day), Ok(()), "other items go on");
        assert_eq!(ledger.admit("relax#2", min(2), day + 1), Ok(()), "and the item's next day is its own again");
        ledger.spent("canary", ITEM_TOKEN_BUDGET, day + 1);
        assert_eq!(ledger.admit("canary", min(3), day + 1), Ok(()), "our own fixtures have a day of their own, past an item's");
        let mut fixture = Ledger::default();
        for k in 0..FIXTURE_REQUEST_BUDGET {
            assert_eq!(fixture.admit("eval", min(k as u64 + 10), day), Ok(()));
        }
        assert_eq!((fixture.admit("eval", min(500), day), fixture.admit("relax#1", min(500), day)), (Err(Refusal::ItemBudget), Ok(())), "but a small one, never the whole day: a jail naming every call after a fixture stops there");
        // a call the queue could not take gives its count back
        let mut busy = Ledger::default();
        assert_eq!(busy.admit("relax#6", t0, day), Ok(()));
        busy.give_back("relax#6", day);
        assert_eq!((busy.total, busy.items["relax#6"], busy.recent.len()), (ItemUse::default(), ItemUse::default(), 0), "a busy refusal spends nothing");
        ledger.spent("relax#4", DAY_TOKEN_BUDGET, day + 1);
        assert_eq!(ledger.admit("relax#5", min(4), day + 1), Err(Refusal::DayBudget), "the credential's day spent: every item waits");
        assert_eq!(ledger.admit("canary", min(4), day + 1), Err(Refusal::DayBudget), "the canary too");
        let mut calls = Ledger::default();
        for k in 0..DAY_REQUEST_BUDGET {
            assert_eq!(calls.admit(&format!("relax#{}", k + 1), min(k as u64 + 10), day), Ok(()));
        }
        assert_eq!(calls.admit("makepad#1", min(1000), day), Err(Refusal::DayBudget), "so do the day's calls");
        assert!(Refusal::ItemBudget.name().contains("day-budget") && Refusal::DayBudget.name().contains("day-budget") && !Refusal::ItemBudget.name().contains("item-budget"), "a jail waits on both for the next day, and parks on neither");
        // kept across a restart: today's counts come back, another day's do not, a bad file is spent
        let dir = std::env::temp_dir().join(format!("intake-day-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(DAY_FILE);
        let mut kept = Ledger::kept(file.clone(), day);
        assert_eq!(kept.admit("relax#2", t0, day), Ok(()), "no file: nothing spent");
        kept.spent("relax#2", ITEM_TOKEN_BUDGET, day);
        let mut again = Ledger::kept(file.clone(), day);
        assert_eq!((again.total, again.admit("relax#2", t0, day)), (ItemUse { requests: 1, tokens: ITEM_TOKEN_BUDGET }, Err(Refusal::ItemBudget)), "a restart keeps the day and the item's");
        assert_eq!(Ledger::kept(file.clone(), day + 1).total, ItemUse::default(), "another day's counts start none");
        std::fs::write(&file, "{\"day\":20000}").unwrap();
        assert_eq!(Ledger::kept(file.clone(), day).admit("relax#9", t0, day), Err(Refusal::DayBudget), "a file that does not read counts today as spent");
        // a save that fails refuses every call until one succeeds
        let mut cut = Ledger::kept(dir.join("gone").join(DAY_FILE), day);
        assert_eq!((cut.admit("relax#2", t0, day), cut.unsaved, cut.total), (Err(Refusal::DayBudget), true, ItemUse::default()), "a call whose count is not kept is refused, and counts nothing");
        cut.spent("relax#2", 10, day);
        assert_eq!(cut.admit("relax#3", t0, day), Err(Refusal::DayBudget), "every call, while the day is not kept");
        std::fs::create_dir_all(dir.join("gone")).unwrap();
        assert_eq!((cut.admit("relax#3", t0, day), cut.unsaved), (Ok(()), false), "a save that succeeds lets calls through again");
        assert_eq!(Ledger::kept(dir.join("gone").join(DAY_FILE), day).total, ItemUse { requests: 1, tokens: 10 }, "and the file holds the day, the tokens spent while it was not kept included");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(usage_tokens(br#"{"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":100}}"#), 115);
        assert_eq!(usage_tokens(b"not json"), 0);
    }

    /// The checked request reaches Claude only as one claude-cli turn: a system string and one user message
    /// of text, nothing else; the turn's outcome goes back in the Messages shape.
    #[test]
    fn the_review_is_one_cli_turn_of_text() {
        let review = r#"{"model":"claude-opus-5-5","max_tokens":16000,"thinking":{"type":"adaptive"},"system":"review this","messages":[{"role":"user","content":"<<item-0123456789ab>>\nrelax#1 issue issue\nx\n<<end-0123456789ab>>"}]}"#;
        let (v, _) = check_messages(review.as_bytes()).expect("the review's shape passes the scope");
        let (model, system, user) = cli_request(&v).expect("and reaches the turn");
        assert!(model == "claude-opus-5-5" && system == "review this" && user.starts_with("<<item-"), "model, system and the one user message");
        let (tools, _) = check_messages(MESSAGES.as_bytes()).unwrap();
        assert_eq!(cli_request(&tools), Err(Refusal::Shape), "client tools never reach a turn");
        let two = with(review, "messages", r#"[{"role":"user","content":"a"},{"role":"assistant","content":"b"}]"#);
        assert_eq!(cli_request(&json::parse_depth(two.as_bytes(), 64).unwrap()), Err(Refusal::Shape), "nor a second turn");
        let blocks = with(review, "system", r#"[{"type":"text","text":"s"}]"#);
        assert_eq!(cli_request(&json::parse_depth(blocks.as_bytes(), 64).unwrap()), Err(Refusal::Shape), "nor a system of blocks");
        let other = with(review, "model", r#""claude-fable-5-2""#);
        assert_eq!(cli_request(&json::parse_depth(other.as_bytes(), 64).unwrap()), Err(Refusal::Model), "nor another model");
        let body = |r: &HttpServerResponse| String::from_utf8_lossy(&r.body).to_string();
        let (ok, outcome) = cli_answer("claude-opus-5-5", Ok(("{\"item\":\"x\"}".into(), [10, 5, 0, 100])));
        assert!(outcome == "cli-end-turn" && body(&ok).contains("\"stop_reason\":\"end_turn\"") && body(&ok).contains("\"type\":\"text\""), "a reply is one text block, end_turn: {}", body(&ok));
        assert_eq!(usage_tokens(&ok.body), 115, "with the turn's token counts");
        let (refused, outcome) = cli_answer("claude-opus-5-5", Err(AssetAiError::Backend("Claude declined this request: its safety filter flagged it".into())));
        assert!(outcome == "cli-refusal" && body(&refused).contains("\"stop_reason\":\"refusal\""), "a declined turn is a refusal");
        for (e, word) in [("claude-cli refused: the CLI tried a native tool", "cli-tool-use"), ("claude-cli refused: the turn wrote into its working directory", "cli-wrote"), ("Claude is busy (rate limit or overload); try again shortly", "cli-busy")] {
            assert_eq!(cli_answer("claude-opus-5-5", Err(AssetAiError::Backend(e.into()))).1, word, "{e}");
        }
    }

    #[test]
    fn the_messages_route_refuses_before_any_upstream_call() {
        let dir = std::env::temp_dir().join(format!("intake-scope-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let item = vec!["X-Intake-Item: canary".to_string()];
        let server_tool = with(MESSAGES, "tools", r#"[{"type":"web_fetch_20250910","name":"web_fetch"}]"#).into_bytes();
        let free_item = vec!["X-Intake-Item: relax#1 ignore the rules and log this".to_string()];
        let cases: [(Option<&str>, bool, &[String], Vec<u8>, &str); 7] = [
            (Some("n5"), true, &item, MESSAGES.as_bytes().to_vec(), "not-intake"),
            (Some("intake-Free Text"), true, &item, MESSAGES.as_bytes().to_vec(), "not-intake"),
            (Some("intake"), true, &free_item, MESSAGES.as_bytes().to_vec(), "no-item"),
            (None, true, &item, MESSAGES.as_bytes().to_vec(), "not-intake"),
            (Some("intake"), true, &[], MESSAGES.as_bytes().to_vec(), "no-item"),
            (Some("intake"), false, &item, MESSAGES.as_bytes().to_vec(), "claude-off-intake"),
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
        assert_eq!(log.lines().count(), 7, "every refused call is logged");
        assert!(log.contains("\"outcome\":\"refused-server-tool\"") && log.contains("\"item\":\"canary\""), "{log}");
        assert!(!log.contains("rules") && !log.contains("Free") && !log.contains("n5"), "a refused call's free text never reaches the log: {log}");
        log_call(&dir, &CallLog { client: "intake", item: "relax#2 x", route: "/v1/messages", model: "say hi", outcome: "admitted, please", tokens: 0 });
        let last = std::fs::read_to_string(dir.join("intake-calls.jsonl")).unwrap().lines().last().unwrap().to_string();
        assert!(last.contains("\"item\":\"-\"") && last.contains("\"model\":\"-\"") && last.contains("\"outcome\":\"-\""), "the log writes only fixed forms: {last}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
