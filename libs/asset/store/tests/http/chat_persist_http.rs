//! KEYED (durable) chat sessions over real sockets: create-or-resume by
//! `(principal, client_key, context_key)`, the transcript route, Clear,
//! game retire, eviction under cap pressure, and a server restart.
//! Providers are scripted in-process — no live fleet/vendor calls.

mod common;
use common::*;

use makepad_asset_store::json::Value;
use makepad_asset_store::{AssetServer, ChatConfig, ChatScript, ScriptedLane, ScriptedTurn, ServerConfig};
use std::path::{Path, PathBuf};

/// A level-building script: one catalog query (broker-executed), one
/// parked world call (client-executed), the report, and a second-turn
/// reply. Every session's provider starts this script from the top.
const LEVEL_TURNS: [&str; 4] = [
    "<<tool>>{\"name\":\"assets.query\",\"args\":{\"sql\":\"SELECT COUNT(*) AS n FROM search_annotations WHERE live=1\"}}",
    "Building it.\n<<tool>>{\"name\":\"world.set_source\",\"args\":{\"source\":\"game.sky({})\"}}",
    "The level is live.",
    "Welcome back.",
];

const PLAIN_TURNS: [&str; 3] = ["hi there", "hi again", "hi thrice"];

fn apply_cfg(cfg: &mut ServerConfig, turns: &[&str], per_owner: usize) {
    cfg.chat = ChatConfig {
        fleet: String::new(),
        fleet_bases: Vec::new(),
        max_sessions: 16,
        max_sessions_per_owner: per_owner,
        event_cap: 64,
        event_max_wait_ms: 2_000,
        script: Some(ChatScript {
            fleet_qwen: ScriptedLane {
                available: true,
                model: "qwen-scripted".into(),
                turns: turns.iter().map(|t| ScriptedTurn::Text((*t).into())).collect(),
                ..Default::default()
            },
            ..Default::default()
        }),
    };
}

fn create(c: &mut Client, client_key: Option<&str>, context_key: Option<&str>) -> Response {
    let mut pairs = vec![
        ("api_version", Value::Int(1)),
        ("namespace", jstr("gen")),
        ("provider", jstr("fleet-qwen")),
        ("client", jstr("game")),
    ];
    if let Some(k) = client_key {
        pairs.push(("client_key", jstr(k)));
    }
    if let Some(k) = context_key {
        pairs.push(("context_key", jstr(k)));
    }
    c.post_json("/v1/chat/sessions", &jobj(pairs))
}

