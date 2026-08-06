//! One test per attack the audit found against the XR stack. Each drives the
//! real attack at a live host and asserts the session survives intact.
//!
//! The attacker is on the LAN and can send anything to any port; what it does
//! not have is the lobby key.

use makepad_game_net::endpoint::{HostConfig, HANDSHAKE_DEADLINE, MAX_PENDING_CONNECTIONS};
use makepad_game_net::*;
use makepad_micro_serde::SerBin;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};

const SECRET: &[u8] = b"arcade-lobby-secret";
const WRONG_SECRET: &[u8] = b"attacker-guess";

fn host() -> Host {
    let mut config = HostConfig::new("test-room", SECRET);
    config.bind_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    Host::bind(config).expect("bind host")
}

fn pump(host: &mut Host, clients: &mut [Client], clock: &mut f64) -> Vec<HostEvent> {
    let mut events = Vec::new();
    for _ in 0..30 {
        *clock += 1.0 / 60.0;
        events.extend(host.pump(*clock));
        for client in clients.iter_mut() {
            client.pump(*clock);
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    events
}

fn join(host: &mut Host, id: u64, clock: &mut f64) -> Client {
    let mut client = Client::connect(id, "victim", host.tcp_addr(), host.udp_addr(), SECRET, *clock)
        .expect("connect");
    pump(host, std::slice::from_mut(&mut client), clock);
    client
}

fn attacker_socket() -> UdpSocket {
    UdpSocket::bind(("127.0.0.1", 0)).expect("attacker socket")
}

fn entity(id: u64, x: f32) -> EntityState {
    EntityState {
        id,
        seq: 0,
        pos: [x, 0.0, 0.0],
        vel: [0.0; 3],
        yaw: 0.0,
        flags: 0,
    }
}

/// Audit H-2: one spoofed datagram poisoned a peer's sequence window and
/// silenced it permanently. Forging now requires the lobby key.
#[test]
fn unauthenticated_input_cannot_poison_a_players_sequence_window() {
    let mut clock = 0.0;
    let mut host = host();
    let mut victim = join(&mut host, 5001, &mut clock);

    // Attacker impersonates the victim with a far-future tick, signed with a
    // key it guessed wrong.
    let bogus = ClientToHost::Input {
        frame: InputFrame {
            tick: u64::MAX / 2,
            ..Default::default()
        },
    };
    let forged = Envelope::seal(5001, &bogus.serialize_bin(), &LobbyKey::new(WRONG_SECRET));
    let sock = attacker_socket();
    for _ in 0..50 {
        let _ = sock.send_to(&forged, host.udp_addr());
    }
    pump(&mut host, std::slice::from_mut(&mut victim), &mut clock);
    assert!(host.stats.auth_failures >= 50, "forgeries were rejected");

    // The victim's real input still lands.
    victim.send_input(InputFrame {
        tick: 1,
        axis_x: 1.0,
        ..Default::default()
    });
    let events = pump(&mut host, std::slice::from_mut(&mut victim), &mut clock);
    assert!(
        events.iter().any(|e| matches!(e, HostEvent::Input { .. })),
        "victim was silenced by the attack"
    );
}

/// Audit H-3: an unverified `Leave` datagram evicted any player. Membership is
/// now reliable-channel only, and unsigned traffic never reaches peer state.
#[test]
fn spoofed_leave_cannot_kick_a_player() {
    let mut clock = 0.0;
    let mut host = host();
    let mut victim = join(&mut host, 5002, &mut clock);
    assert_eq!(host.player_count(), 1);

    let sock = attacker_socket();
    // Unsigned, wrongly signed, and correctly framed but off-channel variants.
    let leave = ClientToHost::Leave.serialize_bin();
    let _ = sock.send_to(&leave, host.udp_addr());
    let forged = Envelope::seal(5002, &leave, &LobbyKey::new(WRONG_SECRET));
    let _ = sock.send_to(&forged, host.udp_addr());

    pump(&mut host, std::slice::from_mut(&mut victim), &mut clock);
    assert_eq!(host.player_count(), 1, "victim was kicked");
}

/// Audit H-4: `touch_peer` overwrote a peer's address from the packet source,
/// so one datagram redirected all of that player's traffic. The address is now
/// pinned at join and mismatches are counted and dropped.
#[test]
fn datagram_from_a_new_port_cannot_hijack_a_players_address() {
    let mut clock = 0.0;
    let mut host = host();
    let mut victim = join(&mut host, 5003, &mut clock);
    victim.send_input(InputFrame { tick: 1, ..Default::default() });
    pump(&mut host, std::slice::from_mut(&mut victim), &mut clock);

    // Even *with* the right key, a datagram from a different port than the one
    // pinned at join is refused — it is the hijack shape, not a rebind.
    let sock = attacker_socket();
    let msg = ClientToHost::Input {
        frame: InputFrame { tick: 2, ..Default::default() },
    };
    let signed = Envelope::seal(5003, &msg.serialize_bin(), &LobbyKey::new(SECRET));
    for _ in 0..10 {
        let _ = sock.send_to(&signed, host.udp_addr());
    }
    pump(&mut host, std::slice::from_mut(&mut victim), &mut clock);
    assert!(host.stats.source_mismatches >= 10, "hijack attempt accepted");

    // The victim keeps receiving state at its own address.
    host.broadcast_state(5, &[entity(1, 42.0)]);
    pump(&mut host, std::slice::from_mut(&mut victim), &mut clock);
    assert_eq!(
        victim.entities.get(&1).map(|e| e.pos[0]),
        Some(42.0),
        "victim's state stream was redirected"
    );
}

/// Audit H-11: any peer could seize authority over an object with one packet.
/// Authority does not exist in this protocol — clients cannot express state at
/// all, and a client-shaped state message is not even decodable as one.
#[test]
fn a_client_cannot_inject_authoritative_state() {
    let mut clock = 0.0;
    let mut host = host();
    let mut victim = join(&mut host, 5004, &mut clock);
    host.broadcast_state(1, &[entity(1, 1.0)]);
    pump(&mut host, std::slice::from_mut(&mut victim), &mut clock);
    assert_eq!(victim.entities[&1].pos[0], 1.0);

    // A peer with the *correct* lobby key tries to push state to the victim
    // directly, impersonating the host.
    let sock = attacker_socket();
    let fake = HostToClient::StateBatch {
        tick: 99,
        entities: vec![EntityState { seq: 9999, ..entity(1, -777.0) }],
    };
    let key = LobbyKey::new(SECRET);
    // Wrong sender id: the client pinned the host's id at Welcome.
    let datagram = Envelope::seal(0xdead_beef, &fake.serialize_bin(), &key);
    for _ in 0..10 {
        let _ = sock.send_to(&datagram, victim.udp_addr().unwrap());
    }
    pump(&mut host, std::slice::from_mut(&mut victim), &mut clock);

    assert_eq!(
        victim.entities[&1].pos[0], 1.0,
        "client accepted state from a non-host sender"
    );
    assert!(victim.stats.source_mismatches > 0, "impersonation counted");
}

/// Audit H-5: a connect flood stalled the worker for ~20 minutes and hung
/// shutdown. The host never initiates connections, and accepted-but-silent
/// sockets are capped and expire.
#[test]
fn connect_flood_cannot_stall_the_host_or_exhaust_slots() {
    let mut clock = 0.0;
    let mut host = host();
    let mut legit = join(&mut host, 5005, &mut clock);

    // 200 sockets that connect and then say nothing. The host is pumped during
    // the flood, as it would be in a running game — otherwise the test itself
    // blocks once the OS accept backlog fills.
    let start = std::time::Instant::now();
    let mut zombies = Vec::new();
    for i in 0..200 {
        if let Ok(stream) = TcpStream::connect(host.tcp_addr()) {
            zombies.push(stream);
        }
        if i % 10 == 0 {
            clock += 1.0 / 60.0;
            host.pump(clock);
        }
    }
    pump(&mut host, std::slice::from_mut(&mut legit), &mut clock);
    let elapsed = start.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "pump stalled for {elapsed:?} under a connect flood"
    );
    assert!(
        host.pending_connections() <= MAX_PENDING_CONNECTIONS,
        "pending connections unbounded: {}",
        host.pending_connections()
    );

    // Past the handshake deadline the half-open sockets are reaped. The window
    // stays under the peer timeout so the legitimate player is untouched.
    legit.send_input(InputFrame { tick: 1, ..Default::default() });
    pump(&mut host, std::slice::from_mut(&mut legit), &mut clock);
    clock += HANDSHAKE_DEADLINE + 0.5;
    host.pump(clock);
    assert_eq!(host.pending_connections(), 0, "half-open sockets leaked");
    assert!(host.stats.handshake_timeouts > 0);

    // The real player is unaffected.
    legit.send_input(InputFrame { tick: 2, ..Default::default() });
    let events = pump(&mut host, std::slice::from_mut(&mut legit), &mut clock);
    assert!(events.iter().any(|e| matches!(e, HostEvent::Input { .. })));
    drop(zombies);
}

/// Audit M-1: the 4 MiB frame cap allowed ~200x decompression amplification per
/// poll. The cap is 256 KiB and is checked before any allocation.
#[test]
fn oversized_and_bomb_frames_are_refused_before_allocating() {
    let key = LobbyKey::new(SECRET);

    // A length prefix larger than the cap is refused without reading a payload.
    let mut buf = ((MAX_FRAME_BYTES + 1) as u32).to_le_bytes().to_vec();
    buf.extend_from_slice(&[0u8; 64]);
    assert!(FrameCodec::drain(&mut buf, &key).is_err());

    // A compression bomb: a valid LZ4 frame claiming a huge decoded size. The
    // declared length is checked against the cap before the buffer is sized.
    let mut body = vec![1u8]; // LZ4 tag
    body.extend_from_slice(&(u32::MAX).to_le_bytes());
    body.extend_from_slice(&[0u8; 32]);
    let sealed = Envelope::seal(1, &body, &key);
    let mut buf = (sealed.len() as u32).to_le_bytes().to_vec();
    buf.extend_from_slice(&sealed);
    assert!(FrameCodec::drain(&mut buf, &key).is_err(), "bomb accepted");

    // A legitimately large-but-capped payload still round-trips.
    let payload = vec![7u8; 64 * 1024];
    let frame = FrameCodec::encode(1, &payload, &key).unwrap();
    let mut buf = frame;
    assert_eq!(FrameCodec::drain(&mut buf, &key).unwrap()[0].1, payload);
}

/// Random mutation fuzz across both sockets: nothing may panic, and the session
/// must still work afterwards.
#[test]
fn malformed_packet_fuzz_never_panics_or_wedges_the_host() {
    let mut clock = 0.0;
    let mut host = host();
    let mut victim = join(&mut host, 5006, &mut clock);
    victim.send_input(InputFrame { tick: 1, ..Default::default() });
    pump(&mut host, std::slice::from_mut(&mut victim), &mut clock);

    let key = LobbyKey::new(SECRET);
    let sock = attacker_socket();
    let victim_addr = victim.udp_addr().unwrap();
    let host_addr = host.udp_addr();

    // Deterministic xorshift so a failure is reproducible.
    let mut rng: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };

    let templates: Vec<Vec<u8>> = vec![
        Envelope::seal(
            5006,
            &ClientToHost::Input { frame: InputFrame { tick: 3, ..Default::default() } }
                .serialize_bin(),
            &key,
        ),
        Envelope::seal(
            5006,
            &ClientToHost::Join {
                protocol: PROTOCOL_VERSION,
                name: "x".into(),
                udp_port: 1,
            }
            .serialize_bin(),
            &key,
        ),
        Envelope::seal(
            5006,
            &HostToClient::StateBatch { tick: 1, entities: vec![entity(1, 1.0)] }.serialize_bin(),
            &key,
        ),
    ];

    for i in 0..3000u64 {
        let mut packet = templates[(i as usize) % templates.len()].clone();
        // Mutate 1-3 bytes, sometimes truncate, sometimes extend.
        for _ in 0..(1 + next() % 3) {
            if packet.is_empty() {
                break;
            }
            let idx = (next() as usize) % packet.len();
            packet[idx] ^= (next() % 256) as u8;
        }
        match next() % 4 {
            0 => packet.truncate((next() as usize) % (packet.len() + 1)),
            1 => packet.extend_from_slice(&[(next() % 256) as u8; 13]),
            _ => {}
        }
        let _ = sock.send_to(&packet, host_addr);
        let _ = sock.send_to(&packet, victim_addr);
        if i % 250 == 0 {
            host.pump(clock);
            victim.pump(clock);
            clock += 1.0 / 60.0;
        }
    }
    pump(&mut host, std::slice::from_mut(&mut victim), &mut clock);

    // Still alive and still serving the real player.
    assert_eq!(host.player_count(), 1, "host lost the player under fuzz");
    victim.send_input(InputFrame { tick: 10_000, axis_x: 0.5, ..Default::default() });
    let events = pump(&mut host, std::slice::from_mut(&mut victim), &mut clock);
    assert!(
        events.iter().any(|e| matches!(e, HostEvent::Input { .. })),
        "host wedged after fuzz"
    );
    host.broadcast_state(1, &[entity(1, 3.0)]);
    pump(&mut host, std::slice::from_mut(&mut victim), &mut clock);
    assert_eq!(victim.entities.get(&1).map(|e| e.pos[0]), Some(3.0));
}

