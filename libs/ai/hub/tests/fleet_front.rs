//! The TLS front end to end on this OS's TLS stack: servers proven by their
//! certificate, client proofs per machine credential, the route policy per
//! peer (admin from its bound address, the same credential elsewhere, node,
//! paired), identity stamping, refusals and the auth-failure ban.

use makepad_ai_hub::http_client::{http_fetch, HttpClientRequest};
use makepad_network::fleet::front::{start_front, FrontConfig};
use makepad_network::fleet::id::{self, generate_key, mark_fleet_endpoint, now_secs, point_of, Fleet, Me, Role, Root, Verifier};
use makepad_network::tls::{TlsIdentity, TlsServer};
use makepad_network::SocketStream;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// This test binary's fleet: one root, a revoked name, and this process
/// installed as the admin Mac bound to loopback (the identity and fleet are
/// process-global: first install wins, so every test shares this one).
struct Fixture {
    root: Root,
    fleet: Fleet,
}

const LOOPBACK: [u8; 4] = [127, 0, 0, 1];

fn fixture() -> &'static Fixture {
    static FIXTURE: OnceLock<Fixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let root = Root::generate().unwrap();
        let fleet = Fleet::new(root.point(), ["revoked-one".to_string()].into_iter().collect());
        id::install_fleet(fleet.clone());
        id::install_local(machine(&root, "rik-mac", Role::Device, Some(IpAddr::from(LOOPBACK))));
        Fixture { root, fleet }
    })
}

fn machine(root: &Root, name: &str, role: Role, from: Option<IpAddr>) -> Me {
    let key = generate_key().unwrap();
    let cred = root.issue(name, role, now_secs() + 3600, from, &point_of(&key)).unwrap();
    Me::new(key, cred).unwrap()
}

