//! Full sessions — host plus N clients — driven inside one process over real
//! loopback sockets against a virtual clock.

use makepad_game_net::endpoint::{HostConfig, PEER_TIMEOUT};
use makepad_game_net::*;
use makepad_micro_serde::SerBin;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};

const SECRET: &[u8] = b"arcade-lobby-secret";

fn host() -> Host {
    let mut config = HostConfig::new("test-room", SECRET);
    config.bind_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    Host::bind(config).expect("bind host")
}

fn join(host: &mut Host, id: u64, name: &str, clock: &mut f64) -> Client {
    let mut client = Client::connect(
        id,
        name,
        host.tcp_addr(),
        host.udp_addr(),
        SECRET,
        *clock,
    )
    .expect("connect");
    settle(host, std::slice::from_mut(&mut client), clock);
    client
}

/// Advances the virtual clock a frame at a time, pumping everything, until the
/// sockets go quiet.
fn settle(host: &mut Host, clients: &mut [Client], clock: &mut f64) -> (Vec<HostEvent>, Vec<ClientEvent>) {
    let mut host_events = Vec::new();
    let mut client_events = Vec::new();
    for _ in 0..40 {
        *clock += 1.0 / 60.0;
        host_events.extend(host.pump(*clock));
        for client in clients.iter_mut() {
            client_events.extend(client.pump(*clock));
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    (host_events, client_events)
}

fn entity(id: u64, x: f32) -> EntityState {
    EntityState {
        id,
        seq: 0,
        pos: [x, 0.0, 0.0],
        vel: [0.0, 0.0, 0.0],
        yaw: 0.0,
        flags: 0,
    }
}

#[test]
fn join_leave_and_rejoin_resets_sequence_state() {
    let mut clock = 0.0;
    let mut host = host();

    let mut client = join(&mut host, 1001, "kid", &mut clock);
    assert_eq!(host.player_count(), 1);
    assert_eq!(client.player, Some(PlayerId(1001)));
    assert_eq!(host.player_name(PlayerId(1001)), Some("kid"));

    // Inputs at increasing ticks are delivered.
    for tick in 1..=5 {
        client.send_input(InputFrame {
            tick,
            axis_x: 1.0,
            ..Default::default()
        });
    }
    let (events, _) = settle(&mut host, std::slice::from_mut(&mut client), &mut clock);
    let inputs = events
        .iter()
        .filter(|e| matches!(e, HostEvent::Input { .. }))
        .count();
    assert_eq!(inputs, 5, "all five inputs applied");

    client.leave();
    let (events, _) = settle(&mut host, std::slice::from_mut(&mut client), &mut clock);
    assert!(events.iter().any(|e| matches!(e, HostEvent::Left { .. })));
    assert_eq!(host.player_count(), 0);

    // The same player id returns with its tick counter restarted. Without the
    // rejoin reset the stale-tick window would swallow every input.
    let mut client = join(&mut host, 1001, "kid", &mut clock);
    assert_eq!(host.player_count(), 1);
    client.send_input(InputFrame {
        tick: 1,
        axis_x: -1.0,
        ..Default::default()
    });
    let (events, _) = settle(&mut host, std::slice::from_mut(&mut client), &mut clock);
    assert!(
        events.iter().any(|e| matches!(e, HostEvent::Input { .. })),
        "input after rejoin must not be treated as stale"
    );
}

#[test]
fn client_reconstructs_world_from_snapshot_and_state() {
    let mut clock = 0.0;
    let mut host = host();

    // A session already in progress.
    let world: Vec<EntityState> = (0..8).map(|i| entity(i, i as f32)).collect();
    host.set_snapshot(42, world.clone());

    let mut client = join(&mut host, 7, "latecomer", &mut clock);
    assert_eq!(client.entities.len(), 8, "snapshot reconstructs the world");
    assert_eq!(client.last_tick, 42);

    // Then live state moves them.
    let moved: Vec<EntityState> = (0..8).map(|i| entity(i, 100.0 + i as f32)).collect();
    host.broadcast_state(43, &moved);
    settle(&mut host, std::slice::from_mut(&mut client), &mut clock);

    for i in 0..8 {
        assert_eq!(client.entities[&i].pos[0], 100.0 + i as f32);
    }

    // Reliable events mutate membership.
    host.broadcast_event(44, GameEvent::Spawn { id: 99, kind: 1, state: entity(99, 5.0) });
    host.broadcast_event(45, GameEvent::Remove { id: 0 });
    settle(&mut host, std::slice::from_mut(&mut client), &mut clock);
    assert!(client.entities.contains_key(&99), "spawn applied");
    assert!(!client.entities.contains_key(&0), "removal applied");
}

#[test]
fn six_clients_at_sixty_hz_with_two_hundred_entities() {
    let mut clock = 0.0;
    let mut host = host();
    let mut clients: Vec<Client> = (0..6)
        .map(|i| {
            let mut c = Client::connect(
                2000 + i,
                &format!("p{i}"),
                host.tcp_addr(),
                host.udp_addr(),
                SECRET,
                clock,
            )
            .expect("connect");
            for _ in 0..8 {
                clock += 1.0 / 60.0;
                host.pump(clock);
                c.pump(clock);
            }
            c
        })
        .collect();
    settle(&mut host, &mut clients, &mut clock);
    assert_eq!(host.player_count(), 6, "six players joined");

    // Every client must send an input once so the host pins its datagram
    // address and starts replicating to it.
    for client in clients.iter_mut() {
        client.send_input(InputFrame { tick: 1, ..Default::default() });
    }
    settle(&mut host, &mut clients, &mut clock);

    let mut world: Vec<EntityState> = (0..200).map(|i| entity(i, i as f32)).collect();
    let ticks = 60;
    let start = std::time::Instant::now();
    for tick in 0..ticks {
        clock += 1.0 / 60.0;
        for (i, e) in world.iter_mut().enumerate() {
            e.pos[0] = (tick as f32) * 0.1 + i as f32;
        }
        host.broadcast_state(tick as u64 + 2, &world);
        for client in clients.iter_mut() {
            client.send_input(InputFrame { tick: tick as u64 + 2, ..Default::default() });
        }
        host.pump(clock);
        for client in clients.iter_mut() {
            client.pump(clock);
        }
    }
    let elapsed = start.elapsed();
    settle(&mut host, &mut clients, &mut clock);

    let per_tick_datagrams = host.stats.datagrams_out as f64 / ticks as f64;
    let seconds = ticks as f64 / 60.0;
    println!(
        "6x60Hz x200 entities: {} datagrams out ({:.1}/tick, {:.0} pps), \
         {:.2} MB out ({:.2} MB/s), wall {:?} for {} simulated ticks",
        host.stats.datagrams_out,
        per_tick_datagrams,
        per_tick_datagrams * 60.0,
        host.stats.bytes_out as f64 / 1e6,
        host.stats.bytes_out as f64 / 1e6 / seconds,
        elapsed,
        ticks
    );

    // Entities-per-datagram must stay in the batching sweet spot: one datagram
    // per client per batch, ~17 batches for 200 entities.
    assert!(
        per_tick_datagrams <= 6.0 * 20.0,
        "batching regressed: {per_tick_datagrams} datagrams/tick"
    );

    // Loopback drops under burst; the point is that clients track the world.
    for client in &clients {
        assert!(
            client.entities.len() >= 190,
            "client saw only {} of 200 entities",
            client.entities.len()
        );
    }
    let received: u64 = clients.iter().map(|c| c.stats.datagrams_in).sum();
    assert!(received > 0, "clients received state");
}

#[test]
fn stale_and_duplicate_state_is_ignored_per_entity() {
    let mut clock = 0.0;
    let mut host = host();
    let mut client = join(&mut host, 3001, "p", &mut clock);
    client.send_input(InputFrame { tick: 1, ..Default::default() });
    settle(&mut host, std::slice::from_mut(&mut client), &mut clock);

    host.broadcast_state(1, &[entity(1, 10.0), entity(2, 20.0)]);
    settle(&mut host, std::slice::from_mut(&mut client), &mut clock);
    assert_eq!(client.entities[&1].pos[0], 10.0);

    host.broadcast_state(2, &[entity(1, 11.0), entity(2, 21.0)]);
    settle(&mut host, std::slice::from_mut(&mut client), &mut clock);
    assert_eq!(client.entities[&1].pos[0], 11.0);

    // Replay tick 1's datagram after tick 2 has landed — exactly what UDP
    // reordering looks like. It is correctly signed and genuinely from the
    // host, so only per-entity sequencing can reject it.
    let replay = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    let stale = HostToClient::StateBatch {
        tick: 1,
        entities: vec![
            EntityState { id: 1, seq: 1, pos: [999.0, 0.0, 0.0], vel: [0.0; 3], yaw: 0.0, flags: 0 },
            // A fresh entity in the same datagram must still be applied: the
            // whole batch is not discarded, only the stale members.
            EntityState { id: 3, seq: 1, pos: [30.0, 0.0, 0.0], vel: [0.0; 3], yaw: 0.0, flags: 0 },
        ],
    };
    let key = LobbyKey::new(SECRET);
    let host_id = HostConfig::new("test-room", SECRET).host_id.0;
    let datagram = Envelope::seal(host_id, &stale.serialize_bin(), &key);
    replay
        .send_to(&datagram, client.udp_addr().unwrap())
        .unwrap();

    let before = client.stats.stale_dropped;
    settle(&mut host, std::slice::from_mut(&mut client), &mut clock);
    assert_eq!(client.entities[&1].pos[0], 11.0, "stale entity ignored");
    assert!(client.stats.stale_dropped > before, "staleness was counted");
    assert_eq!(
        client.entities[&3].pos[0], 30.0,
        "fresh entity in the same datagram still applied"
    );

    // A duplicate of the current sequence is also a no-op.
    let dup = client.entities[&2];
    let msg = HostToClient::StateBatch { tick: 2, entities: vec![EntityState { pos: [-5.0, 0.0, 0.0], ..dup }] };
    let datagram = Envelope::seal(host_id, &msg.serialize_bin(), &key);
    replay.send_to(&datagram, client.udp_addr().unwrap()).unwrap();
    settle(&mut host, std::slice::from_mut(&mut client), &mut clock);
    assert_ne!(client.entities[&2].pos[0], -5.0, "duplicate seq ignored");
}

#[test]
fn peer_timeout_drops_a_silent_client() {
    let mut clock = 0.0;
    let mut host = host();
    let mut client = join(&mut host, 4001, "quiet", &mut clock);
    assert_eq!(host.player_count(), 1);

    // Jump past the timeout without the client speaking.
    clock += PEER_TIMEOUT + 1.0;
    let events = host.pump(clock);
    assert!(events
        .iter()
        .any(|e| matches!(e, HostEvent::Left { reason: LeaveReason::Timeout, .. })));
    assert_eq!(host.player_count(), 0);
    let _ = client.pump(clock);
}

#[test]
fn lobby_is_capped_at_max_players() {
    let mut clock = 0.0;
    let mut host = host();
    let mut clients = Vec::new();
    for i in 0..(MAX_PLAYERS + 3) {
        if let Ok(mut c) = Client::connect(
            9000 + i as u64,
            "p",
            host.tcp_addr(),
            host.udp_addr(),
            SECRET,
            clock,
        ) {
            for _ in 0..6 {
                clock += 1.0 / 60.0;
                host.pump(clock);
                c.pump(clock);
            }
            clients.push(c);
        }
    }
    settle(&mut host, &mut clients, &mut clock);
    assert_eq!(host.player_count(), MAX_PLAYERS, "cap enforced");
    assert!(host.stats.rejected_full > 0, "over-cap joins were rejected");
}

#[test]
fn a_client_authoring_request_reaches_the_host() {
    // The keyless-client path: a device with no API key types a request and
    // the host — which owns the agent — receives it verbatim.
    let mut clock = 0.0;
    let mut host = host();
    let mut client = join(&mut host, 7, "tablet", &mut clock);

    client.send_intent(Intent::Authoring {
        text: "make a racing game with boats".to_string(),
    });
    let (events, _) = settle(&mut host, std::slice::from_mut(&mut client), &mut clock);

    let authoring: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            HostEvent::Intent { player, intent: Intent::Authoring { text } } => {
                Some((*player, text.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(authoring.len(), 1, "expected exactly one authoring intent");
    assert_eq!(authoring[0].1, "make a racing game with boats");
}

#[test]
fn a_remote_claude_submits_an_edit_and_is_answered_over_the_wire() {
    // The multi-Claude path: a client's agent asks for the base, submits an
    // edit against it, and the host answers that client alone.
    let mut clock = 0.0;
    let mut host = host();
    let mut client = join(&mut host, 11, "laptop", &mut clock);

    client.send_coedit(CoeditRequest::GetBase);
    let (events, _) = settle(&mut host, std::slice::from_mut(&mut client), &mut clock);
    let asked = events.iter().any(|e| {
        matches!(
            e,
            HostEvent::Coedit {
                req: CoeditRequest::GetBase,
                ..
            }
        )
    });
    assert!(asked, "the host must see the base request");

    // The host answers with the source it holds.
    host.send_coedit(
        PlayerId(11),
        CoeditResponse::Base {
            generation: 0,
            source: "cars {\n  count: 4\n}\n".to_string(),
        },
    );
    let (_, client_events) = settle(&mut host, std::slice::from_mut(&mut client), &mut clock);
    let base = client_events.iter().find_map(|e| match e {
        ClientEvent::Coedit {
            res: CoeditResponse::Base { generation, source },
        } => Some((*generation, source.clone())),
        _ => None,
    });
    assert_eq!(base, Some((0, "cars {\n  count: 4\n}\n".to_string())));

    client.send_coedit(CoeditRequest::Submit {
        intent: "eight cars".to_string(),
        base_generation: 0,
        source: "cars {\n  count: 8\n}\n".to_string(),
    });
    let (events, _) = settle(&mut host, std::slice::from_mut(&mut client), &mut clock);
    let submitted = events.iter().find_map(|e| match e {
        HostEvent::Coedit {
            player,
            req:
                CoeditRequest::Submit {
                    intent,
                    base_generation,
                    source,
                },
        } => Some((*player, intent.clone(), *base_generation, source.clone())),
        _ => None,
    });
    let (player, intent, base_generation, source) =
        submitted.expect("the submission must arrive intact");
    assert_eq!(player, PlayerId(11));
    assert_eq!(intent, "eight cars");
    assert_eq!(base_generation, 0);
    assert_eq!(source, "cars {\n  count: 8\n}\n");
}

#[test]
fn a_malformed_edit_is_refused_without_reaching_the_intent_log() {
    let mut clock = 0.0;
    let mut host = host();
    let mut client = join(&mut host, 12, "laptop", &mut clock);

    // Empty intent and an over-cap source: both must be refused at the edge.
    client.send_coedit(CoeditRequest::Submit {
        intent: "   ".to_string(),
        base_generation: 0,
        source: "x\n".to_string(),
    });
    client.send_coedit(CoeditRequest::Submit {
        intent: "huge".to_string(),
        base_generation: 0,
        source: "x".repeat(MAX_COEDIT_SOURCE + 1),
    });
    let (events, client_events) = settle(&mut host, std::slice::from_mut(&mut client), &mut clock);

    assert!(
        !events
            .iter()
            .any(|e| matches!(e, HostEvent::Coedit { .. })),
        "malformed submissions must not surface as host events"
    );
    let refusals = client_events
        .iter()
        .filter(|e| {
            matches!(
                e,
                ClientEvent::Coedit {
                    res: CoeditResponse::Refused {
                        reason: CoeditRefusal::Malformed
                    }
                }
            )
        })
        .count();
    assert_eq!(refusals, 2, "each malformed submission is answered");
}

#[test]
fn source_cannot_be_rewritten_by_a_datagram() {
    // Coedit is reliable-channel only: accepting it over UDP would let one
    // spoofed datagram rewrite the game.
    let mut clock = 0.0;
    let mut host = host();
    let mut client = join(&mut host, 13, "laptop", &mut clock);

    let payload = ClientToHost::Coedit {
        req: CoeditRequest::Submit {
            intent: "sneaky".to_string(),
            base_generation: 0,
            source: "cars {\n  count: 999\n}\n".to_string(),
        },
    }
    .serialize_bin();
    // Correctly signed, from the real player — only the channel is wrong.
    let datagram = Envelope::seal(13, &payload, &LobbyKey::new(SECRET));
    let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    sock.send_to(&datagram, host.udp_addr()).unwrap();

    let (events, _) = settle(&mut host, std::slice::from_mut(&mut client), &mut clock);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, HostEvent::Coedit { .. })),
        "a datagram must never carry an edit into the intent log"
    );
}
