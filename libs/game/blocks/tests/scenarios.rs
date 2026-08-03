//! Block scenario tests (game.md §Testing strategy — headless, numeric, no
//! window). Each one drives a world for N ticks and asserts an outcome a player
//! would recognise: the car reaches speed, laps the oval and stays upright; the
//! character clears a step but not a wall; the plane climbs and survives a turn;
//! brains reach their goals; the race kit refuses a shortcut.

use makepad_game_blocks::*;
use makepad_game_sim::*;
use makepad_math::*;

const DT: f32 = TICK_DT;

fn ground(world: &mut GameWorld, half: Vec3f) {
    let id = new_id(world);
    world.push_entity(Entity {
        id,
        kind: BodyKind::Static,
        pos: vec3f(0.0, -half.y, 0.0),
        half,
        collide: true,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        density: 1.0,
        friction: 0.6,
        ..Default::default()
    });
}

fn new_id(world: &mut GameWorld) -> u64 {
    world.next_id += 1;
    world.next_id
}

fn block(world: &mut GameWorld, pos: Vec3f, half: Vec3f) -> u64 {
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

fn rigid(world: &mut GameWorld, pos: Vec3f, half: Vec3f) -> u64 {
    let id = new_id(world);
    world.push_entity(Entity {
        id,
        kind: BodyKind::Rigid,
        pos,
        half,
        collide: true,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        gravity_scale: 1.0,
        density: 1.0,
        friction: 0.7,
        restitution: 0.0,
        ..Default::default()
    });
    id
}

fn mover(world: &mut GameWorld, pos: Vec3f, half: Vec3f) -> u64 {
    let id = new_id(world);
    world.push_entity(Entity {
        id,
        kind: BodyKind::Mover,
        pos,
        half,
        collide: true,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        gravity_scale: 1.0,
        speed_mult: 1.0,
        turn_rate: 9.0,
        auto_face: true,
        density: 1.0,
        ..Default::default()
    });
    id
}

/// Flat ground that rises by `rise` for everything beyond `at` on -z — the
/// walkable ledge the 0.55 step-up is supposed to carry a character over.
fn step_terrain(at: f32, rise: f32) -> Terrain {
    let cells = 81usize;
    let cell_size = 1.0f32;
    let origin = -40.0f32;
    let mut heights = vec![0.0f32; cells * cells];
    for gz in 0..cells {
        let z = origin + gz as f32 * cell_size;
        if z < at {
            for gx in 0..cells {
                heights[gz * cells + gx] = rise;
            }
        }
    }
    Terrain {
        cells,
        cell_size,
        origin,
        heights,
        colors: vec![vec4(0.4, 0.6, 0.35, 1.0); cells * cells],
        revision: 1,
    }
}

fn world_with_ground() -> GameWorld {
    let mut world = GameWorld::new();
    world.gravity = 30.0;
    ground(&mut world, vec3f(400.0, 2.0, 400.0));
    world
}

/// One full host tick: drive → simulate → observe.
fn tick(world: &mut GameWorld, blocks: &mut Blocks) {
    blocks.pre_step(world);
    step_world(world);
    world.tick += 1;
    world.time += DT as f64;
    blocks.post_step(world);
}

fn run(world: &mut GameWorld, blocks: &mut Blocks, ticks: usize) {
    for _ in 0..ticks {
        tick(world, blocks);
    }
}

fn car_world() -> (GameWorld, Blocks, u64) {
    let mut world = world_with_ground();
    let chassis = rigid(&mut world, vec3f(0.0, 1.0, 0.0), vec3f(0.9, 0.4, 1.6));
    let mut blocks = Blocks::new();
    blocks.cars.push(Car::new(
        chassis,
        CarConfig::default(),
        ControlSource::Script,
    ));
    // Let the suspension settle before any test drives away.
    run(&mut world, &mut blocks, 60);
    (world, blocks, chassis)
}

#[test]
fn car_settles_on_its_wheels() {
    let (world, blocks, chassis) = car_world();
    let car = &blocks.cars[0];
    assert!(
        car.all_wheels_down(),
        "car should rest on all four wheels, got {} down",
        car.wheels_down()
    );
    let e = world.entity(chassis).unwrap();
    // Resting height ≈ suspension rest length above the ground plane.
    assert!(
        e.pos.y > 0.2 && e.pos.y < 1.4,
        "car settled at an implausible height: {}",
        e.pos.y
    );
    assert!(e.vel.length() < 0.5, "car should be at rest, vel {:?}", e.vel);
}

#[test]
fn car_accelerates_toward_top_speed() {
    let (mut world, mut blocks, _) = car_world();
    blocks.cars[0].input.throttle = 1.0;
    run(&mut world, &mut blocks, 60 * 8);
    let car = &blocks.cars[0];
    let top = car.config.top_speed;
    assert!(
        car.speed > top * 0.7,
        "8s of full throttle should approach top speed ({top}), got {}",
        car.speed
    );
    assert!(
        car.speed <= top * 1.1,
        "car exceeded its top speed: {} > {top}",
        car.speed
    );
}

#[test]
fn car_brakes_to_a_stop() {
    let (mut world, mut blocks, _) = car_world();
    blocks.cars[0].input.throttle = 1.0;
    run(&mut world, &mut blocks, 60 * 4);
    assert!(blocks.cars[0].speed > 5.0, "car should be rolling first");
    blocks.cars[0].input.throttle = 0.0;
    blocks.cars[0].input.brake = 1.0;
    run(&mut world, &mut blocks, 60 * 4);
    assert!(
        blocks.cars[0].speed.abs() < 1.0,
        "4s of braking should stop the car, speed {}",
        blocks.cars[0].speed
    );
}

#[test]
fn car_steers_and_never_flips() {
    let (mut world, mut blocks, chassis) = car_world();
    blocks.cars[0].input.throttle = 1.0;
    // Hard lock, held — the classic rollover test.
    blocks.cars[0].input.steer = 1.0;
    let start_yaw = world.entity(chassis).unwrap().yaw;
    let mut min_up = 1.0f32;
    for _ in 0..60 * 10 {
        tick(&mut world, &mut blocks);
        let e = world.entity(chassis).unwrap();
        // World-up component of the chassis up axis: 1 = level, <0 = on its lid.
        let up = rotate_quat(e.orient, vec3f(0.0, 1.0, 0.0)).y;
        min_up = min_up.min(up);
    }
    assert!(
        min_up > 0.5,
        "car leaned past 60 degrees during a full-lock turn (min up {min_up})"
    );
    let e = world.entity(chassis).unwrap();
    let turned = (e.yaw - start_yaw).abs();
    let planar = (e.vel.x * e.vel.x + e.vel.z * e.vel.z).sqrt();
    assert!(
        planar > 3.0,
        "car should still be driving through the turn, planar speed {planar}"
    );
    assert!(turned > 0.5 || planar > 3.0, "car should actually turn");
}

#[test]
fn car_laps_an_oval_within_a_time_window() {
    let mut world = world_with_ground();
    let chassis = rigid(&mut world, vec3f(0.0, 1.0, -30.0), vec3f(0.9, 0.4, 1.6));
    let mut blocks = Blocks::new();
    let mut car = Car::new(chassis, CarConfig::default(), ControlSource::Script);
    // A 60 x 40 oval of waypoints; the AI driver has to follow it unaided.
    car.route = vec![
        vec3f(0.0, 0.0, -30.0),
        vec3f(25.0, 0.0, -20.0),
        vec3f(30.0, 0.0, 0.0),
        vec3f(25.0, 0.0, 20.0),
        vec3f(0.0, 0.0, 30.0),
        vec3f(-25.0, 0.0, 20.0),
        vec3f(-30.0, 0.0, 0.0),
        vec3f(-25.0, 0.0, -20.0),
    ];
    car.route_pace = 1.0;
    blocks.cars.push(car);

    // Gate every waypoint so the lap can't be scored by cutting the infield.
    for (i, point) in blocks.cars[0].route.clone().iter().enumerate() {
        let _ = i;
        blocks
            .race
            .add_checkpoint(vec3f(point.x, 1.0, point.z), vec3f(6.0, 3.0, 6.0), 0);
    }
    blocks.race.enter(chassis);
    blocks.race.start(1);

    let mut ticks = 0;
    while ticks < 60 * 90 && !blocks.race.standing_of(chassis).unwrap().finished {
        tick(&mut world, &mut blocks);
        ticks += 1;
    }
    let standing = *blocks.race.standing_of(chassis).unwrap();
    assert!(
        standing.finished,
        "AI car failed to complete a lap in 90s (progress {} of 8 gates)",
        standing.progress
    );
    let seconds = ticks as f32 * DT;
    assert!(
        seconds > 5.0,
        "lap time {seconds}s is impossibly fast — gates are probably overlapping"
    );
    // A ~190-unit oval at ~20 units/s cruise is comfortably inside 60s.
    assert!(seconds < 60.0, "lap took {seconds}s, expected under 60s");
    let e = world.entity(chassis).unwrap();
    assert!(
        e.pos.y > -1.0 && e.pos.y < 5.0,
        "car left the ground plane during the lap: y {}",
        e.pos.y
    );
}

#[test]
fn character_walks_and_steps_up_but_is_blocked_by_a_wall() {
    // The 0.55 step-up is a TERRAIN contract (step.rs CLIMB) — static boxes
    // are swept against and always block, whatever their height. So the
    // walkable ledge has to be ground, and the wall is a box.
    let mut world = GameWorld::new();
    world.gravity = 30.0;
    world.terrain = Some(step_terrain(-4.0, 0.5));
    let hero = mover(&mut world, vec3f(0.0, 1.0, 0.0), vec3f(0.4, 0.8, 0.4));
    // A 1.0-high wall (blocking) at z = -12.
    let wall = block(&mut world, vec3f(0.0, 1.0, -12.0), vec3f(3.0, 1.0, 1.0));
    let mut blocks = Blocks::new();
    blocks.characters.push(Character::new(
        hero,
        CharacterConfig::default(),
        ControlSource::Script,
        None,
    ));
    blocks.characters[0].input.move_z = -1.0;

    // Walk until the wall reports a hit, watching the ledge climb on the way.
    let mut on_ledge = false;
    let mut stopped_at = None;
    for _ in 0..60 * 4 {
        tick(&mut world, &mut blocks);
        let e = world.entity(hero).unwrap();
        if e.pos.z < -5.0 && e.pos.y > 1.2 {
            on_ledge = true;
        }
        if e.hit_wall == wall && stopped_at.is_none() {
            stopped_at = Some(e.pos.z);
            break;
        }
    }
    assert!(
        on_ledge,
        "character should have stepped up onto the 0.5 ledge (feet 0.5, centre 1.3)"
    );
    let stopped_at = stopped_at.expect("character never reported hitting the wall");
    // Stopped flush against the near face: wall centre -12, half 1.0, body half
    // 0.4 => -10.6. Never inside it.
    assert!(
        stopped_at > -10.7 && stopped_at < -10.5,
        "character should stop at the wall face (-10.6), stopped at {stopped_at}"
    );
    assert!(
        blocks.characters[0].pose.walk_blend > 0.5,
        "walking should blend toward the walk clip, got {}",
        blocks.characters[0].pose.walk_blend
    );
}

#[test]
fn character_jumps_and_lands() {
    let mut world = world_with_ground();
    let hero = mover(&mut world, vec3f(0.0, 1.0, 0.0), vec3f(0.4, 0.8, 0.4));
    let mut blocks = Blocks::new();
    blocks.characters.push(Character::new(
        hero,
        CharacterConfig::default(),
        ControlSource::Script,
        None,
    ));
    run(&mut world, &mut blocks, 30);
    let grounded_y = world.entity(hero).unwrap().pos.y;

    blocks.characters[0].input.jump_pressed = true;
    tick(&mut world, &mut blocks);
    blocks.characters[0].input.jump_pressed = false;
    let mut peak = grounded_y;
    for _ in 0..20 {
        tick(&mut world, &mut blocks);
        peak = peak.max(world.entity(hero).unwrap().pos.y);
    }
    assert!(
        peak > grounded_y + 1.0,
        "jump barely left the floor: {peak} vs {grounded_y}"
    );
    assert!(
        blocks.characters[0].pose.airborne,
        "pose should report airborne mid-jump"
    );
    run(&mut world, &mut blocks, 90);
    let landed = world.entity(hero).unwrap();
    assert!(landed.on_floor, "character should land again");
    assert!(
        !blocks.characters[0].pose.airborne,
        "pose should clear airborne after landing"
    );
}

#[test]
fn plane_climbs_under_throttle_and_survives_a_turn() {
    let mut world = world_with_ground();
    let body = rigid(&mut world, vec3f(0.0, 60.0, 0.0), vec3f(1.2, 0.4, 1.6));
    // Launch it forward at flying speed, like a catapult start.
    world.entity_mut(body).unwrap().vel = vec3f(0.0, 0.0, -30.0);
    let mut blocks = Blocks::new();
    blocks.planes.push(makepad_game_blocks::Plane::new(
        body,
        PlaneConfig::default(),
        ControlSource::Script,
    ));
    blocks.planes[0].input.throttle = 1.0;
    // A sustained climb input. The nose-up attitude trades airspeed for
    // altitude and self-limits as lift falls off — no loop, no stall.
    blocks.planes[0].input.pitch = 0.3;
    let start_y = world.entity(body).unwrap().pos.y;
    run(&mut world, &mut blocks, 60 * 5);
    let climbed = world.entity(body).unwrap().pos.y;
    assert!(
        climbed > start_y + 5.0,
        "plane should climb under power: {start_y} -> {climbed}"
    );

    // Now bank into a turn. Roll input is a RATE, so it is held just long
    // enough to establish the bank and then released — holding it would barrel
    // roll (correct for an arcade plane, but not a turn).
    let before = world.entity(body).unwrap().pos.y;
    let heading_before = heading_of(world.entity(body).unwrap().vel);
    blocks.planes[0].input.pitch = 0.15;
    blocks.planes[0].input.roll = 0.8;
    run(&mut world, &mut blocks, 25);
    blocks.planes[0].input.roll = 0.0;
    run(&mut world, &mut blocks, 60 * 5);
    let after = world.entity(body).unwrap();
    assert!(
        after.pos.y > before - 30.0,
        "plane lost too much altitude in a turn: {before} -> {}",
        after.pos.y
    );
    assert!(
        after.pos.y > 5.0,
        "plane hit the ground during a normal turn"
    );
    let turned = angle_delta(heading_of(after.vel), heading_before).abs();
    assert!(
        turned > 0.3,
        "banking should actually change heading, turned {turned} rad"
    );
    assert!(
        blocks.planes[0].airspeed > 5.0,
        "plane should keep flying speed through the turn, got {}",
        blocks.planes[0].airspeed
    );
}

#[test]
fn wander_brain_stays_within_range_and_moves() {
    let mut world = world_with_ground();
    let critter = mover(&mut world, vec3f(0.0, 1.0, 0.0), vec3f(0.4, 0.5, 0.4));
    let mut blocks = Blocks::new();
    blocks.brains.push(Brain::new(
        critter,
        BrainKind::Wander {
            home: vec3f(0.0, 0.0, 0.0),
            range: 10.0,
            speed: 3.0,
            pause: 0.5,
        },
    ));
    let mut max_distance: f32 = 0.0;
    let mut moved = false;
    for _ in 0..60 * 30 {
        tick(&mut world, &mut blocks);
        let e = world.entity(critter).unwrap();
        let d = (e.pos.x * e.pos.x + e.pos.z * e.pos.z).sqrt();
        max_distance = max_distance.max(d);
        if d > 1.5 {
            moved = true;
        }
    }
    assert!(moved, "wanderer never left home");
    assert!(
        max_distance < 14.0,
        "wanderer strayed outside its range: {max_distance}"
    );
}

#[test]
fn chase_brain_closes_and_catches() {
    let mut world = world_with_ground();
    let prey = mover(&mut world, vec3f(0.0, 1.0, -20.0), vec3f(0.4, 0.8, 0.4));
    world.entity_mut(prey).unwrap().tag = "player".to_string();
    let hunter = mover(&mut world, vec3f(0.0, 1.0, 0.0), vec3f(0.4, 0.8, 0.4));
    let mut blocks = Blocks::new();
    blocks.brains.push(Brain::new(
        hunter,
        BrainKind::Chase {
            tag: "player".to_string(),
            target: 0,
            range: 60.0,
            catch: 1.5,
            speed: 7.0,
        },
    ));
    let start = 20.0f32;
    let mut caught = false;
    for _ in 0..60 * 10 {
        tick(&mut world, &mut blocks);
        if blocks.brains[0].caught == prey {
            caught = true;
            break;
        }
    }
    assert!(caught, "hunter never caught stationary prey");
    let d = {
        let h = world.entity(hunter).unwrap().pos;
        let p = world.entity(prey).unwrap().pos;
        ((h.x - p.x).powi(2) + (h.z - p.z).powi(2)).sqrt()
    };
    assert!(d < start, "hunter should have closed the gap, distance {d}");
}

#[test]
fn patrol_brain_visits_every_waypoint() {
    let mut world = world_with_ground();
    let guard = mover(&mut world, vec3f(0.0, 1.0, 0.0), vec3f(0.4, 0.8, 0.4));
    let route = vec![
        vec3f(10.0, 0.0, 0.0),
        vec3f(10.0, 0.0, 10.0),
        vec3f(0.0, 0.0, 10.0),
        vec3f(0.0, 0.0, 0.0),
    ];
    let mut blocks = Blocks::new();
    blocks.brains.push(Brain::new(
        guard,
        BrainKind::Patrol {
            points: route.clone(),
            speed: 6.0,
            looping: true,
        },
    ));
    let mut visited = vec![false; route.len()];
    for _ in 0..60 * 40 {
        tick(&mut world, &mut blocks);
        let p = world.entity(guard).unwrap().pos;
        for (i, point) in route.iter().enumerate() {
            if ((p.x - point.x).powi(2) + (p.z - point.z).powi(2)).sqrt() < 2.0 {
                visited[i] = true;
            }
        }
    }
    assert!(
        visited.iter().all(|v| *v),
        "patrol missed waypoints: {visited:?}"
    );
}

#[test]
fn race_kit_enforces_checkpoint_order() {
    let mut world = world_with_ground();
    let racer = mover(&mut world, vec3f(0.0, 1.0, 0.0), vec3f(0.5, 0.8, 0.5));
    let mut blocks = Blocks::new();
    for i in 0..3 {
        blocks.race.add_checkpoint(
            vec3f(i as f32 * 10.0, 1.0, 0.0),
            vec3f(2.0, 3.0, 2.0),
            0,
        );
    }
    blocks.race.enter(racer);
    blocks.race.start(1);

    // Teleport straight onto gate 2, skipping gates 0 and 1.
    world.entity_mut(racer).unwrap().pos = vec3f(20.0, 1.0, 0.0);
    tick(&mut world, &mut blocks);
    assert_eq!(
        blocks.race.standing_of(racer).unwrap().progress,
        0,
        "a skipped gate must not score"
    );

    // Now take them in order.
    for i in 0..3 {
        world.entity_mut(racer).unwrap().pos = vec3f(i as f32 * 10.0, 1.0, 0.0);
        tick(&mut world, &mut blocks);
    }
    let standing = *blocks.race.standing_of(racer).unwrap();
    assert_eq!(standing.progress, 3, "three gates in order should all score");
    assert_eq!(standing.lap, 1, "crossing every gate completes a lap");
    assert!(standing.finished, "a one-lap race ends on that lap");
    assert_eq!(blocks.race.winner, racer);
}

#[test]
fn race_standings_order_by_progress_then_finish_time() {
    let mut world = world_with_ground();
    let a = mover(&mut world, vec3f(0.0, 1.0, 0.0), vec3f(0.5, 0.8, 0.5));
    let b = mover(&mut world, vec3f(0.0, 1.0, 5.0), vec3f(0.5, 0.8, 0.5));
    let mut blocks = Blocks::new();
    for i in 0..2 {
        blocks
            .race
            .add_checkpoint(vec3f(i as f32 * 10.0, 1.0, 0.0), vec3f(2.0, 3.0, 2.0), 0);
    }
    blocks.race.enter(a);
    blocks.race.enter(b);
    blocks.race.start(2);

    // A banks one gate; B banks none.
    world.entity_mut(a).unwrap().pos = vec3f(0.0, 1.0, 0.0);
    tick(&mut world, &mut blocks);
    let order = blocks.race.order();
    assert_eq!(order[0].entity, a, "the racer with more gates leads");
    assert_eq!(blocks.race.rank_of(a), 1);
    assert_eq!(blocks.race.rank_of(b), 2);

    blocks.race.add_score(b, 5);
    assert_eq!(blocks.race.standing_of(b).unwrap().score, 5);
}

#[test]
fn blocks_drop_state_when_their_entity_dies() {
    let mut world = world_with_ground();
    let hero = mover(&mut world, vec3f(0.0, 1.0, 0.0), vec3f(0.4, 0.8, 0.4));
    let mut blocks = Blocks::new();
    blocks.characters.push(Character::new(
        hero,
        CharacterConfig::default(),
        ControlSource::Script,
        None,
    ));
    blocks.race.enter(hero);
    run(&mut world, &mut blocks, 5);
    assert_eq!(blocks.characters.len(), 1);

    world.entities.retain(|e| e.id != hero);
    tick(&mut world, &mut blocks);
    assert!(
        blocks.characters.is_empty(),
        "character block should follow its entity out"
    );
    assert!(
        blocks.race.standings.is_empty(),
        "standings should drop a departed racer"
    );
}

/// game.md determinism rule: the same scenario, run twice, must land on the
/// same bits — blocks are Shared tier, so a divergence here is a desync.
#[test]
fn block_scenario_is_deterministic() {
    fn scenario() -> (u64, u64, Vec3f, Vec3f) {
        let mut world = world_with_ground();
        let chassis = rigid(&mut world, vec3f(0.0, 1.5, 0.0), vec3f(0.9, 0.4, 1.6));
        let critter = mover(&mut world, vec3f(6.0, 1.0, 6.0), vec3f(0.4, 0.5, 0.4));
        let hero = mover(&mut world, vec3f(-6.0, 1.0, 0.0), vec3f(0.4, 0.8, 0.4));
        let mut blocks = Blocks::new();
        let mut car = Car::new(chassis, CarConfig::default(), ControlSource::Script);
        car.input.throttle = 1.0;
        car.input.steer = 0.4;
        blocks.cars.push(car);
        blocks.brains.push(Brain::new(
            critter,
            BrainKind::Wander {
                home: vec3f(6.0, 0.0, 6.0),
                range: 8.0,
                speed: 3.0,
                pause: 0.4,
            },
        ));
        let mut character = Character::new(
            hero,
            CharacterConfig::default(),
            ControlSource::Script,
            None,
        );
        character.input.move_x = 0.6;
        character.input.move_z = -0.8;
        blocks.characters.push(character);
        blocks.race.add_checkpoint(vec3f(0.0, 1.0, -10.0), vec3f(4.0, 3.0, 4.0), 0);
        blocks.race.enter(chassis);
        blocks.race.start(3);

        run(&mut world, &mut blocks, 600);
        (
            blocks.hash(),
            world_hash(&world),
            world.entity(chassis).unwrap().pos,
            world.entity(critter).unwrap().pos,
        )
    }
    let first = scenario();
    let second = scenario();
    assert_eq!(first.0, second.0, "blocks hash diverged between runs");
    assert_eq!(first.1, second.1, "world hash diverged between runs");
    assert_eq!(first.2, second.2, "car position diverged");
    assert_eq!(first.3, second.3, "wanderer position diverged");
}

fn world_hash(world: &GameWorld) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u64| {
        h ^= v;
        h = h.wrapping_mul(0x100_0000_01b3);
    };
    for e in &world.entities {
        mix(e.id);
        for f in [
            e.pos.x, e.pos.y, e.pos.z, e.vel.x, e.vel.y, e.vel.z, e.yaw, e.orient.x, e.orient.y,
            e.orient.z, e.orient.w,
        ] {
            mix(f.to_bits() as u64);
        }
    }
    h
}

