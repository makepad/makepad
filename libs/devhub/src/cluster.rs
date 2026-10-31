//! Node-to-node discovery: every node announces itself on the LAN every [`EVERY`] (a datagram
//! signed with its fleet key, credential attached: `Me::announce`), to the limited broadcast on
//! [`proto::BEACON_PORT`] (shared, reuse-port, with the fleet's other announcers), and lists the
//! announcements that check out against the fleet root (`Fleet::check_announcement`) from
//! fleet nodes and paired devices (JIT hosts on phones and headsets). The list is the node's `peers` answer, which the master reads to learn every node
//! from any one it reaches. The master itself never listens for announcements (on macOS that
//! would ask the Local Network question).

use crate::node::Peers;
use crate::proto;
use makepad_network::fleet::id;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const SERVICE: &str = "devhub/1";
pub const EVERY: Duration = Duration::from_secs(5);
/// A peer not heard for this long leaves the list.
pub const EXPIRY_MS: u64 = 20_000;

/// The payload: `<hub name>|<port>`.
fn payload(name: &str, port: u16) -> Vec<u8> {
    format!("{name}|{port}").into_bytes()
}

/// Starts announcing `name` on `port` and listening for the other nodes. Err: why not (no
/// identity, the LAN rule, the port).
pub fn start(name: &str, port: u16, peers: Arc<Mutex<Peers>>, log: crate::trust::Log) -> Result<(), String> {
    if !makepad_network::lan::service("devhub discovery") {
        return Err("discovery: this process may not use the LAN".into());
    }
    let me = id::me().ok_or_else(id::why_no_identity)?;
    let fleet = id::fleet().ok_or("discovery: no fleet root")?;
    let rx = makepad_core_util::udp::bind_reuse_udp(proto::BEACON_PORT).map_err(|e| format!("discovery: udp {}: {e}", proto::BEACON_PORT))?;
    let tx = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    tx.set_broadcast(true).map_err(|e| e.to_string())?;
    let datagram_of = {
        let name = name.to_string();
        move || me.announce(SERVICE, &payload(&name, port))
    };
    let own = name.to_string();
    std::thread::Builder::new()
        .name("devhub-announce".into())
        .spawn(move || loop {
            if let Ok(d) = datagram_of() {
                let _ = tx.send_to(&d, SocketAddr::from((Ipv4Addr::BROADCAST, proto::BEACON_PORT)));
            }
            std::thread::sleep(EVERY);
        })
        .map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("devhub-discover".into())
        .spawn(move || {
            let mut buf = vec![0u8; 4096];
            let _ = rx.set_read_timeout(Some(Duration::from_secs(2)));
            loop {
                if let Ok((n, from)) = rx.recv_from(&mut buf) {
                    let bytes = &buf[..n];
                    if !id::is_announcement(bytes) || !crate::trust::lan_peer(from.ip()) {
                        continue;
                    }
                    match fleet.check_announcement(bytes, from.ip(), id::now_secs()) {
                        Ok((peer, SERVICE, body)) if peer.node() || peer.role == id::Role::Paired => {
                            let text = String::from_utf8_lossy(body);
                            let Some((hub, port)) = text.split_once('|') else { continue };
                            let Ok(port) = port.parse::<u16>() else { continue };
                            if hub == own || !crate::trust::valid_name(hub) {
                                continue;
                            }
                            peers.lock().unwrap().saw(hub, &format!("{}:{port}", id::normalize(from.ip())), &peer.name);
                        }
                        Ok(_) => {}
                        Err(e) => log(&format!("devhub: announcement from {} dropped: {e}", from.ip())),
                    }
                }
                peers.lock().unwrap().expire(EXPIRY_MS);
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}
