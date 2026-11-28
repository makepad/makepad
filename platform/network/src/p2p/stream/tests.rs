use super::*;
use crate::p2p::{
    natsim::{LinkConfig, NatConfig, NatId, NatKind, Net, PortAlloc},
    punch,
};
use std::net::Ipv4Addr;
use wire::SEGMENT;

const ID: [u8; 8] = *b"stream01";
const KEYS: [[u8; 32]; 2] = [[31; 32], [79; 32]];
const RELIABLE: LinkConfig = LinkConfig { latency_ms: 5, jitter_ms: 0, loss_per_mille: 0 };

struct Pair {
    net: Net,
    streams: [Stream; 2],
    local: [SocketAddr; 2],
    nat: [NatId; 2],
    duplicate: bool,
    duplicates: usize,
    reordered: usize,
    greatest_data: [u64; 2],
    sent_pn: [u64; 2],
}

impl Pair {
    fn new(seed: u64) -> Self {
        let mut net = Net::new(seed, RELIABLE);
        let reflector = Ipv4Addr::new(203, 0, 113, 9);
        net.add_host(reflector, None);
        let nat = [1, 2].map(|i| {
            let mut config = NatConfig::new(NatKind::FullCone);
            config.ports = PortAlloc::Sequential;
            net.add_nat(Ipv4Addr::new(198, 51, 100, i), config)
        });
        let local = [0, 1].map(|i| {
            let ip = Ipv4Addr::new(192, 0, 2, 1 + i as u8);
            net.add_host(ip, Some(nat[i]));
            SocketAddr::new(ip.into(), 5151)
        });
        // Discover both mappings on the same sockets the streams will use.
        let public = local.map(|addr| {
            net.send(addr, SocketAddr::new(reflector.into(), 3478), Vec::new());
            net.advance_to(net.next_at().unwrap()).pop().expect("reflection reaches the public host").from
        });
        let streams =
            [Stream::new(Role::Client, ID, public[1], net.now()), Stream::new(Role::Server, ID, public[0], net.now())];
        Self {
            net,
            streams,
            local,
            nat,
            duplicate: false,
            duplicates: 0,
            reordered: 0,
            greatest_data: [0; 2],
            sent_pn: [0; 2],
        }
    }

    fn step(&mut self) {
        self.step_with(|_, _, _| true);
    }

    /// Drain both endpoints, apply the test's wire fault, then deliver at exactly
    /// the first network or protocol deadline. A stale timer would busy-loop a worker.
    fn step_with(&mut self, mut pass: impl FnMut(usize, SocketAddr, &[u8]) -> bool) {
        let now = self.net.now();
        for (i, side) in self.streams.iter_mut().enumerate() {
            while let Some((to, bytes)) = side.poll_transmit(now) {
                assert!(
                    bytes.len() <= 1200 && bytes[0] > 3 && !punch::is_punch(&bytes),
                    "stream MTU and shared socket discriminator"
                );
                let pn = wire::number(&bytes[10..16]);
                assert!(pn > self.sent_pn[i], "every send, including retry, has a fresh packet number");
                self.sent_pn[i] = pn;
                if !pass(i, to, &bytes) {
                    continue;
                }
                if self.duplicate && pn % 13 == 0 {
                    self.duplicates += 1;
                    self.net.send(self.local[i], to, bytes.clone());
                }
                self.net.send(self.local[i], to, bytes);
            }
        }
        let wake = self.streams.iter().filter_map(Stream::next_timeout).chain(self.net.next_at()).min();
        let Some(at) = wake else { return };
        assert!(at > now, "draining transmit must clear all immediately due timers at {now}");
        for d in self.net.advance_to(at) {
            let i = self.local.iter().position(|&addr| addr == d.to).expect("delivery to a stream");
            if kind(&d.bytes) == DATA {
                let pn = wire::number(&d.bytes[10..16]);
                self.reordered += usize::from(pn < self.greatest_data[i]);
                self.greatest_data[i] = self.greatest_data[i].max(pn);
            }
            self.streams[i].handle_datagram(at, d.from, &d.bytes);
        }
    }

