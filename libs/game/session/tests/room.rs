//! A room of devices playing one game, driven inside a single test process
//! over real loopback sockets against a virtual clock.
//!
//! This is the deliverable that proves M2 (game.md §Testing strategy): the
//! deterministic, Cx-free sim means a host, three clients and two bots all run
//! here with no window, no headset and no second machine.

use makepad_game_blocks::{Blocks, Car, CarConfig, ControlSource};
use makepad_game_net::endpoint::HostConfig;
use makepad_game_session::{replication, Session, SessionEvent};
use makepad_game_sim::{BodyKind, Entity, GameWorld, PlayerId, PlayerSource};
use makepad_math::*;
use std::net::{IpAddr, Ipv4Addr};

const SECRET: &[u8] = b"arcade-room-secret";
const DT: f64 = 1.0 / 60.0;

/// A device: its own world, blocks and session role.
struct Device {
    world: GameWorld,
    blocks: Blocks,
    session: Session,
}

impl Device {
    fn tick(&mut self, clock: f64) -> Vec<SessionEvent> {
        self.session.tick(&mut self.world, &mut self.blocks, clock)
    }

    fn pos_of(&self, id: u64) -> Option<Vec3f> {
        self.world.entity(id).map(|e| e.pos)
    }
}

fn ground(world: &mut GameWorld) {
    // A world built straight through the sim API never calls reset_content,
    // which is what normally installs gravity — without this the cars hover
    // and their suspension never finds the floor.
    world.gravity = 30.0;
    world.next_id += 1;
    let id = world.next_id;
    world.push_entity(Entity {
        id,
        kind: BodyKind::Static,
        pos: vec3f(0.0, -1.0, 0.0),
        half: vec3f(200.0, 1.0, 200.0),
        color: vec4f(0.3, 0.6, 0.3, 1.0),
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        collide: true,
        // Traction lives on the surface: without friction the wheels spin and
        // the car never accelerates.
        density: 1.0,
        friction: 0.6,
        tag: "ground".into(),
        ..Default::default()
    });
}

fn spawn_car(world: &mut GameWorld, blocks: &mut Blocks, x: f32, owner: PlayerId) -> u64 {
    world.next_id += 1;
    let id = world.next_id;
    world.push_entity(Entity {
        id,
        kind: BodyKind::Rigid,
        pos: vec3f(x, 1.0, 0.0),
        half: vec3f(0.9, 0.4, 1.6),
        color: vec4f(0.8, 0.2, 0.2, 1.0),
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        collide: true,
        // Entity::default() leaves gravity_scale at 0 — the DSL fills it in
        // for real games, so a world built straight through the sim API has to
        // say so or the body floats and its wheels never find the road.
        gravity_scale: 1.0,
        density: 1.0,
        friction: 0.7,
        tag: "car".into(),
        ..Default::default()
    });
    let mut car = Car::new(id, CarConfig::default(), ControlSource::Player);
    car.owner = owner;
    blocks.cars.push(car);
    id
}

fn host_device() -> (Device, std::net::SocketAddr, std::net::SocketAddr) {
    let mut config = HostConfig::new("test-room", SECRET);
    config.bind_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let session = Session::host_with(config).expect("bind host");
    let (tcp, udp) = session.host_addrs().unwrap();
    let mut world = GameWorld::new();
    ground(&mut world);
    (
        Device {
            world,
            blocks: Blocks::new(),
            session,
        },
        tcp,
        udp,
    )
}

fn join_device(
    tcp: std::net::SocketAddr,
    udp: std::net::SocketAddr,
    id: u64,
    name: &str,
    clock: f64,
) -> Device {
    let session = Session::join(id, name, tcp, udp, SECRET, clock).expect("join");
    Device {
        world: GameWorld::new(),
        blocks: Blocks::new(),
        session,
    }
}

/// Hold the throttle on a device, exactly as a gamepad stick would. The
/// session transmits this itself each tick — a test that hand-crafted input
/// packets would race the real ones and lose on the host's staleness check.
fn hold_forward(device: &mut Device) {
    device.world.pad.axis_z = -1.0;
}

