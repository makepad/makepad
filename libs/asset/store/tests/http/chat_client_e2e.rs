//! Typed asset-client chat API against a real Asset Server. Providers are
//! scripted in-process — no live OpenAI/xAI/fleet calls.

use makepad_asset_store::{AssetServer, ChatConfig, ChatScript, ScriptedLane, ScriptedTurn, ServerConfig};
use makepad_asset_client::{
    ApiEndpoints, AssetClient, ChatCreateRequest, ChatEventBodyDto, ChatProviderKind,
    ChatProviderStateDto, ChatSendRequest, ClientConfig, ClientError,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn test_root(name: &str) -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "mp_asset_chat_client_{}_{}_{}",
        std::process::id(),
        n,
        name
    ))
}

fn start() -> (AssetServer, String) {
    let root = test_root("root");
    let mut cfg = ServerConfig::new(root.clone());
    cfg.control_addr = "127.0.0.1:0".parse().unwrap();
    cfg.data_addr = "127.0.0.1:0".parse().unwrap();
    cfg.bootstrap_admin = true;
    cfg.log = false;
    cfg.chat = ChatConfig {
        fleet: String::new(),
        fleet_bases: Vec::new(),
        max_sessions: 8,
        max_sessions_per_owner: 4,
        event_cap: 64,
        event_max_wait_ms: 2_000,
        script: Some(ChatScript {
            fleet_qwen: ScriptedLane {
                available: true,
                model: "qwen-scripted".into(),
                turns: vec![
                    ScriptedTurn::Consult {
                        task: "code".into(),
                        prompt: "spawn a turret".into(),
                        provider: "grok".into(),
                        visible: "Consulting Grok.".into(),
                    },
                    ScriptedTurn::Text("Here is the turret Grok drafted.".into()),
                ],
            },
            openai: ScriptedLane {
                available: false,
                model: String::new(),
                turns: Vec::new(),
            },
            grok: ScriptedLane {
                available: true,
                model: "grok-scripted".into(),
                turns: vec![
                    ScriptedTurn::Text("fn spawn_turret() {}".into()),
                    ScriptedTurn::Text("Grok primary reply.".into()),
                ],
            },
        }),
    };
    let server = AssetServer::start(cfg).expect("server start");
    let token = std::fs::read_to_string(root.join("admin-token"))
        .expect("admin token")
        .trim()
        .to_string();
    (server, token)
}

fn connect(server: &AssetServer, token: &str, cache: &str) -> AssetClient {
    let mut cfg = ClientConfig::new(test_root(cache));
    cfg.token = Some(token.to_string());
    AssetClient::connect(
        cfg,
        ApiEndpoints { control: server.control_addr(), data: server.data_addr() },
        Some(server.server_id()),
    )
    .expect("connect")
}

fn drain_done(
    client: &AssetClient,
    id: &makepad_asset_client::ChatSessionId,
) -> Vec<makepad_asset_client::ChatEventDto> {
    let mut after = 0;
    let mut all = Vec::new();
    for _ in 0..20 {
        let page = client.chat_events(id, after, 500, 64).expect("events");
        all.extend(page.events);
        after = page.cursor;
        if all.iter().any(|e| matches!(e.body, ChatEventBodyDto::Done)) {
            return all;
        }
    }
    panic!("turn did not finish; events={all:?}");
}

#[test]
fn typed_client_lists_providers_without_urls() {
    let (server, token) = start();
    let client = connect(&server, &token, "providers");
    let rows = client.chat_providers().expect("providers");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].kind, ChatProviderKind::FleetQwen);
    match &rows[0].state {
        ChatProviderStateDto::Available { model } => assert_eq!(model, "qwen-scripted"),
        other => panic!("{other:?}"),
    }
    match &rows[1].state {
        ChatProviderStateDto::Unavailable { reason } => {
            assert!(!reason.to_ascii_lowercase().contains("http"));
            assert!(!reason.contains("sk-"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(rows[2].kind, ChatProviderKind::Grok);
}

#[test]
fn typed_client_local_session_consults_grok() {
    let (server, token) = start();
    let client = connect(&server, &token, "consult");
    let created = client
        .chat_create(&ChatCreateRequest::new("gen", ChatProviderKind::FleetQwen))
        .expect("create");
    assert_eq!(created.provider, ChatProviderKind::FleetQwen);
    assert_eq!(created.namespace, "gen");
    let turn = client
        .chat_send(&created.session, &ChatSendRequest::text("make a turret"))
        .expect("send");
    assert_eq!(turn, 1);
    let events = drain_done(&client, &created.session);
    assert!(events.iter().any(|e| matches!(&e.body, ChatEventBodyDto::ToolCall { name, .. } if name == "llm.consult")));
    let result = events.iter().find_map(|e| match &e.body {
        ChatEventBodyDto::ToolResult { outcome, .. } => Some(outcome),
        _ => None,
    });
    match result {
        Some(makepad_asset_client::ChatToolOutcomeDto::Ok { value }) => {
            let text = value.get("text").and_then(|v| v.as_str()).unwrap_or("");
            assert!(text.contains("spawn_turret"), "{text}");
        }
        other => panic!("{other:?}"),
    }
    assert!(client.chat_retire(&created.session).expect("retire"));
    match client.chat_get(&created.session) {
        Err(ClientError::NotFound { .. }) => {}
        other => panic!("expected 404 after retire, got {other:?}"),
    }
}

#[test]
fn typed_client_can_choose_external_primary() {
    let (server, token) = start();
    let client = connect(&server, &token, "primary");
    let created = client
        .chat_create(&ChatCreateRequest::new("gen", ChatProviderKind::Grok))
        .expect("create");
    client.chat_send(&created.session, &ChatSendRequest::text("hello")).expect("send");
    let events = drain_done(&client, &created.session);
    let text: String = events
        .iter()
        .filter_map(|e| match &e.body {
            ChatEventBodyDto::Delta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(text.contains("spawn_turret") || text.contains("Grok primary"), "{text}");
}

#[test]
fn typed_client_refuses_unavailable_provider_and_empty_send() {
    let (server, token) = start();
    let client = connect(&server, &token, "refusals");
    match client.chat_create(&ChatCreateRequest::new("gen", ChatProviderKind::OpenAi)) {
        Err(ClientError::Server { status: 503, .. }) => {}
        other => panic!("expected 503, got {other:?}"),
    }
    let created = client
        .chat_create(&ChatCreateRequest::new("gen", ChatProviderKind::Grok))
        .expect("create grok");
    match client.chat_send(&created.session, &ChatSendRequest::text("")) {
        Err(ClientError::InvalidInput { what: "chat message" }) => {}
        other => panic!("expected local empty-send refusal, got {other:?}"),
    }
}
