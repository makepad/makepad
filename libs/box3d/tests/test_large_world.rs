// Port of box3d/test/test_large_world.c
// The Rust port is single precision only (BOX3D_DOUBLE_PRECISION off), so the
// far-from-origin halves of each C subtest (gated behind the define) are not
// ported; the origin halves are kept in full.

use makepad_box3d::body::*;
use makepad_box3d::hull::{make_box_hull, make_cube_hull};
use makepad_box3d::math_functions::{offset_pos, pos, sub_pos, vec3, Pos, Vec3};
use makepad_box3d::physics_world::*;
use makepad_box3d::shape::{create_hull_shape, create_sphere_shape, shape_ray_cast};
use makepad_box3d::types::*;
use makepad_box3d::{ensure, ensure_small};

const STACK_COUNT: usize = 6;
const MAX_STEPS: i32 = 400;

struct StackResult {
    // Final body positions relative to the base, so origin and far runs are directly comparable
    relative_positions: [Vec3; STACK_COUNT],

    // First step on which the top body fell asleep, or -1 if it never settled
    sleep_step: i32,
}

// Drop a short stack of boxes onto a ground box centered at baseX. Records each body's final
// position relative to the base and the step on which the stack settles.
fn run_stack(base_x: f32) -> StackResult {
    let base: Pos = pos(base_x, 0.0, 0.0);

    let world_def = default_world_def();
    let mut world = create_world(&world_def);

    let mut ground_def = default_body_def();
    ground_def.position = base;
    let ground_id = create_body(&mut world, &ground_def);
    let ground_box = make_box_hull(10.0, 1.0, 10.0);
    let ground_shape_def = default_shape_def();
    create_hull_shape(&mut world, ground_id, &ground_shape_def, &ground_box);

    let mut bodies = [makepad_box3d::id::NULL_BODY_ID; STACK_COUNT];
    for i in 0..STACK_COUNT {
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.position = offset_pos(base, vec3(0.0, 2.0 + 1.05 * i as f32, 0.0));
        bodies[i] = create_body(&mut world, &body_def);

        let cube = make_cube_hull(0.5);
        let mut shape_def = default_shape_def();
        shape_def.density = 1.0;
        create_hull_shape(&mut world, bodies[i], &shape_def, &cube);
    }

    let mut result = StackResult { relative_positions: [Vec3::ZERO; STACK_COUNT], sleep_step: -1 };

    for step in 0..MAX_STEPS {
        world_step(&mut world, 1.0 / 60.0, 4);
        if result.sleep_step < 0 && !body_is_awake(&world, bodies[STACK_COUNT - 1]) {
            result.sleep_step = step;
        }
    }

    for i in 0..STACK_COUNT {
        let p = body_get_position(&world, bodies[i]);
        result.relative_positions[i] = sub_pos(p, base);
    }

    destroy_world(world);
    result
}

// A stack at the origin should settle. (The C far-from-origin comparison is
// double precision only and is not ported.)
#[test]
fn large_world_stack_test() {
    let origin = run_stack(0.0);
    ensure!(origin.sleep_step >= 0);
}

// Fire a fast bullet at a thin wall. Returns the bullet's final x relative to the base. If
// continuous collision works the bullet stops at the wall instead of tunneling past it.
fn run_bullet(base_x: f32) -> f32 {
    let base: Pos = pos(base_x, 0.0, 0.0);

    let world_def = default_world_def();
    let mut world = create_world(&world_def);

    // Thin static wall at x = base + 5, spanning y and z
    let mut wall_def = default_body_def();
    wall_def.body_type = BodyType::Static;
    wall_def.position = offset_pos(base, vec3(5.0, 0.0, 0.0));
    let wall_id = create_body(&mut world, &wall_def);
    let wall_box = make_box_hull(0.05, 5.0, 5.0);
    let wall_shape_def = default_shape_def();
    create_hull_shape(&mut world, wall_id, &wall_shape_def, &wall_box);

    // Small fast bullet aimed at the wall, no gravity
    let mut bullet_def = default_body_def();
    bullet_def.body_type = BodyType::Dynamic;
    bullet_def.is_bullet = true;
    bullet_def.gravity_scale = 0.0;
    bullet_def.position = base;
    bullet_def.linear_velocity = vec3(200.0, 0.0, 0.0);
    let bullet_id = create_body(&mut world, &bullet_def);
    let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.1 };
    let mut bullet_shape_def = default_shape_def();
    bullet_shape_def.density = 1.0;
    create_sphere_shape(&mut world, bullet_id, &bullet_shape_def, &sphere);

    for _step in 0..30 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    let relative = sub_pos(body_get_position(&world, bullet_id), base);
    destroy_world(world);
    relative.x
}

