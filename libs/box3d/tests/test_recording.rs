// Port of box3d/test/test_recording.c — recording round trips, the headless
// replay validator, and the incremental player (keyframes, seek, restart,
// per-frame query store, tags).
//
// Not ported (debug draw / debug-shape callbacks are not in the port):
// - DebugShapeCallbacks — exercises b3RecPlayer_SetDebugShapeCallbacks + draw
// - KeyframeHandleReuse — renderer-handle reuse across keyframe restores
// The keyframe restore path those tests also touch is covered by
// scrub_backward / seek_with_hull / player_accessors below.
//
// Port-specific additions at the end: a round trip over the capture test's
// scene, and a worker-count invariance check (record at 4 workers, replay at
// 1 and 4 — the sim is worker-count invariant so all must reproduce).

use std::sync::Arc;

use makepad_box3d::body::*;
use makepad_box3d::compound::create_compound;
use makepad_box3d::distance_joint::*;
use makepad_box3d::ensure;
use makepad_box3d::height_field::create_grid;
use makepad_box3d::hull::{create_hull, make_box_hull};
use makepad_box3d::id::{ShapeId, NULL_BODY_ID};
use makepad_box3d::joint::*;
use makepad_box3d::math_functions::{
    make_quat_from_axis_angle, pos, vec3, Matrix3, Quat, Transform, WorldTransform, AABB,
};
use makepad_box3d::mesh::create_grid_mesh;
use makepad_box3d::motor_joint::*;
use makepad_box3d::parallel_joint::*;
use makepad_box3d::physics_world::*;
use makepad_box3d::prismatic_joint::*;
use makepad_box3d::recording::{
    append_geometry, hash64_blob, hash_query_tag, hash_world_state, intern_geometry,
    load_recording_from_file, save_recording_to_file, world_start_recording, world_stop_recording,
    GeometryKind, GeometryRegistry, REC_HEADER_SIZE,
};
use makepad_box3d::recording_replay::{validate_replay, Player, RecQueryKind};
use makepad_box3d::revolute_joint::*;
use makepad_box3d::shape::*;
use makepad_box3d::spherical_joint::*;
use makepad_box3d::types::*;
use makepad_box3d::weld_joint::*;
use makepad_box3d::wheel_joint::*;

fn read_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
fn read_u64(d: &[u8], o: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[o..o + 8]);
    u64::from_le_bytes(b)
}

// Query callbacks matching the C test's QueryReplay*Fcn set.
fn overlap_fcn() -> impl FnMut(ShapeId) -> bool {
    |_id| true
}
fn cast_fcn() -> impl FnMut(makepad_box3d::id::ShapeId, makepad_box3d::math_functions::Pos, makepad_box3d::math_functions::Vec3, f32, u64, i32, i32) -> f32
{
    // Return the fraction to keep the closest hit, exercising the recorded
    // user-return path.
    |_id, _point, _normal, fraction, _mat, _tri, _child| fraction
}
fn plane_fcn() -> impl FnMut(ShapeId, &[PlaneResult]) -> bool {
    |_id, _planes| true
}

// C: SphereRoundTrip — record/step/stop, then replay and validate.
#[test]
fn sphere_round_trip() {
    let mut world = create_world(&default_world_def());
    world_start_recording(&mut world);

    // Set a non-default gravity so the setter op appears in the stream.
    world_set_gravity(&mut world, vec3(0.0, -10.0, 0.0));

    // Static ground
    let ground_id = create_body(&mut world, &default_body_def());
    let ground_box = make_box_hull(50.0, 1.0, 50.0);
    create_hull_shape(&mut world, ground_id, &default_shape_def(), &ground_box);

    // Dynamic body with a sphere shape
    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    body_def.position = pos(0.0, 5.0, 0.0);
    let body_id = create_body(&mut world, &body_def);
    let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.5 };
    let mut sphere_def = default_shape_def();
    sphere_def.density = 1.0;
    create_sphere_shape(&mut world, body_id, &sphere_def, &sphere);

    for _ in 0..30 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    let rec = world_stop_recording(&mut world).expect("recording active");
    destroy_world(world);

    ensure!(validate_replay(rec.data(), 1));
}

// C: EmptyWorldRoundTrip — an empty world is still seed-serialized; Restart
// restores in place with a stable world id.
#[test]
fn empty_world_round_trip() {
    let mut world = create_world(&default_world_def());
    world_start_recording(&mut world);
    world_set_gravity(&mut world, vec3(0.0, -10.0, 0.0));

    for _ in 0..10 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    let rec = world_stop_recording(&mut world).expect("recording active");
    destroy_world(world);

    let data = rec.data();

    // The seed snapshot is written even with no bodies.
    ensure!(read_u64(data, 24) > 0);

    ensure!(validate_replay(data, 1));

    // Restart restores in place, so the replay world survives a rewind.
    let mut player = Player::create(data, 1).expect("player");
    let world_key = (player.world().world_id, player.world().generation);
    while !player.is_at_end() {
        player.step_frame();
    }
    player.restart();
    ensure!((player.world().world_id, player.world().generation) == world_key);
    ensure!(player.get_frame() == 0);
    ensure!(!player.has_diverged());
    player.destroy();
}

// C: HullDedup — three bodies sharing one hull produce one registry entry.
#[test]
fn hull_dedup() {
    let pts = [
        vec3(-1.0, -1.0, -1.0),
        vec3(1.0, -1.0, -1.0),
        vec3(1.0, 1.0, -1.0),
        vec3(-1.0, 1.0, -1.0),
        vec3(-1.0, -1.0, 1.0),
        vec3(1.0, -1.0, 1.0),
        vec3(1.0, 1.0, 1.0),
        vec3(-1.0, 1.0, 1.0),
    ];
    let hull = create_hull(&pts, 8).expect("hull");

    let mut world = create_world(&default_world_def());
    world_start_recording(&mut world);

    let mut shape_def = default_shape_def();
    shape_def.density = 1.0;

    for i in 0..3 {
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.position = pos((i * 3) as f32, 5.0, 0.0);
        let body_id = create_body(&mut world, &body_def);
        create_hull_shape(&mut world, body_id, &shape_def, &hull);
    }

    for _ in 0..5 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    let rec = world_stop_recording(&mut world).expect("recording active");
    destroy_world(world);

    ensure!(validate_replay(rec.data(), 1));

    // Registry deduped to 1 hull entry: registryOffset at byte 32 of the
    // header, entryCount is a LE u32 at the start of the registry block.
    let data = rec.data();
    ensure!(data.len() >= REC_HEADER_SIZE);
    let reg_off = read_u64(data, 32) as usize;
    ensure!(reg_off != 0 && reg_off + 4 <= data.len());
    ensure!(read_u32(data, reg_off) == 1);
}

// C: MidStreamNoContacts — recording starts after steps, snapshot-seeded, with
// free-falling bodies only.
#[test]
fn mid_stream_no_contacts() {
    let mut world = create_world(&default_world_def());
    world_set_gravity(&mut world, vec3(0.0, -10.0, 0.0));

    let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.5 };
    let mut shape_def = default_shape_def();
    shape_def.density = 1.0;

    for i in 0..4 {
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.position = pos((i * 10) as f32, 50.0, 0.0);
        let body_id = create_body(&mut world, &body_def);
        create_sphere_shape(&mut world, body_id, &shape_def, &sphere);
    }

    for _ in 0..10 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    world_start_recording(&mut world);
    for _ in 0..30 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    let rec = world_stop_recording(&mut world).expect("recording active");
    destroy_world(world);

    ensure!(validate_replay(rec.data(), 1));
}

