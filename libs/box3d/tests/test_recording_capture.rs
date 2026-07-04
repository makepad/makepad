// Capture-side test for the recording op stream (replay lands separately).
// Validates stream structure: header, snapshot seed, framed ops, registry.

use makepad_box3d::body::*;
use makepad_box3d::ensure;
use makepad_box3d::hull::make_box_hull;
use makepad_box3d::joint::create_revolute_joint;
use makepad_box3d::math_functions::{pos, vec3};
use makepad_box3d::physics_world::*;
use makepad_box3d::recording::{
    hash_world_state, load_recording_from_file, save_recording_to_file, world_start_recording,
    world_stop_recording, RecOp, REC_HEADER_SIZE, REC_MAGIC, REC_VERSION_MAJOR,
};
use makepad_box3d::shape::*;
use makepad_box3d::types::*;

fn read_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
fn read_u64(d: &[u8], o: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[o..o + 8]);
    u64::from_le_bytes(b)
}

/// Walk the framed op stream between the snapshot seed and the registry block.
/// Returns (op, payload_offset, payload_len) triples.
fn walk_ops(data: &[u8]) -> Vec<(u8, usize, usize)> {
    let snapshot_size = read_u64(data, 24) as usize;
    let registry_offset = read_u64(data, 32) as usize;
    let mut ops = Vec::new();
    let mut p = REC_HEADER_SIZE + snapshot_size;
    while p < registry_offset {
        let op = data[p];
        let size = data[p + 1] as usize | (data[p + 2] as usize) << 8 | (data[p + 3] as usize) << 16;
        ops.push((op, p + 4, size));
        p += 4 + size;
    }
    assert_eq!(p, registry_offset, "op stream must end exactly at the registry");
    ops
}

fn build_and_run(world: &mut World, record: bool) -> u64 {
    if record {
        world_start_recording(world);
    }

    // Ground box (interns a hull into the registry)
    let ground_hull = make_box_hull(10.0, 0.5, 10.0);
    let ground_def = default_body_def();
    let ground_id = create_body(world, &ground_def);
    let shape_def = default_shape_def();
    let _hull_shape = create_hull_shape(world, ground_id, &shape_def, &ground_hull);

    // Falling bodies: a sphere and a second sphere joined by a revolute joint
    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    body_def.position = pos(0.0, 4.0, 0.0);
    let body_a = create_body(world, &body_def);
    let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.5 };
    let _sa = create_sphere_shape(world, body_a, &shape_def, &sphere);

    body_def.position = pos(1.2, 4.0, 0.0);
    let body_b = create_body(world, &body_def);
    let _sb = create_sphere_shape(world, body_b, &shape_def, &sphere);

    let mut joint_def = makepad_box3d::joint::default_revolute_joint_def();
    joint_def.base.body_id_a = body_a;
    joint_def.base.body_id_b = body_b;
    let _joint = create_revolute_joint(world, &joint_def);

    // A couple of mutators
    body_set_linear_velocity(world, body_a, vec3(0.0, -1.0, 0.0));
    body_set_name(world, body_b, "pendulum");

    // Step some frames
    for _ in 0..10 {
        world_step(world, 1.0 / 60.0, 4);
    }

    // Tagged ray cast + untagged overlap
    let mut filter = default_query_filter();
    filter.id = 42;
    filter.name = "probe";
    let _hit = world_cast_ray_closest(world, pos(0.0, 5.0, 0.0), vec3(0.0, -10.0, 0.0), filter);

    let aabb = makepad_box3d::math_functions::AABB {
        lower_bound: vec3(-2.0, -2.0, -2.0),
        upper_bound: vec3(2.0, 6.0, 2.0),
    };
    let mut count = 0;
    world_overlap_aabb(world, aabb, default_query_filter(), &mut |_id| {
        count += 1;
        true
    });
    ensure!(count > 0);

    // Destroy one body, then a couple more steps
    destroy_body(world, body_b);
    for _ in 0..2 {
        world_step(world, 1.0 / 60.0, 4);
    }

    hash_world_state(world)
}