/// A front for `inner`, its certificate made from a node credential of `root`;
/// returns its address and the machine it runs on.
fn front(root: &Root, fleet: &Fleet, name: &str, inner: SocketAddr) -> (SocketAddr, Me) {
    let server = machine(root, name, Role::Node, None);
    let identity: TlsIdentity = server.tls_identity(&format!("test {name}")).unwrap();
    let verifier = Arc::new(Verifier::new(fleet.clone(), identity.fingerprint, &server.cred));
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let listen = probe.local_addr().unwrap();
    drop(probe);
    start_front(FrontConfig {
        listen,
        inner: Arc::new(move |_: &str| inner),
        inner_headers: Vec::new(),
        tls: TlsServer::new(identity).unwrap(),
        verifier,
        front_secret: "the-front-secret".into(),
        policy: Arc::new(|peer, method: &str, path: &str| makepad_ai_hub::fleet_routes::route_allowed(peer, false, method, path)),
        max_total: makepad_network::fleet::front::LAN_MAX_TOTAL,
        max_per_ip: makepad_network::fleet::front::LAN_MAX_PER_IP,
        label: "front",
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    (listen, server)
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

/// `GET path` as the machine `me` (its own proof for the certificate the
/// front presents), from loopback: `(status, body)`, or None when the front
/// answers nothing (a banned address).
fn get_as(me: &Me, listen: SocketAddr, path: &str) -> Option<(u16, String)> {
    get_with(listen, path, |der| me.authorization(&makepad_network::tls::fingerprint(der)))
}

fn get_with(listen: SocketAddr, path: &str, authorization: impl Fn(&[u8]) -> String) -> Option<(u16, String)> {
    let (mut s, der) = SocketStream::connect_capture("127.0.0.1", &listen.port().to_string()).ok()?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let auth = authorization(&der);
    write!(s, "GET {path} HTTP/1.1\r\nHost: {listen}\r\nAuthorization: {auth}\r\nConnection: close\r\n\r\n").ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..end]).to_string();
            let length: usize = head
                .lines()
                .find_map(|l| l.split_once(':').filter(|(n, _)| n.eq_ignore_ascii_case("content-length")).map(|(_, v)| v.trim().parse().unwrap_or(0)))
                .unwrap_or(0);
            if buf.len() >= end + 4 + length {
                let status = head.split(' ').nth(1)?.parse().ok()?;
                return Some((status, String::from_utf8_lossy(&buf[end + 4..end + 4 + length]).to_string()));
            }
        }
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

#[test]
fn front_end_to_end() {
    let fx = fixture();
    let (listen, this_node) = front(&fx.root, &fx.fleet, "test-node", start_inner());

    // Not yet a fleet endpoint: the client does not speak fleet TLS to it.
    let url = format!("https://{listen}/jobs");
    assert!(http_fetch(&HttpClientRequest::get(&url)).is_err());

    // This process (the admin Mac's credential, bound to loopback) through
    // the HTTP client: the server is proven by its certificate, the inner
    // server sees who called and never the credential.
    mark_fleet_endpoint(&listen.to_string());
    let response = http_fetch(&HttpClientRequest::get(&format!("https://{listen}/control"))).unwrap();
    assert_eq!(response.status, 200);
    let body = String::from_utf8(response.read_body_to_vec(1 << 20).unwrap()).unwrap();
    assert!(body.contains("X-Makepad-Client: rik-mac"), "{body}");
    assert!(body.contains("X-Makepad-Role: device"), "{body}");
    assert!(body.contains("X-Makepad-Admin: 1"), "{body}");
    assert!(body.contains("X-Makepad-Front: the-front-secret"));
    assert!(!body.to_ascii_lowercase().contains("authorization"));
    assert_eq!(id::server_name(&listen.to_string()).as_deref(), Some("test-node"));

    // The admin credential from its bound address reaches control.
    let admin = machine(&fx.root, "admin-mac", Role::Device, Some(IpAddr::from(LOOPBACK)));
    assert_eq!(get_as(&admin, listen, "/control").unwrap().0, 200);
    // The same kind of credential bound to another address: no control,
    // but the fleet-machine routes.
    let elsewhere = machine(&fx.root, "mac-away", Role::Device, Some(IpAddr::from([10, 9, 9, 9])));
    assert_eq!(get_as(&elsewhere, listen, "/control").unwrap().0, 403);
    let (status, body) = get_as(&elsewhere, listen, "/job/job-1").unwrap();
    assert_eq!(status, 200);
    assert!(body.contains("X-Makepad-Admin: 0"), "{body}");
    assert_eq!(get_as(&elsewhere, listen, "/jobs").unwrap().0, 200);
    // A node: job routes and kernels, not control.
    let node = machine(&fx.root, "n217", Role::Node, None);
    let (status, body) = get_as(&node, listen, "/job/job-1").unwrap();
    assert_eq!(status, 200);
    assert!(body.contains("X-Makepad-Role: node"), "{body}");
    assert_eq!(get_as(&node, listen, "/kernel").unwrap().0, 200);
    assert_eq!(get_as(&node, listen, "/control").unwrap().0, 403);
    assert_eq!(get_as(&node, listen, "/control/watch?since=0").unwrap().0, 403);
    // This machine's own WM (the hub's own credential): its machine-local
    // control (the panel's picture and watch, local use, yield, models), no
    // admin settings or panel pages.
    let (status, body) = get_as(&this_node, listen, "/control").unwrap();
    assert_eq!(status, 200);
    assert!(body.contains("X-Makepad-This-Machine: 1") && body.contains("X-Makepad-Admin: 0"), "{body}");
    assert_eq!(get_as(&this_node, listen, "/control/watch?since=0").unwrap().0, 200);
    assert_eq!(get_as(&this_node, listen, "/llm/context").unwrap().0, 403);
    assert_eq!(get_as(&this_node, listen, "/index.html").unwrap().0, 403);
    // Another node's stamp never says this machine.
    assert!(get_as(&node, listen, "/job/job-1").unwrap().1.contains("X-Makepad-This-Machine: 0"));
    // A paired phone: its job routes (the hub checks ownership), nothing else.
    let phone = machine(&fx.root, "phone", Role::Paired, None);
    assert_eq!(get_as(&phone, listen, "/job/job-1").unwrap().0, 200);
    assert_eq!(get_as(&phone, listen, "/jobs").unwrap().0, 403);
    assert_eq!(get_as(&phone, listen, "/control").unwrap().0, 403);

    // Another root, a revoked name, a tampered role: 401.
    let stranger = machine(&Root::generate().unwrap(), "n217", Role::Node, None);
    assert_eq!(get_as(&stranger, listen, "/jobs").unwrap().0, 401);
    let revoked = machine(&fx.root, "revoked-one", Role::Node, None);
    assert_eq!(get_as(&revoked, listen, "/jobs").unwrap().0, 401);
    let promoted = get_with(listen, "/control", |der| node.authorization(&makepad_network::tls::fingerprint(der)).replace(".node.", ".device."));
    assert_eq!(promoted.unwrap().0, 401);

    // Old plain-HTTP clients are not credential guesses: no ban.
    for _ in 0..8 {
        let mut plain = TcpStream::connect(listen).unwrap();
        let _ = plain.write_all(b"GET /health HTTP/1.1\r\n\r\n");
        let mut sink = Vec::new();
        let _ = plain.read_to_end(&mut sink);
        let out = String::from_utf8_lossy(&sink);
        assert!(out.starts_with("HTTP/1.1 400") && out.contains("https only"), "{out}");
    }
    assert_eq!(get_as(&node, listen, "/jobs").unwrap().0, 200);

    // Repeated failures from one address end in a ban: then even a good
    // credential gets no answer from that address for a while.
    for _ in 0..4 {
        let _ = get_as(&stranger, listen, "/jobs");
    }
    assert!(get_as(&node, listen, "/jobs").is_none(), "banned address still served");
    assert!(http_fetch(&HttpClientRequest::get(&url)).is_err(), "banned address still served");
}

/// A server whose certificate is not of this fleet is refused by the
/// client before it sends anything.
#[test]
fn clients_refuse_a_server_of_another_fleet() {
    fixture();
    let other = Root::generate().unwrap();
    let other_fleet = Fleet::new(other.point(), Default::default());
    let (listen, _) = front(&other, &other_fleet, "impostor", start_inner());
    let err = id::connect("127.0.0.1", &listen.port().to_string()).err().expect("a foreign server is refused");
    assert!(err.to_string().contains("not a fleet server"), "{err}");
    mark_fleet_endpoint(&listen.to_string());
    assert!(http_fetch(&HttpClientRequest::get(&format!("https://{listen}/jobs"))).is_err());
}

/// Realtime: a `wss://` websocket through the front to the node's real HTTP
/// server, both directions.
#[test]
fn websocket_through_the_front() {
    use makepad_network::{start_http_server, HttpServer, HttpServerRequest, WebSocketMessage};
    use std::sync::mpsc;

    let fx = fixture();
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
    let (listen, _) = front(&fx.root, &fx.fleet, "ws-node", inner);
    mark_fleet_endpoint(&listen.to_string());

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
}
