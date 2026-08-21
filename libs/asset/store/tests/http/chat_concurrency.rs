//! N simultaneous chat sessions over REAL sockets, against scripted
//! providers with realistic per-turn latency.
//!
//! This is the suite that holds the broker's parallel-slot readiness. The
//! old broker was one actor thread that owned every session, so N turns
//! advanced round-robin 50 ms at a time and any blocking tool froze the
//! lot — a create then hit its reply timeout and answered `503 state
//! unavailable`. Each test here fails loudly if that shape ever comes back:
//!
//! - `n_sessions_advance_their_turns_at_the_same_time` — wallclock far
//!   below the serial sum.
//! - `interleaved_sessions_never_cross_streams` — every session's events
//!   and tool-results are its own, even though the parked call ids are
//!   IDENTICAL across sessions (`tc_1_1`).
//! - `no_session_starves_when_sessions_outnumber_slots` — with fewer
//!   provider slots than sessions, every session still finishes every turn.
//! - `create_is_not_blocked_by_streaming_sessions` — a create issued while
//!   N turns are in flight is a prompt 201, never a 503.
//! - `the_configured_per_owner_cap_is_what_is_enforced` — the cap is a
//!   config knob, and it is the config's number that bites.

mod common;
use common::*;

use makepad_asset_store::json::Value;
use makepad_asset_store::{ChatConfig, ChatScript, ScriptedLane, ScriptedTurn};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// A plain lane: `turns` replies, each costing `delay_ms` of wall clock.
fn lane(model: &str, turns: Vec<&str>, delay_ms: u64) -> ScriptedLane {
    ScriptedLane {
        available: true,
        model: model.into(),
        turns: turns.into_iter().map(|t| ScriptedTurn::Text(t.into())).collect(),
        turn_delay_ms: delay_ms,
    }
}

fn chat_cfg(
    fleet: ScriptedLane,
    max_concurrent_turns: usize,
    max_sessions: usize,
    max_sessions_per_owner: usize,
) -> impl FnOnce(&mut makepad_asset_store::ServerConfig) {
    move |cfg| {
        cfg.chat = ChatConfig {
            fleet: String::new(),
            fleet_bases: Vec::new(),
            max_sessions,
            max_sessions_per_owner,
            event_cap: 256,
            event_max_wait_ms: 2_000,
            script: Some(ChatScript {
                fleet_qwen: fleet,
                openai: ScriptedLane::default(),
                grok: ScriptedLane::default(),
                max_concurrent_turns,
            }),
        };
    }
}

fn create_session(c: &mut Client, profile: Option<&str>) -> String {
    let mut fields = vec![
        ("api_version", Value::Int(1)),
        ("namespace", jstr("gen")),
        ("provider", jstr("fleet-qwen")),
    ];
    if let Some(p) = profile {
        fields.push(("client", jstr(p)));
    }
    let r = c.post_json("/v1/chat/sessions", &jobj(fields));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    r.str_field("session")
}

