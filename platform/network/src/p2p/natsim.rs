//! A deterministic in-process internet, for tests that drive NAT traversal end to end with no
//! sockets and no wall clock: public hosts, private hosts behind one or more NATs, seeded latency
//! and loss, and a virtual time in ms that only the test moves.
//!
//! NAT behaviour follows RFC 4787's terms:
//! - [`NatKind::FullCone`]: one mapping per internal socket; anyone may send to it.
//! - [`NatKind::Restricted`]: one mapping per socket; only addresses (any port) it sent to may answer.
//! - [`NatKind::PortRestricted`]: one mapping per socket; only the exact addresses and ports it sent to.
//! - [`NatKind::Symmetric`]: a mapping per destination; only that destination may answer.
//!
//! Mappings expire after `mapping_timeout_ms` without outbound traffic (inbound traffic never
//! refreshes them, the conservative reading). A router port mapping (what PCP, NAT-PMP or UPnP
//! grant) is [`Net::forward`]: inbound from anyone, and with `outbound` the NAT also sends the
//! socket's traffic from that port, as PCP's MAP does.
//!
//! Driving it: send with [`Net::send`]; move time with [`Net::advance_to`], which returns what arrived
//! at that moment in order. A loop takes the earlier of [`Net::next_at`] and its agents' own next
//! wake, so every agent sees each packet at its exact arrival time.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NatKind {
    FullCone,
    Restricted,
    PortRestricted,
    Symmetric,
}

impl NatKind {
    pub const ALL: [NatKind; 4] = [NatKind::FullCone, NatKind::Restricted, NatKind::PortRestricted, NatKind::Symmetric];
}

/// How a NAT picks a mapping's public port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortAlloc {
    /// The internal port when it is free, else the next free one.
    Preserve,
    /// The next free port after the last one handed out.
    Sequential,
    /// A seeded random free port.
    Random,
}

#[derive(Clone, Debug)]
pub struct NatConfig {
    pub kind: NatKind,
    pub ports: PortAlloc,
    pub mapping_timeout_ms: u64,
    /// Packets from inside to the NAT's own public address loop back in (hairpinning).
    pub hairpin: bool,
}

impl NatConfig {
    /// A typical home router of this kind: symmetric ones hand out sequential ports, the others
    /// keep the internal port; mappings live two minutes; no hairpinning.
    pub fn new(kind: NatKind) -> NatConfig {
        let ports = if kind == NatKind::Symmetric { PortAlloc::Sequential } else { PortAlloc::Preserve };
        NatConfig { kind, ports, mapping_timeout_ms: 120_000, hairpin: false }
    }
}

/// Every path's latency (base plus seeded jitter) and seeded loss.
#[derive(Clone, Copy, Debug)]
pub struct LinkConfig {
    pub latency_ms: u64,
    pub jitter_ms: u64,
    pub loss_per_mille: u32,
}

impl Default for LinkConfig {
    fn default() -> Self {
        LinkConfig { latency_ms: 30, jitter_ms: 10, loss_per_mille: 0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NatId(usize);

/// What happened to the packets sent so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub sent: u64,
    pub delivered: u64,
    /// Dropped by the link's loss.
    pub lost: u64,
    /// Dropped by a NAT's filter (no mapping, or not from an address the mapping sent to).
    pub filtered: u64,
    /// To an address no host has, or to the NAT's own address without hairpinning.
    pub unroutable: u64,
}

/// A packet arriving at a host's socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    pub to: SocketAddr,
    pub from: SocketAddr,
    pub bytes: Vec<u8>,
}

struct Mapping {
    internal: SocketAddrV4,
    /// The one destination of a symmetric NAT's mapping.
    destination: Option<SocketAddrV4>,
    sent_to: BTreeSet<SocketAddrV4>,
    last_out: u64,
}

struct Forward {
    internal: SocketAddrV4,
    outbound: bool,
    sent_to: BTreeSet<SocketAddrV4>,
}

struct Nat {
    outside: Ipv4Addr,
    behind: Option<NatId>,
    config: NatConfig,
    by_port: BTreeMap<u16, Mapping>,
    forwards: BTreeMap<u16, Forward>,
    next_port: u16,
}

impl Nat {
    fn expire(&mut self, now: u64) {
        let timeout = self.config.mapping_timeout_ms;
        self.by_port.retain(|_, m| now.saturating_sub(m.last_out) < timeout);
    }