fn send(c: &mut Client, session: &str, text: &str) {
    let r = c.post_json(
        &format!("/v1/chat/sessions/{session}/send"),
        &jobj(vec![("text", jstr(text))]),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
}

fn ev_type(e: &Value) -> Option<&str> {
    e.get("type").and_then(Value::as_str)
}

/// Poll events until `pred` matches one (a parked turn never reaches
/// `done`, so the predicate is the caller's).
fn wait_until(c: &mut Client, session: &str, pred: impl Fn(&Value) -> bool) -> Vec<Value> {
    let mut last = 0u64;
    let mut all: Vec<Value> = Vec::new();
    for _ in 0..40 {
        let r = c.get(&format!("/v1/chat/sessions/{session}/events?after={last}&wait=500&limit=64"));
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        for ev in r.json().get("events").and_then(Value::as_arr).unwrap() {
            all.push(ev.clone());
            if let Some(seq) = ev.get("seq").and_then(Value::as_u64) {
                last = last.max(seq);
            }
        }
        if all.iter().any(&pred) {
            return all;
        }
    }
    panic!("event never arrived; events={all:?}");
}

fn wait_done(c: &mut Client, session: &str) -> Vec<Value> {
    wait_until(c, session, |e| ev_type(e) == Some("done"))
}

/// Drive one LEVEL_TURNS turn to completion: send, answer the parked
/// world call as the game would, wait for done.
fn run_level_turn(c: &mut Client, session: &str, text: &str) {
    send(c, session, text);
    let events = wait_until(c, session, |e| {
        ev_type(e) == Some("tool_call") && e.get("name").and_then(Value::as_str) == Some("world.set_source")
    });
    let call_id = events
        .iter()
        .rev()
        .find(|e| ev_type(e) == Some("tool_call") && e.get("name").and_then(Value::as_str) == Some("world.set_source"))
        .and_then(|e| e.get("id"))
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    let r = c.post_json(
        &format!("/v1/chat/sessions/{session}/tool-result"),
        &jobj(vec![
            ("id", jstr(call_id)),
            ("outcome", jobj(vec![("outcome", jstr("ok")), ("value", jobj(vec![("eval", jstr("ok"))]))])),
        ]),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    wait_done(c, session);
}

fn transcript(c: &mut Client, session: &str) -> Value {
    let r = c.get(&format!("/v1/chat/sessions/{session}/transcript"));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    r.json()
}

/// `(role, text)` per row.
fn rows(t: &Value) -> Vec<(String, String)> {
    t.get("messages")
        .and_then(Value::as_arr)
        .unwrap()
        .iter()
        .map(|m| {
            (
                m.get("role").and_then(Value::as_str).unwrap().to_string(),
                m.get("text").and_then(Value::as_str).unwrap().to_string(),
            )
        })
        .collect()
}

fn level_rows(user_text: &str) -> Vec<(String, String)> {
    [
        ("user", user_text),
        ("tool", "assets.query · ok"),
        ("assistant", "Building it."),
        ("tool", "world.set_source · ok"),
        ("assistant", "The level is live."),
    ]
    .iter()
    .map(|(r, t)| (r.to_string(), t.to_string()))
    .collect()
}

fn encode(key: &str) -> String {
    key.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// Where the broker keeps this conversation on disk.
fn file_path(root: &Path, owner: &str, client_key: &str, context_key: &str) -> PathBuf {
    root.join("chat").join(owner).join(encode(client_key)).join(format!("{}.jsonl", encode(context_key)))
}

const CLIENT: &str = "ip:10.0.0.7";
const GAME: &str = "ast_00000000000000000000000000000001";
const GAME2: &str = "ast_00000000000000000000000000000002";

#[test]
fn create_or_resume_returns_the_same_session_and_keys_scope_it() {
    let ts = start_server_with("chat_keyed_resume", |cfg| apply_cfg(cfg, &PLAIN_TURNS, 4));
    let admin = ts.admin_token();
    let mut c = ts.control(Some(&admin));
    let player = principal_with(&mut c, &[("chat", "gen")]);
    c.set_token(Some(&player));

    // First time: created, keys echoed.
    let r = create(&mut c, Some(CLIENT), Some(GAME));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.str_field("client_key"), CLIENT);
    assert_eq!(r.str_field("context_key"), GAME);
    let a = r.str_field("session");
    let owner = r.str_field("owner");
    assert!(file_path(&ts.root, &owner, CLIENT, GAME).exists(), "keyed = on disk from the start");

    // Same keys: the same session comes back (200, same id, keys echoed).
    let r = create(&mut c, Some(CLIENT), Some(GAME));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.str_field("session"), a);
    assert_eq!(r.str_field("state"), "idle");
    assert_eq!(r.str_field("client_key"), CLIENT);

    // Another game for the same client: another conversation.
    let r = create(&mut c, Some(CLIENT), Some(GAME2));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    assert_ne!(r.str_field("session"), a);

    // The same keys under ANOTHER principal: never the same session.
    c.set_token(Some(&admin));
    let other = principal_with(&mut c, &[("chat", "gen")]);
    c.set_token(Some(&other));
    let r = create(&mut c, Some(CLIENT), Some(GAME));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    assert_ne!(r.str_field("session"), a);
    c.set_token(Some(&player));

    // Half a key, a malformed key, or an unkeyed session: no resume.
    assert_eq!(create(&mut c, Some(CLIENT), None).status, 400);
    assert_eq!(create(&mut c, None, Some(GAME)).status, 400);
    assert_eq!(create(&mut c, Some("a b"), Some(GAME)).status, 400);
    assert_eq!(create(&mut c, Some("../x"), Some(GAME)).status, 400);
    let r = create(&mut c, None, None);
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    assert!(r.json().get("client_key").is_none());
    assert!(r.json().get("context_key").is_none());

    // The list carries the keys, so an observer can tell games apart.
    let r = c.get("/v1/chat/sessions");
    assert_eq!(r.status, 200);
    let body = r.json();
    let list = body.get("sessions").and_then(Value::as_arr).unwrap();
    assert_eq!(list.len(), 3, "{list:?}");
    let keyed: Vec<&str> = list.iter().filter_map(|v| v.get("context_key").and_then(Value::as_str)).collect();
    assert!(keyed.contains(&GAME) && keyed.contains(&GAME2), "{keyed:?}");
}

#[test]
fn transcript_renders_the_conversation_as_rows() {
    let ts = start_server_with("chat_transcript", |cfg| apply_cfg(cfg, &LEVEL_TURNS, 4));
    let admin = ts.admin_token();
    let mut c = ts.control(Some(&admin));
    let player = principal_with(&mut c, &[("chat", "gen")]);
    c.set_token(Some(&player));
    let r = create(&mut c, Some(CLIENT), Some(GAME));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let session = r.str_field("session");

    // Empty before the first turn.
    let t = transcript(&mut c, &session);
    assert!(rows(&t).is_empty());
    assert_eq!(t.get("turn").and_then(Value::as_u64), Some(0));

    run_level_turn(&mut c, &session, "build me a level");
    let t = transcript(&mut c, &session);
    assert_eq!(t.get("session").and_then(Value::as_str), Some(session.as_str()));
    assert_eq!(t.get("provider").and_then(Value::as_str), Some("fleet-qwen"));
    assert_eq!(t.get("turn").and_then(Value::as_u64), Some(1));
    assert_eq!(t.get("truncated").and_then(Value::as_bool), Some(false));
    assert_eq!(rows(&t), level_rows("build me a level"));
    // Tool rows carry their parts; text rows carry none.
    let msgs = t.get("messages").and_then(Value::as_arr).unwrap();
    assert_eq!(msgs[1].get("tool").and_then(Value::as_str), Some("assets.query"));
    assert_eq!(msgs[1].get("outcome").and_then(Value::as_str), Some("ok"));
    assert!(msgs[0].get("tool").is_none() && msgs[2].get("outcome").is_none());
    // No prompt plumbing leaks: not the tool reminder, not the trained call.
    let raw = String::from_utf8_lossy(&c.get(&format!("/v1/chat/sessions/{session}/transcript")).body).to_string();
    assert!(!raw.contains("tools are live"), "{raw}");
    assert!(!raw.contains("<tool_call>"), "{raw}");

    // Unkeyed sessions have a transcript too (in memory only).
    let r = create(&mut c, None, None);
    let plain = r.str_field("session");
    assert!(rows(&transcript(&mut c, &plain)).is_empty());

    // Foreign principals see nothing; a malformed id is a 400.
    c.set_token(Some(&admin));
    assert_eq!(c.get(&format!("/v1/chat/sessions/{session}/transcript")).status, 404);
    assert_eq!(c.get("/v1/chat/sessions/nope/transcript").status, 400);
}

#[test]
fn clear_wipes_the_persisted_transcript() {
    let ts = start_server_with("chat_clear", |cfg| apply_cfg(cfg, &PLAIN_TURNS, 4));
    let admin = ts.admin_token();
    let mut c = ts.control(Some(&admin));
    let player = principal_with(&mut c, &[("chat", "gen")]);
    c.set_token(Some(&player));
    let r = create(&mut c, Some(CLIENT), Some(GAME));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let a = r.str_field("session");
    let owner = r.str_field("owner");
    send(&mut c, &a, "hello");
    wait_done(&mut c, &a);
    let path = file_path(&ts.root, &owner, CLIENT, GAME);
    let bytes = std::fs::read(&path).expect("transcript file");
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("\"hello\"") && text.contains("hi there"), "{text}");
    assert_eq!(rows(&transcript(&mut c, &a)), vec![("user".to_string(), "hello".to_string()), ("assistant".to_string(), "hi there".to_string())]);

    // Clear: the session and its file are gone, synchronously.
    let r = c.delete(&format!("/v1/chat/sessions/{a}"));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json().get("retired").and_then(Value::as_bool), Some(true));
    assert!(!path.exists(), "Clear must delete the transcript");
    assert_eq!(c.get(&format!("/v1/chat/sessions/{a}")).status, 404);

    // The next create-or-resume is a FRESH conversation.
    let r = create(&mut c, Some(CLIENT), Some(GAME));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let b = r.str_field("session");
    assert_ne!(a, b);
    assert!(rows(&transcript(&mut c, &b)).is_empty());
    // And the late shutdown of the cleared worker never resurrected it.
    std::thread::sleep(std::time::Duration::from_millis(300));
    let t = transcript(&mut c, &b);
    assert!(rows(&t).is_empty());
    let after = std::fs::read(&path).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
    assert!(!after.contains("hello"), "{after}");
}

