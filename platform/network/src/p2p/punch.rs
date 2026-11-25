//! The simultaneous open (hole punching), sans-IO: both sides send small probes to the address the
//! other read out, so each firewall sees the other's packets as answers to its own. Bytes and the
//! time in, datagrams and events out; the caller owns the UDP socket (the same one the reflector
//! saw), so the NAT simulator ([`super::natsim`]) drives this exact code.
//!
//! - **Probes** go to every address known for the peer: the one from its words, and any address a
//!   probe of its arrived from (peer-reflexive, as ICE calls it: a symmetric NAT or a router mapping
//!   shows up as a source the words didn't name). A burst first, then about one a second. Probing
//!   never gives up by itself: the two people type each other's words at different moments, so the
//!   caller decides when to stop and what to tell them ([`Punch::state`] carries how long).
//! - **Open** when the peer acknowledges one of our probes: both directions work.
//! - **Keepalives** after that keep the NATs' mappings alive; traffic of the layers above counts
//!   ([`Punch::note_sent`], [`Punch::note_heard`]). A peer silent for [`Config::quiet_ms`] makes the
//!   path quiet, never closed: probing resumes, and the path reopens, possibly from a new address
//!   when the peer's NAT rebound it. A probe from an address that isn't the open peer's is probed
//!   straight back, so a rebound peer is followed within one round trip of its next packet. Such a
//!   move is only a candidate: the channel on top must see authenticated traffic from the new
//!   address before trusting it, since anyone who knows the tag can answer a probe.
//!
//! The wire: `0xE0 | type (1 probe, 2 ack) | tag (8) | nonce (8)`, 18 bytes. The first byte keeps it
//! apart from STUN (0..=3) and from the layers above ([`KIND`]). The tag names this pairing (both
//! sides derive it from both invites), so stray probes of other pairings are ignored; it is not a
//! secret, and nothing here authenticates the peer: the channel on top does, and a path that fails
//! that check is the caller's to abandon.

use std::net::SocketAddr;

/// The first byte of every punch datagram.
pub const KIND: u8 = 0xE0;
const PROBE: u8 = 1;
const ACK: u8 = 2;
pub const LEN: usize = 18;

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// This pairing's tag.
    pub tag: [u8; 8],
    /// Probes this often for the first `burst_ms` of punching (and of a quiet path).
    pub burst_interval_ms: u64,
    pub burst_ms: u64,
    /// Then this often, for as long as the caller lets it run.
    pub slow_interval_ms: u64,
    /// An open path with nothing sent for this long gets a keepalive probe.
    pub keepalive_ms: u64,
    /// An open path with nothing heard for this long is quiet.
    pub quiet_ms: u64,
}

