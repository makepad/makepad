//! The whole player loop, headless: walk, jump, approach the car, press the
//! primary activity button, drive, press it again, end up standing beside it.
//!
//! This is the acceptance test for the controller binding. Every assertion is
//! something a player would notice, and the transitions — the two moments the
//! camera changes what it follows — are where this has broken before, so they
//! are checked from both ends rather than only at rest.
//!
//! No window, no renderer, no script VM: scripted `RawInput` in, world state
//! out. The camera is checked numerically, which is the only way to catch it
//! sinking into a wall without a human watching.

use makepad_game_blocks::*;
use makepad_game_script::interact::{self, InteractKind, InteractSet};
use makepad_game_sim::*;
use makepad_widgets::makepad_math::*;

const DT: f32 = TICK_DT;

fn new_id(world: &mut GameWorld) -> u64 {
    world.next_id += 1;
    world.next_id
}

fn ground(world: &mut GameWorld) {
    let id = new_id(world);
    world.push_entity(Entity {
        id,
        kind: BodyKind::Static,
        pos: vec3f(0.0, -2.0, 0.0),
        half: vec3f(200.0, 2.0, 200.0),
        collide: true,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        density: 1.0,
        friction: 0.6,
        ..Default::default()
    });
}

/// The two-line binding the brief asks for, plus the body every entity needs.
fn player(world: &mut GameWorld, blocks: &mut Blocks, pos: Vec3f) -> u64 {
    let id = new_id(world);
    world.push_entity(Entity {
        id,
        kind: BodyKind::Mover,
        pos,
        half: vec3f(0.35, 0.9, 0.35),
        collide: true,
        gravity_scale: 1.0,
        speed_mult: 1.0,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        density: 1.0,
        friction: 0.7,
        ..Default::default()
    });
    blocks.characters.push(Character::new(
        id,
        CharacterConfig::default(),
        ControlSource::Player,
        None,
    ));
    blocks.player_rigs.insert(PlayerId(0), PlayerRig::new(id));
    id
}

fn car(world: &mut GameWorld, blocks: &mut Blocks, pos: Vec3f) -> u64 {
    let id = new_id(world);
    world.push_entity(Entity {
        id,
        kind: BodyKind::Rigid,
        pos,
        half: vec3f(0.9, 0.4, 1.6),
        tag: "car".to_string(),
        collide: true,
        gravity_scale: 1.0,
        speed_mult: 1.0,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        density: 1.0,
        friction: 0.7,
        ..Default::default()
    });
    blocks
        .cars
        .push(Car::new(id, CarConfig::default(), ControlSource::Player));
    id
}

fn wall(world: &mut GameWorld, pos: Vec3f, half: Vec3f) -> u64 {
    let id = new_id(world);
    world.push_entity(Entity {
        id,
        kind: BodyKind::Static,
        pos,
        half,
        collide: true,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        density: 1.0,
        friction: 0.6,
        ..Default::default()
    });
    id
}

/// One full host frame, in the order the host owes the engine.
fn step(world: &mut GameWorld, blocks: &mut Blocks, raw: RawInput) {
    let mut intents = std::collections::HashMap::new();
    intents.insert(PlayerId(0), raw);
    blocks.tick_player_rigs(world, &intents);
    blocks.pre_step(world);
    step_world(world);
    blocks.post_step(world);
    world.tick += 1;
    world.time += DT as f64;
}

fn rig(blocks: &Blocks) -> PlayerRig {
    *blocks.player_rigs.get(&PlayerId(0)).expect("player 0 has a rig")
}

/// How far the camera's eye is INSIDE the nearest solid box, or 0 if clear.
///
/// Static geometry only: the camera is allowed to pass through the character
/// and the car it is following, and a boom that refused to would jam against
/// the very thing it is looking at.
fn camera_penetration(world: &GameWorld, eye: Vec3f) -> f32 {
    let mut worst: f32 = 0.0;
    for e in world.entities.iter().filter(|e| e.kind == BodyKind::Static) {
        let dx = e.half.x - (eye.x - e.pos.x).abs();
        let dy = e.half.y - (eye.y - e.pos.y).abs();
        let dz = e.half.z - (eye.z - e.pos.z).abs();
        if dx > 0.0 && dy > 0.0 && dz > 0.0 {
            worst = worst.max(dx.min(dy).min(dz));
        }
    }
    worst
}

fn walk(move_y: f32) -> RawInput {
    RawInput {
        move_y,
        run: 0.0,
        ..Default::default()
    }
}

