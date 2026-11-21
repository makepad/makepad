use makepad_ai_hub::client::ContentProvider;
use makepad_ai_hub::fleet::BoxSnapshot;
use makepad_ai_hub::job::route_generation;
use makepad_ai_hub::protocol::{GenerateRequestJson, HealthJson, ModelInfoJson};
use makepad_ai_hub::registry::Domain;
use makepad_ai_hub::talk::check::Checker;
use makepad_ai_hub::talk::{Talk, TalkContext, TalkJob, TalkPoll, TalkStages, TalkTurn};
use makepad_ai_hub::web_socket::HubWebSocket;
use makepad_micro_serde::DeJson;
use makepad_network::web_socket_parser::WebSocketMessage as Frame;
use makepad_network::{start_http_server, HttpServer, HttpServerRequest, HttpServerResponse, WsMessage, WsSend};
use std::collections::VecDeque;
use std::sync::{mpsc, Arc};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

struct Script {
    base: String,
    release: Option<mpsc::Receiver<()>>,
    answers: usize,
    active: mpsc::Sender<()>,
    stopped: mpsc::Sender<()>,
    first_block: Arc<AtomicBool>,
    overlap: Arc<SpeechProgress>,
    interrupt: bool,
    job_stopped: mpsc::Sender<&'static str>,
}

#[derive(Default)]
struct SpeechProgress {
    second_started: AtomicBool,
    chat_advanced: AtomicBool,
}

struct OnCancel(mpsc::Sender<&'static str>, &'static str);

impl Drop for OnCancel {
    fn drop(&mut self) { let _ = self.0.send(self.1); }
}

impl TalkStages for Script {
    fn transcribe(&mut self, turn: &Arc<TalkTurn>, pcm: &[i16], _: u32) -> Result<String, String> {
        if pcm.is_empty() {
            self.active.send(()).unwrap();
            assert!(!turn.wait(Duration::from_secs(2)), "disconnect cancels and wakes the running job");
            self.stopped.send(()).unwrap();
            return Err("cancelled".into());
        }
        if pcm == &[1] { return Err("recognizer failed".into()); }
        if pcm == &[2] { return Ok("stream".into()); }
        if pcm == &[3] { return Ok("interrupt".into()); }
        // A hub's refusal as a stage's error carries it, with the address it came from.
        if pcm == &[4] { return Err("http error: https://192.0.2.1:8124/generate: http error: http 503: claude-off-lan: this hub's Claude for the LAN switch is off".into()); }
        if pcm == &[5] { return Ok("unlisted".into()); }
        assert_eq!(pcm, &[-32768, -1, 0, 32767], "both PCM blocks reached the job intact");
        if let Some(release) = self.release.take() {
            release.recv_timeout(Duration::from_secs(2)).expect("the second caller is waiting");
            let request = GenerateRequestJson { model: "scripted".into(), queue_policy: Some("queue".into()), ..Default::default() };
            let id = turn.provider(&self.base).request(Domain::Stt, &request).map_err(|e| e.to_string())?;
            assert_eq!(id, "accepted");
        }
        Ok("what time is it".into())
    }
    fn reply(&mut self, _: &Arc<TalkTurn>, heard: &str, model: Option<&str>, _: Option<&TalkContext>) -> Result<TalkJob<String>, String> {
        self.interrupt = heard == "interrupt";
        if heard == "unlisted" {
            let request = GenerateRequestJson { model: "absent".into(), ..Default::default() };
            return Err(route_generation(vec![text_node()], "text", &request).expect_err("no node lists the model"));
        }
        if heard == "what time is it" {
            assert_eq!(model, None, "a caller without a model keeps the hub's choice");
            return Ok(Box::new(|| Ok(TalkPoll::Done("Quarter past two.".into()))));
        }
        assert_eq!(model, Some("voice/β+1&2"), "each turn keeps its connection's decoded chat model");
        assert!(heard == "stream" || self.interrupt, "expected the streaming transcript");
        let first_block = self.first_block.clone();
        self.overlap = Arc::default();
        let overlap = self.overlap.clone();
        let cancel = self.interrupt.then(|| OnCancel(self.job_stopped.clone(), "chat"));
        let mut deltas = ["One.", " ", "Two th", "ree? ", "Fo"].into_iter().enumerate().peekable();
        Ok(Box::new(move || {
            if let Some(&(index, delta)) = deltas.peek() {
                if index >= 2 && !first_block.load(Ordering::Acquire) { return Ok(TalkPoll::Pending); }
                if index == 4 {
                    if !overlap.second_started.load(Ordering::Acquire) { return Ok(TalkPoll::Pending); }
                    overlap.chat_advanced.store(true, Ordering::Release);
                }
                let _ = deltas.next();
                Ok(TalkPoll::Delta(delta.into()))
            } else if cancel.is_some() {
                Ok(TalkPoll::Pending)
            } else {
                Ok(TalkPoll::Done("One. Two three? Four".into()))
            }
        }))
    }
    fn speak(&mut self, _: &Arc<TalkTurn>, sentence: &str) -> Result<TalkJob<Vec<u8>>, String> {
        let second = sentence == "Two three?";
        if second { self.overlap.second_started.store(true, Ordering::Release); }
        let (samples, value) = match sentence {
            "Quarter past two." => {
                self.answers += 1;
                (if self.answers == 3 { 48_000 } else { 1200 }, 16384)
            }
            "One." => (2400, 1000),
            "Two three?" if self.interrupt => {
                let pending = OnCancel(self.job_stopped.clone(), "speech");
                return Ok(Box::new(move || { let _pending = &pending; Ok(TalkPoll::Pending) }));
            }
            "Two three?" => (720, 2000),
            "Four" => (240, 3000),
            other => panic!("speech got an incomplete or repeated sentence: {other:?}"),
        };
        let overlap = self.overlap.clone();
        let mut wav = makepad_ai_hub::wav::encode_wav_pcm16_mono(&vec![value as f32 / 32767.0; samples], 24_000);
        Ok(Box::new(move || {
            // Sentence two cannot finish until chat has advanced during its synthesis.
            if second && !overlap.chat_advanced.load(Ordering::Acquire) { return Ok(TalkPoll::Pending); }
            Ok(TalkPoll::Done(std::mem::take(&mut wav)))
        }))
    }
}