    fn port_free(&self, port: u16) -> bool {
        port >= 1024 && !self.by_port.contains_key(&port) && !self.forwards.contains_key(&port)
    }

    fn outbound(&mut self, internal: SocketAddrV4, to: SocketAddrV4, now: u64, rng: &mut Rng) -> u16 {
        if let Some((port, forward)) = self.forwards.iter_mut().find(|(_, f)| f.internal == internal && f.outbound) {
            forward.sent_to.insert(to);
            return *port;
        }
        self.expire(now);
        let symmetric = self.config.kind == NatKind::Symmetric;
        let existing = self
            .by_port
            .iter_mut()
            .find(|(_, m)| m.internal == internal && (!symmetric || m.destination == Some(to)));
        if let Some((port, mapping)) = existing {
            mapping.sent_to.insert(to);
            mapping.last_out = now;
            return *port;
        }
        let port = self.allocate(internal.port(), rng);
        let mut sent_to = BTreeSet::new();
        sent_to.insert(to);
        self.by_port.insert(port, Mapping { internal, destination: symmetric.then_some(to), sent_to, last_out: now });
        port
    }

    fn allocate(&mut self, wanted: u16, rng: &mut Rng) -> u16 {
        if self.config.ports == PortAlloc::Preserve && self.port_free(wanted) {
            return wanted;
        }
        loop {
            let port = match self.config.ports {
                PortAlloc::Random => 1024 + (rng.next() % (65536 - 1024)) as u16,
                PortAlloc::Preserve | PortAlloc::Sequential => {
                    let port = self.next_port;
                    self.next_port = if self.next_port == u16::MAX { 1024 } else { self.next_port + 1 };
                    port
                }
            };
            if self.port_free(port) {
                return port;
            }
        }
    }

    fn inbound(&mut self, port: u16, from: SocketAddrV4, now: u64) -> Option<SocketAddrV4> {
        if let Some(forward) = self.forwards.get(&port) {
            return Some(forward.internal);
        }
        self.expire(now);
        let mapping = self.by_port.get(&port)?;
        let allowed = match self.config.kind {
            NatKind::FullCone => true,
            NatKind::Restricted => mapping.sent_to.iter().any(|to| to.ip() == from.ip()),
            NatKind::PortRestricted | NatKind::Symmetric => mapping.sent_to.contains(&from),
        };
        allowed.then_some(mapping.internal)
    }
}

/// SplitMix64: small, seeded, the same on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

struct InFlight {
    /// Where the packet is going, and who it appears to be from on the wire.
    to: SocketAddrV4,
    from: SocketAddrV4,
    /// The network reached after outbound translation. Each inbound translation enters a child network.
    network: Option<NatId>,
    bytes: Vec<u8>,
}

/// The simulated internet.
pub struct Net {
    now: u64,
    seq: u64,
    rng: Rng,
    link: LinkConfig,
    nats: Vec<Nat>,
    /// A router's outside interface is reachable only on the network it is plugged into.
    interfaces: HashMap<(Option<NatId>, Ipv4Addr), NatId>,
    /// Every host's address, and the NAT it sits behind (`None`: on the public internet).
    hosts: HashMap<Ipv4Addr, Option<NatId>>,
    queue: BTreeMap<(u64, u64), InFlight>,
    stats: Stats,
}

fn v4(addr: SocketAddr) -> SocketAddrV4 {
    match addr {
        SocketAddr::V4(addr) => addr,
        SocketAddr::V6(addr) => panic!("natsim is IPv4 only: {addr}"),
    }
}

impl Net {
    pub fn new(seed: u64, link: LinkConfig) -> Net {
        Net { now: 0, seq: 0, rng: Rng(seed), link, nats: Vec::new(), interfaces: HashMap::new(), hosts: HashMap::new(), queue: BTreeMap::new(), stats: Stats::default() }
    }

