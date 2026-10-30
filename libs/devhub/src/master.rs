//! The master role: the user's Mac. It listens on nothing but its local control socket. It dials
//! the node hubs (seeds, then what they gossip), routes inspect to their JITs through the remote
//! hub's `route` (the same router as `makepad inspect <host>:<app>`), and merges every node's
//! records with the Mac's own JITs into one feed and the inbox.
//!
//! Control socket services: the hub's `route` and `hosts`, plus `watch` (all records, every
//! machine), `ps` (this Mac), `reply` (this Mac's JITs; a node's through `route <node> reply`).
//! Leases: `lease beat release leases nodes node-dev linspect` ([`crate::lease::master`]).

use crate::inbox::Inbox;
use crate::local::Local;
use crate::proto::{self, hub_record, Record, Sev};
use crate::trust::Trust;
use makepad_network::fleet::id;
use makepad_remote::hosts::{Host, Hosts, Via};
use makepad_remote::hub::Hub;
use makepad_remote::link::{Client, Target};
use makepad_remote::watch::WatchSink;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Where records go besides the control feed (the inbox, the tray).
pub type OnRecord = Arc<dyn Fn(&Record) + Send + Sync>;

pub struct Master {
    pub name: String,
    pub local: Arc<Local>,
    pub hub: Hub,
    sink: WatchSink,
    followed: Mutex<HashSet<String>>,
    /// nodes learned from the nodes' `peers` (their signed LAN announcements)
    heard: Arc<Mutex<Vec<Heard>>>,
    on_record: OnRecord,
    /// what the user has not cleared yet (err / warn / req records of every machine)
    pub inbox: Arc<Mutex<Inbox>>,
    /// leases: the scheduler and its control services (DESIGN.md §10)
    pub leases: Arc<crate::lease::master::LeaseMaster>,
    /// the last records of every machine, info included (the panel's recent events)
    pub recent: Arc<Mutex<std::collections::VecDeque<Record>>>,
}

/// The master's node list, `~/.makepad/devhub/nodes.conf`: one line per node,
/// `<name> addr=<ip[:port]> [dev=on|off]` (`#` comments; no `dev=` means on, for every node). It is the master's seeds (the master
/// never listens for announcements: receiving LAN broadcasts would ask the macOS Local Network
/// question) and the dev switch of each node (DESIGN.md §10.7).
pub fn nodes_conf() -> PathBuf {
    proto::home().join("nodes.conf")
}

/// The node hubs to dial: `nodes.conf`, the master's one list of machines. (The remote
/// registry's hosts — ssh and dial-in hosts of `makepad hub` — are not devhub nodes.)
pub fn seeds() -> Hosts {
    let mut out = Hosts::default();
    for line in std::fs::read_to_string(nodes_conf()).unwrap_or_default().lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut w = line.split_whitespace();
        let Some(name) = w.next().filter(|n| crate::trust::valid_name(n)) else { continue };
        let Some(addr) = w.find_map(|kv| kv.strip_prefix("addr=")) else { continue };
        let addr = if addr.contains(':') { addr.to_string() } else { format!("{addr}:{}", proto::NODE_PORT) };
        if !out.hosts.iter().any(|h| h.name == name) {
            out.hosts.push(Host { name: name.to_string(), via: Via::Fleet { addr } });
        }
    }
    out
}

fn fleet_addr(h: &Host) -> &str {
    match &h.via {
        Via::Fleet { addr } => addr,
        _ => "",
    }
}

/// A node one of the hubs heard announce itself: its hub name, address, fleet identity.
#[derive(Clone, Debug, PartialEq)]
pub struct Heard {
    pub name: String,
    pub addr: String,
    pub fleet: String,
}

