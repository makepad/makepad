//! Provider tests: fleet Qwen over a scripted fleet transport, and the
//! shared OpenAI/Grok Responses driver over a scripted HTTP transport.
//! No subprocess is spawned and no external API is contacted.

use makepad_asset_chat::grok::{self, GrokChatProvider};
use makepad_asset_chat::openai::{self, OpenAiChatProvider};
use makepad_asset_chat::provider::{ChatProvider, ProviderEvent, TurnInput};
use makepad_asset_chat::qwen::{FleetQwenChatProvider, FleetTransport};
use makepad_asset_chat::responses::{
    parse_responses_body, ApiKey, RawHttp, ResponsesConfig, ResponsesTransport,
    DEFAULT_GROK_TIMEOUT, MAX_RESPONSES_BODY,
};
use makepad_asset_chat::wire::{MAX_DELTA_BYTES, MAX_MESSAGE_BYTES, MAX_MESSAGES};
use makepad_asset_chat::wire::{ChatMessage, ChatRole, ProviderAvailability, ProviderKind};
use makepad_asset_client::json::{self, Value};
use makepad_network::blocking_http::CancelToken;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// -------------------------------------------------------------------- qwen

/// Scripted fleet transport: URL -> queued responses; records POST bodies.
#[derive(Default)]
struct ScriptedFleet {
    gets: RefCell<std::collections::HashMap<String, VecDeque<Result<Value, String>>>>,
    posts: Rc<RefCell<Vec<(String, Value)>>>,
    seen_gets: Rc<RefCell<Vec<String>>>,
    post_response: Option<Value>,
}

impl ScriptedFleet {
    fn on_get(&mut self, url: &str, v: Result<Value, String>) {
        self.gets.borrow_mut().entry(url.to_string()).or_default().push_back(v);
    }
}

impl FleetTransport for ScriptedFleet {
    fn get_json(&mut self, url: &str) -> Result<Value, String> {
        self.seen_gets.borrow_mut().push(url.to_string());
        let mut gets = self.gets.borrow_mut();
        let queue = gets.get_mut(url).unwrap_or_else(|| panic!("unexpected GET {url}"));
        let front = queue.pop_front().unwrap_or_else(|| panic!("script exhausted for {url}"));
        if queue.is_empty() {
            queue.push_back(front.clone());
        }
        front
    }
    fn post_json(&mut self, url: &str, body: &Value) -> Result<Value, String> {
        self.posts.borrow_mut().push((url.to_string(), body.clone()));
        Ok(self.post_response.clone().unwrap_or(Value::Null))
    }
}

fn health(caps: &[&str]) -> Value {
    json::obj(vec![(
        "capabilities",
        Value::Arr(caps.iter().map(|c| json::s(*c)).collect()),
    )])
}

fn models(rows: Vec<Value>) -> Value {
    json::obj(vec![("models", Value::Arr(rows))])
}

fn model_row(id: &str, domain: &str, available: bool, why: &str) -> Value {
    let mut pairs = vec![
        ("id", json::s(id)),
        ("domain", json::s(domain)),
        ("available", Value::Bool(available)),
    ];
    if !available {
        pairs.push(("unavailable_reason", json::s(why)));
    }
    json::obj(pairs)
}

#[test]
fn qwen_availability_is_honest_per_node() {
    let mut t = ScriptedFleet::default();
    t.on_get("http://n1:8765/health", Ok(health(&["image", "text"])));
    t.on_get(
        "http://n1:8765/models",
        Ok(models(vec![model_row("flux1-schnell", "image", true, "")])),
    );
    let mut p = FleetQwenChatProvider::new(t, vec!["http://n1:8765".into()]);
    match p.availability() {
        ProviderAvailability::Unavailable { reason } => {
            assert!(
                reason.contains("no chat capability") || reason.contains("n1:8765"),
                "{reason}"
            )
        }
        other => panic!("expected unavailable: {other:?}"),
    }

    let mut t = ScriptedFleet::default();
    t.on_get("http://n1:8765/health", Ok(health(&["chat"])));
    t.on_get(
        "http://n1:8765/models",
        Ok(models(vec![model_row("qwen3.8-27b", "chat", false, "weights downloading")])),
    );
    let mut p = FleetQwenChatProvider::new(t, vec!["http://n1:8765".into()]);
    match p.availability() {
        ProviderAvailability::Unavailable { reason } => {
            assert!(reason.contains("weights downloading"), "{reason}")
        }
        other => panic!("expected unavailable: {other:?}"),
    }

    let mut p = FleetQwenChatProvider::new(ScriptedFleet::default(), vec![]);
    assert!(!p.availability().is_available());
}

#[test]
fn qwen_prefers_qwen38_and_reports_the_model() {
    let mut t = ScriptedFleet::default();
    t.on_get("http://n1:8765/health", Ok(health(&["chat"])));
    t.on_get(
        "http://n1:8765/models",
        Ok(models(vec![
            model_row("qwen3.5-9b", "chat", true, ""),
            model_row("qwen3.8-27b", "chat", true, ""),
        ])),
    );
    let mut p = FleetQwenChatProvider::new(t, vec!["http://n1:8765".into()]);
    match p.availability() {
        ProviderAvailability::Available { model, detail } => {
            assert_eq!(model, "qwen3.8-27b");
            assert_eq!(detail, "http://n1:8765");
        }
        other => panic!("expected available: {other:?}"),
    }
    assert_eq!(p.kind(), ProviderKind::FleetQwen);
}

fn model_row_state(id: &str, domain: &str, available: bool, state: &str) -> Value {
    json::obj(vec![
        ("id", json::s(id)),
        ("domain", json::s(domain)),
        ("available", Value::Bool(available)),
        ("state", json::s(state)),
    ])
}

