use crate::cmd::{Out, Req};
use crate::conn::{Channel, Conn, Event, Hooks, Io, CLOSE_UNSUPPORTED};
use crate::endpoint::{Endpoint, Factory};
use crate::frame::{kind, write_frame, FrameReader, MAX_FRAME};
use crate::hello::{self, Hello};
use crate::link::{Client, Error, Target};
use crate::ring::{Cursor, Kind, Ring};
use crate::stream::{parse_chunk, read_varint, TAG_DROP};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(10);

fn dispatch(req: &Req, out: &mut Out) {
    match req.cmd.as_str() {
        "ping" => out.line("{\"pong\":1}"),
        "big" => {
            for i in 0..2000 {
                out.line(&format!("{{\"row\":{i}}}"));
            }
        }
        "boom" => panic!("a bug in a command"),
        c => out.err(&format!("unknown command `{c}`")),
    }
}

/// Echoes every message back on its channel.
fn echo() -> Factory {
    Arc::new(|_, open| {
        std::thread::spawn(move || {
            while let Some(ev) = open.channel.recv() {
                match ev {
                    Event::Data(d) => {
                        if open.channel.send(d).is_err() {
                            return;
                        }
                    }
                    Event::Closed(..) => return,
                    Event::Opened(_) => {}
                }
            }
        });
        Ok(())
    })
}

/// Connects a client `Conn` to `ep` over an in-process socket pair.
pub(crate) fn pair(ep: &Endpoint) -> Conn {
    let (a, b) = UnixStream::pair().unwrap();
    let ep = ep.clone();
    std::thread::spawn(move || {
        let (r, w, s) = (a.try_clone().unwrap(), a.try_clone().unwrap(), a);
        let _ = ep.serve(FrameReader::new(Box::new(r)), Box::new(w), Box::new(move || {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }));
    });
    let (r, w, s) = (b.try_clone().unwrap(), b.try_clone().unwrap(), b);
    let mut reader = FrameReader::new(Box::new(r));
    let mut writer: Box<dyn Write + Send> = Box::new(w);
    let ok = hello::client_handshake(&mut reader, &mut *writer, &Hello { name: "test".into(), link_id: "t".into(), ..Hello::default() }).unwrap();
    Conn::start(Io { reader, writer, shutdown: Box::new(move || {
        let _ = s.shutdown(std::net::Shutdown::Both);
    }) }, ok, true, None, Hooks::default())
}

fn data(ch: &Channel) -> Vec<u8> {
    match ch.recv_timeout(WAIT) {
        Ok(Event::Data(d)) => d,
        other => panic!("expected data, got {other:?}"),
    }
}

#[test]
fn frame_round_trip_and_limit() {
    let mut buf = Vec::new();
    write_frame(&mut buf, 7, kind::DATA, 1, b"hello").unwrap();
    write_frame(&mut buf, 9, kind::CLOSE, 0, b"").unwrap();
    let mut r = FrameReader::new(Box::new(std::io::Cursor::new(buf)));
    let f = r.next().unwrap();
    assert_eq!((f.chan, f.kind, f.flags, f.payload.as_slice()), (7, kind::DATA, 1, &b"hello"[..]));
    assert_eq!(r.next().unwrap().kind, kind::CLOSE);
    assert!(r.next().is_err());
    // a frame announcing more than 1 MiB is refused
    let mut big = ((MAX_FRAME + 1) as u32).to_le_bytes().to_vec();
    big.extend_from_slice(&[0; 16]);
    let e = FrameReader::new(Box::new(std::io::Cursor::new(big))).next().unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
    assert!(write_frame(&mut Vec::new(), 1, kind::DATA, 0, &vec![0; MAX_FRAME]).is_err());
}

#[test]
fn big_message_is_split_and_reassembled() {
    let ep = Endpoint::new("t");
    ep.register("echo", 1, echo());
    let conn = pair(&ep);
    let ch = conn.open("echo", "").unwrap();
    let msg: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    ch.send(msg.clone()).unwrap();
    assert!(data(&ch) == msg, "3 MB echoed back whole");
    assert_eq!(conn.peer().caps, vec!["echo/1".to_string()]);
}