    fn until(&mut self, done: impl Fn(&Self) -> bool) {
        let deadline = self.net.now() + 2 * IDLE;
        while !done(self) {
            assert!(
                self.net.now() < deadline,
                "the expected transition is reachable, states {:?}",
                self.streams.each_ref().map(Stream::state)
            );
            self.step();
        }
    }

    fn open(&mut self) {
        self.until(|w| w.streams.iter().all(|s| s.state() == State::Open));
    }

    fn keys(&mut self) {
        for (i, side) in self.streams.iter_mut().enumerate() {
            assert!(side.set_keys(KEYS[i], KEYS[1 - i]), "install directional keys after open");
        }
    }

    fn transfer(&mut self, bytes: &[u8], mut sent: usize, received: &mut Vec<u8>) {
        let deadline = self.net.now() + 10 * 60_000;
        while received.len() < bytes.len() {
            assert!(
                self.net.now() < deadline && self.streams[0].state() == State::Open,
                "the writer makes progress under loss"
            );
            sent += self.streams[0].write(&bytes[sent..]);
            self.step();
            drain(&mut self.streams[1], received);
        }
        assert_eq!(sent, bytes.len(), "completed transfer accepted the entire source");
    }
}

fn drain(side: &mut Stream, bytes: &mut Vec<u8>) {
    let mut buf = [0; 4096];
    loop {
        let n = side.read(&mut buf);
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
    }
}

fn content(n: usize) -> Vec<u8> {
    let mut state = 77u32;
    (0..n)
        .map(|_| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 24) as u8
        })
        .collect()
}

fn kind(bytes: &[u8]) -> u8 {
    bytes[1] & 0x7f
}

#[test]
fn ordered_megabyte_under_loss_reordering_and_duplication() {
    let data = content(1_000_000);
    for (seed, loss) in [(11, 30), (73, 70), (197, 100)] {
        let mut w = Pair::new(seed);
        w.net.set_link(LinkConfig { latency_ms: 3, jitter_ms: 11, loss_per_mille: loss });
        w.duplicate = true;
        w.open();
        w.keys();
        let mut received = Vec::new();
        let initial = w.streams[0].write(&data);
        assert_eq!(initial, 10 * SEGMENT, "seed {seed}: exactly the initial window fits");
        assert_eq!(w.streams[0].write(b"more"), 0, "seed {seed}: no extra buffering above cwnd");
        w.transfer(&data, initial, &mut received);
        assert!(received == data, "seed {seed}: all bytes delivered once in order");
        assert!(
            w.net.stats().lost > 0 && w.reordered > 0 && w.duplicates > 0,
            "seed {seed}: all three link faults exercised"
        );
    }
}

#[test]
fn stalled_reader_and_lost_window_update() {
    let mut w = Pair::new(23);
    w.open();
    let data = content(usize::from(SLOTS) * SEGMENT * 3);
    let mut sent = 0;
    let mut probed = false;
    while !probed {
        assert!(w.net.now() < IDLE, "stalled reader reaches zero credit");
        sent += w.streams[0].write(&data[sent..]);
        w.step_with(|i, _, bytes| {
            probed |= i == 0 && kind(bytes) == PING;
            true
        });
    }
    assert_eq!(w.streams[1].rx.credit(), 0, "stopped reads exhaust the receive window");
    sent += w.streams[0].write(&data[sent..]);
    assert!(sent < data.len() && w.streams[0].write(&data[sent..]) == 0, "backpressure reaches the application writer");
    assert!(w.streams[0].tx.blocked_on_credit(), "the sender has queued data behind the zero window");
    assert!(w.streams[0].set_keys(KEYS[0], KEYS[1]), "the sender installs keys while blocked");
    let mut received = Vec::new();
    drain(&mut w.streams[1], &mut received);
    let mut dropped_update = false;
    let mut probed_again = false;
    while !probed_again {
        assert!(w.net.now() < KEEPALIVE, "zero-window timer fires before the idle keepalive");
        w.step_with(|i, _, bytes| {
            probed_again |= i == 0 && kind(bytes) == PING;
            if i == 1 && kind(bytes) == ACK && !probed_again {
                dropped_update = true;
                return false;
            }
            true
        });
    }
    assert!(dropped_update, "the first reopened window announcement was lost");
    let mut unsigned_update = false;
    while w.streams[1].rx.ready.is_empty() {
        assert!(w.net.now() < IDLE, "an unsigned window update releases the queued data before key exchange times out");
        w.step_with(|i, _, bytes| {
            if i == 1 && bytes[1] == ACK && u16::from_be_bytes(bytes[24..26].try_into().unwrap()) > 0 {
                unsigned_update = true;
            }
            true
        });
    }
    assert!(
        unsigned_update && !w.streams[0].gap_closed() && w.streams[0].verified_replay.high == 0,
        "the peer reopened the window before sending any signed packet"
    );
    assert!(w.streams[1].set_keys(KEYS[1], KEYS[0]), "the reader installs keys after receiving the queued data");
    w.transfer(&data, sent, &mut received);
    assert!(received == data, "the persist probe restores flow without a gap");
    assert!(w.streams.iter().all(Stream::gap_closed), "the transfer ends with both unsigned gaps closed");
}