#[test]
fn retiring_the_game_drops_every_conversation_about_it() {
    let ts = start_server_with("chat_game_retire", |cfg| apply_cfg(cfg, &PLAIN_TURNS, 8));
    let admin = ts.admin_token();
    let mut control = ts.control(Some(&admin));
    let mut data = ts.data(Some(&admin));
    let (asset_id, _rev) = publish_prop_http(&mut control, &mut data, "gen", "gen/game-level", b"GLB-1", b"PNG-1");

    // Two players talk about this game; one of them also about another.
    let p1 = principal_with(&mut control, &[("chat", "gen")]);
    let p2 = principal_with(&mut control, &[("chat", "gen")]);
    let mut c1 = ts.control(Some(&p1));
    let mut c2 = ts.control(Some(&p2));
    let r = create(&mut c1, Some("ip:10.0.0.1"), Some(&asset_id));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let s1 = r.str_field("session");
    let o1 = r.str_field("owner");
    send(&mut c1, &s1, "hello");
    wait_done(&mut c1, &s1);
    let r = create(&mut c2, Some("ip:10.0.0.2"), Some(&asset_id));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let s2 = r.str_field("session");
    let o2 = r.str_field("owner");
    let r = create(&mut c1, Some("ip:10.0.0.1"), Some(GAME2));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let s_other = r.str_field("session");
    // A conversation that is only ON DISK (its worker retired without a
    // Clear is what a restart or eviction leaves) goes too: fake one by
    // planting a file for a third player.
    let planted = file_path(&ts.root, "prin_000000000000000000000000000000ab", "ip:10.0.0.3", &asset_id);
    std::fs::create_dir_all(planted.parent().unwrap()).unwrap();
    std::fs::write(&planted, "{\"k\":\"h\",\"v\":1}\n").unwrap();
    for p in [
        file_path(&ts.root, &o1, "ip:10.0.0.1", &asset_id),
        file_path(&ts.root, &o2, "ip:10.0.0.2", &asset_id),
        file_path(&ts.root, &o1, "ip:10.0.0.1", GAME2),
    ] {
        assert!(p.exists(), "{p:?}");
    }

    // Retire the game asset.
    let r = control.delete(&format!("/v1/assets/{asset_id}"));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));

    assert_eq!(c1.get(&format!("/v1/chat/sessions/{s1}")).status, 404);
    assert_eq!(c2.get(&format!("/v1/chat/sessions/{s2}")).status, 404);
    assert!(!file_path(&ts.root, &o1, "ip:10.0.0.1", &asset_id).exists());
    assert!(!file_path(&ts.root, &o2, "ip:10.0.0.2", &asset_id).exists());
    assert!(!planted.exists());
    // The other game's conversation is untouched.
    assert_eq!(c1.get(&format!("/v1/chat/sessions/{s_other}")).status, 200);
    assert!(file_path(&ts.root, &o1, "ip:10.0.0.1", GAME2).exists());
    // Resuming the retired game's chat starts from nothing.
    let r = create(&mut c1, Some("ip:10.0.0.1"), Some(&asset_id));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    assert_ne!(r.str_field("session"), s1);
}