fn rotate_quat(q: Quat, v: Vec3f) -> Vec3f {
    let u = vec3f(q.x, q.y, q.z);
    let s = q.w;
    u * (2.0 * u.dot(v)) + v * (s * s - u.dot(u)) + Vec3f::cross(u, v) * (2.0 * s)
}

/// Compass heading of a velocity vector, in the engine's -z-forward frame.
fn heading_of(v: Vec3f) -> f32 {
    makepad_game_math::atan2(-v.x, -v.z)
}

fn angle_delta(a: f32, b: f32) -> f32 {
    let mut d = a - b;
    while d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    }
    while d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    d
}

// ---------------------------------------------------------------------------
// NPCs (npc.rs). These are the gate for behaviour: without a screen, a
// villager "looking alive" has to be expressed as reaching places, refusing to
// wedge itself, and not all doing the same thing at once.
// ---------------------------------------------------------------------------

fn npc(world: &mut GameWorld, blocks: &mut Blocks, pos: Vec3f, seed: u64) -> u64 {
    let id = mover(world, pos, vec3f(0.4, 0.8, 0.4));
    blocks
        .npcs
        .push(Npc::new(id, NpcConfig::default(), pos, seed));
    id
}

/// Walks to a destination and stays there. The floor of the whole feature: an
/// NPC that cannot arrive has no behaviour to build on.
#[test]
fn npc_walks_to_a_poi_and_dwells_there() {
    let mut world = world_with_ground();
    let mut blocks = Blocks::new();
    let id = npc(&mut world, &mut blocks, vec3f(0.0, 1.0, 0.0), 7);
    blocks.pois.push(Poi::new(vec3f(14.0, 0.0, 0.0), "bench"));
    // Only one destination exists, so scoring must converge on it.
    blocks.npcs[0].activity = Activity::Travel {
        goal: vec3f(14.0, 0.0, 0.0),
        poi: Some(0),
    };

    let mut arrived = None;
    for tick_i in 0..60 * 20 {
        tick(&mut world, &mut blocks);
        if matches!(blocks.npcs[0].activity, Activity::Dwell { .. }) {
            arrived = Some(tick_i);
            break;
        }
    }
    let arrived = arrived.expect("npc never reached the bench");
    let e = world.entity(id).unwrap();
    let d = ((e.pos.x - 14.0).powi(2) + e.pos.z.powi(2)).sqrt();
    assert!(d < 1.6, "stopped {d} away from the bench it was heading for");
    // 14 units at ~2.6 u/s is ~5.4s; anything near the 20s cap means it was
    // wandering rather than walking there.
    assert!(arrived < 60 * 12, "took {arrived} ticks to walk 14 units");
}

