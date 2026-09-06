use super::*;
use super::submission::{read_reply, Reply, Transport};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::{Arc, Barrier, atomic::{AtomicBool, AtomicUsize, Ordering}};
use std::time::{Duration, Instant};

const NODE: &str = "http://fixture:8765";
const GATE: &[u8] = b"HTTP/1.1 503 Service Unavailable\r\nCross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Embedder-Policy: require-corp\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
fn response(status: u16, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status} Response\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()
}
fn refusal(status: u16) -> Vec<u8> { response(status, r#"{"job_id":null,"error":"queue full: 2 jobs already queued on this node"}"#) }
fn accepted() -> Vec<u8> { response(200, r#"{"job_id":"accepted","error":null}"#) }
fn request(seed: u64) -> GenerateRequestJson {
    GenerateRequestJson { model: "minimax-h3".into(), seed: Some(seed), origin_key: Some("fixture-origin".into()),
        origin_epoch: Some(42), chat_session: Some("session-kept".into()), prompt: Some("private prompt".into()),
        input_b64: Some("cHJpdmF0ZSBpbmJvdW5kIHBheWxvYWQ=".into()), ..Default::default() }
}
struct Script {
    now: Instant, replies: VecDeque<Vec<u8>>, posts: Vec<Vec<u8>>,
    cancel_on_sleep: Option<Arc<AtomicBool>>, cancel_on_post: Option<Arc<AtomicBool>>,
    post_elapsed: Duration, io_error: Option<std::io::ErrorKind>,
}
impl Script {
    fn new(replies: impl IntoIterator<Item=Vec<u8>>) -> Self {
        Self { now: Instant::now(), replies: replies.into_iter().collect(), posts: vec![],
            cancel_on_sleep: None, cancel_on_post: None, post_elapsed: Duration::ZERO, io_error: None }
    }
}
impl Transport for Script {
    fn post(&mut self, url: &str, _: &[(String, String)], body: &[u8], _: Duration) -> Result<Reply, AssetAiError> {
        assert_eq!(url, format!("{NODE}/generate"));
        self.posts.push(body.to_vec());
        self.now += self.post_elapsed;
        if let Some(cancel) = &self.cancel_on_post { cancel.store(true, Ordering::Relaxed); }
        if let Some(kind) = self.io_error { return Err(std::io::Error::from(kind).into()); }
        // Exercise the shipping strict response reader, including fragments.
        struct Fragments(std::io::Cursor<Vec<u8>>);
        impl Read for Fragments {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let len = buf.len().min(7); self.0.read(&mut buf[..len])
            }
        }
        read_reply(&mut Fragments(std::io::Cursor::new(self.replies.pop_front().expect("unexpected POST replay"))))
    }
    fn now(&self) -> Instant { self.now }
    fn sleep(&mut self, duration: Duration) {
        assert!(duration <= Duration::from_millis(50));
        self.now += duration;
        if let Some(cancel) = &self.cancel_on_sleep { cancel.store(true, Ordering::Relaxed); }
    }
}
fn run(script: &mut Script, cancel: &AtomicBool, notes: &mut Vec<String>) -> Result<String, AssetAiError> {
    LocalService::new(NODE).request_pending_using(Domain::Video, &request(u64::MAX),
        &|| cancel.load(Ordering::Relaxed), &mut |note| notes.push(note.into()), script)
}

#[test]
fn disk_refusal_returns_for_fleet_failover_without_reposting_to_full_disk() {
    let mut script = Script::new([response(409,
        r#"{"job_id":null,"error":"model unavailable: disk-space: insufficient on C:"}"#)]);
    let mut notes = Vec::new();
    let error = run(&mut script, &AtomicBool::new(false), &mut notes).unwrap_err();
    assert!(matches!(error, AssetAiError::Unavailable(ref reason) if reason.starts_with("disk-space:")));
    assert_eq!(script.posts.len(), 1);
    assert!(notes.is_empty());
}

#[test]
fn burst_six_callers_gate_then_json_refusal_accepts_exactly_six_unchanged_requests() {
    let barrier = Barrier::new(6);
    let accepted_count = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for caller in 0..6 {
            let barrier = &barrier;
            let count = &accepted_count;
            scope.spawn(move || {
                let service = LocalService::new(NODE);
                let wire = request(u64::MAX - caller);
                let bytes = wire.serialize_json().into_bytes();
                let mut script = Script::new([GATE.to_vec(), refusal([409, 429, 503][caller as usize % 3]), accepted()]);
                let mut notes = Vec::new();
                barrier.wait();
                let id = service.request_pending_using(Domain::Video, &wire, &|| false,
                    &mut |note| notes.push(note.to_string()), &mut script).unwrap();
                assert_eq!(id, "accepted");
                count.fetch_add(1, Ordering::Relaxed);
                assert_eq!(script.posts, vec![bytes; 3]);
                assert_eq!(notes.len(), 2);
                assert!(notes[0].contains(NODE) && notes[0].contains("http 503"));
                assert!(notes[1].contains("queue full: 2"));
                assert_eq!(*service.lease_origin.lock().unwrap(), Some(("fixture-origin".into(), 42)));
            });
        }
    });
    assert_eq!(accepted_count.load(Ordering::Relaxed), 6);
}

#[test]
fn pending_cancellation_interrupts_wait_and_never_posts_again() {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut script = Script::new([GATE.to_vec(), accepted()]);
    script.cancel_on_sleep = Some(cancel.clone());
    assert_eq!(run(&mut script, &cancel, &mut vec![]), Err(AssetAiError::Cancelled));
    assert_eq!(script.posts.len(), 1);
    let mut before = Script::new([]);
    assert_eq!(run(&mut before, &cancel, &mut vec![]), Err(AssetAiError::Cancelled));
    assert!(before.posts.is_empty());
}

#[test]
fn cancellation_during_accepted_post_returns_ownership() {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut script = Script::new([accepted()]);
    script.cancel_on_post = Some(cancel.clone());
    assert_eq!(run(&mut script, &cancel, &mut vec![]).unwrap(), "accepted");
    assert_eq!(script.posts.len(), 1);
}

#[test]
fn ambiguous_malformed_truncated_or_unrecognized_responses_never_replay() {
    let gate = String::from_utf8(GATE.to_vec()).unwrap();
    let mut bad = vec![
        b"HTTP/1.1 503".to_vec(),
        gate.replace("Content-Length: 0\r\n", "").into_bytes(),
        gate.replace("Content-Length: 0", "Content-Length: 0\r\nContent-Length: 0").into_bytes(),
        gate.replace("Content-Length: 0", "Transfer-Encoding: chunked\r\nContent-Length: 0").into_bytes(),
        gate.replace("Content-Length: 0", "Content-Length: 1").into_bytes(),
        gate.replace("Connection: close", "Connection: keep-alive").into_bytes(),
        response(500, r#"{"job_id":null,"error":"busy"}"#),
        response(503, "{malformed"), response(503, "null"),
        response(503, r#"{"error":"model inference failed"}"#),
        response(409, r#"{"job_id":42,"error":"busy"}"#),
        response(409, r#"{"job_id":null,"error":"busy","think_open":42}"#),
        response(409, r#"{"job_id":"","error":"busy"}"#),
        response(409, r#"{"job_id":null,"job_id":null,"error":"busy"}"#),
        response(503, r#"{"job_id":null,"error":"busy","artifacts":[]}"#),
        response(409, r#"{"job_id":null,"error":"busy"} trailing"#),
        response(307, r#"{"job_id":null,"error":"busy"}"#),
    ];
    let mut truncated = refusal(409); truncated.pop(); bad.push(truncated);
    let valid_json = r#"{"job_id":null,"error":"busy"}"#;
    bad.push(format!("HTTP/1.1 409 Conflict\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{valid_json}", valid_json.len() + 1).into_bytes());
    for wire in bad {
        let mut script = Script::new([wire, accepted()]);
        assert!(run(&mut script, &AtomicBool::new(false), &mut vec![]).is_err());
        assert_eq!(script.posts.len(), 1);
    }
    for kind in [std::io::ErrorKind::TimedOut, std::io::ErrorKind::ConnectionReset,
        std::io::ErrorKind::UnexpectedEof, std::io::ErrorKind::ConnectionRefused] {
        let mut script = Script::new([]); script.io_error = Some(kind);
        assert!(run(&mut script, &AtomicBool::new(false), &mut vec![]).is_err());
        assert_eq!(script.posts.len(), 1);
    }
}

#[test]
fn legacy_provider_default_stays_one_shot_and_preserves_accepted_ownership() {
    struct Legacy { posts: AtomicUsize, cancel: AtomicBool, accept: bool }
    impl ContentProvider for Legacy {
        fn health(&self) -> Result<HealthJson, AssetAiError> { unreachable!() }
        fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError> { unreachable!() }
        fn request(&self, _: Domain, _: &GenerateRequestJson) -> Result<String, AssetAiError> {
            self.posts.fetch_add(1, Ordering::Relaxed);
            self.cancel.store(true, Ordering::Relaxed);
            if self.accept { Ok("legacy-owned".into()) } else { Err(AssetAiError::Busy) }
        }
        fn poll(&self, _: &str) -> Result<JobStatusJson, AssetAiError> { unreachable!() }
        fn fetch_artifact(&self, _: &str) -> Result<ArtifactBytes, AssetAiError> { unreachable!() }
    }
    for accept in [false, true] {
        let provider = Legacy { posts: AtomicUsize::new(0), cancel: AtomicBool::new(false), accept };
        let result = provider.request_pending(Domain::Video, &request(42),
            &|| provider.cancel.load(Ordering::Relaxed), &mut |_| panic!("legacy provider cannot wait"));
        assert_eq!(result, if accept { Ok("legacy-owned".into()) } else { Err(AssetAiError::Busy) });
        assert_eq!(provider.posts.load(Ordering::Relaxed), 1);
        assert_eq!(provider.request_pending(Domain::Video, &request(42),
            &|| true, &mut |_| {}), Err(AssetAiError::Cancelled));
        assert_eq!(provider.posts.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn any_returned_job_id_precludes_replay_even_on_error_status() {
    let mut script = Script::new([response(503, r#"{"job_id":"owned","error":"busy"}"#)]);
    assert_eq!(run(&mut script, &AtomicBool::new(false), &mut vec![]).unwrap(), "owned");
    assert_eq!(script.posts.len(), 1);
}

#[test]
fn finite_attempt_and_time_budgets_name_endpoint_and_last_refusal() {
    for elapsed in [Duration::ZERO, Duration::from_secs(90)] {
        let mut script = Script::new((0..8).map(|_| refusal(429)));
        script.post_elapsed = elapsed;
        let mut notes = vec![];
        let error = run(&mut script, &AtomicBool::new(false), &mut notes).unwrap_err().to_string();
        assert!(error.contains(NODE) && error.contains("http 429") && error.contains("queue full: 2"), "{error}");
        assert_eq!(script.posts.len(), if elapsed.is_zero() { 8 } else { 1 });
    }
}

#[test]
fn progress_and_terminal_errors_redact_payloads_and_bound_server_text() {
    let wire = request(u64::MAX);
    let error = format!("busy: {} {} token-secret {}\n", wire.prompt.unwrap(), wire.input_b64.unwrap(), "é".repeat(2000));
    let body = makepad_strict_json::obj(vec![("job_id", makepad_strict_json::Value::Null),
        ("error", makepad_strict_json::s(error))]).to_json();
    let mut script = Script::new((0..8).map(|_| response(503, &body)));
    let mut notes = vec![];
    let error = LocalService::new(NODE).with_secret("token-secret").request_pending_using(Domain::Video,
        &request(u64::MAX), &|| false, &mut |note| notes.push(note.to_string()), &mut script).unwrap_err();
    notes.push(error.to_string());
    for note in notes {
        assert!(note.chars().count() < 800);
        assert!(!note.contains("private prompt") && !note.contains("cHJpdmF0") && !note.contains("token-secret"));
        assert!(!note.contains('\n'));
    }
}

// Explicitly run outside restricted sandboxes. Test threads are fixture
// clients/server only; shipping submission never starts a thread or pool.
#[test]
#[ignore = "requires loopback bind; run explicitly with --ignored"]
fn loopback_six_real_local_services_accept_once_and_poll_error_never_reposts() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback fixture bind");
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let stop = AtomicBool::new(false);
    let barrier = Barrier::new(6);
    std::thread::scope(|scope| {
        let server = scope.spawn(|| {
            let mut posts = std::collections::HashMap::<Vec<u8>, usize>::new();
            let mut polls = 0;
            while !stop.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(s) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => { std::thread::sleep(Duration::from_millis(1)); continue; }
                    Err(e) => panic!("fixture accept: {e}"),
                };
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut bytes = Vec::new();
                let mut byte = [0];
                while !bytes.ends_with(b"\r\n\r\n") { stream.read_exact(&mut byte).unwrap(); bytes.push(byte[0]); }
                let head = String::from_utf8(bytes).unwrap();
                if head.starts_with("GET /job/") {
                    polls += 1;
                    stream.write_all(&response(500, r#"{"error":"fixture poll failure"}"#)).unwrap();
                    continue;
                }
                assert!(head.starts_with("POST /generate "));
                let length: usize = head.lines().find_map(|line| line.strip_prefix("Content-Length: ")).unwrap().parse().unwrap();
                let mut body = vec![0; length]; stream.read_exact(&mut body).unwrap();
                let count = posts.entry(body).or_default(); *count += 1;
                stream.write_all(&match *count { 1 => GATE.to_vec(), 2 => refusal(409), 3 => accepted(), _ => panic!("accepted job replayed") }).unwrap();
            }
            assert_eq!(posts.len(), 6);
            assert!(posts.values().all(|n| *n == 3));
            assert_eq!(polls, 6);
        });
        let clients: Vec<_> = (0..6).map(|i| {
            let url = &url; let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                let service = LocalService::new(url);
                let id = service.request_pending(Domain::Video, &request(u64::MAX - i), &|| false, &mut |_| {}).unwrap();
                assert!(service.poll(&id).unwrap_err().to_string().contains("fixture poll failure"));
            })
        }).collect();
        let results: Vec<_> = clients.into_iter().map(|t| t.join()).collect();
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
        assert!(results.into_iter().all(|r| r.is_ok()));
    });
}
