// Tests for the external task-system hooks (C: b3WorldDef
// enqueueTask/finishTask/userTaskContext, types.h). Not a C test port: the C
// suite exercises these through the samples' enkiTS integration; here a toy
// thread-per-task system and an inline (null-returning) system stand in.

use makepad_box3d::body::*;
use makepad_box3d::core::{hash, HASH_INIT};
use makepad_box3d::ensure;
use makepad_box3d::hull::make_box_hull;
use makepad_box3d::math_functions::{pos, vec3};
use makepad_box3d::physics_world::*;
use makepad_box3d::shape::*;
use makepad_box3d::test_utils::{
    inline_task_enqueue, inline_task_finish, thread_per_task_enqueue, thread_per_task_finish,
};
use makepad_box3d::id::BodyId;
use makepad_box3d::types::*;

// A stack of boxes plus loose spheres: enough contacts/islands to exercise the
// parallel pair, collide, solver, and sensor passes.
fn build_scene(world: &mut World) -> Vec<BodyId> {
    let ground_def = default_body_def();
    let ground_id = create_body(world, &ground_def);
    let ground_hull = make_box_hull(20.0, 0.5, 20.0);
    let shape_def = default_shape_def();
    create_hull_shape(world, ground_id, &shape_def, &ground_hull);

    let box_hull = make_box_hull(0.5, 0.5, 0.5);
    let mut ids = Vec::new();

    for i in 0..10 {
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.position = pos(0.0, 1.0 + 1.05 * i as f32, 0.0);
        let body_id = create_body(world, &body_def);
        create_hull_shape(world, body_id, &shape_def, &box_hull);
        ids.push(body_id);
    }

    for i in 0..12 {
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.position = pos(-4.0 + 0.7 * i as f32, 3.0 + 0.4 * i as f32, 2.0);
        let body_id = create_body(world, &body_def);
        let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.4 };
        create_sphere_shape(world, body_id, &shape_def, &sphere);
        ids.push(body_id);
    }

    ids
}

fn run_and_hash(world_def: WorldDef) -> u32 {
    let mut world = create_world(&world_def);
    let ids = build_scene(&mut world);

    for _ in 0..120 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    let mut h = HASH_INIT;
    for id in &ids {
        let t = body_get_transform(&world, *id);
        // Position components are f32 or f64 depending on the precision mode;
        // widen to f64 so the hash covers full precision in both.
        let vals: [f64; 7] = [
            t.p.x as f64,
            t.p.y as f64,
            t.p.z as f64,
            t.q.v.x as f64,
            t.q.v.y as f64,
            t.q.v.z as f64,
            t.q.s as f64,
        ];
        for f in vals {
            h = hash(h, &f.to_le_bytes());
        }
    }

    destroy_world(world);
    h
}

// The external system must produce results bit-identical to the serial path
// and the internal scheduler: same tasks, same reductions, different threads.
#[test]
fn external_task_system_matches_serial_and_internal() {
    let serial_hash = run_and_hash(default_world_def());

    let mut internal_def = default_world_def();
    internal_def.worker_count = 4;
    let internal_hash = run_and_hash(internal_def);

    let mut external_def = default_world_def();
    external_def.worker_count = 4;
    external_def.enqueue_task = Some(thread_per_task_enqueue);
    external_def.finish_task = Some(thread_per_task_finish);
    let external_hash = run_and_hash(external_def);

    println!(
        "serial=0x{:08X} internal=0x{:08X} external=0x{:08X}",
        serial_hash, internal_hash, external_hash
    );
    ensure!(serial_hash != 0);
    ensure!(internal_hash == serial_hash);
    ensure!(external_hash == serial_hash);
}

// C contract: an enqueue callback may execute the task synchronously and
// return NULL; Box3D must then never call finish for that task (the finish
// callback here panics if it is ever invoked), and results stay identical.
#[test]
fn external_inline_null_return_path() {
    let serial_hash = run_and_hash(default_world_def());

    let mut inline_def = default_world_def();
    inline_def.worker_count = 4;
    inline_def.enqueue_task = Some(inline_task_enqueue);
    inline_def.finish_task = Some(inline_task_finish);
    let inline_hash = run_and_hash(inline_def);

    ensure!(inline_hash == serial_hash);
}

// External system with worker_count == 1: C allows this (workerCount > 0 with
// callbacks selects the external system) — tasks still route through the user
// callbacks.
#[test]
fn external_task_system_single_worker() {
    let serial_hash = run_and_hash(default_world_def());

    let mut external_def = default_world_def();
    external_def.worker_count = 1;
    external_def.enqueue_task = Some(thread_per_task_enqueue);
    external_def.finish_task = Some(thread_per_task_finish);
    let external_hash = run_and_hash(external_def);

    ensure!(external_hash == serial_hash);
}
