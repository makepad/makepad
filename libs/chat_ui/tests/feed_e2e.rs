//! One real turn, end to end, on the in-app session (aicore P8): a scripted
//! PROVIDER through the feed's factory seam, the worker thread, a tool the
//! `gen` profile parks on the app — executed and answered by function call —
//! and the transcript that comes out the other side. No broker anywhere:
//! that is the point.

use makepad_asset_chat::wire::ToolOutcome;
use makepad_ai_hub::chat_wire::{ProviderAvailability, ProviderKind, ServingFacts};
use makepad_ai_hub::providers::provider::{ChatProvider, ProviderEvent, TurnInput};
use makepad_chat_ui::feed::{ChatFeed, ClientTools, FeedConfig};
use makepad_chat_ui::transcript::{ChatData, ChatRole, CHAT};
use makepad_asset_client::json::{self, Value};
use makepad_asset_client::ApiEndpoints;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Scripted provider: each `begin_turn` shifts the next event script.
/// Send-safe (the feed's worker owns it across threads).
struct Scripted {
    scripts: Mutex<Vec<Vec<ProviderEvent>>>,
    pending: Mutex<Vec<ProviderEvent>>,
}

impl Scripted {
    fn new(scripts: Vec<Vec<ProviderEvent>>) -> Scripted {
        Scripted { scripts: Mutex::new(scripts), pending: Mutex::new(Vec::new()) }
    }
}

impl ChatProvider for Scripted {
    fn kind(&self) -> ProviderKind {
        ProviderKind::FleetQwen
    }
    fn availability(&mut self) -> ProviderAvailability {
        ProviderAvailability::Available { model: "scripted".into(), detail: "test".into() }
    }
    fn begin_turn(&mut self, _input: &TurnInput) -> Result<(), String> {
        let mut scripts = self.scripts.lock().unwrap();
        if scripts.is_empty() {
            return Err("script exhausted".to_string());
        }
        *self.pending.lock().unwrap() = scripts.remove(0);
        Ok(())
    }
    fn poll(&mut self) -> Vec<ProviderEvent> {
        std::mem::take(&mut *self.pending.lock().unwrap())
    }
    fn cancel(&mut self) {
        self.pending.lock().unwrap().clear();
    }
    fn continue_function(&mut self, _call_id: &str, _output: &str) -> Result<(), String> {
        let mut scripts = self.scripts.lock().unwrap();
        if scripts.is_empty() {
            return Err("script exhausted".to_string());
        }
        *self.pending.lock().unwrap() = scripts.remove(0);
        Ok(())
    }
}

/// The app under test: records the parked call, answers ok.
struct RecordingTools {
    calls: Sender<(String, Value)>,
}

impl ClientTools for RecordingTools {
    fn execute(&mut self, name: &str, args: &Value) -> ToolOutcome {
        let _ = self.calls.send((name.to_string(), args.clone()));
        ToolOutcome::Ok { value: json::obj(vec![("queued", Value::Bool(true))]) }
    }
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn serving() -> ServingFacts {
    ServingFacts {
        gen_tokens: 4,
        lanes_active: None,
        slots_total: None,
        think_tokens: None,
        visible_tokens: Some(4),
        prefix_ingested: None,
        prefix_resumed: None,
    }
}

#[test]
fn a_turn_streams_runs_the_apps_tool_and_lands() {
    let (calls_tx, calls_rx): (Sender<(String, Value)>, Receiver<(String, Value)>) =
        mpsc::channel();
    // Turn script: stream, park image.generate on the app, then finish.
    let scripts = Arc::new(Mutex::new(Some(vec![
        vec![
            ProviderEvent::Delta("Making a trawler…".to_string()),
            ProviderEvent::Serving(serving()),
            ProviderEvent::FunctionCall {
                call_id: "call_1".to_string(),
                name: "image_generate".to_string(),
                arguments: json::obj(vec![
                    ("prompt", json::s("a rusty trawler")),
                    ("width", Value::Int(768)),
                    ("height", Value::Int(512)),
                ])
                .to_json(),
            },
        ],
        vec![
            ProviderEvent::Delta("Queued the trawler image.".to_string()),
            ProviderEvent::Done { text: String::new() },
        ],
    ])));
    // Endpoints nobody answers: the executor half degrades to honest
    // "unreachable" capability text; parked tools never need it.
    let endpoints = ApiEndpoints {
        control: "127.0.0.1:1".parse().unwrap(),
        data: "127.0.0.1:1".parse().unwrap(),
    };
    let mut cfg = FeedConfig::new(
        endpoints,
        None,
        std::env::temp_dir().join(format!("mp_chat_ui_feed_{}", std::process::id())),
        "gen",
        "gen",
    );
    cfg.provider_factory = Some(Arc::new(move || {
        let scripts = scripts
            .lock()
            .unwrap()
            .take()
            .expect("one session per test");
        Box::new(Scripted::new(scripts))
    }));
    let feed = ChatFeed::start(cfg, Box::new(RecordingTools { calls: calls_tx }));

    // The app owns the user's bubble — exactly as a host does it.
    ChatData::push(ChatRole::User, "make me a trawler");
    feed.send("make me a trawler".into(), Vec::new());

    // The session parked image.generate on us and the worker executed it.
    let (name, args) = match calls_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(pair) => pair,
        Err(_) => {
            let data = CHAT.read().unwrap();
            let dump: Vec<String> = data
                .messages
                .iter()
                .map(|m| format!("{:?}: {}", m.role, m.text))
                .collect();
            panic!("tool never called; transcript: {dump:?} status={} activity={}",
                data.status, data.activity);
        }
    };
    assert_eq!(name, "image.generate");
    assert_eq!(args.get("width").and_then(Value::as_i64), Some(768));

    wait_for("the turn to land", || !ChatData::is_streaming());

    let data = CHAT.read().unwrap();
    let roles: Vec<ChatRole> = data.messages.iter().map(|m| m.role).collect();
    assert!(roles.contains(&ChatRole::User));
    assert!(
        roles.contains(&ChatRole::Assistant),
        "the streamed reply landed as an assistant bubble: {roles:?}"
    );
    let text: String = data
        .messages
        .iter()
        .map(|m| m.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("trawler"), "{text}");
}
