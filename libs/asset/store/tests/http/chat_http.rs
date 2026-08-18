//! Chat broker over REAL sockets. Providers are scripted in-process — no
//! live OpenAI/xAI/fleet calls.

mod common;
use common::*;

use makepad_asset_store::json::Value;
use makepad_asset_store::{ChatConfig, ChatScript, ScriptedLane, ScriptedTurn};

fn scripted_cfg() -> impl FnOnce(&mut makepad_asset_store::ServerConfig) {
    |cfg| {
        cfg.chat = ChatConfig {
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
                            task: "level".into(),
                            prompt: "draft a quarry arena".into(),
                            provider: "grok".into(),
                            visible: "I will ask Grok to draft the level.".into(),
                        },
                        ScriptedTurn::Text("Here is the quarry arena Grok drafted.".into()),
                    ],
                },
                openai: ScriptedLane {
                    available: true,
                    model: "gpt-scripted".into(),
                    turns: vec![ScriptedTurn::Text("OpenAI primary reply.".into())],
                },
                grok: ScriptedLane {
                    available: true,
                    model: "grok-scripted".into(),
                    turns: vec![ScriptedTurn::Text("fn spawn_quarry_arena() {}".into())],
                },
            }),
        };
    }
}

fn wait_events(client: &mut Client, session: &str, after: u64) -> Vec<Value> {
    let mut last = after;
    let mut all = Vec::new();
    for _ in 0..20 {
        let r = client.get(&format!(
            "/v1/chat/sessions/{session}/events?after={last}&wait=500&limit=64"
        ));
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        let body = r.json();
        let events = body.get("events").and_then(Value::as_arr).unwrap();
        for ev in events {
            all.push(ev.clone());
            if let Some(seq) = ev.get("seq").and_then(Value::as_u64) {
                last = last.max(seq);
            }
        }
        if all.iter().any(|e| e.get("type").and_then(Value::as_str) == Some("done")) {
            return all;
        }
    }
    panic!("turn did not finish; events={all:?}");
}

#[test]
fn providers_list_is_honest_and_secretless() {
    let ts = start_server_with("chat_providers", scripted_cfg());
    let tok = ts.admin_token();
    let mut c = ts.control(Some(&tok));
    let r = c.get("/v1/chat/providers");
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let raw = String::from_utf8_lossy(&r.body).to_ascii_lowercase();
    for leak in ["sk-", "xai-", "bearer", "authorization", "api_key", "http://", "https://"] {
        assert!(!raw.contains(leak), "provider list leaked {leak}: {raw}");
    }
    let rows = r.json().get("providers").and_then(Value::as_arr).unwrap().to_vec();
    assert_eq!(rows.len(), 3);
    let kinds: Vec<&str> = rows.iter().filter_map(|r| r.get("kind").and_then(Value::as_str)).collect();
    assert_eq!(kinds, vec!["fleet-qwen", "openai", "grok"]);
    for row in &rows {
        assert_eq!(row.get("state").and_then(Value::as_str), Some("available"));
        assert!(row.get("detail").is_none());
    }
}

#[test]
fn local_session_can_consult_external_for_level() {
    let ts = start_server_with("chat_consult", scripted_cfg());
    let admin = ts.admin_token();
    let mut c = ts.control(Some(&admin));
    let player = principal_with(&mut c, &[("chat", "gen")]);
    c.set_token(Some(&player));

    let r = c.post_json(
        "/v1/chat/sessions",
        &jobj(vec![
            ("api_version", Value::Int(1)),
            ("namespace", jstr("gen")),
            ("provider", jstr("fleet-qwen")),
        ]),
    );
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.str_field("provider"), "fleet-qwen");
    let session = r.str_field("session");

    let r = c.post_json(
        &format!("/v1/chat/sessions/{session}/send"),
        &jobj(vec![("text", jstr("make a quarry arena"))]),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));

    let events = wait_events(&mut c, &session, 0);
    let types: Vec<&str> = events.iter().filter_map(|e| e.get("type").and_then(Value::as_str)).collect();
    assert!(types.contains(&"tool_call"), "{types:?}");
    assert!(types.contains(&"tool_result"), "{types:?}");
    assert!(types.contains(&"done"), "{types:?}");
    let call = events.iter().find(|e| e.get("type").and_then(Value::as_str) == Some("tool_call")).unwrap();
    assert_eq!(call.get("name").and_then(Value::as_str), Some("llm.consult"));
    let result = events.iter().find(|e| e.get("type").and_then(Value::as_str) == Some("tool_result")).unwrap();
    let outcome = result.get("result").unwrap();
    assert_eq!(outcome.get("outcome").and_then(Value::as_str), Some("ok"));
    let text = outcome.get("value").and_then(|v| v.get("text")).and_then(Value::as_str).unwrap();
    assert!(text.contains("spawn_quarry_arena"), "{text}");
    assert_eq!(
        outcome.get("value").and_then(|v| v.get("provider")).and_then(Value::as_str),
        Some("grok")
    );
}