/// The machines the master dials: every `nodes.conf` seed, then each heard node that is not one
/// of them. Machines are matched by fleet identity (a seed's identity is what its certificate
/// proved when it was dialled: `identity(addr)`), and by address before a seed was reached, so
/// `n100` in nodes.conf and the node announcing itself as `100` are one machine.
pub fn merge(seeds: &Hosts, heard: &[Heard], identity: &dyn Fn(&str) -> Option<String>) -> Hosts {
    let mut out = seeds.clone();
    let seed_ids: Vec<String> = seeds.hosts.iter().filter_map(|h| identity(fleet_addr(h))).collect();
    let mut taken: Vec<String> = Vec::new();
    for h in heard {
        let same_seed = seed_ids.contains(&h.fleet) || seeds.hosts.iter().any(|s| fleet_addr(s) == h.addr);
        let named = out.hosts.iter().any(|x| x.name == h.name);
        if same_seed || named || taken.contains(&h.fleet) {
            continue;
        }
        taken.push(h.fleet.clone());
        out.hosts.push(Host { name: h.name.clone(), via: Via::Fleet { addr: h.addr.clone() } });
    }
    out
}

impl Master {
    pub fn start(name: &str, hitch_ms: f64, trust: Option<Arc<dyn Trust>>, on_record: OnRecord) -> Result<Arc<Master>, String> {
        let inbox = Arc::new(Mutex::new(Inbox::open(proto::home().join("inbox.jsonl"))));
        let (ib, user) = (inbox.clone(), on_record);
        let recent: Arc<Mutex<std::collections::VecDeque<Record>>> = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        let rc = recent.clone();
        let on_record: OnRecord = Arc::new(move |r| {
            ib.lock().unwrap().add(r.clone());
            {
                let mut q = rc.lock().unwrap();
                q.push_back(r.clone());
                while q.len() > 400 {
                    q.pop_front();
                }
            }
            user(r);
        });
        let slot: Arc<Mutex<Option<(WatchSink, OnRecord)>>> = Arc::new(Mutex::new(None));
        let s = slot.clone();
        let local = Local::start(
            name,
            hitch_ms,
            false,
            Arc::new(move |r| {
                if let Some((sink, on)) = s.lock().unwrap().as_ref() {
                    sink.emit(&r.to_json());
                    on(&r);
                }
            }),
        );
        let hub = Hub::new(&Hosts::default());
        if let Some(t) = trust {
            hub.set_fleet(Arc::new(move |addr: &str| t.dial(addr)));
        }
        hub.sync(&seeds());
        let heard: Arc<Mutex<Vec<Heard>>> = Arc::new(Mutex::new(Vec::new()));
        let h2 = heard.clone();
        hub.set_source(Arc::new(move || merge(&seeds(), &h2.lock().unwrap(), &id::server_name)));
        // the control socket: route + hosts (the hub's), `watch` with every machine's records,
        // and `ps` / `reply` for this Mac's JITs (local inspect stays `makepad inspect`)
        let ep = hub.client_endpoint();
        let sink = ep.register_watch();
        let l = local.clone();
        ep.register("ps", 1, makepad_remote::lines::service("ps", Arc::new(move |_, out| l.ps(out))));
        // `reply <id> <status> <message>`: to the JIT that posted request <id>, on whichever
        // machine its record came from (request ids carry the poster's host name, not the hub's)
        let (l, ib, me) = (local.clone(), inbox.clone(), name.to_string());
        ep.register(
            "reply",
            1,
            makepad_remote::lines::service(
                "reply",
                Arc::new(move |line, out| {
                    let mut w = line.splitn(3, ' ');
                    let (id, status, msg) = (w.next().unwrap_or(""), w.next().unwrap_or(""), w.next().unwrap_or(""));
                    let machine = ib.lock().unwrap().get(id).map(|e| e.record.machine.clone()).unwrap_or_else(|| me.clone());
                    if machine == me {
                        match l.reply(id, status, msg) {
                            Ok(ack) => out.push(ack),
                            Err(e) => out.push(format!("{{\"err\":{}}}", makepad_remote::json_str(&e))),
                        }
                        return;
                    }
                    let c = Client::new(Target::Unix(proto::control_socket()), "devhub-reply");
                    if let Err(e) = makepad_remote::hub::ask(&c, &machine, "reply", "", line, &mut |a| {
                        out.push(a.to_string());
                        true
                    }) {
                        out.push(format!("{{\"err\":{}}}", makepad_remote::json_str(&format!("{machine}: {e}"))));
                    }
                }),
            ),
        );
        let ib = inbox.clone();
        ep.register("inbox", 1, makepad_remote::lines::service("inbox", Arc::new(move |line, out| inbox_cmd(&ib, line, out))));
        crate::sync::register_control(&ep, &hub);
        let (s, o) = (sink.clone(), on_record.clone());
        let leases = crate::lease::master::register(&ep, name, Arc::new(move |r: Record| {
            s.emit(&r.to_json());
            o(&r);
        }));
        // `jitcache plan|clear`: this Mac's JIT caches (a node's through `route <node> jitcache`)
        let (s, o, me) = (sink.clone(), on_record.clone(), name.to_string());
        ep.register(
            "jitcache",
            1,
            makepad_remote::lines::service(
                "jitcache",
                Arc::new(move |line, out| {
                    crate::jitcache::service(&me, line, out, &|r| {
                        s.emit(&r.to_json());
                        o(&r);
                    })
                }),
            ),
        );
        crate::node::listen_control(&ep)?;
        *slot.lock().unwrap() = Some((sink.clone(), on_record.clone()));
        let m = Arc::new(Master { name: name.to_string(), local, hub, sink, followed: Mutex::new(HashSet::new()), heard, on_record, inbox, leases, recent });
        let m2 = m.clone();
        let _ = std::thread::Builder::new().name("devhub-master".into()).spawn(move || m2.follow_loop());
        let m3 = m.clone();
        m.leases.start(Arc::new(move || {
            m3.hosts()
                .hosts
                .into_iter()
                .map(|h| {
                    let addr = match h.via {
                        Via::Fleet { addr } => addr,
                        _ => String::new(),
                    };
                    (h.name, addr)
                })
                .collect()
        }));
        crate::sync::start_autosync(proto::control_socket());
        Ok(m)
    }