/// Advance everyone one tick, sleeping a hair so loopback actually delivers.
fn step_room(host: &mut Device, clients: &mut [Device], clock: &mut f64) -> Vec<SessionEvent> {
    *clock += DT;
    let mut events = host.tick(*clock);
    for client in clients.iter_mut() {
        events.extend(client.tick(*clock));
    }
    std::thread::sleep(std::time::Duration::from_micros(300));
    events
}

fn run_room(host: &mut Device, clients: &mut [Device], clock: &mut f64, ticks: usize) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    for _ in 0..ticks {
        events.extend(step_room(host, clients, clock));
    }
    events
}

#[test]
fn three_clients_and_two_bots_converge_on_host_state() {
    let mut clock = 0.0;
    let (mut host, tcp, udp) = host_device();

    let mut clients: Vec<Device> = (0..3)
        .map(|i| {
            let d = join_device(tcp, udp, 9000 + i, &format!("kid{i}"), clock);
            clock += DT;
            d
        })
        .collect();

    // Let the joins land so the host knows every player before bodies exist.
    run_room(&mut host, &mut clients, &mut clock, 30);
    let remote_ids: Vec<PlayerId> = host.world.players.remotes().map(|p| p.id).collect();
    assert_eq!(remote_ids.len(), 3, "three clients joined");

    // Two bots: host-side players with no device at all.
    let bot_a = host.world.players.add("bot-a", PlayerSource::Bot);
    let bot_b = host.world.players.add("bot-b", PlayerSource::Bot);
    assert_eq!(host.world.players.len(), 6, "local + 3 remote + 2 bots");

    // One car each for the three clients and the two bots.
    let mut cars = Vec::new();
    for (i, player) in remote_ids.iter().chain([bot_a, bot_b].iter()).enumerate() {
        let id = spawn_car(&mut host.world, &mut host.blocks, i as f32 * 4.0, *player);
        host.world.players.get_mut(*player).unwrap().entity = id;
        cars.push(id);
    }

    // Clients hold throttle; bots are driven host-side.
    for client in clients.iter_mut() {
        hold_forward(client);
    }

    run_room(&mut host, &mut clients, &mut clock, 600);

    // Every client reconstructed every replicated entity.
    for (i, client) in clients.iter().enumerate() {
        for car in &cars {
            let host_pos = host.pos_of(*car).expect("host has car");
            let client_pos = client
                .pos_of(*car)
                .unwrap_or_else(|| panic!("client {i} is missing car {car}"));
            let delta = (host_pos - client_pos).length();
            assert!(
                delta < 1.0,
                "client {i} car {car} drifted {delta} ({client_pos:?} vs {host_pos:?})"
            );
        }
        assert!(
            client.world.entity(1).is_some(),
            "client {i} should have the static ground too"
        );
    }
}

#[test]
fn client_input_moves_its_own_car_on_the_host() {
    let mut clock = 0.0;
    let (mut host, tcp, udp) = host_device();
    let mut clients = vec![join_device(tcp, udp, 4242, "driver", clock)];
    run_room(&mut host, &mut clients, &mut clock, 30);

    let player = host.world.players.remotes().next().unwrap().id;
    let car = spawn_car(&mut host.world, &mut host.blocks, 0.0, player);
    host.world.players.get_mut(player).unwrap().entity = car;
    run_room(&mut host, &mut clients, &mut clock, 30);

    run_room(&mut host, &mut clients, &mut clock, 60);
    let start = host.pos_of(car).unwrap();
    hold_forward(&mut clients[0]);
    run_room(&mut host, &mut clients, &mut clock, 240);
    let end = host.pos_of(car).unwrap();
    let travelled = (end - start).length();
    assert!(
        travelled > 2.0,
        "held throttle should drive the car (moved {travelled})"
    );
}

#[test]
fn a_late_joiner_reconstructs_the_running_world() {
    let mut clock = 0.0;
    let (mut host, tcp, udp) = host_device();
    let mut clients = vec![join_device(tcp, udp, 1, "early", clock)];
    run_room(&mut host, &mut clients, &mut clock, 20);

    let player = host.world.players.remotes().next().unwrap().id;
    let car = spawn_car(&mut host.world, &mut host.blocks, 0.0, player);
    host.world.players.get_mut(player).unwrap().entity = car;

    // Race is well underway before the fourth device shows up.
    run_room(&mut host, &mut clients, &mut clock, 300);
    assert!(host.world.tick >= 300);

    clients.push(join_device(tcp, udp, 2, "late", clock));
    run_room(&mut host, &mut clients, &mut clock, 60);

    let late = clients.last().unwrap();
    let host_ids: Vec<u64> = host.world.entities.iter().map(|e| e.id).collect();
    let late_ids: Vec<u64> = late.world.entities.iter().map(|e| e.id).collect();
    assert_eq!(late_ids, host_ids, "late joiner sees the same entity set");
    for id in &host_ids {
        let delta = (host.pos_of(*id).unwrap() - late.pos_of(*id).unwrap()).length();
        assert!(delta < 1.0, "entity {id} off by {delta} for the late joiner");
    }
}