#[test]
fn qwen_does_not_let_later_preferred_overwrite_qwen38() {
    let mut t = ScriptedFleet::default();
    t.on_get("http://n1:8765/health", Ok(health(&["text"])));
    t.on_get(
        "http://n1:8765/models",
        Ok(models(vec![
            model_row_state("qwen3.8-27b", "text", true, "loaded"),
            model_row_state("qwen3.6-27b", "text", true, "absent"),
        ])),
    );
    let mut p = FleetQwenChatProvider::new(t, vec!["http://n1:8765".into()]);
    match p.availability() {
        ProviderAvailability::Available { model, .. } => {
            assert_eq!(model, "qwen3.8-27b");
        }
        other => panic!("expected 3.8, got {other:?}"),
    }
}

#[test]
fn qwen_poll_emits_stage_status_before_tokens() {
    let mut t = ScriptedFleet::default();
    t.on_get("http://n1:8765/health", Ok(health(&["chat"])));
    t.on_get(
        "http://n1:8765/models",
        Ok(models(vec![model_row("qwen3.8-27b", "chat", true, "")])),
    );
    t.post_response = Some(json::obj(vec![("job_id", json::s("j-2"))]));
    t.on_get(
        "http://n1:8765/job/j-2",
        Ok(json::obj(vec![
            ("state", json::s("queued")),
        ])),
    );
    t.on_get(
        "http://n1:8765/job/j-2",
        Ok(json::obj(vec![
            ("state", json::s("running")),
            ("stage", json::s("load llm gguf (17.1GB)")),
            ("progress", Value::F64(0.2)),
        ])),
    );
    t.on_get(
        "http://n1:8765/job/j-2",
        Ok(json::obj(vec![
            ("state", json::s("running")),
            ("stage", json::s("tokens 3/1024")),
            ("progress", Value::F64(0.4)),
        ])),
    );
    let mut p = FleetQwenChatProvider::new(t, vec!["http://n1:8765".into()]);
    p.begin_turn(&TurnInput::new("SYS", vec![ChatMessage::new(ChatRole::User, "hi")]))
        .unwrap();
    assert_eq!(
        p.poll(),
        vec![ProviderEvent::Status {
            note: "queued behind another GPU job".into(),
            permille: 0
        }]
    );
    assert_eq!(
        p.poll(),
        vec![ProviderEvent::Status {
            note: "loading 20%".into(),
            permille: 200
        }]
    );
    // Token ticks stay off the status line.
    assert!(p.poll().is_empty());
}

#[test]
fn qwen_ignores_cached_download_and_token_stages() {
    let mut t = ScriptedFleet::default();
    t.on_get("http://n1:8765/health", Ok(health(&["chat"])));
    t.on_get(
        "http://n1:8765/models",
        Ok(models(vec![model_row("qwen3.8-27b", "chat", true, "")])),
    );
    t.post_response = Some(json::obj(vec![("job_id", json::s("j-3"))]));
    t.on_get(
        "http://n1:8765/job/j-3",
        Ok(json::obj(vec![
            ("state", json::s("running")),
            ("stage", json::s("download")),
            ("progress", Value::F64(1.0)),
        ])),
    );
    t.on_get(
        "http://n1:8765/job/j-3",
        Ok(json::obj(vec![
            ("state", json::s("running")),
            ("stage", json::s("load")),
            ("progress", Value::F64(0.0)),
        ])),
    );
    t.on_get(
        "http://n1:8765/job/j-3",
        Ok(json::obj(vec![
            ("state", json::s("running")),
            ("stage", json::s("prefill 480 tok")),
            ("progress", Value::F64(0.02)),
        ])),
    );
    let mut p = FleetQwenChatProvider::new(t, vec!["http://n1:8765".into()]);
    p.begin_turn(&TurnInput::new("SYS", vec![ChatMessage::new(ChatRole::User, "hi")]))
        .unwrap();
    assert!(p.poll().is_empty(), "cached download 100% must stay silent");
    assert!(p.poll().is_empty(), "load 0% must stay silent");
    assert!(p.poll().is_empty(), "prefill must stay silent");
}

#[test]
fn qwen_probe_caches_and_skips_dead_nodes() {
    let mut t = ScriptedFleet::default();
    t.on_get("http://dead:8765/health", Err("timeout".into()));
    t.on_get("http://n1:8765/health", Ok(health(&["chat"])));
    t.on_get(
        "http://n1:8765/models",
        Ok(models(vec![model_row("qwen3.8-27b", "chat", true, "")])),
    );
    t.on_get(
        "http://later:8765/health",
        Err("should not be probed after a live pick".into()),
    );
    let seen = t.seen_gets.clone();
    let mut p = FleetQwenChatProvider::new(
        t,
        vec![
            "http://dead:8765".into(),
            "http://n1:8765".into(),
            "http://later:8765".into(),
        ],
    );
    assert!(p.availability().is_available());
    let first = seen.borrow().clone();
    assert_eq!(
        first,
        vec![
            // A failed idempotent GET retries once before the node is
            // marked dead (a flaky LAN drop must not cost DEAD_TTL).
            "http://dead:8765/health".to_string(),
            "http://dead:8765/health".to_string(),
            "http://n1:8765/health".to_string(),
            "http://n1:8765/models".to_string(),
        ]
    );
    // Second send must not wait on the dead box again.
    assert!(p.availability().is_available());
    assert_eq!(*seen.borrow(), first);
}