    fn publish(&self, r: Record) {
        self.sink.emit(&r.to_json());
        (self.on_record)(&r);
    }

    /// Follows every node's `watch` (new nodes as nodes.conf and gossip add them).
    fn follow_loop(self: Arc<Self>) {
        let me = Client::new(Target::Unix(proto::control_socket()), "devhub-master");
        loop {
            let hosts = self.hosts();
            self.hub.sync(&hosts);
            // one link per machine: drop names that turned out to be another name of a seed
            let names: Vec<String> = hosts.hosts.iter().map(|h| h.name.clone()).collect();
            self.hub.retain(&|n| names.iter().any(|x| x == n));
            // what the reachable nodes have heard on their LAN
            let mut heard: Vec<Heard> = Vec::new();
            for h in hosts.hosts.iter() {
                let _ = makepad_remote::hub::ask(&me, &h.name, "peers", "", "peers", &mut |l| {
                    if let Ok(v) = makepad_strict_json::parse(l.as_bytes()) {
                        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                        let (name, addr, fleet) = (s("hub"), s("addr"), s("fleet"));
                        if crate::trust::valid_name(&name) && !fleet.is_empty() && addr.parse::<std::net::SocketAddr>().is_ok_and(|a| crate::trust::lan_peer(a.ip())) && !heard.iter().any(|x| x.fleet == fleet) {
                            heard.push(Heard { name, addr, fleet });
                        }
                    }
                    true
                });
            }
            *self.heard.lock().unwrap() = heard;
            for h in self.hosts().hosts {
                if !self.followed.lock().unwrap().insert(h.name.clone()) {
                    continue;
                }
                let m = self.clone();
                let _ = std::thread::Builder::new().name("devhub-follow".into()).spawn(move || m.follow(h.name));
            }
            std::thread::sleep(Duration::from_secs(10));
        }
    }

    /// The machines the master dials: nodes.conf, then the nodes they heard (one per machine).
    pub fn hosts(&self) -> Hosts {
        merge(&seeds(), &self.heard.lock().unwrap(), &id::server_name)
    }