    pub fn now(&self) -> u64 {
        self.now
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    pub fn set_link(&mut self, link: LinkConfig) {
        self.link = link;
    }

    /// A NAT with public address `public`.
    pub fn add_nat(&mut self, public: Ipv4Addr, config: NatConfig) -> NatId {
        self.register_nat(public, None, config)
    }

    /// A NAT whose outside interface is on `behind`'s private network.
    pub fn add_nat_behind(&mut self, outside: Ipv4Addr, behind: NatId, config: NatConfig) -> NatId {
        assert!(behind.0 < self.nats.len(), "unknown parent NAT");
        self.register_nat(outside, Some(behind), config)
    }

    fn register_nat(&mut self, outside: Ipv4Addr, behind: Option<NatId>, config: NatConfig) -> NatId {
        assert!(self.hosts.get(&outside) != Some(&behind) && !self.interfaces.contains_key(&(behind, outside)), "{outside} is taken");
        let id = NatId(self.nats.len());
        self.nats.push(Nat { outside, behind, config, by_port: BTreeMap::new(), forwards: BTreeMap::new(), next_port: 40000 });
        self.interfaces.insert((behind, outside), id);
        id
    }

    /// A host at `ip`: on the public internet (`behind: None`) or inside a NAT's private network.
    pub fn add_host(&mut self, ip: Ipv4Addr, behind: Option<NatId>) {
        assert!(!self.hosts.contains_key(&ip) && !self.interfaces.contains_key(&(behind, ip)), "{ip} is taken");
        self.hosts.insert(ip, behind);
    }

    /// A router port mapping: `external_port` on its outside interface reaches `internal` from
    /// anyone on that network. With `outbound`, `internal`'s traffic also leaves from that port.
    pub fn forward(&mut self, nat: NatId, external_port: u16, internal: SocketAddr, outbound: bool) {
        let nat = &mut self.nats[nat.0];
        nat.by_port.remove(&external_port);
        nat.forwards.insert(external_port, Forward { internal: v4(internal), outbound, sent_to: BTreeSet::new() });
    }

    pub fn unforward(&mut self, nat: NatId, external_port: u16) {
        self.nats[nat.0].forwards.remove(&external_port);
    }

    /// The NAT forgets every dynamic mapping (a router reboot, or a rebinding).
    pub fn flush_mappings(&mut self, nat: NatId) {
        self.nats[nat.0].by_port.clear();
    }

    /// This NAT's outside address, which may be private when it sits behind another NAT.
    pub fn nat_public(&self, nat: NatId) -> Ipv4Addr {
        self.nats[nat.0].outside
    }

    /// Sends `bytes` from a host's socket `from` to `to`, at the current time.
    pub fn send(&mut self, from: SocketAddr, to: SocketAddr, bytes: Vec<u8>) {
        let (mut from, to) = (v4(from), v4(to));
        let mut network = *self.hosts.get(from.ip()).unwrap_or_else(|| panic!("natsim: no host {}", from.ip()));
        self.stats.sent += 1;
        while let Some(id) = network {
            if self.hosts.get(to.ip()) == Some(&network) || self.interfaces.contains_key(&(network, *to.ip())) {
                break;
            }
            let nat = &mut self.nats[id.0];
            if to.ip() == &nat.outside && !nat.config.hairpin {
                self.stats.unroutable += 1;
                return;
            }
            let port = nat.outbound(from, to, self.now, &mut self.rng);
            from = SocketAddrV4::new(nat.outside, port);
            network = nat.behind;
        }
        if self.link.loss_per_mille > 0 && (self.rng.next() % 1000) < u64::from(self.link.loss_per_mille) {
            self.stats.lost += 1;
            return;
        }
        let jitter = if self.link.jitter_ms > 0 { self.rng.next() % (self.link.jitter_ms + 1) } else { 0 };
        let at = self.now + (self.link.latency_ms + jitter).max(1);
        self.seq += 1;
        self.queue.insert((at, self.seq), InFlight { to, from, network, bytes });
    }

    /// When the next packet arrives.
    pub fn next_at(&self) -> Option<u64> {
        self.queue.keys().next().map(|(at, _)| *at)
    }

    /// Moves time to `t` (never backwards) and returns what arrived up to then, in order.
    pub fn advance_to(&mut self, t: u64) -> Vec<Delivery> {
        let mut out = Vec::new();
        while let Some(entry) = self.queue.first_entry() {
            let (at, _) = *entry.key();
            if at > t {
                break;
            }
            let packet = entry.remove();
            self.now = self.now.max(at);
            if let Some(delivery) = self.arrive(packet) {
                out.push(delivery);
            }
        }
        self.now = self.now.max(t);
        out
    }

    fn arrive(&mut self, packet: InFlight) -> Option<Delivery> {
        let (mut to, mut network) = (packet.to, packet.network);
        while self.hosts.get(to.ip()) != Some(&network) {
            let Some(&id) = self.interfaces.get(&(network, *to.ip())) else {
                self.stats.unroutable += 1;
                return None;
            };
            let Some(internal) = self.nats[id.0].inbound(to.port(), packet.from, self.now) else {
                self.stats.filtered += 1;
                return None;
            };
            to = internal;
            network = Some(id);
        }
        self.stats.delivered += 1;
        Some(Delivery { to: SocketAddr::V4(to), from: SocketAddr::V4(packet.from), bytes: packet.bytes })
    }
}

/// `IpAddr` shorthand for tests and callers.
pub fn ip(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(a, b, c, d))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    /// Sends `from` -> `to`, then delivers everything in flight.
    fn exchange(net: &mut Net, from: &str, to: &str) -> Option<Delivery> {
        net.send(addr(from), addr(to), b"hello".to_vec());
        let t = net.next_at().unwrap_or(net.now());
        net.advance_to(t).pop()
    }