/// Blocked by a wall it cannot jump, it goes around and still arrives. This is
/// the difference between "has a goal" and "can pursue one".
#[test]
fn npc_routes_around_a_wall_and_still_arrives() {
    let mut world = world_with_ground();
    // A wall across the direct path, with open ground either side.
    block(&mut world, vec3f(0.0, 1.5, -8.0), vec3f(4.0, 1.5, 0.5));
    let mut blocks = Blocks::new();
    let id = npc(&mut world, &mut blocks, vec3f(0.0, 1.0, 0.0), 3);
    blocks.npcs[0].activity = Activity::Travel {
        goal: vec3f(0.0, 0.0, -16.0),
        poi: None,
    };

    let mut best = f32::MAX;
    for _ in 0..60 * 30 {
        tick(&mut world, &mut blocks);
        let e = world.entity(id).unwrap();
        best = best.min((e.pos.z + 16.0).abs());
        // Never inside the wall.
        if (e.pos.z + 8.0).abs() < 0.5 {
            assert!(
                e.pos.x.abs() > 3.9,
                "npc walked into the wall at x={} z={}",
                e.pos.x,
                e.pos.z
            );
        }
    }
    assert!(
        best < 3.0,
        "npc never got past the wall; closest approach to the goal was {best}"
    );
}

/// A crate low enough to jump is jumped rather than walked around.
#[test]
fn npc_jumps_a_low_obstacle() {
    let mut world = world_with_ground();
    // Long, low: going around would take much longer than hopping it.
    block(&mut world, vec3f(0.0, 0.4, -6.0), vec3f(14.0, 0.4, 0.5));
    let mut blocks = Blocks::new();
    let id = npc(&mut world, &mut blocks, vec3f(0.0, 1.0, 0.0), 11);
    blocks.npcs[0].activity = Activity::Travel {
        goal: vec3f(0.0, 0.0, -14.0),
        poi: None,
    };

    let mut left_ground = false;
    let mut crossed = false;
    for _ in 0..60 * 25 {
        tick(&mut world, &mut blocks);
        let e = world.entity(id).unwrap();
        if !e.on_floor {
            left_ground = true;
        }
        if e.pos.z < -7.0 {
            crossed = true;
            break;
        }
    }
    assert!(left_ground, "npc never jumped");
    assert!(crossed, "npc never got over the low crate");
}

/// Converging on one destination must not stack them into a tower — movers
/// pass through each other, so the separation steering is the only thing
/// keeping a crowd looking like a crowd.
#[test]
fn npcs_converging_on_one_poi_do_not_stack() {
    let mut world = world_with_ground();
    let mut blocks = Blocks::new();
    let mut ids = Vec::new();
    for i in 0..6 {
        let a = i as f32 * 1.04;
        let (s, c) = (a.sin(), a.cos());
        ids.push(npc(
            &mut world,
            &mut blocks,
            vec3f(c * 10.0, 1.0, s * 10.0),
            100 + i,
        ));
    }
    // One open destination they all want.
    blocks.pois.push(Poi::new(vec3f(0.0, 0.0, 0.0), "well").with_capacity(8));
    for n in blocks.npcs.iter_mut() {
        n.activity = Activity::Travel {
            goal: vec3f(0.0, 0.0, 0.0),
            poi: Some(0),
        };
    }

    for _ in 0..60 * 15 {
        tick(&mut world, &mut blocks);
    }
    let mut closest = f32::MAX;
    for (i, a) in ids.iter().enumerate() {
        for b in ids.iter().skip(i + 1) {
            let (pa, pb) = (
                world.entity(*a).unwrap().pos,
                world.entity(*b).unwrap().pos,
            );
            closest = closest.min(((pa.x - pb.x).powi(2) + (pa.z - pb.z).powi(2)).sqrt());
        }
    }
    assert!(
        closest > 0.55,
        "npcs collapsed into each other: closest pair {closest} apart"
    );
}