// C: MidStreamContacts — snapshot with warm-start manifolds, islands, colors.
#[test]
fn mid_stream_contacts() {
    let mut world = create_world(&default_world_def());
    world_set_gravity(&mut world, vec3(0.0, -10.0, 0.0));

    let ground_id = create_body(&mut world, &default_body_def());
    let ground_box = make_box_hull(50.0, 1.0, 50.0);
    create_hull_shape(&mut world, ground_id, &default_shape_def(), &ground_box);

    let mut dynamic_shape = default_shape_def();
    dynamic_shape.density = 1.0;

    for i in 0..3 {
        let bx = make_box_hull(0.5, 0.5, 0.5);
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.position = pos((i * 2) as f32 - 2.0, 5.0, 0.0);
        let body_id = create_body(&mut world, &body_def);
        create_hull_shape(&mut world, body_id, &dynamic_shape, &bx);
    }

    // Let the scene settle: manifolds, islands, graph colors.
    for _ in 0..60 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    world_start_recording(&mut world);
    for _ in 0..30 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    let rec = world_stop_recording(&mut world).expect("recording active");
    destroy_world(world);

    ensure!(validate_replay(rec.data(), 1));
}

// C: ScrubBackward — forward pass recording per-frame hashes, then backward
// seeks that must reproduce each recorded hash exactly.
#[test]
fn scrub_backward() {
    let mut world = create_world(&default_world_def());
    world_set_gravity(&mut world, vec3(0.0, -10.0, 0.0));
    world_start_recording(&mut world);

    let ground_id = create_body(&mut world, &default_body_def());
    let ground_box = make_box_hull(20.0, 1.0, 20.0);
    create_hull_shape(&mut world, ground_id, &default_shape_def(), &ground_box);

    let mut box_shape = default_shape_def();
    box_shape.density = 1.0;
    for i in 0..4 {
        let bx = make_box_hull(0.5, 0.5, 0.5);
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.position = pos(0.0, 2.0 + i as f32 * 1.5, 0.0);
        let body_id = create_body(&mut world, &body_def);
        create_hull_shape(&mut world, body_id, &box_shape, &bx);
    }

    let total_frames = 80;
    for _ in 0..total_frames {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    let rec = world_stop_recording(&mut world).expect("recording active");
    destroy_world(world);

    let mut player = Player::create(rec.data(), 1).expect("player");
    ensure!(player.get_frame_count() == total_frames);

    // Forward pass: record per-frame hashes.
    let mut hashes = vec![0u64; (total_frames + 1) as usize];
    while !player.is_at_end() {
        player.step_frame();
        let f = player.get_frame();
        if f <= total_frames {
            hashes[f as usize] = hash_world_state(player.world());
        }
    }
    ensure!(player.get_frame() == total_frames);
    ensure!(!player.has_diverged());

    // Backward seeks must land exactly and reproduce the recorded hash.
    let seek_targets = [total_frames, total_frames / 2, 5, total_frames - 1, 0, 1];
    for &target in seek_targets.iter() {
        player.seek_frame(target);
        ensure!(player.get_frame() == target);
        ensure!(!player.has_diverged());
        if target > 0 {
            ensure!(hash_world_state(player.world()) == hashes[target as usize]);
        }
    }

    player.destroy();
}

// C: SeekWithHull — seek across keyframes with shared custom hulls; the
// keyframe capture path re-serializes the world against the pre-seeded
// registry, which must not grow.
#[test]
fn seek_with_hull() {
    let pts = [
        vec3(-1.0, -1.0, -1.0),
        vec3(1.0, -1.0, -1.0),
        vec3(1.0, 1.0, -1.0),
        vec3(-1.0, 1.0, -1.0),
        vec3(-1.0, -1.0, 1.0),
        vec3(1.0, -1.0, 1.0),
        vec3(1.0, 1.0, 1.0),
        vec3(-1.0, 1.0, 1.0),
    ];
    let hull = create_hull(&pts, 8).expect("hull");

    let mut world = create_world(&default_world_def());
    world_set_gravity(&mut world, vec3(0.0, -10.0, 0.0));
    world_start_recording(&mut world);

    let ground_id = create_body(&mut world, &default_body_def());
    let ground_box = make_box_hull(20.0, 1.0, 20.0);
    create_hull_shape(&mut world, ground_id, &default_shape_def(), &ground_box);

    let mut sd = default_shape_def();
    sd.density = 1.0;
    for i in 0..3 {
        let mut bd = default_body_def();
        bd.body_type = BodyType::Dynamic;
        bd.position = pos((i * 4) as f32 - 4.0, 5.0, 0.0);
        let body_id = create_body(&mut world, &bd);
        create_hull_shape(&mut world, body_id, &sd, &hull);
    }

    let total_frames = 40;
    for _ in 0..total_frames {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    let rec = world_stop_recording(&mut world).expect("recording active");
    destroy_world(world);

    let mut player = Player::create(rec.data(), 1).expect("player");
    while !player.is_at_end() {
        player.step_frame();
    }
    ensure!(!player.has_diverged());

    let mid_frame = total_frames / 2;
    player.seek_frame(mid_frame);
    ensure!(player.get_frame() == mid_frame);
    ensure!(!player.has_diverged());

    player.seek_frame(0);
    ensure!(player.get_frame() == 0);

    player.destroy();
}

// C: PlayerAccessors — recording info, creation-ordinal body tracking (seeded
// from the snapshot), divergence frame, keyframe policy.
#[test]
fn player_accessors() {
    let mut world = create_world(&default_world_def());
    world_set_gravity(&mut world, vec3(0.0, -10.0, 0.0));

    // Static ground (creation ordinal 0)
    let ground_id = create_body(&mut world, &default_body_def());
    let ground_box = make_box_hull(20.0, 1.0, 20.0);
    create_hull_shape(&mut world, ground_id, &default_shape_def(), &ground_box);

    // Four dynamic boxes (ordinals 1..4)
    let dynamic_count = 4;
    let mut box_shape = default_shape_def();
    box_shape.density = 1.0;
    for i in 0..dynamic_count {
        let bx = make_box_hull(0.5, 0.5, 0.5);
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.position = pos(0.0, 2.0 + i as f32 * 1.5, 0.0);
        let body_id = create_body(&mut world, &body_def);
        create_hull_shape(&mut world, body_id, &box_shape, &bx);
    }

    let time_step = 1.0 / 60.0;
    let sub_step_count = 4;

    // Settle, then record with a snapshot of the populated world.
    for _ in 0..10 {
        world_step(&mut world, time_step, sub_step_count);
    }

    world_start_recording(&mut world);
    let total_frames = 80;
    for _ in 0..total_frames {
        world_step(&mut world, time_step, sub_step_count);
    }
    let rec = world_stop_recording(&mut world).expect("recording active");
    destroy_world(world);

    let mut player = Player::create(rec.data(), 1).expect("player");

    // Info reflects the recorded tuning and a non-degenerate bounds.
    let info = player.get_info();
    ensure!(info.frame_count == total_frames);
    ensure!(info.sub_step_count == sub_step_count);
    ensure!(info.time_step > 0.0);
    let extent = makepad_box3d::math_functions::sub(info.bounds.upper_bound, info.bounds.lower_bound);
    ensure!(extent.x > 0.0 && extent.y > 0.0 && extent.z > 0.0);

    // Body ordinals: ground + 4 dynamic, seeded from the snapshot.
    ensure!(player.get_body_count() == 1 + dynamic_count);
    let ground = player.get_body_id(0);
    ensure!(ground != NULL_BODY_ID);
    ensure!(body_get_type(player.world(), ground) == BodyType::Static);
    for i in 1..=dynamic_count {
        let id = player.get_body_id(i);
        ensure!(id != NULL_BODY_ID);
        ensure!(body_get_type(player.world(), id) == BodyType::Dynamic);
    }
    ensure!(player.get_body_id(1 + dynamic_count) == NULL_BODY_ID);

    // No divergence on a clean serial replay.
    player.seek_frame(total_frames);
    ensure!(!player.has_diverged());
    ensure!(player.get_diverge_frame() == -1);

    // Ordinals survive a backward seek that restores from a keyframe.
    let before = player.get_body_id(2);
    player.seek_frame(total_frames / 2);
    player.seek_frame(total_frames);
    let after = player.get_body_id(2);
    ensure!(before == after);

    // Keyframe policy: defaults present, setter takes effect, ring cleared.
    ensure!(player.get_keyframe_min_interval() == 16);
    player.set_keyframe_policy(256 * 1024 * 1024, 8);
    ensure!(player.get_keyframe_min_interval() == 8);
    ensure!(player.get_keyframe_interval() == 8);
    ensure!(player.get_keyframe_budget() == 256 * 1024 * 1024);
    ensure!(player.get_keyframe_bytes() == 0);

    player.destroy();
}

// C: QueryReplay — all seven world queries each frame; a clean replay proves
// the queries reproduce; the per-frame store surfaces all seven.
#[test]
fn query_replay() {
    let mut world = create_world(&default_world_def());
    world_set_gravity(&mut world, vec3(0.0, -10.0, 0.0));

    let ground_id = create_body(&mut world, &default_body_def());
    let ground_box = make_box_hull(20.0, 1.0, 20.0);
    create_hull_shape(&mut world, ground_id, &default_shape_def(), &ground_box);

    // A few dynamic spheres for the queries to find.
    for i in 0..4 {
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.position = pos(i as f32 - 1.5, 3.0, 0.0);
        let body_id = create_body(&mut world, &body_def);
        let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.5 };
        let mut sphere_def = default_shape_def();
        sphere_def.density = 1.0;
        create_sphere_shape(&mut world, body_id, &sphere_def, &sphere);
    }

    world_start_recording(&mut world);

    let filter = default_query_filter();
    let total_frames = 30;
    for _ in 0..total_frames {
        let origin = pos(0.0, 6.0, 0.0);
        let translation = vec3(0.0, -8.0, 0.0);
        let aabb = AABB { lower_bound: vec3(-5.0, -1.0, -5.0), upper_bound: vec3(5.0, 6.0, 5.0) };

        let proxy_pts = [vec3(0.0, 0.0, 0.0)];
        let proxy = ShapeProxy { points: &proxy_pts, radius: 0.5 };
        let mover = Capsule { center1: vec3(0.0, 0.0, 0.0), center2: vec3(0.0, 1.0, 0.0), radius: 0.3 };

        world_overlap_aabb(&mut world, aabb, filter, &mut overlap_fcn());
        world_overlap_shape(&world, origin, &proxy, filter, &mut overlap_fcn());
        world_cast_ray(&world, origin, translation, filter, &mut cast_fcn());
        world_cast_ray_closest(&world, origin, translation, filter);
        world_cast_shape(&world, origin, &proxy, translation, filter, &mut cast_fcn());
        world_cast_mover(&world, origin, &mover, translation, filter, Some(&mut overlap_fcn()));
        world_collide_mover(&world, origin, &mover, filter, &mut plane_fcn());

        world_step(&mut world, 1.0 / 60.0, 4);
    }

    let rec = world_stop_recording(&mut world).expect("recording active");
    destroy_world(world);

    // Headless validation re-issues every recorded query and compares.
    ensure!(validate_replay(rec.data(), 1));

    // Player path: the per-frame store holds all seven at a mid frame.
    let mut player = Player::create(rec.data(), 1).expect("player");
    player.seek_frame(15);
    ensure!(!player.has_diverged());
    ensure!(player.get_frame_query_count() == 7);

    let first = player.get_frame_query(0);
    ensure!(first.kind == RecQueryKind::OverlapAabb);

    // The ray cast finds at least the ground: non-empty recorded hit list.
    let mut saw_cast_ray = false;
    for qi in 0..player.get_frame_query_count() {
        let info = player.get_frame_query(qi);
        if info.kind == RecQueryKind::CastRay {
            saw_cast_ray = true;
            ensure!(info.hit_count > 0);
        }
    }
    ensure!(saw_cast_ray);

    player.destroy();
}

// C: TaggedQuery — caller (id, label) keys ride the QueryTag op and intern in
// the trailing tag table; distinct ids under one label are distinct keys; all
// survive a file round trip; untagged queries report key 0 / id 0 / no name.
#[test]
fn tagged_query() {
    let mut world = create_world(&default_world_def());

    let ground_id = create_body(&mut world, &default_body_def());
    let ground_box = make_box_hull(20.0, 1.0, 20.0);
    create_hull_shape(&mut world, ground_id, &default_shape_def(), &ground_box);

    world_start_recording(&mut world);

    let mut bullet53 = default_query_filter();
    bullet53.id = 53;
    bullet53.name = "bullet";

    let mut bullet54 = default_query_filter();
    bullet54.id = 54;
    bullet54.name = "bullet";

    let untagged = default_query_filter();

    let key53 = hash_query_tag(53, "bullet");
    let key54 = hash_query_tag(54, "bullet");
    ensure!(key53 != 0 && key54 != 0 && key53 != key54);

    let total_frames = 10;
    for _ in 0..total_frames {
        let origin = pos(0.0, 6.0, 0.0);
        let translation = vec3(0.0, -8.0, 0.0);
        let aabb = AABB { lower_bound: vec3(-5.0, -1.0, -5.0), upper_bound: vec3(5.0, 6.0, 5.0) };

        world_cast_ray(&world, origin, translation, bullet53, &mut cast_fcn());
        world_cast_ray(&world, origin, translation, bullet54, &mut cast_fcn());
        world_overlap_aabb(&mut world, aabb, untagged, &mut overlap_fcn());

        world_step(&mut world, 1.0 / 60.0, 4);
    }

    let rec = world_stop_recording(&mut world).expect("recording active");
    destroy_world(world);

    ensure!(validate_replay(rec.data(), 1));

    // Round trip through a file so the interned tag table is exercised on the
    // persisted bytes.
    let path = std::env::temp_dir().join("box3d_tagged_query_test.b3rc");
    let path = path.to_str().unwrap();
    ensure!(save_recording_to_file(&rec, path));
    let loaded = load_recording_from_file(path).expect("load");

    let mut player = Player::create(loaded.data(), 1).expect("player");
    player.seek_frame(5);
    ensure!(!player.has_diverged());
    ensure!(player.get_frame_query_count() == 3);

    let (mut saw53, mut saw54, mut saw_untagged) = (false, false, false);
    for qi in 0..player.get_frame_query_count() {
        let info = player.get_frame_query(qi);
        if info.key == key53 {
            saw53 = true;
            ensure!(info.id == 53 && info.name.as_deref() == Some("bullet"));
        } else if info.key == key54 {
            saw54 = true;
            ensure!(info.id == 54 && info.name.as_deref() == Some("bullet"));
        } else {
            saw_untagged = true;
            ensure!(info.key == 0 && info.id == 0 && info.name.is_none());
        }
    }
    ensure!(saw53 && saw54 && saw_untagged);

    player.destroy();
    let _ = std::fs::remove_file(path);
}

// C: TransformedHullRoundTrip — a transformed hull bakes transform + scale at
// create time; it must record like any other shape create or every later
// shape id drifts on replay.
#[test]
fn transformed_hull_round_trip() {
    let pts = [
        vec3(-1.0, -1.0, -1.0),
        vec3(1.0, -1.0, -1.0),
        vec3(1.0, 1.0, -1.0),
        vec3(-1.0, 1.0, -1.0),
        vec3(-1.0, -1.0, 1.0),
        vec3(1.0, -1.0, 1.0),
        vec3(1.0, 1.0, 1.0),
        vec3(-1.0, 1.0, 1.0),
    ];
    let hull = create_hull(&pts, 8).expect("hull");

    let mut world = create_world(&default_world_def());
    world_start_recording(&mut world);

    let mut shape_def = default_shape_def();
    shape_def.density = 1.0;

    let xf = Transform {
        p: vec3(0.25, 0.0, -0.5),
        q: make_quat_from_axis_angle(vec3(0.0, 0.0, 1.0), 0.3),
    };
    let scl = vec3(1.5, 0.5, 2.0);
    for i in 0..3 {
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.position = pos((i * 3) as f32, 5.0, 0.0);
        let body_id = create_body(&mut world, &body_def);
        let sid = create_transformed_hull_shape(&mut world, body_id, &shape_def, &hull, xf, scl);
        ensure!(sid.index1 != 0);
    }

    // A plain hull after the transformed ones: a desynced id pool would make
    // this shape's recorded id mismatch on replay.
    {
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.position = pos(0.0, 10.0, 0.0);
        let body_id = create_body(&mut world, &body_def);
        let sid = create_hull_shape(&mut world, body_id, &shape_def, &hull);
        ensure!(sid.index1 != 0);
    }

    // Step past the keyframe interval so replay captures a keyframe, which
    // re-serializes the live world against the pre-seeded registry.
    for _ in 0..20 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    let rec = world_stop_recording(&mut world).expect("recording active");
    destroy_world(world);

    ensure!(validate_replay(rec.data(), 1));
    ensure!(validate_replay(rec.data(), 4));
}

// C: AllOps — every recorded op in one session; validate at two worker
// counts, round trip through a file, drive the incremental player.
#[test]
fn all_ops() {
    let mut world_def = default_world_def();
    world_def.worker_count = 1;
    let mut world = create_world(&world_def);

    world_start_recording(&mut world);

    // Static ground with a box-hull shape
    let ground_id = create_body(&mut world, &default_body_def());
    let ground_box = make_box_hull(50.0, 1.0, 50.0);
    let ground_shape_id = create_hull_shape(&mut world, ground_id, &default_shape_def(), &ground_box);
    ensure!(ground_shape_id.index1 != 0);

    // Dynamic body with a sphere shape. Name intentionally longer than
    // BODY_NAME_LENGTH so replay exercises the over-length name path.
    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    body_def.position = pos(0.0, 5.0, 0.0);
    body_def.name = String::from("testBodyWithVeryLongNameThatExceedsTheNameLength");
    let body_id = create_body(&mut world, &body_def);

    let mut sphere_shape_def = default_shape_def();
    sphere_shape_def.density = 1.0;
    let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.5 };
    let sphere_shape_id = create_sphere_shape(&mut world, body_id, &sphere_shape_def, &sphere);
    ensure!(sphere_shape_id.index1 != 0);

    // Capsule shape on a second dynamic body
    let mut capsule_body_def = default_body_def();
    capsule_body_def.body_type = BodyType::Dynamic;
    capsule_body_def.position = pos(3.0, 5.0, 0.0);
    let capsule_body_id = create_body(&mut world, &capsule_body_def);

    let mut capsule_shape_def = default_shape_def();
    capsule_shape_def.density = 1.0;
    let capsule = Capsule { center1: vec3(0.0, -0.4, 0.0), center2: vec3(0.0, 0.4, 0.0), radius: 0.25 };
    let capsule_shape_id = create_capsule_shape(&mut world, capsule_body_id, &capsule_shape_def, &capsule);
    ensure!(capsule_shape_id.index1 != 0);

    // Custom hull shape on a third dynamic body
    let hull_pts = [
        vec3(-0.5, -0.5, -0.5),
        vec3(0.5, -0.5, -0.5),
        vec3(0.5, 0.5, -0.5),
        vec3(-0.5, 0.5, -0.5),
        vec3(-0.5, -0.5, 0.5),
        vec3(0.5, -0.5, 0.5),
        vec3(0.5, 0.5, 0.5),
        vec3(-0.5, 0.5, 0.5),
    ];
    let custom_hull = create_hull(&hull_pts, 8).expect("hull");

    let mut hull_body_def = default_body_def();
    hull_body_def.body_type = BodyType::Dynamic;
    hull_body_def.position = pos(-3.0, 5.0, 0.0);
    let hull_body_id = create_body(&mut world, &hull_body_def);

    let mut hull_shape_def = default_shape_def();
    hull_shape_def.density = 1.0;
    let hull_shape_id = create_hull_shape(&mut world, hull_body_id, &hull_shape_def, &custom_hull);
    ensure!(hull_shape_id.index1 != 0);

    // Box hull shape on a fourth dynamic body
    let mut box_body_def = default_body_def();
    box_body_def.body_type = BodyType::Dynamic;
    box_body_def.position = pos(6.0, 5.0, 0.0);
    let box_body_id = create_body(&mut world, &box_body_def);

    let mut box_shape_def = default_shape_def();
    box_shape_def.density = 2.0;
    let box_hull = make_box_hull(0.5, 0.5, 0.5);
    let box_shape_id = create_hull_shape(&mut world, box_body_id, &box_shape_def, &box_hull);
    ensure!(box_shape_id.index1 != 0);

    // Transformed hull shape on a fifth dynamic body
    let mut xform_body_def = default_body_def();
    xform_body_def.body_type = BodyType::Dynamic;
    xform_body_def.position = pos(12.0, 5.0, 0.0);
    let xform_body_id = create_body(&mut world, &xform_body_def);
    let mut xform_shape_def = default_shape_def();
    xform_shape_def.density = 1.0;
    let xform_xf = Transform {
        p: vec3(0.1, 0.2, -0.1),
        q: make_quat_from_axis_angle(vec3(0.0, 1.0, 0.0), 0.4),
    };
    let xform_shape_id = create_transformed_hull_shape(
        &mut world,
        xform_body_id,
        &xform_shape_def,
        &custom_hull,
        xform_xf,
        vec3(1.25, 0.75, 1.5),
    );
    ensure!(xform_shape_id.index1 != 0);

    // Mesh, height field, and compound static shapes (3D-only)
    let mut mesh_body_def = default_body_def();
    mesh_body_def.position = pos(20.0, 0.0, 0.0);
    let mesh_body_id = create_body(&mut world, &mesh_body_def);
    let mesh_data = create_grid_mesh(3, 3, 2.0, 0, false);
    create_mesh_shape(&mut world, mesh_body_id, &default_shape_def(), &mesh_data, vec3(1.0, 1.0, 1.0));

    let mut hf_body_def = default_body_def();
    hf_body_def.position = pos(-20.0, 0.0, 0.0);
    let hf_body_id = create_body(&mut world, &hf_body_def);
    let hf = create_grid(4, 4, vec3(2.0, 1.0, 2.0), false);
    create_height_field_shape(&mut world, hf_body_id, &default_shape_def(), &hf);

    let mut compound_body_def = default_body_def();
    compound_body_def.position = pos(30.0, 0.0, 0.0);
    let compound_body_id = create_body(&mut world, &compound_body_def);
    let compound_def = CompoundDef {
        spheres: vec![CompoundSphereDef {
            sphere: Sphere { center: vec3(0.0, 0.0, 0.0), radius: 1.0 },
            material: default_surface_material(),
        }],
        ..Default::default()
    };
    let compound = create_compound(&compound_def);
    create_compound_shape(&mut world, compound_body_id, &default_shape_def(), &compound);

    // Throwaway shape to exercise DestroyShape
    let tmp_sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.1 };
    let tmp_shape_id = create_sphere_shape(&mut world, capsule_body_id, &capsule_shape_def, &tmp_sphere);
    destroy_shape(&mut world, tmp_shape_id, true);

    // Shape mutators
    shape_set_friction(&mut world, box_shape_id, 0.3);
    shape_set_restitution(&mut world, capsule_shape_id, 0.5);
    shape_set_density(&mut world, box_shape_id, 3.0, true);
    let mut surf_mat = default_surface_material();
    surf_mat.friction = 0.7;
    surf_mat.restitution = 0.1;
    shape_set_surface_material(&mut world, capsule_shape_id, surf_mat);
    let mut shape_filter = default_filter();
    shape_filter.category_bits = 0x2;
    shape_set_filter(&mut world, box_shape_id, shape_filter, false);
    shape_enable_sensor_events(&mut world, capsule_shape_id, true);
    shape_enable_contact_events(&mut world, capsule_shape_id, true);
    shape_enable_hit_events(&mut world, box_shape_id, true);
    shape_enable_pre_solve_events(&mut world, box_shape_id, true);
    shape_apply_wind(&mut world, capsule_shape_id, vec3(1.0, 0.0, 0.0), 0.1, 0.0, 10.0, true);
    let new_sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.45 };
    shape_set_sphere(&mut world, sphere_shape_id, &new_sphere);
    let new_capsule = Capsule { center1: vec3(0.0, -0.3, 0.0), center2: vec3(0.0, 0.3, 0.0), radius: 0.3 };
    shape_set_capsule(&mut world, capsule_shape_id, &new_capsule);

    // Body mutators
    body_set_transform(&mut world, body_id, pos(1.0, 6.0, 0.0), Quat::IDENTITY);
    body_set_linear_velocity(&mut world, body_id, vec3(0.5, 0.0, 0.0));
    body_set_angular_velocity(&mut world, body_id, vec3(0.0, 0.25, 0.0));
    body_set_name(&mut world, body_id, "renamedBody");
    body_set_linear_damping(&mut world, body_id, 0.1);
    body_set_angular_damping(&mut world, body_id, 0.05);
    body_set_gravity_scale(&mut world, body_id, 0.9);
    body_set_sleep_threshold(&mut world, body_id, 0.02);
    body_enable_sleep(&mut world, body_id, false);
    body_set_bullet(&mut world, body_id, true);
    body_enable_contact_recycling(&mut world, body_id, false);
    body_enable_hit_events(&mut world, body_id, true);
    body_set_motion_locks(
        &mut world,
        body_id,
        MotionLocks {
            linear_x: false,
            linear_y: false,
            linear_z: false,
            angular_x: false,
            angular_y: false,
            angular_z: true,
        },
    );
    let mass_data = MassData { mass: 2.0, center: vec3(0.0, 0.0, 0.0), inertia: Matrix3::IDENTITY };
    body_set_mass_data(&mut world, body_id, mass_data);
    body_apply_mass_from_shapes(&mut world, body_id);
    body_set_type(&mut world, capsule_body_id, BodyType::Kinematic);
    body_set_type(&mut world, capsule_body_id, BodyType::Dynamic);
    body_set_awake(&mut world, body_id, true);

    // Kinematic body to exercise SetTargetTransform
    let mut kinematic_def = default_body_def();
    kinematic_def.body_type = BodyType::Kinematic;
    kinematic_def.position = pos(-6.0, 5.0, 0.0);
    let kinematic_id = create_body(&mut world, &kinematic_def);
    let kin_box = make_box_hull(0.4, 0.4, 0.4);
    create_hull_shape(&mut world, kinematic_id, &default_shape_def(), &kin_box);
    let kin_target = WorldTransform { p: pos(-5.0, 5.0, 0.0), q: Quat::IDENTITY };
    body_set_target_transform(&mut world, kinematic_id, kin_target, 1.0 / 60.0, true);

    // Body to exercise Disable/Enable
    let mut disable_def = default_body_def();
    disable_def.body_type = BodyType::Dynamic;
    disable_def.position = pos(9.0, 5.0, 0.0);
    let disable_id = create_body(&mut world, &disable_def);
    let disable_sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.3 };
    create_sphere_shape(&mut world, disable_id, &sphere_shape_def, &disable_sphere);
    body_disable(&mut world, disable_id);
    body_enable(&mut world, disable_id);

    // Force/impulse/torque
    body_apply_force(&mut world, body_id, vec3(0.0, 50.0, 0.0), pos(1.0, 6.0, 0.0), true);
    body_apply_force_to_center(&mut world, body_id, vec3(5.0, 0.0, 0.0), true);
    body_apply_torque(&mut world, body_id, vec3(0.0, 1.0, 0.0), true);
    body_apply_linear_impulse(&mut world, body_id, vec3(0.1, 0.0, 0.0), pos(1.0, 6.0, 0.0), true);
    body_apply_linear_impulse_to_center(&mut world, body_id, vec3(0.0, 0.1, 0.0), true);
    body_apply_angular_impulse(&mut world, body_id, vec3(0.0, 0.05, 0.0), true);

    // Joint bodies: a row of dynamic bodies connected by each joint type
    let mut jb = Vec::new();
    for i in 0..9 {
        let mut jbd = default_body_def();
        jbd.body_type = BodyType::Dynamic;
        jbd.position = pos(-8.0 + i as f32 * 2.0, 10.0, 0.0);
        let jid = create_body(&mut world, &jbd);
        let js = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.25 };
        let mut jsd = default_shape_def();
        jsd.density = 1.0;
        create_sphere_shape(&mut world, jid, &jsd, &js);
        jb.push(jid);
    }

    // Revolute joint with full setter coverage and the generic mutators
    let mut rev_def = default_revolute_joint_def();
    rev_def.base.body_id_a = jb[0];
    rev_def.base.body_id_b = jb[1];
    rev_def.base.local_frame_a.p = vec3(1.0, 0.0, 0.0);
    rev_def.base.local_frame_b.p = vec3(-1.0, 0.0, 0.0);
    let rev_id = create_revolute_joint(&mut world, &rev_def);
    ensure!(rev_id.index1 != 0);
    revolute_joint_enable_limit(&mut world, rev_id, true);
    revolute_joint_set_limits(&mut world, rev_id, -1.0, 1.0);
    revolute_joint_enable_motor(&mut world, rev_id, true);
    revolute_joint_set_motor_speed(&mut world, rev_id, 0.5);
    revolute_joint_set_max_motor_torque(&mut world, rev_id, 10.0);
    revolute_joint_enable_spring(&mut world, rev_id, true);
    revolute_joint_set_spring_hertz(&mut world, rev_id, 2.0);
    revolute_joint_set_spring_damping_ratio(&mut world, rev_id, 0.5);
    revolute_joint_set_target_angle(&mut world, rev_id, 0.25);
    joint_set_local_frame_a(&mut world, rev_id, Transform { p: vec3(1.0, 0.0, 0.0), q: Quat::IDENTITY });
    joint_set_local_frame_b(&mut world, rev_id, Transform { p: vec3(-1.0, 0.0, 0.0), q: Quat::IDENTITY });
    joint_set_constraint_tuning(&mut world, rev_id, 60.0, 2.0);
    joint_set_force_threshold(&mut world, rev_id, 100.0);
    joint_set_torque_threshold(&mut world, rev_id, 50.0);
    joint_set_collide_connected(&mut world, rev_id, false);
    joint_wake_bodies(&mut world, rev_id);

    // Distance joint
    let mut dist_def = default_distance_joint_def();
    dist_def.base.body_id_a = jb[1];
    dist_def.base.body_id_b = jb[2];
    dist_def.length = 2.0;
    let dist_id = create_distance_joint(&mut world, &dist_def);
    distance_joint_set_length(&mut world, dist_id, 2.2);
    distance_joint_enable_spring(&mut world, dist_id, true);
    distance_joint_set_spring_hertz(&mut world, dist_id, 3.0);
    distance_joint_set_spring_damping_ratio(&mut world, dist_id, 0.4);
    distance_joint_set_spring_force_range(&mut world, dist_id, -50.0, 50.0);
    distance_joint_enable_limit(&mut world, dist_id, true);
    distance_joint_set_length_range(&mut world, dist_id, 1.0, 4.0);
    distance_joint_enable_motor(&mut world, dist_id, true);
    distance_joint_set_motor_speed(&mut world, dist_id, 0.3);
    distance_joint_set_max_motor_force(&mut world, dist_id, 5.0);

    // Filter joint (plus a throwaway to exercise DestroyJoint)
    let mut filter_def = default_filter_joint_def();
    filter_def.base.body_id_a = jb[2];
    filter_def.base.body_id_b = jb[3];
    let filter_id = create_filter_joint(&mut world, &filter_def);
    ensure!(filter_id.index1 != 0);

    let mut tmp_joint_def = default_distance_joint_def();
    tmp_joint_def.base.body_id_a = jb[0];
    tmp_joint_def.base.body_id_b = jb[8];
    tmp_joint_def.length = 5.0;
    let tmp_joint_id = create_distance_joint(&mut world, &tmp_joint_def);
    destroy_joint(&mut world, tmp_joint_id, true);

    // Motor joint
    let mut motor_def = default_motor_joint_def();
    motor_def.base.body_id_a = jb[3];
    motor_def.base.body_id_b = jb[4];
    let motor_id = create_motor_joint(&mut world, &motor_def);
    motor_joint_set_linear_velocity(&mut world, motor_id, vec3(0.1, 0.0, 0.0));
    motor_joint_set_angular_velocity(&mut world, motor_id, vec3(0.0, 0.2, 0.0));
    motor_joint_set_max_velocity_force(&mut world, motor_id, 10.0);
    motor_joint_set_max_velocity_torque(&mut world, motor_id, 10.0);
    motor_joint_set_linear_hertz(&mut world, motor_id, 2.0);
    motor_joint_set_linear_damping_ratio(&mut world, motor_id, 0.5);
    motor_joint_set_angular_hertz(&mut world, motor_id, 2.0);
    motor_joint_set_angular_damping_ratio(&mut world, motor_id, 0.5);
    motor_joint_set_max_spring_force(&mut world, motor_id, 20.0);
    motor_joint_set_max_spring_torque(&mut world, motor_id, 20.0);

    // Prismatic joint
    let mut pris_def = default_prismatic_joint_def();
    pris_def.base.body_id_a = jb[4];
    pris_def.base.body_id_b = jb[5];
    let pris_id = create_prismatic_joint(&mut world, &pris_def);
    prismatic_joint_enable_spring(&mut world, pris_id, true);
    prismatic_joint_set_spring_hertz(&mut world, pris_id, 2.0);
    prismatic_joint_set_spring_damping_ratio(&mut world, pris_id, 0.5);
    prismatic_joint_set_target_translation(&mut world, pris_id, 0.1);
    prismatic_joint_enable_limit(&mut world, pris_id, true);
    prismatic_joint_set_limits(&mut world, pris_id, -1.0, 1.0);
    prismatic_joint_enable_motor(&mut world, pris_id, true);
    prismatic_joint_set_motor_speed(&mut world, pris_id, 0.2);
    prismatic_joint_set_max_motor_force(&mut world, pris_id, 8.0);

    // Spherical joint (3D-only)
    let mut sph_def = default_spherical_joint_def();
    sph_def.base.body_id_a = jb[5];
    sph_def.base.body_id_b = jb[6];
    let sph_id = create_spherical_joint(&mut world, &sph_def);
    spherical_joint_enable_cone_limit(&mut world, sph_id, true);
    spherical_joint_set_cone_limit(&mut world, sph_id, 0.5);
    spherical_joint_enable_twist_limit(&mut world, sph_id, true);
    spherical_joint_set_twist_limits(&mut world, sph_id, -0.3, 0.3);
    spherical_joint_enable_spring(&mut world, sph_id, true);
    spherical_joint_set_spring_hertz(&mut world, sph_id, 3.0);
    spherical_joint_set_spring_damping_ratio(&mut world, sph_id, 0.5);
    spherical_joint_set_target_rotation(&mut world, sph_id, Quat::IDENTITY);
    spherical_joint_enable_motor(&mut world, sph_id, true);
    spherical_joint_set_motor_velocity(&mut world, sph_id, vec3(0.0, 0.1, 0.0));
    spherical_joint_set_max_motor_torque(&mut world, sph_id, 5.0);

    // Weld joint
    let mut weld_def = default_weld_joint_def();
    weld_def.base.body_id_a = jb[6];
    weld_def.base.body_id_b = jb[7];
    let weld_id = create_weld_joint(&mut world, &weld_def);
    weld_joint_set_linear_hertz(&mut world, weld_id, 5.0);
    weld_joint_set_linear_damping_ratio(&mut world, weld_id, 0.6);
    weld_joint_set_angular_hertz(&mut world, weld_id, 5.0);
    weld_joint_set_angular_damping_ratio(&mut world, weld_id, 0.6);

    // Wheel joint
    let mut wheel_def = default_wheel_joint_def();
    wheel_def.base.body_id_a = jb[7];
    wheel_def.base.body_id_b = jb[8];
    let wheel_id = create_wheel_joint(&mut world, &wheel_def);
    wheel_joint_enable_suspension(&mut world, wheel_id, true);
    wheel_joint_set_suspension_hertz(&mut world, wheel_id, 4.0);
    wheel_joint_set_suspension_damping_ratio(&mut world, wheel_id, 0.7);
    wheel_joint_enable_suspension_limit(&mut world, wheel_id, true);
    wheel_joint_set_suspension_limits(&mut world, wheel_id, -0.5, 0.5);
    wheel_joint_enable_spin_motor(&mut world, wheel_id, true);
    wheel_joint_set_spin_motor_speed(&mut world, wheel_id, 1.0);
    wheel_joint_set_max_spin_torque(&mut world, wheel_id, 6.0);
    wheel_joint_enable_steering(&mut world, wheel_id, true);
    wheel_joint_set_steering_hertz(&mut world, wheel_id, 2.0);
    wheel_joint_set_steering_damping_ratio(&mut world, wheel_id, 0.5);
    wheel_joint_set_max_steering_torque(&mut world, wheel_id, 3.0);
    wheel_joint_enable_steering_limit(&mut world, wheel_id, true);
    wheel_joint_set_steering_limits(&mut world, wheel_id, -0.5, 0.5);
    wheel_joint_set_target_steering_angle(&mut world, wheel_id, 0.1);

    // Parallel joint (3D-only)
    let mut parallel_def = default_parallel_joint_def();
    parallel_def.base.body_id_a = ground_id;
    parallel_def.base.body_id_b = body_id;
    let parallel_id = create_parallel_joint(&mut world, &parallel_def);
    parallel_joint_set_spring_hertz(&mut world, parallel_id, 2.0);
    parallel_joint_set_spring_damping_ratio(&mut world, parallel_id, 0.5);
    parallel_joint_set_max_torque(&mut world, parallel_id, 20.0);

    // World config mutators
    world_set_gravity(&mut world, vec3(0.0, -9.8, 0.0));
    world_enable_sleeping(&mut world, true);
    world_enable_continuous(&mut world, false);
    world_enable_warm_starting(&mut world, true);
    world_enable_speculative(&mut world, true);
    world_set_restitution_threshold(&mut world, 1.5);
    world_set_hit_event_threshold(&mut world, 2.0);
    world_set_contact_tuning(&mut world, 30.0, 10.0, 3.0);
    world_set_contact_recycle_distance(&mut world, 0.05);
    world_set_maximum_linear_speed(&mut world, 100.0);
    world_rebuild_static_tree(&mut world);

    let mut explosion = default_explosion_def();
    explosion.position = pos(0.0, 5.0, 0.0);
    explosion.radius = 3.0;
    explosion.falloff = 1.0;
    explosion.impulse_per_area = 2.0;
    world_explode(&mut world, &explosion);

    // Pre-step queries (all seven kinds)
    let qfilter = default_query_filter();
    let qaabb = AABB { lower_bound: vec3(-10.0, -5.0, -10.0), upper_bound: vec3(10.0, 15.0, 10.0) };
    world_overlap_aabb(&mut world, qaabb, qfilter, &mut overlap_fcn());
    let qorigin = pos(0.0, 15.0, 0.0);
    let proxy_pts = [vec3(0.0, 0.0, 0.0)];
    let proxy = ShapeProxy { points: &proxy_pts, radius: 0.5 };
    world_overlap_shape(&world, qorigin, &proxy, qfilter, &mut overlap_fcn());
    let q_translation = vec3(0.0, -20.0, 0.0);
    world_cast_ray(&world, qorigin, q_translation, qfilter, &mut cast_fcn());
    world_cast_ray_closest(&world, qorigin, q_translation, qfilter);
    world_cast_shape(&world, qorigin, &proxy, q_translation, qfilter, &mut cast_fcn());
    let mover = Capsule { center1: vec3(0.0, 0.0, 0.0), center2: vec3(0.0, 1.0, 0.0), radius: 0.3 };
    world_cast_mover(&world, qorigin, &mover, q_translation, qfilter, Some(&mut overlap_fcn()));
    world_collide_mover(&world, qorigin, &mover, qfilter, &mut plane_fcn());

    let time_step = 1.0 / 60.0;
    let sub_step_count = 4;
    for i in 0..12 {
        // Inject mutators mid-simulation
        if i == 6 {
            body_apply_linear_impulse_to_center(&mut world, capsule_body_id, vec3(2.0, 0.0, 0.0), true);
            body_set_gravity_scale(&mut world, body_id, 1.0);
        }

        // Issue queries mid-loop to exercise recording across steps
        if i == 3 {
            world_overlap_aabb(&mut world, qaabb, qfilter, &mut overlap_fcn());
            world_cast_ray(&world, qorigin, q_translation, qfilter, &mut cast_fcn());
        }

        world_step(&mut world, time_step, sub_step_count);
    }

    let rec = world_stop_recording(&mut world).expect("recording active");
    destroy_world(world);

    let rec_data = rec.data();
    ensure!(!rec_data.is_empty());

    // Replay headless at worker count 1 and 4 — cross-thread determinism.
    ensure!(validate_replay(rec_data, 1));
    ensure!(validate_replay(rec_data, 4));

    // File round trip
    let path = std::env::temp_dir().join("box3d_recording_allops_test.b3rc");
    let path = path.to_str().unwrap();
    ensure!(save_recording_to_file(&rec, path));
    let loaded = load_recording_from_file(path).expect("load");
    ensure!(validate_replay(loaded.data(), 1));

    // Drive the incremental player: per-frame stepping, restart, getters.
    {
        let mut player = Player::create(rec_data, 1).expect("player");

        let info = player.get_info();
        let rec_extents =
            makepad_box3d::math_functions::sub(info.bounds.upper_bound, info.bounds.lower_bound);
        ensure!(rec_extents.x > 0.0 && rec_extents.y > 0.0);

        let mut frames = 0;
        while player.step_frame() {
            frames += 1;
        }
        ensure!(frames == 12);
        ensure!(player.get_frame() == 12);
        ensure!(player.is_at_end());
        ensure!(!player.has_diverged());

        // The trailing DestroyWorld is an end marker; the world stays usable.
        ensure!(!player.world().bodies.is_empty());

        // Restart reproduces the same run without reloading the file.
        player.restart();
        ensure!(player.get_frame() == 0);
        ensure!(!player.is_at_end());

        let mut frames2 = 0;
        while player.step_frame() {
            frames2 += 1;
        }
        ensure!(frames2 == 12);
        ensure!(!player.has_diverged());

        player.destroy();
    }

    let _ = std::fs::remove_file(path);
}