// The bullet must be caught by the wall, not tunnel through it.
#[test]
fn large_world_bullet_test() {
    // Wall front face is at x = 5 - 0.05; the bullet radius is 0.1, so a caught bullet stays well
    // short of the wall center at x = 5.
    let origin_x = run_bullet(0.0);
    ensure!(origin_x < 5.0);
}

struct QueryResult {
    cast_hit: bool,
    cast_rel_x: f32, // shape cast hit point x relative to the base
    overlap_hit: bool,
    mover_fraction: f32,
    plane_count: i32,
    ray_hit: bool,
    ray_rel_x: f32, // world ray cast hit point x relative to the base
    shape_ray_hit: bool,
    shape_ray_rel_x: f32, // direct shape ray cast hit point x relative to the base
}

// Run the four origin relative spatial queries against a static box centered at the base. The query
// inputs are all relative to the base, so passing base as the origin keeps them precise far out.
fn run_queries(base_x: f32) -> QueryResult {
    let base: Pos = pos(base_x, 0.0, 0.0);

    let world_def = default_world_def();
    let mut world = create_world(&world_def);

    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Static;
    body_def.position = base;
    let body_id = create_body(&mut world, &body_def);
    let box_hull = make_box_hull(1.0, 1.0, 1.0);
    let shape_def = default_shape_def();
    let shape_id = create_hull_shape(&mut world, body_id, &shape_def, &box_hull);

    world_step(&mut world, 1.0 / 60.0, 1);

    // Sphere proxy swept from the left into the box, hitting the left face at relative x = -1
    let cast_point = [vec3(-5.0, 0.0, 0.0)];
    let cast_proxy = ShapeProxy { points: &cast_point, radius: 0.25 };
    let mut cast_hit = false;
    let mut cast_rel_x = 0.0f32;
    world_cast_shape(
        &world,
        base,
        &cast_proxy,
        vec3(10.0, 0.0, 0.0),
        default_query_filter(),
        &mut |_shape_id, point, _normal, fraction, _material_id, _triangle_index, _child_index| {
            cast_hit = true;
            cast_rel_x = sub_pos(point, base).x;
            fraction
        },
    );

    // Sphere proxy sitting at the box center
    let overlap_point = [vec3(0.0, 0.0, 0.0)];
    let overlap_proxy = ShapeProxy { points: &overlap_point, radius: 0.5 };
    let mut overlap_hit = false;
    world_overlap_shape(&world, base, &overlap_proxy, default_query_filter(), &mut |_shape_id| {
        overlap_hit = true;
        true
    });

    // Capsule swept from the left into the box
    let mover_cast = Capsule { center1: vec3(-5.0, -0.3, 0.0), center2: vec3(-5.0, 0.3, 0.0), radius: 0.25 };
    let mover_fraction =
        world_cast_mover(&world, base, &mover_cast, vec3(10.0, 0.0, 0.0), default_query_filter(), None);

    // Capsule overlapping the left face, should report a contact plane
    let mover_collide = Capsule { center1: vec3(-1.1, -0.3, 0.0), center2: vec3(-1.1, 0.3, 0.0), radius: 0.3 };
    let mut plane_count = 0i32;
    world_collide_mover(&world, base, &mover_collide, default_query_filter(), &mut |_shape_id, planes| {
        plane_count += planes.len() as i32;
        true
    });

    // World ray cast from the left into the box, hitting the left face at relative x = -1
    let ray_origin = offset_pos(base, vec3(-5.0, 0.0, 0.0));
    let ray = world_cast_ray_closest(&world, ray_origin, vec3(10.0, 0.0, 0.0), default_query_filter());
    let ray_hit = ray.hit;
    let ray_rel_x = if ray.hit { sub_pos(ray.point, base).x } else { 0.0 };

    // Direct shape ray cast against the same box
    let shape_ray = shape_ray_cast(&world, shape_id, ray_origin, vec3(10.0, 0.0, 0.0));
    let shape_ray_hit = shape_ray.hit;
    let shape_ray_rel_x = if shape_ray.hit { sub_pos(shape_ray.point, base).x } else { 0.0 };

    destroy_world(world);

    QueryResult {
        cast_hit,
        cast_rel_x,
        overlap_hit,
        mover_fraction,
        plane_count,
        ray_hit,
        ray_rel_x,
        shape_ray_hit,
        shape_ray_rel_x,
    }
}