    /// One node's records, through this master's own router (resume and gap markers included).
    fn follow(&self, host: String) {
        let me = Client::new(Target::Unix(proto::control_socket()), "devhub-master");
        let mut feed = Client::routed(&me, &host).watch();
        let mut up = false;
        loop {
            if !self.hosts().hosts.iter().any(|h| h.name == host) {
                // another name of a machine followed under its nodes.conf name
                self.followed.lock().unwrap().remove(&host);
                return;
            }
            match feed.next(Duration::from_secs(2), 1) {
                Ok(Some((_, line))) => {
                    if !up {
                        up = true;
                        self.publish(hub_record(&self.name, "", 0, "hub", Sev::Info, format!("hub: node {host} linked"), String::new()));
                    }
                    let text = String::from_utf8_lossy(&line);
                    match Record::parse(&text) {
                        // under the master's name for the machine (nodes.conf), which routing knows
                        Some(mut r) => {
                            r.machine = host.clone();
                            self.publish(r)
                        }
                        // the route's gap marker while the link was down
                        None if text.contains("\"drop\"") => self.publish(hub_record(&host, "", 0, "hub", Sev::Info, format!("hub: link to {host} was down; records may be missing"), String::new())),
                        None => {}
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    if up {
                        up = false;
                        self.publish(hub_record(&self.name, "", 0, "hub", Sev::Info, format!("hub: node {host} unreachable"), String::new()));
                    }
                    std::thread::sleep(Duration::from_secs(15));
                    feed = Client::routed(&me, &host).watch();
                }
            }
        }
    }
}

/// `inbox list [all] | show <id> | seen <id|all> | clear <id|all>`.
fn inbox_cmd(ib: &Mutex<Inbox>, line: &str, out: &mut Vec<String>) {
    let mut w = line.split_whitespace();
    let mut ib = ib.lock().unwrap();
    match (w.next(), w.next()) {
        (Some("list"), all) => ib.list(all == Some("all"), out),
        (Some("show"), Some(id)) => match ib.get(id) {
            Some(e) => out.push(e.record.to_json()),
            None => out.push(format!("{{\"err\":{}}}", makepad_remote::json_str(&format!("no inbox entry {id}")))),
        },
        (Some("seen"), Some(id)) => {
            ib.mark_seen(id);
            out.push(format!("{{\"unseen\":{}}}", ib.unseen()));
        }
        (Some("clear"), Some(id)) => {
            let n = ib.clear(id);
            out.push(format!("{{\"cleared\":{n},\"left\":{}}}", ib.entries.len()));
        }
        _ => out.push("{\"err\":\"inbox list [all] | show <id> | seen <id|all> | clear <id|all>\"}".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_machine_one_host_by_fleet_identity() {
        let seeds = Hosts { hosts: vec![Host { name: "n100".into(), via: Via::Fleet { addr: "10.0.0.100:8797".into() } }, Host { name: "mini230".into(), via: Via::Fleet { addr: "10.0.0.230:8797".into() } }] };
        let heard = vec![
            // n100 announcing under its default name: same address
            Heard { name: "100".into(), addr: "10.0.0.100:8797".into(), fleet: "node-100".into() },
            // the mini on its Wi-Fi address: same fleet identity as the dialled seed
            Heard { name: "mini".into(), addr: "10.0.0.162:8797".into(), fleet: "mini".into() },
            // a machine nodes.conf lacks
            Heard { name: "217".into(), addr: "10.0.0.217:8797".into(), fleet: "node-217".into() },
            // the same machine heard twice
            Heard { name: "217b".into(), addr: "10.0.0.218:8797".into(), fleet: "node-217".into() },
        ];
        let identity = |addr: &str| match addr {
            "10.0.0.230:8797" => Some("mini".to_string()),
            _ => None,
        };
        let names: Vec<String> = merge(&seeds, &heard, &identity).hosts.into_iter().map(|h| h.name).collect();
        assert_eq!(names, vec!["n100", "mini230", "217"]);
    }
}