// C: ReservedHeaderBytes — reserved header fields must not affect replay.
#[test]
fn reserved_header_bytes() {
    let mut world = create_world(&default_world_def());
    world_start_recording(&mut world);
    world_set_gravity(&mut world, vec3(0.0, -10.0, 0.0));

    let mut bd = default_body_def();
    bd.body_type = BodyType::Dynamic;
    bd.position = pos(0.0, 5.0, 0.0);
    let body_id = create_body(&mut world, &bd);
    let s = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.5 };
    let mut sd = default_shape_def();
    sd.density = 1.0;
    create_sphere_shape(&mut world, body_id, &sd, &s);

    for _ in 0..10 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    let rec = world_stop_recording(&mut world).expect("recording active");
    destroy_world(world);

    // Patch reserved fields at their byte offsets in the header:
    //   byte 11: reserved, bytes 16-19: reserved2, bytes 20-23: reserved3.
    let mut patched = rec.data().to_vec();
    ensure!(patched.len() >= REC_HEADER_SIZE);
    patched[11] = 0xAB;
    patched[16] = 0xCD;
    patched[17] = 0xEF;
    patched[18] = 0x12;
    patched[19] = 0x34;
    patched[20] = 0x56;
    patched[21] = 0x78;
    patched[22] = 0x9A;
    patched[23] = 0xBC;
    ensure!(validate_replay(&patched, 1));
}

