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
use makepad_remote::hosts::{registry_path, Host, Hosts, Via};
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
    learned: Mutex<Hosts>,
    on_record: OnRecord,
    /// what the user has not cleared yet (err / warn / req records of every machine)
    pub inbox: Arc<Mutex<Inbox>>,
    /// leases: the scheduler and its control services (DESIGN.md §10)
    pub leases: Arc<crate::lease::master::LeaseMaster>,
}

/// The master's node list, `~/.makepad/devhub/nodes.conf`: one line per node,
/// `<name> addr=<ip[:port]> [dev=on|off]` (`#` comments). It is the master's seeds (the master
/// never listens for announcements: receiving LAN broadcasts would ask the macOS Local Network
/// question) and the dev switch of each node (DESIGN.md §10.7).
pub fn nodes_conf() -> PathBuf {
    proto::home().join("nodes.conf")
}

/// The node hubs to dial: `via = "fleet"` hosts of the remote registry, then `nodes.conf`.
pub fn seeds() -> Hosts {
    let mut out = Hosts::default();
    for h in Hosts::load(&registry_path()).unwrap_or_default().hosts {
        if matches!(h.via, Via::Fleet { .. }) {
            out.set(h);
        }
    }
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

impl Master {
    pub fn start(name: &str, hitch_ms: f64, trust: Option<Arc<dyn Trust>>, on_record: OnRecord) -> Result<Arc<Master>, String> {
        let inbox = Arc::new(Mutex::new(Inbox::open(proto::home().join("inbox.jsonl"))));
        let (ib, user) = (inbox.clone(), on_record);
        let on_record: OnRecord = Arc::new(move |r| {
            ib.lock().unwrap().add(r.clone());
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
        let registry = Hosts::load(&registry_path()).unwrap_or_default();
        let hub = Hub::new(&registry);
        if let Some(t) = trust {
            hub.set_fleet(Arc::new(move |addr: &str| t.dial(addr)));
        }
        hub.set_source(Arc::new(|| {
            let mut h = Hosts::load(&registry_path()).unwrap_or_default();
            for s in seeds().hosts {
                if !h.hosts.iter().any(|x| x.name == s.name) {
                    h.hosts.push(s);
                }
            }
            h
        }));
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
        crate::node::listen_control(&ep)?;
        *slot.lock().unwrap() = Some((sink.clone(), on_record.clone()));
        let m = Arc::new(Master { name: name.to_string(), local, hub, sink, followed: Mutex::new(HashSet::new()), learned: Mutex::new(Hosts::default()), on_record, inbox, leases });
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
        Ok(m)
    }

    fn publish(&self, r: Record) {
        self.sink.emit(&r.to_json());
        (self.on_record)(&r);
    }

    /// Follows every fleet host's `watch` (new hosts as seeds and gossip add them).
    fn follow_loop(self: Arc<Self>) {
        let me = Client::new(Target::Unix(proto::control_socket()), "devhub-master");
        loop {
            let mut hosts = seeds();
            for h in self.learned.lock().unwrap().hosts.iter() {
                if !hosts.hosts.iter().any(|x| x.name == h.name) {
                    hosts.hosts.push(h.clone());
                }
            }
            self.hub.sync(&hosts);
            // what the reachable nodes have heard on their LAN
            for h in hosts.hosts.iter() {
                let mut found = Vec::new();
                let _ = makepad_remote::hub::ask(&me, &h.name, "peers", "", "peers", &mut |l| {
                    if let Ok(v) = makepad_strict_json::parse(l.as_bytes()) {
                        if let (Some(n), Some(a)) = (v.get("hub").and_then(|x| x.as_str()), v.get("addr").and_then(|x| x.as_str())) {
                            if crate::trust::valid_name(n) && a.parse::<std::net::SocketAddr>().is_ok_and(|a| crate::trust::lan_peer(a.ip())) {
                                found.push(Host { name: n.to_string(), via: Via::Fleet { addr: a.to_string() } });
                            }
                        }
                    }
                    true
                });
                let mut learned = self.learned.lock().unwrap();
                for f in found {
                    if !hosts.hosts.iter().any(|x| x.name == f.name) {
                        learned.set(f);
                    }
                }
            }
            for h in hosts.hosts {
                if !self.followed.lock().unwrap().insert(h.name.clone()) {
                    continue;
                }
                let m = self.clone();
                let _ = std::thread::Builder::new().name("devhub-follow".into()).spawn(move || m.follow(h.name));
            }
            std::thread::sleep(Duration::from_secs(10));
        }
    }

    /// The hosts the master dials: seeds plus what the nodes told it.
    pub fn hosts(&self) -> Hosts {
        let mut h = seeds();
        for l in self.learned.lock().unwrap().hosts.iter() {
            if !h.hosts.iter().any(|x| x.name == l.name) {
                h.hosts.push(l.clone());
            }
        }
        h
    }

    /// One node's records, through this master's own router (resume and gap markers included).
    fn follow(&self, host: String) {
        let me = Client::new(Target::Unix(proto::control_socket()), "devhub-master");
        let mut feed = Client::routed(&me, &host).watch();
        let mut up = false;
        loop {
            match feed.next(Duration::from_secs(2), 1) {
                Ok(Some((_, line))) => {
                    if !up {
                        up = true;
                        self.publish(hub_record(&self.name, "", 0, "hub", Sev::Info, format!("hub: node {host} linked"), String::new()));
                    }
                    let text = String::from_utf8_lossy(&line);
                    match Record::parse(&text) {
                        Some(r) => self.publish(r),
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