impl Config {
    pub fn new(tag: [u8; 8]) -> Config {
        Config { tag, burst_interval_ms: 100, burst_ms: 2_000, slow_interval_ms: 1_000, keepalive_ms: 15_000, quiet_ms: 45_000 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Probing since `since`, nothing acknowledged yet.
    Punching { since: u64 },
    /// Both directions work, to and from `peer`.
    Open { peer: SocketAddr, since: u64 },
    /// It was open to `peer`, but nothing has been heard since `since`; probing again.
    Quiet { peer: SocketAddr, since: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Opened(SocketAddr),
    /// Open again after being quiet; `moved` when the peer's address changed.
    Reopened { peer: SocketAddr, moved: bool },
    Quiet(SocketAddr),
}

pub struct Punch {
    config: Config,
    nonce: [u8; 8],
    targets: Vec<SocketAddr>,
    state: State,
    /// When the current burst began (punching, or going quiet).
    burst_from: u64,
    next_probe_at: u64,
    last_sent: u64,
    last_heard: u64,
}

/// Whether `packet` is a punch datagram (for the caller's demultiplexing).
pub fn is_punch(packet: &[u8]) -> bool {
    packet.len() == LEN && packet[0] == KIND
}

fn datagram(kind: u8, tag: &[u8; 8], nonce: &[u8; 8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(LEN);
    out.push(KIND);
    out.push(kind);
    out.extend_from_slice(tag);
    out.extend_from_slice(nonce);
    out
}

impl Punch {
    /// Starts punching toward `peer` (the address from its words) at `now`. `nonce` is random per
    /// punch (the OS's random source in use); the peer's ack must echo it.
    pub fn new(config: Config, peer: SocketAddr, nonce: [u8; 8], now: u64) -> Punch {
        Punch {
            config,
            nonce,
            targets: vec![peer],
            state: State::Punching { since: now },
            burst_from: now,
            next_probe_at: now,
            last_sent: now,
            last_heard: now,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// The open path's peer address.
    pub fn peer(&self) -> Option<SocketAddr> {
        match self.state {
            State::Open { peer, .. } => Some(peer),
            _ => None,
        }
    }

    /// Every address probed: the words' one, then those learned from the peer's probes.
    pub fn targets(&self) -> &[SocketAddr] {
        &self.targets
    }

    fn probe_interval(&self, now: u64) -> u64 {
        if now.saturating_sub(self.burst_from) < self.config.burst_ms {
            self.config.burst_interval_ms
        } else {
            self.config.slow_interval_ms
        }
    }

    /// The datagrams due at `now`, as (to, bytes); and the quiet transition when it is due.
    pub fn poll(&mut self, now: u64) -> (Vec<(SocketAddr, Vec<u8>)>, Option<Event>) {
        let mut event = None;
        if let State::Open { peer, .. } = self.state {
            if now.saturating_sub(self.last_heard) >= self.config.quiet_ms {
                self.state = State::Quiet { peer, since: self.last_heard };
                self.burst_from = now;
                self.next_probe_at = now;
                event = Some(Event::Quiet(peer));
            }
        }
        let mut out = Vec::new();
        match self.state {
            State::Open { peer, .. } => {
                if now.saturating_sub(self.last_sent) >= self.config.keepalive_ms {
                    out.push((peer, datagram(PROBE, &self.config.tag, &self.nonce)));
                    self.last_sent = now;
                }
            }
            State::Punching { .. } | State::Quiet { .. } => {
                if now >= self.next_probe_at {
                    for target in &self.targets {
                        out.push((*target, datagram(PROBE, &self.config.tag, &self.nonce)));
                    }
                    self.last_sent = now;
                    self.next_probe_at = now + self.probe_interval(now);
                }
            }
        }
        (out, event)
    }

    /// When [`Punch::poll`] next has something to do.
    pub fn next_wake(&self) -> u64 {
        match self.state {
            State::Open { .. } => (self.last_sent + self.config.keepalive_ms).min(self.last_heard + self.config.quiet_ms),
            State::Punching { .. } | State::Quiet { .. } => self.next_probe_at,
        }
    }

    /// Takes a punch datagram from `from`: answers a probe (an ack, and a probe of ours when `from`
    /// isn't the open peer), to send now, and opens on an ack of ours. Datagrams of other pairings,
    /// and anything malformed, change nothing.
    pub fn handle(&mut self, from: SocketAddr, packet: &[u8], now: u64) -> (Vec<(SocketAddr, Vec<u8>)>, Option<Event>) {
        if !is_punch(packet) || packet[2..10] != self.config.tag {
            return (Vec::new(), None);
        }
        let mut nonce = [0u8; 8];
        nonce.copy_from_slice(&packet[10..18]);
        match packet[1] {
            PROBE => {
                if !self.targets.contains(&from) {
                    self.targets.push(from);
                }
                let mut out = vec![(from, datagram(ACK, &self.config.tag, &nonce))];
                if self.peer() == Some(from) {
                    self.last_heard = now;
                } else {
                    out.push((from, datagram(PROBE, &self.config.tag, &self.nonce)));
                }
                self.last_sent = now;
                (out, None)
            }
            ACK if nonce == self.nonce => {
                self.last_heard = now;
                let event = match self.state {
                    State::Punching { .. } => Some(Event::Opened(from)),
                    State::Quiet { peer, .. } => Some(Event::Reopened { peer: from, moved: peer != from }),
                    State::Open { peer, .. } if peer != from => Some(Event::Reopened { peer: from, moved: true }),
                    State::Open { .. } => None,
                };
                if event.is_some() {
                    self.state = State::Open { peer: from, since: now };
                    if !self.targets.contains(&from) {
                        self.targets.push(from);
                    }
                }
                (Vec::new(), event)
            }
            _ => (Vec::new(), None),
        }
    }

    /// The layers above sent a datagram on the open path at `now` (it keeps the NAT mapping alive).
    pub fn note_sent(&mut self, now: u64) {
        self.last_sent = self.last_sent.max(now);
    }

    /// The layers above took a datagram from `from` at `now` that the channel on top authenticated.
    pub fn note_heard(&mut self, from: SocketAddr, now: u64) {
        if self.peer() == Some(from) {
            self.last_heard = self.last_heard.max(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::discover::{self, Assessment, Found, Hint};
    use super::super::natsim::{LinkConfig, NatConfig, NatId, NatKind, Net, PortAlloc};
    use super::super::reflect::{self, Mapping};
    use super::super::words::{Invite, TEST_COMMITMENT};
    use super::*;
    use std::net::{Ipv4Addr, SocketAddrV4};

    const REFLECTORS: [Ipv4Addr; 2] = [Ipv4Addr::new(203, 0, 113, 9), Ipv4Addr::new(203, 0, 113, 10)];
    const PUBLICS: [Ipv4Addr; 2] = [Ipv4Addr::new(198, 51, 100, 23), Ipv4Addr::new(192, 0, 2, 7)];

    struct Side {
        nat: NatId,
        local: SocketAddr,
        query: reflect::Query,
        router_grant: Option<SocketAddrV4>,
        /// What the other person reads from this side's words.
        invite: Option<Invite>,
        words_at: u64,
        punch: Option<Punch>,
        opened_at: Option<u64>,
        events: Vec<Event>,
    }

    impl Side {
        fn write_words(&mut self, now: u64) {
            let reflection = self.query.result();
            let reflected = reflection.mapped().map(|addr| {
                let SocketAddr::V4(addr) = addr else { unreachable!("IPv4 simulator") };
                (addr, reflection.mapping())
            });
            let mut choice = discover::choose(&Found { router: None, reflected });
            if let Some(grant) = self.router_grant {
                choice = choice.with_simulated_grant(grant);
            }
            if let Some(addr) = choice.address {
                let invite = Invite { addr, nat: choice.hint, commit: TEST_COMMITMENT };
                self.invite = Some(Invite::from_words(&invite.to_words().join(" ")).unwrap().0);
                self.words_at = now;
            }
        }
    }

    struct World {
        net: Net,
        sides: [Side; 2],
    }

    /// Two people behind NATs of `kinds`; `forwards[i]`: side i's router granted a mapping
    /// (outbound too, as PCP's MAP does).
    fn world(seed: u64, link: LinkConfig, kinds: [NatKind; 2], forwards: [bool; 2]) -> World {
        world_with(seed, link, kinds.map(NatConfig::new), forwards)
    }

    fn world_with(seed: u64, link: LinkConfig, configs: [NatConfig; 2], forwards: [bool; 2]) -> World {
        world_with_modems(seed, link, configs, [None, None], forwards)
    }

    fn world_with_modems(seed: u64, link: LinkConfig, routers: [NatConfig; 2], modems: [Option<NatKind>; 2], forwards: [bool; 2]) -> World {
        let mut net = Net::new(seed, link);
        for reflector in REFLECTORS {
            net.add_host(reflector, None);
        }
        let reflectors: Vec<SocketAddr> = REFLECTORS.iter().map(|ip| SocketAddr::new((*ip).into(), 3478)).collect();
        let privates = [Ipv4Addr::new(10, 77, 0, 2), Ipv4Addr::new(192, 168, 77, 20)];
        let router_outsides = [Ipv4Addr::new(192, 168, 77, 2), Ipv4Addr::new(100, 64, 0, 2)];
        let mut seeds = seed.wrapping_mul(977);
        let sides = [0, 1].map(|i| {
            let nat = if let Some(kind) = modems[i] {
                let mut config = NatConfig::new(kind);
                // Even a cone modem may allocate a different port from the router's grant.
                config.ports = PortAlloc::Sequential;
                let modem = net.add_nat(PUBLICS[i], config);
                net.add_nat_behind(router_outsides[i], modem, routers[i].clone())
            } else {
                net.add_nat(PUBLICS[i], routers[i].clone())
            };
            net.add_host(privates[i], Some(nat));
            let local = SocketAddr::V4(SocketAddrV4::new(privates[i], 51820));
            let router_grant = forwards[i].then(|| {
                let port = 41000 + i as u16;
                net.forward(nat, port, local, true);
                SocketAddrV4::new(net.nat_public(nat), port)
            });
            let query = reflect::Query::new(
                &reflectors,
                reflect::Style::Padded,
                &mut |bytes| {
                    for byte in bytes {
                        seeds = seeds.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                        *byte = (seeds >> 56) as u8;
                    }
                },
                0,
            );
            let mut side = Side { nat, local, query, router_grant, invite: None, words_at: 0, punch: None, opened_at: None, events: Vec::new() };
            side.write_words(net.now());
            side
        });
        World { net, sides }
    }

    impl World {
        /// Runs until `until`; side `i` starts punching once its reflection is in and `starts[i]` has come.
        fn run(&mut self, starts: [u64; 2], until: u64) {
            // The raw punch tests also probe pairings that discovery would decline.
            self.drive(starts, until, false);
        }

        fn assessment(&self) -> Option<Assessment> {
            let [a, b] = &self.sides;
            Some(discover::assess(a.invite.as_ref()?.nat, b.invite.as_ref()?.nat))
        }

        fn drive(&mut self, starts: [u64; 2], until: u64, stop_on_outcome: bool) {
            let tag = [0x5a; 8];
            loop {
                let now = self.net.now();
                // Start punching where both words are known and this side's person has typed them.
                if let Some(verdict) = self.assessment() {
                    if stop_on_outcome && (verdict == Assessment::NotPossible || self.sides.iter().all(|s| s.opened_at.is_some())) {
                        return;
                    }
                    for i in 0..2 {
                        if self.sides[i].punch.is_none() && now >= starts[i] {
                            let peer = self.sides[1 - i].invite.as_ref().unwrap().addr.into();
                            self.sides[i].punch = Some(Punch::new(Config::new(tag), peer, [i as u8 + 1; 8], now));
                        }
                    }
                }
                for side in &mut self.sides {
                    for (to, bytes) in side.query.poll(now) {
                        self.net.send(side.local, to, bytes);
                    }
                    if let Some(punch) = &mut side.punch {
                        let (out, event) = punch.poll(now);
                        side.events.extend(event);
                        for (to, bytes) in out {
                            self.net.send(side.local, to, bytes);
                        }
                    }
                }
                let wakes = self.sides.iter().flat_map(|s| {
                    let start = if s.punch.is_none() { starts.iter().copied().filter(|t| *t > now).min() } else { None };
                    [s.query.next_wake(), s.punch.as_ref().map(Punch::next_wake), start]
                });
                let next = wakes.flatten().chain(self.net.next_at()).filter(|t| *t > now).min().unwrap_or(until).min(until);
                if now >= until {
                    return;
                }
                for delivery in self.net.advance_to(next) {
                    let now = self.net.now();
                    if REFLECTORS.iter().any(|ip| delivery.to.ip() == std::net::IpAddr::V4(*ip)) {
                        if let Some(answer) = reflect::answer(&delivery.bytes, delivery.from) {
                            self.net.send(delivery.to, delivery.from, answer);
                        }
                        continue;
                    }
                    let side = self.sides.iter_mut().find(|s| s.local == delivery.to).expect("a delivery to a side");
                    if side.query.handle(delivery.from, &delivery.bytes) {
                        if side.query.done() && side.invite.is_none() {
                            side.write_words(now);
                        }
                        continue;
                    }
                    if let Some(punch) = &mut side.punch {
                        let (out, event) = punch.handle(delivery.from, &delivery.bytes, now);
                        if let Some(Event::Opened(_)) = event {
                            side.opened_at = Some(now);
                        }
                        side.events.extend(event);
                        for (to, bytes) in out {
                            self.net.send(side.local, to, bytes);
                        }
                    }
                }
            }
        }
    }

    /// RFC 4787's verdict for a direct path: possible unless a symmetric NAT faces a symmetric or a
    /// port-restricted one, and no router mapping helps.
    fn punchable(kinds: [NatKind; 2], forwards: [bool; 2]) -> bool {
        use NatKind::*;
        forwards[0] || forwards[1] || !matches!((kinds[0], kinds[1]), (Symmetric, Symmetric) | (Symmetric, PortRestricted) | (PortRestricted, Symmetric))
    }

    #[test]
    fn every_nat_pairing_opens_or_honestly_does_not() {
        let link = LinkConfig { latency_ms: 40, jitter_ms: 20, loss_per_mille: 100 };
        for (case, (a, b)) in NatKind::ALL.iter().flat_map(|a| NatKind::ALL.iter().map(move |b| (*a, *b))).enumerate() {
            for forwards in [[false, false], [true, false], [false, true]] {
                let mut world = world(case as u64 + 1, link, [a, b], forwards);
                // B types A's words 20 s after A typed B's.
                world.run([0, 20_000], 80_000);
                let expected = punchable([a, b], forwards);
                for (i, side) in world.sides.iter().enumerate() {
                    let what = format!("{a:?} x {b:?}, router mappings {forwards:?}, side {i}");
                    let want_mapping = if !forwards[i] && [a, b][i] == NatKind::Symmetric { Mapping::DestinationDependent } else { Mapping::EndpointIndependent };
                    assert_eq!(side.query.result().mapping(), want_mapping, "{what}: the reflector's mapping hint");
                    assert_eq!(side.opened_at.is_some(), expected, "{what}: opened {:?}, events {:?}", side.opened_at, side.events);
                    if let Some(at) = side.opened_at {
                        assert!(at >= 20_000 && at < 30_000, "{what}: opens soon after the second person starts, at {at}");
                        assert!(matches!(side.punch.as_ref().unwrap().state(), State::Open { .. }), "{what}: still open at the end");
                    } else {
                        assert!(matches!(side.punch.as_ref().unwrap().state(), State::Punching { .. }), "{what}: still punching, never a false open");
                    }
                }
            }
        }
    }

    #[test]
    fn nested_nat_matrix_uses_the_words_to_assess_and_open() {
        use Hint::{ConeUnknownFiltering as ConeHint, Symmetric as SymmetricHint};
        use NatKind::{PortRestricted as Cone, Symmetric};
        // Router, optional modem, router grant, expected discovery hint. A grant pins even a
        // symmetric inner router's port, but the modem still chooses the public port.
        let stacks = [
            (Cone, None, false, ConeHint),
            (Symmetric, None, false, SymmetricHint),
            (Cone, Some(Cone), false, ConeHint),
            (Cone, Some(Symmetric), false, SymmetricHint),
            (Symmetric, Some(Cone), false, SymmetricHint),
            (Symmetric, Some(Symmetric), false, SymmetricHint),
            (Symmetric, Some(Cone), true, ConeHint),
            (Cone, Some(Symmetric), true, SymmetricHint),
        ];
        let link = LinkConfig { latency_ms: 30, jitter_ms: 10, loss_per_mille: 50 };
        for (i, a) in stacks.into_iter().enumerate() {
            for (j, b) in stacks.into_iter().enumerate() {
                for later in [0, 20_000] {
                    let until = later + 30_000;
                    let case = format!("{a:?} facing {b:?}, second start {later} ms");
                    let mut world = world_with_modems(
                        (i * stacks.len() + j) as u64 + 1, link,
                        [NatConfig::new(a.0), NatConfig::new(b.0)], [a.1, b.1], [a.2, b.2],
                    );
                    world.drive([0, later], until, true);
                    for (index, (side, hint)) in world.sides.iter().zip([a.3, b.3]).enumerate() {
                        let invite = side.invite.as_ref().expect("both sides learned an endpoint");
                        assert_eq!(invite.nat, hint, "{case}: the words describe the composite mapping");
                        assert_eq!(*invite.addr.ip(), PUBLICS[index], "{case}: the words name the internet-facing NAT");
                        assert_eq!(Some(SocketAddr::V4(invite.addr)), side.query.result().mapped(), "{case}: the words carry the outer endpoint");
                        if let Some(inner) = side.router_grant {
                            assert_ne!(invite.addr.port(), inner.port(), "{case}: the modem remapped the router's grant");
                        }
                    }
                    let verdict = world.assessment().unwrap();
                    assert_eq!(verdict, discover::assess(a.3, b.3), "{case}: assess the decoded words");
                    if verdict == Assessment::NotPossible {
                        assert!(world.sides.iter().all(|s| s.punch.is_none()), "{case}: declined before either side probed");
                        assert_eq!(world.net.now(), world.sides[0].words_at.max(world.sides[1].words_at),
                            "{case}: declined as soon as both hints were known");
                        continue;
                    }
                    let opened = world.sides.each_ref().map(|side| side.opened_at);
                    assert_eq!(opened[0].is_some(), opened[1].is_some(), "{case}: both sides agree on the open");
                    if let [Some(at_a), Some(at_b)] = opened {
                        assert!([at_a, at_b].iter().all(|at| (later..=until).contains(at)), "{case}: opened at {opened:?}");
                        assert_open_path_carries_data(&mut world, until, &case);
                    } else {
                        assert_eq!(verdict, Assessment::StillTrying, "{case}: a likely pairing must open within 30 s of the later start");
                        assert_eq!(world.net.now(), until, "{case}: reached the caller's deadline");
                        for side in &world.sides {
                            let punch = side.punch.as_ref().unwrap();
                            assert!(matches!(punch.state(), State::Punching { .. }), "{case}: no false open");
                            assert!((until + 1..=until + punch.config.slow_interval_ms).contains(&punch.next_wake()),
                                "{case}: another retry is scheduled within the probe interval");
                        }
                    }
                }
            }
        }
    }

    fn assert_open_path_carries_data(world: &mut World, until: u64, case: &str) {
        let peers = world.sides.each_ref().map(|side| side.punch.as_ref().unwrap().peer().expect("still open"));
        let mut received = [false; 2];
        let mut retry = reflect::FIRST_RETRY_MS;
        while received != [true; 2] && world.net.now() < until {
            for (i, side) in world.sides.iter().enumerate() {
                if !received[1 - i] {
                    world.net.send(side.local, peers[i], vec![i as u8]);
                }
            }
            let deliveries = world.net.advance_to((world.net.now() + retry).min(until));
            for (i, side) in world.sides.iter().enumerate() {
                received[i] |= deliveries.iter().any(|d| d.to == side.local && d.from == peers[i] && d.bytes == [(1 - i) as u8]);
            }
            retry = (retry * 2).min(reflect::MAX_RETRY_MS);
        }
        assert_eq!(received, [true; 2], "{case}: each side receives application data from its open peer by the deadline");
    }

    #[test]
    fn keepalives_hold_the_path_and_a_rebound_peer_is_followed() {
        let link = LinkConfig { latency_ms: 30, jitter_ms: 5, loss_per_mille: 0 };
        // A's NAT lets B's address in on any port (so a rebound B can reach it); B's NAT hands out a
        // new port for every new mapping.
        let mut config_b = NatConfig::new(NatKind::FullCone);
        config_b.ports = super::super::natsim::PortAlloc::Sequential;
        let mut world = world_with(42, link, [NatConfig::new(NatKind::Restricted), config_b], [false, false]);
        world.run([0, 0], 10 * 60_000);
        for (i, side) in world.sides.iter().enumerate() {
            assert_eq!(side.events.len(), 1, "side {i}: opened once and never went quiet over 10 minutes: {:?}", side.events);
            assert!(matches!(side.events[0], Event::Opened(_)), "side {i}: {:?}", side.events);
        }
        // Side B's NAT reboots: its next packets leave from a new port.
        world.net.flush_mappings(world.sides[1].nat);
        let before = world.sides[0].punch.as_ref().unwrap().peer().unwrap();
        let flushed_at = world.net.now();
        world.run([0, 0], flushed_at + 3 * 60_000);
        let after = world.sides[0].punch.as_ref().unwrap().peer();
        assert!(after.is_some() && after != Some(before), "side A follows B to its new address: {before} -> {after:?}, events {:?}", world.sides[0].events);
        assert!(
            world.sides[0].events.iter().any(|e| matches!(e, Event::Reopened { moved: true, .. })),
            "side A reports the move: {:?}",
            world.sides[0].events
        );
        assert!(
            !world.sides[0].events.iter().any(|e| matches!(e, Event::Quiet(_))),
            "followed at B's next keepalive, before the path went quiet: {:?}",
            world.sides[0].events
        );
    }

    #[test]
    fn stray_and_malformed_datagrams_change_nothing() {
        let peer: SocketAddr = "198.51.100.23:51820".parse().unwrap();
        let mut punch = Punch::new(Config::new([1; 8]), peer, [9; 8], 0);
        let (sent, _) = punch.poll(0);
        assert_eq!(sent.len(), 1, "a probe to the words' address at once");
        assert!(punch.poll(50).0.is_empty(), "no probe before the burst interval");
        assert_eq!(punch.poll(100).0.len(), 1, "the burst's next probe");
        let other_tag = datagram(ACK, &[2; 8], &[9; 8]);
        let wrong_nonce = datagram(ACK, &[1; 8], &[8; 8]);
        for (what, packet) in [("another pairing's ack", other_tag), ("an ack of a nonce not ours", wrong_nonce), ("a short packet", vec![KIND, ACK, 1]), ("an unknown type", datagram(7, &[1; 8], &[9; 8]))] {
            assert_eq!(punch.handle(peer, &packet, 120), (Vec::new(), None), "{what}");
            assert!(matches!(punch.state(), State::Punching { .. }), "{what}: still punching");
        }
        let elsewhere: SocketAddr = "192.0.2.50:40000".parse().unwrap();
        let (out, event) = punch.handle(elsewhere, &datagram(PROBE, &[1; 8], &[3; 8]), 130);
        assert_eq!(
            out,
            vec![(elsewhere, datagram(ACK, &[1; 8], &[3; 8])), (elsewhere, datagram(PROBE, &[1; 8], &[9; 8]))],
            "a probe is answered where it came from, echoing its nonce, and probed back"
        );
        assert_eq!(event, None, "a probe alone doesn't open");
        assert_eq!(punch.targets(), &[peer, elsewhere], "the probe's source is learned (peer-reflexive)");
        let (_, event) = punch.handle(elsewhere, &datagram(ACK, &[1; 8], &[9; 8]), 140);
        assert_eq!(event, Some(Event::Opened(elsewhere)), "our ack opens the path to where it came from");
        let mut t = 140;
        for _ in 0..100 {
            t = punch.next_wake().max(t + 1);
            punch.poll(t);
        }
        assert!(matches!(punch.state(), State::Quiet { .. }), "a silent peer makes the path quiet, never closed");
        let (sent, _) = punch.poll(punch.next_wake());
        assert_eq!(sent.len(), 2, "quiet: probing every known address again");
    }
}
