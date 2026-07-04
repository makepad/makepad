// Port of box3d/test/test_body_query.c
//
// The per-body query functions take an explicit world origin and a world body transform.
// Everything is re-centered on the origin so the float collision math stays accurate far from
// the world origin. These tests pin that framing: results come back in world space, the supplied
// transform drives the geometry (not the body's stored pose), and a large origin offset must not
// change a hit fraction or normal.
//
// CastRay and CastShape never touch the body's stored transform, so a static body at the origin
// holding local-frame shapes is enough. No step is needed for any query.

use makepad_box3d::body::*;
use makepad_box3d::hull::make_box_hull;
use makepad_box3d::math_functions::*;
use makepad_box3d::physics_world::*;
use makepad_box3d::types::*;
use makepad_box3d::{ensure, ensure_small};

fn create_query_world() -> (World, makepad_box3d::id::BodyId) {
    let world_def = default_world_def();
    let mut world = create_world(&world_def);
    let body_def = default_body_def();
    let body_id = create_body(&mut world, &body_def);
    (world, body_id)
}

fn identity_at(x: f32, y: f32, z: f32) -> WorldTransform {
    WorldTransform { p: pos(x, y, z), q: Quat::IDENTITY }
}

// CastRay ----------------------------------------------------------------------------------

#[test]
fn cast_ray_hits_sphere() {
    let (mut world, body_id) = create_query_world();

    let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 1.0 };
    let shape_def = default_shape_def();
    makepad_box3d::shape::create_sphere_shape(&mut world, body_id, &shape_def, &sphere);

    // Body sphere at world (5,0,0), ray straight at it along +X.
    let body_transform = identity_at(5.0, 0.0, 0.0);
    let result = body_cast_ray(
        &world,
        body_id,
        pos(0.0, 0.0, 0.0),
        vec3(10.0, 0.0, 0.0),
        default_query_filter(),
        1.0,
        body_transform,
    );

    ensure!(result.hit);
    ensure!(shape_is_valid(&world, result.shape_id));
    ensure_small!(result.fraction - 0.4, 1e-5);
    ensure_small!(result.normal.x + 1.0, 1e-5);
    ensure_small!(result.normal.y, 1e-5);
    ensure_small!(result.normal.z, 1e-5);

    let point = to_vec3(result.point);
    ensure_small!(point.x - 4.0, 1e-4);
    ensure_small!(point.y, 1e-4);
    ensure_small!(point.z, 1e-4);

    destroy_world(world);
}

#[test]
fn cast_ray_miss() {
    let (mut world, body_id) = create_query_world();

    let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 1.0 };
    let shape_def = default_shape_def();
    makepad_box3d::shape::create_sphere_shape(&mut world, body_id, &shape_def, &sphere);

    // Ray runs parallel to the body, never reaching it.
    let body_transform = identity_at(5.0, 0.0, 0.0);
    let result = body_cast_ray(
        &world,
        body_id,
        pos(0.0, 0.0, 0.0),
        vec3(0.0, 10.0, 0.0),
        default_query_filter(),
        1.0,
        body_transform,
    );

    ensure!(result.hit == false);

    destroy_world(world);
}

#[test]
fn cast_ray_closest_shape() {
    let (mut world, body_id) = create_query_world();

    let shape_def = default_shape_def();
    let near_sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 1.0 };
    let far_sphere = Sphere { center: vec3(4.0, 0.0, 0.0), radius: 1.0 };
    let near_id = makepad_box3d::shape::create_sphere_shape(&mut world, body_id, &shape_def, &near_sphere);
    makepad_box3d::shape::create_sphere_shape(&mut world, body_id, &shape_def, &far_sphere);

    // Ray crosses both spheres; the loop must shrink maxFraction to the nearer hit.
    let body_transform = identity_at(0.0, 0.0, 0.0);
    let result = body_cast_ray(
        &world,
        body_id,
        pos(-5.0, 0.0, 0.0),
        vec3(10.0, 0.0, 0.0),
        default_query_filter(),
        1.0,
        body_transform,
    );

    ensure!(result.hit);
    ensure!(result.shape_id.index1 == near_id.index1);
    ensure!(result.shape_id.generation == near_id.generation);
    ensure_small!(result.fraction - 0.4, 1e-5);

    destroy_world(world);
}

