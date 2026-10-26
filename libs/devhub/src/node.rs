//! The node role: a Linux WM's hub. It follows this machine's JITs ([`Local`]), serves them to
//! the master over the LAN, and talks to the other hubs.
//!
//! Two endpoints, chosen per connection by who the peer proved to be ([`crate::trust`]):
//! - the **master endpoint** (the master only; also this machine's own control socket, same
//!   user): the relay's `list` / `session` / `fwd` (any service of a local JIT: all of inspect),
//!   `watch` (this node's records, resumable), `ps`, `reply`, `peers`;
//! - the **peer endpoint** (other hubs): `peers` only. Hubs get no inspect rights on each other.
//!
//! DEVHUB2 adds `sync` to the master endpoint and `sync-peer` to the peer endpoint; leases add
//! `lease-*`, `policy`, `caps`, `disk`, `recover-*` to the master endpoint ([`crate::lease::exec`]).

use crate::local::Local;
use crate::proto::{self, hub_record, Sev};
use crate::trust::{PeerRole, Trust};
use makepad_remote::json_str;
use makepad_remote::{lines, relay, Endpoint};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub use crate::conf::{NodeConf, NODE_CONF_ENV};

/// A hub this node knows: name, address, when it was last heard.
#[derive(Clone, Debug)]
pub struct PeerInfo {
    pub name: String,
    pub addr: String,
    pub seen_ms: u64,
}

#[derive(Default)]
pub struct Peers {
    pub list: Vec<PeerInfo>,
}

impl Peers {
    pub fn saw(&mut self, name: &str, addr: &str) {
        let now = proto::wall_ms();
        match self.list.iter_mut().find(|p| p.name == name) {
            Some(p) => {
                p.addr = addr.to_string();
                p.seen_ms = now;
            }
            None => self.list.push(PeerInfo { name: name.to_string(), addr: addr.to_string(), seen_ms: now }),
        }
    }

    pub fn expire(&mut self, ms: u64) {
        let now = proto::wall_ms();
        self.list.retain(|p| now.saturating_sub(p.seen_ms) < ms);
    }

    pub fn lines(&self, out: &mut Vec<String>) {
        for p in &self.list {
            out.push(format!("{{\"hub\":{},\"addr\":{},\"seen_ms\":{}}}", json_str(&p.name), json_str(&p.addr), p.seen_ms));
        }
    }
}

pub struct Node {
    pub conf: NodeConf,
    pub local: Arc<Local>,
    pub master_ep: Endpoint,
    pub peers: Arc<Mutex<Peers>>,
    /// the LAN listener's address, when it listens
    pub addr: Option<SocketAddr>,
    /// why it does not listen (no LAN address, no fleet key, ...)
    pub lan_off: Option<String>,
}

/// The endpoint the master (and this machine's own user, on the control socket) is served by.
pub fn master_endpoint(name: &str, launch: Vec<String>, local: &Arc<Local>, peers: &Arc<Mutex<Peers>>) -> (Endpoint, makepad_remote::watch::WatchSink) {
    // sessions the master launches run hidden and muted, like every agent launch
    let env = vec![("MAKEPAD_HIDE_WINDOWS".to_string(), "1".to_string()), ("SANDBOX_MUTE".to_string(), "1".to_string())];
    let ep = relay::endpoint(relay::Config { name: format!("devhub {name}"), launch, env, ledger: relay::default_ledger() });
    let sink = ep.register_watch();
    let l = local.clone();
    ep.register("ps", 1, lines::service("ps", Arc::new(move |_, out| l.ps(out))));
    let l = local.clone();
    ep.register(
        "reply",
        1,
        lines::service(
            "reply",
            Arc::new(move |line, out| {
                let mut w = line.splitn(3, ' ');
                let (id, status, msg) = (w.next().unwrap_or(""), w.next().unwrap_or(""), w.next().unwrap_or(""));
                match l.reply(id, status, msg) {
                    Ok(ack) => out.push(ack),
                    Err(e) => out.push(format!("{{\"err\":{}}}", json_str(&e))),
                }
            }),
        ),
    );
    let p = peers.clone();
    ep.register("peers", 1, lines::service("peers", Arc::new(move |_, out| p.lock().unwrap().lines(out))));
    (ep, sink)
}