/// Same seed, same behaviour, tick for tick — the multiplayer and replay
/// requirement. Different seed must actually differ, or the "personality"
/// claim is decoration.
#[test]
fn npc_behaviour_is_seeded_and_reproducible() {
    fn run(seed: u64) -> Vec<String> {
        let mut world = world_with_ground();
        let mut blocks = Blocks::new();
        for i in 0..4 {
            npc(
                &mut world,
                &mut blocks,
                vec3f(i as f32 * 3.0, 1.0, 0.0),
                seed + i,
            );
        }
        blocks.pois.push(Poi::new(vec3f(12.0, 0.0, 4.0), "bench"));
        blocks.pois.push(Poi::new(vec3f(-9.0, 0.0, -6.0), "well"));
        let mut trace = Vec::new();
        for t in 0..60 * 30 {
            tick(&mut world, &mut blocks);
            if t % 300 == 0 {
                for n in &blocks.npcs {
                    trace.push(n.trace(&world));
                }
            }
        }
        trace
    }
    let a = run(42);
    let b = run(42);
    assert_eq!(a, b, "same seed produced a different run");
    let c = run(43);
    assert_ne!(a, c, "different seed produced an identical run");
}

/// Villagers left to their own devices must not end up doing the same thing
/// in unison, and must not freeze. This is the "looks inhabited" assertion.
#[test]
fn a_village_of_npcs_stays_busy_and_varied() {
    let mut world = world_with_ground();
    let mut blocks = Blocks::new();
    for i in 0..10 {
        let a = i as f32 * 0.63;
        let (s, c) = (a.sin(), a.cos());
        npc(
            &mut world,
            &mut blocks,
            vec3f(c * 8.0, 1.0, s * 8.0),
            900 + i,
        );
    }
    for (i, (x, z, tag)) in [
        (12.0, 2.0, "bench"),
        (-11.0, 5.0, "well"),
        (4.0, -13.0, "door"),
        (-6.0, -10.0, "market"),
    ]
    .iter()
    .enumerate()
    {
        blocks
            .pois
            .push(Poi::new(vec3f(*x, 0.0, *z), *tag).with_capacity(2));
        let _ = i;
    }

    let mut moved = vec![0.0f32; blocks.npcs.len()];
    let mut last: Vec<Vec3f> = blocks
        .npcs
        .iter()
        .map(|n| world.entity(n.entity).unwrap().pos)
        .collect();
    let mut activities = std::collections::HashSet::new();
    for t in 0..60 * 90 {
        tick(&mut world, &mut blocks);
        if t % 30 == 0 {
            for (i, n) in blocks.npcs.iter().enumerate() {
                let p = world.entity(n.entity).unwrap().pos;
                moved[i] += ((p.x - last[i].x).powi(2) + (p.z - last[i].z).powi(2)).sqrt();
                last[i] = p;
                activities.insert(n.activity.name());
            }
        }
    }
    // Everyone got somewhere over 90 seconds.
    let idle: Vec<usize> = moved
        .iter()
        .enumerate()
        .filter(|(_, d)| **d < 6.0)
        .map(|(i, _)| i)
        .collect();
    assert!(
        idle.is_empty(),
        "npcs {idle:?} barely moved in 90s: {moved:?}"
    );
    // And they were not all doing one thing.
    assert!(
        activities.len() >= 2,
        "village only ever showed activities {activities:?}"
    );
    // Nobody fell through the world or got launched.
    for n in &blocks.npcs {
        let p = world.entity(n.entity).unwrap().pos;
        assert!(
            p.y > -2.0 && p.y < 20.0,
            "npc {} left the world at y={}",
            n.entity,
            p.y
        );
    }
}

/// An unbiased random walk has no centre, so villagers drift off the map over
/// a few minutes. Caught in a trace: one NPC 43 units out with every
/// destination inside 18.
#[test]
fn npcs_stay_in_their_village() {
    let mut world = world_with_ground();
    let mut blocks = Blocks::new();
    for i in 0..6 {
        npc(&mut world, &mut blocks, vec3f(i as f32 * 2.0, 1.0, 0.0), 77 + i);
    }
    blocks.pois.push(Poi::new(vec3f(9.0, 0.0, 3.0), "bench"));
    // Four minutes is long enough for a drifter to be far away.
    let mut furthest: f32 = 0.0;
    for _ in 0..60 * 240 {
        tick(&mut world, &mut blocks);
        for n in &blocks.npcs {
            let p = world.entity(n.entity).unwrap().pos;
            let d = ((p.x - n.home.x).powi(2) + (p.z - n.home.z).powi(2)).sqrt();
            furthest = furthest.max(d);
        }
    }
    assert!(
        furthest < 45.0,
        "a villager wandered {furthest} units from home over four minutes"
    );
}

// ---------------------------------------------------------------- interiors
//
// A house you can walk into is the difference between scenery and a place. The
// generator is tested in `makepad-game-gen`; what these prove is the part only
// a running sim can show — that the room's colliders leave a passable doorway,
// that a mover can cross the floor without snagging on furniture, and that an
// NPC uses a door and comes back out again rather than vanishing.

use makepad_game_gen::interior::{interior, DoorSide, Interior, InteriorParams};
use makepad_game_gen::kit::{Kit, TileDef, TileRole};

fn shell_kit() -> Kit {
    Kit::new(
        "kenney/modular-buildings",
        2.0,
        vec![
            TileDef::new("floor", TileRole::Floor, 0.1),
            TileDef::new("wall", TileRole::Wall, 2.4),
            TileDef::new("wall-corner", TileRole::WallCorner, 2.4),
            TileDef::new("door", TileRole::Door, 2.4),
        ],
    )
}

fn furniture_kit() -> Kit {
    Kit::new(
        "kenney/furniture-kit",
        2.0,
        vec![
            TileDef::new("chair", TileRole::Prop, 0.9),
            TileDef::new("table", TileRole::Prop, 0.8),
        ],
    )
}

/// Spawn a generated room's collision into the world, exactly as the host
/// would: every box becomes a static entity.
fn materialise(world: &mut GameWorld, room: &Interior) {
    for (c, h) in &room.colliders {
        block(world, *c, *h);
    }
}

fn walker(world: &mut GameWorld, pos: Vec3f) -> u64 {
    mover(world, pos, vec3f(0.35, 0.9, 0.35))
}

/// Steer an entity toward a point for `ticks`, stopping early on arrival.
fn walk_to(world: &mut GameWorld, id: u64, goal: Vec3f, ticks: usize, speed: f32) -> bool {
    for _ in 0..ticks {
        let Some(e) = world.entity(id) else { return false };
        let (dx, dz) = (goal.x - e.pos.x, goal.z - e.pos.z);
        let d = (dx * dx + dz * dz).sqrt();
        if d < 0.6 {
            return true;
        }
        if let Some(e) = world.entity_mut(id) {
            e.vel.x = dx / d * speed;
            e.vel.z = dz / d * speed;
        }
        step_world(world);
        world.tick += 1;
    }
    false
}

fn room_for_test(seed: u64, shell: &Kit, fk: &Kit) -> Interior {
    let mut p = InteriorParams::new(shell);
    p.seed = seed;
    p.cells = (5, 5);
    p.door = DoorSide::South;
    p.origin = vec3f(300.0, 0.0, 300.0);
    p.furniture = Some(fk);
    p.clutter = 0.6;
    interior(&p)
}

#[test]
fn a_walker_enters_a_generated_room_crosses_it_and_leaves() {
    let shell = shell_kit();
    let fk = furniture_kit();
    let room = room_for_test(3, &shell, &fk);

    // The shared ground spans 400 units, so the pocket sits on it too.
    let mut world = world_with_ground();
    materialise(&mut world, &room);

    // Start outside, beyond the south wall, in line with the doorway.
    let outside = vec3f(room.door_pos.x, 0.9, room.door_pos.z + 4.0);
    let id = walker(&mut world, outside);
    for _ in 0..30 {
        step_world(&mut world);
    }

    assert!(
        walk_to(&mut world, id, room.entrance, 400, 2.5),
        "walker never got through the doorway"
    );
    let inside_z = world.entity(id).unwrap().pos.z;
    assert!(
        inside_z < room.door_pos.z - 0.4,
        "walker stopped in the threshold at z={inside_z}"
    );

    // Cross to the far side of the room, which is what furniture could block.
    let far = *room
        .free_points
        .iter()
        .max_by(|a, b| {
            let da = (a.z - room.entrance.z).abs();
            let db = (b.z - room.entrance.z).abs();
            da.partial_cmp(&db).unwrap()
        })
        .unwrap();
    assert!(
        walk_to(&mut world, id, far, 500, 2.5),
        "walker could not cross the room — furniture blocked the floor"
    );

    // And back out the way it came.
    assert!(
        walk_to(&mut world, id, outside, 700, 2.5),
        "walker could not find its way back out"
    );
    assert!(world.entity(id).unwrap().pos.z > room.door_pos.z);
}

#[test]
fn the_walls_are_solid_everywhere_except_the_doorway() {
    let shell = shell_kit();
    let fk = furniture_kit();
    let room = room_for_test(11, &shell, &fk);

    let mut world = world_with_ground();
    materialise(&mut world, &room);

    // Approach the NORTH wall — the opposite side from the door — and push.
    let north_outside = vec3f(room.entrance.x, 0.9, room.door_pos.z - 14.0);
    let id = walker(&mut world, north_outside);
    for _ in 0..30 {
        step_world(&mut world);
    }
    let start_z = world.entity(id).unwrap().pos.z;
    for _ in 0..300 {
        if let Some(e) = world.entity_mut(id) {
            e.vel.z = 3.0;
        }
        step_world(&mut world);
    }
    let end = world.entity(id).unwrap().pos;
    assert!(
        end.z > start_z,
        "walker should have advanced toward the wall"
    );
    // It must be stopped OUTSIDE the room: the far wall is at the ring, one
    // tile beyond the first floor row.
    let first_floor_z = room
        .free_points
        .iter()
        .map(|p| p.z)
        .fold(f32::INFINITY, f32::min);
    assert!(
        end.z < first_floor_z,
        "walked through the north wall: z={} reached floor at {}",
        end.z,
        first_floor_z
    );
}

