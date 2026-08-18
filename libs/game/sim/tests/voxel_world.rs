//! Voxel terrain in a full stepped world (mix.md T4 gates): movers stand on
//! blocky builds, walk through carved tunnels without heightfield snap-back,
//! rigids rest across collider hot-swaps, and the whole thing snapshots
//! bit-exactly.

use makepad_game_sim::voxel::REMESH_BUDGET_PER_TICK;
use makepad_game_sim::*;
use makepad_math::*;

/// A 65×65 heightfield with a ridge across the middle: h = 6 for |z| ≤ 4,
/// ramping to 0 at |z| = 8.
fn ridge_terrain() -> Terrain {
    let cells = 65;
    let mut heights = vec![0.0f32; cells * cells];
    let mut colors = vec![vec4f(0.4, 0.6, 0.4, 1.0); cells * cells];
    for gz in 0..cells {
        for gx in 0..cells {
            let z = -32.0 + gz as f32;
            let az = z.abs();
            let h = if az <= 4.0 {
                6.0
            } else if az < 8.0 {
                6.0 * (8.0 - az) / 4.0
            } else {
                0.0
            };
            heights[gz * cells + gx] = h;
            if h > 0.5 {
                colors[gz * cells + gx] = vec4f(0.5, 0.45, 0.4, 1.0);
            }
        }
    }
    Terrain {
        cells,
        cell_size: 1.0,
        origin: -32.0,
        heights,
        colors,
        revision: 1,
    }
}

fn flat_terrain() -> Terrain {
    let cells = 65;
    Terrain {
        cells,
        cell_size: 1.0,
        origin: -32.0,
        heights: vec![0.0; cells * cells],
        colors: vec![vec4f(0.4, 0.6, 0.4, 1.0); cells * cells],
        revision: 1,
    }
}

fn world_with(terrain: Terrain) -> GameWorld {
    let mut w = GameWorld::new();
    w.gravity = 30.0;
    w.terrain = Some(terrain);
    w
}

fn declare_volume(w: &mut GameWorld, min: Vec3f, max: Vec3f, mode: VoxelMode) {
    let voxel = w
        .voxel
        .get_or_insert_with(|| Box::new(VoxelField::new(0.5)));
    voxel.declare_volume(min, max, mode);
}

fn dig(w: &mut GameWorld, pos: Vec3f, r: f32, mode: DigMode) {
    w.apply_voxel_op(VoxelOp::Dig {
        pos,
        r,
        mode,
        material: 1,
    });
}

fn set_block(w: &mut GameWorld, x: i32, y: i32, z: i32, material: u8) {
    w.apply_voxel_op(VoxelOp::SetBlock { x, y, z, material });
}

fn mover(w: &mut GameWorld, pos: Vec3f) -> u64 {
    w.next_id += 1;
    let id = w.next_id;
    w.push_entity(Entity {
        id,
        kind: BodyKind::Mover,
        pos,
        half: vec3f(0.35, 0.9, 0.35),
        collide: true,
        gravity_scale: 1.0,
        push_mass: 1.0,
        speed_mult: 1.0,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        density: 1.0,
        friction: 0.6,
        ..Default::default()
    });
    id
}

fn rigid_ball(w: &mut GameWorld, pos: Vec3f, r: f32) -> u64 {
    w.next_id += 1;
    let id = w.next_id;
    w.push_entity(Entity {
        id,
        kind: BodyKind::Rigid,
        shape: Shape::Sphere,
        pos,
        half: vec3f(r, r, r),
        collide: true,
        gravity_scale: 1.0,
        push_mass: 1.0,
        speed_mult: 1.0,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        density: 1.0,
        friction: 0.5,
        restitution: 0.1,
        ..Default::default()
    });
    id
}

/// Drain the remesh queue: run world ticks until no chunk is dirty. Bounded
/// so a scheduling bug fails the test instead of hanging it.
fn settle_meshes(w: &mut GameWorld) {
    for _ in 0..64 {
        step_world(w);
        if w.voxel.as_ref().map_or(true, |v| v.dirty_len() == 0) {
            return;
        }
    }
    panic!(
        "remesh queue never drained: {} dirty",
        w.voxel.as_ref().unwrap().dirty_len()
    );
}