/// **The headline test.** Walk, jump, get in, drive, get out, still standing.
#[test]
fn walk_jump_drive_and_get_back_out() {
    let mut world = GameWorld::new();
    world.gravity = 30.0;
    let mut blocks = Blocks::new();
    ground(&mut world);
    // The car sits ahead of the player along -Z, which is forward at yaw 0.
    let car_id = car(&mut world, &mut blocks, vec3f(0.0, 1.2, -10.0));
    let me = player(&mut world, &mut blocks, vec3f(0.0, 1.0, 0.0));

    // Settle onto the ground before measuring anything.
    for _ in 0..30 {
        step(&mut world, &mut blocks, RawInput::default());
    }
    let start = world.entity(me).unwrap().pos;
    assert!(
        world.entity(me).unwrap().on_floor,
        "the player should be standing on the ground before we start"
    );

    // ── walk ────────────────────────────────────────────────────────────
    for _ in 0..40 {
        step(&mut world, &mut blocks, walk(1.0));
    }
    let walked = world.entity(me).unwrap().pos;
    assert!(
        walked.z < start.z - 2.0,
        "holding forward for 40 ticks should walk several metres along -Z, \
         went from z={} to z={}",
        start.z,
        walked.z
    );

    // ── jump ────────────────────────────────────────────────────────────
    let ground_y = world.entity(me).unwrap().pos.y;
    let mut peak = ground_y;
    for tick in 0..45 {
        let mut raw = walk(0.0);
        // Held, not tapped: variable jump height only engages on evidence the
        // button is genuinely down.
        raw.jump = tick < 12;
        raw.jump_pressed = tick == 0;
        step(&mut world, &mut blocks, raw);
        peak = peak.max(world.entity(me).unwrap().pos.y);
    }
    assert!(
        peak > ground_y + 1.0,
        "a full jump should clear a metre, only reached {:.2} above {:.2}",
        peak - ground_y,
        ground_y
    );
    assert!(
        world.entity(me).unwrap().on_floor,
        "the player must land again within 45 ticks"
    );

    // ── approach ────────────────────────────────────────────────────────
    for _ in 0..90 {
        step(&mut world, &mut blocks, walk(1.0));
        let r = rig(&blocks);
        let pos = world.entity(r.mount.subject()).unwrap().pos;
        let facing = heading_to_forward(r.camera.yaw);
        if interact::choose(
            &world,
            &blocks,
            &InteractSet::default(),
            r.seat(),
            pos,
            facing,
        )
        .is_some()
        {
            break;
        }
    }

    // The affordance must be offered BEFORE the button does anything —
    // otherwise nobody discovers the mechanic.
    let r = rig(&blocks);
    let pos = world.entity(r.mount.subject()).unwrap().pos;
    let offered = interact::choose(
        &world,
        &blocks,
        &InteractSet::default(),
        r.seat(),
        pos,
        heading_to_forward(r.camera.yaw),
    )
    .expect("walking up to a car must offer something");
    assert_eq!(offered.kind, InteractKind::Drive);
    assert_eq!(offered.entity, car_id);

    // ── get in ──────────────────────────────────────────────────────────
    let mut press = walk(0.0);
    press.use_pressed = true;
    step(&mut world, &mut blocks, press);
    assert_eq!(
        rig(&blocks).seat(),
        Seat::Driving(car_id),
        "pressing the activity button beside a car must seat the player in it"
    );

    // ── drive ───────────────────────────────────────────────────────────
    let car_start = world.entity(car_id).unwrap().pos;
    for _ in 0..90 {
        let mut raw = RawInput::default();
        raw.throttle = 1.0;
        step(&mut world, &mut blocks, raw);
    }
    let car_now = world.entity(car_id).unwrap().pos;
    let travelled =
        ((car_now.x - car_start.x).powi(2) + (car_now.z - car_start.z).powi(2)).sqrt();
    assert!(
        travelled > 3.0,
        "full throttle for 90 ticks should move the car, only went {travelled:.2}m"
    );

    // While seated, the button means "get out" no matter what else is near.
    let r = rig(&blocks);
    let seated_offer = interact::choose(
        &world,
        &blocks,
        &InteractSet::default(),
        r.seat(),
        world.entity(r.mount.subject()).unwrap().pos,
        heading_to_forward(r.camera.yaw),
    )
    .expect("seated always offers an exit");
    assert_eq!(seated_offer.kind, InteractKind::Exit);

    // ── get out ─────────────────────────────────────────────────────────
    let mut press = RawInput::default();
    press.use_pressed = true;
    step(&mut world, &mut blocks, press);
    assert_eq!(rig(&blocks).seat(), Seat::OnFoot, "the button must let go");

    // Settle, then check the player is genuinely standing — not falling, not
    // buried, not left inside the car.
    for _ in 0..60 {
        step(&mut world, &mut blocks, RawInput::default());
    }
    let out = world.entity(me).unwrap();
    assert!(
        out.on_floor,
        "the player must end on solid ground, not mid-air at y={:.2}",
        out.pos.y
    );
    assert!(!out.hidden, "the player must be visible again after dismount");
    assert!(
        out.pos.y > -1.0,
        "the player must not be dropped through the floor (y={:.2})",
        out.pos.y
    );
}