    #[test]
    fn nat_kinds_map_and_filter_as_rfc4787_says() {
        for kind in NatKind::ALL {
            let mut net = Net::new(1, LinkConfig::default());
            let nat = net.add_nat(Ipv4Addr::new(198, 51, 100, 1), NatConfig::new(kind));
            net.add_host(Ipv4Addr::new(10, 77, 0, 2), Some(nat));
            for server in [Ipv4Addr::new(203, 0, 113, 9), Ipv4Addr::new(203, 0, 113, 10)] {
                net.add_host(server, None);
            }
            let inside = "10.77.0.2:5000";
            let seen_a = exchange(&mut net, inside, "203.0.113.9:3478").expect("outbound passes").from;
            let seen_b = exchange(&mut net, inside, "203.0.113.9:3479").expect("outbound passes").from;
            assert_eq!(seen_a.ip(), ip(198, 51, 100, 1), "{kind:?}: the public address");
            assert_eq!(seen_a == seen_b, kind != NatKind::Symmetric, "{kind:?}: one mapping per socket unless symmetric");
            if kind != NatKind::Symmetric {
                assert_eq!(seen_a.port(), 5000, "{kind:?}: the internal port is kept");
            }
            let back = exchange(&mut net, "203.0.113.9:3478", &seen_a.to_string()).expect("the answer to a sent packet passes");
            assert_eq!(back.to, addr(inside), "{kind:?}: translated back in");
            let other_port = exchange(&mut net, "203.0.113.9:4000", &seen_a.to_string());
            assert_eq!(other_port.is_some(), matches!(kind, NatKind::FullCone | NatKind::Restricted), "{kind:?}: same address, other port");
            let other_host = exchange(&mut net, "203.0.113.10:3478", &seen_a.to_string());
            assert_eq!(other_host.is_some(), kind == NatKind::FullCone, "{kind:?}: a host never sent to");
        }
    }

