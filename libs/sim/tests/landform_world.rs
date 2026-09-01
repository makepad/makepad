//! Landform + tunnel + dig at world scale: the acceptance shapes of the
//! unified destructible world, meshed to completion, with hard bounds on
//! how far any mesh vertex or heightfield vertex may stand from the ground.
//! Regression net for the "floating curtain wall" class of bug.

use makepad_game_sim::voxel::DigMode;
use makepad_game_sim::*;
use makepad_math::*;

fn rolling_terrain() -> Terrain {
    let cells = 97;
    let cell_size = 200.0 / 96.0;
    let origin = -100.0;
    let mut heights = vec![0.0f32; cells * cells];
    for gz in 0..cells {
        for gx in 0..cells {
            let x = origin + gx as f32 * cell_size;
            let z = origin + gz as f32 * cell_size;
            heights[gz * cells + gx] = 2.0 + (x * 0.03).sin() + (z * 0.025).cos();
        }
    }
    Terrain {
        cells,
        cell_size,
        origin,
        heights,
        colors: vec![vec4f(0.4, 0.6, 0.4, 1.0); cells * cells],
        revision: 1,
    }
}

#[test]
fn acceptance_shapes_stay_bounded() {
    let mut w = GameWorld::new();
    w.terrain = Some(rolling_terrain());
    // Mirror of the live acceptance script: dig, mountain, hill, tunnel.
    let g_dig = w.surface_height_at(-20.0, 10.0).unwrap();
    w.apply_voxel_op(VoxelOp::Dig {
        pos: vec3f(-20.0, g_dig, 10.0),
        r: 6.0,
        mode: DigMode::Carve,
        material: 1,
    });
    w.apply_voxel_op(VoxelOp::Landform {
        pos: vec3f(45.0, w.surface_height_at(45.0, -30.0).unwrap(), -30.0),
        kind: LandKind::Mountain.to_u8(),
        r: 34.0,
        height: 22.0,
        seed: 9,
    });
    w.apply_voxel_op(VoxelOp::Landform {
        pos: vec3f(-55.0, w.surface_height_at(-55.0, -50.0).unwrap(), -50.0),
        kind: LandKind::Hill.to_u8(),
        r: 24.0,
        height: 12.0,
        seed: 4,
    });
    let my = w.surface_height_at(-55.0, -20.0).unwrap() + 1.4;
    w.apply_voxel_op(VoxelOp::Tunnel {
        from: vec3f(-55.0, my, -20.0),
        to: vec3f(-55.0, my, -80.0),
        r: 2.4,
    });

    // Heights stay within base + mountain reach.
    let t = w.terrain.as_ref().unwrap();
    let (mut hmin, mut hmax) = (f32::MAX, f32::MIN);
    for h in &t.heights {
        assert!(h.is_finite(), "non-finite height");
        hmin = hmin.min(*h);
        hmax = hmax.max(*h);
    }
    assert!(hmax < 28.0, "heightfield exploded: max {hmax}");
    assert!(hmin > -10.0, "heightfield exploded: min {hmin}");

    // Mesh everything, then every voxel mesh vertex must hug the ground
    // band — no floating curtain walls in the sky.
    for _ in 0..4096 {
        update_world_voxel(&mut w, true);
        if w.voxel.as_deref().map_or(0, |v| v.dirty_len()) == 0 {
            break;
        }
    }
    let field = w.voxel.as_deref().unwrap();
    let chunks = field.chunk_count();
    assert!(chunks > 0, "nothing materialized");
    assert!(chunks < 400, "materialization ran away: {chunks} chunks");
    let mut vmax = f32::MIN;
    let mut vmin = f32::MAX;
    for mesh in field.meshes.values() {
        for v in mesh.verts.chunks_exact(makepad_game_sim::voxel::MESH_VERTEX_FLOATS) {
            vmax = vmax.max(v[1]);
            vmin = vmin.min(v[1]);
        }
    }
    println!(
        "chunks {chunks}, heights {hmin:.2}..{hmax:.2}, mesh y {vmin:.2}..{vmax:.2}"
    );
    assert!(vmax < 28.0, "voxel mesh towers into the sky: max y {vmax}");
    assert!(vmin > -30.0, "voxel mesh under the world: min y {vmin}");

    // The tunnel bore is open air mid-hill.
    assert!(
        field.is_carved_air(vec3f(-55.0, my, -50.0)),
        "tunnel bore closed"
    );
    // The seam sees the mountain.
    let peak = w.surface_height_at(45.0, -30.0).unwrap();
    assert!(peak > 10.0, "mountain missing from the seam: {peak}");

    // A walker at the mouth strolls INTO the hill through the bore — under
    // the raised ground, not over it (the tunnel is the voxel layer's whole
    // point).
    let id = {
        let mut e = Entity::default();
        w.next_id += 1;
        e.id = w.next_id;
        e.kind = BodyKind::Mover;
        e.pos = vec3f(-55.0, my + 0.1, -16.0);
        e.half = vec3f(0.35, 0.85, 0.35);
        e.collide = true;
        let id = e.id;
        w.push_entity(e);
        id
    };
    let mouth_y = my;
    for _ in 0..600 {
        if let Some(e) = w.entity_mut(id) {
            e.vel.x = 0.0;
            e.vel.z = -3.0;
        }
        step_world(&mut w);
    }
    let e = w.entity(id).unwrap();
    assert!(
        e.pos.z < -30.0,
        "walker never entered the tunnel (z {})",
        e.pos.z
    );
    let hill_here = w.terrain.as_ref().unwrap().height_at(e.pos.x, e.pos.z).unwrap();
    assert!(
        e.pos.y < mouth_y + 2.5 && e.pos.y < hill_here - 1.0,
        "walker went OVER the hill instead of through it (y {} vs hill {hill_here})",
        e.pos.y
    );
}