// C: GeometryHashCollision — colliding content hashes must dedup exactly, and
// an already-seeded registry (byte-identical duplicate slots included) must
// resolve a live blob without growing.
#[test]
fn geometry_hash_collision() {
    // The content hash must use its full width: a one-byte change must perturb
    // the high word too.
    {
        let p = [0x11u8; 16];
        let mut q = [0x11u8; 16];
        q[7] = 0x12;
        let hp = hash64_blob(&p);
        let hq = hash64_blob(&q);
        ensure!(hp != hq);
        ensure!((hp >> 32) as u32 != (hq >> 32) as u32);
    }

    let n = 64usize;
    let shared_hash = 0x1234_5678_9ABC_DEF0u64;

    let mut reg = GeometryRegistry::default();
    let blob_a = vec![0xAAu8; n];
    let blob_b = vec![0xBBu8; n];

    // Distinct blobs colliding on the hash become two entries.
    let id_a = intern_geometry(&mut reg, GeometryKind::Hull, shared_hash, blob_a);
    let id_b = intern_geometry(&mut reg, GeometryKind::Hull, shared_hash, blob_b);
    ensure!(id_a != id_b);
    ensure!(reg.entries.len() == 2);

    // Re-interning either blob must find it through the hash chain and never
    // grow the registry, including the one shadowed behind the bucket head.
    ensure!(intern_geometry(&mut reg, GeometryKind::Hull, shared_hash, vec![0xAAu8; n]) == id_a);
    ensure!(reg.entries.len() == 2);
    ensure!(intern_geometry(&mut reg, GeometryKind::Hull, shared_hash, vec![0xBBu8; n]) == id_b);
    ensure!(reg.entries.len() == 2);

    // Seed-then-capture: appending byte-identical duplicate slots keeps
    // id == slot index, and a later exact intern resolves without appending.
    let mut seeded = GeometryRegistry::default();
    ensure!(append_geometry(&mut seeded, GeometryKind::Hull, shared_hash, vec![0xAAu8; n]) == 0);
    ensure!(append_geometry(&mut seeded, GeometryKind::Hull, shared_hash, vec![0xBBu8; n]) == 1);
    ensure!(append_geometry(&mut seeded, GeometryKind::Hull, shared_hash, vec![0xAAu8; n]) == 2);

    let resolved = intern_geometry(&mut seeded, GeometryKind::Hull, shared_hash, vec![0xAAu8; n]);
    ensure!(seeded.entries.len() == 3); // no growth
    ensure!(resolved == 0 || resolved == 2);
}