// The origin relative queries hit at the origin. (The C far-from-origin
// comparison is double precision only and is not ported.)
#[test]
fn large_world_query_test() {
    let origin = run_queries(0.0);
    ensure!(origin.cast_hit);
    ensure!(origin.overlap_hit);
    ensure!(origin.mover_fraction < 1.0);
    ensure!(origin.plane_count > 0);
    ensure_small!(origin.cast_rel_x + 1.0, 0.05);
    ensure!(origin.ray_hit);
    ensure_small!(origin.ray_rel_x + 1.0, 0.05);
    ensure!(origin.shape_ray_hit);
    ensure_small!(origin.shape_ray_rel_x + 1.0, 0.05);
}

// Port of the BOX3D_DOUBLE_PRECISION halves: a stack, a bullet, and the queries far from
// the origin must behave identically to the origin runs in double precision mode.

#[cfg(feature = "double-precision")]
#[test]
fn large_world_stack_far_test() {
    let origin = run_stack(0.0);
    ensure!(origin.sleep_step >= 0);

    let far = run_stack(1.0e7);
    ensure!(far.sleep_step >= 0);

    // Sleeps on the same frame and lands in the same relative configuration
    ensure!(far.sleep_step == origin.sleep_step);
    for i in 0..STACK_COUNT {
        ensure_small!(far.relative_positions[i].x - origin.relative_positions[i].x, 1.0e-3);
        ensure_small!(far.relative_positions[i].y - origin.relative_positions[i].y, 1.0e-3);
        ensure_small!(far.relative_positions[i].z - origin.relative_positions[i].z, 1.0e-3);
    }
}

#[cfg(feature = "double-precision")]
#[test]
fn large_world_bullet_far_test() {
    // The blocking check is that the catch still holds far from the origin where the swept
    // query box rounds back to float with large ULP.
    let far_x = run_bullet(1.0e7);
    ensure!(far_x < 5.0);
}

#[cfg(feature = "double-precision")]
#[test]
fn large_world_query_far_test() {
    let origin = run_queries(0.0);

    let far = run_queries(1.0e7);
    ensure!(far.cast_hit);
    ensure!(far.overlap_hit);
    ensure!(far.mover_fraction < 1.0);
    ensure!(far.plane_count > 0);
    ensure!(far.ray_hit);
    ensure!(far.shape_ray_hit);

    ensure_small!(far.cast_rel_x - origin.cast_rel_x, 1.0e-3);
    ensure_small!(far.mover_fraction - origin.mover_fraction, 1.0e-3);
    ensure!(far.plane_count == origin.plane_count);
    ensure_small!(far.ray_rel_x - origin.ray_rel_x, 1.0e-3);
    ensure_small!(far.shape_ray_rel_x - origin.shape_ray_rel_x, 1.0e-3);
}
