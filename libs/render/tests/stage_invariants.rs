//! Pure stage transforms and environment selection.
use makepad_render::stage::{Stage, DIORAMA_SCALE};
use makepad_draw::*;

#[test]
fn the_diorama_shrinks_the_view_not_the_world() {
    // A crate at rest sits at the same world height regardless of stage; it
    // is only its *appearance* in the room that shrinks.
    let settled = vec3f(0.3, 0.5, -0.2);

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
    let crate_pos = vec3f(0.3, 0.5, -0.2);
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