#[test]
fn cast_ray_rotated_body() {
    let (mut world, body_id) = create_query_world();

    // Local center (0,2,0) rotated +90 deg about Z lands at world (-2,0,0).
    let sphere = Sphere { center: vec3(0.0, 2.0, 0.0), radius: 0.5 };
    let shape_def = default_shape_def();
    makepad_box3d::shape::create_sphere_shape(&mut world, body_id, &shape_def, &sphere);

    let body_transform = WorldTransform {
        p: pos(0.0, 0.0, 0.0),
        q: make_quat_from_axis_angle(vec3(0.0, 0.0, 1.0), 0.5 * PI),
    };
    let result = body_cast_ray(
        &world,
        body_id,
        pos(0.0, 0.0, 0.0),
        vec3(-4.0, 0.0, 0.0),
        default_query_filter(),
        1.0,
        body_transform,
    );

    ensure!(result.hit);
    ensure_small!(result.fraction - 0.375, 1e-5);
    ensure_small!(result.normal.x - 1.0, 1e-5);

    let point = to_vec3(result.point);
    ensure_small!(point.x + 1.5, 1e-4);

    destroy_world(world);
}

#[test]
fn cast_ray_far_from_origin() {
    let (mut world, body_id) = create_query_world();

    let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 1.0 };
    let shape_def = default_shape_def();
    makepad_box3d::shape::create_sphere_shape(&mut world, body_id, &shape_def, &sphere);

    // Same geometry as cast_ray_hits_sphere shifted far from the world origin. The relative
    // framing keeps the subtraction exact, so fraction and normal must be unchanged.
    let origin = pos(1.0e6, -2.0e6, 5.0e5);
    let body_transform = WorldTransform { p: offset_pos(origin, vec3(5.0, 0.0, 0.0)), q: Quat::IDENTITY };
    let result = body_cast_ray(
        &world,
        body_id,
        origin,
        vec3(10.0, 0.0, 0.0),
        default_query_filter(),
        1.0,
        body_transform,
    );

    ensure!(result.hit);
    ensure_small!(result.fraction - 0.4, 1e-5);
    ensure_small!(result.normal.x + 1.0, 1e-5);
    ensure_small!(result.normal.y, 1e-5);
    ensure_small!(result.normal.z, 1e-5);

    destroy_world(world);
}

// CastShape --------------------------------------------------------------------------------

#[test]
fn cast_shape_hits_box() {
    let (mut world, body_id) = create_query_world();

    let box_hull = make_box_hull(1.0, 1.0, 1.0);
    let shape_def = default_shape_def();
    makepad_box3d::shape::create_hull_shape(&mut world, body_id, &shape_def, &box_hull);

    // Sphere proxy of radius 0.5 cast along +X into a box whose front face is at world x = 4.
    let point = [vec3(0.0, 0.0, 0.0)];
    let proxy = ShapeProxy { points: &point, radius: 0.5 };
    let body_transform = identity_at(5.0, 0.0, 0.0);
    let result = body_cast_shape(
        &world,
        body_id,
        pos(0.0, 0.0, 0.0),
        &proxy,
        vec3(10.0, 0.0, 0.0),
        default_query_filter(),
        1.0,
        false,
        body_transform,
    );

    // Front face at world x = 4. The fraction carries a small shape-cast skin, the contact point
    // and normal do not.
    ensure!(result.hit);
    ensure!(shape_is_valid(&world, result.shape_id));
    ensure_small!(result.fraction - 0.35, 1e-2);
    ensure_small!(result.normal.x + 1.0, 1e-4);

    let hit = to_vec3(result.point);
    ensure_small!(hit.x - 4.0, 1e-3);

    destroy_world(world);
}