// ---------------------------------------------------------------------------
// Port-specific additions
// ---------------------------------------------------------------------------

// The capture test's scene (tests/test_recording_capture.rs::build_and_run)
// must round-trip: record -> replay with state-hash verification at every step
// marker (validate_replay checks each recorded StateHash op).
#[test]
fn capture_stream_round_trip() {
    let mut world = create_world(&default_world_def());
    world_start_recording(&mut world);

    let ground_hull = make_box_hull(10.0, 0.5, 10.0);
    let ground_id = create_body(&mut world, &default_body_def());
    let shape_def = default_shape_def();
    create_hull_shape(&mut world, ground_id, &shape_def, &ground_hull);

    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    body_def.position = pos(0.0, 4.0, 0.0);
    let body_a = create_body(&mut world, &body_def);
    let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.5 };
    create_sphere_shape(&mut world, body_a, &shape_def, &sphere);

    body_def.position = pos(1.2, 4.0, 0.0);
    let body_b = create_body(&mut world, &body_def);
    create_sphere_shape(&mut world, body_b, &shape_def, &sphere);

    let mut joint_def = default_revolute_joint_def();
    joint_def.base.body_id_a = body_a;
    joint_def.base.body_id_b = body_b;
    create_revolute_joint(&mut world, &joint_def);

    body_set_linear_velocity(&mut world, body_a, vec3(0.0, -1.0, 0.0));
    body_set_name(&mut world, body_b, "pendulum");

    for _ in 0..10 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    let mut filter = default_query_filter();
    filter.id = 42;
    filter.name = "probe";
    world_cast_ray_closest(&world, pos(0.0, 5.0, 0.0), vec3(0.0, -10.0, 0.0), filter);

    let aabb = AABB { lower_bound: vec3(-2.0, -2.0, -2.0), upper_bound: vec3(2.0, 6.0, 2.0) };
    world_overlap_aabb(&mut world, aabb, default_query_filter(), &mut overlap_fcn());

    destroy_body(&mut world, body_b);
    for _ in 0..2 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    let final_hash = hash_world_state(&world);
    let rec = world_stop_recording(&mut world).expect("recording active");
    destroy_world(world);

    ensure!(validate_replay(rec.data(), 1));
    ensure!(validate_replay(rec.data(), 4));

    // The player's end state must match the recording world's final hash.
    let mut player = Player::create(rec.data(), 1).expect("player");
    while player.step_frame() {}
    ensure!(!player.has_diverged());
    ensure!(hash_world_state(player.world()) == final_hash);
    player.destroy();
}