#[test]
fn rebound_nat_waits_for_the_challenge_echo() {
    let mut w = Pair::new(33);
    w.open();
    w.keys();
    let data = content(37 * SEGMENT);
    let mut received = Vec::new();
    let mut sent = w.streams[0].write(&data);
    w.until(|w| !w.streams[1].rx.ready.is_empty());
    drain(&mut w.streams[1], &mut received);
    assert!(received.len() < data.len(), "transfer still active at rebinding");
    let old = w.streams[1].peer();
    w.net.flush_mappings(w.nat[0]);
    while w.streams[1].candidate.is_none() {
        assert!(w.net.now() < IDLE, "a packet from the new mapping arrives");
        sent += w.streams[0].write(&data[sent..]);
        w.step();
    }
    let new = w.streams[1].candidate.as_ref().unwrap().addr;
    assert_ne!(new, old, "flushing the NAT allocated a different port");
    assert_eq!(w.streams[1].write(b"back"), 4, "reverse traffic queued during validation");
    let mut challenges = 0;
    let mut echoes = 0;
    let mut saw_old_data = false;
    while w.streams[1].peer() == old {
        assert!(w.net.now() < IDLE, "lost challenge echo is retried");
        w.step_with(|i, to, bytes| {
            match (i, kind(bytes)) {
                (1, CHALLENGE) => {
                    assert_eq!(to, new, "challenge goes to candidate");
                    challenges += 1;
                }
                (1, DATA) => {
                    assert_eq!(to, old, "data still uses validated address while echo is outstanding");
                    saw_old_data = true;
                }
                (0, RESPONSE) => {
                    echoes += 1;
                    return echoes > 1;
                }
                _ => (),
            }
            true
        });
    }
    assert!(saw_old_data && challenges >= 2 && echoes >= 2, "only a delivered echo moves the path");
    assert_eq!(w.streams[1].peer(), new, "peer query reports validated rebind");
    w.transfer(&data, sent, &mut received);
    assert!(received == data, "retransmissions bridge the address change");
    w.until(|w| !w.streams[0].rx.ready.is_empty());
    let mut reverse = Vec::new();
    drain(&mut w.streams[0], &mut reverse);
    assert_eq!(reverse, b"back", "reverse traffic is retried on the new path");
}

#[test]
fn both_peers_move_and_unreachable_peers_time_out() {
    let mut w = Pair::new(44);
    w.open();
    w.keys();
    let old = w.streams.each_ref().map(Stream::peer);
    // Both outbound mappings change together. One old ingress survives the
    // handover long enough for an authenticated packet to reveal a new source.
    for nat in w.nat {
        w.net.flush_mappings(nat);
    }
    w.net.forward(w.nat[0], old[1].port(), w.local[0], false);
    for side in &mut w.streams {
        assert_eq!(side.write(b"moving"), 6, "queued data across move");
    }
    w.until(|w| (0..2).all(|i| w.streams[i].peer() != old[i]));
    w.net.unforward(w.nat[0], old[1].port());
    w.until(|w| w.streams.iter().all(|s| !s.rx.ready.is_empty()));
    for side in &mut w.streams {
        let mut got = Vec::new();
        drain(side, &mut got);
        assert_eq!(got, b"moving", "both validated addresses carry the outstanding data");
    }
    // With neither old ingress left there is no way to learn either new address.
    for nat in w.nat {
        w.net.flush_mappings(nat);
    }
    w.until(|w| w.streams.iter().all(|s| s.state() == State::Closed(CloseReason::TimedOut)));
    assert!(w.streams.iter().all(|s| s.next_timeout().is_none()), "dead peers release their timers");
}