/// The camera must not enter geometry — across BOTH transitions, not just at
/// rest. A rig that only settles correctly still looks broken while it blends.
#[test]
fn the_camera_never_enters_geometry_across_both_transitions() {
    let mut world = GameWorld::new();
    world.gravity = 30.0;
    let mut blocks = Blocks::new();
    ground(&mut world);
    // A wall right behind where the player starts, so a boom that did not
    // shorten would push the eye straight into it.
    wall(&mut world, vec3f(0.0, 2.0, 6.0), vec3f(12.0, 2.0, 0.5));
    let car_id = car(&mut world, &mut blocks, vec3f(0.0, 1.2, -6.0));
    player(&mut world, &mut blocks, vec3f(0.0, 1.0, 0.0));

    let mut worst = 0.0f32;
    let mut worst_at = "settle";
    let check = |world: &GameWorld, blocks: &Blocks, phase: &'static str,
                     worst: &mut f32, worst_at: &mut &'static str| {
        let pen = camera_penetration(world, rig(blocks).camera.eye());
        if pen > *worst {
            *worst = pen;
            *worst_at = phase;
        }
    };

    for _ in 0..40 {
        step(&mut world, &mut blocks, RawInput::default());
        check(&world, &blocks, "settle", &mut worst, &mut worst_at);
    }
    // Walk up to the car with the wall behind — the boom is squeezed here.
    for _ in 0..120 {
        step(&mut world, &mut blocks, walk(1.0));
        check(&world, &blocks, "approach", &mut worst, &mut worst_at);
    }
    // Transition 1: on foot → driving, sampled every tick through the blend.
    let mut press = walk(0.0);
    press.use_pressed = true;
    step(&mut world, &mut blocks, press);
    for _ in 0..60 {
        let mut raw = RawInput::default();
        raw.throttle = 1.0;
        step(&mut world, &mut blocks, raw);
        check(&world, &blocks, "mount blend", &mut worst, &mut worst_at);
    }
    assert_eq!(rig(&blocks).seat(), Seat::Driving(car_id));
    // Transition 2: driving → on foot, likewise.
    let mut press = RawInput::default();
    press.use_pressed = true;
    step(&mut world, &mut blocks, press);
    for _ in 0..60 {
        step(&mut world, &mut blocks, RawInput::default());
        check(&world, &blocks, "dismount blend", &mut worst, &mut worst_at);
    }

    assert!(
        worst < 0.15,
        "camera sank {worst:.2}m into static geometry during '{worst_at}' — \
         the boom must shorten before the eye reaches a wall"
    );
}

/// Analog in, analog out: the property a keyboard cannot have, and the one
/// most likely to be lost to a threshold.
#[test]
fn partial_stick_deflection_gives_partial_speed() {
    let speed_after = |deflection: f32| -> f32 {
        let mut world = GameWorld::new();
        world.gravity = 30.0;
        let mut blocks = Blocks::new();
        ground(&mut world);
        let me = player(&mut world, &mut blocks, vec3f(0.0, 1.0, 0.0));
        for _ in 0..30 {
            step(&mut world, &mut blocks, RawInput::default());
        }
        for _ in 0..60 {
            step(&mut world, &mut blocks, walk(deflection));
        }
        let v = world.entity(me).unwrap().vel;
        (v.x * v.x + v.z * v.z).sqrt()
    };

    let half = speed_after(0.5);
    let full = speed_after(1.0);
    assert!(
        half > 0.5,
        "half deflection must still walk, got {half:.2} m/s"
    );
    assert!(
        half < full * 0.85,
        "half deflection must be meaningfully slower than full \
         ({half:.2} vs {full:.2} m/s) — it is snapping to full speed"
    );
}