// Record a multithreaded session (worker_count = 4) and replay it at 1 and 4
// workers. The sim is worker-count invariant, so every recorded StateHash must
// reproduce at both counts.
#[test]
fn multithread_record_round_trip() {
    let mut world_def = default_world_def();
    world_def.worker_count = 4;
    let mut world = create_world(&world_def);
    world_set_gravity(&mut world, vec3(0.0, -10.0, 0.0));
    world_start_recording(&mut world);

    let ground_id = create_body(&mut world, &default_body_def());
    let ground_box = make_box_hull(20.0, 1.0, 20.0);
    create_hull_shape(&mut world, ground_id, &default_shape_def(), &ground_box);

    // Enough falling boxes to give the solver real islands to partition.
    let mut box_shape = default_shape_def();
    box_shape.density = 1.0;
    let bx = make_box_hull(0.5, 0.5, 0.5);
    for i in 0..24 {
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.position =
            pos(((i % 4) as f32 - 1.5) * 1.2, 1.0 + (i / 4) as f32 * 1.1, ((i % 3) as f32 - 1.0) * 1.2);
        let body_id = create_body(&mut world, &body_def);
        create_hull_shape(&mut world, body_id, &box_shape, &bx);
    }

    for _ in 0..40 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    let rec = world_stop_recording(&mut world).expect("recording active");
    destroy_world(world);

    ensure!(validate_replay(rec.data(), 1));
    ensure!(validate_replay(rec.data(), 4));
}

