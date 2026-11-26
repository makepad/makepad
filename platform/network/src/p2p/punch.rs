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
//! - **Open** when the peer acknowledges one of our probes: both directions work. With a preferred
//!   address, an ack from another address waits up to [`Config::prefer_ms`] from first contact.
//! - **Keepalives** after that keep the NATs' mappings alive; traffic of the layers above counts
//!   ([`Punch::note_sent`], [`Punch::note_heard`]). A peer silent for [`Config::quiet_ms`] makes the
//!   path quiet, never closed: probing resumes, and the path reopens, possibly from a new address
//!   when the peer's NAT rebound it. With a preferred address the open path stays put until quiet.
//!   Otherwise a new source is probed straight back to follow a rebound within one round trip. Such a
//!   move is only a candidate: the channel on top must see authenticated traffic from the new
//!   address before trusting it, since anyone who knows the tag can answer a probe.
//!
//! - **Several tags, and a quiet pace.** A caller whose tag rotates (a reconnect, which must not
//!   hand an observer one handle to follow across networks) sends one tag and accepts a short list
//!   ([`Config::accepting`], [`Punch::set_tags`]): an ack echoes the tag of the probe it answers, so
//!   each side's own rotation never matters to the other. A caller that probes for as long as a
//!   device is wanted can add a third pace ([`Config::idle`]): after `idle_after_ms` without any
//!   sign of the peer, one probe every `idle_interval_ms`, which also keeps the NAT mapping alive.
//!   A tagged datagram from the peer returns the pace to the fast one. Without it nothing changes.
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

/// How many further tags a [`Config`] accepts.
pub const MAX_ACCEPTED: usize = 4;

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// This pairing's tag, the one the probes carry.
    pub tag: [u8; 8],
    /// Tags besides `tag` that a datagram may carry (the first `accepted_len`).
    pub accepted: [[u8; 8]; MAX_ACCEPTED],
    pub accepted_len: usize,
    /// Probes this often for the first `burst_ms` of punching (and of a quiet path).
    pub burst_interval_ms: u64,
    pub burst_ms: u64,
    /// Then this often, for as long as the caller lets it run.
    pub slow_interval_ms: u64,
    /// How long after first hearing the peer to wait for a preferred address to acknowledge.
    pub prefer_ms: u64,
    /// An open path with nothing sent for this long gets a keepalive probe.
    pub keepalive_ms: u64,
    /// An open path with nothing heard for this long is quiet.
    pub quiet_ms: u64,
    /// Punching for this long (since the burst began, or since a sign of the peer) moves to
    /// `idle_interval_ms`. `u64::MAX` never does.
    pub idle_after_ms: u64,
    pub idle_interval_ms: u64,
}

impl Config {
    pub fn new(tag: [u8; 8]) -> Config {
        Config {
            tag, accepted: [[0; 8]; MAX_ACCEPTED], accepted_len: 0,
            burst_interval_ms: 100, burst_ms: 2_000, slow_interval_ms: 1_000, prefer_ms: 1_500, keepalive_ms: 15_000, quiet_ms: 45_000,
            idle_after_ms: u64::MAX, idle_interval_ms: 1_000,
        }
    }

    /// Also accepts datagrams carrying any of `tags` (the first [`MAX_ACCEPTED`]).
    pub fn accepting(mut self, tags: &[[u8; 8]]) -> Config {
        self.set_accepted(tags);
        self
    }

    /// After `after_ms` of punching with no sign of the peer, probes only every `interval_ms`.
    pub fn idle(mut self, after_ms: u64, interval_ms: u64) -> Config {
        (self.idle_after_ms, self.idle_interval_ms) = (after_ms, interval_ms);
        self
    }

    fn set_accepted(&mut self, tags: &[[u8; 8]]) {
        self.accepted_len = tags.len().min(MAX_ACCEPTED);
        self.accepted[..self.accepted_len].copy_from_slice(&tags[..self.accepted_len]);
    }