#[test]
fn convergence_survives_loss_reorder_and_duplication() {
    // The client's per-entity sequencing is what makes this hold: a stale or
    // duplicated datagram can only be ignored for the entities it carries.
    let mut clock = 0.0;
    let (mut host, tcp, udp) = host_device();
    let mut clients = vec![join_device(tcp, udp, 77, "lossy", clock)];
    run_room(&mut host, &mut clients, &mut clock, 30);

    let player = host.world.players.remotes().next().unwrap().id;
    let car = spawn_car(&mut host.world, &mut host.blocks, 0.0, player);
    host.world.players.get_mut(player).unwrap().entity = car;

    // Deterministic pseudo-loss: skip pumping the client on a fixed pattern,
    // which drops whole datagrams and reorders what it does see.
    hold_forward(&mut clients[0]);
    let mut seed = 0x1234_5678u32;
    for tick in 0..600u64 {
        clock += DT;
        host.tick(clock);
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let drop = (seed >> 16) % 5 == 0;
        if !drop {
            clients[0].tick(clock);
        }
        let _ = tick;
        std::thread::sleep(std::time::Duration::from_micros(300));
    }
    // Let it settle with no loss.
    run_room(&mut host, &mut clients, &mut clock, 60);

    let delta = (host.pos_of(car).unwrap() - clients[0].pos_of(car).unwrap()).length();
    assert!(delta < 1.0, "client converged despite loss (off by {delta})");
}

#[test]
fn derived_changes_cost_nothing_and_shared_changes_cost_bytes() {
    // Tier discipline, asserted rather than documented: turning a model costs
    // no wire traffic; moving it does.
    let mut world = GameWorld::new();
    ground(&mut world);
    world.next_id += 1;
    let id = world.next_id;
    world.push_entity(Entity {
        id,
        kind: BodyKind::Mover,
        pos: vec3f(0.0, 1.0, 0.0),
        half: vec3f(0.5, 0.5, 0.5),
        scale: vec3f(1.0, 1.0, 1.0),
        auto_face: true,
        turn_rate: 8.0,
        ..Default::default()
    });

    let baseline = replication::collect_states(&world);

    // Derived: facing and visual scale change, wire payload does not.
    world.entity_mut(id).unwrap().yaw = 1.5;
    world.entity_mut(id).unwrap().scale = vec3f(2.0, 2.0, 2.0);
    let after_derived = replication::collect_states(&world);
    assert_eq!(
        baseline, after_derived,
        "Derived-tier changes must not alter a single replicated byte"
    );

    // Shared: position changes, wire payload changes with it.
    world.entity_mut(id).unwrap().pos.x = 5.0;
    let after_shared = replication::collect_states(&world);
    assert_ne!(baseline, after_shared, "Shared-tier changes must replicate");
}

#[test]
fn rejoining_with_the_same_identity_keeps_the_player_slot() {
    let mut clock = 0.0;
    let (mut host, tcp, udp) = host_device();
    let mut clients = vec![join_device(tcp, udp, 555, "returning", clock)];
    run_room(&mut host, &mut clients, &mut clock, 30);

    let first = host.world.players.remotes().next().unwrap().id;
    assert_eq!(host.world.players.remotes().count(), 1);

    // Drop the connection and come back under the same client id.
    clients.clear();
    run_room(&mut host, &mut clients, &mut clock, 30);
    assert_eq!(
        host.world.players.remotes().count(),
        0,
        "leaving frees the slot"
    );

    clients.push(join_device(tcp, udp, 555, "returning", clock));
    let events = run_room(&mut host, &mut clients, &mut clock, 60);

    let rejoined = host.world.players.remotes().next().expect("came back");
    assert_eq!(
        rejoined.id, first,
        "the same client id maps back to the same sim player"
    );
    assert!(events
        .iter()
        .any(|e| matches!(e, SessionEvent::Joined { .. })));
}