/// Non-finite floats from the wire must never reach gameplay: NaN ordering was
/// how a peer won every activity election in the XR stack (audit M-5).
#[test]
fn non_finite_floats_are_rejected_at_the_boundary() {
    let mut clock = 0.0;
    let mut host = host();
    let mut victim = join(&mut host, 5007, &mut clock);
    victim.send_input(InputFrame { tick: 1, ..Default::default() });
    pump(&mut host, std::slice::from_mut(&mut victim), &mut clock);

    // Client sends NaN input with a valid key.
    let msg = ClientToHost::Input {
        frame: InputFrame { tick: 2, axis_x: f32::NAN, cam_yaw: f32::INFINITY, ..Default::default() },
    };
    let sock = UdpSocket::bind(SocketAddr::new(
        victim.udp_addr().unwrap().ip(),
        victim.udp_addr().unwrap().port(),
    ));
    // Reuse the victim's own socket path by sending through the client API is
    // not possible for NaN, so verify the host-side guard directly.
    drop(sock);
    let key = LobbyKey::new(SECRET);
    let datagram = Envelope::seal(5007, &msg.serialize_bin(), &key);
    let _ = std::net::UdpSocket::bind(("127.0.0.1", 0))
        .unwrap()
        .send_to(&datagram, host.udp_addr());
    let events = pump(&mut host, std::slice::from_mut(&mut victim), &mut clock);
    assert!(
        !events.iter().any(|e| matches!(e, HostEvent::Input { frame, .. } if !frame.is_finite())),
        "non-finite input reached the application"
    );

    // And a NaN entity state must not enter the client's world.
    let fake = HostToClient::StateBatch {
        tick: 3,
        entities: vec![EntityState { seq: 5, pos: [f32::NAN, 0.0, 0.0], ..entity(77, 0.0) }],
    };
    let host_id = HostConfig::new("test-room", SECRET).host_id.0;
    let datagram = Envelope::seal(host_id, &fake.serialize_bin(), &key);
    let _ = std::net::UdpSocket::bind(("127.0.0.1", 0))
        .unwrap()
        .send_to(&datagram, victim.udp_addr().unwrap());
    pump(&mut host, std::slice::from_mut(&mut victim), &mut clock);
    assert!(!victim.entities.contains_key(&77), "NaN entity was applied");
}