#[test]
fn game_can_choose_external_as_primary() {
    let ts = start_server_with("chat_primary_grok", scripted_cfg());
    let admin = ts.admin_token();
    let mut c = ts.control(Some(&admin));
    let player = principal_with(&mut c, &[("chat", "gen")]);
    c.set_token(Some(&player));

    let r = c.post_json(
        "/v1/chat/sessions",
        &jobj(vec![
            ("api_version", Value::Int(1)),
            ("namespace", jstr("gen")),
            ("provider", jstr("openai")),
        ]),
    );
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let session = r.str_field("session");
    let r = c.post_json(
        &format!("/v1/chat/sessions/{session}/send"),
        &jobj(vec![("text", jstr("hello openai"))]),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let events = wait_events(&mut c, &session, 0);
    let deltas: Vec<&str> = events
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("delta"))
        .filter_map(|e| e.get("text").and_then(Value::as_str))
        .collect();
    assert!(deltas.iter().any(|t| t.contains("OpenAI primary reply")), "{deltas:?}");
}

#[test]
fn foreign_principal_cannot_see_or_drive_session() {
    let ts = start_server_with("chat_owner_scope", scripted_cfg());
    let admin = ts.admin_token();
    let mut c = ts.control(Some(&admin));
    let a = principal_with(&mut c, &[("chat", "gen")]);
    let b = principal_with(&mut c, &[("chat", "gen")]);
    c.set_token(Some(&a));
    let r = c.post_json(
        "/v1/chat/sessions",
        &jobj(vec![
            ("api_version", Value::Int(1)),
            ("namespace", jstr("gen")),
            ("provider", jstr("openai")),
        ]),
    );
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let session = r.str_field("session");
    c.set_token(Some(&b));
    let r = c.get(&format!("/v1/chat/sessions/{session}"));
    assert_eq!(r.status, 404, "{}", String::from_utf8_lossy(&r.body));
    let r = c.post_json(
        &format!("/v1/chat/sessions/{session}/send"),
        &jobj(vec![("text", jstr("hijack"))]),
    );
    assert_eq!(r.status, 404, "{}", String::from_utf8_lossy(&r.body));
}

#[test]
fn missing_chat_grant_is_denied_and_unknown_provider_is_refused() {
    let ts = start_server_with("chat_authz", scripted_cfg());
    let admin = ts.admin_token();
    let mut c = ts.control(Some(&admin));
    let reader = principal_with(&mut c, &[]);
    c.set_token(Some(&reader));
    let r = c.post_json(
        "/v1/chat/sessions",
        &jobj(vec![
            ("api_version", Value::Int(1)),
            ("namespace", jstr("gen")),
            ("provider", jstr("fleet-qwen")),
        ]),
    );
    assert_eq!(r.status, 403, "{}", String::from_utf8_lossy(&r.body));
    c.set_token(Some(&admin));
    let r = c.post_json(
        "/v1/chat/sessions",
        &jobj(vec![
            ("api_version", Value::Int(1)),
            ("namespace", jstr("gen")),
            ("provider", jstr("claude")),
        ]),
    );
    assert_eq!(r.status, 400, "{}", String::from_utf8_lossy(&r.body));
}

#[test]
fn retire_forgets_the_session() {
    let ts = start_server_with("chat_retire", scripted_cfg());
    let admin = ts.admin_token();
    let mut c = ts.control(Some(&admin));
    let r = c.post_json(
        "/v1/chat/sessions",
        &jobj(vec![
            ("api_version", Value::Int(1)),
            ("namespace", jstr("gen")),
            ("provider", jstr("openai")),
        ]),
    );
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let session = r.str_field("session");
    let r = c.request("DELETE", &format!("/v1/chat/sessions/{session}"), &[], None);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let r = c.get(&format!("/v1/chat/sessions/{session}"));
    assert_eq!(r.status, 404, "{}", String::from_utf8_lossy(&r.body));
}

#[test]
fn unauthenticated_chat_is_uniform_401() {
    let ts = start_server_with("chat_unauth", scripted_cfg());
    let mut c = ts.control(None);
    let r = c.get("/v1/chat/providers");
    assert_eq!(r.status, 401);
    let r = c.post_json(
        "/v1/chat/sessions",
        &jobj(vec![
            ("api_version", Value::Int(1)),
            ("namespace", jstr("gen")),
            ("provider", jstr("openai")),
        ]),
    );
    assert_eq!(r.status, 401);
}