#[test]
fn cast_shape_miss() {
    let (mut world, body_id) = create_query_world();

    let box_hull = make_box_hull(1.0, 1.0, 1.0);
    let shape_def = default_shape_def();
    makepad_box3d::shape::create_hull_shape(&mut world, body_id, &shape_def, &box_hull);

    let point = [vec3(0.0, 0.0, 0.0)];
    let proxy = ShapeProxy { points: &point, radius: 0.5 };
    let body_transform = identity_at(5.0, 0.0, 0.0);
    let result = body_cast_shape(
        &world,
        body_id,
        pos(0.0, 0.0, 0.0),
        &proxy,
        vec3(0.0, 10.0, 0.0),
        default_query_filter(),
        1.0,
        false,
        body_transform,
    );

    ensure!(result.hit == false);

    destroy_world(world);
}

#[test]
fn cast_shape_rotated_body() {
    let (mut world, body_id) = create_query_world();

    // Body sphere local center (0,2,0) rotated +90 deg about Z lands at world (-2,0,0).
    let sphere = Sphere { center: vec3(0.0, 2.0, 0.0), radius: 1.0 };
    let shape_def = default_shape_def();
    makepad_box3d::shape::create_sphere_shape(&mut world, body_id, &shape_def, &sphere);

    let point = [vec3(0.0, 0.0, 0.0)];
    let proxy = ShapeProxy { points: &point, radius: 0.5 };
    let body_transform = WorldTransform {
        p: pos(0.0, 0.0, 0.0),
        q: make_quat_from_axis_angle(vec3(0.0, 0.0, 1.0), 0.5 * PI),
    };
    let result = body_cast_shape(
        &world,
        body_id,
        pos(0.0, 0.0, 0.0),
        &proxy,
        vec3(-4.0, 0.0, 0.0),
        default_query_filter(),
        1.0,
        false,
        body_transform,
    );

    ensure!(result.hit);
    ensure_small!(result.fraction - 0.125, 1e-2);
    ensure_small!(result.normal.x - 1.0, 1e-4);

    let hit = to_vec3(result.point);
    ensure_small!(hit.x + 1.0, 1e-3);

    destroy_world(world);
}

#[test]
fn cast_shape_far_from_origin() {
    let (mut world, body_id) = create_query_world();

    let box_hull = make_box_hull(1.0, 1.0, 1.0);
    let shape_def = default_shape_def();
    makepad_box3d::shape::create_hull_shape(&mut world, body_id, &shape_def, &box_hull);

    let point = [vec3(0.0, 0.0, 0.0)];
    let proxy = ShapeProxy { points: &point, radius: 0.5 };
    let origin = pos(1.0e6, -2.0e6, 5.0e5);
    let body_transform = WorldTransform { p: offset_pos(origin, vec3(5.0, 0.0, 0.0)), q: Quat::IDENTITY };
    let result = body_cast_shape(
        &world,
        body_id,
        origin,
        &proxy,
        vec3(10.0, 0.0, 0.0),
        default_query_filter(),
        1.0,
        false,
        body_transform,
    );

    ensure!(result.hit);
    ensure_small!(result.fraction - 0.35, 1e-2);
    ensure_small!(result.normal.x + 1.0, 1e-4);

    destroy_world(world);
}

// OverlapShape -----------------------------------------------------------------------------

#[test]
fn overlap_true() {
    let (mut world, body_id) = create_query_world();

    let box_hull = make_box_hull(1.0, 1.0, 1.0);
    let shape_def = default_shape_def();
    makepad_box3d::shape::create_hull_shape(&mut world, body_id, &shape_def, &box_hull);

    // Proxy sits at the box center.
    let point = [vec3(0.0, 0.0, 0.0)];
    let proxy = ShapeProxy { points: &point, radius: 0.5 };
    let body_transform = identity_at(5.0, 0.0, 0.0);
    let overlaps = body_overlap_shape(&world, body_id, pos(5.0, 0.0, 0.0), &proxy, default_query_filter(), body_transform);

    ensure!(overlaps);

    destroy_world(world);
}