#[test]
fn recording_capture_stream_structure() {
    let world_def = default_world_def();
    let mut world = create_world(&world_def);
    let _final_hash = build_and_run(&mut world, true);

    let rec = world_stop_recording(&mut world).expect("recording active");
    let data = rec.data();

    // Header
    ensure!(data.len() > REC_HEADER_SIZE);
    ensure!(read_u32(data, 0) == REC_MAGIC);
    ensure!(u16::from_le_bytes([data[4], data[5]]) == REC_VERSION_MAJOR);
    let snapshot_size = read_u64(data, 24) as usize;
    let registry_offset = read_u64(data, 32) as usize;
    let registry_byte_count = read_u64(data, 40) as usize;
    ensure!(snapshot_size > 0); // snapshot-seeded (empty world still serializes)
    ensure!(registry_offset >= REC_HEADER_SIZE + snapshot_size);
    ensure!(registry_offset + registry_byte_count == data.len());

    // Ops
    let ops = walk_ops(data);
    let count_op = |op: RecOp| ops.iter().filter(|(o, _, _)| *o == op as u8).count();

    ensure!(count_op(RecOp::Step) == 12);
    // StateHash: one anchor at start + one per step
    ensure!(count_op(RecOp::StateHash) == 13);
    ensure!(count_op(RecOp::CreateBody) == 3);
    ensure!(count_op(RecOp::CreateHullShape) == 1);
    ensure!(count_op(RecOp::CreateSphereShape) == 2);
    ensure!(count_op(RecOp::CreateRevoluteJoint) == 1);
    ensure!(count_op(RecOp::BodySetLinearVelocity) == 1);
    ensure!(count_op(RecOp::BodySetName) == 1);
    ensure!(count_op(RecOp::DestroyBody) == 1);
    ensure!(count_op(RecOp::QueryCastRayClosest) == 1);
    ensure!(count_op(RecOp::QueryOverlapAABB) == 1);
    // Tagged ray cast emits its QueryTag right before the query record
    ensure!(count_op(RecOp::QueryTag) == 1);
    let tag_pos = ops.iter().position(|(o, _, _)| *o == RecOp::QueryTag as u8).unwrap();
    ensure!(ops[tag_pos + 1].0 == RecOp::QueryCastRayClosest as u8);
    // Stop wrote bounds + the end-of-stream marker as the last two records
    ensure!(ops[ops.len() - 2].0 == RecOp::RecordingBounds as u8);
    ensure!(ops[ops.len() - 1].0 == RecOp::DestroyWorld as u8);

    // Registry: the ground hull was interned once; the tag table has one entry
    let entry_count = read_u32(data, registry_offset) as usize;
    ensure!(entry_count == 1);
    let mut p = registry_offset + 4;
    let kind = data[p];
    ensure!(kind == 0); // GeometryKind::Hull
    let byte_count = read_u32(data, p + 1) as usize;
    ensure!(byte_count > 0);
    p += 5 + byte_count;
    let tag_count = read_u32(data, p) as usize;
    ensure!(tag_count == 1);
    ensure!(read_u64(data, p + 4 + 8) == 42); // tag id

    // Save/load round trip validates the magic path
    let path = std::env::temp_dir().join("box3d_capture_test.b3rc");
    let path = path.to_str().unwrap();
    ensure!(save_recording_to_file(&rec, path));
    let loaded = load_recording_from_file(path).expect("load");
    ensure!(loaded.data() == rec.data());
    let _ = std::fs::remove_file(path);

    destroy_world(world);
}

#[test]
fn recording_is_zero_change() {
    // The same scene stepped with and without a recording must produce
    // bit-identical simulation state (hooks are observers only).
    let mut world_a = create_world(&default_world_def());
    let hash_a = build_and_run(&mut world_a, false);
    destroy_world(world_a);

    let mut world_b = create_world(&default_world_def());
    let hash_b = build_and_run(&mut world_b, true);
    let _rec = world_stop_recording(&mut world_b).expect("recording active");
    destroy_world(world_b);

    ensure!(hash_a == hash_b);
}