#[test]
fn a_keyed_session_survives_a_server_restart() {
    let root = test_root("chat_restart");
    let start = || {
        let mut cfg = base_config(root.clone());
        apply_cfg(&mut cfg, &LEVEL_TURNS, 4);
        TestServer { server: AssetServer::start(cfg).expect("server start"), root: root.clone() }
    };
    let ts = start();
    let admin = ts.admin_token();
    let mut c = ts.control(Some(&admin));
    let player = principal_with(&mut c, &[("chat", "gen")]);
    c.set_token(Some(&player));
    let r = create(&mut c, Some(CLIENT), Some(GAME));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let a = r.str_field("session");
    run_level_turn(&mut c, &a, "build me a level");
    assert_eq!(rows(&transcript(&mut c, &a)), level_rows("build me a level"));
    drop(c);
    drop(ts);

    // Same root, new process life: the conversation is where the player
    // left it — same id, same rows — and the next turn works on a fresh
    // provider.
    let ts = start();
    let mut c = ts.control(Some(&player));
    let r = create(&mut c, Some(CLIENT), Some(GAME));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.str_field("session"), a);
    assert_eq!(r.json().get("turn").and_then(Value::as_u64), Some(1));
    let t = transcript(&mut c, &a);
    assert_eq!(t.get("turn").and_then(Value::as_u64), Some(1));
    assert_eq!(rows(&t), level_rows("build me a level"));

    run_level_turn(&mut c, &a, "and a tower");
    let t = transcript(&mut c, &a);
    assert_eq!(t.get("turn").and_then(Value::as_u64), Some(2));
    let mut expect = level_rows("build me a level");
    expect.extend(level_rows("and a tower"));
    assert_eq!(rows(&t), expect);
    // An unkeyed session from before the restart is simply gone.
}