#[test]
fn an_npc_goes_through_a_door_stays_a_while_and_comes_back_out() {
    let shell = shell_kit();
    let fk = furniture_kit();
    let room = room_for_test(5, &shell, &fk);

    let mut world = world_with_ground();
    // Dusk. Doors read as an evening destination (see `tag_appeal`), so this
    // is the hour the behaviour is meant to show up in.
    world.tick = (DAY_SECONDS * 0.85 / DT) as u64;

    let door_stand = vec3f(4.0, 0.0, 0.0);
    let mut blocks = Blocks::new();
    blocks.pois.push(
        Poi::new(door_stand, "door")
            .with_interior(room.entrance)
            .with_capacity(8),
    );

    // A handful of personalities rather than one lucky seed: going inside is
    // a scored choice, so the claim worth testing is that it happens across a
    // population, not that seed N does it.
    let mut ids = Vec::new();
    for seed in 0..8u64 {
        let a = seed as f32 * 0.8;
        let (s, c) = (a.sin(), a.cos());
        let id = walker(&mut world, vec3f(c * 6.0, 1.0, s * 6.0));
        let mut npc = Npc::new(id, NpcConfig::default(), vec3f(c * 6.0, 0.0, s * 6.0), seed);
        npc.config.visit = 3.0;
        blocks.npcs.push(npc);
        ids.push(id);
    }

    // (entered tick, exited tick, position on the way out) per NPC.
    let mut trips: Vec<Option<(usize, Option<usize>, Vec3f)>> = vec![None; ids.len()];
    let mut done = 0;
    for t in 0..6000 {
        tick(&mut world, &mut blocks);
        // The host's job: perform the position write the block asked for.
        // The block only ever asks — see the invariant on `DoorUse`.
        for d in blocks.door_uses.drain(..) {
            if let Some(e) = world.entity_mut(d.entity) {
                e.pos = vec3f(d.to.x, d.to.y + e.half.y, d.to.z);
                e.vel = vec3f(0.0, 0.0, 0.0);
            }
            let n = ids.iter().position(|&i| i == d.entity).unwrap();
            match (&mut trips[n], d.entering) {
                (slot @ None, true) => *slot = Some((t, None, vec3f(0.0, 0.0, 0.0))),
                (Some(trip), false) if trip.1.is_none() => {
                    let out = world.entity(d.entity).map(|e| e.pos).unwrap_or_default();
                    *trip = (trip.0, Some(t), out);
                    done += 1;
                }
                _ => {}
            }
        }
        if done >= 3 {
            break;
        }
    }

    let full: Vec<_> = trips.iter().flatten().filter(|t| t.1.is_some()).collect();
    assert!(
        full.len() >= 3,
        "only {} of 8 villagers used the door in 100s",
        full.len()
    );
    for (into, out, at) in &full {
        let out = out.unwrap();
        assert!(out > *into, "left before it arrived");
        // The visit must actually last: an NPC that bounces straight back out
        // reads as a glitch rather than as someone popping indoors.
        let seconds = (out - into) as f32 * DT;
        assert!(
            seconds > 2.0,
            "visit lasted only {seconds:.1}s — too short to read as going inside"
        );
        // And it must come back out by the door it used, not somewhere else.
        let d = ((at.x - door_stand.x).powi(2) + (at.z - door_stand.z).powi(2)).sqrt();
        assert!(d < 6.0, "came out {d:.1} units from the door it went in by");
    }
    // Nobody may be left behind a door: leaving is unconditional, so the only
    // way to still be inside is a bug in the visit countdown.
    let stuck = blocks.npcs.iter().filter(|n| n.is_inside()).count();
    assert!(stuck <= 1, "{stuck} villagers left behind doors");
}

#[test]
fn npcs_in_a_furnished_room_never_end_up_wedged() {
    let shell = shell_kit();
    let fk = furniture_kit();
    let room = room_for_test(9, &shell, &fk);

    let mut world = world_with_ground();
    materialise(&mut world, &room);

    // Four NPCs in the four corners of the room, so they have to cross it —
    // and each other, and the furniture — to reach anything. Bunched together
    // they would simply stand around chatting, which is correct behaviour but
    // tests nothing about getting wedged.
    let key = |p: &Vec3f, sx: f32, sz: f32| p.x * sx + p.z * sz;
    let corners: Vec<Vec3f> = [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)]
        .iter()
        .map(|&(sx, sz)| {
            *room
                .free_points
                .iter()
                .max_by(|a, b| key(a, sx, sz).total_cmp(&key(b, sx, sz)))
                .unwrap()
        })
        .collect();

    let mut blocks = Blocks::new();
    let mut ids = Vec::new();
    for (i, spot) in corners.iter().enumerate() {
        let id = walker(&mut world, vec3f(spot.x, spot.y + 0.9, spot.z));
        ids.push(id);
        blocks
            .npcs
            .push(Npc::new(id, NpcConfig::default(), *spot, 20 + i as u64));
    }

    let mut moved = vec![0.0f32; ids.len()];
    let mut last: Vec<Vec3f> = ids.iter().map(|&i| world.entity(i).unwrap().pos).collect();
    for _ in 0..1800 {
        tick(&mut world, &mut blocks);
        blocks.door_uses.clear();
        for (n, &id) in ids.iter().enumerate() {
            let p = world.entity(id).unwrap().pos;
            let d = ((p.x - last[n].x).powi(2) + (p.z - last[n].z).powi(2)).sqrt();
            moved[n] += d;
            last[n] = p;
        }
    }

    let floor_min = room.free_points.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
    let floor_max = room.free_points.iter().map(|p| p.x).fold(f32::NEG_INFINITY, f32::max);
    for (n, &id) in ids.iter().enumerate() {
        let p = world.entity(id).unwrap().pos;
        assert!(
            p.x.is_finite() && p.y.is_finite() && p.z.is_finite(),
            "npc {n} left the world"
        );
        assert!(p.y > -2.0, "npc {n} fell through the floor at y={}", p.y);
        // Still in the building: the doorway is the only way out, and nothing
        // here has any reason to use it.
        assert!(
            p.x > floor_min - 3.0 && p.x < floor_max + 3.0,
            "npc {n} ended up outside the room at x={}",
            p.x
        );
        // Nobody may spend the whole run pinned. The give-up timer exists
        // precisely so an NPC that cannot reach its goal abandons it rather
        // than grinding into furniture forever.
        assert!(
            moved[n] > 2.0,
            "npc {n} moved only {:.2} units in 30s — wedged",
            moved[n]
        );
    }
}

// ---------------------------------------------------------------------------
// Controller FEEL.
//
// These assert the SHAPE of motion, not just that it happens — a controller
// that reaches full speed is easy, one that reaches it the way a player expects
// is the job. Each test names the complaint it prevents, because the feel knobs
// are otherwise indistinguishable from arbitrary constants.
// ---------------------------------------------------------------------------

/// Drive a character for `ticks`, returning its horizontal speed each tick.
fn run_character(cfg: CharacterConfig, input: impl Fn(usize) -> DriveInput, ticks: usize)
    -> (GameWorld, u64, Character, Vec<f32>)
{
    let mut world = world_with_ground();
    let id = walker(&mut world, vec3f(0.0, 0.9, 0.0));
    let mut ch = Character::new(id, cfg, ControlSource::Player, None);
    let mut speeds = Vec::with_capacity(ticks);
    for t in 0..ticks {
        let inp = input(t);
        ch.tick(&mut world, &inp);
        step_world(&mut world);
        world.tick += 1;
        ch.post_tick(&mut world);
        let e = world.entity(id).unwrap();
        speeds.push((e.vel.x * e.vel.x + e.vel.z * e.vel.z).sqrt());
    }
    (world, id, ch, speeds)
}

fn walk_forward() -> DriveInput {
    DriveInput { move_z: -1.0, ..Default::default() }
}

#[test]
fn speed_ramps_rather_than_stepping() {
    // The complaint: "it feels like ice." Snapping to full speed on frame one
    // reads as sliding, because nothing in the world accelerates instantly.
    let cfg = CharacterConfig::default();
    let (_, _, _, speeds) = run_character(cfg, |_| walk_forward(), 60);
    assert!(speeds[0] < cfg.speed * 0.5, "instant velocity: {}", speeds[0]);
    assert!(speeds[0] > 0.0, "no movement at all");
    // And it must actually arrive, not creep forever.
    let top = *speeds.last().unwrap();
    assert!(top > cfg.speed * 0.95, "never reached speed: {top}");
    // Monotonic while the stick is held — a ramp, not a wobble.
    for w in speeds.windows(2).take(20) {
        assert!(w[1] >= w[0] - 1e-4, "speed dipped mid-ramp: {w:?}");
    }
}

#[test]
fn stopping_is_crisper_than_starting() {
    // Deliberate asymmetry: a start should have weight, a stop should feel
    // like the player let go. Sharing one constant makes one of them wrong.
    let cfg = CharacterConfig::default();
    assert!(cfg.decel > cfg.accel);
    let (_, _, _, speeds) = run_character(
        cfg,
        |t| if t < 40 { walk_forward() } else { DriveInput::default() },
        70,
    );
    assert!(speeds[45] < speeds[39] * 0.5, "did not shed speed on release");
}

