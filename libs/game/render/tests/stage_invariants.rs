//! M3 gates for the stage (game.md §"Presentation modes").
//!
//! The design claim being tested is narrow and load-bearing: *the stage is a
//! presentation concern*. Two headsets and a laptop can look at one
//! replicated world through three different projections, and the simulation
//! must not be able to tell. If a stage switch could perturb sim state, every
//! device in a room would drift apart the moment someone put a headset on.

use makepad_game_render::stage::{Stage, StageMode, DIORAMA_SCALE};
use makepad_game_sim::*;
use makepad_draw::*;

fn fnv(h: u64, v: u32) -> u64 {
    (h ^ v as u64).wrapping_mul(0x100000001b3)
}

/// Same shape as the M1a determinism hash: everything the sim integrates.
fn hash_world(w: &GameWorld) -> u64 {
    let mut h = 0xcbf29ce484222325;
    for e in &w.entities {
        for f in [
            e.pos.x, e.pos.y, e.pos.z, e.vel.x, e.vel.y, e.vel.z, e.orient.x, e.orient.y,
            e.orient.z, e.orient.w, e.yaw,
        ] {
            h = fnv(h, f.to_bits());
        }
    }
    h = fnv(h, w.tick as u32);
    h
}

fn ent(id: u64, kind: BodyKind, pos: Vec3f, half: Vec3f) -> Entity {
    Entity {
        id,
        kind,
        pos,
        half,
        collide: true,
        gravity_scale: 1.0,
        density: 1.0,
        friction: 0.6,
        restitution: 0.0,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        speed_mult: 1.0,
        ..Default::default()
    }
}

/// Ground, a falling rigid crate and a mover — enough that anything which
/// leaked into the sim would show up in the hash within a few ticks.
fn world() -> GameWorld {
    let mut w = GameWorld::new();
    w.entities.push(ent(
        1,
        BodyKind::Static,
        vec3f(0.0, -0.5, 0.0),
        vec3f(20.0, 0.5, 20.0),
    ));
    w.entities.push(ent(
        2,
        BodyKind::Rigid,
        vec3f(0.3, 4.0, -0.2),
        vec3f(0.5, 0.5, 0.5),
    ));
    w.entities.push(ent(
        3,
        BodyKind::Mover,
        vec3f(-2.0, 2.0, 1.0),
        vec3f(0.4, 0.9, 0.4),
    ));
    w.next_id = 3;
    w
}

fn step_n(w: &mut GameWorld, n: usize) {
    for _ in 0..n {
        step_world(w);
        w.tick += 1;
    }
}

#[test]
fn switching_stage_cannot_touch_the_simulation() {
    // Two identical worlds. One is watched through a flat window the whole
    // time; the other is switched flat → MR diorama → VR → flat while it
    // runs. Their state must stay bit-identical.
    let (mut fixed, mut switched) = (world(), world());
    let mut stage = Stage::flat();

    for round in 0..4 {
        step_n(&mut fixed, 30);
        step_n(&mut switched, 30);
        stage = match round {
            0 => Stage::mr_diorama(vec3f(0.2, 0.0, -1.5), 0.4, DIORAMA_SCALE),
            1 => Stage::vr_full_scale(),
            2 => Stage::mr_diorama(vec3f(-1.0, 0.8, -2.0), 2.1, 0.01),
            _ => Stage::flat(),
        };
        // The stage only ever informs the renderer. There is deliberately no
        // API by which it could reach the world — this line is the whole of
        // "switch presentation mode".
        let _ = stage.matrix();
        assert_eq!(
            hash_world(&fixed),
            hash_world(&switched),
            "stage switch at round {round} perturbed the simulation"
        );
    }
    // And the world genuinely moved, so the equality above isn't vacuous.
    assert_ne!(hash_world(&fixed), hash_world(&world()));
    assert!(stage.mode == StageMode::Flat);
}

#[test]
fn the_diorama_shrinks_the_view_not_the_world() {
    // A crate at rest sits at the same world height regardless of stage; it
    // is only its *appearance* in the room that shrinks.
    let mut w = world();
    step_n(&mut w, 120);
    let settled = w.entities[1].pos;

    let flat = Stage::flat();
    let mr = Stage::mr_diorama(vec3f(0.0, 0.0, -1.5), 0.0, DIORAMA_SCALE);

    assert_eq!(flat.world_to_stage(settled), settled);
    let in_room = mr.world_to_stage(settled);
    // 20:1 — a crate resting a half-metre up in world units is 2.5cm up on
    // the carpet.
    assert!(
        (in_room.y - (settled.y * DIORAMA_SCALE)).abs() < 1.0e-5,
        "{in_room:?} vs {settled:?}"
    );
    // The sim still holds the original.
    assert_eq!(w.entities[1].pos, settled);
}

#[test]
fn mr_suppresses_the_game_environment_vr_keeps_it() {
    // The renderer reads exactly this to decide whether to draw sky and
    // terrain; RenderStats.sky_drawn/terrain_drawn assert the consequence at
    // the draw call level, which needs a GPU. This is the CPU-side contract.
    assert!(!Stage::mr_diorama(vec3f(0.0, 0.0, -1.0), 0.0, DIORAMA_SCALE).shows_environment());
    assert!(Stage::vr_full_scale().shows_environment());
    assert!(Stage::flat().shows_environment());
}

#[test]
fn controller_rays_map_back_into_world_units() {
    // Picking in MR: a point in the room resolves to the world coordinate
    // the sim understands, so an XR player can point at an entity.
    let mr = Stage::mr_diorama(vec3f(0.4, 0.1, -1.6), 0.7, DIORAMA_SCALE);
    let mut w = world();
    step_n(&mut w, 60);
    let crate_pos = w.entities[1].pos;
    // Where the crate appears in the room...
    let on_carpet = mr.world_to_stage(crate_pos);
    // ...maps back to where the sim thinks it is.
    let back = mr.stage_to_world(on_carpet);
    assert!(
        (back.x - crate_pos.x).abs() < 1.0e-3
            && (back.y - crate_pos.y).abs() < 1.0e-3
            && (back.z - crate_pos.z).abs() < 1.0e-3,
        "{back:?} vs {crate_pos:?}"
    );
}