struct Caller {
    ws: HubWebSocket,
    inbox: VecDeque<WsMessage>,
    check: Checker,
    blocks: Vec<usize>,
}

impl Caller {
    fn open(base: &str, path: &str, rate: u32) -> Self {
        let ws = HubWebSocket::open(base, path).unwrap();
        ws.set_read_timeout(Duration::from_secs(2));
        let mut caller = Self { ws, inbox: VecDeque::new(), check: Checker::default(), blocks: Vec::new() };
        caller.check.ready();
        caller.expect("status ready");
        caller.text(&format!("hello 1 {rate}"));
        caller
    }
    fn next(&mut self) -> WsMessage {
        while self.inbox.is_empty() {
            self.ws.read(|message| {
                let message = match message {
                    Frame::Text(text) => {
                        self.check.down(false, text.as_bytes()).unwrap();
                        WsMessage::Text(text.into())
                    }
                    Frame::Binary(bytes) => {
                        self.check.down(true, bytes).unwrap();
                        self.blocks.push((bytes.len() - 8) / 2);
                        WsMessage::Binary(std::mem::take(bytes))
                    }
                    _ => unreachable!(),
                };
                self.inbox.push_back(message);
            }).expect("a reply on the socket");
        }
        self.inbox.pop_front().unwrap()
    }
    fn expect(&mut self, text: &str) { assert_eq!(self.next(), WsMessage::Text(text.into())); }
    fn text(&mut self, text: &str) {
        self.check.up(false, text.as_bytes()).unwrap();
        self.ws.send(WsSend::Text(text.into())).unwrap();
    }
    fn take(&mut self, turn: u16) {
        self.blocks.clear();
        self.text(&format!("take {turn}"));
    }
    fn block(&mut self, turn: u16, offset: u32, samples: &[i16]) {
        let bytes = pcm(b'M', turn, offset, samples);
        self.check.up(true, &bytes).unwrap();
        self.ws.send(WsSend::Binary(bytes)).unwrap();
    }
    fn submit(&mut self, turn: u16) {
        self.take(turn);
        self.block(turn, 0, &[-32768, -1]);
        self.block(turn, 2, &[0, 32767]);
        self.text(&format!("end {turn} 4"));
    }
    fn streaming_take(&mut self, turn: u16, sample: i16, heard: &str) {
        self.take(turn);
        self.block(turn, 0, &[sample]);
        self.text(&format!("end {turn} 1"));
        self.expect("status hearing");
        self.expect(&format!("heard {heard}"));
        self.expect("status thinking");
        self.expect("say One.");
        self.expect("status speaking");
    }
    fn speech(&mut self, turn: u16, offset: u32, count: usize, value: i16) {
        let expected = pcm(b'A', turn, offset, &vec![value; count]);
        assert_eq!(self.next(), WsMessage::Binary(expected), "sentence audio and offset at sample {offset}");
    }
    fn reply(&mut self) {
        self.expect("heard what time is it");
        self.expect("status thinking");
        self.expect("say Quarter past two.");
        self.expect("status speaking");
    }
    fn finish(&mut self, turn: u16) {
        loop {
            let message = self.next();
            if let WsMessage::Binary(bytes) = &message {
                let pcm = &bytes[8..];
                assert!(pcm.chunks_exact(2).any(|s| i16::from_le_bytes([s[0], s[1]]) > 0), "speech carries the scripted WAV");
            }
            if let WsMessage::Text(s) = message {
                assert!(!s.starts_with("error "), "expected done, received {s}");
                if s.starts_with("done ") {
                    assert!(s.starts_with(&format!("done {turn} ")), "expected done for turn {turn}, received {s}");
                    break;
                }
            }
        }
        self.expect("status idle");
    }
}