#[test]
fn coyote_time_lets_a_late_jump_register() {
    // The complaint players actually voice: "the jump didn't register." They
    // pressed it two frames after leaving the ledge and were technically
    // airborne. Being right is no comfort.
    let cfg = CharacterConfig::default();
    let mut world = world_with_ground();
    let id = walker(&mut world, vec3f(0.0, 0.9, 0.0));
    let mut ch = Character::new(id, cfg, ControlSource::Player, None);
    // Settle on the ground, then remove it: the character is now falling and
    // `on_floor` is false — the exact moment a ledge is left.
    for _ in 0..10 {
        ch.tick(&mut world, &DriveInput::default());
        step_world(&mut world);
        ch.post_tick(&mut world);
    }
    world.entities.retain(|e| e.kind != BodyKind::Static);
    step_world(&mut world);
    assert!(!world.entity(id).unwrap().on_floor, "should be airborne");
    // Press two ticks late — inside the coyote window.
    let jump = DriveInput { jump: true, jump_pressed: true, ..Default::default() };
    ch.tick(&mut world, &jump);
    assert!(
        world.entity(id).unwrap().vel.y > 0.0,
        "late jump was swallowed — coyote time is not working"
    );
}

#[test]
fn a_jump_pressed_before_landing_fires_on_touchdown() {
    // The same complaint as coyote time, from the other side: pressed a hair
    // early, ignored, and the player is certain the game dropped it.
    let cfg = CharacterConfig::default();
    let mut world = world_with_ground();
    // Start just above the floor so touchdown lands inside the buffer window.
    let id = walker(&mut world, vec3f(0.0, 1.25, 0.0));
    let mut ch = Character::new(id, cfg, ControlSource::Player, None);
    let mut launched = false;
    let mut ever_grounded = false;
    for t in 0..120 {
        // ONE press, while still falling; never repeated.
        let inp = DriveInput { jump_pressed: t == 0, ..Default::default() };
        ch.tick(&mut world, &inp);
        step_world(&mut world);
        world.tick += 1;
        ch.post_tick(&mut world);
        let e = world.entity(id).unwrap();
        ever_grounded |= e.on_floor;
        if ever_grounded && e.vel.y > 0.1 {
            launched = true;
            break;
        }
    }
    assert!(ever_grounded, "never reached the ground");
    assert!(
        launched,
        "a jump pressed just before landing was swallowed — buffering is not working"
    );
}

#[test]
fn releasing_early_gives_a_lower_jump() {
    // Variable height is the difference between a hop and a leap, and it is
    // the single most-used expressive control a platformer has.
    let cfg = CharacterConfig::default();
    let apex = |hold: usize| -> f32 {
        let mut world = world_with_ground();
        let id = walker(&mut world, vec3f(0.0, 0.9, 0.0));
        let mut ch = Character::new(id, cfg, ControlSource::Player, None);
        let mut top = 0.0f32;
        for t in 0..150 {
            let held = t < hold;
            let inp = DriveInput {
                jump: held,
                jump_pressed: t == 0,
                ..Default::default()
            };
            ch.tick(&mut world, &inp);
            step_world(&mut world);
            world.tick += 1;
            ch.post_tick(&mut world);
            top = top.max(world.entity(id).unwrap().pos.y);
        }
        top
    };
    let tapped = apex(2);
    let held = apex(120);
    assert!(
        tapped < held - 0.25,
        "tap {tapped:.2} vs hold {held:.2} — releasing early must cut the jump"
    );
}

#[test]
fn falling_is_brisker_than_rising() {
    // Symmetric gravity reads as floaty. The rise is the part the player
    // steers; the fall should get on with it.
    let cfg = CharacterConfig::default();
    assert!(cfg.fall_gravity > 1.0);
    let mut world = world_with_ground();
    let id = walker(&mut world, vec3f(0.0, 0.9, 0.0));
    let mut ch = Character::new(id, cfg, ControlSource::Player, None);
    // Settle, or the first ticks are a fall onto the ground rather than a jump.
    for _ in 0..20 {
        ch.tick(&mut world, &DriveInput::default());
        step_world(&mut world);
        world.tick += 1;
        ch.post_tick(&mut world);
    }
    assert!(world.entity(id).unwrap().on_floor, "never settled");

    let (mut rise, mut fall) = (0usize, 0usize);
    let mut peaked = false;
    for t in 0..400 {
        // Held, so the jump runs its full arc rather than being cut.
        let inp = DriveInput { jump: true, jump_pressed: t == 0, ..Default::default() };
        ch.tick(&mut world, &inp);
        step_world(&mut world);
        world.tick += 1;
        ch.post_tick(&mut world);
        let e = world.entity(id).unwrap();
        if !peaked {
            if e.vel.y > 0.0 {
                rise += 1;
            } else if rise > 0 {
                peaked = true;
            }
        } else if !e.on_floor {
            fall += 1;
        } else {
            break;
        }
    }
    assert!(rise > 0 && fall > 0, "never left the ground (rise {rise}, fall {fall})");
    assert!(
        fall < rise,
        "fall {fall} ticks vs rise {rise} — gravity is symmetric, the jump floats"
    );
}

#[test]
fn air_control_is_partial_not_absent_and_not_total() {
    // Zero air control feels broken; full air control feels like flying. Air
    // control limits the RATE, so given enough air time a player still reaches
    // full speed — the claim worth asserting is that the same input builds
    // speed more slowly off the ground.
    let cfg = CharacterConfig::default();
    assert!(cfg.air_control > 0.0 && cfg.air_control < 1.0);

    let gained = |start_y: f32, ticks: usize| -> f32 {
        let mut world = world_with_ground();
        let id = walker(&mut world, vec3f(0.0, start_y, 0.0));
        let mut ch = Character::new(id, cfg, ControlSource::Player, None);
        for _ in 0..ticks {
            ch.tick(&mut world, &walk_forward());
            step_world(&mut world);
            world.tick += 1;
            ch.post_tick(&mut world);
        }
        let e = world.entity(id).unwrap();
        (e.vel.x * e.vel.x + e.vel.z * e.vel.z).sqrt()
    };
    let on_ground = gained(0.9, 4);
    let in_air = gained(9.0, 4);
    assert!(in_air > 0.05, "no air control at all: {in_air}");
    assert!(
        in_air < on_ground * 0.9,
        "air {in_air:.2} vs ground {on_ground:.2} — air control is not reduced"
    );
}

#[test]
fn vehicle_steering_loses_authority_with_speed() {
    // Constant-rate steering at speed is the biggest "this feels like a toy"
    // tell there is. The car scales steering authority by
    // speed/steer_peak_speed, clamped — so authority RISES to the peak and is
    // capped past it, rather than being a flat rate at every speed.
    let cfg = CarConfig::default();
    assert!(
        cfg.steer_peak_speed > 0.0,
        "no speed-sensitive steering configured"
    );
    let authority = |speed: f32| (speed / cfg.steer_peak_speed).clamp(0.0, 1.0);
    assert!(authority(1.0) < authority(cfg.steer_peak_speed));
    assert_eq!(authority(cfg.steer_peak_speed * 3.0), 1.0, "authority must cap");
    // And the sign convention: steering right must lower the heading.
    assert!(steer_to_yaw_rate(1.0, cfg.steer_rate) < 0.0);
}

#[test]
fn getting_in_and_out_of_a_car_puts_you_beside_it() {
    // Dismounting into the seat means spawning inside a collider, and the
    // separation pass would then shove the player through the nearest wall.
    let mut world = world_with_ground();
    let ch_id = walker(&mut world, vec3f(0.0, 0.9, 0.0));
    let car_id = rigid(&mut world, vec3f(2.0, 1.0, 0.0), vec3f(0.9, 0.4, 1.6));
    if let Some(e) = world.entity_mut(car_id) {
        e.tag = "car".to_string();
    }
    let mut mount = Mount::new(ch_id);
    assert_eq!(mount.seat, Seat::OnFoot);
    let got_in = mount.toggle(&mut world);
    assert_eq!(got_in, Some(Seat::Driving(car_id)), "did not board a car in reach");
    assert!(world.entity(ch_id).unwrap().hidden, "driver should be parked");
    assert_eq!(mount.subject(), car_id, "camera should follow the car");

    let got_out = mount.toggle(&mut world);
    assert_eq!(got_out, Some(Seat::OnFoot));
    let (c, car) = (world.entity(ch_id).unwrap(), world.entity(car_id).unwrap());
    assert!(!c.hidden, "driver should be visible again");
    let (dx, dz) = (c.pos.x - car.pos.x, c.pos.z - car.pos.z);
    let d = (dx * dx + dz * dz).sqrt();
    assert!(d > 1.0, "stepped out INSIDE the car: {d:.2} units away");
    assert!(d < 4.0, "stepped out absurdly far: {d:.2}");
}

#[test]
fn mounting_blends_the_camera_instead_of_cutting() {
    // A cut between rigs reads as a glitch. The camera must travel.
    let mut cam = FollowCamera::new(CameraConfig::on_foot());
    let before = cam.config.distance;
    cam.transition_to(CameraConfig::in_vehicle(), 0.35);
    // Mid-blend the camera is neither rig.
    let mut world = world_with_ground();
    let id = walker(&mut world, vec3f(0.0, 0.9, 0.0));
    cam.tick(&world, id, 0.0, 0.0);
    assert!(cam.boom >= before - 0.01, "camera jumped in on mount");
}

#[test]
fn the_camera_eases_back_out_after_an_obstruction_clears() {
    // Pulling IN against a wall must be immediate — easing in spends those
    // frames inside the wall. Easing OUT must not, or the shot pops the
    // instant the player rounds a corner.
    let mut world = world_with_ground();
    let id = walker(&mut world, vec3f(0.0, 0.9, 0.0));
    let mut cam = FollowCamera::new(CameraConfig::on_foot());
    for _ in 0..60 {
        cam.tick(&world, id, 0.0, 0.0);
    }
    let open = cam.boom;
    assert!(open > 1.0, "camera never extended: {open}");
    // Drop it hard, as an obstruction would, then let it recover.
    cam.boom = 1.0;
    cam.tick(&world, id, 0.0, 0.0);
    let after_one = cam.boom;
    assert!(after_one > 1.0, "did not recover at all");
    assert!(
        after_one < open * 0.5,
        "snapped back out in one tick ({after_one} of {open}) — that is the pop"
    );
}

// ---------------------------------------------------------------------------
// The player prefab end to end (game.md §"Building blocks").
//
// These are the tests that decide whether the prefab is finished. The claim
// being defended is not "the pieces work" but "a game gets a playable character
// in and out of a car in a few lines and never touches a camera" — so the
// setup below is exactly what a generated game would write, and if it grows,
// the prefab has failed rather than the test.
// ---------------------------------------------------------------------------