#[test]
fn captured_datagram_from_another_address_cannot_move_the_peer() {
    let mut w = Pair::new(55);
    w.open();
    w.keys();
    let old = w.streams[1].peer();
    let mut captured = Vec::new();
    w.streams[0].write(b"original");
    w.step_with(|i, _, bytes| {
        if i == 0 && kind(bytes) == DATA {
            captured = bytes.to_vec();
        }
        true
    });
    let mut got = Vec::new();
    drain(&mut w.streams[1], &mut got);
    assert_eq!(got, b"original", "captured original was accepted on the validated path");
    let other = Ipv4Addr::new(203, 0, 113, 73);
    w.net.add_host(other, None);
    w.net.send(SocketAddr::new(other.into(), 4111), w.streams[0].peer(), captured);
    w.step();
    assert!(
        w.streams[1].candidate.is_none() && w.streams[1].peer() == old,
        "replay does not even start path validation"
    );
    let mut extra = [0; 8];
    assert_eq!(w.streams[1].read(&mut extra), 0, "replay never duplicates stream bytes");
}

#[test]
fn key_cutover_and_forged_segments() {
    let mut w = Pair::new(66);
    let peer = w.streams[1].peer();
    let addr = w.streams[0].peer();
    let stray: SocketAddr = "203.0.113.6:5151".parse().unwrap();
    let forge = |kind, number, body: &[u8], id: &[u8; 8], key| {
        Packet { kind, number, seq: 0, ack: 0, credit: SLOTS, echo: 0, body }.encode(id, key)
    };
    let server = &mut w.streams[1];
    server.handle_datagram(w.net.now(), stray, &forge(OPEN, 1, &[], &ID, None));
    server.handle_datagram(w.net.now(), peer, &forge(OPEN, 1, &[], b"other-id", None));
    assert!(
        server.poll_transmit(w.net.now()).is_none() && server.state() == State::Opening,
        "server answers only its expected pairing"
    );
    w.open();
    for k in [RESET, FIN] {
        w.streams[1].handle_datagram(w.net.now(), peer, &forge(k, MAX_PN, &[], &ID, None));
    }
    assert_eq!(w.streams[1].state(), State::Open, "unkeyed resets and closes are silent");
    w.streams[0].write(b"key record");
    w.step_with(|i, _, bytes| i != 0 || kind(bytes) != DATA);
    // Sender installed while its in-band key record was in flight and lost.
    assert!(w.streams[0].set_keys(KEYS[0], KEYS[1]), "first side installs keys");
    w.until(|w| !w.streams[1].rx.ready.is_empty());
    let mut got = Vec::new();
    drain(&mut w.streams[1], &mut got);
    assert_eq!(got, b"key record", "tagged retransmission can complete the in-band key exchange");
    // A speculative segment beyond the TLS record has never been ACKed.
    let ahead = Packet { kind: DATA, number: MAX_PN, seq: 2, ack: 0, credit: SLOTS, echo: 0, body: b"before auth" };
    w.streams[1].handle_datagram(w.net.now(), peer, &ahead.encode(&ID, None));
    assert!(w.streams[1].set_keys(KEYS[1], KEYS[0]), "second side installs keys");
    assert_eq!(addr, w.streams[0].peer(), "key installation preserves the path");
    for forged in [forge(DATA, MAX_PN, b"stray", &ID, None), forge(RESET, MAX_PN, &[], &ID, Some(&[0; 32]))] {
        w.streams[1].handle_datagram(w.net.now(), peer, &forged);
    }
    // The lost packet reduced cwnd to one. Wait for the key record's ACK
    // before the application can enqueue its next write.
    while w.streams[0].write(b"next record") == 0 {
        assert!(w.net.now() < IDLE, "authenticated peer acknowledges the key record");
        w.step();
    }
    let mut tampered = false;
    let now = w.net.now();
    // Deliver the modified copy before the valid one, using the same fresh PN.
    while let Some((_, bytes)) = w.streams[0].poll_transmit(now) {
        if kind(&bytes) == DATA {
            let mut bad = bytes.clone();
            let last_payload = bad.len() - 9;
            bad[last_payload] ^= 1;
            w.streams[1].handle_datagram(now, peer, &bad);
            assert_eq!(w.streams[1].read(&mut [0; 16]), 0, "a bad tag releases no bytes");
            tampered = true;
        }
        w.net.send(w.local[0], addr, bytes);
    }
    assert!(tampered, "a real tagged data packet was modified");
    w.until(|w| !w.streams[1].rx.ready.is_empty());
    got.clear();
    drain(&mut w.streams[1], &mut got);
    assert_eq!(got, b"next record", "bad tags neither close the stream nor poison packet numbers");
}

