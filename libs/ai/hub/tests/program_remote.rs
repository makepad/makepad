//! Compute programs on a node: a loopback node, its kernel stream and the remote session. What a
//! node sends back must be what the same program gives in-process, bit for bit.

use makepad_ai_hub::download::Downloader;
use makepad_ai_hub::kernel_client::{KernelNode, KernelStream};
use makepad_ai_hub::program_client::{RemoteConfig, RemoteSession};
use makepad_ai_hub::registry::Registry;
use makepad_ai_hub::server::{start_service, ServiceConfig};
use makepad_compute_program::codec::Codec;
use makepad_compute_program::{programs, Class, Limits, OutputData, Program, ProgramError, Session, SessionConfig, Updates, Value};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

/// One node for the test binary (the service holds a process lock); one test at a time.
fn node() -> (MutexGuard<'static, ()>, Arc<KernelStream>) {
    static TEST: Mutex<()> = Mutex::new(());
    let guard = TEST.lock().unwrap_or_else(|e| e.into_inner());
    static URL: OnceLock<String> = OnceLock::new();
    let url = URL.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("makepad-ai-hub-program-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let handle = start_service(ServiceConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            cache_dir: dir,
            registry: Registry::embedded().unwrap(),
            downloader: Downloader::new("http://127.0.0.1:1", None).unwrap(),
            peer: makepad_ai_hub::peer_serve::PeerOptions { serve: Some(false), sources: Some(Vec::new()), ..Default::default() },
            fleet: makepad_ai_hub::discovery::DEFAULT_FLEET.to_string(),
        })
        .unwrap();
        let url = format!("http://{}", handle.addr);
        std::mem::forget(handle);
        url
    });
    let stream = KernelStream::open(&KernelNode::new(url)).unwrap();
    (guard, Arc::new(stream))
}

fn n(x: f64) -> Value {
    Value::Number(x)
}

fn nums(xs: &[f64]) -> Value {
    Value::Array(xs.iter().map(|x| n(*x)).collect())
}

/// A fractal tile message: corner and texel step `at`, `res` texels a side.
fn tile(slot: u32, at: [f64; 3], res: f64) -> Value {
    Value::object(&[
        ("slot", n(slot as f64)),
        ("x", n(at[0])),
        ("y", n(at[1])),
        ("step", n(at[2])),
        ("res", n(res)),
        ("kind", n(0.0)),
        ("power", n(2.0)),
        ("max_iter", n(200.0)),
        ("de", n(1.0)),
        ("trap", n(0.0)),
        ("julia", nums(&[0.0, 0.0])),
    ])
}

/// The next `name` output past `version`.
fn next_output(mut take: impl FnMut() -> Updates, name: &str, version: u64) -> makepad_compute_program::Output {
    loop {
        let u = take();
        if let Some(Err(e)) = &u.end {
            panic!("the program ended: {e}");
        }
        if let Some(o) = u.outputs.into_iter().find(|o| o.name == name && o.version > version) {
            return o;
        }
    }
}