    /// Whether a datagram carrying `tag` belongs to this pairing.
    pub fn accepts(&self, tag: &[u8]) -> bool {
        tag == self.tag || self.accepted[..self.accepted_len].iter().any(|t| t == tag)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Probing since `since`, still choosing a path.
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
    preferred: Option<SocketAddr>,
    held: Option<SocketAddr>,
    /// Arrival of the first tagged datagram in this round of path selection.
    first_heard: Option<u64>,
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
            preferred: None,
            held: None,
            first_heard: None,
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

    /// Every target, including addresses learned from the peer.
    pub fn targets(&self) -> &[SocketAddr] {
        &self.targets
    }

    fn add_target(&mut self, addr: SocketAddr) {
        if !self.targets.contains(&addr) {
            self.targets.push(addr);
        }
    }

    /// Probes `addr` alongside the other targets and gives it priority when choosing a path.
    pub fn prefer(&mut self, addr: SocketAddr) {
        self.add_target(addr);
        self.preferred = Some(addr);
    }

    /// Opens the path the layer above saw the peer choose.
    pub fn select(&mut self, addr: SocketAddr, now: u64) -> Option<Event> {
        let event = match self.state {
            State::Punching { .. } => Some(Event::Opened(addr)),
            State::Quiet { peer, .. } => Some(Event::Reopened { peer: addr, moved: peer != addr }),
            State::Open { peer, .. } if peer != addr => Some(Event::Reopened { peer: addr, moved: true }),
            State::Open { .. } => None,
        };
        self.add_target(addr);
        self.state = State::Open { peer: addr, since: now };
        self.last_heard = now;
        self.held = None;
        event
    }

    fn held_candidate(&self) -> Option<(SocketAddr, u64)> {
        Some((self.held?, self.first_heard?.saturating_add(self.config.prefer_ms)))
    }

    fn probe_interval(&self, now: u64) -> u64 {
        let since = now.saturating_sub(self.burst_from);
        if since < self.config.burst_ms {
            self.config.burst_interval_ms
        } else if since < self.config.idle_after_ms {
            self.config.slow_interval_ms
        } else {
            self.config.idle_interval_ms
        }
    }

    /// Replaces the tag the probes carry and the others accepted (a rotation).
    pub fn set_tags(&mut self, tag: [u8; 8], accepted: &[[u8; 8]]) {
        self.config.tag = tag;
        self.config.set_accepted(accepted);
    }

    /// Whether a datagram carrying `tag` belongs to this punch.
    pub fn accepts(&self, tag: &[u8]) -> bool {
        self.config.accepts(tag)
    }

    /// The datagrams due at `now`, as (to, bytes), and any transition due.
    pub fn poll(&mut self, now: u64) -> (Vec<(SocketAddr, Vec<u8>)>, Option<Event>) {
        let mut event = None;
        if let State::Open { peer, .. } = self.state {
            if now.saturating_sub(self.last_heard) >= self.config.quiet_ms {
                self.state = State::Quiet { peer, since: self.last_heard };
                self.held = None;
                self.first_heard = None;
                self.burst_from = now;
                self.next_probe_at = now;
                event = Some(Event::Quiet(peer));
            }
        } else if let Some((addr, deadline)) = self.held_candidate() {
            if now >= deadline {
                event = self.select(addr, now);
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
            State::Punching { .. } | State::Quiet { .. } => self.held_candidate().map_or(self.next_probe_at, |(_, deadline)| self.next_probe_at.min(deadline)),
        }
    }

    /// Answers a probe at its source and considers a path open only after an ack of our nonce.
    pub fn handle(&mut self, from: SocketAddr, packet: &[u8], now: u64) -> (Vec<(SocketAddr, Vec<u8>)>, Option<Event>) {
        if !is_punch(packet) || !self.config.accepts(&packet[2..10]) || !matches!(packet[1], PROBE | ACK) {
            return (Vec::new(), None);
        }
        // A peer that returns after a long silence gets the fast pace back at once.
        if !matches!(self.state, State::Open { .. }) && now.saturating_sub(self.burst_from) >= self.config.idle_after_ms {
            self.burst_from = now;
            self.next_probe_at = now;
        }
        self.first_heard.get_or_insert(now);
        let tag: [u8; 8] = packet[2..10].try_into().unwrap();
        let mut nonce = [0u8; 8];
        nonce.copy_from_slice(&packet[10..18]);
        match packet[1] {
            PROBE => {
                self.add_target(from);
                let mut out = vec![(from, datagram(ACK, &tag, &nonce))];
                match self.peer() {
                    Some(peer) if peer == from => {
                        self.last_heard = now;
                        self.last_sent = now;
                    }
                    // Answer other paths without restarting their probes or delaying our keepalive.
                    Some(_) if self.preferred.is_some() => {}
                    _ => {
                        out.push((from, datagram(PROBE, &self.config.tag, &self.nonce)));
                        self.last_sent = now;
                    }
                }
                (out, None)
            }
            ACK if nonce == self.nonce => {
                let event = match self.peer() {
                    Some(peer) if peer == from => {
                        self.last_heard = now;
                        None
                    }
                    Some(_) if self.preferred.is_some() => None,
                    None if self.preferred.is_some_and(|addr| addr != from) => {
                        self.held.get_or_insert(from);
                        None
                    }
                    _ => self.select(from, now),
                };
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
                let invite = Invite { addr, nat: choice.hint, commit: TEST_COMMITMENT, lan: None };
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
    fn preferred_private_path_stays_open_when_the_public_mapping_also_answers() {
        let link = LinkConfig { latency_ms: 10, jitter_ms: 0, loss_per_mille: 0 };
        let mut net = Net::new(1, link);
        let mut router = NatConfig::new(NatKind::FullCone);
        router.hairpin = true;
        let nat = net.add_nat(PUBLICS[0], router);
        let private = [2, 3].map(|host| {
            let ip = Ipv4Addr::new(10, 77, 0, host);
            net.add_host(ip, Some(nat));
            SocketAddr::new(ip.into(), 51820)
        });
        let public = [0, 1].map(|i| {
            let port = 41000 + i as u16;
            net.forward(nat, port, private[i], true);
            SocketAddr::new(PUBLICS[0].into(), port)
        });
        let mut punches = [0, 1].map(|i| {
            let mut punch = Punch::new(Config::new([0x5a; 8]), public[1 - i], [i as u8 + 1; 8], 0);
            punch.prefer(private[1 - i]);
            punch
        });

        let mut events = [Vec::new(), Vec::new()];
        let mut public_ack = [false; 2];
        let mut keepalive_ack = [false; 2];
        let first_ack_at = 2 * link.latency_ms;
        let until = first_ack_at + 30_000;
        loop {
            let now = net.now();
            for (i, punch) in punches.iter_mut().enumerate() {
                let (out, event) = punch.poll(now);
                events[i].extend(event);
                if now == 0 {
                    for addr in [public[1 - i], private[1 - i]] {
                        assert!(out.iter().any(|(to, bytes)| *to == addr && bytes[1] == PROBE), "side {i} probes {addr} on its first poll");
                    }
                }
                for (to, bytes) in out {
                    net.send(private[i], to, bytes);
                }
            }
            if now == until {
                break;
            }
            let next = punches.iter().map(Punch::next_wake).chain(net.next_at()).min().unwrap().min(until);
            assert!(next > now, "handling the deliveries and polling both punches advances virtual time");
            for delivery in net.advance_to(next) {
                let i = private.iter().position(|addr| *addr == delivery.to).expect("packet reaches one of the two hosts");
                if delivery.bytes[1] == ACK {
                    public_ack[i] |= delivery.from == public[1 - i];
                    keepalive_ack[i] |= delivery.from == private[1 - i] && next >= punches[i].config.keepalive_ms;
                }
                let (out, event) = punches[i].handle(delivery.from, &delivery.bytes, next);
                events[i].extend(event);
                for (to, bytes) in out {
                    net.send(private[i], to, bytes);
                }
            }
        }
        for (i, punch) in punches.iter().enumerate() {
            let peer = private[1 - i];
            assert!(public_ack[i], "side {i} also receives an ack through the public mapping");
            assert_eq!(events[i], [Event::Opened(peer)], "side {i} opens only once, on its preferred address");
            assert_eq!(punch.state(), State::Open { peer, since: first_ack_at }, "side {i} stays open for 30 s from the first private ack");
            assert!(keepalive_ack[i], "side {i} receives a keepalive ack on its private path");
        }
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
    #[test]
    fn a_rotating_tag_is_sent_one_way_and_accepted_in_a_list() {
        let peer: SocketAddr = "198.51.100.23:51820".parse().unwrap();
        let (now_tag, old_tag, next_tag, far_tag) = ([2; 8], [1; 8], [3; 8], [4; 8]);
        let mut punch = Punch::new(Config::new(now_tag).accepting(&[old_tag, next_tag]), peer, [9; 8], 0);
        assert_eq!(punch.poll(0).0, vec![(peer, datagram(PROBE, &now_tag, &[9; 8]))], "probes carry only the current tag");
        for (tag, accepted) in [(now_tag, true), (old_tag, true), (next_tag, true), (far_tag, false)] {
            let (out, _) = punch.handle(peer, &datagram(PROBE, &tag, &[5; 8]), 10);
            assert_eq!(!out.is_empty(), accepted, "a probe with tag {tag:?}: accepted is {accepted}");
            if accepted {
                assert_eq!(out[0], (peer, datagram(ACK, &tag, &[5; 8])), "the ack echoes the probe's own tag, whatever hour its sender is in");
            }
        }
        // An ack of our nonce under a neighbouring hour's tag opens, one under a far tag doesn't.
        assert_eq!(punch.handle(peer, &datagram(ACK, &far_tag, &[9; 8]), 20).1, None, "an ack under an unknown tag is ignored");
        assert_eq!(punch.handle(peer, &datagram(ACK, &old_tag, &[9; 8]), 30).1, Some(Event::Opened(peer)), "an ack under an accepted tag opens");
        punch.set_tags(next_tag, &[now_tag]);
        assert!(punch.accepts(&now_tag) && punch.accepts(&next_tag) && !punch.accepts(&old_tag), "a rotation replaces both the sent and the accepted tags");
    }

    #[test]
    fn the_idle_pace_follows_the_burst_and_a_sign_of_the_peer_ends_it() {
        let peer: SocketAddr = "198.51.100.23:51820".parse().unwrap();
        let mut punch = Punch::new(Config::new([1; 8]).idle(10_000, 17_000), peer, [9; 8], 0);
        let mut times = Vec::new();
        let mut t = 0;
        while t < 60_000 {
            if !punch.poll(t).0.is_empty() {
                times.push(t);
            }
            t = punch.next_wake().max(t + 1);
        }
        assert!(times.iter().filter(|t| **t < 2_000).count() >= 19, "the burst runs first: {times:?}");
        let slow: Vec<u64> = times.iter().copied().filter(|t| (2_000..10_000).contains(t)).collect();
        assert!(slow.windows(2).all(|w| w[1] - w[0] == 1_000), "then the slow pace until the idle time: {slow:?}");
        let idle: Vec<u64> = times.iter().copied().filter(|t| *t >= 10_000).collect();
        assert!(idle.len() >= 3 && idle.windows(2).all(|w| w[1] - w[0] == 17_000), "then one probe every 17 s: {idle:?}");
        // A tagged probe from the peer returns to the fast pace; one under another tag does not.
        let last = *times.last().unwrap();
        let (out, _) = punch.handle(peer, &datagram(PROBE, &[2; 8], &[7; 8]), last + 1_000);
        assert!(out.is_empty() && punch.next_wake() == last + 17_000, "an unknown tag changes nothing");
        punch.handle(peer, &datagram(PROBE, &[1; 8], &[7; 8]), last + 1_000);
        assert_eq!(punch.next_wake(), last + 1_000, "a sign of the peer brings the next probe at once");
        assert_eq!(punch.poll(last + 1_000).0.len(), 1, "that probe goes out");
        assert_eq!(punch.next_wake(), last + 1_100, "and the fast pace is back");
    }
}