#[test]
fn first_verified_packet_ends_unsigned_input() {
    let mut w = Pair::new(67);
    w.open();
    let now = w.net.now();
    let side = &mut w.streams[0];
    let peer = side.peer();
    let other = "203.0.113.8:5151".parse().unwrap();
    let frame = |kind, number, seq, ack, body: &[u8], key: Option<&[u8; 32]>| {
        Packet { kind, number, seq, ack, credit: SLOTS, echo: 0, body }.encode(&ID, key)
    };

    assert_eq!(side.write(b"pending"), 7, "queue data whose acknowledgement can be checked");
    let (_, sent) = side.poll_transmit(now).expect("queued data is sent on the validated path");
    assert_eq!(kind(&sent), DATA, "the next cumulative ACK would acknowledge an in-flight segment");

    side.handle_datagram(now, peer, &frame(DATA, 20, 1, 0, b"b", None));
    assert!(side.set_keys(KEYS[0], KEYS[1]), "the local keys are installed with a gap in the receive stream");
    let (_, ack) = side.poll_transmit(now).expect("the gap prompts an immediate ACK");
    let (ack, verified) = Packet::decode(&ack, &ID, Some(&KEYS[0])).expect("gap ACK decodes with the send key");
    assert!(verified && ack.kind == ACK, "even a pending ACK is tagged immediately after local key installation");
    side.handle_datagram(now, other, &frame(DATA, 21, 0, 0, b"other address", None));
    assert!(
        side.candidate.is_none() && side.rx.ready.is_empty(),
        "unsigned input from another address cannot contribute data or start a move"
    );
    for (i, kind) in [FIN, RESET, CHALLENGE, RESPONSE].into_iter().enumerate() {
        let pn = 22 + i as u64;
        let body: &[u8] = if matches!(kind, CHALLENGE | RESPONSE) { b"12345678" } else { b"" };
        side.handle_datagram(now, peer, &frame(kind, pn, 0, 0, body, None));
        assert!(side.state() == State::Open && side.poll_transmit(now).is_none(), "unsigned control {kind} is silent");
        side.handle_datagram(now, peer, &frame(PING, pn, 0, 0, &[], None));
        let reply = side.poll_transmit(now);
        assert!(
            reply.is_some_and(|(_, b)| b[1] == (ACK | 0x80)),
            "control {kind} leaves the same number available for an unsigned PING and its signed ACK"
        );
    }
    side.handle_datagram(now, peer, &frame(DATA, 30, 0, 0, b"wrong tag", Some(&KEYS[0])));
    assert!(!side.gap_closed() && side.rx.ready.is_empty(), "a bad tag cannot release data or latch the peer");
    side.handle_datagram(now, peer, &frame(DATA, 30, 0, 0, b"a", None));
    let mut got = Vec::new();
    drain(side, &mut got);
    assert_eq!(got, b"ab", "unsigned DATA can fill a gap buffered before local key installation");

    side.handle_datagram(now, peer, &frame(DATA, 31, 3, 0, b"speculative", None));
    side.handle_datagram(now, peer, &frame(DATA, 3, 2, 0, b"c", Some(&KEYS[1])));
    got.clear();
    drain(side, &mut got);
    assert!(side.gap_closed(), "unsigned packet numbers cannot mask a lower first verified packet");
    assert_eq!(got, b"c", "the latch discards unsigned out-of-order bytes before processing the gap filler");

    side.handle_datagram(now, peer, &frame(ACK, 40, 0, 1, &[], None));
    side.handle_datagram(now, peer, &frame(DATA, 41, 3, 0, b"unsigned", None));
    assert_eq!(side.read(&mut [0; 16]), 0, "unsigned DATA from the original address is dropped after the latch");
    let tagged = frame(DATA, 42, 3, 0, b"d", Some(&KEYS[1]));
    let mut stripped = tagged[..tagged.len() - 8].to_vec();
    stripped[1] = DATA;
    side.handle_datagram(now, peer, &stripped);
    assert_eq!(side.read(&mut [0; 16]), 0, "clearing the signed bit and stripping the tag cannot deliver data");
    let retry_at = now + side.tx.rto;
    let (_, retry) = side.poll_transmit(retry_at).expect("the unsigned cumulative ACK leaves retransmission armed");
    let (retried, verified) = Packet::decode(&retry, &ID, Some(&KEYS[0])).expect("retransmission decodes");
    assert!(
        verified && retried.kind == DATA && retried.body == b"pending",
        "the unacknowledged data is retransmitted with a valid tag"
    );
    side.handle_datagram(retry_at, peer, &tagged);
    got.clear();
    drain(side, &mut got);
    assert_eq!(got, b"d", "the authentic copy remains eligible after the stripped copy");
}

