//! The plan layer over the destructible terrain (worldgen DESIGN.md,
//! amendment B): plan presses compose over history at read time and never
//! touch history bytes; a retracted feature leaves nothing; a human fill
//! under a plan press survives it; holes and the border answer as
//! themselves; the same (plan, history) composes to the same bytes twice.

use makepad_game_sim::voxel::{ChunkKey, DigMode};
use makepad_game_sim::*;
use makepad_math::*;

fn flat_terrain(h: f32) -> Terrain {
    let cells = 33;
    let cell_size = 2.0;
    let origin = -32.0;
    Terrain {
        cells,
        cell_size,
        origin,
        heights: vec![h; cells * cells],
        colors: vec![vec4f(0.4, 0.6, 0.4, 1.0); cells * cells],
        revision: 1,
    }
}

fn world() -> GameWorld {
    let mut w = GameWorld::new();
    w.terrain = Some(flat_terrain(4.0));
    w
}

fn field(w: &mut GameWorld) -> &mut makepad_game_sim::voxel::VoxelField {
    w.voxel
        .get_or_insert_with(|| Box::new(makepad_game_sim::voxel::VoxelField::new(0.5)))
}

/// A history op that materializes the chunks around the origin: a shallow
/// fill dome, so the surface there is ABOVE the plane a press will ask for.
fn fill_mound(w: &mut GameWorld) {
    w.apply_voxel_op(VoxelOp::Dig {
        pos: vec3f(0.0, 4.0, 0.0),
        r: 3.0,
        mode: DigMode::Fill,
        material: 2,
    });
}

#[test]
fn a_plan_press_never_touches_history_bytes() {
    let mut w = world();
    fill_mound(&mut w);
    let before = w.surface_height_at(0.0, 0.0).expect("mound owns the surface");
    assert!(before > 4.5, "the fill raised the ground: {before}");
    let field = w.voxel.as_deref().unwrap();
    let key = ChunkKey::of_site(field.world_site(vec3f(0.0, 4.0, 0.0)));
    let history_before = field.chunk_history(key).unwrap().to_vec();
    let hash_before = field.field_hash();

    // The plan presses a pad plane at y = 4.2 over the mound.
    w.voxel
        .as_deref_mut()
        .unwrap()
        .plan_press(7, vec3f(-4.0, 0.0, -4.0), vec3f(4.0, 20.0, 4.0), 4.2);
    let pressed = w.surface_height_at(0.0, 0.0).expect("still owned");
    assert!(
        pressed <= 4.2 + 0.6 && pressed < before,
        "the press clips the mound to its plane: {pressed} (was {before})"
    );
    let field = w.voxel.as_deref().unwrap();
    assert_eq!(
        field.chunk_history(key).unwrap(),
        history_before.as_slice(),
        "history bytes are untouched by a plan press"
    );
    assert_ne!(field.field_hash(), hash_before, "the composed field did change");

    // Retracting the owner gives the mound back, byte-exact.
    w.voxel.as_deref_mut().unwrap().retract_plan(7);
    let after = w.surface_height_at(0.0, 0.0).unwrap();
    assert!((after - before).abs() < 1e-4, "retraction restores the surface: {after} vs {before}");
    assert_eq!(w.voxel.as_deref().unwrap().field_hash(), hash_before, "retraction is exact");
}

#[test]
fn a_human_fill_under_a_plan_press_survives_the_press() {
    let mut w = world();
    // Plan first (a railway bed), then the player fills under it.
    field(&mut w).plan_press(3, vec3f(-6.0, 0.0, -6.0), vec3f(6.0, 20.0, 6.0), 3.0);
    fill_mound(&mut w);
    // While pressed, the surface is the pad plane.
    let pressed = w.surface_height_at(0.0, 0.0).unwrap();
    assert!(pressed <= 3.6, "pressed under the railway: {pressed}");
    // The railway is removed: the fill the player made is still there.
    w.voxel.as_deref_mut().unwrap().retract_plan(3);
    let free = w.surface_height_at(0.0, 0.0).unwrap();
    assert!(free > 4.5, "the human fill survived the plan press: {free}");
}

#[test]
fn reset_content_clears_the_plan_but_keeps_history() {
    let mut w = world();
    fill_mound(&mut w);
    let mound = w.surface_height_at(0.0, 0.0).unwrap();
    w.voxel
        .as_deref_mut()
        .unwrap()
        .plan_press(1, vec3f(-4.0, 0.0, -4.0), vec3f(4.0, 20.0, 4.0), 4.0);
    assert!(w.surface_height_at(0.0, 0.0).unwrap() < mound);
    w.reset_content();
    w.terrain = Some(flat_terrain(4.0));
    assert!(w.voxel.as_deref().unwrap().plan.is_empty(), "the plan layer is script content");
    let back = w.surface_height_at(0.0, 0.0).unwrap();
    assert!((back - mound).abs() < 1e-4, "history survives reset_content: {back} vs {mound}");
}