    #[test]
    fn router_behind_modem_translates_and_filters_at_both_hops() {
        let mut net = Net::new(7, LinkConfig::default());
        let mut modem_config = NatConfig::new(NatKind::PortRestricted);
        modem_config.ports = PortAlloc::Sequential;
        let modem = net.add_nat(Ipv4Addr::new(198, 51, 100, 23), modem_config);
        let router = net.add_nat_behind(Ipv4Addr::new(192, 168, 77, 2), modem, NatConfig::new(NatKind::PortRestricted));
        net.add_host(Ipv4Addr::new(10, 77, 0, 2), Some(router));
        net.add_host(Ipv4Addr::new(192, 168, 77, 3), Some(modem));
        net.add_host(Ipv4Addr::new(203, 0, 113, 9), None);
        let inside = "10.77.0.2:5000";
        let server = "203.0.113.9:3478";
        let unsolicited = "203.0.113.9:9999";
        let seen = exchange(&mut net, inside, server).expect("outbound through both NATs").from;
        assert_eq!(seen.ip(), ip(198, 51, 100, 23), "the public host sees the modem");
        assert_ne!(seen.port(), addr(inside).port(), "the modem allocates its own port");
        let reply = exchange(&mut net, server, &seen.to_string()).expect("both mappings admit the reply");
        assert_eq!((reply.from, reply.to), (addr(server), addr(inside)));
        assert!(exchange(&mut net, unsolicited, &seen.to_string()).is_none(), "the modem filters an unsolicited packet");

        net.forward(router, 6000, addr(inside), true);
        let neighbor = "192.168.77.3:5000";
        let on_lan = exchange(&mut net, neighbor, "192.168.77.2:6000").expect("the inner forward is reachable on the modem LAN");
        assert_eq!((on_lan.from, on_lan.to), (addr(neighbor), addr(inside)));
        assert_eq!(exchange(&mut net, inside, neighbor).unwrap().from, addr("192.168.77.2:6000"), "a LAN reply crosses only the router");
        assert!(exchange(&mut net, unsolicited, "192.168.77.2:6000").is_none(), "the router's outside is private");
        assert!(exchange(&mut net, unsolicited, "198.51.100.23:6000").is_none(), "only the router forwarded");
        net.forward(modem, 7000, addr("192.168.77.2:6001"), true);
        assert!(exchange(&mut net, unsolicited, "198.51.100.23:7000").is_none(), "the router filters the unforwarded port");
        net.forward(modem, 7000, addr("192.168.77.2:6000"), true);
        let forwarded = exchange(&mut net, unsolicited, "198.51.100.23:7000").expect("both ports forwarded");
        assert_eq!((forwarded.from, forwarded.to), (addr(unsolicited), addr(inside)));
        assert_eq!(exchange(&mut net, inside, server).unwrap().from, addr("198.51.100.23:7000"), "both outbound forwards apply");
    }

    #[test]
    fn mappings_expire_forwards_stay_and_loss_is_seeded() {
        let mut net = Net::new(7, LinkConfig { latency_ms: 20, jitter_ms: 0, loss_per_mille: 0 });
        let mut config = NatConfig::new(NatKind::PortRestricted);
        config.mapping_timeout_ms = 30_000;
        let nat = net.add_nat(Ipv4Addr::new(198, 51, 100, 1), config);
        net.add_host(Ipv4Addr::new(10, 77, 0, 2), Some(nat));
        net.add_host(Ipv4Addr::new(203, 0, 113, 9), None);
        let seen = exchange(&mut net, "10.77.0.2:5000", "203.0.113.9:3478").unwrap().from;
        net.advance_to(net.now() + 29_000);
        assert!(exchange(&mut net, "203.0.113.9:3478", &seen.to_string()).is_some(), "alive before the timeout");
        net.advance_to(net.now() + 31_000);
        assert!(exchange(&mut net, "203.0.113.9:3478", &seen.to_string()).is_none(), "inbound traffic doesn't refresh: expired");
        assert_eq!(net.stats().filtered, 1, "the expired mapping filtered it");

        net.forward(nat, 6000, addr("10.77.0.2:5000"), true);
        let anyone = exchange(&mut net, "203.0.113.9:9999", "198.51.100.1:6000").expect("a forward takes anyone");
        assert_eq!(anyone.to, addr("10.77.0.2:5000"), "the forward's internal address");
        assert_eq!(exchange(&mut net, "10.77.0.2:5000", "203.0.113.9:3478").unwrap().from, addr("198.51.100.1:6000"), "outbound from the forward's port");

        let lossy = |seed| {
            let mut net = Net::new(seed, LinkConfig { latency_ms: 20, jitter_ms: 15, loss_per_mille: 300 });
            net.add_host(Ipv4Addr::new(203, 0, 113, 9), None);
            net.add_host(Ipv4Addr::new(203, 0, 113, 10), None);
            for i in 0..200u8 {
                net.send(addr("203.0.113.9:1"), addr("203.0.113.10:1"), vec![i]);
            }
            let arrived: Vec<u8> = net.advance_to(1000).into_iter().map(|d| d.bytes[0]).collect();
            (arrived, net.stats())
        };
        let (first, stats) = lossy(3);
        assert_eq!(lossy(3).0, first, "the same seed, the same packets in the same order");
        assert!(stats.lost > 30 && stats.lost < 90, "about 30% lost: {stats:?}");
        assert_eq!(stats.delivered + stats.lost, 200, "every packet accounted for");
    }
}