#[test]
fn cumulative_handshake_ack_ends_unsigned_input() {
    let mut w = Pair::new(68);
    w.open();
    assert_eq!(w.streams[1].write(b"peer key"), 8, "the peer queues its handshake output");
    w.until(|w| w.streams[1].tx.acked_bytes == 8);
    let side = &mut w.streams[0];
    let mut got = Vec::new();
    drain(side, &mut got);
    assert_eq!(got, b"peer key", "the local worker has received the peer's handshake bytes");
    let now = w.net.now();
    let peer = side.peer();
    let next = side.rx.next as u32;
    let unsigned = |kind, number, seq, ack, credit, body: &[u8]| {
        Packet { kind, number, seq, ack, credit, echo: 0, body }.encode(&ID, None)
    };

    let handshake = content(SEGMENT + 1);
    assert_eq!(side.write(&handshake), handshake.len(), "both handshake segments fit in the sender");
    let (_, first) = side.poll_transmit(now).expect("the first handshake segment is sent");
    assert_eq!(kind(&first), DATA, "a handshake segment is in flight while its last byte is still queued");
    assert!(
        side.set_keys(KEYS[0], KEYS[1]) && !side.gap_closed(),
        "queued and in-flight handshake bytes keep the gap open"
    );
    let tx = side.tx.clone();
    side.handle_datagram(now, peer, &unsigned(ACK, 100, 0, 2, SLOTS + 1, &[]));
    assert!(
        side.tx == tx && !side.gap_closed(),
        "an ACK beyond the last transmission cannot retire unsent bytes, grant credit or close the gap"
    );
    side.handle_datagram(now, peer, &unsigned(ACK, 101, 0, 1, SLOTS, &[]));
    assert!(
        side.tx.acked_bytes == SEGMENT as u64 && !side.gap_closed(),
        "one unacknowledged handshake byte keeps the gap open"
    );

    // The first post-key write shares the unsent handshake tail and spills into a third segment.
    assert_eq!(side.write(&handshake[..SEGMENT]), SEGMENT, "later writes can extend the tail past the cutover offset");
    for seq in [1, 2] {
        let (_, bytes) = side.poll_transmit(now).expect("each remaining segment can be sent");
        let (p, verified) = Packet::decode(&bytes, &ID, Some(&KEYS[0])).expect("post-install transmission decodes");
        assert!(verified && p.kind == DATA && p.seq == seq, "post-install transmission {seq} carries tagged data");
    }
    side.handle_datagram(now, peer, &unsigned(DATA, 102, next + 2, 1, SLOTS, b"speculative"));
    side.handle_datagram(now, peer, &unsigned(DATA, 103, next, 2, SLOTS, b"final unsigned data"));
    got.clear();
    drain(side, &mut got);
    assert_eq!(got, b"final unsigned data", "the packet carrying the boundary ACK is processed in full");
    assert!(side.gap_closed() && side.verified_replay.high == 0, "the cumulative ACK alone closes the gap");
    assert!(side.tx.acked_bytes < side.tx.written_bytes, "later writes do not move the handshake boundary");
    assert!(side.rx.reordered.is_empty(), "the ACK boundary discards speculative out-of-order data");

    let before = side.clone();
    let wake = side.next_timeout().expect("live stream has a pending transmission").max(now + 1);
    let expected = before.clone().poll_transmit(wake);
    assert!(expected.is_some(), "there is an actual next datagram to compare");
    for (name, bytes) in [
        ("DATA", unsigned(DATA, 104, next + 1, 2, SLOTS, b"late data")),
        ("ACK", unsigned(ACK, 105, 0, 3, SLOTS, &[])),
        ("PING", unsigned(PING, 106, 0, 2, SLOTS, &[])),
        ("window update", unsigned(ACK, 107, 0, 2, SLOTS + 1, &[])),
    ] {
        side.handle_datagram(now + 1, peer, &bytes);
        assert!(*side == before, "refused unsigned {name} preserves send, receive, replay, path and timer state");
        assert_eq!(
            side.clone().poll_transmit(wake),
            expected,
            "refused unsigned {name} preserves the next transmission"
        );
    }
    let other = &mut w.streams[1];
    assert!(
        other.set_keys(KEYS[1], KEYS[0]) && other.gap_closed(),
        "already acknowledged handshake bytes close the gap at installation"
    );
}