fn words(o: &makepad_compute_program::Output) -> Vec<u32> {
    match &o.data {
        OutputData::Words(w) => w.to_vec(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_remote_fractal_tile_is_the_in_process_tile() {
    let (_guard, stream) = node();
    let program = Program::new("fractal.splash", programs::FRACTAL);
    let views = [[-2.0, -1.25, 2.5 / 32.0], [-0.75, 0.05, 0.1 / 32.0]];
    for codec in [Codec::DeltaLz4, Codec::DeltaDeflate, Codec::Raw] {
        let remote = RemoteSession::open(&stream, &program, RemoteConfig { codec: Some(codec), class: Class::Interactive, ..RemoteConfig::default() }).unwrap();
        let device = remote.device();
        for (key, want) in [("kind", "cpu"), ("backend", "native"), ("class", "interactive")] {
            assert_eq!(device.get(key), Some(&Value::String(want.into())), "{device:?}");
        }
        // an interactive session takes the box where the node runs in a scope it can weigh
        assert!(matches!(device.get("weight"), Some(Value::Nil) | Some(Value::Number(10000.0))), "{device:?}");
        let local = Session::open(&program, SessionConfig::default());
        for (k, at) in views.iter().enumerate() {
            let name = ["tile0", "tile1"][k];
            let rev = remote.send("tile", tile(k as u32, *at, 32.0));
            local.send("tile", tile(k as u32, *at, 32.0));
            let theirs = next_output(|| remote.wait(), name, 0);
            let mine = next_output(|| local.wait(), name, 0);
            assert_eq!(theirs.rev, rev);
            assert_eq!(words(&theirs), words(&mine), "{codec:?}: tile {k}");
        }
        remote.send("stop", Value::Bool(true));
        assert_eq!(remote.wait_end(), Ok(n(2.0)));
        // The end stays: a later wait returns at once.
        assert!(remote.wait().is_empty());
    }
}

#[test]
fn a_bounded_wait_comes_back_from_a_silent_program_and_its_cancel_lands() {
    let (_guard, stream) = node();
    let silent = RemoteSession::open(&stream, &Program::new("t.splash", "{run: fn(s, p) { s.wait_inbox() return 1 }}"), RemoteConfig::default()).unwrap();
    assert!(silent.wait_within(std::time::Duration::from_millis(5)).is_empty());
    silent.cancel();
    assert_eq!(silent.wait_end(), Err(ProgramError::Cancelled));
    // ended: an hour's wait does not wait
    assert!(silent.wait_within(std::time::Duration::from_secs(3600)).is_empty());
}

/// An open on a node that takes the stream and then says nothing comes back once it goes stale;
/// the session the node opens after all is cancelled, as nobody will ever use it.
#[test]
fn an_open_goes_stale_on_a_silent_node_and_a_late_session_is_cancelled() {
    use makepad_compute_program::wire;
    use makepad_network::web_socket_parser::{WebSocketMessage, WebSocketMessageFormat, WebSocketMessageHeader, WebSocketParser};
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (gave_up, answer) = std::sync::mpsc::channel::<()>();
    let (told, cancelled) = std::sync::mpsc::channel::<u64>();
    std::thread::spawn(move || {
        let (mut link, _) = listener.accept().unwrap();
        let mut buf = vec![0u8; 1 << 16];
        let mut head = Vec::new();
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = link.read(&mut buf).unwrap();
            head.extend_from_slice(&buf[..n]);
        }
        link.write_all(b"HTTP/1.1 101 Switching Protocols\r\n\r\n").unwrap();
        let mut parser = WebSocketParser::new();
        let mut next = |link: &mut std::net::TcpStream, kind: u8| loop {
            let n = link.read(&mut buf).unwrap();
            let mut word = None;
            parser.parse(&buf[..n], |r| {
                if let Ok(WebSocketMessage::Binary(f)) = r {
                    if f[0] == kind {
                        word = Some(u64::from_le_bytes(f[1..9].try_into().unwrap()));
                    }
                }
            });
            if let Some(word) = word {
                return word;
            }
        };
        let request = next(&mut link, wire::OPEN);
        answer.recv().unwrap();
        let mut frame = vec![wire::OPENED];
        frame.extend_from_slice(&request.to_le_bytes());
        frame.extend_from_slice(&wire::encode_opened(Ok((77, Value::Nil))));
        let header = WebSocketMessageHeader::from_len(frame.len(), WebSocketMessageFormat::Binary, false);
        link.write_all(header.as_slice()).unwrap();
        link.write_all(&frame).unwrap();
        told.send(next(&mut link, wire::CANCEL)).unwrap();
    });
    let stream = Arc::new(KernelStream::open(&KernelNode::new(&url)).unwrap());
    let mut asked = 0;
    let opened = RemoteSession::open_unless(&stream, &Program::new("t.splash", "{run: fn(s, p) { return 1 }}"), RemoteConfig::default(), &mut || {
        asked += 1;
        asked == 2
    });
    assert!(matches!(opened, Ok(None)), "a stale open comes back with no session");
    gave_up.send(()).unwrap();
    assert_eq!(cancelled.recv_timeout(std::time::Duration::from_secs(5)), Ok(77));
}

#[test]
fn a_remote_program_ends_with_its_typed_error() {
    let (_guard, stream) = node();
    let open = |source: &str, limits: Limits| RemoteSession::open(&stream, &Program::new("t.splash", source), RemoteConfig { limits, ..RemoteConfig::default() }).unwrap();
    let small = Limits { fuel: 1_000_000, ..Limits::default() };
    assert_eq!(open("{run: fn(s, p) { var k = 0 loop { k = k + 1 } }}", small).wait_end(), Err(ProgramError::OverBudget("instructions")));
    match open("{run: fn(s, p) { return s.nothing() }}", Limits::default()).wait_end() {
        Err(ProgramError::Program(m)) => assert!(m.contains("nothing"), "{m}"),
        other => panic!("{other:?}"),
    }
    let waiting = open("{run: fn(s, p) { s.wait_inbox() return 1 }}", Limits::default());
    waiting.cancel();
    assert_eq!(waiting.wait_end(), Err(ProgramError::Cancelled));
    // The node's ceiling, not the client's ask: a refusal value the program sees.
    let big = open("use mod.pod.*\n{run: fn(s, p) { let b = s.buffer(f32, [32768, 16384, 3]) return b.refused }}", Limits { buffer_bytes: u64::MAX, ..Limits::default() });
    assert_eq!(big.wait_end(), Ok(Value::Bool(true)));
}

#[test]
fn a_slow_app_gets_the_newest_versions_not_a_backlog() {
    let (_guard, stream) = node();
    // 200 versions as fast as the program can publish, then it waits to be told to stop.
    let source = r#"
use mod.pod.*
{run: fn(s, p) {
    let b = s.buffer(f32, 1024)
    for k in 0..200 {
        s.fill(b, k)
        s.publish("level", b)
    }
    s.value("done", true)
    s.wait_inbox()
    return 1
}}
"#;
    let remote = RemoteSession::open(&stream, &Program::new("t.splash", source), RemoteConfig::default()).unwrap();
    let mut seen = Vec::new();
    let mut stopped = false;
    loop {
        let u = remote.wait();
        seen.extend(u.outputs.iter().map(|o| o.version));
        if !stopped && u.value("done").is_some() {
            remote.send("stop", Value::Bool(true));
            stopped = true;
        }
        if let Some(end) = u.end {
            assert_eq!(end, Ok(n(1.0)));
            break;
        }
    }
    // The newest version always arrives; the link carried far fewer than 200.
    assert_eq!(seen.iter().max(), Some(&200), "{seen:?}");
    assert!(seen.len() < 200, "every version came: {}", seen.len());
}

/// Opens in one slot: the second names the input the first sent by its hash and runs on the same
/// words; another slot's open of other words, then the first slot's again, run on theirs.
#[test]
fn an_open_in_a_slot_runs_on_the_inputs_kept_there() {
    let (_guard, stream) = node();
    let source = "{run: fn(s, p) { let b = s.input(\"big\") return s.read(b, p.at, 1)[0] }}";
    let program = Program::new("t.splash", source);
    let big = |offset: u32| makepad_compute_program::Input::words("big", makepad_compute_program::ElemTy::U32, (0..1u32 << 20).map(|k| k + offset).collect::<Vec<_>>().into());
    let run = |offset: u32, keep: &str, at: f64| {
        let config = RemoteConfig { params: Value::object(&[("at", n(at))]), inputs: vec![big(offset)], keep: keep.into(), ..RemoteConfig::default() };
        RemoteSession::open(&stream, &program, config).unwrap().wait_end()
    };
    assert_eq!(run(0, "step", 5.0), Ok(n(5.0)));
    assert_eq!(run(0, "step", 999_999.0), Ok(n(999_999.0)));
    assert_eq!(run(7, "other", 5.0), Ok(n(12.0)));
    assert_eq!(run(0, "step", 6.0), Ok(n(6.0)));
    assert_eq!(run(7, "", 6.0), Ok(n(13.0)), "an open in no slot names what the slots keep, and keeps nothing");
}
