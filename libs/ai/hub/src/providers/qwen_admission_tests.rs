use super::*;
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

const N1: &str = "http://n1:1";
const N2: &str = "http://n2:1";
const MODEL: &str = "qwen3.8-27b";
// Exact platform/network/src/utils.rs::http_error_out_until(503, None) wire.
const EMPTY503: &[u8] = b"HTTP/1.1 503 Service Unavailable\r\n\
Cross-Origin-Opener-Policy: same-origin\r\n\
Cross-Origin-Embedder-Policy: require-corp\r\n\
Content-Length: 0\r\nConnection: close\r\n\r\n";

fn raw_reply(mut wire: &[u8]) -> Result<Value, FleetError> {
    fleet_http::read_json_response(&mut wire)
        .map_err(HttpFleetTransport::request_error)
        .and_then(|(status, value)| HttpFleetTransport::response(status, value, None, None))
}

#[derive(Clone)]
struct Script(Rc<RefCell<State>>);
struct State {
    now: Instant,
    posts: Vec<(String, Value)>,
    gets: Vec<String>,
    replies: VecDeque<Result<Value, FleetError>>,
    polls: VecDeque<Result<Value, String>>,
    models: BTreeMap<String, Value>,
    cancel_on_probe: Option<Arc<AtomicBool>>,
    cancels: usize,
}