#[test]
fn orderly_close_survives_lost_final_ack() {
    let mut w = Pair::new(77);
    w.open();
    w.keys();
    let start = w.net.now();
    let last_sent = w.streams.each_ref().map(|s| s.last_sent);
    let mut ping_at = None;
    while ping_at.is_none() {
        let now = w.net.now();
        w.step_with(|i, _, bytes| {
            if kind(bytes) == PING {
                ping_at = Some(now - last_sent[i]);
            }
            true
        });
    }
    assert_eq!(ping_at.unwrap(), KEEPALIVE, "idle NAT keepalive is due 15 s after the last send");
    for s in &mut w.streams {
        s.write(b"last bytes");
    }
    w.streams[0].close();
    assert_eq!(w.streams[0].write(b"late"), 0, "a closing stream stops accepting writes");
    let mut lost = false;
    while !w.streams.iter().all(|s| s.state() == State::Closed(CloseReason::Finished)) {
        assert!(w.net.now() < start + 2 * IDLE, "both close handshakes finish");
        w.step_with(|i, _, bytes| {
            if i == 0 && kind(bytes) == ACK && !lost && u32::from_be_bytes(bytes[20..24].try_into().unwrap()) == 2 {
                lost = true;
                return false;
            }
            true
        });
    }
    assert!(lost, "the final cumulative ACK was dropped");
    for s in &mut w.streams {
        let mut got = Vec::new();
        drain(s, &mut got);
        assert_eq!(got, b"last bytes", "FIN follows all accepted writes in both directions");
        assert!(s.next_timeout().is_none(), "clean close has no pending wake");
    }
}

#[test]
fn unsent_bytes_tracks_first_transmissions_under_a_rate_cap() {
    let mut w = Pair::new(29);
    w.open();
    w.keys();
    w.streams[0].set_rate_cap(Some(80_000));
    let data = vec![7; 20 * SEGMENT];
    // A short first write leaves a partial segment at each end of the transfer.
    let mut written = w.streams[0].write(&data[..SEGMENT / 2]);
    let mut sent = 0;
    let deadline = w.net.now() + IDLE;
    while sent < data.len() {
        assert!(w.net.now() < deadline, "the capped stream sends all accepted bytes");
        assert_eq!(w.streams[0].unsent_bytes(), written - sent, "queued payload before transmission");
        w.step_with(|side, _, bytes| {
            if side == 0 && kind(bytes) == DATA {
                let (packet, _) = Packet::decode(bytes, &ID, Some(&KEYS[0])).expect("valid data datagram");
                sent += packet.body.len();
            }
            true
        });
        assert_eq!(w.streams[0].unsent_bytes(), written - sent, "sending retires payload before its ACK");
        if written < data.len() {
            written += w.streams[0].write(&data[written..]);
            if written == data.len() {
                w.streams[0].close(); // Queue the FIN behind the remaining data.
            }
        }
    }
    assert_eq!(w.streams[0].unsent_bytes(), 0, "all payload has gone out");
    assert!(w.streams[0].tx.acked_bytes < sent as u64, "the last data is still in flight");
}