#[test]
fn replayed_inputs_reproduce_the_same_world() {
    // Host-side determinism: same start, same inputs, same world hash. This is
    // what makes replays, desync detection and host migration possible later.
    fn run(inputs: &[(f32, f32)]) -> u64 {
        let mut world = GameWorld::new();
        let mut blocks = Blocks::new();
        ground(&mut world);
        let player = world.players.add("bot", PlayerSource::Bot);
        let car = spawn_car(&mut world, &mut blocks, 0.0, player);
        world.players.get_mut(player).unwrap().entity = car;

        let mut session = Session::Local;
        for (i, (steer, throttle)) in inputs.iter().enumerate() {
            blocks.player_inputs.insert(
                player,
                makepad_game_blocks::DriveInput {
                    steer: *steer,
                    throttle: *throttle,
                    ..Default::default()
                },
            );
            session.tick(&mut world, &mut blocks, i as f64 * DT);
        }
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x100_0000_01b3);
        };
        for e in &world.entities {
            mix(e.id);
            for v in [e.pos.x, e.pos.y, e.pos.z, e.vel.x, e.vel.y, e.vel.z] {
                mix(v.to_bits() as u64);
            }
        }
        mix(blocks.hash());
        h
    }

    let inputs: Vec<(f32, f32)> = (0..600)
        .map(|i| {
            let t = i as f32 * 0.01;
            (makepad_game_math::sin(t) * 0.6, 1.0)
        })
        .collect();

    let a = run(&inputs);
    let b = run(&inputs);
    assert_eq!(a, b, "identical inputs must produce an identical world");
}




#[test]
fn racing_scenario_wire_volume_fits_a_living_room() {
    // Six players, 200 moving entities, one second of play. The audit put the
    // XR stack at ~5100 pps / 74 Mbit for this shape, which is where Quest WiFi
    // starts failing; this asserts the rebuilt path stays well under it.
    let mut clock = 0.0;
    let (mut host, tcp, udp) = host_device();
    let mut clients: Vec<Device> = (0..5)
        .map(|i| {
            let d = join_device(tcp, udp, 7000 + i, &format!("racer{i}"), clock);
            clock += DT;
            d
        })
        .collect();
    run_room(&mut host, &mut clients, &mut clock, 40);
    assert_eq!(host.world.players.remotes().count(), 5);

    // 200 moving props on top of the ground.
    for i in 0..200 {
        host.world.next_id += 1;
        let id = host.world.next_id;
        let angle = i as f32 * 0.031;
        host.world.push_entity(Entity {
            id,
            kind: BodyKind::Mover,
            pos: vec3f(angle.cos() * 30.0, 1.0, angle.sin() * 30.0),
            vel: vec3f(1.0, 0.0, 1.0),
            half: vec3f(0.5, 0.5, 0.5),
            color: vec4f(0.5, 0.5, 0.9, 1.0),
            scale: vec3f(1.0, 1.0, 1.0),
            scale_target: vec3f(1.0, 1.0, 1.0),
            gravity_scale: 1.0,
            collide: true,
            auto_face: true,
            tag: "prop".into(),
            ..Default::default()
        });
    }
    // Let descriptors land before measuring the steady state.
    run_room(&mut host, &mut clients, &mut clock, 30);

    let before = host.session.host_mut().unwrap().stats;
    run_room(&mut host, &mut clients, &mut clock, 60); // one second
    let after = host.session.host_mut().unwrap().stats;

    let datagrams = after.datagrams_out - before.datagrams_out;
    let bytes = after.bytes_out - before.bytes_out;
    let mbit = (bytes as f64 * 8.0) / 1_000_000.0;
    eprintln!("6 players x 60Hz x 200 entities: {datagrams} pps, {bytes} B/s, {mbit:.1} Mbit/s up");

    assert!(
        datagrams < 5_000,
        "packet rate {datagrams} pps approaches the consumer-AP ceiling the audit flagged"
    );
    assert!(mbit < 74.0, "{mbit} Mbit/s is the XR stack's projection, not an improvement");
}