fn pcm(magic: u8, turn: u16, offset: u32, samples: &[i16]) -> Vec<u8> {
    let mut bytes = vec![magic, 0];
    bytes.extend(turn.to_le_bytes());
    bytes.extend(offset.to_le_bytes());
    for x in samples { bytes.extend(x.to_le_bytes()); }
    bytes
}

#[test]
fn checker_tracks_both_directions_and_late_cancel_acknowledgements() {
    let mut check = Checker::default();
    let first = check.up(false, b"take 65535").unwrap_err();
    assert!(first.contains("up message 1") && first.contains("hello"), "{first}");
    check.up(false, b"hello 1 16000").unwrap();
    check.up(false, b"take 65535").unwrap();
    check.up(false, b"take 0").expect_err("the previous take is still recording");
    check.up(true, &pcm(b'M', u16::MAX, 0, &[1])).unwrap();
    check.up(false, b"end 65535 0").expect_err("one sample was recorded");
    check.up(false, b"end 65535 1").unwrap();
    check.down(false, b"status busy").expect_err("unknown status word");
    check.down(false, b"heard ").unwrap();
    check.down(false, b"heard again").expect_err("only one transcript");
    check.down(false, b"status ready").unwrap();
    check.down(true, &pcm(b'A', u16::MAX, 0, &[1])).expect_err("speech requires its first say");
    check.down(false, b"say a few words").unwrap();
    check.down(false, b"say ").unwrap();
    let block = pcm(b'A', u16::MAX, 0, &[1, 2]);
    check.down(true, &block[..9]).expect_err("odd PCM byte count");
    check.down(true, &block).unwrap();
    check.down(true, &block).expect_err("overlapping speech");
    check.down(false, b"say next sentence").unwrap();
    check.down(false, b"heard late").expect_err("transcript cannot follow speech");

    check.up(false, b"cancel 65535").unwrap();
    check.down(true, &pcm(b'A', u16::MAX, 2, &[3])).expect_err("speech after cancel");
    check.down(false, b"error 65535 late").expect_err("only done can acknowledge cancel");
    check.up(false, b"take 0").unwrap();
    check.down(false, b"done 65535 0").expect_err("the cancelled turn sent two samples");
    check.down(false, b"done 65535 2").unwrap();
    check.down(false, b"done 65535 2").expect_err("duplicate acknowledgement");
    check.up(false, b"end 0 0").unwrap();
    check.down(false, b"error 0 no speech").expect_err("a reason starts with its code");
    check.down(false, b"error 0 chat-failed: http error: https://192.0.2.1:8124/generate").expect_err("no URL reaches the device");
    check.down(false, b"error 0 chat-failed: refused by 192.0.2.1").expect_err("no address reaches the device");
    check.down(false, b"error 0 nothing-heard: nothing was heard").unwrap();
    check.down(false, b"heard late").expect_err("reply after error");
    check.ready();
    let reset = check.up(true, &block).unwrap_err();
    assert!(reset.contains("up message 1") && reset.contains("hello"), "{reset}");
    check.up(false, b"hello 1 8000").unwrap();
    check.up(false, b"take 65535").unwrap();
    let stale = check.down(false, b"done 65535 2").unwrap_err();
    assert!(stale.contains("down message 4") && stale.contains("end or cancel"), "{stale}");
}