fn send(c: &mut Client, session: &str, text: &str) {
    let r = c.post_json(
        &format!("/v1/chat/sessions/{session}/send"),
        &jobj(vec![("text", jstr(text))]),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
}

/// Poll from `after` until `pred` matches, returning (matched event, new
/// cursor). Panics with the whole stream if the deadline passes.
fn poll_until(
    c: &mut Client,
    session: &str,
    after: u64,
    budget: Duration,
    pred: impl Fn(&Value) -> bool,
) -> (Value, u64) {
    let deadline = Instant::now() + budget;
    let mut cursor = after;
    let mut seen: Vec<Value> = Vec::new();
    while Instant::now() < deadline {
        let r = c.get(&format!(
            "/v1/chat/sessions/{session}/events?after={cursor}&wait=500&limit=128"
        ));
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        let body = r.json();
        for ev in body.get("events").and_then(Value::as_arr).unwrap() {
            let seq = ev.get("seq").and_then(Value::as_u64).unwrap_or(cursor);
            cursor = cursor.max(seq);
            if pred(ev) {
                return (ev.clone(), cursor);
            }
            seen.push(ev.clone());
        }
    }
    panic!("event never arrived for {session}; saw {seen:?}");
}

fn wait_done(c: &mut Client, session: &str, after: u64, budget: Duration) -> u64 {
    poll_until(c, session, after, budget, |e| {
        matches!(e.get("type").and_then(Value::as_str), Some("done") | Some("error"))
    })
    .1
}

/// Every event a session has ever emitted, read from the start.
fn all_events(c: &mut Client, session: &str) -> Vec<Value> {
    let r = c.get(&format!("/v1/chat/sessions/{session}/events?after=0&limit=256"));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    r.json().get("events").and_then(Value::as_arr).unwrap().to_vec()
}

fn principals(admin: &mut Client, n: usize) -> Vec<String> {
    (0..n).map(|_| principal_with(admin, &[("chat", "gen")])).collect()
}

// ---------------------------------------------------------------------------

/// N sessions × M turns, each turn costing real wall clock. If ANY shared
/// thing serialises the turns, the total is the serial sum; it must not be.
#[test]
fn n_sessions_advance_their_turns_at_the_same_time() {
    const SESSIONS: usize = 6;
    const TURNS: usize = 2;
    const DELAY_MS: u64 = 300;

    let ts = start_server_with(
        "chat_parallel",
        chat_cfg(
            lane("qwen-scripted", vec!["first reply", "second reply"], DELAY_MS),
            0,
            32,
            8,
        ),
    );
    let admin = ts.admin_token();
    let mut c = ts.control(Some(&admin));
    let tokens = principals(&mut c, SESSIONS);
    let addr: SocketAddr = ts.server.control_addr();

    let started = Instant::now();
    let workers: Vec<_> = tokens
        .into_iter()
        .map(|token| {
            std::thread::spawn(move || {
                let mut c = Client::new(addr, Some(&token));
                let session = create_session(&mut c, None);
                let mut cursor = 0;
                for turn in 0..TURNS {
                    send(&mut c, &session, &format!("turn {turn}"));
                    cursor = wait_done(&mut c, &session, cursor, Duration::from_secs(20));
                }
                session
            })
        })
        .collect();
    let sessions: Vec<String> = workers.into_iter().map(|w| w.join().expect("driver")).collect();
    let elapsed = started.elapsed();

    // Every driver really got its own session.
    assert_eq!(sessions.iter().collect::<HashSet<_>>().len(), SESSIONS, "{sessions:?}");

    let serial = Duration::from_millis(DELAY_MS * (SESSIONS * TURNS) as u64);
    assert!(
        elapsed < serial / 2,
        "turns did not overlap: {elapsed:?} against a {serial:?} serial sum \
         ({SESSIONS} sessions x {TURNS} turns x {DELAY_MS} ms)"
    );
}

/// The parked call ids are IDENTICAL across sessions (`tc_1_1`), and every
/// session parks at the same time. Each answer must reach exactly the
/// session it was posted to.
#[test]
fn interleaved_sessions_never_cross_streams() {
    const SESSIONS: usize = 5;
    const DELAY_MS: u64 = 150;

    let ts = start_server_with(
        "chat_isolation",
        chat_cfg(
            lane(
                "qwen-scripted",
                vec![
                    "<<tool>>{\"name\":\"world.set_source\",\"args\":{\"source\":\"game.sky({})\"}}",
                    "the level is live",
                ],
                DELAY_MS,
            ),
            0,
            32,
            8,
        ),
    );
    let admin = ts.admin_token();
    let mut c = ts.control(Some(&admin));
    let tokens = principals(&mut c, SESSIONS);
    let addr: SocketAddr = ts.server.control_addr();

    let workers: Vec<_> = tokens
        .into_iter()
        .enumerate()
        .map(|(i, token)| {
            std::thread::spawn(move || {
                let mut c = Client::new(addr, Some(&token));
                let session = create_session(&mut c, Some("game"));
                let tag = format!("tag-{i}-{}", session);
                send(&mut c, &session, "build me a level");
                let (call, cursor) =
                    poll_until(&mut c, &session, 0, Duration::from_secs(20), |e| {
                        e.get("type").and_then(Value::as_str) == Some("tool_call")
                            && e.get("name").and_then(Value::as_str) == Some("world.set_source")
                    });
                let call_id = call.get("id").and_then(Value::as_str).unwrap().to_string();
                // Every session's park id is the same string; only the
                // routing keeps the answers apart.
                assert_eq!(call_id, "tc_1_1", "the ids are shared on purpose");
                // Hold the park a moment so all sessions are parked at once.
                std::thread::sleep(Duration::from_millis(150));
                let r = c.post_json(
                    &format!("/v1/chat/sessions/{session}/tool-result"),
                    &jobj(vec![
                        ("id", jstr(call_id)),
                        (
                            "outcome",
                            jobj(vec![
                                ("outcome", jstr("ok")),
                                ("value", jobj(vec![("eval", jstr(tag.clone()))])),
                            ]),
                        ),
                    ]),
                );
                assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
                wait_done(&mut c, &session, cursor, Duration::from_secs(20));
                let events = all_events(&mut c, &session);
                (session, tag, events)
            })
        })
        .collect();
    let runs: Vec<(String, String, Vec<Value>)> =
        workers.into_iter().map(|w| w.join().expect("driver")).collect();

    let all_tags: Vec<String> = runs.iter().map(|(_, tag, _)| tag.clone()).collect();
    for (session, tag, events) in &runs {
        let results: Vec<&Value> = events
            .iter()
            .filter(|e| e.get("type").and_then(Value::as_str) == Some("tool_result"))
            .collect();
        assert_eq!(results.len(), 1, "{session} saw {} tool results", results.len());
        let value = results[0]
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.get("eval"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(&value, tag, "{session} got another session's tool result");
        // And nothing else from another session leaked into this stream.
        let text = format!("{events:?}");
        for other in &all_tags {
            if other != tag {
                assert!(!text.contains(other), "{session} leaked {other}");
            }
        }
        assert!(
            events
                .iter()
                .any(|e| e.get("type").and_then(Value::as_str) == Some("done")),
            "{session} never finished"
        );
    }
}

/// More sessions than the serving tier admits at once. Capacity bounds the
/// throughput; it must not decide that some session never runs.
#[test]
fn no_session_starves_when_sessions_outnumber_slots() {
    const SESSIONS: usize = 6;
    const TURNS: usize = 2;
    const DELAY_MS: u64 = 200;
    const SLOTS: usize = 2;

    let ts = start_server_with(
        "chat_fairness",
        chat_cfg(
            lane("qwen-scripted", vec!["reply one", "reply two"], DELAY_MS),
            SLOTS,
            32,
            8,
        ),
    );
    let admin = ts.admin_token();
    let mut c = ts.control(Some(&admin));
    let tokens = principals(&mut c, SESSIONS);
    let addr: SocketAddr = ts.server.control_addr();

    let started = Instant::now();
    let workers: Vec<_> = tokens
        .into_iter()
        .map(|token| {
            std::thread::spawn(move || {
                let mut c = Client::new(addr, Some(&token));
                let session = create_session(&mut c, None);
                let mut cursor = 0;
                let mut done = 0;
                for turn in 0..TURNS {
                    send(&mut c, &session, &format!("turn {turn}"));
                    cursor = wait_done(&mut c, &session, cursor, Duration::from_secs(30));
                    done += 1;
                }
                (session, done)
            })
        })
        .collect();
    let finished: Vec<(String, usize)> =
        workers.into_iter().map(|w| w.join().expect("driver")).collect();
    let elapsed = started.elapsed();

    for (session, done) in &finished {
        assert_eq!(*done, TURNS, "{session} starved after {done} turns");
    }
    // Ideal is (total turns / slots) waves of DELAY_MS. Well under a serial
    // run, and nowhere near a starving one.
    let ideal = Duration::from_millis(DELAY_MS * (SESSIONS * TURNS / SLOTS) as u64);
    assert!(
        elapsed < ideal * 4,
        "capacity-bound run took {elapsed:?}, ideal {ideal:?}"
    );
}

/// Create used to ride the same thread every turn ran on, so a busy broker
/// answered it `503 state unavailable`. It must be prompt while N turns are
/// in flight — including turns that are slower than create's own timeout
/// would have been.
#[test]
fn create_is_not_blocked_by_streaming_sessions() {
    const SESSIONS: usize = 4;
    const DELAY_MS: u64 = 1_500;

    let ts = start_server_with(
        "chat_create_under_load",
        chat_cfg(
            lane("qwen-scripted", vec!["slow reply", "slow reply two"], DELAY_MS),
            0,
            32,
            8,
        ),
    );
    let admin = ts.admin_token();
    let mut c = ts.control(Some(&admin));
    let tokens = principals(&mut c, SESSIONS);
    let late = principal_with(&mut c, &[("chat", "gen")]);
    let addr: SocketAddr = ts.server.control_addr();

    let workers: Vec<_> = tokens
        .into_iter()
        .map(|token| {
            std::thread::spawn(move || {
                let mut c = Client::new(addr, Some(&token));
                let session = create_session(&mut c, None);
                send(&mut c, &session, "start a slow turn");
                wait_done(&mut c, &session, 0, Duration::from_secs(30));
            })
        })
        .collect();

    // Every session is now inside a 1.5 s provider turn.
    std::thread::sleep(Duration::from_millis(300));
    let mut late_client = Client::new(addr, Some(&late));
    for _ in 0..3 {
        let at = Instant::now();
        let r = late_client.post_json(
            "/v1/chat/sessions",
            &jobj(vec![
                ("api_version", Value::Int(1)),
                ("namespace", jstr("gen")),
                ("provider", jstr("fleet-qwen")),
            ]),
        );
        assert_eq!(
            r.status,
            201,
            "create under load: {}",
            String::from_utf8_lossy(&r.body)
        );
        assert!(
            at.elapsed() < Duration::from_secs(1),
            "create waited {:?} behind streaming sessions",
            at.elapsed()
        );
        // Reads stay live too while the turns run.
        let at = Instant::now();
        let r = late_client.get("/v1/chat/sessions");
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        assert!(at.elapsed() < Duration::from_secs(1), "list waited {:?}", at.elapsed());
    }

    for w in workers {
        w.join().expect("driver");
    }
}

/// The per-owner cap stays, and it is the CONFIGURED number that bites —
/// a box serving more parallel slots raises it in config, not in a rebuild.
#[test]
fn the_configured_per_owner_cap_is_what_is_enforced() {
    const PER_OWNER: usize = 3;
    let ts = start_server_with(
        "chat_owner_cap",
        chat_cfg(lane("qwen-scripted", vec!["hi"], 0), 0, 32, PER_OWNER),
    );
    let admin = ts.admin_token();
    let mut c = ts.control(Some(&admin));
    let mine = principal_with(&mut c, &[("chat", "gen")]);
    let theirs = principal_with(&mut c, &[("chat", "gen")]);

    c.set_token(Some(&mine));
    for _ in 0..PER_OWNER {
        create_session(&mut c, None);
    }
    let r = c.post_json(
        "/v1/chat/sessions",
        &jobj(vec![
            ("api_version", Value::Int(1)),
            ("namespace", jstr("gen")),
            ("provider", jstr("fleet-qwen")),
        ]),
    );
    assert_eq!(r.status, 413, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json().get("error").and_then(Value::as_str), Some("over_budget"));

    // Another principal is unaffected: the cap is per owner, not global.
    c.set_token(Some(&theirs));
    create_session(&mut c, None);
}

/// Concurrent creates from ONE owner cannot both slip past the cap: the
/// reservation is taken under the registry lock, counting creates in
/// flight.
#[test]
fn concurrent_creates_cannot_race_past_the_owner_cap() {
    const PER_OWNER: usize = 2;
    const ATTEMPTS: usize = 8;
    let ts = start_server_with(
        "chat_cap_race",
        chat_cfg(lane("qwen-scripted", vec!["hi"], 0), 0, 32, PER_OWNER),
    );
    let admin = ts.admin_token();
    let mut c = ts.control(Some(&admin));
    let token = principal_with(&mut c, &[("chat", "gen")]);
    let addr: SocketAddr = ts.server.control_addr();

    let workers: Vec<_> = (0..ATTEMPTS)
        .map(|_| {
            let token = token.clone();
            std::thread::spawn(move || {
                let mut c = Client::new(addr, Some(&token));
                c.post_json(
                    "/v1/chat/sessions",
                    &jobj(vec![
                        ("api_version", Value::Int(1)),
                        ("namespace", jstr("gen")),
                        ("provider", jstr("fleet-qwen")),
                    ]),
                )
                .status
            })
        })
        .collect();
    let statuses: Vec<u16> = workers.into_iter().map(|w| w.join().expect("create")).collect();
    let created = statuses.iter().filter(|s| **s == 201).count();
    let refused = statuses.iter().filter(|s| **s == 413).count();
    assert_eq!(created, PER_OWNER, "{statuses:?}");
    assert_eq!(created + refused, ATTEMPTS, "{statuses:?}");

    c.set_token(Some(&token));
    let r = c.get("/v1/chat/sessions");
    assert_eq!(r.status, 200);
    let rows = r.json().get("sessions").and_then(Value::as_arr).unwrap().len();
    assert_eq!(rows, PER_OWNER);
}
