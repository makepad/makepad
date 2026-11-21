#![cfg(not(target_arch = "wasm32"))]

use makepad_ai_hub::web_socket::HubWebSocket;
use makepad_network::tls::TlsServer;
use makepad_network::web_socket_parser::WebSocketParser;
use makepad_network::WsSend;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn hold_after_upgrade(mut stream: impl Read + Write, release: Receiver<()>) {
    let mut key = None;
    for line in BufReader::new(&mut stream).lines() {
        let line = line.unwrap();
        if line.is_empty() { break; }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("Sec-WebSocket-Key") { key = Some(value.trim().to_owned()); }
        }
    }
    let reply = WebSocketParser::create_upgrade_response(&key.expect("upgrade key"));
    stream.write_all(reply.as_bytes()).unwrap();
    stream.flush().unwrap();
    let _ = release.recv();
}

fn cut_stalled_write(tls: Option<TlsServer>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let scheme = if tls.is_some() { "https" } else { "http" };
    let base = format!("{scheme}://{}", listener.local_addr().unwrap());
    let (release, hold) = mpsc::channel();
    let peer = thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        match tls {
            Some(tls) => hold_after_upgrade(tls.accept(tcp).unwrap(), hold),
            None => hold_after_upgrade(tcp, hold),
        }
    });
    let (handle, closer) = mpsc::channel();
    let (started, writes) = mpsc::channel();
    let (failed, result) = mpsc::channel();
    let writer = thread::spawn(move || {
        let mut ws = HubWebSocket::open(&base, "/stream").unwrap();
        handle.send(ws.closer().unwrap()).unwrap();
        let frame = WsSend::Binary(vec![0; 32 * 1024]);
        for index in 0u64.. {
            started.send(index).unwrap();
            if let Err(error) = ws.send(frame.clone()) {
                failed.send((index, error)).unwrap();
                break;
            }
        }
    });

    let limit = Duration::from_secs(2);
    let closer = closer.recv_timeout(limit).expect("socket upgraded");
    let deadline = Instant::now() + limit;
    assert_eq!(writes.recv_timeout(limit).unwrap(), 0);
    let mut pending = writes.recv_timeout(limit).expect("at least one frame fit");
    loop {
        assert!(Instant::now() < deadline, "the unread peer never filled");
        match writes.recv_timeout(Duration::from_millis(75)) {
            Ok(next) => pending = next,
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => panic!("write failed before the cut"),
        }
    }
    // Hold the unread peer open until the very write that stalled has returned an error.
    assert!(matches!(result.try_recv(), Err(mpsc::TryRecvError::Empty)), "write failed before close");
    let (closed, cut) = mpsc::channel();
    let closing = thread::spawn(move || closed.send(closer.close()).unwrap());
    cut.recv_timeout(limit).expect("close blocked behind the writer").unwrap();
    let (index, error) = result.recv_timeout(limit).expect("close did not wake the writer");
    assert_eq!(index, pending, "a later write failed instead: {error}");
    drop(release);
    closing.join().unwrap();
    writer.join().unwrap();
    peer.join().unwrap();
}

#[test]
fn cross_thread_close_wakes_a_blocked_writer() {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let dir = std::env::temp_dir().join(format!("makepad-ws-close-{}-{}", std::process::id(), now.as_nanos()));
    std::fs::create_dir(&dir).unwrap();
    std::env::set_var("MAKEPAD_HOME", &dir);
    cut_stalled_write(None);

    // These fleet TLS backends expose the TCP socket under the session.
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios", target_os = "tvos", target_os = "windows"))]
    {
        use makepad_network::fleet::id::{self, Fleet, Me, Role, Root};

        let root = Root::generate().unwrap();
        let key = id::generate_key().unwrap();
        let cred = root.issue("test", Role::Node, id::now_secs() + 3600, None, &id::point_of(&key)).unwrap();
        let me = Me::new(key, cred).unwrap();
        let server = TlsServer::new(me.tls_identity("test").unwrap()).unwrap();
        id::install_local(me);
        id::install_fleet(Fleet::new(root.point(), Default::default()));
        cut_stalled_write(Some(server));
    }
    std::fs::remove_dir_all(dir).unwrap();
}
