//! The TLS front end to end on this OS's TLS stack: pinned TLS, fleet
//! credentials, route policy per role, identity stamping, the credential
//! guard in the HTTP client, and the auth-failure ban.

use makepad_ai_hub::fleet_auth::{pin_endpoint, Authority, Role, Trust};
use makepad_ai_hub::front::{start_front, FrontConfig};
use makepad_ai_hub::http_client::{http_fetch, BearerAuth, HttpClientRequest};
use makepad_network::tls::{TlsIdentity, TlsServer};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

/// Both tests set the process-wide credential variable: one at a time.
static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mk-front-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A stand-in node HTTP server: answers every request with the identity
/// headers the front stamped on it.
fn start_inner() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let stamped: Vec<&str> = head
                    .lines()
                    .filter(|l| l.to_ascii_lowercase().starts_with("x-makepad-") || l.to_ascii_lowercase().starts_with("authorization"))
                    .collect();
                let body = stamped.join("\n");
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            });
        }
    });
    addr
}

#[test]
fn front_end_to_end() {
    let _env = ENV.lock().unwrap_or_else(|p| p.into_inner());
    let dir = temp("e2e");
    let authority = Authority::create(&dir.join("authority")).unwrap();
    let point = makepad_network::tls::from_hex::<65>(&authority.public_hex()).unwrap();
    let trust = Arc::new(Trust::new(point, ["revoked-one".to_string()].into_iter().collect()));
    let identity = TlsIdentity::load_or_create(&dir.join("tls"), "test node").unwrap();
    let pin = identity.fingerprint;
    let inner = start_inner();
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let listen = probe.local_addr().unwrap();
    drop(probe);
    start_front(FrontConfig {
        listen,
        inner,
        tls: TlsServer::new(identity).unwrap(),
        trust,
        front_secret: "the-front-secret".into(),
        edge: false,
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(100));

    let expiry = makepad_ai_hub::fleet_auth::now_secs() + 3600;
    let lan = authority.issue("tester", Role::Lan, expiry).unwrap();
    let device = authority.issue("phone", Role::Device, expiry).unwrap();
    let revoked = authority.issue("revoked-one", Role::Lan, expiry).unwrap();

    // Before the endpoint is pinned, a fleet credential is never sent.
    let url = format!("https://{listen}/jobs");
    let mut request = HttpClientRequest::get(&url);
    request.bearer = Some(BearerAuth { token: &lan, host_suffix: "127.0.0.1" });
    let err = http_fetch(&request).err().expect("unpinned endpoint must be refused");
    assert!(err.to_string().contains("not a verified fleet node"), "{err}");

    pin_endpoint(&listen.to_string(), pin);
    let get = |path: &str, token: &str| {
        std::env::set_var("MAKEPAD_AI_HUB_TOKEN", token);
        let url = format!("https://{listen}{path}");
        let response = http_fetch(&HttpClientRequest::get(&url)).unwrap();
        let status = response.status;
        (status, String::from_utf8(response.read_body_to_vec(1 << 20).unwrap()).unwrap())
    };

    // A LAN credential reaches everything; the inner server sees who it is
    // and never sees the credential.
    let (status, body) = get("/jobs", &lan);
    assert_eq!(status, 200);
    assert!(body.contains("X-Makepad-Client: tester"), "{body}");
    assert!(body.contains("X-Makepad-Role: lan"));
    assert!(body.contains("X-Makepad-Front: the-front-secret"));
    assert!(!body.to_ascii_lowercase().contains("authorization"));

    // A device credential: its own job routes only.
    assert_eq!(get("/jobs", &device).0, 403);
    assert_eq!(get("/job/job-1", &device).0, 200);

    // Revoked and forged credentials.
    assert_eq!(get("/jobs", &revoked).0, 401);
    assert_eq!(get("/jobs", &lan.replace(".lan.", ".node.")).0, 401);

    // A wrong pin: the client refuses before sending anything.
    pin_endpoint(&listen.to_string(), [0u8; 32]);
    std::env::set_var("MAKEPAD_AI_HUB_TOKEN", &lan);
    let url = format!("https://{listen}/jobs");
    assert!(http_fetch(&HttpClientRequest::get(&url)).is_err());
    pin_endpoint(&listen.to_string(), pin);

    // Repeated failures from one address end in a ban: then even a good
    // credential gets no answer from that address for a while.
    for _ in 0..4 {
        std::env::set_var("MAKEPAD_AI_HUB_TOKEN", "mkc1.nobody.lan.1.00");
        let url = format!("https://{listen}/jobs");
        let _ = http_fetch(&HttpClientRequest::get(&url));
    }
    std::env::set_var("MAKEPAD_AI_HUB_TOKEN", &lan);
    let url = format!("https://{listen}/jobs");
    assert!(http_fetch(&HttpClientRequest::get(&url)).is_err(), "banned address still served");

    // Plain HTTP to the front gets nothing useful.
    let mut plain = TcpStream::connect(listen).unwrap();
    plain.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let _ = plain.write_all(b"GET /jobs HTTP/1.1\r\n\r\n");
    let mut out = Vec::new();
    let _ = plain.read_to_end(&mut out);
    assert!(!String::from_utf8_lossy(&out).contains("200 OK"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Realtime: a pinned `wss://` websocket through the front to the node's
/// real HTTP server, both directions.
#[test]
fn websocket_through_the_front() {
    let _env = ENV.lock().unwrap_or_else(|p| p.into_inner());
    use makepad_network::{start_http_server, HttpServer, HttpServerRequest, WebSocketMessage};
    use std::sync::mpsc;

    let dir = temp("ws");
    let authority = Authority::create(&dir.join("authority")).unwrap();
    let point = makepad_network::tls::from_hex::<65>(&authority.public_hex()).unwrap();
    let trust = Arc::new(Trust::new(point, Default::default()));
    let identity = TlsIdentity::load_or_create(&dir.join("tls"), "ws node").unwrap();
    let pin = identity.fingerprint;

    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let inner = probe.local_addr().unwrap();
    drop(probe);
    let (tx, rx) = mpsc::channel();
    start_http_server(HttpServer {
        listen_address: inner,
        request: tx,
        post_max_size: 1 << 20,
        post_max_size_overrides: Vec::new(),
        pre_admit_posts: false,
        client_ip_resolver: None,
        trusted_proxy: None,
        allowed_methods: None,
    })
    .unwrap();
    std::thread::spawn(move || {
        while let Ok(request) = rx.recv() {
            if let HttpServerRequest::BinaryMessage { response_sender, data, .. } = request {
                // The server frames what it is handed.
                let _ = response_sender.send(data.iter().rev().copied().collect());
            }
        }
    });
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let listen = probe.local_addr().unwrap();
    drop(probe);
    start_front(FrontConfig {
        listen,
        inner,
        tls: TlsServer::new(identity).unwrap(),
        trust,
        front_secret: "s".into(),
        edge: false,
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    pin_endpoint(&listen.to_string(), pin);
    let token = authority.issue("ws-tester", Role::Lan, makepad_ai_hub::fleet_auth::now_secs() + 600).unwrap();
    std::env::set_var("MAKEPAD_AI_HUB_TOKEN", &token);

    let (wtx, wrx) = mpsc::channel();
    let request = makepad_network::HttpRequest::new(format!("wss://{listen}/realtime/job-x"), makepad_network::HttpMethod::GET);
    let mut socket = makepad_network::plain_web_socket::PlainWebSocket::open(makepad_live_id::LiveId(0), request, wtx);
    for round in 0..3u8 {
        socket.send_message(WebSocketMessage::Binary(vec![round, 1, 2, 3])).unwrap();
        match wrx.recv_timeout(Duration::from_secs(5)) {
            Ok(WebSocketMessage::Binary(data)) => assert_eq!(data, vec![3, 2, 1, round]),
            other => panic!("expected an echo, got {other:?}"),
        }
    }
    socket.close();
    let _ = std::fs::remove_dir_all(&dir);
}