#[test]
fn qwen_turn_streams_partial_text_and_finishes() {
    let mut t = ScriptedFleet::default();
    t.on_get("http://n1:8765/health", Ok(health(&["chat"])));
    t.on_get(
        "http://n1:8765/models",
        Ok(models(vec![model_row("qwen3.8-27b", "chat", true, "")])),
    );
    t.post_response = Some(json::obj(vec![("job_id", json::s("j-1"))]));
    t.on_get(
        "http://n1:8765/job/j-1",
        Ok(json::obj(vec![("state", json::s("running")), ("partial_text", json::s("Hel"))])),
    );
    t.on_get(
        "http://n1:8765/job/j-1",
        Ok(json::obj(vec![("state", json::s("running")), ("partial_text", json::s("Hello"))])),
    );
    t.on_get(
        "http://n1:8765/job/j-1",
        Ok(json::obj(vec![("state", json::s("done")), ("partial_text", json::s("Hello!"))])),
    );
    let posts = t.posts.clone();
    let mut p = FleetQwenChatProvider::new(t, vec!["http://n1:8765".into()]);

    p.begin_turn(&TurnInput::new(
        "SYS",
        vec![
            ChatMessage::new(ChatRole::User, "hi there"),
            ChatMessage::new(ChatRole::Assistant, "prev"),
            ChatMessage::new(ChatRole::User, "again"),
        ],
    ))
    .unwrap();

    let (url, body) = posts.borrow()[0].clone();
    assert_eq!(url, "http://n1:8765/generate");
    assert_eq!(body.get("model").and_then(Value::as_str), Some("qwen3.8-27b"));
    assert_eq!(body.get("domain").and_then(Value::as_str), Some("chat"));
    assert_eq!(body.get("chat_system").and_then(Value::as_str), Some("SYS"));
    assert_eq!(body.get("prompt").and_then(Value::as_str), Some("again"));
    assert_eq!(body.get("chat_messages").and_then(Value::as_arr).unwrap().len(), 3);
    let encoded = body.to_json().to_lowercase();
    for forbidden in ["\"token\"", "secret", "api_key", "authorization", "bearer", "mpat_"] {
        assert!(!encoded.contains(forbidden), "{forbidden} in {encoded}");
    }

    let e1 = p.poll();
    assert_eq!(e1, vec![ProviderEvent::Delta("Hel".into())]);
    let e2 = p.poll();
    assert_eq!(e2, vec![ProviderEvent::Delta("lo".into())]);
    let e3 = p.poll();
    assert_eq!(
        e3,
        vec![
            ProviderEvent::Delta("!".into()),
            ProviderEvent::Done { text: "Hello!".into() }
        ]
    );
    assert!(p.poll().is_empty());
}

#[test]
fn qwen_cancel_posts_job_cancel() {
    let mut t = ScriptedFleet::default();
    t.on_get("http://n1:8765/health", Ok(health(&["chat"])));
    t.on_get(
        "http://n1:8765/models",
        Ok(models(vec![model_row("qwen3.8-27b", "chat", true, "")])),
    );
    t.post_response = Some(json::obj(vec![("job_id", json::s("j-9"))]));
    let posts = t.posts.clone();
    let mut p = FleetQwenChatProvider::new(t, vec!["http://n1:8765".into()]);
    p.begin_turn(&TurnInput::new(
        String::new(),
        vec![ChatMessage::new(ChatRole::User, "x")],
    ))
    .unwrap();
    p.cancel();
    let recorded = posts.borrow();
    assert_eq!(recorded.last().unwrap().0, "http://n1:8765/job/j-9/cancel");
    drop(recorded);
    assert!(p.poll().is_empty());
}

/// N concurrent chat sessions each own a provider, but the fleet roster is
/// a fact about the LAN — with a shared pick cache the SECOND provider
/// inherits the first one's scan instead of paying its own `/health` +
/// `/models` (and its own connect timeouts on a dark box).
#[test]
fn qwen_providers_share_one_probe_through_the_pick_cache() {
    let picks = std::sync::Arc::new(makepad_asset_chat::qwen::FleetPickCache::new());

    let mut first = ScriptedFleet::default();
    first.on_get("http://n1:8765/health", Ok(health(&["chat"])));
    first.on_get(
        "http://n1:8765/models",
        Ok(models(vec![model_row("qwen3.8-27b", "chat", true, "")])),
    );
    let seen_first = first.seen_gets.clone();
    let mut a =
        FleetQwenChatProvider::with_pick_cache(first, vec!["http://n1:8765".into()], picks.clone());
    assert!(a.availability().is_available());
    assert_eq!(seen_first.borrow().len(), 2, "the first provider scans once");

    // A transport that PANICS on any GET: the second provider must not
    // touch the network at all.
    let second = ScriptedFleet::default();
    let seen_second = second.seen_gets.clone();
    let mut b = FleetQwenChatProvider::with_pick_cache(
        second,
        vec!["http://n1:8765".into()],
        picks.clone(),
    );
    match b.availability() {
        ProviderAvailability::Available { model, .. } => assert_eq!(model, "qwen3.8-27b"),
        other => panic!("shared pick was not reused: {other:?}"),
    }
    assert!(seen_second.borrow().is_empty(), "{:?}", seen_second.borrow());

    // A private cache (the plain constructor) is unchanged: it scans.
    let mut third = ScriptedFleet::default();
    third.on_get("http://n1:8765/health", Ok(health(&["chat"])));
    third.on_get(
        "http://n1:8765/models",
        Ok(models(vec![model_row("qwen3.8-27b", "chat", true, "")])),
    );
    let seen_third = third.seen_gets.clone();
    let mut c = FleetQwenChatProvider::new(third, vec!["http://n1:8765".into()]);
    assert!(c.availability().is_available());
    assert_eq!(seen_third.borrow().len(), 2);
}

// -------------------------------------------------------------- responses

#[derive(Clone)]
struct RecordedHop {
    url: String,
    auth: String,
    body: String,
}

struct ScriptedInner {
    hops: Mutex<Vec<RecordedHop>>,
    replies: Mutex<VecDeque<Result<RawHttp, String>>>,
    hold: Mutex<bool>,
    cancel_observed: Mutex<bool>,
}

#[derive(Clone)]
struct ScriptedResponses(Arc<ScriptedInner>);