/// A node on loopback with one ready text model, `available`.
fn text_node() -> BoxSnapshot {
    BoxSnapshot {
        base_url: "http://127.0.0.1".into(),
        health: Some(HealthJson::deserialize_json(r#"{"service":"makepad-asset-ai","version":"1","models_loaded":["available"],"jobs_pending":0}"#).unwrap()),
        models: vec![ModelInfoJson::deserialize_json(r#"{"id":"available","domain":"text","backend":"text","available":true,"gated":false,"state":"loaded"}"#).unwrap()],
    }
}

#[test]
fn an_unlisted_chat_model_is_refused_even_with_a_ready_text_model() {
    let node = text_node();
    let mut request = GenerateRequestJson { model: "available".into(), ..Default::default() };
    assert_eq!(route_generation(vec![node.clone()], "text", &request).expect("the listed model can serve chat").1, "available");
    request.model = "absent".into();
    let reason = route_generation(vec![node], "text", &request).expect_err("the ready model cannot substitute for the requested one");
    assert!(reason.contains("no node advertises") && reason.contains("absent"), "the refusal names the unlisted model: {reason}");
}

#[test]
fn spoken_turns_over_loopback() {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    let dir = std::env::temp_dir().join(format!("makepad-talk-{}-{}", std::process::id(), now.as_nanos()));
    std::fs::create_dir(&dir).unwrap();
    std::env::set_var("MAKEPAD_HOME", &dir);
    std::env::set_var("MAKEPAD_NETWORKED_AI_HUB", "0");
    let (tx, requests) = mpsc::channel();
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);
    let server = start_http_server(HttpServer {
        listen_address: addr,
        request: tx,
        post_max_size: 4096,
        post_max_size_overrides: Vec::new(),
        pre_admit_posts: false,
        client_ip_resolver: None,
        trusted_proxy: None,
        allowed_methods: None,
    }).unwrap();
    let (release, gate) = mpsc::channel();
    let base = format!("http://{addr}");
    let (active, running) = mpsc::channel();
    let (stopped, cancelled) = mpsc::channel();
    let first_block = Arc::new(AtomicBool::new(false));
    let (job_stopped, stops) = mpsc::channel();
    let script = Script { base: base.clone(), release: Some(gate), answers: 0, active, stopped,
        first_block: first_block.clone(), overlap: Arc::default(), interrupt: false, job_stopped };
    let route = std::thread::spawn(move || {
        let (mut talk, worker) = Talk::start(Box::new(script), None);
        let mut submissions = Vec::new();
        for mut request in requests {
            if let HttpServerRequest::Post { headers, body, response } = request {
                assert_eq!(headers.path, "/generate");
                let (status, json) = if submissions.is_empty() {
                    ("409 Conflict", r#"{"job_id":null,"error":"model unavailable: local-use: quiet"}"#)
                } else {
                    ("200 OK", r#"{"job_id":"accepted"}"#)
                };
                submissions.push(body);
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", json.len());
                response.send(HttpServerResponse::new(head, json.as_bytes().to_vec())).unwrap();
            } else {
                talk.handle(&mut request);
            }
        }
        assert_eq!(submissions.len(), 2, "retry only the refused submission");
        assert_eq!(submissions[0], submissions[1], "retry the original job");
        drop(talk);
        worker.join().unwrap();
    });
    let mut a = Caller::open(&base, "/talk", 11_025);
    let mut b = Caller::open(&base, "/talk?unused=1", 16_000);
    a.check.up(false, b"future 7").expect_err("unknown up verb");
    a.ws.send(WsSend::Text("future 7".into())).unwrap();
    a.check.up(true, b"X\0").expect_err("unknown up block");
    a.ws.send(WsSend::Binary(b"X\0".to_vec())).unwrap();
    a.submit(7);
    a.expect("status hearing");
    b.submit(7);
    b.expect("status waiting");
    release.send(()).unwrap();
    a.expect("status waiting");
    a.reply();
    a.finish(7);
    assert_eq!(a.blocks, [220, 221, 110], "20 ms boundaries at 11.025 kHz and the tail");
    b.expect("status hearing");
    b.reply();
    b.finish(7);
    assert_eq!(b.blocks, [320, 320, 160], "the second connection kept its own rate");

    a.take(8);
    let gap = pcm(b'M', 8, 1, &[0]);
    a.check.up(true, &gap).expect_err("missing microphone sample");
    a.ws.send(WsSend::Binary(gap)).unwrap();
    a.expect("error 8 gap: the take had a gap");
    a.expect("status idle");

    a.submit(9);
    a.expect("status hearing");
    a.reply();
    assert!(matches!(a.next(), WsMessage::Binary(_)));
    a.text("cancel 9");
    a.text("cancel 9");
    a.text("take 10");
    a.text("cancel 10");
    a.finish(9);
    let sent = a.blocks.iter().sum::<usize>();
    assert!(sent > 0 && sent < 22_050, "cancel ended speech mid-take");
    a.expect("done 10 0");
    a.expect("status idle");
    b.take(8);
    b.block(8, 0, &[1]);
    b.text("end 8 1");
    b.expect("status hearing");
    b.expect("error 8 stt-failed: speech recognition failed");
    b.expect("status idle");
    // The device hears the hub's reason, never the address the refusal came from.
    b.take(9);
    b.block(9, 0, &[4]);
    b.text("end 9 1");
    b.expect("status hearing");
    b.expect("error 9 claude-off-lan: Claude is off for this network");
    b.expect("status idle");
    b.take(10);
    b.block(10, 0, &[5]);
    b.text("end 10 1");
    b.expect("status hearing");
    b.expect("heard unlisted");
    b.expect("status thinking");
    b.expect("error 10 model-unlisted: no hub lists the chat model");
    b.expect("status idle");

    let mut stream = Caller::open(&base, "/talk?other=ignored&model=voice%2F%CE%B2%2B1%262", 24_000);
    stream.streaming_take(1, 2, "stream");
    stream.speech(1, 0, 480, 1000);
    first_block.store(true, Ordering::Release);
    for block in 1..5 { stream.speech(1, block * 480, 480, 1000); }
    stream.expect("say Two three?");
    stream.speech(1, 2400, 480, 2000);
    stream.speech(1, 2880, 240, 2000);
    stream.expect("say Four");
    stream.speech(1, 3120, 240, 3000);
    stream.expect("done 1 3360");
    stream.expect("status idle");

    first_block.store(false, Ordering::Release);
    stream.streaming_take(2, 3, "interrupt");
    stream.speech(2, 0, 480, 1000);
    first_block.store(true, Ordering::Release);
    for block in 1..5 { stream.speech(2, block * 480, 480, 1000); }
    stream.expect("say Two three?");
    stream.text("cancel 2");
    let mut stopped = [stops.recv_timeout(Duration::from_secs(2)).expect("first job stopped on cancel"),
        stops.recv_timeout(Duration::from_secs(2)).expect("second job stopped on cancel")];
    stopped.sort();
    assert_eq!(stopped, ["chat", "speech"], "cancel releases both unfinished jobs");
    stream.expect("done 2 2400");
    stream.expect("status idle");
    drop(stream);

    a.take(11);
    a.text("end 11 0");
    a.expect("status hearing");
    running.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(a);
    cancelled.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(b);
    server.stop();
    server.join().unwrap();
    route.join().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
