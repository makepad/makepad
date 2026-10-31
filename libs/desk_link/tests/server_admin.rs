//! Desk Link's server on the fleet identity: it refuses to start without one,
//! proves itself with its fleet certificate, and lets only the admin (the
//! device credential from its bound address) view; anyone else is refused
//! with the reason.

use makepad_desk_link::server::{Server, ServerConfig, ToUi};
use makepad_desk_link::{self as proto, ClientMsg, Inbox, ServerMsg};
use makepad_network::fleet::id::{self, generate_key, now_secs, point_of, Fleet, Me, Role, Root};
use makepad_network::SocketStream;
use std::io::Read;
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn machine(root: &Root, name: &str, role: Role, from: Option<IpAddr>) -> Me {
    let key = generate_key().unwrap();
    let cred = root.issue(name, role, now_secs() + 3600, from, &point_of(&key)).unwrap();
    Me::new(key, cred).unwrap()
}

fn config(addr: SocketAddr) -> ServerConfig {
    ServerConfig {
        addr,
        tls_name: "makepad desk link test".into(),
        log: Arc::new(|t: &str| eprintln!("{t}")),
        clock: || 0,
        input: Arc::new(|| Err("view only in this test".to_string())),
    }
}

/// Hello as `me`; the server's first answer within `wait` (None: none).
fn hello_within(addr: SocketAddr, me: &Me, wait: Duration) -> Option<ServerMsg> {
    let (mut stream, der) = SocketStream::connect_capture("127.0.0.1", &addr.port().to_string()).unwrap();
    let authorization = me.authorization(&makepad_network::tls::fingerprint(&der));
    let hello = ClientMsg::Hello { version: proto::VERSION, name: "test".into(), authorization, screen: -1, paused: true, caps: 0 };
    proto::write_all(&mut stream, &hello.encode()).unwrap();
    stream.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
    let (mut inbox, mut buf, started) = (Inbox::default(), vec![0u8; 1 << 16], Instant::now());
    while started.elapsed() < wait {
        if let Some((ty, payload)) = inbox.next().unwrap() {
            return ServerMsg::decode(ty, &payload).ok();
        }
        match stream.read(&mut buf) {
            Ok(0) => return None,
            Ok(n) => inbox.feed(&buf[..n]),
            Err(_) => {}
        }
    }
    None
}

fn hello(addr: SocketAddr, me: &Me) -> Option<ServerMsg> {
    hello_within(addr, me, Duration::from_secs(5))
}

#[test]
fn only_the_admin_views_a_desktop() {
    let dir = std::env::temp_dir().join(format!("mk-desk-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var("MAKEPAD_FLEET_DIR", &dir);
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);

    // No identity: it does not listen, and says why.
    let err = Server::start(config(addr), Arc::new(|| {})).err().expect("no identity, no server");
    assert!(err.contains("no fleet identity") && err.contains("fleet-identity.md"), "{err}");

    // This desktop's identity: a node credential of a test root.
    let root = Root::generate().unwrap();
    let key = generate_key().unwrap();
    let cred = root.issue("desk-node", Role::Node, now_secs() + 3600, None, &point_of(&key)).unwrap();
    id::write_key(&dir.join(id::ID_KEY), &key).unwrap();
    std::fs::write(dir.join(id::ID_CRED), cred.text()).unwrap();
    id::write_root_pub(&dir, &root.point()).unwrap();
    let server = Server::start(config(addr), Arc::new(|| {})).unwrap();
    assert_eq!(server.name, "desk-node");

    // The admin from its bound address views.
    let loopback = IpAddr::from([127, 0, 0, 1]);
    let admin = machine(&root, "rik-mac", Role::Device, Some(loopback));
    // (its Welcome comes from the desktop's owner, which this test is: nothing to wait for)
    assert!(!matches!(hello_within(addr, &admin, Duration::from_millis(500)), Some(ServerMsg::Refused { .. })), "the admin is refused");
    let deadline = Instant::now() + Duration::from_secs(5);
    let client = loop {
        match server.rx.recv_timeout(Duration::from_millis(100)) {
            Ok(ToUi::Opened { client, .. }) => break client,
            Ok(_) => {}
            Err(_) if Instant::now() < deadline => {}
            Err(e) => panic!("no session opened: {e}"),
        }
    };
    assert_eq!(client, "rik-mac");

    // The same kind of credential bound to another address, a node, another fleet: refused, saying why.
    let away = machine(&root, "mac-away", Role::Device, Some(IpAddr::from([10, 9, 9, 9])));
    match hello(addr, &away) {
        Some(ServerMsg::Refused { reason }) => assert!(reason.contains("not its bound address"), "{reason}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    match hello(addr, &machine(&root, "n217", Role::Node, None)) {
        Some(ServerMsg::Refused { reason }) => assert!(reason.contains("not the admin credential"), "{reason}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    match hello(addr, &machine(&Root::generate().unwrap(), "rik-mac", Role::Device, Some(loopback))) {
        Some(ServerMsg::Refused { reason }) => assert!(reason.contains("not a credential of this fleet"), "{reason}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    // The viewer side: the desktop's certificate checks out against the root.
    let (_, der) = SocketStream::connect_capture("127.0.0.1", &addr.port().to_string()).unwrap();
    let fleet = Fleet::new(root.point(), Default::default());
    assert_eq!(fleet.check_server(&der, loopback, now_secs()).unwrap().name, "desk-node");
    let _ = std::fs::remove_dir_all(&dir);
}