/// The endpoint other hubs are served by.
pub fn peer_endpoint(name: &str, peers: &Arc<Mutex<Peers>>) -> Endpoint {
    let ep = Endpoint::new(&format!("devhub {name}"));
    let p = peers.clone();
    ep.register("peers", 1, lines::service("peers", Arc::new(move |_, out| p.lock().unwrap().lines(out))));
    ep
}

/// Listens on the control socket unless a live hub already serves it.
pub fn listen_control(ep: &Endpoint) -> Result<PathBuf, String> {
    let path = proto::control_socket();
    if makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(path.clone()), "probe").conn().is_ok() {
        return Err(format!("a devhub already serves {}", path.display()));
    }
    makepad_remote::carrier::unix::listen_at(ep, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

impl Node {
    /// Starts the node: local JITs, the control socket, and the LAN listener when `trust` allows it.
    pub fn start(conf: NodeConf, trust: Option<Arc<dyn Trust>>, log: crate::trust::Log) -> Result<Node, String> {
        let peers = Arc::new(Mutex::new(Peers::default()));
        let sink_slot: Arc<Mutex<Option<makepad_remote::watch::WatchSink>>> = Arc::new(Mutex::new(None));
        let s = sink_slot.clone();
        let local = Local::start(
            &conf.name,
            conf.hitch_ms,
            conf.embedded,
            Arc::new(move |r| {
                if let Some(sink) = s.lock().unwrap().as_ref() {
                    sink.emit(&r.to_json());
                }
            }),
        );
        let (master_ep, sink) = master_endpoint(&conf.name, conf.launch.clone(), &local, &peers);
        *sink_slot.lock().unwrap() = Some(sink.clone());
        let s2 = sink.clone();
        let sync_ctx = Arc::new(crate::sync::NodeCtx { machine: conf.name.clone(), publish: Arc::new(move |r| { s2.emit(&r.to_json()); }) });
        crate::sync::register_master(&master_ep, &sync_ctx);
        crate::lease::exec::register(&master_ep, &sync_ctx);
        listen_control(&master_ep)?;
        let mut node = Node { conf, local, master_ep, peers, addr: None, lan_off: None };
        let Some(trust) = trust else {
            node.lan_off = Some("no fleet trust on this machine: local only".into());
            return Ok(node);
        };
        let Some(ip) = node.conf.host.or_else(|| proto::lan_ipv4().map(std::net::IpAddr::V4)) else {
            node.lan_off = Some("no private LAN address: local only".into());
            return Ok(node);
        };
        let peer_ep = peer_endpoint(&node.conf.name, &node.peers);
        crate::sync::register_peer(&peer_ep, &sync_ctx);
        let (mep, pep) = (node.master_ep.clone(), peer_ep);
        let pick: crate::trust::Pick = Arc::new(move |p| match p.role {
            PeerRole::Master => Ok(mep.clone()),
            PeerRole::Node => Ok(pep.clone()),
            PeerRole::Other => Err(format!("credential `{}` has no rights on this hub", p.id)),
        });
        let (machine, sink2, log2) = (node.conf.name.clone(), sink, log.clone());
        let accepted: crate::trust::Accepted = Arc::new(move |p, _conn| {
            match p.role {
                PeerRole::Master => {
                    sink2.emit(&hub_record(&machine, "", 0, "hub", Sev::Info, format!("hub: master `{}` connected from {}", p.id, p.addr.ip()), String::new()).to_json());
                }
                PeerRole::Node => {}
                PeerRole::Other => {}
            }
            log2(&format!("devhub: {} `{}` ({}) connected from {}", match p.role { PeerRole::Master => "master", PeerRole::Node => "hub", PeerRole::Other => "peer" }, p.name, p.id, p.addr));
        });
        match trust.listen(SocketAddr::new(ip, node.conf.port), pick, Some(accepted), log.clone()) {
            Ok(a) => {
                node.addr = Some(a);
                if let Err(e) = crate::cluster::start(&node.conf.name, a.port(), node.peers.clone(), log.clone()) {
                    log(&format!("devhub: {e}"));
                }
            }
            Err(e) => node.lan_off = Some(format!("not listening on the LAN: {e}")),
        }
        Ok(node)
    }
}

