// World snapshot round-trip test (port-specific; the C snapshot path is only
// exercised through the recording/replay keyframe machinery, which is not
// ported).
//
// A populated world (hull ground, mesh terrain, stacked hulls, spheres, a
// capsule, revolute + distance joints, a sensor) is stepped, serialized,
// restored into a shell world, and both worlds are stepped on — the restored
// world must continue bit-identically to the original.

use makepad_box3d::body::*;
use makepad_box3d::core::{hash, HASH_INIT};
use makepad_box3d::id::BodyId;
use makepad_box3d::ensure;
use makepad_box3d::hull::make_box_hull;
use makepad_box3d::joint::{create_distance_joint, create_revolute_joint};
use makepad_box3d::math_functions::{pos, vec3, Vec3};
use makepad_box3d::mesh::create_grid_mesh;
use makepad_box3d::physics_world::*;
use makepad_box3d::recording::{write_registry, RecBuffer, Recording};
use makepad_box3d::recording_replay::load_registry;
use makepad_box3d::shape::*;
use makepad_box3d::types::*;
use makepad_box3d::world_snapshot::{deserialize_into_shell, serialize_world};

fn hash_f32(h: u32, v: f32) -> u32 {
    hash(h, &v.to_le_bytes())
}

fn hash_vec3(h: u32, v: Vec3) -> u32 {
    let h = hash_f32(h, v.x);
    let h = hash_f32(h, v.y);
    hash_f32(h, v.z)
}

/// Hash a world position at full width so the round-trip comparison stays
/// exact in double precision mode.
#[cfg(not(feature = "double-precision"))]
fn hash_pos(h: u32, p: makepad_box3d::math_functions::Pos) -> u32 {
    hash_vec3(h, p)
}

#[cfg(feature = "double-precision")]
fn hash_pos(h: u32, p: makepad_box3d::math_functions::Pos) -> u32 {
    let mut b = [0u8; 24];
    b[0..8].copy_from_slice(&p.x.to_le_bytes());
    b[8..16].copy_from_slice(&p.y.to_le_bytes());
    b[16..24].copy_from_slice(&p.z.to_le_bytes());
    hash(h, &b)
}

// Deterministic hash over the transforms and velocities of the given bodies.
fn hash_bodies(world: &World, body_ids: &[BodyId]) -> u32 {
    let mut h = HASH_INIT;
    for &id in body_ids {
        let p = body_get_position(world, id);
        let q = body_get_rotation(world, id);
        let v = body_get_linear_velocity(world, id);
        let w = body_get_angular_velocity(world, id);
        h = hash_pos(h, p);
        h = hash_vec3(h, q.v);
        h = hash_f32(h, q.s);
        h = hash_vec3(h, v);
        h = hash_vec3(h, w);
    }
    h
}

struct Scene {
    world: World,
    dynamic_ids: Vec<BodyId>,
}

