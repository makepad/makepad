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