#[test]
fn slow_reader_does_not_block_cmd() {
    let ep = Endpoint::new("t");
    ep.register_cmd(dispatch);
    let held: Arc<Mutex<Vec<Channel>>> = Arc::new(Mutex::new(Vec::new()));
    let h = held.clone();
    // a service that accepts the channel and never reads it
    ep.register("stream", 1, Arc::new(move |_, open| {
        h.lock().unwrap().push(open.channel);
        Ok(())
    }));
    let conn = pair(&ep);
    let slow = conn.open("stream", "").unwrap();
    let tx = slow.tx();
    let sender = std::thread::spawn(move || {
        // 4 MiB cannot be accepted by a receiver that never drains: this call blocks
        let mut sent = 0;
        while tx.send(vec![0u8; 64 << 10]).is_ok() {
            sent += 64 << 10;
            if sent > 4 << 20 {
                break;
            }
        }
        sent
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(!sender.is_finished(), "the starved channel is held by credit");
    let cmd = conn.open("cmd", "").unwrap();
    cmd.send(b"ping".to_vec()).unwrap();
    let t = std::time::Instant::now();
    let mut text = String::new();
    while !text.contains("\"end\"") {
        text.push_str(&String::from_utf8(data(&cmd)).unwrap());
    }
    assert!(text.contains("pong") && t.elapsed() < Duration::from_secs(5));
    // the receiver drains: the sender goes on
    let rx_side = held.lock().unwrap().pop().unwrap();
    let drain = std::thread::spawn(move || {
        let mut n = 0;
        while let Some(Event::Data(d)) = rx_side.recv() {
            n += d.len();
            if n >= 4 << 20 {
                break;
            }
        }
        n
    });
    assert!(sender.join().unwrap() > 0);
    assert!(drain.join().unwrap() >= 4 << 20);
}

#[test]
fn different_major_version_is_refused_with_both_versions() {
    let ep = Endpoint::new("t");
    let (a, b) = UnixStream::pair().unwrap();
    let (r, w) = (a.try_clone().unwrap(), a.try_clone().unwrap());
    let h = std::thread::spawn(move || ep.serve(FrameReader::new(Box::new(r)), Box::new(w), Box::new(|| {})).err().map(|e| e.to_string()));
    let mut p = vec![hello::OP_HELLO];
    p.extend_from_slice(b"major=9\nminor=2\nrole=client\nname=x\nlink_id=1\n");
    let mut w = b.try_clone().unwrap();
    write_frame(&mut w, 0, kind::DATA, 0, &p).unwrap();
    let mut rd = FrameReader::new(Box::new(b));
    let f = rd.next().unwrap();
    assert_eq!(f.payload[0], hello::OP_GOODBYE);
    let msg = String::from_utf8(f.payload[1..].to_vec()).unwrap();
    assert!(msg.contains("1.0") && msg.contains("9.2"), "{msg}");
    assert!(h.join().unwrap().unwrap().contains("9.2"));
}

#[test]
fn unknown_service_is_refused() {
    let ep = Endpoint::new("t");
    let conn = pair(&ep);
    match conn.open("nope", "") {
        Err((code, why)) => assert!(code == CLOSE_UNSUPPORTED && why.contains("nope"), "{why}"),
        Ok(_) => panic!("opened a service that does not exist"),
    }
}

#[test]
fn cmd_answers_end_and_truncation() {
    let ep = Endpoint::new("t");
    ep.register_cmd(dispatch);
    let conn = pair(&ep);
    let ask = |line: &str| -> Vec<String> {
        let ch = conn.open("cmd", "").unwrap();
        ch.send(line.as_bytes().to_vec()).unwrap();
        let mut lines = Vec::new();
        while !lines.last().is_some_and(|l: &String| l.starts_with("{\"end\":")) {
            lines.extend(String::from_utf8(data(&ch)).unwrap().lines().map(str::to_string));
        }
        lines
    };
    let l = ask("ping");
    assert_eq!(l[0], "{\"pong\":1}");
    assert!(l[1].starts_with("{\"end\":\"ping\"") && !l[1].contains("truncated"));
    let l = ask("big");
    assert!(l.last().unwrap().contains("\"truncated\":true") && l.len() < 2000);
    let l = ask("big full=1");
    assert_eq!(l.len(), 2001);
    let l = ask("boom");
    assert!(l[0].starts_with("{\"err\":") && l.last().unwrap().starts_with("{\"end\":\"boom\""));
    // several cmd channels at once
    let a = conn.open("cmd", "").unwrap();
    let b = conn.open("cmd", "").unwrap();
    a.send(b"ping".to_vec()).unwrap();
    b.send(b"ping".to_vec()).unwrap();
    assert!(String::from_utf8(data(&a)).unwrap().contains("pong") && String::from_utf8(data(&b)).unwrap().contains("pong"));
}

#[test]
fn ring_drops_oldest_and_resume_counts_the_loss() {
    let ring = Ring::new(1000);
    let chunk = [7u8; 88]; // 100 bytes with the record header: ten fit
    for _ in 0..25 {
        ring.push_stream(&chunk);
    }
    assert_eq!(ring.last_seq(Kind::Stream), 25);
    let mut cur = ring.after(Kind::Stream, 5);
    let r = ring.read(Kind::Stream, &mut cur, usize::MAX);
    assert_eq!(r.recs.first().map(|x| x.0), Some(16), "oldest still held");
    assert_eq!(r.lost, 10, "records 6..=15 were dropped");
    assert_eq!(r.recs.len(), 10);
    assert_eq!(cur.seq, 25);
    // a reader that is current loses nothing
    let mut cur = ring.after(Kind::Stream, 20);
    let r = ring.read(Kind::Stream, &mut cur, usize::MAX);
    assert_eq!((r.lost, r.recs.len()), (0, 5));
    // the wrap of the buffer keeps record contents intact
    for i in 0..40u8 {
        ring.push_stream(&[i; 61]);
    }
    let mut cur = Cursor { pos: 0, seq: 0 };
    let r = ring.read(Kind::Stream, &mut cur, usize::MAX);
    assert!(r.recs.iter().all(|(s, d)| d.iter().all(|b| *b == (*s - 26) as u8) && d.len() == 61));
    // watch keeps the last 256
    let w = Ring::new(1000);
    for i in 0..300u32 {
        w.push_watch(format!("{{\"n\":{i}}}").as_bytes());
    }
    let mut cur = w.after(Kind::Watch, 10);
    let r = w.read(Kind::Watch, &mut cur, usize::MAX);
    assert_eq!((r.lost, r.recs.len(), r.recs[0].0), (34, 256, 45));
}

#[test]
fn resume_replays_the_ring_with_a_drop_marker() {
    let ep = Endpoint::with_ring("t", 1000);
    let sink = ep.register_stream();
    let watch = ep.register_watch();
    for i in 0..25u8 {
        sink.push(1, &[i; 88 - 3]);
    }
    for i in 0..300 {
        watch.emit(&format!("{{\"n\":{i}}}"));
    }
    let conn = pair(&ep);
    conn.send_control(hello::encode_resume(&hello::Resume { link_id: "t".into(), seqs: vec![("stream".into(), 5), ("watch".into(), 10)] }));
    let s = conn.open("stream", "").unwrap();
    let first = data(&s);
    let seq = u64::from_le_bytes(first[..8].try_into().unwrap());
    let (tag, payload) = parse_chunk(&first[8..]).unwrap();
    assert_eq!(tag, TAG_DROP);
    assert_eq!(read_varint(payload).unwrap().0, 10, "chunks 6..=15");
    assert_eq!(seq, 15);
    let mut last = 0;
    while last < 25 {
        last = u64::from_le_bytes(data(&s)[..8].try_into().unwrap());
    }
    let w = conn.open("watch", "").unwrap();
    let first = data(&w);
    assert_eq!(u64::from_le_bytes(first[..8].try_into().unwrap()), 44);
    assert_eq!(&first[8..], b"{\"event\":\"drop\",\"lost\":34}");
    let next = data(&w);
    assert_eq!(u64::from_le_bytes(next[..8].try_into().unwrap()), 45);
}

pub(crate) fn shared() -> &'static (Endpoint, crate::watch::WatchSink) {
    static S: OnceLock<(Endpoint, crate::watch::WatchSink)> = OnceLock::new();
    S.get_or_init(|| {
        let ep = Endpoint::new("makepad-remote-test");
        ep.register_cmd(app_dispatch);
        ep.register("frames", 1, fake_frames());
        let w = ep.register_watch();
        ep.register_stream();
        ep.listen_unix(",\"extra\":\"x\"").unwrap();
        (ep, w)
    })
}

#[test]
fn unix_end_to_end_with_discovery_and_reconnect() {
    let (_, watch) = shared();
    let me = std::process::id();
    let app = crate::discovery::find(&me.to_string()).expect("discovery file");
    assert_eq!(app.app, "makepad-remote-test");
    assert!(app.serves("cmd") && app.serves("watch") && app.serves("stream"));
    assert_eq!(app.text("extra").as_deref(), Some("x"));
    let client = Client::connect("makepad-remote-test").unwrap();
    assert!(client.caps().contains(&"cmd/1".to_string()));
    let mut lines = client.cmd("ping").unwrap();
    assert!(lines.next().unwrap().unwrap().starts_with("{\"pong\":1"));
    assert!(lines.next().is_none() && lines.end().unwrap().starts_with("{\"end\":\"ping\""));
    match client.open("nope", "") {
        Err(Error::Refused(c, _)) => assert_eq!(c, CLOSE_UNSUPPORTED),
        _ => panic!("expected a refusal"),
    }
    // a feed continues after the link is dropped: nothing lost, nothing twice
    let mut feed = client.watch();
    assert!(feed.next(Duration::from_millis(100), 3).unwrap().is_none());
    watch.emit("{\"event\":\"a\"}");
    watch.emit("{\"event\":\"b\"}");
    let a = feed.next(WAIT, 3).unwrap().unwrap();
    let b = feed.next(WAIT, 3).unwrap().unwrap();
    assert_eq!(b.0, a.0 + 1);
    client.conn().unwrap().close("test drop");
    watch.emit("{\"event\":\"c\"}");
    watch.emit("{\"event\":\"d\"}");
    let c = feed.next(WAIT, 20).unwrap().unwrap();
    let d = feed.next(WAIT, 20).unwrap().unwrap();
    assert_eq!((c.0, d.0), (b.0 + 1, b.0 + 2));
    assert_eq!(c.1, b"{\"event\":\"c\"}");
    // an in-flight request fails with LinkLost
    let mut lines = client.cmd("ping").unwrap();
    client.conn().unwrap().close("drop again");
    let all: Vec<_> = lines.by_ref().collect();
    assert!(all.iter().all(|l| l.is_ok()) || all.iter().any(|l| matches!(l, Err(Error::LinkLost(_)))));
    let _ = Target::Command(Vec::new());
}

#[test]
fn tcp_loopback_with_tls_and_key_proof() {
    use makepad_network::tls::TlsIdentity;
    use makepad_network::tunnel::{ClientCredentials, Psk};
    let dir = std::env::temp_dir().join(format!("makepad-remote-tls-{}", std::process::id()));
    let identity = TlsIdentity::load_or_create(&dir, "makepad-remote-test").unwrap();
    let fp = identity.fingerprint;
    let key = Psk::generate().unwrap();
    let ep = Endpoint::new("tcp-test");
    ep.register_cmd(dispatch);
    let addr = crate::carrier::tcp::listen(&ep, "127.0.0.1:0", identity, { let k = key.clone(); Arc::new(move || vec![k.clone()]) }, None).unwrap();
    let creds = |k: &Psk| ClientCredentials { keys: vec![k.clone()], known_hosts: None, expect: Some(fp) };
    let client = Client::connect_tcp(&addr.to_string(), creds(&key)).unwrap();
    let mut lines = client.cmd("ping").unwrap();
    assert!(lines.next().unwrap().unwrap().starts_with("{\"pong\":1"));
    let big = client.cmd("big full=1").unwrap().count();
    assert_eq!(big, 2000);
    // a key the endpoint does not hold never gets a connection
    assert!(Client::connect_tcp(&addr.to_string(), creds(&Psk::generate().unwrap())).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Both ends send 8 MB at once over loopback TLS (32 channels, so far more is in flight than the
/// socket buffers hold) and both must receive all of it.
#[test]
fn tcp_both_ends_send_8mb_at_once() {
    use makepad_network::tls::TlsIdentity;
    use makepad_network::tunnel::{ClientCredentials, Psk};
    const CHANNELS: usize = 32;
    const PER_CHANNEL: usize = (8 << 20) / CHANNELS;
    fn blast(ch: Channel) -> std::thread::JoinHandle<usize> {
        let tx = ch.tx();
        std::thread::spawn(move || {
            let sender = std::thread::spawn(move || {
                let mut sent = 0;
                while sent < PER_CHANNEL {
                    tx.send(vec![0xA5u8; 32 << 10]).unwrap();
                    sent += 32 << 10;
                }
            });
            let mut got = 0;
            while got < PER_CHANNEL {
                match ch.recv_timeout(Duration::from_secs(60)) {
                    Ok(Event::Data(d)) => got += d.len(),
                    _ => break,
                }
            }
            sender.join().unwrap();
            got
        })
    }
    let dir = std::env::temp_dir().join(format!("makepad-remote-tls8-{}", std::process::id()));
    let identity = TlsIdentity::load_or_create(&dir, "makepad-remote-test").unwrap();
    let fp = identity.fingerprint;
    let key = Psk::generate().unwrap();
    let ep = Endpoint::new("tcp-blast");
    let server_side: Arc<Mutex<Vec<std::thread::JoinHandle<usize>>>> = Arc::new(Mutex::new(Vec::new()));
    let s2 = server_side.clone();
    ep.register("blast", 1, Arc::new(move |_, open| {
        s2.lock().unwrap().push(blast(open.channel));
        Ok(())
    }));
    let addr = crate::carrier::tcp::listen(&ep, "127.0.0.1:0", identity, { let k = key.clone(); Arc::new(move || vec![k.clone()]) }, None).unwrap();
    let creds = ClientCredentials { keys: vec![key], known_hosts: None, expect: Some(fp) };
    let client = Client::connect_tcp(&addr.to_string(), creds).unwrap();
    let t = std::time::Instant::now();
    let handles: Vec<_> = (0..CHANNELS).map(|_| blast(client.open("blast", "").unwrap())).collect();
    let got: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();
    assert_eq!(got, CHANNELS * PER_CHANNEL, "client received everything");
    let theirs: Vec<_> = std::mem::take(&mut *server_side.lock().unwrap());
    assert_eq!(theirs.len(), CHANNELS);
    let got: usize = theirs.into_iter().map(|h| h.join().unwrap()).sum();
    assert_eq!(got, CHANNELS * PER_CHANNEL, "server received everything");
    assert!(t.elapsed() < Duration::from_secs(60), "{:?}", t.elapsed());
    eprintln!("8 MB each way over TLS in {:?}", t.elapsed());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The dispatch of the shared endpoint: what an app answers for `makepad-remote app`.
fn app_dispatch(req: &Req, out: &mut Out) {
    match req.cmd.as_str() {
        "ping" => out.line(&format!("{{\"pong\":1,\"pid\":{},\"app\":\"makepad-remote-test\"}}", std::process::id())),
        "status" => out.line("{\"app\":\"x\",\"pid\":1,\"w\":[{\"i\":0,\"t\":\"say \\\"i\\\":9\",\"sz\":[4,3]},{\"i\":2,\"t\":\"b\"}],\"closed\":[{\"i\":7}]}"),
        "quit" => out.line("{\"ok\":1}"),
        "echo" => out.line(&format!("{{\"cmd\":{},\"pos\":{},\"t\":{}}}", crate::json_str(&req.cmd), req.pos.len(), crate::json_str(req.get("t").unwrap_or("")))),
        _ => dispatch(req, out),
    }
}

/// A `frames` service that sends `n` fake PNGs of `window` (4x3), or fails for window 99.
fn fake_frames() -> Factory {
    Arc::new(|_, open| {
        let arg = |k: &str| open.args.split_whitespace().find_map(|a| a.strip_prefix(k)?.strip_prefix('=')).map(str::to_string);
        let window: usize = arg("window").and_then(|v| v.parse().ok()).unwrap_or(1);
        let n: usize = if arg("once").is_some() { 1 } else { arg("n").and_then(|v| v.parse().ok()).unwrap_or(1) };
        std::thread::spawn(move || {
            let ch = open.channel;
            if window == 99 {
                ch.close(crate::conn::CLOSE_REFUSED, "no window 99");
                return;
            }
            for i in 0..n {
                let head = format!("{{\"seq\":{i},\"w\":4,\"h\":3,\"window\":{window},\"t_ns\":5,\"codec\":\"png\",\"capture_ms\":1.5,\"encode_ms\":0.5,\"capture_kind\":\"pixels\"}}");
                if ch.send(crate::frames::encode_record(&head, format!("PNG{window}-{i}").as_bytes())).is_err() {
                    return;
                }
            }
            ch.close(crate::conn::CLOSE_NORMAL, "");
        });
        Ok(())
    })
}

fn run_app(argv: &[&str]) -> (Vec<String>, bool) {
    shared();
    let client = Client::connect("makepad-remote-test").unwrap();
    let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    let mut lines = Vec::new();
    let err = crate::app::run(&client, &argv, &mut |l| {
        lines.push(l.to_string());
        true
    })
    .unwrap();
    (lines, err)
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("makepad-remote-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn frames_records_round_trip_and_reject_truncation() {
    let head = "{\"seq\":3,\"w\":4,\"codec\":\"png\",\"t\":\"a\\\"b\"}";
    let m = crate::frames::encode_record(head, &[1, 2, 3, 0, 255]);
    assert_eq!(&m[..4], &(head.len() as u32).to_le_bytes());
    let r = crate::frames::decode_record(&m).unwrap();
    assert_eq!(r.bytes, [1, 2, 3, 0, 255]);
    assert_eq!((r.num("seq"), r.num("w"), r.text("codec").as_deref(), r.text("t").as_deref()), (Some(3.0), Some(4.0), Some("png"), Some("a\"b")));
    assert!(r.num("missing").is_none());
    assert!(crate::frames::decode_record(&m[..2]).is_err());
    assert!(crate::frames::decode_record(&m[..20]).is_err());
    let empty = crate::frames::decode_record(&crate::frames::encode_record("{}", &[])).unwrap();
    assert!(empty.bytes.is_empty());
    // a record bigger than a frame is split and put back together by the channel
    let big = vec![7u8; 3 << 20];
    let rec = crate::frames::decode_record(&crate::frames::encode_record("{\"seq\":0}", &big)).unwrap();
    assert_eq!(rec.bytes.len(), big.len());
}

#[test]
fn frames_request_arguments_match_the_services_vocabulary() {
    use crate::frames::Request;
    assert_eq!(Request { window: Some(2), scale: Some(0.5), raw: false, codec: None, n: Some(1), fps: None, cap: false }.args(), "window=2 scale=0.5 codec=png once=1");
    assert_eq!(Request { window: None, scale: None, raw: true, codec: None, n: Some(8), fps: Some(20.0), cap: false }.args(), "codec=raw n=8 fps=20");
    assert_eq!(Request::default().args(), "codec=png");
}

#[test]
fn app_commands_split_words_from_arguments_and_keep_spaces() {
    let argv: Vec<String> = ["tweak", "apply", "path=a.b", "splash={padding: 20}", "--x=1", "=odd", "all"].iter().map(|s| s.to_string()).collect();
    let (words, kv) = crate::app::split_args(&argv);
    assert_eq!(words, ["tweak", "apply", "=odd", "all"]);
    assert_eq!(kv[1], ("splash".to_string(), "{padding: 20}".to_string()));
    assert_eq!(kv[2].0, "x");
    let line = crate::app::request_line(&words[..2], &kv);
    let req = Req::parse(&line);
    assert_eq!((req.cmd.as_str(), req.pos.as_slice()), ("tweak", &["apply".to_string()][..]));
    assert_eq!(req.get("splash"), Some("{padding: 20}"));
    // control characters survive the JSON request
    let req = Req::parse(&crate::app::request_line(&["text".to_string()], &[("t".to_string(), "a\tb\u{1b}c \"q\"".to_string())]));
    assert_eq!(req.get("t"), Some("a\tb\u{1b}c \"q\""));
    // a window id is told from a window title that mentions one
    assert_eq!(crate::app::window_ids("{\"app\":\"x\",\"w\":[{\"i\":0,\"t\":\"say \\\"i\\\":9\"},{\"i\":12}],\"closed\":[{\"i\":7}]}"), [0, 12]);
    assert!(crate::app::window_ids("{\"err\":\"x\"}").is_empty());
}

#[test]
fn app_cmd_goes_through_with_words_and_arguments() {
    let (lines, err) = run_app(&["echo", "one", "t=hello world"]);
    assert!(!err);
    assert_eq!(lines, ["{\"cmd\":\"echo\",\"pos\":1,\"t\":\"hello world\"}"]);
    let (lines, err) = run_app(&["nope"]);
    assert!(err && lines[0].contains("unknown command"));
}

#[test]
fn grab_writes_the_png_on_the_clients_side() {
    let dir = scratch("grab");
    let file = dir.join("one.png");
    let (lines, err) = run_app(&["grab", "w=2", &format!("out={}", file.display())]);
    assert!(!err, "{lines:?}");
    assert_eq!(std::fs::read(&file).unwrap(), b"PNG2-0");
    assert!(lines[0].contains("\"w\":2") && lines[0].contains("\"sz\":[4,3]") && lines[0].contains("\"capture_ms\":1.500") && lines[0].contains(&file.display().to_string()), "{lines:?}");
    // the default folder is the client's temp dir, app-pid named, numbered past what is there
    let (a, _) = run_app(&["grab"]);
    let (b, _) = run_app(&["tweak", "grab"]);
    let path = |l: &str| l.split("\"png\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
    let (pa, pb) = (path(&a[0]), path(&b[0]));
    assert!(pa.contains(&format!("makepad-remote-test-{}", std::process::id())) && pa.ends_with(".png"), "{pa}");
    assert_ne!(pa, pb);
    assert_eq!(std::fs::read(&pa).unwrap(), b"PNG1-0");
    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);
    let _ = std::fs::remove_dir(std::path::Path::new(&pa).parent().unwrap());
    // a failure on the app's side is an error line
    let (lines, err) = run_app(&["grab", "w=99"]);
    assert!(err && lines[0].contains("no window 99"), "{lines:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gseq_writes_n_frames_with_timings_and_validates() {
    let dir = scratch("gseq");
    let (lines, err) = run_app(&["gseq", "n=3", "every_ms=20", &format!("out={}", dir.display())]);
    assert!(!err, "{lines:?}");
    let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["grab-w1-00001.png", "grab-w1-00002.png", "grab-w1-00003.png"]);
    assert_eq!(std::fs::read(dir.join("grab-w1-00003.png")).unwrap(), b"PNG1-2");
    assert!(lines[0].contains("\"n\":3,\"every_ms\":20") && lines[0].contains("\"scheduled_ms\":40,\"pixels_ms\":41.500"), "{lines:?}");
    // later sequences continue the numbering
    let (_, err) = run_app(&["gseq", "n=2", &format!("out={}", dir.display())]);
    assert!(!err);
    assert!(dir.join("grab-w1-00005.png").exists());
    // too many frames, too fast, or a span over 60 s: refused before anything is asked of the app
    for bad in [&["gseq", "n=0"][..], &["gseq", "n=65"], &["gseq", "every_ms=7"], &["gseq", "n=64", "every_ms=1000"]] {
        let (lines, err) = run_app(bad);
        assert!(err && lines.len() == 1 && lines[0].starts_with("{\"err\":\"gseq "), "{bad:?}: {lines:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gq_grabs_every_window_then_quits() {
    let (lines, err) = run_app(&["gq"]);
    assert!(!err, "{lines:?}");
    assert!(lines[0].starts_with("{\"png\":[") && lines[0].ends_with("],\"quit\":1}"), "{lines:?}");
    // the two windows of `status` (0 and 2), not the title or the closed one
    assert_eq!(lines[0].matches("grab-w").count(), 2, "{lines:?}");
    let files: Vec<String> = lines[0].split('"').filter(|s| s.contains("grab-w")).map(str::to_string).collect();
    assert!(files[0].contains("grab-w0-") && files[1].contains("grab-w2-"));
    for f in files {
        let _ = std::fs::remove_file(&f);
        // best effort: the folder goes too once the other tests are done with it
        let _ = std::fs::remove_dir(std::path::Path::new(&f).parent().unwrap());
    }
}

#[test]
fn loopback_carrier_requires_its_token_and_end_lines_carry_host_fields() {
    let ep = Endpoint::new("loop-test");
    ep.register_cmd(dispatch);
    ep.set_end_fields(|s| s.push_str(",\"user_seq\":7"));
    let token = crate::carrier::loopback::new_token().unwrap();
    assert_eq!(token.len(), 64);
    let addr = crate::carrier::loopback::serve(&ep, &token).unwrap().to_string();
    let good = Client::new(Target::Loopback(addr.clone(), token.clone()), "good");
    let mut lines = good.cmd("ping").unwrap();
    assert_eq!(lines.next().unwrap().unwrap(), "{\"pong\":1}");
    assert!(lines.next().is_none());
    let end = lines.end().unwrap().to_string();
    assert!(end.starts_with("{\"end\":\"ping\"") && end.ends_with(",\"user_seq\":7}"), "{end}");
    for wrong in [String::new(), "0".repeat(64), token[..63].to_string()] {
        let bad = Client::new(Target::Loopback(addr.clone(), wrong), "bad");
        match bad.cmd("ping") {
            Err(Error::Refused(_, why)) => assert!(why.contains("token"), "{why}"),
            _ => panic!("a peer without the token must be refused"),
        }
    }
    assert!(crate::carrier::loopback::dial("10.1.2.3:80").is_err(), "only loopback is dialled");
}

#[test]
fn a_request_past_its_timeout_is_closed_and_the_connection_goes_on() {
    let ep = Endpoint::new("timeout-test");
    ep.register_cmd(|req, out| match req.cmd.as_str() {
        "slow" => {
            std::thread::sleep(Duration::from_millis(600));
            out.line("{\"late\":1}");
        }
        _ => out.line("{\"fast\":1}"),
    });
    let token = crate::carrier::loopback::new_token().unwrap();
    let addr = crate::carrier::loopback::serve(&ep, &token).unwrap().to_string();
    let client = Client::new(Target::Loopback(addr, token), "t");
    client.set_timeout(Some(Duration::from_millis(100)));
    let t = std::time::Instant::now();
    let mut lines = client.cmd("slow").unwrap();
    assert!(matches!(lines.next(), Some(Err(Error::Timeout))));
    assert!(t.elapsed() < Duration::from_millis(500));
    assert!(lines.next().is_none());
    // the same connection answers the next request in time
    let first = client.cmd("quick").unwrap().next().unwrap().unwrap();
    assert_eq!(first, "{\"fast\":1}");
    // no timeout set: it waits for the slow one
    client.set_timeout(None);
    assert_eq!(client.cmd("slow").unwrap().next().unwrap().unwrap(), "{\"late\":1}");
}