// Ground hull + mesh terrain + ~20 dynamic bodies + 2 joints + a sensor.
fn build_scene(world_def: &WorldDef) -> Scene {
    let mut world = create_world(world_def);
    let mut dynamic_ids = Vec::new();

    let shape_def = default_shape_def();

    // Static ground box
    let ground_def = default_body_def();
    let ground_id = create_body(&mut world, &ground_def);
    let ground_hull = make_box_hull(20.0, 0.5, 20.0);
    let _ = create_hull_shape(&mut world, ground_id, &shape_def, &ground_hull);

    // Mesh terrain next to the ground box
    let mut terrain_def = default_body_def();
    terrain_def.position = pos(50.0, 0.0, 0.0);
    let terrain_id = create_body(&mut world, &terrain_def);
    let grid = create_grid_mesh(8, 8, 2.0, 1, true);
    let _ = create_mesh_shape(&mut world, terrain_id, &shape_def, &grid, vec3(1.0, 1.0, 1.0));

    // Sensor box hovering over the ground
    {
        let mut sensor_body_def = default_body_def();
        sensor_body_def.position = pos(0.0, 2.0, 0.0);
        let sensor_body = create_body(&mut world, &sensor_body_def);
        let mut sensor_def = default_shape_def();
        sensor_def.is_sensor = true;
        sensor_def.enable_sensor_events = true;
        let sensor_hull = make_box_hull(3.0, 2.0, 3.0);
        let _ = create_hull_shape(&mut world, sensor_body, &sensor_def, &sensor_hull);
    }

    // A stack of box hulls (guaranteed persistent contacts)
    let box_hull = make_box_hull(0.5, 0.5, 0.5);
    for k in 0..5 {
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.position = pos(0.0, 1.1 + 1.05 * k as f32, 0.0);
        let id = create_body(&mut world, &body_def);
        let mut sd = default_shape_def();
        sd.enable_sensor_events = true;
        let _ = create_hull_shape(&mut world, id, &sd, &box_hull);
        dynamic_ids.push(id);
    }

    // Spheres raining on the terrain mesh
    for k in 0..8 {
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.position = pos(52.0 + 0.9 * (k % 4) as f32, 4.0 + 1.5 * (k / 4) as f32, 3.0 + 0.8 * (k % 3) as f32);
        let id = create_body(&mut world, &body_def);
        let sphere = Sphere { center: Vec3::ZERO, radius: 0.4 };
        let _ = create_sphere_shape(&mut world, id, &shape_def, &sphere);
        dynamic_ids.push(id);
    }

    // A capsule
    {
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.position = pos(3.0, 3.0, 3.0);
        let id = create_body(&mut world, &body_def);
        let capsule = Capsule { center1: vec3(0.0, -0.4, 0.0), center2: vec3(0.0, 0.4, 0.0), radius: 0.3 };
        let _ = create_capsule_shape(&mut world, id, &shape_def, &capsule);
        dynamic_ids.push(id);
    }

    // A pendulum: static pivot + revolute joint + distance-joined bob
    {
        let mut pivot_def = default_body_def();
        pivot_def.position = pos(-5.0, 6.0, 0.0);
        let pivot = create_body(&mut world, &pivot_def);

        let mut arm_def = default_body_def();
        arm_def.body_type = BodyType::Dynamic;
        arm_def.position = pos(-4.0, 6.0, 0.0);
        let arm = create_body(&mut world, &arm_def);
        let _ = create_hull_shape(&mut world, arm, &shape_def, &box_hull);
        dynamic_ids.push(arm);

        let mut rev_def = makepad_box3d::joint::default_revolute_joint_def();
        rev_def.base.body_id_a = pivot;
        rev_def.base.body_id_b = arm;
        rev_def.base.local_frame_a.p = vec3(0.0, 0.0, 0.0);
        rev_def.base.local_frame_b.p = vec3(1.0, 0.0, 0.0);
        let _ = create_revolute_joint(&mut world, &rev_def);

        let mut bob_def = default_body_def();
        bob_def.body_type = BodyType::Dynamic;
        bob_def.position = pos(-4.0, 4.0, 0.0);
        let bob = create_body(&mut world, &bob_def);
        let bob_sphere = Sphere { center: Vec3::ZERO, radius: 0.3 };
        let _ = create_sphere_shape(&mut world, bob, &shape_def, &bob_sphere);
        dynamic_ids.push(bob);

        let mut dist_def = makepad_box3d::joint::default_distance_joint_def();
        dist_def.base.body_id_a = arm;
        dist_def.base.body_id_b = bob;
        dist_def.length = 2.0;
        let _ = create_distance_joint(&mut world, &dist_def);
    }

    Scene { world, dynamic_ids }
}

