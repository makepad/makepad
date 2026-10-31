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

    // A fleet node (TLS front + beacon) on this machine's private LAN address, when it has one.
    let Some(ip) = makepad_ai_hub::discovery::network_addresses().into_iter().find(|ip| ip.is_private()) else {
        eprintln!("no private LAN address here: the fleet-front half is skipped");
        return;
    };
    let dir = scratch("stop-fleet");
    install_identity();
    let handle = start_service(config(&dir, &ip.to_string(), 0)).unwrap();
    let addr = handle.addr;
    assert!(!addr.ip().is_loopback());
    assert!(answers(addr), "the TLS front listens");
    handle.stop();
    assert!(!answers(addr), "the TLS front is closed after stop");
    let again = start_service(config(&dir, &ip.to_string(), addr.port())).expect("same port again");
    assert!(answers(again.addr));
    again.stop();
}