#[test]
fn cap_pressure_evicts_the_longest_idle_keyed_session_and_resume_rebuilds_it() {
    let ts = start_server_with("chat_evict", |cfg| apply_cfg(cfg, &PLAIN_TURNS, 2));
    let admin = ts.admin_token();
    let mut c = ts.control(Some(&admin));
    let player = principal_with(&mut c, &[("chat", "gen")]);
    c.set_token(Some(&player));

    let r = create(&mut c, Some(CLIENT), Some(GAME));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let k1 = r.str_field("session");
    send(&mut c, &k1, "hello");
    wait_done(&mut c, &k1);
    let r = create(&mut c, Some(CLIENT), Some(GAME2));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let k2 = r.str_field("session");

    // The owner cap (2) is full of idle keyed sessions: the third create
    // evicts the longest idle one (k1) to disk instead of refusing.
    let r = create(&mut c, Some(CLIENT), Some("ast_00000000000000000000000000000003"));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let k3 = r.str_field("session");
    assert_eq!(c.get(&format!("/v1/chat/sessions/{k1}")).status, 404, "k1 was evicted");
    assert_eq!(c.get(&format!("/v1/chat/sessions/{k2}")).status, 200);
    assert_eq!(c.get(&format!("/v1/chat/sessions/{k3}")).status, 200);

    // Resume k1: rebuilt from disk under its own id, transcript intact
    // (which evicts the next idle one, k2).
    let r = create(&mut c, Some(CLIENT), Some(GAME));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.str_field("session"), k1);
    assert_eq!(
        rows(&transcript(&mut c, &k1)),
        vec![("user".to_string(), "hello".to_string()), ("assistant".to_string(), "hi there".to_string())]
    );
    assert_eq!(c.get(&format!("/v1/chat/sessions/{k2}")).status, 404, "k2 was evicted in turn");
    send(&mut c, &k1, "again");
    wait_done(&mut c, &k1);
    assert_eq!(rows(&transcript(&mut c, &k1)).len(), 4);

    // Unkeyed sessions are never evicted: with the cap full of them the
    // refusal is what it always was.
    let r = create(&mut c, None, None);
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let r = create(&mut c, None, None);
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let r = create(&mut c, None, None);
    assert_eq!(r.status, 413, "{}", String::from_utf8_lossy(&r.body));
}

/// `world.new_level`: the game's answer ends the turn — `done`, no further
/// model round — and the transcript shows the chip.
#[test]
fn a_new_level_answer_ends_the_turn_over_the_wire() {
    let ts = start_server_with("chat_new_level", |cfg| {
        apply_cfg(
            cfg,
            &[
                "New level coming.\n<<tool>>{\"name\":\"world.new_level\",\"args\":{\"title\":\"Quarry\",\"source\":\"game.sky({})\"}}",
                "THIS ROUND MUST NEVER RUN",
            ],
            4,
        )
    });
    let admin = ts.admin_token();
    let mut c = ts.control(Some(&admin));
    let r = create(&mut c, Some(CLIENT), Some(GAME));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let session = r.str_field("session");
    send(&mut c, &session, "make me a new level");
    let events = wait_until(&mut c, &session, |e| {
        ev_type(e) == Some("tool_call") && e.get("name").and_then(Value::as_str) == Some("world.new_level")
    });
    let call = events.iter().find(|e| ev_type(e) == Some("tool_call")).unwrap();
    assert_eq!(call.get("args").and_then(|a| a.get("title")).and_then(Value::as_str), Some("Quarry"));
    let call_id = call.get("id").and_then(Value::as_str).unwrap().to_string();
    let r = c.post_json(
        &format!("/v1/chat/sessions/{session}/tool-result"),
        &jobj(vec![
            ("id", jstr(call_id)),
            (
                "outcome",
                jobj(vec![
                    ("outcome", jstr("ok")),
                    (
                        "value",
                        jobj(vec![
                            ("asset_id", jstr(GAME2)),
                            ("alias", jstr("games/quarry")),
                            ("title", jstr("Quarry")),
                        ]),
                    ),
                ]),
            ),
        ]),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let events = wait_done(&mut c, &session);
    let text: String = events
        .iter()
        .filter(|e| ev_type(e) == Some("delta"))
        .filter_map(|e| e.get("text").and_then(Value::as_str))
        .collect();
    assert!(!text.contains("MUST NEVER RUN"), "{text}");
    assert!(!events.iter().any(|e| ev_type(e) == Some("error")), "{events:?}");
    let r = c.get(&format!("/v1/chat/sessions/{session}"));
    assert_eq!(r.str_field("state"), "idle");
    let t = transcript(&mut c, &session);
    assert_eq!(
        rows(&t),
        vec![
            ("user".to_string(), "make me a new level".to_string()),
            ("assistant".to_string(), "New level coming.".to_string()),
            ("tool".to_string(), "world.new_level · ok".to_string()),
        ]
    );
}
