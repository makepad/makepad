//! Whatever a viewer holds down goes up when its session ends: when it closes the connection,
//! and when it falls silent (no close ever comes). An up whose down never came is not passed on.

use makepad_desk_link::server::{Input, Server, ServerConfig};
use makepad_desk_link::{self as proto, ClientMsg, GamepadState};
use makepad_network::fleet::id::{self, generate_key, now_secs, point_of, Me, Role, Root};
use makepad_network::SocketStream;
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Log(Arc<Mutex<Vec<String>>>);

impl Input for Log {
    fn pointer(&mut self, _x: u16, _y: u16) {}
    fn button(&mut self, down: bool, button: u8) {
        self.0.lock().unwrap().push(format!("button {button} {}", if down { "down" } else { "up" }));
    }
    fn scroll(&mut self, _dx: i32, _dy: i32) {}
    fn key(&mut self, down: bool, keycode: u32) {
        self.0.lock().unwrap().push(format!("key {keycode} {}", if down { "down" } else { "up" }));
    }
    fn gamepad(&mut self, _slot: u8, _state: &GamepadState) {}
    fn gamepad_gone(&mut self, _slot: u8) {}
}

fn wait_for(log: &Arc<Mutex<Vec<String>>>, want: &[&str], within: Duration) -> Vec<String> {
    let started = Instant::now();
    loop {
        let got = log.lock().unwrap().clone();
        if got.len() >= want.len() || started.elapsed() > within {
            return got;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_gone_viewer_lets_go_of_what_it_held() {
    let dir = std::env::temp_dir().join(format!("mk-desk-link-input-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var("MAKEPAD_FLEET_DIR", &dir);
    let root = Root::generate().unwrap();
    let key = generate_key().unwrap();
    let cred = root.issue("desk-node", Role::Node, now_secs() + 3600, None, &point_of(&key)).unwrap();
    id::write_key(&dir.join(id::ID_KEY), &key).unwrap();
    std::fs::write(dir.join(id::ID_CRED), cred.text()).unwrap();
    id::write_root_pub(&dir, &root.point()).unwrap();
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr: SocketAddr = probe.local_addr().unwrap();
    drop(probe);
    let log = Arc::new(Mutex::new(Vec::new()));
    let l = log.clone();
    let config = ServerConfig {
        addr,
        tls_name: "makepad desk link input test".into(),
        log: Arc::new(|t: &str| eprintln!("{t}")),
        clock: || 0,
        input: Arc::new(move || Ok(Box::new(Log(l.clone())) as Box<dyn Input>)),
    };
    let server = Server::start(config, Arc::new(|| {})).unwrap();
    let admin = {
        let key = generate_key().unwrap();
        let cred = root.issue("rik-mac", Role::Device, now_secs() + 3600, Some(IpAddr::from([127, 0, 0, 1])), &point_of(&key)).unwrap();
        Me::new(key, cred).unwrap()
    };
    let session = |msgs: &[ClientMsg]| {
        let (mut stream, der) = SocketStream::connect_capture("127.0.0.1", &addr.port().to_string()).unwrap();
        let authorization = admin.authorization(&makepad_network::tls::fingerprint(&der));
        let hello = ClientMsg::Hello { version: proto::VERSION, name: "test".into(), authorization, screen: -1, paused: true, caps: 0 };
        proto::write_all(&mut stream, &hello.encode()).unwrap();
        for m in msgs {
            proto::write_all(&mut stream, &m.encode()).unwrap();
        }
        stream
    };
    let held = [
        ClientMsg::Key { down: false, keycode: 99, mods: 0 }, // never down: not passed on
        ClientMsg::Key { down: true, keycode: 7, mods: 0 },
        ClientMsg::Key { down: true, keycode: 3, mods: 8 },
        ClientMsg::Button { down: true, button: 0 },
    ];
    let downs = ["key 7 down", "key 3 down", "button 0 down"];
    let ups = ["button 0 up", "key 3 up", "key 7 up"];

    // closed: the ups come at once
    let stream = session(&held);
    assert_eq!(wait_for(&log, &downs, Duration::from_secs(5)), downs);
    drop(stream);
    let all: Vec<&str> = downs.iter().chain(ups.iter()).copied().collect();
    assert_eq!(wait_for(&log, &all, Duration::from_secs(5)), all);

    // silent (connected, nothing sent, no close): the ups come after the silence limit
    log.lock().unwrap().clear();
    let started = Instant::now();
    let _stream = session(&held);
    let got = wait_for(&log, &all, Duration::from_secs(12));
    assert_eq!(got, all);
    assert!(started.elapsed() >= Duration::from_secs(5), "released only once silent");
    while server.rx.try_recv().is_ok() {}
    let _ = std::fs::remove_dir_all(&dir);
}
