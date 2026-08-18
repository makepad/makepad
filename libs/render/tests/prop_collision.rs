//! A walker must be stopped by a stock prop's real collider.
//!
//! The existing collision tests pass a house wall and a tree trunk, both of
//! which are large parts of large models. A park bench is small, thin and
//! close to the ground, and the user reported walking straight through one
//! three times — so the only test worth having here uses the ACTUAL bench
//! model, scaled and placed the way the demo places it, rather than a
//! synthetic box that would have passed all along.
//!
//! Skips (rather than fails) when the asset packs are absent, so a fresh
//! checkout without `apps/sandbox/download_assets.sh` still runs green.

use makepad_render::model::StaticModel;
use makepad_game_sim::*;
use makepad_draw::*;

const PACKS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../apps/sandbox/resources/models/kenney"
);

fn load(rel: &str) -> Option<StaticModel> {
    let bytes = std::fs::read(std::path::Path::new(PACKS).join(rel)).ok()?;
    StaticModel::parse_glb(&bytes).ok()
}

fn ent(id: u64, kind: BodyKind, pos: Vec3f, half: Vec3f) -> Entity {
    Entity {
        id,
        kind,
        shape: Shape::Box,
        pos,
        half,
        collide: true,
        gravity_scale: 1.0,
        density: 1.0,
        friction: 0.6,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        speed_mult: 1.0,
        ..Default::default()
    }
}

/// Reproduces `SandboxView::compose_village`'s placement maths: scale the model
/// to a target height, then turn each collider part into a world-space box.
fn prop_collider_boxes(m: &StaticModel, target_h: f32, at: Vec3f) -> Vec<(Vec3f, Vec3f)> {
    let native_h = (m.max.y - m.min.y).max(0.001);
    let s = target_h / native_h;
    m.collider_parts()
        .iter()
        .map(|(a, b)| {
            let c = vec3f(
                (a.x + b.x) * 0.5 * s,
                (a.y + b.y) * 0.5 * s,
                (a.z + b.z) * 0.5 * s,
            );
            let e = vec3f(
                (b.x - a.x) * 0.5 * s,
                (b.y - a.y) * 0.5 * s,
                (b.z - a.z) * 0.5 * s,
            );
            (
                vec3f(at.x + c.x, c.y.max(e.y), at.z + c.z),
                vec3f(e.x.max(0.08), e.y.max(0.08), e.z.max(0.08)),
            )
        })
        .collect()
}

/// The bench must produce a collider that is actually in the walker's way:
/// tall enough to intersect a person and wide enough to be hit. A bench that
/// collapses to the 0.08 minimum in any axis is scenery, not furniture.
#[test]
fn a_bench_produces_a_collider_a_person_can_hit() {
    let Some(m) = load("graveyard-kit/bench.glb").or_else(|| load("coaster-kit/bench.glb")) else {
        eprintln!("skip: no bench model — run apps/sandbox/download_assets.sh");
        return;
    };
    let boxes = prop_collider_boxes(&m, 0.9, vec3f(0.0, 0.0, 0.0));
    assert!(!boxes.is_empty(), "bench produced no collider at all");
    let (pos, half) = boxes[0];
    // Reaches from the ground up to roughly seat height...
    assert!(
        pos.y - half.y < 0.15,
        "bench collider floats: base at {}",
        pos.y - half.y
    );
    assert!(
        pos.y + half.y > 0.55,
        "bench collider too short to stop anyone: top at {}",
        pos.y + half.y
    );
    // ...and is a real obstacle in plan, not a sliver.
    assert!(
        half.x.min(half.z) > 0.1,
        "bench collider is a sliver: half = {half:?}"
    );
}

/// The user's actual complaint: walking the pavement into a bench and passing
/// through it. Walks a person-sized mover along the demo's patrol line into a
/// bench placed on that line, and requires that he stops short of it.
#[test]
fn a_walker_is_stopped_by_a_real_bench() {
    let Some(m) = load("graveyard-kit/bench.glb").or_else(|| load("coaster-kit/bench.glb")) else {
        eprintln!("skip: no bench model — run apps/sandbox/download_assets.sh");
        return;
    };
    let mut w = GameWorld::new();
    w.reset_content();
    w.push_entity(ent(
        1,
        BodyKind::Static,
        vec3f(0.0, -0.5, 0.0),
        vec3f(30.0, 0.5, 30.0),
    ));

    // A bench on the walking line, five units along.
    let bench_x = 5.0;
    let mut next_id = 2;
    for (pos, half) in prop_collider_boxes(&m, 0.9, vec3f(bench_x, 0.0, 0.0)) {
        w.push_entity(ent(next_id, BodyKind::Static, pos, half));
        next_id += 1;
    }
    let bench_near_face = w
        .entities
        .iter()
        .filter(|e| e.id >= 2)
        .map(|e| e.pos.x - e.half.x)
        .fold(f32::MAX, f32::min);

    // Person-sized mover, same half-extents the demo gives the Knight.
    let walker_half = vec3f(0.7, 1.8, 0.7);
    w.push_entity(ent(
        next_id,
        BodyKind::Mover,
        vec3f(0.0, walker_half.y, 0.0),
        walker_half,
    ));

    // Walk into it for two seconds: unobstructed he would reach x = 6.4.
    for _ in 0..120 {
        if let Some(e) = w.entity_mut(next_id) {
            e.vel.x = 3.2;
        }
        step_world(&mut w);
    }

    let x = w.entity(next_id).unwrap().pos.x;
    assert!(
        x < bench_near_face - walker_half.x + 0.05,
        "walked through the bench: stopped at {x}, bench face at {bench_near_face}"
    );
}