#[test]
fn snapshot_round_trip() {
    let world_def = default_world_def();
    let scene = build_scene(&world_def);
    let mut world = scene.world;
    let body_ids = scene.dynamic_ids;

    // Step so bodies are moving, stacked, and touching
    for _ in 0..30 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    let hash_before = hash_bodies(&world, &body_ids);

    // Serialize the world + the geometry registry
    let mut snap_buf = RecBuffer::new();
    let mut rec = Recording::new();
    let byte_count = serialize_world(&world, &mut snap_buf, &mut rec);
    ensure!(byte_count > 0);
    ensure!(snap_buf.size() == byte_count as usize);
    ensure!(!rec.registry.entries.is_empty());

    write_registry(&mut rec);
    let registry_bytes = rec.buffer.data.clone();
    let image = snap_buf.data.clone();

    // Restore into a shell world
    let rdr = load_registry(&registry_bytes).expect("registry should load");
    let mut restored = create_world(&world_def);
    let ok = deserialize_into_shell(&image, &mut restored, &rdr);
    ensure!(ok);

    // Identical state immediately after restore
    let hash_after = hash_bodies(&restored, &body_ids);
    ensure!(hash_before == hash_after);

    // Bit-identical continuation: step BOTH worlds and compare periodically
    for frame in 0..60 {
        world_step(&mut world, 1.0 / 60.0, 4);
        world_step(&mut restored, 1.0 / 60.0, 4);

        if frame % 10 == 9 {
            let h1 = hash_bodies(&world, &body_ids);
            let h2 = hash_bodies(&restored, &body_ids);
            assert!(h1 == h2, "restored world diverged at frame {}", frame);
        }
    }

    // Post-restore usability: ray cast, create a new body, step again
    {
        let filter = default_query_filter();
        let result = world_cast_ray_closest(&restored, pos(0.0, 10.0, 0.0), vec3(0.0, -20.0, 0.0), filter);
        ensure!(result.hit);

        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.position = pos(1.5, 8.0, -1.5);
        let new_body = create_body(&mut restored, &body_def);
        let sphere = Sphere { center: Vec3::ZERO, radius: 0.4 };
        let shape_def = default_shape_def();
        let new_shape = create_sphere_shape(&mut restored, new_body, &shape_def, &sphere);
        ensure!(shape_is_valid(&restored, new_shape));

        for _ in 0..30 {
            world_step(&mut restored, 1.0 / 60.0, 4);
        }
        ensure!(body_is_valid(&restored, new_body));
    }

    destroy_world(restored);
    destroy_world(world);
}

#[test]
fn snapshot_rejects_corrupt_input() {
    let world_def = default_world_def();
    let scene = build_scene(&world_def);
    let mut world = scene.world;

    for _ in 0..10 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    let mut snap_buf = RecBuffer::new();
    let mut rec = Recording::new();
    let _ = serialize_world(&world, &mut snap_buf, &mut rec);
    write_registry(&mut rec);
    let registry_bytes = rec.buffer.data.clone();
    let image = snap_buf.data.clone();

    let rdr = load_registry(&registry_bytes).expect("registry should load");

    // Bad version
    {
        let mut bad = image.clone();
        bad[4] ^= 0xFF;
        let mut shell = create_world(&world_def);
        ensure!(!deserialize_into_shell(&bad, &mut shell, &rdr));
        destroy_world(shell);
    }

    // Bad magic
    {
        let mut bad = image.clone();
        bad[0] ^= 0xFF;
        let mut shell = create_world(&world_def);
        ensure!(!deserialize_into_shell(&bad, &mut shell, &rdr));
        destroy_world(shell);
    }

    // Truncations at various points must fail cleanly, never panic
    for cut in [8usize, 64, 256, image.len() / 2, image.len() - 8] {
        let bad = &image[..cut.min(image.len())];
        let mut shell = create_world(&world_def);
        ensure!(!deserialize_into_shell(bad, &mut shell, &rdr));
        destroy_world(shell);
    }

    // Truncated registry must fail cleanly
    {
        let cut = registry_bytes.len() / 2;
        ensure!(load_registry(&registry_bytes[..cut]).is_none());
    }

    destroy_world(world);
}