#[test]
fn the_wire_press_op_lands_in_the_plan_not_the_chunks() {
    let mut w = world();
    fill_mound(&mut w);
    let key = ChunkKey::of_site(w.voxel.as_deref().unwrap().world_site(vec3f(0.0, 4.0, 0.0)));
    let history = w.voxel.as_deref().unwrap().chunk_history(key).unwrap().to_vec();
    w.apply_voxel_op(VoxelOp::Press {
        min: vec3f(-4.0, 0.0, -4.0),
        max: vec3f(4.0, 20.0, 4.0),
        y: 4.1,
    });
    let field = w.voxel.as_deref().unwrap();
    assert_eq!(field.plan.presses.len(), 1, "a Press op is a plan press");
    assert_eq!(field.chunk_history(key).unwrap(), history.as_slice());
    assert!(w.surface_height_at(0.0, 0.0).unwrap() <= 4.7);
}

#[test]
fn holes_answer_hole_and_the_border_answers_outside() {
    let mut w = world();
    // Carve every solid site of the column inside the one chunk band the
    // capsule materializes ([0, 16) m): the ground beneath is gone there,
    // and the base layer below the band is not the column's business.
    // A carve whose box sits exactly on the chunk band [0, 16) — its sphere
    // removes every solid site of the column there (ground is at 4).
    w.apply_voxel_op(VoxelOp::Dig {
        pos: vec3f(0.0, 2.0, 0.0),
        r: 2.0,
        mode: DigMode::Carve,
        material: 0,
    });
    match w.surface_sample_at(0.0, 0.0) {
        SurfaceSample::Hole => {}
        other => panic!("a column carved through is a Hole, not {other:?}"),
    }
    assert_eq!(w.surface_height_at(0.0, 0.0), None, "no fake heightfield ground over a hole");
    assert_eq!(w.surface_sample_at(500.0, 500.0), SurfaceSample::Outside);
    assert_eq!(w.surface_sample_at(-32.0, -32.0).height(), Some(4.0), "the near edge is inside");
    assert!(w.surface_sample_at(10.0, 10.0).is_ground());
    // A punched heightfield cell is a hole even with no voxel chunk there.
    let t = w.terrain.as_ref().unwrap();
    let side = t.cells - 1;
    let mut m = TerrainMaterials::default();
    m.indices = vec![0u8; side * side];
    let ix = ((20.0 - t.origin) / t.cell_size).floor() as usize;
    let iz = ((20.0 - t.origin) / t.cell_size).floor() as usize;
    m.indices[iz * side + ix] = 0xFF;
    w.terrain_materials = Some(m);
    assert_eq!(w.surface_sample_at(20.5, 20.5), SurfaceSample::Hole);
    assert!(w.surface_sample_at(24.5, 24.5).is_ground());
}

#[test]
fn plan_landforms_are_not_history() {
    let mut w = world();
    w.in_plan_eval = true;
    landform::host_apply_landform(
        &mut w,
        VoxelOp::Landform {
            pos: vec3f(10.0, 4.0, 10.0),
            kind: LandKind::Hill.to_u8(),
            r: 12.0,
            height: 6.0,
            seed: 3,
        },
    );
    w.in_plan_eval = false;
    assert!(w.surface_height_at(10.0, 10.0).unwrap() > 6.0, "the plan hill rose");
    let recorded = w.voxel.as_deref().map_or(0, |f| f.land_ops.len() + f.persist_ops.len());
    assert_eq!(recorded, 0, "a plan landform is never recorded or persisted as history");
    // A landform issued outside eval IS history.
    landform::host_apply_landform(
        &mut w,
        VoxelOp::Landform {
            pos: vec3f(-10.0, 4.0, -10.0),
            kind: LandKind::Hill.to_u8(),
            r: 8.0,
            height: 3.0,
            seed: 4,
        },
    );
    let field = w.voxel.as_deref().unwrap();
    assert_eq!(field.land_ops.len(), 1, "a brush landform is history");
}

#[test]
fn the_same_plan_and_history_compose_identically() {
    let build = || {
        let mut w = world();
        fill_mound(&mut w);
        w.apply_voxel_op(VoxelOp::Dig {
            pos: vec3f(6.0, 4.0, 2.0),
            r: 2.0,
            mode: DigMode::Carve,
            material: 0,
        });
        w.voxel
            .as_deref_mut()
            .unwrap()
            .plan_press(9, vec3f(-3.0, 0.0, -3.0), vec3f(5.0, 20.0, 5.0), 4.3);
        w.voxel.as_deref().unwrap().field_hash()
    };
    assert_eq!(build(), build());
}