#[test]
fn overlap_false() {
    let (mut world, body_id) = create_query_world();

    let box_hull = make_box_hull(1.0, 1.0, 1.0);
    let shape_def = default_shape_def();
    makepad_box3d::shape::create_hull_shape(&mut world, body_id, &shape_def, &box_hull);

    let point = [vec3(0.0, 0.0, 0.0)];
    let proxy = ShapeProxy { points: &point, radius: 0.5 };
    let body_transform = identity_at(5.0, 0.0, 0.0);
    let overlaps = body_overlap_shape(&world, body_id, pos(20.0, 0.0, 0.0), &proxy, default_query_filter(), body_transform);

    ensure!(overlaps == false);

    destroy_world(world);
}

#[test]
fn overlap_respects_body_transform() {
    let (mut world, body_id) = create_query_world();

    let box_hull = make_box_hull(1.0, 1.0, 1.0);
    let shape_def = default_shape_def();
    makepad_box3d::shape::create_hull_shape(&mut world, body_id, &shape_def, &box_hull);

    // Fixed proxy and origin: only the supplied transform decides the overlap.
    let point = [vec3(0.0, 0.0, 0.0)];
    let proxy = ShapeProxy { points: &point, radius: 0.5 };
    let origin = pos(0.0, 0.0, 0.0);

    ensure!(body_overlap_shape(&world, body_id, origin, &proxy, default_query_filter(), identity_at(0.0, 0.0, 0.0)));
    ensure!(
        body_overlap_shape(&world, body_id, origin, &proxy, default_query_filter(), identity_at(20.0, 0.0, 0.0)) == false
    );

    destroy_world(world);
}

#[test]
fn overlap_filter() {
    let (mut world, body_id) = create_query_world();

    let box_hull = make_box_hull(1.0, 1.0, 1.0);
    let shape_def = default_shape_def();
    makepad_box3d::shape::create_hull_shape(&mut world, body_id, &shape_def, &box_hull);

    let point = [vec3(0.0, 0.0, 0.0)];
    let proxy = ShapeProxy { points: &point, radius: 0.5 };
    let body_transform = identity_at(0.0, 0.0, 0.0);

    // Geometry overlaps, but a zero mask rejects every category.
    let mut filter = default_query_filter();
    filter.mask_bits = 0;
    let overlaps = body_overlap_shape(&world, body_id, pos(0.0, 0.0, 0.0), &proxy, filter, body_transform);

    ensure!(overlaps == false);

    destroy_world(world);
}

// CollideMover -----------------------------------------------------------------------------

#[test]
fn mover_touches_box() {
    let (mut world, body_id) = create_query_world();

    let box_hull = make_box_hull(0.5, 0.5, 0.5);
    let shape_def = default_shape_def();
    makepad_box3d::shape::create_hull_shape(&mut world, body_id, &shape_def, &box_hull);

    // Mover core runs above the +Y face; its 0.2 radius reaches 0.1 into it.
    let mover = Capsule { center1: vec3(-0.3, 0.6, 0.0), center2: vec3(0.3, 0.6, 0.0), radius: 0.2 };
    let mut planes = [BodyPlaneResult::default(); 4];
    let body_transform = identity_at(0.0, 0.0, 0.0);
    let count = body_collide_mover(
        &world,
        body_id,
        &mut planes,
        pos(0.0, 0.0, 0.0),
        &mover,
        default_query_filter(),
        body_transform,
    );

    ensure!(count == 1);
    ensure!(shape_is_valid(&world, planes[0].shape_id));
    ensure!(is_normalized(planes[0].result.plane.normal));
    ensure!(planes[0].result.plane.normal.y > 0.99);
    ensure_small!(planes[0].result.plane.offset - 0.1, 1e-4);

    destroy_world(world);
}