impl ScriptedResponses {
    fn new(replies: Vec<Result<RawHttp, String>>) -> Self {
        ScriptedResponses(Arc::new(ScriptedInner {
            hops: Mutex::new(Vec::new()),
            replies: Mutex::new(replies.into()),
            hold: Mutex::new(false),
            cancel_observed: Mutex::new(false),
        }))
    }

    fn hold_until_cancel(&self) {
        *self.0.hold.lock().unwrap() = true;
    }

    fn hops(&self) -> Vec<RecordedHop> {
        self.0.hops.lock().unwrap().clone()
    }

    fn cancel_was_observed(&self) -> bool {
        *self.0.cancel_observed.lock().unwrap()
    }
}

impl ResponsesTransport for ScriptedResponses {
    fn post_json(
        &self,
        url: &str,
        api_key: &str,
        body: &[u8],
        cancel: &CancelToken,
        _timeout: std::time::Duration,
    ) -> Result<RawHttp, String> {
        self.0.hops.lock().unwrap().push(RecordedHop {
            url: url.to_string(),
            auth: format!("Bearer [redacted] (len={})", api_key.len()),
            body: String::from_utf8_lossy(body).into_owned(),
        });
        if api_key.contains("secret") || api_key.starts_with("sk-") || api_key.starts_with("xai-") {
            assert!(!self.0.hops.lock().unwrap().last().unwrap().auth.contains(api_key));
        }
        if *self.0.hold.lock().unwrap() {
            while !cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
            *self.0.cancel_observed.lock().unwrap() = true;
            return Err("cancelled".to_string());
        }
        self.0
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err("script exhausted".to_string()))
    }
}

fn text_response(id: &str, text: &str) -> RawHttp {
    let body = json::obj(vec![
        ("id", json::s(id)),
        ("status", json::s("completed")),
        (
            "output",
            Value::Arr(vec![json::obj(vec![
                ("type", json::s("message")),
                ("status", json::s("completed")),
                (
                    "content",
                    Value::Arr(vec![json::obj(vec![
                        ("type", json::s("output_text")),
                        ("text", json::s(text)),
                    ])]),
                ),
            ])]),
        ),
    ])
    .to_json()
    .into_bytes();
    RawHttp { status: 200, body }
}

fn function_response(id: &str, text: &str, call_id: &str, name: &str, args: &str) -> RawHttp {
    let body = json::obj(vec![
        ("id", json::s(id)),
        ("status", json::s("completed")),
        (
            "output",
            Value::Arr(vec![
                json::obj(vec![
                    ("type", json::s("message")),
                    ("status", json::s("completed")),
                    (
                        "content",
                        Value::Arr(vec![json::obj(vec![
                            ("type", json::s("output_text")),
                            ("text", json::s(text)),
                        ])]),
                    ),
                ]),
                json::obj(vec![
                    ("type", json::s("function_call")),
                    ("status", json::s("completed")),
                    ("call_id", json::s(call_id)),
                    ("name", json::s(name)),
                    ("arguments", json::s(args)),
                ]),
            ]),
        ),
    ])
    .to_json()
    .into_bytes();
    RawHttp { status: 200, body }
}

