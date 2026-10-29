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
//! `desk on|off|state` switches the Desk Link endpoint of a JIT host that embeds one
//! ([`crate::embed`]; its state rides in `caps`).

use crate::local::Local;
use crate::proto::{self, hub_record, Sev};
use crate::trust::{PeerRole, Trust};
use makepad_remote::json_str;
use makepad_remote::{lines, relay, Endpoint};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub use crate::conf::{NodeConf, NODE_CONF_ENV};

/// A hub this node knows: name, address, its fleet identity (the credential name its signed
/// announcement checked out under), when it was last heard.
#[derive(Clone, Debug)]
pub struct PeerInfo {
    pub name: String,
    pub addr: String,
    pub fleet: String,
    pub seen_ms: u64,
}

#[derive(Default)]
pub struct Peers {
    pub list: Vec<PeerInfo>,
}

impl Peers {
    /// One machine is one entry: keyed by its fleet identity (a box that announces on two
    /// addresses, or under a new hub name, stays one peer).
    pub fn saw(&mut self, name: &str, addr: &str, fleet: &str) {
        let now = proto::wall_ms();
        match self.list.iter_mut().find(|p| p.fleet == fleet) {
            Some(p) => {
                p.name = name.to_string();
                p.addr = addr.to_string();
                p.seen_ms = now;
            }
            None => self.list.push(PeerInfo { name: name.to_string(), addr: addr.to_string(), fleet: fleet.to_string(), seen_ms: now }),
        }
    }

    pub fn expire(&mut self, ms: u64) {
        let now = proto::wall_ms();
        self.list.retain(|p| now.saturating_sub(p.seen_ms) < ms);
    }

    pub fn lines(&self, out: &mut Vec<String>) {
        for p in &self.list {
            out.push(format!("{{\"hub\":{},\"addr\":{},\"fleet\":{},\"seen_ms\":{}}}", json_str(&p.name), json_str(&p.addr), json_str(&p.fleet), p.seen_ms));
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
    let machine = name.to_string();
    ep.register(
        "top",
        1,
        lines::service(
            "top",
            Arc::new(move |line, out| {
                let n = line.split_whitespace().find_map(|w| w.parse::<usize>().ok()).unwrap_or(12).min(100);
                out.extend(crate::top::top(&machine, n));
            }),
        ),
    );
    let l = local.clone();
    ep.register("jitlog", 1, lines::service("jitlog", Arc::new(move |line, out| jit_log(&l, line, out))));
    // the Desk Link endpoint of a JIT host that embeds one (`crate::embed`)
    ep.register("desk", 1, lines::service("desk", Arc::new(|line, out| out.push(crate::embed::desk_service(line)))));
    (ep, sink)
}

/// `jitlog <pid|app> [n]`: the last `n` lines of a JIT's session log, or of its launch log when
/// devhub launched it.
fn jit_log(local: &Local, line: &str, out: &mut Vec<String>) {
    let mut w = line.split_whitespace().skip_while(|x| *x == "jitlog");
    let who = w.next().unwrap_or("");
    let n = w.next().and_then(|x| x.parse::<usize>().ok()).unwrap_or(40).min(2000);
    let jits = local.jits();
    let jit = jits.iter().find(|j| j.pid.to_string() == who).or_else(|| jits.iter().filter(|j| j.app == who).max_by_key(|j| j.started_ms));
    let pid = jit.map(|j| j.pid).or_else(|| who.parse().ok());
    let path = jit.and_then(|j| j.session_log.clone()).filter(|p| !p.is_empty()).map(PathBuf::from).or_else(|| pid.and_then(crate::sync::launch_log));
    let Some(path) = path else {
        return out.push(format!("{{\"err\":{}}}", json_str(&format!("no JIT or launch `{who}` with a log here"))));
    };
    let text = read_tail(&path, 512 << 10);
    let lines: Vec<&str> = text.lines().collect();
    out.push(format!("{{\"log\":{},\"pid\":{}}}", json_str(&path.display().to_string()), pid.unwrap_or(0)));
    for l in &lines[lines.len().saturating_sub(n)..] {
        out.push(format!("{{\"line\":{}}}", json_str(&proto::clean(l, 1000))));
    }
}

/// The last `max` bytes of a file, from a line start.
fn read_tail(path: &std::path::Path, max: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(max)));
    let mut b = Vec::new();
    let _ = f.read_to_end(&mut b);
    let s = String::from_utf8_lossy(&b).into_owned();
    if len > max { s.split_once('\n').map_or(s.clone(), |(_, r)| r.to_string()) } else { s }
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
        let (machine, s3) = (conf.name.clone(), sink.clone());
        master_ep.register(
            "jitcache",
            1,
            lines::service("jitcache", Arc::new(move |line, out| crate::jitcache::service(&machine, line, out, &|r| { s3.emit(&r.to_json()); }))),
        );
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
                if !node.conf.discovery {
                    log("devhub: discovery off: no LAN beacon (the master dials this node from its nodes.conf)");
                } else if let Err(e) = crate::cluster::start(&node.conf.name, a.port(), node.peers.clone(), log.clone()) {
                    log(&format!("devhub: {e}"));
                }
            }
            Err(e) => node.lan_off = Some(format!("not listening on the LAN: {e}")),
        }
        Ok(node)
    }
}