#[test]
fn mover_separated() {
    let (mut world, body_id) = create_query_world();

    let box_hull = make_box_hull(0.5, 0.5, 0.5);
    let shape_def = default_shape_def();
    makepad_box3d::shape::create_hull_shape(&mut world, body_id, &shape_def, &box_hull);

    let mover = Capsule { center1: vec3(-0.3, 5.0, 0.0), center2: vec3(0.3, 5.0, 0.0), radius: 0.2 };
    let mut planes = [BodyPlaneResult::default(); 4];
    let body_transform = identity_at(0.0, 0.0, 0.0);
    let count = body_collide_mover(
        &world,
        body_id,
        &mut planes,
        pos(0.0, 0.0, 0.0),
        &mover,
        default_query_filter(),
        body_transform,
    );

    ensure!(count == 0);

    destroy_world(world);
}

#[test]
fn mover_rotated_body() {
    let (mut world, body_id) = create_query_world();

    let box_hull = make_box_hull(0.5, 0.5, 0.5);
    let shape_def = default_shape_def();
    makepad_box3d::shape::create_hull_shape(&mut world, body_id, &shape_def, &box_hull);

    // Rotating +90 deg about X turns the local +Y face toward world +Z. The mover sits above the
    // world +Z face, so the returned normal must come back rotated into world space.
    let mover = Capsule { center1: vec3(-0.3, 0.0, 0.6), center2: vec3(0.3, 0.0, 0.6), radius: 0.2 };
    let mut planes = [BodyPlaneResult::default(); 4];
    let body_transform = WorldTransform {
        p: pos(0.0, 0.0, 0.0),
        q: make_quat_from_axis_angle(vec3(1.0, 0.0, 0.0), 0.5 * PI),
    };
    let count = body_collide_mover(
        &world,
        body_id,
        &mut planes,
        pos(0.0, 0.0, 0.0),
        &mover,
        default_query_filter(),
        body_transform,
    );

    ensure!(count == 1);
    ensure!(is_normalized(planes[0].result.plane.normal));
    ensure!(planes[0].result.plane.normal.z > 0.99);
    ensure_small!(planes[0].result.plane.offset - 0.1, 1e-4);

    destroy_world(world);
}

#[test]
fn mover_capacity() {
    let (mut world, body_id) = create_query_world();

    // Two spheres each touch a mover that runs between them along X at y = 0.
    let shape_def = default_shape_def();
    let left = Sphere { center: vec3(-0.4, 0.6, 0.0), radius: 0.5 };
    let right = Sphere { center: vec3(0.4, 0.6, 0.0), radius: 0.5 };
    makepad_box3d::shape::create_sphere_shape(&mut world, body_id, &shape_def, &left);
    makepad_box3d::shape::create_sphere_shape(&mut world, body_id, &shape_def, &right);

    let mover = Capsule { center1: vec3(-1.0, 0.0, 0.0), center2: vec3(1.0, 0.0, 0.0), radius: 0.2 };
    let mut planes = [BodyPlaneResult::default(); 4];
    let body_transform = identity_at(0.0, 0.0, 0.0);

    // Capacity caps the result and prevents writing past the buffer.
    let capped = body_collide_mover(
        &world,
        body_id,
        &mut planes[..1],
        pos(0.0, 0.0, 0.0),
        &mover,
        default_query_filter(),
        body_transform,
    );
    ensure!(capped == 1);

    let full = body_collide_mover(
        &world,
        body_id,
        &mut planes,
        pos(0.0, 0.0, 0.0),
        &mover,
        default_query_filter(),
        body_transform,
    );
    ensure!(full == 2);

    destroy_world(world);
}