// Corrupt input must fail cleanly, never panic.
#[test]
fn corrupt_input_rejected() {
    ensure!(!validate_replay(&[], 1));
    ensure!(!validate_replay(&[0u8; 16], 1));
    ensure!(!validate_replay(&[0u8; 64], 1));

    // A valid recording truncated mid-stream stops without panicking.
    let mut world = create_world(&default_world_def());
    world_start_recording(&mut world);
    let mut bd = default_body_def();
    bd.body_type = BodyType::Dynamic;
    bd.position = pos(0.0, 5.0, 0.0);
    let body_id = create_body(&mut world, &bd);
    let s = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.5 };
    create_sphere_shape(&mut world, body_id, &default_shape_def(), &s);
    for _ in 0..5 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }
    let rec = world_stop_recording(&mut world).expect("recording active");
    destroy_world(world);

    // Chop the registry off and half the op stream: replay must not panic.
    // (The truncated stream has no registry locator, so the ops run to the
    // truncation point; a missing hull registry entry fails the read cleanly.)
    let data = rec.data();
    let snapshot_size = read_u64(data, 24) as usize;
    let cut = REC_HEADER_SIZE + snapshot_size + (data.len() - REC_HEADER_SIZE - snapshot_size) / 2;
    let mut truncated = data[..cut].to_vec();
    // Zero the registry locator since the block is gone.
    truncated[32..48].fill(0);
    let _ = validate_replay(&truncated, 1);

    let _ = Arc::new(0); // keep the Arc import exercised under all features
}
