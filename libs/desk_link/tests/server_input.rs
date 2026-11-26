//! Whatever a viewer holds down goes up when its session ends: when it closes the connection,
//! and when it falls silent (no close ever comes). An up whose down never came is not passed on.

use makepad_desk_link::server::{Control, Input, Out, Server, ServerConfig, ToUi};
use makepad_desk_link::{self as proto, ClientMsg, GamepadState, Inbox, ServerMsg};
use makepad_network::fleet::id::{self, generate_key, now_secs, point_of, Me, Role, Root};
use makepad_network::SocketStream;
use std::io::Read;
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Log(Arc<Mutex<Vec<String>>>);

impl Input for Log {
    fn pointer(&mut self, _x: u16, _y: u16) {
        self.0.lock().unwrap().push("pointer".into());
    }
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

fn input_server(desktop_caps: u32) -> (Server, Me, Arc<Mutex<Vec<String>>>) {
    // Starting a server reads the process-wide fleet directory.
    static SETUP: Mutex<()> = Mutex::new(());
    let _setup = SETUP.lock().unwrap();
    let dir = std::env::temp_dir().join(format!("mk-desk-link-input-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
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
        desktop_caps,
        log: Arc::new(|t: &str| eprintln!("{t}")),
        clock: || 0,
        input: Arc::new(move || Ok(Box::new(Log(l.clone())) as Box<dyn Input>)),
    };
    let previous = std::env::var_os("MAKEPAD_FLEET_DIR");
    std::env::set_var("MAKEPAD_FLEET_DIR", &dir);
    let server = Server::start(config, Arc::new(|| {}));
    match previous {
        Some(path) => std::env::set_var("MAKEPAD_FLEET_DIR", path),
        None => std::env::remove_var("MAKEPAD_FLEET_DIR"),
    }
    std::fs::remove_dir_all(&dir).unwrap();
    let admin = {
        let key = generate_key().unwrap();
        let cred = root.issue("my-mac", Role::Device, now_secs() + 3600, Some(IpAddr::from([127, 0, 0, 1])), &point_of(&key)).unwrap();
        Me::new(key, cred).unwrap()
    };
    (server.unwrap(), admin, log)
}

fn connect(addr: SocketAddr, admin: &Me, caps: u32) -> SocketStream {
    let (mut stream, der) = SocketStream::connect_capture("127.0.0.1", &addr.port().to_string()).unwrap();
    let authorization = admin.authorization(&makepad_network::tls::fingerprint(&der));
    let hello = ClientMsg::Hello { version: proto::VERSION, name: "test".into(), authorization, screen: -1, paused: true, caps };
    proto::write_all(&mut stream, &hello.encode()).unwrap();
    stream
}

#[test]
fn a_gone_viewer_lets_go_of_what_it_held() {
    let (server, admin, log) = input_server(0);
    let session = |msgs: &[ClientMsg]| {
        let mut stream = connect(server.addr, &admin, 0);
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

    // a drag: press, moves and release reach the input in the order they were sent, none coalesced
    log.lock().unwrap().clear();
    let drag = [
        ClientMsg::Pointer { x: 100, y: 100 },
        ClientMsg::Button { down: true, button: 0 },
        ClientMsg::Pointer { x: 200, y: 150 },
        ClientMsg::Pointer { x: 300, y: 200 },
        ClientMsg::Button { down: false, button: 0 },
    ];
    let order = ["pointer", "button 0 down", "pointer", "pointer", "button 0 up"];
    let _stream = session(&drag);
    assert_eq!(wait_for(&log, &order, Duration::from_secs(5)), order);
    while server.rx.try_recv().is_ok() {}
}

#[test]
fn negotiated_clipboard_crosses_both_ways_before_paste() {
    fn read(stream: &mut SocketStream, inbox: &mut Inbox) -> ServerMsg {
        let mut buf = [0; 16 << 10];
        loop {
            if let Some((ty, payload)) = inbox.next().unwrap() {
                return ServerMsg::decode(ty, &payload).unwrap();
            }
            let n = stream.read(&mut buf).unwrap();
            assert_ne!(n, 0, "session closed before the reply");
            inbox.feed(&buf[..n]);
        }
    }

    let (server, admin, log) = input_server(proto::desktop_caps::CLIPBOARD);
    let next = || server.rx.recv_timeout(Duration::from_secs(5)).expect("session event");
    let open = |caps| {
        let stream = connect(server.addr, &admin, caps);
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let ToUi::Opened { session, out, .. } = next() else { panic!("expected an open session") };
        let welcome = ServerMsg::Welcome {
            version: proto::VERSION, session, screens: vec![], screen: 0, stream: [1, 1],
            input: true, input_note: String::new(), server: String::new(),
        };
        out.send(Out::Msg(welcome.encode())).unwrap();
        (stream, Inbox::default(), out)
    };
    let (mut viewer, mut inbox, out) = open(proto::caps::CLIPBOARD);
    assert!(matches!(read(&mut viewer, &mut inbox), ServerMsg::DesktopCaps { caps: proto::desktop_caps::CLIPBOARD }));
    assert!(matches!(read(&mut viewer, &mut inbox), ServerMsg::Welcome { .. }));

    let mut text = "é".repeat(1 << 19);
    text.push_str(&"😀".repeat(1 << 18));
    for part in ClientMsg::clipboard(&text) {
        proto::write_all(&mut viewer, &part.encode()).unwrap();
    }
    let key = ClientMsg::Key { down: true, keycode: 7, mods: 0 };
    proto::write_all(&mut viewer, &key.encode()).unwrap();
    // Pause reaches the owner after the session has handled the key.
    proto::write_all(&mut viewer, &ClientMsg::Pause.encode()).unwrap();
    assert!(matches!(next(), ToUi::Control { control: Control::Clipboard(whole), .. } if whole == text), "owner gets the whole text");
    assert!(matches!(next(), ToUi::Control { control: Control::Pause, .. }));
    assert!(log.lock().unwrap().is_empty(), "key injected before the clipboard was taken");
    out.send(Out::ClipboardTaken).unwrap();
    assert_eq!(wait_for(&log, &["key 7 down"], Duration::from_secs(1)), ["key 7 down"]);

    out.send(Out::Clipboard(text.clone())).unwrap();
    let mut parts = proto::ClipboardParts::default();
    let mut saw_part = false;
    let whole = loop {
        match read(&mut viewer, &mut inbox) {
            ServerMsg::ClipboardPart { text } => {
                saw_part = true;
                parts.part(&text);
            }
            ServerMsg::Clipboard { text } => break parts.end(text),
            _ => panic!("expected clipboard text"),
        }
    };
    assert!(saw_part && whole == text, "viewer gets the whole text in parts");

    let (mut older, mut inbox, out) = open(0);
    out.send(Out::Clipboard(text)).unwrap();
    out.send(Out::Msg(ServerMsg::Stream { screen: 0, stream: [2, 2] }.encode())).unwrap();
    assert!(matches!(read(&mut older, &mut inbox), ServerMsg::Welcome { .. }), "older viewer goes straight to Welcome");
    assert!(matches!(read(&mut older, &mut inbox), ServerMsg::Stream { .. }), "older viewer skips the clipboard broadcast");
    for msg in [ClientMsg::ClipboardPart { text: "a".into() }, ClientMsg::Clipboard { text: "b".into() }, ClientMsg::Pause] {
        proto::write_all(&mut older, &msg.encode()).unwrap();
    }
    assert!(matches!(next(), ToUi::Control { control: Control::Pause, .. }), "older viewer sends no clipboard to the owner");
}