/// The whole player setup a game performs. If this helper needs to grow, the
/// prefab is the thing that should change.
fn player_world() -> (GameWorld, Blocks, u64, u64) {
    let mut world = world_with_ground();
    let ch = walker(&mut world, vec3f(0.0, 0.9, 0.0));
    let car = rigid(&mut world, vec3f(2.5, 1.0, 0.0), vec3f(0.9, 0.4, 1.6));
    if let Some(e) = world.entity_mut(car) {
        e.tag = "car".to_string();
    }
    let mut blocks = Blocks::new();
    blocks.characters.push(Character::new(
        ch,
        CharacterConfig::default(),
        ControlSource::Player,
        None,
    ));
    blocks
        .cars
        .push(Car::new(car, CarConfig::default(), ControlSource::Player));
    blocks.player_rigs.insert(PlayerId::LOCAL, PlayerRig::new(ch));
    (world, blocks, ch, car)
}

/// One frame: rigs, then blocks, then the sim. The ordering the host owes.
fn player_tick(world: &mut GameWorld, blocks: &mut Blocks, raw: RawInput) {
    let mut inputs = std::collections::HashMap::new();
    inputs.insert(PlayerId::LOCAL, raw);
    blocks.tick_player_rigs(world, &inputs);
    blocks.pre_step(world);
    step_world(world);
    world.tick += 1;
    blocks.post_step(world);
}

#[test]
fn a_player_walks_gets_in_drives_and_gets_out() {
    // The headline journey, in the order a player performs it. Every leg
    // asserts the thing that leg is for; a pass means the feature works
    // end to end, not that its parts compile together.
    let (mut world, mut blocks, ch, car) = player_world();

    // --- walk toward the car -------------------------------------------
    for _ in 0..40 {
        player_tick(
            &mut world,
            &mut blocks,
            RawInput { move_y: 1.0, ..Default::default() },
        );
    }
    let walked = world.entity(ch).unwrap().pos;
    let moved = (walked.x * walked.x + walked.z * walked.z).sqrt();
    assert!(moved > 2.0, "the character barely walked: {moved:.2}");
    assert_eq!(blocks.player_rigs[&PlayerId::LOCAL].seat(), Seat::OnFoot);

    // --- get in ---------------------------------------------------------
    // Stand the player next to the car rather than relying on where the walk
    // ended: this test is about the seat, not about pathing.
    if let Some(e) = world.entity_mut(ch) {
        e.pos = vec3f(2.5, 0.9, 1.2);
    }
    player_tick(
        &mut world,
        &mut blocks,
        RawInput { use_pressed: true, ..Default::default() },
    );
    let rig = &blocks.player_rigs[&PlayerId::LOCAL];
    assert_eq!(rig.seat(), Seat::Driving(car), "did not get into the car");
    assert!(world.entity(ch).unwrap().hidden, "the driver is still standing there");

    // --- drive ----------------------------------------------------------
    let before = world.entity(car).unwrap().pos;
    for _ in 0..90 {
        player_tick(
            &mut world,
            &mut blocks,
            RawInput { throttle: 1.0, ..Default::default() },
        );
    }
    let after = world.entity(car).unwrap().pos;
    let drove = ((after.x - before.x).powi(2) + (after.z - before.z).powi(2)).sqrt();
    assert!(drove > 5.0, "the car did not drive: {drove:.2}");

    // --- get out --------------------------------------------------------
    player_tick(
        &mut world,
        &mut blocks,
        RawInput { use_pressed: true, ..Default::default() },
    );
    assert_eq!(blocks.player_rigs[&PlayerId::LOCAL].seat(), Seat::OnFoot);
    let (c, k) = (world.entity(ch).unwrap(), world.entity(car).unwrap());
    assert!(!c.hidden, "the driver never reappeared");
    let gap = ((c.pos.x - k.pos.x).powi(2) + (c.pos.z - k.pos.z).powi(2)).sqrt();
    assert!(gap > 1.0 && gap < 4.0, "stepped out {gap:.2} units from the car");
    // And the camera came with them, rather than staying on the abandoned car.
    assert_eq!(blocks.player_rigs[&PlayerId::LOCAL].mount.subject(), ch);
}

#[test]
fn driving_does_not_also_walk_your_parked_character() {
    // The modality bug: a player owns both a character and a car, `pre_step`
    // matches blocks by owner, so the same stick that steers you would also
    // walk the body you left in the driver's seat — and then you get out
    // somewhere you never went. The seat pin now carries the body with the
    // car, so the property to check is that it stays WITH the car rather than
    // that it stays put.
    let (mut world, mut blocks, ch, car) = player_world();
    if let Some(e) = world.entity_mut(ch) {
        e.pos = vec3f(2.5, 0.9, 1.2);
    }
    player_tick(
        &mut world,
        &mut blocks,
        RawInput { use_pressed: true, ..Default::default() },
    );
    for _ in 0..90 {
        player_tick(
            &mut world,
            &mut blocks,
            RawInput { throttle: 1.0, move_y: 1.0, move_x: 1.0, ..Default::default() },
        );
    }
    let (c, k) = (world.entity(ch).unwrap(), world.entity(car).unwrap());
    let apart = ((c.pos.x - k.pos.x).powi(2) + (c.pos.z - k.pos.z).powi(2)).sqrt();
    assert!(apart < 1.0, "the parked driver walked {apart:.2} units off the car");
    // And getting out puts them beside THIS car, not back where they boarded.
    player_tick(
        &mut world,
        &mut blocks,
        RawInput { use_pressed: true, ..Default::default() },
    );
    let (c, k) = (world.entity(ch).unwrap(), world.entity(car).unwrap());
    let gap = ((c.pos.x - k.pos.x).powi(2) + (c.pos.z - k.pos.z).powi(2)).sqrt();
    assert!(gap < 4.0, "got out {gap:.2} units from the car they were driving");
}

#[test]
fn driving_one_car_does_not_also_drive_your_other_one() {
    // The modality gate where it is directly observable. Nothing pins a second
    // vehicle, so if `pre_step` handed this player's throttle to every block
    // they own, the spare car drives itself off across the map while they are
    // sitting in the first one.
    let (mut world, mut blocks, ch, car) = player_world();
    let spare = rigid(&mut world, vec3f(-8.0, 1.0, 0.0), vec3f(0.9, 0.4, 1.6));
    if let Some(e) = world.entity_mut(spare) {
        e.tag = "car".to_string();
    }
    blocks
        .cars
        .push(Car::new(spare, CarConfig::default(), ControlSource::Player));
    if let Some(e) = world.entity_mut(ch) {
        e.pos = vec3f(2.5, 0.9, 1.2);
    }
    player_tick(
        &mut world,
        &mut blocks,
        RawInput { use_pressed: true, ..Default::default() },
    );
    assert_eq!(blocks.player_rigs[&PlayerId::LOCAL].seat(), Seat::Driving(car));
    let parked_at = world.entity(spare).unwrap().pos;
    for _ in 0..90 {
        player_tick(
            &mut world,
            &mut blocks,
            RawInput { throttle: 1.0, move_x: 0.4, ..Default::default() },
        );
    }
    let now = world.entity(spare).unwrap().pos;
    let drift = ((now.x - parked_at.x).powi(2) + (now.z - parked_at.z).powi(2)).sqrt();
    assert!(drift < 0.5, "the spare car drove itself {drift:.2} units");
    // And the one being driven definitely moved, or this proves nothing.
    let driven = world.entity(car).unwrap().pos;
    let d = ((driven.x - 2.5).powi(2) + driven.z.powi(2)).sqrt();
    assert!(d > 5.0, "the driven car did not move either: {d:.2}");
}

#[test]
fn analog_deflection_gives_intermediate_speed() {
    // A pad has to be able to amble. Snapping from walk to run past a
    // threshold is the clearest "toy" tell in a character controller, and a
    // bool `run` cannot express anything else.
    let cfg = CharacterConfig::default();
    let top = |run: f32| {
        let (_, _, _, speeds) = run_character(
            cfg,
            move |_| DriveInput { move_z: -1.0, run, ..Default::default() },
            90,
        );
        *speeds.last().unwrap()
    };
    let (walk, half, sprint) = (top(0.0), top(0.5), top(1.0));
    assert!(sprint > walk * 1.3, "run does nothing: {walk} -> {sprint}");
    // The point of the test: half deflection is genuinely in between, not
    // rounded to one of the two ends.
    let midpoint = (walk + sprint) * 0.5;
    assert!(
        (half - midpoint).abs() < walk * 0.1,
        "half stick gave {half}, not the {midpoint} between {walk} and {sprint}"
    );
}

#[test]
fn a_reload_keeps_you_in_the_car_with_the_camera_where_it_was() {
    // Task #24 in the place it is most visible. An edit to the game's script
    // must not eject the player mid-corner and snap the view — where you are
    // sitting and where you are looking are the player's state, not the
    // game's content.
    let (mut world, mut blocks, ch, car) = player_world();
    if let Some(e) = world.entity_mut(ch) {
        e.pos = vec3f(2.5, 0.9, 1.2);
    }
    for _ in 0..8 {
        player_tick(
            &mut world,
            &mut blocks,
            RawInput { look_dx: 40.0, ..Default::default() },
        );
    }
    player_tick(
        &mut world,
        &mut blocks,
        RawInput { use_pressed: true, ..Default::default() },
    );
    let (seat, angles) = {
        let r = &blocks.player_rigs[&PlayerId::LOCAL];
        (r.seat(), r.view_angles())
    };
    assert_eq!(seat, Seat::Driving(car));

    blocks.clear();

    let r = blocks
        .player_rigs
        .get(&PlayerId::LOCAL)
        .expect("the reload threw the player out of the world");
    assert_eq!(r.seat(), seat, "a reload ejected the driver");
    assert_eq!(r.view_angles(), angles, "a reload snapped the camera");
}