/// Carve a tunnel through the ridge along z at x=0: spheres every metre from
/// well before the ridge to well after, at roof-safe depth.
fn carve_tunnel(w: &mut GameWorld) {
    for i in -9..=9 {
        dig(w, vec3f(0.0, 1.0, i as f32), 1.6, DigMode::Carve);
    }
}

#[test]
fn a_mover_walks_through_a_tunnel_under_the_ridge() {
    let mut w = world_with(ridge_terrain());
    declare_volume(
        &mut w,
        vec3f(-6.0, -3.0, -12.0),
        vec3f(6.0, 12.0, 12.0),
        VoxelMode::Smooth,
    );
    carve_tunnel(&mut w);
    settle_meshes(&mut w);

    let id = mover(&mut w, vec3f(0.0, 1.0, -10.5));
    let mut max_y = f32::MIN;
    for _ in 0..600 {
        if let Some(e) = w.entity_mut(id) {
            e.vel.x = 0.0;
            e.vel.z = 3.0;
        }
        step_world(&mut w);
        let e = w.entity(id).unwrap();
        max_y = max_y.max(e.pos.y);
    }
    let e = w.entity(id).unwrap();
    assert!(
        e.pos.z > 9.0,
        "mover stuck at z={:.2} — never crossed the ridge through the tunnel",
        e.pos.z
    );
    // The ridge tops at 6; a mover teleported onto it would peak ≥ 6.9.
    assert!(
        max_y < 4.0,
        "mover peaked at y={max_y:.2} — it went OVER the ridge, not through it"
    );
}

#[test]
fn a_blocky_tower_holds_a_mover_and_blocks_walking() {
    let mut w = world_with(flat_terrain());
    declare_volume(
        &mut w,
        vec3f(8.0, -2.0, 8.0),
        vec3f(24.0, 14.0, 24.0),
        VoxelMode::Blocky,
    );
    // A 2×2 tower, 8 blocks tall, on the ground at (20..21, 20..21) sites →
    // world x/z 10..11, top at site y=8 → world y 4.0.
    for x in 20..22 {
        for z in 20..22 {
            for y in 0..8 {
                set_block(&mut w, x, y, z, 3);
            }
        }
    }
    settle_meshes(&mut w);
    let top = 8.0 * 0.5;

    // Stand on it: drop a mover over the tower.
    let id = mover(&mut w, vec3f(10.5, top + 3.0, 10.5));
    for _ in 0..120 {
        step_world(&mut w);
    }
    let e = w.entity(id).unwrap();
    assert!(e.on_floor, "mover never landed on the tower");
    assert!(
        (e.pos.y - (top + 0.9)).abs() < 0.1,
        "mover rests at y={:.2}, expected ≈ {:.2} (tower top {top})",
        e.pos.y,
        top + 0.9
    );

    // Walk into it: a mover on the ground cannot pass through the tower.
    let walker = mover(&mut w, vec3f(7.5, 0.9, 10.5));
    for _ in 0..180 {
        if let Some(e) = w.entity_mut(walker) {
            e.vel.x = 3.0;
            e.vel.z = 0.0;
        }
        step_world(&mut w);
    }
    let e = w.entity(walker).unwrap();
    assert!(
        e.pos.x < 10.0,
        "walker at x={:.2} passed through the tower wall at x=10",
        e.pos.x
    );
    assert!(e.pos.y < 2.0, "walker climbed the tower to y={:.2}", e.pos.y);
}

#[test]
fn a_rigid_ball_falls_into_a_dug_crater() {
    let mut w = world_with(flat_terrain());
    declare_volume(
        &mut w,
        vec3f(-12.0, -6.0, -12.0),
        vec3f(12.0, 8.0, 12.0),
        VoxelMode::Smooth,
    );
    dig(&mut w, vec3f(4.0, 0.0, 4.0), 3.0, DigMode::Carve);
    settle_meshes(&mut w);
    assert!(
        w.dynamics.voxel_body_count() > 0,
        "no voxel colliders after the dig"
    );

    let ball = rigid_ball(&mut w, vec3f(4.0, 3.0, 4.0), 0.4);
    for _ in 0..300 {
        step_world(&mut w);
    }
    let e = w.entity(ball).unwrap();
    assert!(
        e.pos.y < -0.5,
        "ball rests at y={:.2} — it never fell below the old ground into the crater",
        e.pos.y
    );
}