fn models(id: &str, available: bool, state: &str) -> Value {
    json::obj(vec![("models", Value::Arr(vec![json::obj(vec![
        ("id", json::s(id)), ("domain", json::s("chat")),
        ("available", Value::Bool(available)), ("state", json::s(state)),
        ("unavailable_reason", json::s("weights disabled")),
    ])]))])
}
fn accepted() -> Result<Value, FleetError> {
    accepted_with_think(false)
}
fn accepted_with_think(think_open: bool) -> Result<Value, FleetError> {
    let body = if think_open {
        r#"{"job_id":"j1","think_open":true}"#
    } else {
        r#"{"job_id":"j1","think_open":false}"#
    };
    raw_reply(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes())
}
fn refused(status: u16) -> Result<Value, FleetError> {
    let body = r#"{"job_id":null,"error":"all lanes busy"}"#;
    raw_reply(format!("HTTP/1.1 {status} Rejected\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes())
}
fn done(text: &str) -> Result<Value, String> {
    Ok(json::obj(vec![("state", json::s("done")), ("partial_text", json::s(text))]))
}
fn cancelled(reason: &str) -> Result<Value, String> {
    Ok(json::obj(vec![("state", json::s("cancelled")), ("error", json::s(reason))]))
}
fn input() -> TurnInput {
    let mut input = TurnInput::new("system with <tools>unchanged</tools>", vec![
        ChatMessage::new(ChatRole::User, "prompt kept exactly"),
    ]);
    input.dynamic_context = "dynamic context".into();
    input
}
impl Script {
    fn new(replies: impl IntoIterator<Item = Result<Value, FleetError>>) -> Self {
        Self(Rc::new(RefCell::new(State {
            now: Instant::now(), posts: Vec::new(), gets: Vec::new(),
            replies: replies.into_iter().collect(), polls: VecDeque::from([done("answer")]),
            models: BTreeMap::new(), cancel_on_probe: None, cancels: 0,
        })))
    }
    fn advance(&self) { self.0.borrow_mut().now += Duration::from_secs(9); }
    fn provider(&self, bases: &[&str]) -> FleetQwenChatProvider<Self> {
        FleetQwenChatProvider::new(self.clone(), bases.iter().map(|s| s.to_string()).collect())
            .with_preferred_model(Some(MODEL.into()))
            .with_max_tokens(Some(73)).with_thinking(Some(false))
    }
}
impl FleetTransport for Script {
    fn now(&self) -> Instant { self.0.borrow().now }
    fn get_json(&mut self, url: &str) -> Result<Value, String> {
        let mut s = self.0.borrow_mut();
        s.gets.push(url.into());
        if url.ends_with("/health") {
            if let Some(signal) = &s.cancel_on_probe { signal.store(true, Ordering::Relaxed); }
            return Ok(json::obj(vec![("capabilities", Value::Arr(vec![json::s("chat")]))]));
        }
        if let Some(base) = url.strip_suffix("/models") {
            return Ok(s.models.get(base).cloned().unwrap_or_else(|| models(MODEL, true, "loaded")));
        }
        assert!(url.ends_with("/job/j1"), "unexpected GET {url}");
        s.polls.pop_front().expect("unexpected extra job poll")
    }
    fn post_json(&mut self, url: &str, body: &Value) -> Result<Value, String> {
        self.post_json_detailed(url, body).map_err(|e| e.to_string())
    }
    fn post_json_detailed(&mut self, url: &str, body: &Value) -> Result<Value, FleetError> {
        let mut s = self.0.borrow_mut();
        if url.ends_with("/cancel") {
            s.cancels += 1;
            return Ok(Value::Null);
        }
        assert!(url.ends_with("/generate"));
        s.posts.push((url.into(), body.clone()));
        s.replies.pop_front().expect("unexpected extra POST")
    }
}
fn terminal(events: &[ProviderEvent]) -> usize {
    events.iter().filter(|e| matches!(e, ProviderEvent::Done { .. } | ProviderEvent::Error(_))).count()
}
fn finish(p: &mut FleetQwenChatProvider<Script>, t: &Script) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    for _ in 0..30 {
        events.extend(p.poll());
        if terminal(&events) > 0 {
            assert_eq!(terminal(&events), 1);
            assert!(p.poll().is_empty());
            return events;
        }
        t.advance();
    }
    panic!("admission never terminated");
}

#[test]
fn refusals_then_acceptance_keep_request_identity_and_transcript() {
    let t = Script::new([refused(409), refused(429), refused(503), accepted()]);
    let mut p = t.provider(&[N1]);
    p.begin_turn(&input()).unwrap();
    assert!(matches!(p.poll().as_slice(), [ProviderEvent::Status { note, .. }] if note.contains("http 409")));
    let probes = t.0.borrow().gets.len();
    for _ in 0..100 { assert!(p.poll().is_empty()); }
    assert_eq!(t.0.borrow().posts.len(), 1);
    assert_eq!(t.0.borrow().gets.len(), probes);
    t.advance();
    let events = finish(&mut p, &t);
    assert_eq!(events.iter().filter_map(|e| match e { ProviderEvent::Delta(s) => Some(s.as_str()), _ => None }).collect::<String>(), "answer");
    assert!(events.contains(&ProviderEvent::Done { text: "answer".into() }));
    assert!(p.picks.lock().dead_until.is_empty());
    let posts = &t.0.borrow().posts;
    assert_eq!(posts.len(), 4);
    for (_, body) in posts { assert_eq!(body, &posts[0].1); }
    assert_eq!(p.wire.len(), 2);
    assert_eq!(posts[0].1.get("max_tokens").and_then(Value::as_u64), Some(73));
    assert_eq!(posts[0].1.get("thinking").and_then(Value::as_bool), Some(false));
    assert_eq!(posts[0].1.get("chat_session").and_then(Value::as_str), Some(p.conversation.as_str()));
    assert_eq!(posts[0].1.get("prompt").and_then(Value::as_str), Some("prompt kept exactly"));
    assert_eq!(posts[0].1.get("chat_system").and_then(Value::as_str), Some(input().system.as_str()));
}

#[test]
fn complete_empty503_waits_then_accepts_once_with_identical_request() {
    assert!(matches!(raw_reply(EMPTY503), Err(FleetError::Http { status: 503, no_job: true, .. })));
    for json503_too in [false, true] {
        let mut replies = vec![raw_reply(EMPTY503)];
        if json503_too { replies.push(refused(503)); }
        replies.push(accepted());
        let t = Script::new(replies);
        let mut p = t.provider(&[N1]);
        p.begin_turn(&input()).unwrap();
        assert!(p.pending.is_some());
        assert!(p.active.is_none());
        assert!(matches!(p.poll().as_slice(), [ProviderEvent::Status { note, .. }]
            if note.contains("waiting") && note.contains("http 503") && note.contains("HTTP server overloaded before job admission")));
        let probes = t.0.borrow().gets.len();
        for _ in 0..100 { assert!(p.poll().is_empty()); }
        assert_eq!(t.0.borrow().posts.len(), 1);
        assert_eq!(t.0.borrow().gets.len(), probes);
        t.advance();
        let events = finish(&mut p, &t);
        assert_eq!(events.iter().filter_map(|e| match e { ProviderEvent::Delta(s) => Some(s.as_str()), _ => None }).collect::<String>(), "answer");
        assert!(events.contains(&ProviderEvent::Done { text: "answer".into() }));
        assert!(p.pending.is_none());
        assert!(p.picks.lock().dead_until.is_empty());
        assert_eq!(p.wire.len(), 2);
        let s = t.0.borrow();
        assert_eq!(s.posts.len(), if json503_too { 3 } else { 2 });
        assert!(s.posts[0].1.get("seed").and_then(Value::as_u64).is_some());
        assert_eq!(s.posts[0].1.get("chat_session").and_then(Value::as_str), Some(p.conversation.as_str()));
        for post in &s.posts { assert_eq!(post, &s.posts[0]); }
    }
}

#[test]
fn incomplete_or_non_refusal_http_never_replays() {
    let mut replies: Vec<_> = (0..EMPTY503.len()).map(|end| raw_reply(&EMPTY503[..end])).collect();
    let empty = std::str::from_utf8(EMPTY503).unwrap();
    for wire in [
        empty.replace("Content-Length: 0\r\n", ""),
        empty.replace("Connection: close\r\n", ""),
        empty.replace("Content-Length: 0", "Content-Length: 1"),
        empty.replace("Content-Length: 0", "Content-Length: 1\r\nContent-Length: 0"),
        empty.replace("Content-Length: 0", "Transfer-Encoding: chunked\r\nContent-Length: 0"),
        empty.replace("503 Service Unavailable", "429 Too Many Requests"),
        empty.replace("503 Service Unavailable", "409 Conflict"),
        empty.replace("503 Service Unavailable", "500 Internal Server Error"),
        empty.replace("503 Service Unavailable", "200 OK"),
        format!("{empty}{{\"job_id\":\"j1\"}}"),
    ] { replies.push(raw_reply(wire.as_bytes())); }
    for body in ["null", "{", r#"{"error":"busy","job_id":"j1"}"#] {
        replies.push(raw_reply(format!("HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()));
    }
    // Even valid JSON cannot establish safety when the declared body is truncated.
    let body = r#"{"error":"busy","job_id":null}"#;
    replies.push(raw_reply(format!("HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len() + 1).as_bytes()));
    for reply in replies {
        let t = Script::new([reply]);
        let mut p = t.provider(&[N1, N2]);
        assert!(p.begin_turn(&input()).is_err());
        assert!(p.pending.is_none());
        for _ in 0..10 { t.advance(); assert!(p.poll().is_empty()); }
        assert_eq!(t.0.borrow().posts.len(), 1);
    }
}

#[test]
fn every_refusal_consumes_a_bounded_budget() {
    let t = Script::new((0..MAX_ADMISSION_ATTEMPTS).map(|_| refused(503)));
    let mut p = t.provider(&[N1]);
    p.begin_turn(&input()).unwrap();
    let events = finish(&mut p, &t);
    assert!(matches!(events.last(), Some(ProviderEvent::Error(e)) if e.contains("http 503") && e.contains("all lanes busy") && e.contains("8 rounds")));
    assert_eq!(t.0.borrow().posts.len(), MAX_ADMISSION_ATTEMPTS as usize);
}

#[test]
fn second_node_accepts_same_model_and_body() {
    let t = Script::new([refused(503), accepted()]);
    let mut p = t.provider(&[N1, N2]);
    p.begin_turn(&input()).unwrap();
    assert!(finish(&mut p, &t).contains(&ProviderEvent::Done { text: "answer".into() }));
    let s = t.0.borrow();
    assert_eq!(s.posts[0].0, format!("{N1}/generate"));
    assert_eq!(s.posts[1].0, format!("{N2}/generate"));
    assert_eq!(s.posts[0].1, s.posts[1].1);
}

#[test]
fn unavailable_nodes_cannot_change_model_or_bypass_readiness() {
    for (base, row, reason) in [
        (N2, models("qwen3.6-27b", true, "loaded"), "not eligible"),
        (N2, models(MODEL, false, "loaded"), "weights disabled"),
        (N2, models(MODEL, true, "downloading"), "not ready"),
    ] {
        {
            let t = Script::new([refused(503)]);
            let mut p = t.provider(&[N1, base]);
            p.begin_turn(&input()).unwrap();
            t.0.borrow_mut().models.insert(N1.into(), models(MODEL, false, "loaded"));
            t.0.borrow_mut().models.insert(base.into(), row);
            let events = finish(&mut p, &t);
            assert!(matches!(events.last(), Some(ProviderEvent::Error(e)) if e.contains("http 503") && e.contains(reason)), "{events:?}");
            assert_eq!(t.0.borrow().posts.len(), 1);
        }
    }
}

#[test]
fn bad_input_auth_unknown_model_and_ambiguous_failures_never_replay() {
    let mut failures: Vec<_> = [400, 401, 403, 404, 422, 500].into_iter().map(refused).collect();
    for reason in ["write: disconnect after send", "read: connect n1: reset", "incomplete response body"] {
        failures.push(Err(FleetError::Other(reason.into())));
    }
    for reply in failures {
        let t = Script::new([reply]);
        let mut p = t.provider(&[N1, N2]);
        assert!(p.begin_turn(&input()).is_err());
        for _ in 0..10 { t.advance(); assert!(p.poll().is_empty()); }
        assert_eq!(t.0.borrow().posts.len(), 1);
        assert!(p.picks.lock().dead_until.is_empty());
    }
}

#[test]
fn accepted_job_poll_failure_keeps_job_and_never_replays_output() {
    let t = Script::new([accepted()]);
    t.0.borrow_mut().polls = VecDeque::from([
        Ok(json::obj(vec![("state", json::s("running")), ("partial_text", json::s("an"))])),
        Err("read: disconnected".into()), done("answer"),
    ]);
    let mut p = t.provider(&[N1]);
    p.begin_turn(&input()).unwrap();
    let events = finish(&mut p, &t);
    assert_eq!(events.iter().filter_map(|e| match e { ProviderEvent::Delta(s) => Some(s.as_str()), _ => None }).collect::<String>(), "answer");
    assert_eq!(t.0.borrow().posts.len(), 1);
    assert!(t.0.borrow().gets.iter().filter(|s| s.contains("/job/")).all(|s| s == &format!("{N1}/job/j1")));
}

#[test]
fn cancel_waiting_or_during_reprobe_prevents_next_post() {
    for (reply, during_probe) in [(refused(409), false), (refused(409), true),
        (raw_reply(EMPTY503), false), (raw_reply(EMPTY503), true)] {
        let t = Script::new([reply]);
        let signal = Arc::new(AtomicBool::new(false));
        let mut p = t.provider(&[N1]).with_cancel_signal(signal.clone());
        p.begin_turn(&input()).unwrap();
        p.poll(); // drain first waiting status
        if during_probe { t.0.borrow_mut().cancel_on_probe = Some(signal); }
        else { p.cancel(); }
        for _ in 0..10 { t.advance(); assert!(p.poll().is_empty()); }
        assert!(p.pending.is_none());
        assert!(p.wire.is_empty());
        assert_eq!(t.0.borrow().posts.len(), 1);
        assert_eq!(t.0.borrow().cancels, 0);
    }
}

#[test]
fn three_concurrent_turns_finish_once_with_one_refusing() {
    let scripts = [Script::new([refused(409), accepted()]), Script::new([accepted()]), Script::new([accepted()])];
    let cache = Arc::new(FleetPickCache::new());
    let mut providers: Vec<_> = scripts.iter().enumerate().map(|(i, t)| {
        t.0.borrow_mut().polls = VecDeque::from([done(&format!("answer {i}"))]);
        let mut p = t.provider(&[N1]);
        p.picks = cache.clone();
        p.begin_turn(&TurnInput::new("system", vec![ChatMessage::new(ChatRole::User, format!("prompt {i}"))])).unwrap();
        p
    }).collect();
    let mut outputs = [Vec::new(), Vec::new(), Vec::new()];
    for _ in 0..8 {
        for i in 0..3 { outputs[i].extend(providers[i].poll()); scripts[i].advance(); }
    }
    for i in 0..3 {
        assert_eq!(terminal(&outputs[i]), 1);
        assert!(outputs[i].contains(&ProviderEvent::Done { text: format!("answer {i}") }));
        assert_eq!(outputs[i].iter().filter_map(|e| match e { ProviderEvent::Delta(s) => Some(s.as_str()), _ => None }).collect::<String>(), format!("answer {i}"));
        assert_eq!(scripts[i].0.borrow().posts.len(), if i == 0 { 2 } else { 1 });
        assert_eq!(providers[i].wire.len(), 2);
    }
}

#[test]
fn detailed_rejection_preserves_status_and_only_bounded_safe_reason() {
    let body = json::obj(vec![("prompt", json::s("private prompt")), ("chat_system", json::s("private system"))]);
    let response = json::obj(vec![
        ("job_id", Value::Null),
        ("error", json::s(format!("busy: private prompt private system secret-token {}\nAuthorization: Bearer secret-token", "é".repeat(1000)))),
        ("prompt", json::s("must not copy full response")),
    ]);
    let e = HttpFleetTransport::response(503, response, Some(&body), Some("secret-token")).unwrap_err();
    assert!(matches!(&e, FleetError::Http { status: 503, no_job: true, reason } if reason.chars().count() <= 385 && reason.contains("busy")));
    for private in ["private prompt", "private system", "secret-token", "Authorization:", "must not copy"] {
        assert!(!e.to_string().contains(private));
    }
}

#[test]
fn job_id_or_non_error_body_is_never_an_admission_refusal() {
    for body in [
        json::obj(vec![("job_id", json::s("accepted")), ("error", json::s("busy"))]),
        json::obj(vec![("job_id", Value::Int(1)), ("error", json::s("busy"))]),
        Value::Null,
    ] {
        let reply = HttpFleetTransport::response(503, body, None, None);
        let t = Script::new([reply]);
        let mut p = t.provider(&[N1]);
        assert!(p.begin_turn(&input()).is_err());
        assert!(p.pending.is_none());
        assert_eq!(t.0.borrow().posts.len(), 1);
    }
}

#[test]
fn proven_connect_failure_can_retry_but_legacy_error_strings_cannot() {
    let t = Script::new([Err(FleetError::Connection("connect refused".into())), accepted()]);
    let mut p = t.provider(&[N1]);
    p.begin_turn(&input()).unwrap();
    assert!(p.picks.is_dead(N1));
    assert!(finish(&mut p, &t).contains(&ProviderEvent::Done { text: "answer".into() }));

    struct Legacy(Script);
    impl FleetTransport for Legacy {
        fn get_json(&mut self, url: &str) -> Result<Value, String> { self.0.get_json(url) }
        fn post_json(&mut self, _: &str, _: &Value) -> Result<Value, String> {
            Err("connect n1: ambiguous legacy error".into())
        }
    }
    let mut p = FleetQwenChatProvider::new(Legacy(Script::new([])), vec![N1.into()]);
    assert!(p.begin_turn(&input()).is_err());
    assert!(p.pending.is_none());
}

#[test]
fn elapsed_deadline_expires_without_another_post() {
    let t = Script::new([refused(429)]);
    let mut p = t.provider(&[N1]);
    p.begin_turn(&input()).unwrap();
    p.poll();
    t.0.borrow_mut().now += ADMISSION_BUDGET;
    assert!(matches!(p.poll().as_slice(), [ProviderEvent::Error(error)] if error.contains("http 429")));
    assert_eq!(t.0.borrow().posts.len(), 1);
    assert!(p.poll().is_empty());
}

#[test]
fn cancellation_signal_clears_wait_and_new_turn_rebuilds_unsent_tail() {
    let t = Script::new([raw_reply(EMPTY503), accepted()]);
    let signal = Arc::new(AtomicBool::new(false));
    let mut p = t.provider(&[N1]).with_cancel_signal(signal.clone());
    p.begin_turn(&input()).unwrap();
    signal.store(true, Ordering::Relaxed);
    t.advance();
    assert!(p.poll().is_empty());
    assert!(p.pending.is_none());
    assert_eq!(t.0.borrow().posts.len(), 1);
    signal.store(false, Ordering::Relaxed);
    p.begin_turn(&TurnInput::new("new system", vec![ChatMessage::new(ChatRole::User, "replacement")])).unwrap();
    let s = t.0.borrow();
    let messages = s.posts[1].1.get("chat_messages").and_then(Value::as_arr).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].get("text").and_then(Value::as_str), Some("replacement"));
    assert_eq!(s.posts[0].1.get("chat_session"), s.posts[1].1.get("chat_session"));
    assert_ne!(s.posts[0].1.get("seed"), s.posts[1].1.get("seed"));
}

#[test]
fn server_unavailable_reply_without_job_id_is_a_safe_refusal() {
    // /generate's unavailable-model gate emits ErrorJson without job_id.
    let reply = HttpFleetTransport::response(503, json::obj(vec![
        ("error", json::s("model qwen3.8-27b is unavailable: disabled")),
    ]), None, None);
    assert!(matches!(reply, Err(FleetError::Http { status: 503, no_job: true, .. })));
    let t = Script::new([reply, accepted()]);
    let mut p = t.provider(&[N1, N2]);
    p.begin_turn(&input()).unwrap();
    assert!(finish(&mut p, &t).contains(&ProviderEvent::Done { text: "answer".into() }));
    assert_eq!(t.0.borrow().posts.len(), 2);
}

#[test]
fn exhausted_poll_errors_never_resubmit_an_accepted_job() {
    let t = Script::new([accepted()]);
    t.0.borrow_mut().polls = (0..MAX_POLL_FAILS).map(|_| Err("read: job temporarily unreachable".into())).collect();
    let mut p = t.provider(&[N1, N2]);
    p.begin_turn(&input()).unwrap();
    let events = finish(&mut p, &t);
    assert!(matches!(events.as_slice(), [ProviderEvent::Error(error)] if error.contains("job temporarily unreachable")));
    assert_eq!(t.0.borrow().posts.len(), 1);
}

#[test]
fn local_use_cancellation_retries_on_another_node_with_identical_submission() {
    let t = Script::new([accepted(), accepted()]);
    t.0.borrow_mut().polls = VecDeque::from([cancelled("local-use: foreign-gpu-load"), done("answer")]);
    let mut p = t.provider(&[N1, N2]);
    p.begin_turn(&input()).unwrap();
    let first = t.0.borrow().posts[0].1.clone();
    let first_events = p.poll();
    assert!(first_events.iter().all(|event| !matches!(event, ProviderEvent::Delta(_))));
    assert!(first_events.iter().any(|event| matches!(event, ProviderEvent::Status { note, .. } if note.contains("local-use"))));
    assert!(p.pending.is_some());
    t.advance();
    let events = finish(&mut p, &t);
    assert!(events.contains(&ProviderEvent::Done { text: "answer".into() }));
    let s = t.0.borrow();
    assert_eq!(s.posts.len(), 2);
    assert_eq!(s.posts[0].0, format!("{N1}/generate"));
    assert_eq!(s.posts[1].0, format!("{N2}/generate"));
    assert_eq!(s.posts[1].1, first);
}

#[test]
fn empty_loading_poll_does_not_emit_think_before_local_use_recovery() {
    let t = Script::new([accepted_with_think(true), accepted_with_think(true)]);
    t.0.borrow_mut().polls = VecDeque::from([
        Ok(json::obj(vec![("state", json::s("running"))])),
        cancelled("local-use: foreign-gpu-load"),
        done("</think>answer"),
    ]);
    let mut p = t.provider(&[N1, N2]);
    p.begin_turn(&input()).unwrap();
    assert!(p.poll().is_empty());
    let events = p.poll();
    assert!(events.iter().all(|event| !matches!(event, ProviderEvent::Delta(_))));
    t.advance();
    let events = finish(&mut p, &t);
    assert!(events.contains(&ProviderEvent::Done { text: "<think></think>answer".into() }));
    let deltas = events.iter().filter_map(|event| match event {
        ProviderEvent::Delta(text) => Some(text.as_str()),
        _ => None,
    }).collect::<String>();
    assert_eq!(deltas.matches("<think>").count(), 1);
    assert_eq!(t.0.borrow().posts.len(), 2);
}

#[test]
fn user_cancellation_does_not_retry() {
    let t = Script::new([accepted(), accepted()]);
    t.0.borrow_mut().polls = VecDeque::from([cancelled("user requested cancel")]);
    let mut p = t.provider(&[N1, N2]);
    p.begin_turn(&input()).unwrap();
    let events = p.poll();
    assert!(matches!(events.as_slice(), [ProviderEvent::Error(error)] if error.contains("user requested cancel")));
    assert_eq!(t.0.borrow().posts.len(), 1);
    assert!(p.pending.is_none());
}

#[test]
fn local_use_cancellation_after_published_text_is_terminal_without_replay() {
    let t = Script::new([accepted(), accepted()]);
    t.0.borrow_mut().polls = VecDeque::from([
        Ok(json::obj(vec![("state", json::s("running")), ("partial_text", json::s("hello"))])),
        cancelled("local-use: foreign-gpu-load"),
    ]);
    let mut p = t.provider(&[N1, N2]);
    p.begin_turn(&input()).unwrap();
    let first = p.poll();
    assert!(first.contains(&ProviderEvent::Delta("hello".into())));
    let events = p.poll();
    assert!(matches!(events.as_slice(), [ProviderEvent::Error(error)] if error.contains("local-use: foreign-gpu-load")));
    assert_eq!(t.0.borrow().posts.len(), 1);
    assert!(p.pending.is_none());
}

#[test]
fn cancel_during_local_use_recovery_prevents_the_next_post() {
    let t = Script::new([accepted(), accepted()]);
    t.0.borrow_mut().polls = VecDeque::from([cancelled("local-use: foreign-gpu-load")]);
    let mut p = t.provider(&[N1, N2]);
    p.begin_turn(&input()).unwrap();
    p.poll();
    assert!(p.pending.is_some());
    p.cancel();
    t.advance();
    for _ in 0..5 { assert!(p.poll().is_empty()); }
    assert_eq!(t.0.borrow().posts.len(), 1);
    assert!(p.pending.is_none());
    assert!(p.wire.is_empty());
}

#[test]
fn three_local_use_interruptions_are_bounded_and_never_return_to_an_interrupted_node() {
    let n3 = "http://n3:1";
    let t = Script::new([accepted(), accepted(), accepted()]);
    t.0.borrow_mut().polls = VecDeque::from([
        cancelled("local-use: foreign-gpu-load"),
        cancelled("local-use: foreign-gpu-load"),
        cancelled("local-use: foreign-gpu-load"),
    ]);
    let mut p = t.provider(&[N1, N2, n3]);
    p.begin_turn(&input()).unwrap();
    let terminal_events = finish(&mut p, &t);
    assert_eq!(terminal(&terminal_events), 1);
    assert!(terminal_events.iter().any(|event| matches!(event, ProviderEvent::Error(error) if error.contains("local-use: foreign-gpu-load"))));
    let s = t.0.borrow();
    assert_eq!(s.posts.len(), 3);
    assert_eq!(s.posts.iter().map(|(url, _)| url.as_str()).collect::<Vec<_>>(), vec![
        format!("{N1}/generate"), format!("{N2}/generate"), format!("{n3}/generate"),
    ]);
    assert!(p.pending.is_none());
}
