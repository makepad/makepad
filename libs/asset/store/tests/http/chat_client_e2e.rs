//! Typed asset-client chat API against a real Asset Server. Providers are
//! scripted in-process — no live OpenAI/xAI/fleet calls.

use makepad_asset_store::{AssetServer, ChatConfig, ChatScript, ScriptedLane, ScriptedTurn, ServerConfig};
use makepad_asset_client::{
    ChatProviderLocality,
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
                ..Default::default()
            },
            openai: ScriptedLane {
                available: false,
                model: String::new(),
                turns: Vec::new(),
                ..Default::default()
            },
            grok: ScriptedLane {
                available: true,
                model: "grok-scripted".into(),
                turns: vec![
                    ScriptedTurn::Text("fn spawn_turret() {}".into()),
                    ScriptedTurn::Text("Grok primary reply.".into()),
                ],
                ..Default::default()
            },
            ..Default::default()
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
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[0].kind, ChatProviderKind::FleetQwen);
    assert_eq!(rows[0].locality, ChatProviderLocality::Local);
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
    // The vendor CLIs ride the same list, always marked cloud; a scripted
    // server has none of them, and says so without leaking a path.
    for (row, kind) in rows[3..].iter().zip([
        ChatProviderKind::ClaudeCli,
        ChatProviderKind::CodexCli,
        ChatProviderKind::GrokCli,
    ]) {
        assert_eq!(row.kind, kind);
        assert_eq!(row.locality, ChatProviderLocality::Cloud);
        match &row.state {
            ChatProviderStateDto::Unavailable { reason } => assert!(!reason.contains('/'), "{reason}"),
            other => panic!("{other:?}"),
        }
    }
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
            ChatEventBodyDto::Delta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(text.contains("spawn_turret") || text.contains("Grok primary"), "{text}");
}

/// The typed client's durable-session flow, as the sandbox uses it:
/// create-or-resume by (client, game), read the transcript, Clear.
#[test]
fn typed_client_resumes_reads_and_clears_a_keyed_session() {
    let (server, token) = start();
    let client = connect(&server, &token, "keyed");
    let request = ChatCreateRequest::new("gen", ChatProviderKind::Grok)
        .with_client("game")
        .with_client_key("ip:10.0.0.9")
        .with_context_key("ast_00000000000000000000000000000009");
    let created = client.chat_create(&request).expect("create");
    assert_eq!(created.client_key.as_deref(), Some("ip:10.0.0.9"));
    assert_eq!(created.context_key.as_deref(), Some("ast_00000000000000000000000000000009"));
    assert!(client.chat_transcript(&created.session).expect("transcript").is_empty());

    client.chat_send(&created.session, &ChatSendRequest::text("hello")).expect("send");
    drain_done(&client, &created.session);
    let rows = client.chat_transcript(&created.session).expect("transcript");
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].role, makepad_asset_client::ChatTranscriptRole::User);
    assert_eq!(rows[0].text, "hello");
    assert_eq!(rows[1].role, makepad_asset_client::ChatTranscriptRole::Assistant);
    assert!(rows[1].text.contains("spawn_turret"), "{rows:?}");
    let full = client.chat_transcript_full(&created.session).expect("transcript");
    assert_eq!(full.session, created.session);
    assert_eq!(full.provider, ChatProviderKind::Grok);
    assert_eq!(full.turn, 1);
    assert!(!full.truncated);

    // Resume: the same session, transcript intact.
    let again = client.chat_create(&request).expect("resume");
    assert_eq!(again.session, created.session);
    assert_eq!(again.turn, 1);
    assert_eq!(client.chat_transcript(&created.session).expect("transcript").len(), 2);

    // Clear: gone, and the next create is fresh.
    assert!(client.chat_retire(&created.session).expect("retire"));
    let fresh = client.chat_create(&request).expect("create after clear");
    assert_ne!(fresh.session, created.session);
    assert!(client.chat_transcript(&fresh.session).expect("transcript").is_empty());

    // An unkeyed create still answers an unkeyed session.
    let plain = client
        .chat_create(&ChatCreateRequest::new("gen", ChatProviderKind::Grok))
        .expect("plain");
    assert_eq!(plain.client_key, None);
    assert_eq!(plain.context_key, None);
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