#[test]
fn collider_hot_swap_never_drops_a_resting_rigid() {
    let mut w = world_with(flat_terrain());
    declare_volume(
        &mut w,
        vec3f(-12.0, -6.0, -12.0),
        vec3f(12.0, 8.0, 12.0),
        VoxelMode::Smooth,
    );
    // Materialize the chunk (and punch its heightfield cells) with a small
    // crater in one corner, then rest a ball on the UNDUG voxel surface of
    // the same chunk.
    dig(&mut w, vec3f(2.0, 0.0, 2.0), 1.5, DigMode::Carve);
    settle_meshes(&mut w);
    let ball = rigid_ball(&mut w, vec3f(10.0, 1.0, 10.0), 0.4);
    for _ in 0..240 {
        step_world(&mut w);
    }
    let settled = w.entity(ball).unwrap().pos;
    assert!(
        settled.y > 0.2 && settled.y < 0.7,
        "ball settled at y={:.2}, expected ≈ 0.4 on the voxel ground",
        settled.y
    );

    // Remesh the SAME chunk (another dig far from the ball) — the collider
    // hot-swaps. The ball must neither drop nor pop across the swap.
    let mut min_y = f32::MAX;
    for i in 0..90 {
        if i % 30 == 0 {
            dig(
                &mut w,
                vec3f(2.0, 0.0, 6.0 + i as f32 * 0.02),
                1.5,
                DigMode::Carve,
            );
        }
        step_world(&mut w);
        min_y = min_y.min(w.entity(ball).unwrap().pos.y);
    }
    let after = w.entity(ball).unwrap().pos;
    assert!(
        min_y > settled.y - 0.05,
        "ball dipped to y={min_y:.3} during collider swaps (settled {:.3}) — a frame ran without collision",
        settled.y
    );
    assert!(
        (after.y - settled.y).abs() < 0.05,
        "ball moved from {:.3} to {:.3} across remeshes",
        settled.y,
        after.y
    );
}

#[test]
fn voxel_worlds_snapshot_and_step_bit_identically() {
    let mut w = world_with(flat_terrain());
    declare_volume(
        &mut w,
        vec3f(-12.0, -6.0, -12.0),
        vec3f(12.0, 8.0, 12.0),
        VoxelMode::Smooth,
    );
    dig(&mut w, vec3f(4.0, 0.0, 4.0), 3.0, DigMode::Carve);
    settle_meshes(&mut w);
    let ball = rigid_ball(&mut w, vec3f(4.5, 2.0, 3.5), 0.4);
    for _ in 0..30 {
        step_world(&mut w);
    }
    // Snapshot mid-flight (the Clone the rollback ring performs) and run
    // both worlds forward: bit-identical, voxel colliders included.
    let mut fork = w.clone();
    for _ in 0..60 {
        step_world(&mut w);
        step_world(&mut fork);
    }
    let a = w.entity(ball).unwrap();
    let b = fork.entity(ball).unwrap();
    assert_eq!(a.pos, b.pos, "snapshot fork diverged");
    assert_eq!(a.vel, b.vel);
    assert_eq!(
        w.voxel.as_ref().unwrap().field_hash(),
        fork.voxel.as_ref().unwrap().field_hash()
    );
}

