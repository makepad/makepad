//! One real turn, end to end: a REAL Asset Server with a scripted serving
//! lane, the shared feed's worker thread, and an app whose tool the broker
//! parks back on it.
//!
//! This is the whole "new rules" path in one test — session create over
//! `/v1/chat/sessions`, the worker channel, the event stream, a client-
//! executed tool answered on the tool-result route, and the transcript that
//! comes out the other side. No mocks inside the app path: only the GPU is
//! scripted.

use makepad_asset_chat::wire::ToolOutcome;
use makepad_asset_chat_ui::feed::{ChatFeed, ClientTools, FeedConfig};
use makepad_asset_chat_ui::transcript::{ChatData, ChatRole, CHAT};
use makepad_asset_client::json::{self, Value};
use makepad_asset_client::{ApiEndpoints, ChatProviderKind};
use makepad_asset_store::{
    AssetServer, ChatConfig, ChatScript, ScriptedLane, ScriptedTurn, ServerConfig,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn test_root(name: &str) -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("mp_chat_ui_{}_{}_{}", std::process::id(), n, name))
}

/// A serving lane that calls one app tool and then answers.
fn start_server() -> (AssetServer, String) {
    let root = test_root("root");
    let mut cfg = ServerConfig::new(root.clone());
    cfg.control_addr = "127.0.0.1:0".parse().unwrap();
    cfg.data_addr = "127.0.0.1:0".parse().unwrap();
    cfg.bootstrap_admin = true;
    cfg.log = false;
    cfg.chat = ChatConfig {
        script: Some(ChatScript {
            fleet_qwen: ScriptedLane {
                available: true,
                model: "qwen-scripted".into(),
                turns: vec![
                    ScriptedTurn::Text(
                        "On it.\n<<tool>>{\"name\":\"image.generate\",\
                         \"args\":{\"prompt\":\"a rusty trawler at dawn\",\"width\":768,\
                         \"height\":768}}"
                            .into(),
                    ),
                    ScriptedTurn::Text("Queued it.".into()),
                ],
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };
    let server = AssetServer::start(cfg).expect("server start");
    let token = std::fs::read_to_string(root.join("admin-token"))
        .expect("admin token")
        .trim()
        .to_string();
    (server, token)
}

/// The app side: whatever the broker parks lands here.
struct RecordingTools {
    calls: Sender<(String, Value)>,
}

impl ClientTools for RecordingTools {
    fn execute(&mut self, name: &str, args: &Value) -> ToolOutcome {
        let _ = self.calls.send((name.to_string(), args.clone()));
        ToolOutcome::Ok {
            value: json::obj(vec![
                ("queued", Value::Bool(true)),
                ("kind", json::s("image")),
            ]),
        }
    }

    fn call_title(&mut self, name: &str, _args: &Value) -> String {
        format!("running {name}")
    }
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let data = CHAT.read().unwrap();
    panic!(
        "timed out waiting for {what}; streaming={} messages={:?}",
        data.is_streaming,
        data.messages.iter().map(|m| (m.role, m.text.clone())).collect::<Vec<_>>()
    );
}

#[test]
fn a_turn_streams_runs_the_apps_tool_and_lands() {
    let (server, token) = start_server();
    ChatData::clear();
    let (calls_tx, calls_rx): (Sender<(String, Value)>, Receiver<(String, Value)>) =
        mpsc::channel();
    let mut cfg = FeedConfig::new(
        ApiEndpoints { control: server.control_addr(), data: server.data_addr() },
        Some(token),
        test_root("cache"),
        "gen",
        // The profile that parks the generate tools on this app.
        "gen",
    );
    cfg.provider = ChatProviderKind::FleetQwen;
    let feed = ChatFeed::start(cfg, Box::new(RecordingTools { calls: calls_tx }));

    // The app owns the user's bubble — exactly as a host does it.
    ChatData::push(ChatRole::User, "make me a trawler");
    feed.send("make me a trawler".into(), Vec::new());

    // The broker parked image.generate on us and the worker executed it.
    let (name, args) = calls_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the app's tool was called");
    assert_eq!(name, "image.generate");
    assert_eq!(args.get("width").and_then(Value::as_i64), Some(768));

    wait_for("the turn to land", || !ChatData::is_streaming());

    let data = CHAT.read().unwrap();
    let roles: Vec<ChatRole> = data.messages.iter().map(|m| m.role).collect();
    // The feed must not echo the user's message: the app pushed it, and a
    // second push put the same bubble on screen twice.
    assert_eq!(
        roles.iter().filter(|r| **r == ChatRole::User).count(),
        1,
        "one send, one user bubble: {roles:?}"
    );
    assert_eq!(roles[0], ChatRole::User, "{roles:?}");
    assert!(
        roles.contains(&ChatRole::Tool),
        "the tool call must show as a chip: {roles:?}"
    );
    assert!(
        roles.iter().rev().any(|r| *r == ChatRole::Assistant),
        "the reply must land: {roles:?}"
    );
    let tool = data
        .messages
        .iter()
        .find(|m| m.role == ChatRole::Tool)
        .expect("a chip");
    assert!(
        tool.text.contains("image.generate") || tool.text.contains("queued"),
        "the chip keeps the call: {}",
        tool.text
    );
    // The chip was completed in place with the outcome the broker echoed.
    assert!(
        tool.detail.as_deref().unwrap_or("").contains("queued"),
        "the chip's detail carries the outcome: {:?}",
        tool.detail
    );
    assert!(
        data.messages.iter().all(|m| m.role != ChatRole::System),
        "a healthy turn writes no system line: {:?}",
        data.messages.iter().map(|m| m.text.clone()).collect::<Vec<_>>()
    );
    drop(data);
    ChatData::clear();
}
