//! The hub and the relay, in process: a dial-style link over a socket pair, clients over another.

use crate::carrier::Carrier;
use crate::conn::{Channel, Conn, Event, CLOSE_REFUSED};
use crate::endpoint::Endpoint;
use crate::frame::FrameReader;
use crate::hosts::{Host, Hosts, Via};
use crate::hub::Hub;
use crate::relay;
use crate::tests::{pair, shared};
use std::os::unix::net::UnixStream;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(15);

fn carrier(s: UnixStream) -> Carrier {
    let (r, w) = (s.try_clone().unwrap(), s.try_clone().unwrap());
    Carrier {
        reader: FrameReader::new(Box::new(r)),
        writer: Box::new(w),
        shutdown: Box::new(move || {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }),
    }
}

fn hosts() -> Hosts {
    Hosts { hosts: vec![Host { name: "d".into(), via: Via::Dial { key: "00".repeat(32), fingerprint: String::new() } }] }
}

fn ledger_dir(test: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("makepad-remote-test-ledger-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn relay_endpoint() -> Endpoint {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    relay::endpoint(relay::Config { name: "d".into(), launch: vec!["/bin/sleep".into()], env: vec![("SANDBOX_MUTE".into(), "1".into())], ledger: ledger_dir(&format!("shared{n}")) })
}

/// A relay on an existing ledger (what a restarted relay does).
fn relay_on(dir: &std::path::Path, launch: Vec<String>, env: Vec<(String, String)>) -> Endpoint {
    relay::endpoint(relay::Config { name: "d".into(), launch, env, ledger: dir.to_path_buf() })
}

/// A relay named `d` links itself to the hub over a socket pair.
fn link(hub: &Hub, relay: &Endpoint) -> Conn {
    let (a, b) = UnixStream::pair().unwrap();
    let h = hub.clone();
    std::thread::spawn(move || {
        let _ = h.accept_dialed(carrier(a));
    });
    relay.connect(carrier(b), "test-link").unwrap()
}

fn client(hub: &Hub) -> Conn {
    pair(&hub.client_endpoint())
}

fn lines_of(ch: &Channel) -> Vec<String> {
    let mut out = Vec::new();
    loop {
        match ch.recv_timeout(WAIT) {
            Ok(Event::Data(d)) => {
                for l in String::from_utf8_lossy(&d).lines() {
                    if l.starts_with("{\"end\":") {
                        return out;
                    }
                    out.push(l.to_string());
                }
            }
            other => panic!("no end line, got {other:?} after {out:?}"),
        }
    }
}

fn request(c: &Conn, route: &str, line: &str) -> Vec<String> {
    let ch = c.open("route", route).unwrap();
    ch.send(line.as_bytes().to_vec()).unwrap();
    lines_of(&ch)
}

#[test]
fn list_and_cmd_round_trip_through_the_hub() {
    let (_, _) = shared();
    let hub = Hub::new(&hosts());
    let _relay_conn = link(&hub, &relay_endpoint());
    let c = client(&hub);
    let me = std::process::id();
    let list = request(&c, "d list", "list");
    assert!(list.iter().any(|l| l.contains(&format!("\"pid\":{me},")) && l.contains("makepad-remote-test")), "{list:?}");
}

#[test]
fn cmd_goes_to_the_endpoint() {
    let (_, _) = shared();
    let hub = Hub::new(&hosts());
    let _relay_conn = link(&hub, &relay_endpoint());
    let c = client(&hub);
    let me = std::process::id();
    for target in [format!("d:makepad-remote-test"), format!("d:{me}"), format!("d:x#{me}")] {
        let ch = c.open("route", &format!("{target} cmd")).unwrap();
        ch.send(b"ping".to_vec()).unwrap();
        let reply = lines_of(&ch);
        assert!(reply.len() == 1 && reply[0].starts_with("{\"pong\":1"), "{target}: {reply:?}");
        // a second request on the same channel
        ch.send(b"ping".to_vec()).unwrap();
        let reply = lines_of(&ch);
        assert!(reply.len() == 1 && reply[0].starts_with("{\"pong\":1"), "{reply:?}");
    }
}

#[test]
fn unknown_host_and_unknown_app_are_errors() {
    let (_, _) = shared();
    let hub = Hub::new(&hosts());
    let _relay_conn = link(&hub, &relay_endpoint());
    let c = client(&hub);
    let e = c.open("route", "nope cmd").err().unwrap();
    assert_eq!(e.0, CLOSE_REFUSED);
    assert!(e.1.contains("unknown host `nope`") && e.1.contains("known: d"), "{}", e.1);
    let ch = c.open("route", "d:no-such-app cmd").unwrap();
    match ch.recv_timeout(WAIT) {
        Ok(Event::Closed(_, why)) => assert!(why.contains("no running makepad-remote endpoint `no-such-app`"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn unreachable_dial_host_says_so() {
    let hub = Hub::new(&hosts());
    let c = client(&hub);
    let ch = c.open("route", "d cmd").unwrap();
    match ch.recv_timeout(WAIT) {
        Ok(Event::Closed(_, why)) => assert!(why.contains("not connected"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn hosts_report_shows_state_and_rtt() {
    let hub = Hub::new(&hosts());
    let c = client(&hub);
    let ch = c.open("hosts", "").unwrap();
    ch.send(b"hosts".to_vec()).unwrap();
    let l = lines_of(&ch);
    assert!(l[0].contains("\"host\":\"d\"") && l[0].contains("\"state\":\"waiting\""), "{l:?}");
    let _relay_conn = link(&hub, &relay_endpoint());
    // the hub registers the link on its own thread: give it a moment
    let mut l = Vec::new();
    for _ in 0..50 {
        ch.send(b"hosts".to_vec()).unwrap();
        l = lines_of(&ch);
        if l[0].contains("\"state\":\"up\"") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(l[0].contains("\"state\":\"up\"") && l[0].contains("\"rtt_ms\":"), "{l:?}");
}

#[test]
fn session_launch_and_kill_only_own_pids() {
    let hub = Hub::new(&hosts());
    let _relay_conn = link(&hub, &relay_endpoint());
    let c = client(&hub);
    let ch = c.open("route", "d session").unwrap();
    let ask = |line: &str| {
        ch.send(line.as_bytes().to_vec()).unwrap();
        lines_of(&ch)
    };
    let launched = ask("launch 30");
    assert!(launched[0].starts_with("{\"launched\":"), "{launched:?}");
    let pid: u32 = launched[0]["{\"launched\":".len()..].split(',').next().unwrap().parse().unwrap();
    assert!(ask("launched")[0].contains("\"state\":\"running\""));
    let other = ask(&format!("kill {}", std::process::id()));
    assert!(other[0].contains("was not launched"), "{other:?}");
    assert_eq!(ask(&format!("kill {pid}"))[0], format!("{{\"killed\":{pid}}}"));
    let mut state = String::new();
    for _ in 0..100 {
        state = ask("launched")[0].clone();
        if state.contains("exited") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(state.contains("exited"), "{state}");
}

#[test]
fn a_dropped_link_resumes_the_watch_feed_with_a_gap_marker() {
    let (_, watch) = shared();
    let hub = Hub::new(&hosts());
    let relay_ep = relay_endpoint();
    let first = link(&hub, &relay_ep);
    let c = client(&hub);
    let me = std::process::id();
    let w = c.open("route", &format!("d:{me} watch")).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    watch.emit("{\"hub\":1}");
    let mut seen = Vec::new();
    let next = |seen: &mut Vec<String>| match w.recv_timeout(WAIT) {
        Ok(Event::Data(d)) => {
            let l = String::from_utf8_lossy(&d[8..]).into_owned();
            seen.push(l.clone());
            l
        }
        other => panic!("{other:?}"),
    };
    while !next(&mut seen).contains("\"hub\":1") {}
    // the link drops; an event happens while it is down; the relay comes back
    first.close("test drop");
    first.wait_closed();
    std::thread::sleep(Duration::from_millis(300));
    watch.emit("{\"hub\":2}");
    let again = relay_ep.clone();
    let h = hub.clone();
    let t = std::thread::spawn(move || link(&h, &again));
    let mut tail = Vec::new();
    loop {
        let l = next(&mut tail);
        if l.contains("\"hub\":2") {
            break;
        }
    }
    let gap = tail.iter().position(|l| l.contains("\"event\":\"gap\"")).expect("gap marker");
    assert!(gap < tail.len() - 1, "the gap marker comes before the event that happened while down: {tail:?}");
    assert_eq!(tail.iter().filter(|l| l.contains("\"hub\":1")).count(), 0, "no replay of what was delivered: {tail:?}");
    let _keep = t.join().unwrap();
}

fn dial_host(name: &str, key: &makepad_network::tunnel::Psk) -> Host {
    Host { name: name.into(), via: Via::Dial { key: key.to_hex(), fingerprint: String::new() } }
}

fn relay_named(name: &str) -> Endpoint {
    relay::endpoint(relay::Config { name: name.into(), launch: vec![], env: vec![], ledger: ledger_dir(name) })
}

fn dial_relay(ep: Endpoint, addr: &str, key: makepad_network::tunnel::Psk, fp: [u8; 32]) {
    let addr = addr.to_string();
    std::thread::spawn(move || {
        let creds = makepad_network::tunnel::ClientCredentials { keys: vec![key], known_hosts: None, expect: Some(fp) };
        relay::dial_forever(&ep, &addr, &creds)
    });
}

#[test]
fn dial_in_binds_the_key_to_its_host_and_reads_keys_live() {
    use makepad_network::tls::TlsIdentity;
    use makepad_network::tunnel::Psk;
    use std::sync::{Arc, Mutex};
    let _ = shared();
    let dir = std::env::temp_dir().join(format!("makepad-remote-hub-tls-{}", std::process::id()));
    let identity = TlsIdentity::load_or_create(&dir, "makepad-remote-test").unwrap();
    let fp = identity.fingerprint;
    let (ka, kb, kc) = (Psk::generate().unwrap(), Psk::generate().unwrap(), Psk::generate().unwrap());
    // the registry the hub reads on every accept: only A and B at first
    let registry = Arc::new(Mutex::new(Hosts { hosts: vec![dial_host("a", &ka), dial_host("b", &kb)] }));
    let hub = Hub::new(&registry.lock().unwrap().clone());
    let r2 = registry.clone();
    let addr = hub.listen_dial("127.0.0.1:0", identity, Arc::new(move || r2.lock().unwrap().clone())).unwrap().to_string();
    let c = client(&hub);

    // A's key under B's name: refused, B stays unconnected
    dial_relay(relay_named("b"), &addr, ka.clone(), fp);
    // A's key under A's name works
    dial_relay(relay_named("a"), &addr, ka.clone(), fp);
    let me = std::process::id();
    let l = request(&c, "a list", "list");
    assert!(l.iter().any(|l| l.contains(&format!("\"pid\":{me},"))), "{l:?}");
    let ch = c.open("route", "b cmd").unwrap();
    // (the impostor retries with backoff, never becomes b's link)
    match ch.recv_timeout(WAIT) {
        Ok(Event::Closed(_, why)) => assert!(why.contains("not connected"), "{why}"),
        other => panic!("{other:?}"),
    }

    // C is paired after the hub started: no restart needed
    registry.lock().unwrap().set(dial_host("c", &kc));
    dial_relay(relay_named("c"), &addr, kc, fp);
    let l = request(&c, "c list", "list");
    assert!(l.iter().any(|l| l.contains(&format!("\"pid\":{me},"))), "{l:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hosts_registry_round_trips() {
    let text = "[host.149]\nvia = \"ssh\"\nssh = \"arch@10.0.0.149\"\nmakepad = \"/opt/makepad\"\n\n[host.phone]\nvia = \"dial\"\nkey = \"ab\"\nfingerprint = \"cd\"\n";
    let h = Hosts::parse(text).unwrap();
    assert_eq!(h.names(), vec!["149", "phone"]);
    assert_eq!(Hosts::parse(&h.to_text()).unwrap(), h);
    assert!(Hosts::parse("[host.x]\nvia = \"carrier-pigeon\"\n").unwrap_err().contains("via must be"));
    assert!(Hosts::parse("[host.x]\nvia = \"ssh\"\n").unwrap_err().contains("needs ssh"));
}

fn ask(ch: &Channel, line: &str) -> Vec<String> {
    ch.send(line.as_bytes().to_vec()).unwrap();
    lines_of(ch)
}

fn field(line: &str, key: &str) -> String {
    let pat = format!("\"{key}\":");
    let rest = &line[line.find(&pat).unwrap() + pat.len()..];
    let rest = rest.trim_start_matches('"');
    rest.split(|c| c == ',' || c == '}' || c == '"').next().unwrap().to_string()
}

fn wait_state(ch: &Channel, pid: u32, want: &str) -> String {
    let mut last = String::new();
    for _ in 0..200 {
        last = ask(ch, "launched").into_iter().find(|l| l.contains(&format!("\"pid\":{pid},"))).unwrap_or_default();
        if last.contains(want) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    last
}

#[test]
fn the_ledger_lets_a_restarted_relay_kill_and_drops_stale_entries() {
    let dir = ledger_dir("restart");
    // the first relay launches a session, then goes away
    let hub1 = Hub::new(&hosts());
    let first = link(&hub1, &relay_on(&dir, vec!["/bin/sleep".into()], vec![]));
    let c1 = client(&hub1);
    let ch1 = c1.open("route", "d session").unwrap();
    let pid: u32 = field(&ask(&ch1, "launch 30")[0], "launched").parse().unwrap();
    assert!(dir.join(format!("{pid}.json")).exists());
    first.close("relay restart");
    first.wait_closed();
    // a stale entry: this very process with a start time it never had
    std::fs::write(dir.join(format!("{}.json", std::process::id())), format!("{{\"pid\":{},\"start\":1,\"app\":\"x\",\"args\":\"\",\"cwd\":\"\",\"env_keys\":\"\",\"log\":\"/dev/null\"}}", std::process::id())).unwrap();
    // a new relay on the same ledger knows the session
    let hub2 = Hub::new(&hosts());
    let _second = link(&hub2, &relay_on(&dir, vec!["/bin/sleep".into()], vec![]));
    let c2 = client(&hub2);
    let ch2 = c2.open("route", "d session").unwrap();
    let listed = ask(&ch2, "launched");
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert!(listed[0].contains(&format!("\"pid\":{pid},")) && listed[0].contains("\"state\":\"running\""), "{listed:?}");
    assert!(!dir.join(format!("{}.json", std::process::id())).exists(), "the stale entry is deleted");
    let refused = ask(&ch2, &format!("kill {}", std::process::id()));
    assert!(refused[0].contains("was not launched"), "{refused:?}");
    assert_eq!(ask(&ch2, &format!("kill {pid}"))[0], format!("{{\"killed\":{pid}}}"));
    assert!(wait_state(&ch2, pid, "exited").contains("exited"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn relaunch_starts_the_same_app_again() {
    let dir = ledger_dir("relaunch");
    let hub = Hub::new(&hosts());
    let _l = link(&hub, &relay_on(&dir, vec!["/bin/sleep".into()], vec![]));
    let c = client(&hub);
    let ch = c.open("route", "d session").unwrap();
    let old: u32 = field(&ask(&ch, "launch 30")[0], "launched").parse().unwrap();
    let r = ask(&ch, &format!("relaunch {old}"));
    assert!(r[0].starts_with("{\"relaunched\":"), "{r:?}");
    let new: u32 = field(&r[0], "relaunched").parse().unwrap();
    assert_ne!(new, old);
    assert_eq!(field(&r[0], "was"), old.to_string());
    assert!(wait_state(&ch, old, "exited").contains("exited"));
    assert!(wait_state(&ch, new, "running").contains("running"));
    let none = ask(&ch, "relaunch 1");
    assert!(none[0].contains("was not launched"), "{none:?}");
    assert_eq!(ask(&ch, &format!("kill {new}"))[0], format!("{{\"killed\":{new}}}"));
    assert!(wait_state(&ch, new, "exited").contains("exited"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn launches_get_the_relays_env() {
    let dir = ledger_dir("env");
    let hub = Hub::new(&hosts());
    let _l = link(&hub, &relay_on(&dir, vec!["/usr/bin/env".into()], vec![("T3B_PROBE".into(), "from-hosts-toml".into())]));
    let c = client(&hub);
    let ch = c.open("route", "d session").unwrap();
    let l = ask(&ch, "launch printenv T3B_PROBE");
    let pid: u32 = field(&l[0], "launched").parse().unwrap();
    let log = field(&l[0], "log");
    assert!(wait_state(&ch, pid, "exited").contains("exited"));
    assert_eq!(std::fs::read_to_string(&log).unwrap().trim(), "from-hosts-toml");
    let _ = std::fs::remove_file(&log);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn host_env_round_trips_in_hosts_toml() {
    let text = "[host.n]\nvia = \"ssh\"\nssh = \"a@b\"\nenv = { DISPLAY = \":0\", WAYLAND_DISPLAY = \"wayland-1\" }\n";
    let h = Hosts::parse(text).unwrap();
    let Via::Ssh { env, .. } = &h.get("n").unwrap().via else { panic!() };
    assert_eq!(env.len(), 2);
    assert!(env.contains(&("DISPLAY".to_string(), ":0".to_string())));
    assert_eq!(Hosts::parse(&h.to_text()).unwrap(), h);
    assert!(Hosts::parse("[host.d]\nvia = \"dial\"\nkey = \"00\"\nenv = { A = \"b\" }\n").is_err());
}