#[test]
fn hot_reload_keeps_edits_and_a_redeclared_volume_remeshes_them() {
    let mut w = world_with(flat_terrain());
    declare_volume(
        &mut w,
        vec3f(-12.0, -6.0, -12.0),
        vec3f(12.0, 8.0, 12.0),
        VoxelMode::Smooth,
    );
    dig(&mut w, vec3f(0.0, 0.0, 0.0), 3.0, DigMode::Carve);
    settle_meshes(&mut w);
    let hash = w.voxel.as_ref().unwrap().field_hash();
    assert!(w.voxel.as_ref().unwrap().is_carved_air(vec3f(0.0, -1.0, 0.0)));

    // The script hot-reload path: content resets, script re-runs.
    w.reset_content();
    assert_eq!(
        w.voxel.as_ref().unwrap().field_hash(),
        hash,
        "edits did not survive reset_content"
    );
    w.terrain = Some(flat_terrain());
    declare_volume(
        &mut w,
        vec3f(-12.0, -6.0, -12.0),
        vec3f(12.0, 8.0, 12.0),
        VoxelMode::Smooth,
    );
    settle_meshes(&mut w);
    assert!(
        w.voxel.as_ref().unwrap().is_carved_air(vec3f(0.0, -1.0, 0.0)),
        "crater gone after reload"
    );
    assert!(
        w.dynamics.voxel_body_count() > 0,
        "voxel colliders did not come back after reload"
    );
}

/// T7 measurement: ms per chunk remesh, printed for the report. The bound is
/// deliberately loose — this is a regression tripwire, not a benchmark.
#[test]
fn remesh_cost_per_chunk_is_bounded() {
    let mut w = world_with(flat_terrain());
    declare_volume(
        &mut w,
        vec3f(-30.0, -6.0, -30.0),
        vec3f(30.0, 8.0, 30.0),
        VoxelMode::Smooth,
    );
    // Materialize a wide area.
    for ix in -1..=1 {
        for iz in -1..=1 {
            dig(
                &mut w,
                vec3f(ix as f32 * 16.0, 0.0, iz as f32 * 16.0),
                3.0,
                DigMode::Carve,
            );
        }
    }
    let voxel = w.voxel.as_mut().unwrap();
    let dirty = voxel.dirty_len();
    assert!(dirty >= 9);
    let start = std::time::Instant::now();
    let mut done = 0;
    while voxel.dirty_len() > 0 {
        done += voxel.update_meshes(BaseSample::World(w.terrain.as_ref()), REMESH_BUDGET_PER_TICK);
    }
    let elapsed = start.elapsed();
    let per_chunk = elapsed.as_secs_f64() * 1000.0 / done as f64;
    println!(
        "remesh: {done} chunks in {:.2} ms — {per_chunk:.3} ms/chunk (budget {REMESH_BUDGET_PER_TICK}/tick)",
        elapsed.as_secs_f64() * 1000.0
    );
    assert!(
        per_chunk < 25.0,
        "remesh cost exploded: {per_chunk:.2} ms per chunk"
    );

    // The full in-world path: budgeted meshing + hole punches + box3d mesh
    // collider builds, as step_world runs it. This is the number a tick
    // actually pays while someone digs.
    let mut w = world_with(flat_terrain());
    declare_volume(
        &mut w,
        vec3f(-30.0, -6.0, -30.0),
        vec3f(30.0, 8.0, 30.0),
        VoxelMode::Smooth,
    );
    for ix in -1..=1 {
        for iz in -1..=1 {
            dig(
                &mut w,
                vec3f(ix as f32 * 16.0, 0.0, iz as f32 * 16.0),
                3.0,
                DigMode::Carve,
            );
        }
    }
    let dirty = w.voxel.as_ref().unwrap().dirty_len();
    let start = std::time::Instant::now();
    let mut ticks = 0;
    while w.voxel.as_ref().unwrap().dirty_len() > 0 && ticks < 64 {
        step_world(&mut w);
        ticks += 1;
    }
    let elapsed = start.elapsed();
    println!(
        "remesh+colliders: {dirty} chunks over {ticks} ticks in {:.2} ms — {:.3} ms/chunk, {:.3} ms/tick at budget {REMESH_BUDGET_PER_TICK}",
        elapsed.as_secs_f64() * 1000.0,
        elapsed.as_secs_f64() * 1000.0 / dirty as f64,
        elapsed.as_secs_f64() * 1000.0 / ticks as f64,
    );
}