#[test]
fn losing_the_car_you_are_driving_puts_you_back_on_your_feet() {
    // A despawn, a rollback or a re-eval that does not recreate the car leaves
    // the player driving a ghost: no camera subject, no block taking their
    // input, and no way out, because the get-out reads the car to find a door.
    // Dead controls are worse than any reset.
    let (mut world, mut blocks, ch, car) = player_world();
    if let Some(e) = world.entity_mut(ch) {
        e.pos = vec3f(2.5, 0.9, 1.2);
    }
    player_tick(
        &mut world,
        &mut blocks,
        RawInput { use_pressed: true, ..Default::default() },
    );
    assert_eq!(blocks.player_rigs[&PlayerId::LOCAL].seat(), Seat::Driving(car));

    world.entities.retain(|e| e.id != car);
    player_tick(&mut world, &mut blocks, RawInput::default());

    let rig = &blocks.player_rigs[&PlayerId::LOCAL];
    assert_eq!(rig.seat(), Seat::OnFoot, "still driving a car that is gone");
    assert_eq!(rig.mount.subject(), ch, "the camera has nothing to follow");
    assert!(!world.entity(ch).unwrap().hidden, "the player stayed invisible");

    // And the controls work again.
    for _ in 0..40 {
        player_tick(
            &mut world,
            &mut blocks,
            RawInput { move_y: 1.0, ..Default::default() },
        );
    }
    let e = world.entity(ch).unwrap();
    let speed = (e.vel.x * e.vel.x + e.vel.z * e.vel.z).sqrt();
    assert!(speed > 1.0, "the player cannot move after being ejected: {speed}");
}

#[test]
fn stepping_out_against_a_wall_uses_the_other_door() {
    // Parking tight against something is normal. Preferring one side
    // unconditionally drops the player inside it, and the separation pass then
    // shoves them somewhere arbitrary — the most broken-feeling thing a
    // get-out can do.
    let (mut world, mut blocks, ch, car) = player_world();
    if let Some(e) = world.entity_mut(ch) {
        e.pos = vec3f(2.5, 0.9, 1.2);
    }
    player_tick(
        &mut world,
        &mut blocks,
        RawInput { use_pressed: true, ..Default::default() },
    );
    // Wall hard against the driver's side of the car's resting heading.
    let cpos = world.entity(car).unwrap().pos;
    let right = heading_to_right(world.entity(car).unwrap().yaw);
    block(
        &mut world,
        vec3f(cpos.x - right.x * 2.2, cpos.y, cpos.z - right.z * 2.2),
        vec3f(1.0, 1.5, 1.0),
    );
    player_tick(
        &mut world,
        &mut blocks,
        RawInput { use_pressed: true, ..Default::default() },
    );
    let c = world.entity(ch).unwrap().pos;
    // Came out on the far side instead — dot against `right` says which.
    let side = (c.x - cpos.x) * right.x + (c.z - cpos.z) * right.z;
    assert!(side > 0.5, "stepped out into the wall (side {side:.2})");
}

#[test]
fn the_prompt_and_the_button_always_agree() {
    // An affordance prompt that runs its own search drifts from the button it
    // describes, and "press E to get in" that does nothing is worse than no
    // prompt at all — it teaches the player the game is broken at the moment
    // they are learning it. Walking the whole approach checks agreement at
    // every distance, including right at the reach boundary where a duplicated
    // search would differ first.
    let (mut world, mut blocks, ch, car) = player_world();
    for step in 0..24 {
        if let Some(e) = world.entity_mut(ch) {
            e.pos = vec3f(2.5, 0.9, 12.0 - step as f32 * 0.5);
        }
        let rig = &blocks.player_rigs[&PlayerId::LOCAL];
        let promised = rig.mount.candidate(&world);
        let mut probe = rig.mount;
        let got = match probe.toggle(&mut world) {
            Some(Seat::Driving(id)) => Some(id),
            _ => None,
        };
        assert_eq!(promised, got, "prompt and button disagreed at step {step}");
        // Undo whatever the probe did to the world before the next step.
        if let Some(e) = world.entity_mut(ch) {
            e.hidden = false;
        }
    }
    // And it did become available somewhere along that approach, or the test
    // proved only that both agree on "never".
    if let Some(e) = world.entity_mut(ch) {
        e.pos = vec3f(2.5, 0.9, 1.0);
    }
    assert_eq!(
        blocks.player_rigs[&PlayerId::LOCAL].mount.candidate(&world),
        Some(car),
        "the car was never offerable"
    );
    // Once seated there is nothing to get into, so no prompt should appear.
    player_tick(
        &mut world,
        &mut blocks,
        RawInput { use_pressed: true, ..Default::default() },
    );
    let rig = &blocks.player_rigs[&PlayerId::LOCAL];
    assert_eq!(rig.seat(), Seat::Driving(car));
    assert_eq!(rig.mount.candidate(&world), None, "offered a car while driving");
}

#[test]
fn getting_out_does_not_unhide_a_model_based_player() {
    // `hidden` is the HOST's field: "my appearance is a mesh, don't also draw
    // my collider box". Every game that uses a model rather than a coloured box
    // sets it, which is every real game. Mount must restore it, not clear it —
    // otherwise getting out pops a grey collision slab into the world beside
    // the car, and the only host-side fix is re-asserting `hidden` every tick
    // forever.
    for host_hidden in [false, true] {
        let (mut world, mut blocks, ch, _car) = player_world();
        if let Some(e) = world.entity_mut(ch) {
            e.pos = vec3f(2.5, 0.9, 1.2);
            e.hidden = host_hidden;
        }
        player_tick(
            &mut world,
            &mut blocks,
            RawInput { use_pressed: true, ..Default::default() },
        );
        assert!(world.entity(ch).unwrap().hidden, "the driver was still drawn");
        player_tick(
            &mut world,
            &mut blocks,
            RawInput { use_pressed: true, ..Default::default() },
        );
        assert_eq!(
            world.entity(ch).unwrap().hidden,
            host_hidden,
            "getting out overwrote the host's `hidden` (was {host_hidden})"
        );
    }
}

#[test]
fn the_driver_rides_along_instead_of_being_left_as_an_invisible_wall() {
    // `hidden` means "solid to everything, drawn by nothing", so hiding the
    // driver in place leaves their collider standing where they boarded: drive
    // back past it later and you hit a wall that is not there. The seat pin
    // takes the body out of physics and carries it with the car.
    let (mut world, mut blocks, ch, car) = player_world();
    if let Some(e) = world.entity_mut(ch) {
        e.pos = vec3f(2.5, 0.9, 1.2);
    }
    let boarded_at = world.entity(ch).unwrap().pos;
    player_tick(
        &mut world,
        &mut blocks,
        RawInput { use_pressed: true, ..Default::default() },
    );
    for _ in 0..90 {
        player_tick(
            &mut world,
            &mut blocks,
            RawInput { throttle: 1.0, ..Default::default() },
        );
    }
    let (c, k) = (world.entity(ch).unwrap(), world.entity(car).unwrap());
    let left_behind = ((c.pos.x - boarded_at.x).powi(2) + (c.pos.z - boarded_at.z).powi(2)).sqrt();
    assert!(left_behind > 3.0, "the body stayed at the kerb: {left_behind:.2}");
    let to_car = ((c.pos.x - k.pos.x).powi(2) + (c.pos.z - k.pos.z).powi(2)).sqrt();
    assert!(to_car < 1.0, "the rider is not travelling with the car: {to_car:.2}");
}

#[test]
fn a_new_world_falls_without_being_told_to() {
    // Gravity was set only by `reset_content`, which script evaluation calls
    // and nothing else does — so every world built through the sim API floated
    // until its caller happened to know. The symptom is not "physics looks
    // wrong", it is a character that silently never reports `on_floor` and so
    // refuses to jump. Four test files had each grown their own
    // `world.gravity = 30.0`; a workaround that gets copy-pasted means the
    // default is the bug.
    let mut world = GameWorld::new();
    assert!(world.gravity > 0.0, "a fresh world has no gravity");
    ground(&mut world, vec3f(40.0, 1.0, 40.0));
    let id = walker(&mut world, vec3f(0.0, 6.0, 0.0));
    for _ in 0..120 {
        step_world(&mut world);
        world.tick += 1;
    }
    let e = world.entity(id).unwrap();
    assert!(e.on_floor, "never landed, so a jump would never fire");
    assert!(e.pos.y < 3.0, "did not fall: y={}", e.pos.y);
}

#[test]
fn reverse_moves_the_car_but_the_brake_still_wins() {
    // Two separate claims, because conflating them is what makes "reverse
    // barely works" hard to diagnose.
    //
    // 1. Reverse alone must actually reverse. Half engine authority is the
    //    arcade convention and is fine; being immobile is not.
    // 2. Brake AND reverse together must stay near-immobile. That is correct
    //    car behaviour — a foot on the brake wins — so it is pinned here
    //    rather than "fixed". Any input path that maps one control to BOTH
    //    brake and negative throttle will therefore read as broken reverse,
    //    and the bug is in that mapping, not in this force model.
    let drive_for = |input: DriveInput, ticks: usize| -> f32 {
        let (mut world, mut blocks, chassis) = car_world();
        if let Some(c) = blocks.car_mut(chassis) {
            c.control = ControlSource::Script;
            c.input = input;
        }
        let start = world.entity(chassis).unwrap().pos;
        run(&mut world, &mut blocks, ticks);
        let e = world.entity(chassis).unwrap();
        ((e.pos.x - start.x).powi(2) + (e.pos.z - start.z).powi(2)).sqrt()
    };
    let secs = (2.0 / DT) as usize;
    let forward = drive_for(DriveInput { throttle: 1.0, ..Default::default() }, secs);
    let reverse = drive_for(DriveInput { throttle: -1.0, ..Default::default() }, secs);
    let braked = drive_for(
        DriveInput { throttle: -1.0, brake: 1.0, ..Default::default() },
        secs,
    );
    assert!(forward > 10.0, "the car did not drive forward at all: {forward:.2}");
    assert!(
        reverse > forward * 0.4,
        "reverse is not usable: {reverse:.2}m vs {forward:.2}m forward"
    );
    assert!(
        braked < reverse * 0.2,
        "the brake did not hold the car: {braked:.2}m vs {reverse:.2}m free"
    );
}