#[test]
fn a_rate_cap_paces_new_data_and_loses_nothing() {
    const CAP: u64 = 400_000;
    let data = content(150_000);
    let mut w = Pair::new(29);
    w.net.set_link(LinkConfig { latency_ms: 10, jitter_ms: 3, loss_per_mille: 10 });
    w.open();
    w.keys();
    w.streams[0].set_rate_cap(Some(CAP));
    let before = w.streams[0].sent_bytes();
    let start = w.net.now();
    // every datagram the capped side sends: (ms, bytes)
    let mut log: Vec<(u64, usize)> = Vec::new();
    let (mut sent, mut received) = (0, Vec::new());
    while received.len() < data.len() {
        assert!(w.net.now() < start + 60_000, "the capped writer makes progress");
        sent += w.streams[0].write(&data[sent..]);
        let now = w.net.now();
        w.step_with(|i, _, bytes| {
            if i == 0 {
                log.push((now, bytes.len()));
            }
            true
        });
        drain(&mut w.streams[1], &mut received);
    }
    assert!(received == data, "every byte arrives once, in order, under the cap");
    assert!(w.net.stats().lost > 0, "the link lost datagrams, so repairs ran under the cap");
    let total: usize = log.iter().map(|&(_, n)| n).sum();
    assert_eq!(w.streams[0].sent_bytes() - before, total as u64, "sent_bytes counts every datagram handed out");

    // no 250 ms holds more than the cap's share, its burst, one datagram of overdraft and a
    // few repairs (they never wait)
    let window = 250;
    let allowance = (CAP * window / 1000 / 8) as usize + 2 * 1200 + 1200 + 3 * 1200;
    for (i, &(t, _)) in log.iter().enumerate() {
        let in_window: usize = log[i..].iter().take_while(|&&(u, _)| u < t + window).map(|&(_, n)| n).sum();
        assert!(in_window <= allowance, "{in_window} bytes in the 250 ms from {t} ms, over the cap's {allowance}");
    }
    let elapsed = log.last().unwrap().0 - start;
    let paced = total as u64 * 8 * 1000 / CAP;
    assert!(elapsed * 10 >= paced * 9, "the cap holds: {total} bytes went in {elapsed} ms, {paced} ms at the cap");
    assert!(elapsed * 10 <= paced * 13, "and the stream keeps up with it: {elapsed} ms against {paced} ms");

    // lifting the cap sends at the stream's own pace again
    w.streams[0].set_rate_cap(None);
    let start = w.net.now();
    let mut received = Vec::new();
    w.transfer(&data, 0, &mut received);
    assert!(received == data, "uncapped again, every byte arrives");
    let uncapped = w.net.now() - start;
    assert!(uncapped * 3 < elapsed, "without the cap the same bytes take {uncapped} ms, not {elapsed}");
}

#[test]
fn queued_peer_input_is_serviced_before_the_idle_timeout_after_a_stall() {
    let mut pair = Pair::new(7);
    pair.open();
    pair.keys();
    let now = pair.net.now();
    assert_eq!(pair.streams[0].write(b"still here"), 10);
    let (_, packet) = pair.streams[0].poll_transmit(now).unwrap();
    let receiver = &mut pair.streams[1];
    let peer = receiver.peer();
    let due = receiver.deadline;
    let mut silent = receiver.clone();
    silent.poll_transmit(due - 1);
    assert_eq!(silent.state(), State::Open);
    silent.poll_transmit(due);
    assert_eq!(silent.state(), State::Closed(CloseReason::TimedOut));

    receiver.handle_datagram(due + 1, peer, &packet);
    assert_eq!(receiver.state(), State::Open, "a queued authenticated packet survives the scheduling stall");
    assert_eq!(receiver.deadline, due + 1 + IDLE);
    receiver.handle_datagram(receiver.deadline, peer, &packet);
    receiver.poll_transmit(receiver.deadline);
    assert_eq!(receiver.state(), State::Closed(CloseReason::TimedOut), "a replay cannot renew the idle timer");
}
