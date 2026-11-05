//! `ServiceHandle::stop`: a service stopped in-process closes its ports and its locks, so
//! the same process can start it again on the same port (the desktop AI Hub app's Enabled
//! switch and backend toggles).

use makepad_ai_hub::download::Downloader;
use makepad_ai_hub::registry::Registry;
use makepad_ai_hub::server::{start_service, ServiceConfig};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

fn config(dir: &std::path::Path, host: &str, port: u16) -> ServiceConfig {
    ServiceConfig {
        host: host.to_string(),
        port,
        cache_dir: dir.to_path_buf(),
        registry: Registry { models: Vec::new() },
        downloader: Downloader::new("http://127.0.0.1:1", None).unwrap(),
        peer: makepad_ai_hub::peer_serve::PeerOptions { serve: Some(false), sources: Some(Vec::new()), ..Default::default() },
        // A partition nobody listens for: the beacon this test sends is ignored by the fleet.
        fleet: "hubapp-stop-test".to_string(),
    }
}

fn answers(addr: SocketAddr) -> bool {
    let addr = if addr.ip().is_unspecified() { SocketAddr::from(([127, 0, 0, 1], addr.port())) } else { addr };
    TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok()
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("makepad-ai-hub-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A fleet node needs this machine's fleet identity: a test root, a node
/// credential issued by it (process-global; this binary installs it once).
fn install_identity() {
    use makepad_network::fleet::id::{self, generate_key, now_secs, point_of, Fleet, Me, Role, Root};
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let root = Root::generate().unwrap();
        let key = generate_key().unwrap();
        let cred = root.issue("stop-test", Role::Node, now_secs() + 3600, None, &point_of(&key)).unwrap();
        id::install_fleet(Fleet::new(root.point(), Default::default()));
        id::install_local(Me::new(key, cred).unwrap());
    });
}

/// A hub that stops closes its streams: a client hears it (a session that would never answer
/// ends as lost) and the stop does not wait for the client to leave. The session stays held
/// until the stop returned: that hold kept a stopping hub up and its client at 0 iterations.
#[cfg(feature = "kernel")]
#[test]
fn a_stopping_hub_closes_its_streams_and_no_client_holds_it_up() {
    use makepad_ai_hub::kernel_client::{KernelNode, KernelStream};
    use makepad_ai_hub::program_client::{RemoteConfig, RemoteSession};
    use makepad_compute_program::{Program, ProgramError};
    let handle = start_service(config(&scratch("stop-stream"), "127.0.0.1", 0)).unwrap();
    let stream = std::sync::Arc::new(KernelStream::open(&KernelNode::new(&format!("http://{}", handle.addr))).unwrap());
    let silent = RemoteSession::open(&stream, &Program::new("t.splash", "{run: fn(s, p) { s.wait_inbox() return 1 }}"), RemoteConfig::default()).unwrap();
    let (stopped, told) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        handle.stop();
        let _ = stopped.send(());
    });
    told.recv_timeout(Duration::from_secs(30)).expect("the stop ends with the client still connected");
    assert!(matches!(silent.wait_end(), Err(ProgramError::Lost(_))), "the session ends as lost");
    assert!(stream.is_closed());
}

#[test]
fn a_stopped_service_closes_its_ports_and_starts_again() {
    // Loopback node.
    let dir = scratch("stop-loopback");
    let handle = start_service(config(&dir, "127.0.0.1", 0)).unwrap();
    let addr = handle.addr;
    assert!(answers(addr));
    handle.stop();
    assert!(!answers(addr), "the port is closed after stop");
    let again = start_service(config(&dir, "127.0.0.1", addr.port())).expect("the locks were released");
    assert!(answers(again.addr));
    again.stop();

    // a non-loopback bind is a LAN listener, which the Networked AI hub setting must allow
    std::env::set_var("MAKEPAD_NETWORKED_AI_HUB", "1");
    // A non-loopback bind selects the TLS front; dial it over loopback so
    // this check needs no LAN interface or external fleet node.
    let host = "0.0.0.0";
    let dir = scratch("stop-fleet");
    install_identity();
    let handle = start_service(config(&dir, host, 0)).unwrap();
    let addr = handle.addr;
    assert!(!addr.ip().is_loopback());
    assert!(answers(addr), "the TLS front listens");
    handle.stop();
    assert!(!answers(addr), "the TLS front is closed after stop");
    let again = start_service(config(&dir, host, addr.port())).expect("same port again");
    assert!(answers(again.addr));
    again.stop();
}