fn skip_json_object(s: &str, start: usize) -> Option<usize> {
    let b = s.as_bytes();
    if start >= b.len() || b[start] != b'{' {
        return None;
    }
    let mut depth = 0i32;
    let mut i = start;
    let mut in_str = false;
    let mut esc = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
        } else {
            match c {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// The crate JSON parser's depth cap cannot reparse a request that embeds the
/// full tool parameter schemas. Flatten `parameters` objects so tests can
/// still inspect model/input/tools names.
fn parse_recorded_json(body: &str) -> Value {
    let mut out = String::new();
    let mut rest = body;
    while let Some(i) = rest.find("\"parameters\":") {
        out.push_str(&rest[..i]);
        out.push_str("\"parameters\":{}");
        let after = &rest[i + 13..];
        let start = after.find('{').expect("parameters object");
        let end = skip_json_object(after, start).expect("parameters end");
        rest = &after[end..];
    }
    out.push_str(rest);
    json::parse(out.as_bytes()).expect("recorded request json")
}

fn user_turn() -> TurnInput {
    TurnInput::new("SYS-NATIVE", vec![ChatMessage::new(ChatRole::User, "hello")])
}

fn wait_poll(p: &mut dyn ChatProvider) -> Vec<ProviderEvent> {
    let start = std::time::Instant::now();
    loop {
        let ev = p.poll();
        if !ev.is_empty() {
            return ev;
        }
        if start.elapsed() > Duration::from_secs(2) {
            panic!("provider poll timed out");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn openai_provider(t: ScriptedResponses) -> OpenAiChatProvider<ScriptedResponses> {
    openai::with_transport(
        ResponsesConfig::openai(ApiKey::new("sk-test-secret-key").unwrap(), openai::DEFAULT_OPENAI_MODEL),
        t,
    )
    .expect("openai transport constructor")
}

fn grok_provider(t: ScriptedResponses) -> GrokChatProvider<ScriptedResponses> {
    grok::with_transport(
        ResponsesConfig::grok(ApiKey::new("xai-test-secret-key").unwrap(), grok::DEFAULT_GROK_MODEL),
        t,
    )
    .expect("grok transport constructor")
}

fn with_isolated_env(keys: &[&str], f: impl FnOnce()) {
    static ENV_LOCK: Mutex<()> = Mutex::new(());
    let _guard = ENV_LOCK.lock().unwrap();
    let saved: Vec<(&str, Option<std::ffi::OsString>)> =
        keys.iter().map(|k| (*k, std::env::var_os(k))).collect();
    for k in keys {
        std::env::remove_var(k);
    }
    f();
    for (k, v) in saved {
        match v {
            Some(v) => std::env::set_var(k, v),
            None => std::env::remove_var(k),
        }
    }
}

#[test]
fn openai_pins_url_model_and_request_json() {
    let t = ScriptedResponses::new(vec![Ok(text_response("resp_1", "Hello"))]);
    let mut p = openai_provider(t.clone());
    assert_eq!(p.kind(), ProviderKind::OpenAi);
    match p.availability() {
        ProviderAvailability::Available { model, detail } => {
            assert_eq!(model, "gpt-5.6");
            assert_eq!(detail, "api.openai.com");
        }
        other => panic!("{other:?}"),
    }
    p.begin_turn(&user_turn()).unwrap();
    let ev = wait_poll(&mut p);
    assert_eq!(
        ev,
        vec![
            ProviderEvent::Delta("Hello".into()),
            ProviderEvent::Done { text: "Hello".into() }
        ]
    );
    let hop = &t.hops()[0];
    assert_eq!(hop.url, "https://api.openai.com/v1/responses");
    assert_eq!(hop.auth, "Bearer [redacted] (len=18)");
    assert!(!hop.body.contains("sk-test-secret-key"));
    assert!(!hop.auth.contains("sk-test"));
    let body = parse_recorded_json(&hop.body);
    assert_eq!(body.get("model").and_then(Value::as_str), Some("gpt-5.6"));
    assert_eq!(body.get("instructions").and_then(Value::as_str), Some("SYS-NATIVE"));
    assert_eq!(body.get("tool_choice").and_then(Value::as_str), Some("auto"));
    assert_eq!(body.get("parallel_tool_calls").and_then(Value::as_bool), Some(false));
    assert!(body.get("max_output_tokens").and_then(Value::as_i64).unwrap() > 0);
    assert!(body.get("previous_response_id").is_none());
    let tools = body.get("tools").and_then(Value::as_arr).unwrap();
    assert!(tools.iter().any(|t| t.get("name").and_then(Value::as_str) == Some("asset_search")));
    assert!(tools.iter().all(|t| t.get("strict").and_then(Value::as_bool) == Some(false)));
    let input = body.get("input").and_then(Value::as_arr).unwrap();
    assert_eq!(input.len(), 1);
    assert_eq!(input[0].get("role").and_then(Value::as_str), Some("user"));
    assert_eq!(input[0].get("content").and_then(Value::as_str), Some("hello"));
}

#[test]
fn consult_turn_omits_native_tools() {
    let t = ScriptedResponses::new(vec![Ok(text_response("resp_c", "fn x() {}"))]);
    let mut p = openai_provider(t.clone());
    let mut input = user_turn();
    input.tools_enabled = false;
    p.begin_turn(&input).unwrap();
    let _ = wait_poll(&mut p);
    let body = parse_recorded_json(&t.hops()[0].body);
    assert_eq!(body.get("tool_choice").and_then(Value::as_str), Some("none"));
    assert_eq!(body.get("tools").and_then(Value::as_arr).map(|a| a.len()), Some(0));
}

#[test]
fn grok_pins_url_model_and_auth_redaction() {
    let t = ScriptedResponses::new(vec![Ok(text_response("resp_g", "Hi"))]);
    let mut p = grok_provider(t.clone());
    assert_eq!(p.kind(), ProviderKind::Grok);
    match p.availability() {
        ProviderAvailability::Available { model, detail } => {
            assert_eq!(model, "grok-4.5");
            assert_eq!(detail, "api.x.ai");
        }
        other => panic!("{other:?}"),
    }
    p.begin_turn(&user_turn()).unwrap();
    let ev = wait_poll(&mut p);
    assert_eq!(
        ev,
        vec![ProviderEvent::Delta("Hi".into()), ProviderEvent::Done { text: "Hi".into() }]
    );
    let hop = &t.hops()[0];
    assert_eq!(hop.url, "https://api.x.ai/v1/responses");
    assert!(!hop.body.contains("xai-test-secret-key"));
    assert!(!hop.auth.contains("xai-test-secret-key"));
    let body = parse_recorded_json(&hop.body);
    assert_eq!(body.get("model").and_then(Value::as_str), Some("grok-4.5"));
}

#[test]
fn responses_missing_key_is_honest() {
    with_isolated_env(
        &[
            openai::OPENAI_API_KEY_ENV,
            openai::OPENAI_MODEL_ENV,
            grok::GROK_API_KEY_ENV,
        ],
        || {
            std::env::set_var(grok::GROK_API_KEY_ENV, "xai-should-not-unlock-openai");
            let t = ScriptedResponses::new(vec![]);
            let mut p = openai::with_transport(ResponsesConfig::openai_from_env(), t)
                .expect("openai config kind");
            match p.availability() {
                ProviderAvailability::Unavailable { reason } => {
                    assert!(reason.contains("OPENAI_API_KEY"), "{reason}");
                    assert!(!reason.contains("xai-should-not-unlock-openai"), "{reason}");
                }
                other => panic!("{other:?}"),
            }
            assert!(p.begin_turn(&user_turn()).is_err());
        },
    );
}

#[test]
fn responses_function_call_and_continuation() {
    let t = ScriptedResponses::new(vec![
        Ok(function_response(
            "resp_1",
            "Looking.",
            "call_abc",
            "asset_search",
            r#"{"query":"neon"}"#,
        )),
        Ok(text_response("resp_2", "Done looking.")),
    ]);
    let mut p = openai_provider(t.clone());
    p.begin_turn(&user_turn()).unwrap();
    let ev = wait_poll(&mut p);
    assert_eq!(
        ev,
        vec![
            ProviderEvent::Delta("Looking.".into()),
            ProviderEvent::FunctionCall {
                call_id: "call_abc".into(),
                name: "asset_search".into(),
                arguments: r#"{"query":"neon"}"#.into(),
            }
        ]
    );
    p.continue_function("call_abc", r#"{"outcome":"ok"}"#).unwrap();
    let ev = wait_poll(&mut p);
    assert_eq!(
        ev,
        vec![
            ProviderEvent::Delta("Done looking.".into()),
            ProviderEvent::Done { text: "Done looking.".into() }
        ]
    );
    let hops = t.hops();
    assert_eq!(hops.len(), 2);
    let cont = parse_recorded_json(&hops[1].body);
    assert_eq!(cont.get("previous_response_id").and_then(Value::as_str), Some("resp_1"));
    assert_eq!(cont.get("instructions").and_then(Value::as_str), Some("SYS-NATIVE"));
    let input = cont.get("input").and_then(Value::as_arr).unwrap();
    assert_eq!(input.len(), 1);
    assert_eq!(input[0].get("type").and_then(Value::as_str), Some("function_call_output"));
    assert_eq!(input[0].get("call_id").and_then(Value::as_str), Some("call_abc"));
    assert_eq!(input[0].get("output").and_then(Value::as_str), Some(r#"{"outcome":"ok"}"#));
}

#[test]
fn responses_wrong_call_id_is_refused() {
    let t = ScriptedResponses::new(vec![Ok(function_response(
        "resp_1",
        "",
        "call_abc",
        "asset_search",
        r#"{}"#,
    ))]);
    let mut p = openai_provider(t);
    p.begin_turn(&user_turn()).unwrap();
    let _ = wait_poll(&mut p);
    let err = p.continue_function("call_zzz", "{}").unwrap_err();
    assert!(err.contains("mismatched"), "{err}");
    assert!(!err.contains("sk-"));
}

#[test]
fn responses_unsent_tail_uses_previous_response_id() {
    let t = ScriptedResponses::new(vec![
        Ok(text_response("resp_1", "first")),
        Ok(text_response("resp_2", "second")),
    ]);
    let mut p = openai_provider(t.clone());
    p.begin_turn(&TurnInput::new(
        "S",
        vec![ChatMessage::new(ChatRole::User, "one")],
    ))
    .unwrap();
    let _ = wait_poll(&mut p);
    p.begin_turn(&TurnInput::new(
        "S",
        vec![
            ChatMessage::new(ChatRole::User, "one"),
            ChatMessage::new(ChatRole::Assistant, "first"),
            ChatMessage::new(ChatRole::User, "two"),
        ],
    ))
    .unwrap();
    let _ = wait_poll(&mut p);
    let hops = t.hops();
    let second = parse_recorded_json(&hops[1].body);
    assert_eq!(second.get("previous_response_id").and_then(Value::as_str), Some("resp_1"));
    let input = second.get("input").and_then(Value::as_arr).unwrap();
    assert_eq!(input.len(), 1);
    assert_eq!(input[0].get("content").and_then(Value::as_str), Some("two"));
}

#[test]
fn responses_cancel_idles_and_ignores_late_events() {
    let t = ScriptedResponses::new(vec![Ok(text_response("resp_late", "too late"))]);
    t.hold_until_cancel();
    let mut p = openai_provider(t);
    p.begin_turn(&user_turn()).unwrap();
    p.cancel();
    assert!(p.poll().is_empty());
    std::thread::sleep(Duration::from_millis(30));
    assert!(p.poll().is_empty());
}

#[test]
fn responses_http_error_does_not_leak_message() {
    let body = json::obj(vec![(
        "error",
        json::obj(vec![
            ("message", json::s("invalid api key sk-secret-ABC")),
            ("type", json::s("invalid_request_error")),
        ]),
    )])
    .to_json()
    .into_bytes();
    let t = ScriptedResponses::new(vec![Ok(RawHttp { status: 401, body })]);
    let mut p = openai_provider(t);
    p.begin_turn(&user_turn()).unwrap();
    let ev = wait_poll(&mut p);
    match &ev[..] {
        [ProviderEvent::Error(m)] => {
            assert!(!m.contains("sk-secret"), "{m}");
            assert!(!m.contains("invalid api key"), "{m}");
            assert!(m.contains("authentication") || m.contains("invalid request"), "{m}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn responses_oversize_body_is_rejected() {
    let huge = vec![b'x'; MAX_RESPONSES_BODY + 8];
    let t = ScriptedResponses::new(vec![Ok(RawHttp { status: 200, body: huge })]);
    let mut p = openai_provider(t);
    p.begin_turn(&user_turn()).unwrap();
    let ev = wait_poll(&mut p);
    assert!(matches!(&ev[..], [ProviderEvent::Error(m)] if m.contains("too large")));
}

#[test]
fn parse_responses_text_and_function_and_errors() {
    let ok = parse_responses_body(
        br#"{"id":"resp_1","status":"completed","output":[{"type":"message","status":"completed","content":[{"type":"output_text","text":"Hi"}]}]}"#,
    )
    .unwrap();
    assert_eq!(ok.id, "resp_1");
    assert_eq!(ok.text, "Hi");
    assert!(ok.function_call.is_none());

    let fc = parse_responses_body(
        br#"{"id":"resp_2","status":"completed","output":[{"type":"function_call","status":"completed","call_id":"call_1","name":"asset_search","arguments":"{}"}]}"#,
    )
    .unwrap();
    assert_eq!(fc.function_call.unwrap().call_id, "call_1");

    assert!(parse_responses_body(br#"{"output":[]}"#).is_err());
    assert!(parse_responses_body(br#"{"id":"","status":"completed","output":[]}"#).is_err());
    assert!(parse_responses_body(
        br#"{"id":"r","status":"completed","output":[{"type":"function_call","call_id":"a","name":"n","arguments":"{}"},{"type":"function_call","call_id":"b","name":"n","arguments":"{}"}]}"#
    )
    .is_err());
    let reasoned = parse_responses_body(
        br#"{"id":"resp_r","status":"completed","error":null,"output":[{"type":"reasoning","status":"completed","summary":[]},{"type":"message","status":"completed","content":[{"type":"output_text","text":"Hi"}]}]}"#,
    )
    .unwrap();
    assert_eq!(reasoned.text, "Hi");
    assert!(parse_responses_body(
        br#"{"id":"r","status":"completed","output":[{"type":"web_search_call"}]}"#
    )
    .is_err());
    assert!(parse_responses_body(
        br#"{"id":"r","status":"completed","output":[{"type":"message","content":[{"type":"image"}]}]}"#
    )
    .is_err());
    let err = parse_responses_body(
        br#"{"error":{"message":"nope sk-secret-ABC","type":"invalid_request_error"}}"#,
    )
    .unwrap_err();
    assert_eq!(err, "api error: invalid request");
    assert!(!err.contains("sk-secret"));
    assert!(!err.contains("nope"));
    assert!(parse_responses_body(
        br#"{"id":"r","output":[{"type":"message","content":[{"type":"output_text","text":"Hi"}]}]}"#
    )
    .is_err());
}

#[test]
fn parse_responses_rejects_oversize_and_keeps_errors_clean() {
    let big = vec![b'a'; MAX_RESPONSES_BODY + 1];
    assert!(parse_responses_body(&big).unwrap_err().contains("too large"));
    let leaky = parse_responses_body(
        br#"{"error":{"message":"bad key Authorization: Bearer sk-leak","type":"authentication_error"}}"#,
    )
    .unwrap_err();
    assert_eq!(leaky, "api error: authentication");
    assert!(!leaky.contains("sk-leak"));
    assert!(!leaky.contains("Authorization"));
    assert!(!leaky.contains("bad key"));
}

#[test]
fn parse_error_null_and_completed_status_succeed() {
    let v = parse_responses_body(
        br#"{"id":"resp_ok","status":"completed","error":null,"output":[{"type":"output_text","text":"ok"}]}"#,
    )
    .unwrap();
    assert_eq!(v.text, "ok");
}

#[test]
fn parse_completed_accepts_null_incomplete_details() {
    // Realistic successful Responses body: status completed, both
    // error and incomplete_details serialized as JSON null.
    let v = parse_responses_body(
        br#"{
            "id":"resp_01completednull",
            "object":"response",
            "created_at":1710000000,
            "status":"completed",
            "error":null,
            "incomplete_details":null,
            "model":"gpt-5.6",
            "output":[{
                "type":"message",
                "id":"msg_1",
                "status":"completed",
                "role":"assistant",
                "content":[{"type":"output_text","text":"hello from fixture"}]
            }]
        }"#,
    )
    .unwrap();
    assert_eq!(v.id, "resp_01completednull");
    assert_eq!(v.text, "hello from fixture");
    assert!(v.function_call.is_none());
}

#[test]
fn parse_incomplete_without_output_fails_with_details() {
    let err = parse_responses_body(
        br#"{"id":"resp_i","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[]}"#,
    )
    .unwrap_err();
    assert!(err.contains("incomplete"), "{err}");
    assert!(!err.contains("max_output_tokens") || err == "response incomplete: output limit");
    assert_eq!(err, "response incomplete: output limit");
}

#[test]
fn parse_incomplete_with_function_call_is_rejected() {
    let err = parse_responses_body(
        br#"{"id":"resp_i","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[{"type":"function_call","call_id":"call_1","name":"asset_search","arguments":"{}"}]}"#,
    )
    .unwrap_err();
    assert_eq!(err, "response incomplete: output limit");
}

#[test]
fn parse_output_text_requires_string_text() {
    assert!(parse_responses_body(
        br#"{"id":"r","status":"completed","output":[{"type":"output_text"}]}"#,
    )
    .is_err());
    assert!(parse_responses_body(
        br#"{"id":"r","status":"completed","output":[{"type":"output_text","text":1}]}"#,
    )
    .is_err());
    assert!(parse_responses_body(
        br#"{"id":"r","status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":false}]}]}"#,
    )
    .is_err());
}

#[test]
fn parse_incomplete_details_reason_must_be_string_when_present() {
    let err = parse_responses_body(
        br#"{"id":"r","status":"incomplete","incomplete_details":{"reason":1},"output":[]}"#,
    )
    .unwrap_err();
    assert!(err.contains("reason"), "{err}");
}

#[test]
fn begin_turn_rejects_oversize_history_before_encoding() {
    let t = ScriptedResponses::new(vec![]);
    let mut p = openai_provider(t);
    let too_many = vec![ChatMessage::new(ChatRole::User, "x"); MAX_MESSAGES + 1];
    let err = p
        .begin_turn(&TurnInput::new("S", too_many))
        .unwrap_err();
    assert!(err.contains("too many"), "{err}");
    let huge = vec![ChatMessage::new(ChatRole::User, "x".repeat(MAX_MESSAGE_BYTES + 1))];
    let err = p
        .begin_turn(&TurnInput::new("S", huge))
        .unwrap_err();
    assert!(err.contains("message"), "{err}");
}

#[test]
fn parse_item_status_incomplete_is_rejected() {
    let err = parse_responses_body(
        br#"{"id":"resp_i","status":"completed","output":[{"type":"function_call","status":"incomplete","call_id":"call_1","name":"asset_search","arguments":"{}"}]}"#,
    )
    .unwrap_err();
    assert_eq!(err, "output item is not completed");
}

#[test]
fn parse_safety_refusal_is_bounded_done() {
    let v = parse_responses_body(
        br#"{"id":"resp_s","status":"incomplete","incomplete_details":{"reason":"content_filter"},"output":[{"type":"message","status":"incomplete","content":[{"type":"refusal","refusal":"I cannot help with sk-secret-ABC"}]}]}"#,
    )
    .unwrap();
    assert_eq!(v.id, "resp_s");
    assert_eq!(v.text, "The request was refused by the model's safety policy.");
    assert!(v.function_call.is_none());
    assert!(!v.text.contains("sk-secret"));
}

#[test]
fn responses_delta_chunks_utf8_safely() {
    let big = "é".repeat((MAX_DELTA_BYTES / 2) + 8);
    let t = ScriptedResponses::new(vec![Ok(text_response("resp_big", &big))]);
    let mut p = openai_provider(t);
    p.begin_turn(&user_turn()).unwrap();
    let ev = wait_poll(&mut p);
    let deltas: Vec<&str> = ev
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Delta(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    assert!(deltas.len() >= 2, "expected chunked deltas, got {deltas:?}");
    assert!(deltas.iter().all(|d| d.len() <= MAX_DELTA_BYTES));
    assert_eq!(deltas.concat(), big);
    assert!(matches!(ev.last(), Some(ProviderEvent::Done { .. })));
}

#[test]
fn grok_timeout_is_raised_and_bounded() {
    let cfg = ResponsesConfig::grok(
        ApiKey::new("xai-test-secret-key").unwrap(),
        grok::DEFAULT_GROK_MODEL,
    );
    assert!(cfg.request_timeout() >= DEFAULT_GROK_TIMEOUT);
    assert!(cfg.request_timeout() <= makepad_asset_chat::responses::MAX_GROK_TIMEOUT);
    let short = cfg.with_request_timeout(std::time::Duration::from_secs(1));
    assert!(short.request_timeout() >= std::time::Duration::from_secs(5));
}

#[test]
fn continue_error_does_not_replay_tool_result_as_user_text() {
    let t = ScriptedResponses::new(vec![
        Ok(function_response(
            "resp_1",
            "Looking.",
            "call_abc",
            "asset_search",
            r#"{"query":"neon"}"#,
        )),
        Err("upstream failed".into()),
        Ok(text_response("resp_2", "fresh")),
    ]);
    let mut p = openai_provider(t.clone());
    p.begin_turn(&user_turn()).unwrap();
    let ev = wait_poll(&mut p);
    assert!(matches!(ev.last(), Some(ProviderEvent::FunctionCall { .. })));
    p.continue_function("call_abc", r#"{"outcome":"ok","value":{}}"#).unwrap();
    let ev = wait_poll(&mut p);
    assert!(matches!(&ev[..], [ProviderEvent::Error(m)] if m.contains("upstream")));
    let err = p
        .begin_turn(&TurnInput::new(
            "SYS-NATIVE",
            vec![
                ChatMessage::new(ChatRole::User, "hello"),
                ChatMessage::new(ChatRole::Assistant, "Looking."),
                ChatMessage::new(ChatRole::Tool, r#"{"outcome":"ok","value":{}}"#),
                ChatMessage::new(ChatRole::User, "try again"),
            ],
        ))
        .unwrap_err();
    assert!(err.contains("unresolved"), "{err}");
    assert_eq!(t.hops().len(), 2);
}

#[test]
fn drop_cancels_in_flight_request() {
    let t = ScriptedResponses::new(vec![Ok(text_response("resp_late", "too late"))]);
    t.hold_until_cancel();
    {
        let mut p = openai_provider(t.clone());
        p.begin_turn(&user_turn()).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        drop(p);
    }
    let started = std::time::Instant::now();
    while !t.cancel_was_observed() {
        if started.elapsed() > Duration::from_secs(2) {
            panic!("drop did not cancel the in-flight request token");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn cancel_while_idle_retains_previous_response_chain() {
    let t = ScriptedResponses::new(vec![
        Ok(text_response("resp_1", "first")),
        Ok(text_response("resp_2", "second")),
    ]);
    let mut p = openai_provider(t.clone());
    p.begin_turn(&TurnInput::new(
        "S",
        vec![ChatMessage::new(ChatRole::User, "one")],
    ))
    .unwrap();
    let _ = wait_poll(&mut p);
    p.cancel();
    p.begin_turn(&TurnInput::new(
        "S",
        vec![
            ChatMessage::new(ChatRole::User, "one"),
            ChatMessage::new(ChatRole::Assistant, "first"),
            ChatMessage::new(ChatRole::User, "two"),
        ],
    ))
    .unwrap();
    let _ = wait_poll(&mut p);
    let hops = t.hops();
    let second = parse_recorded_json(&hops[1].body);
    assert_eq!(second.get("previous_response_id").and_then(Value::as_str), Some("resp_1"));
}

#[test]
fn openai_transport_rejects_grok_config() {
    let t = ScriptedResponses::new(vec![]);
    let cfg = ResponsesConfig::grok(ApiKey::new("xai-test-secret-key").unwrap(), grok::DEFAULT_GROK_MODEL);
    let err = openai::with_transport(cfg, t).err().expect("kind mismatch");
    assert!(err.contains("cannot accept"), "{err}");
}

#[test]
fn grok_transport_rejects_openai_config() {
    let t = ScriptedResponses::new(vec![]);
    let cfg =
        ResponsesConfig::openai(ApiKey::new("sk-test-secret-key").unwrap(), openai::DEFAULT_OPENAI_MODEL);
    let err = grok::with_transport(cfg, t).err().expect("kind mismatch");
    assert!(err.contains("cannot accept"), "{err}");
}

#[test]
fn production_constructors_pin_origin() {
    let o = ResponsesConfig::openai(ApiKey::new("sk-test-secret-key").unwrap(), "gpt-5.6");
    assert_eq!(o.kind(), ProviderKind::OpenAi);
    assert_eq!(o.endpoint(), openai::OPENAI_RESPONSES_URL);
    let g = ResponsesConfig::grok(ApiKey::new("xai-test-secret-key").unwrap(), "grok-4.5");
    assert_eq!(g.kind(), ProviderKind::Grok);
    assert_eq!(g.endpoint(), grok::GROK_RESPONSES_URL);
}