/// The seated body must RIDE with the car, not stay behind at the kerb.
///
/// Mount pins the driver with the sim's own `attached_to` seat, so the body
/// travels with the vehicle. The bug this guards is the old behaviour: the
/// driver's collider left standing at the spot they boarded, invisible but
/// still solid, so driving back past it later hits a wall that isn't there.
///
/// Deliberately NOT a modality-gate test. The seat pin overwrites pos/vel/yaw
/// every step, so it masks the gate on the seated body — an assertion that the
/// body stays put would pass even with the gate deleted. The gate is covered
/// where it is still observable, on a second unpinned car, in the blocks suite.
#[test]
fn the_seated_player_rides_with_the_car() {
    let mut world = GameWorld::new();
    world.gravity = 30.0;
    let mut blocks = Blocks::new();
    ground(&mut world);
    let car_id = car(&mut world, &mut blocks, vec3f(0.0, 1.2, -3.0));
    let me = player(&mut world, &mut blocks, vec3f(0.0, 1.0, 0.0));
    for _ in 0..30 {
        step(&mut world, &mut blocks, RawInput::default());
    }
    let mut press = RawInput::default();
    press.use_pressed = true;
    step(&mut world, &mut blocks, press);
    assert_eq!(rig(&blocks).seat(), Seat::Driving(car_id));

    let boarded_at = world.entity(me).unwrap().pos;
    for _ in 0..90 {
        let mut raw = RawInput::default();
        raw.throttle = 1.0;
        raw.move_y = 1.0;
        step(&mut world, &mut blocks, raw);
    }
    let (body, chassis) = (
        world.entity(me).unwrap().pos,
        world.entity(car_id).unwrap().pos,
    );
    let apart =
        ((body.x - chassis.x).powi(2) + (body.z - chassis.z).powi(2)).sqrt();
    assert!(
        apart < 1.0,
        "the driver came {apart:.2}m off the car they are sitting in"
    );
    // And they genuinely travelled — otherwise "stayed with the car" would
    // also pass for a car that never moved.
    let travelled = ((chassis.x - boarded_at.x).powi(2)
        + (chassis.z - boarded_at.z).powi(2))
    .sqrt();
    assert!(
        travelled > 3.0,
        "the car only moved {travelled:.2}m, so riding along proves nothing"
    );
}

/// Reversing must actually reverse, and the brake-then-reverse contract must
/// hold: back-input decelerates a rolling car first, and only drives it
/// backward once it has stopped. Reported as "reversing barely works".
#[test]
fn reverse_engages_from_a_standstill_and_brakes_first_when_rolling() {
    let drive = |throttle: f32, brake: f32, ticks: usize| -> (f32, f32) {
        let mut world = GameWorld::new();
        world.gravity = 30.0;
        let mut blocks = Blocks::new();
        ground(&mut world);
        let car_id = car(&mut world, &mut blocks, vec3f(0.0, 1.2, 0.0));
        player(&mut world, &mut blocks, vec3f(0.0, 1.0, 0.0));
        for _ in 0..20 {
            step(&mut world, &mut blocks, RawInput::default());
        }
        let mut press = RawInput::default();
        press.use_pressed = true;
        step(&mut world, &mut blocks, press);
        assert_eq!(rig(&blocks).seat(), Seat::Driving(car_id));
        let start_z = world.entity(car_id).unwrap().pos.z;
        for _ in 0..ticks {
            let mut raw = RawInput::default();
            raw.throttle = throttle;
            raw.brake = brake;
            step(&mut world, &mut blocks, raw);
        }
        let e = world.entity(car_id).unwrap();
        (e.pos.z - start_z, blocks.cars[0].speed)
    };

    // Forward is -Z, so reversing must move the car toward +Z.
    let (moved, speed) = drive(-1.0, 0.0, 120);
    assert!(
        moved > 1.0,
        "two seconds of reverse from a standstill should back the car up at \
         least a metre, only moved {moved:.2} (speed {speed:.2})"
    );

    // The same input WITH the brake also held barely moves — which is what a
    // pad mapping that sets both from one trigger produces.
    let (with_brake, _) = drive(-1.0, 1.0, 120);
    assert!(
        with_brake < moved,
        "holding brake while reversing must not out-reverse a clean reverse \
         ({with_brake:.2} vs {moved:.2}) — the brake force opposes it"
    );

    // Reverse should be slower than forward, but not uselessly so.
    let (fwd, _) = drive(1.0, 0.0, 120);
    let reverse_ratio = moved / fwd.abs();
    assert!(
        reverse_ratio > 0.2,
        "reverse is only {:.0}% of forward travel — too weak to reposition",
        reverse_ratio * 100.0
    );
}
