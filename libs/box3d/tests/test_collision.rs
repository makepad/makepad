// Port of box3d/test/test_collision.c
// The BOX3D_DOUBLE_PRECISION blocks are not ported (single precision build).

use makepad_box3d::aabb::ray_cast_aabb;
use makepad_box3d::convex_manifold::collide_hulls;
use makepad_box3d::hull::make_box_hull;
use makepad_box3d::math_functions::*;
use makepad_box3d::shape::{compute_fat_shape_aabb, Shape, ShapeGeometry};
use makepad_box3d::types::{LocalManifold, SATCache};
use makepad_box3d::{ensure, ensure_small};

#[test]
fn aabb_test() {
    let mut a = AABB {
        lower_bound: vec3(-1.0, -1.0, -1.0),
        upper_bound: vec3(-2.0, -2.0, -2.0),
    };

    ensure!(is_valid_aabb(a) == false);

    a.upper_bound = vec3(1.0, 1.0, 0.0);
    ensure!(is_valid_aabb(a) == true);

    let b = AABB {
        lower_bound: vec3(2.0, 2.0, 0.0),
        upper_bound: vec3(4.0, 4.0, 0.0),
    };
    ensure!(aabb_overlaps(a, b) == false);
    ensure!(aabb_contains(a, b) == false);
}

#[test]
fn test_ray_aabb_intersection() {
    // Test 1: Ray passing through center of AABB
    {
        let a = AABB { lower_bound: vec3(-1.0, -1.0, -1.0), upper_bound: vec3(1.0, 1.0, 1.0) };
        let p1 = vec3(-2.0, 0.0, 0.0);
        let p2 = vec3(2.0, 0.0, 0.0);
        let (mut min_fraction, mut max_fraction) = (0.0f32, 0.0f32);

        let hit = ray_cast_aabb(a, p1, p2, &mut min_fraction, &mut max_fraction);

        ensure!(hit == true);
        ensure!(abs_float(min_fraction - 0.25) < 0.001); // Enters at 25% of ray
        ensure!(abs_float(max_fraction - 0.75) < 0.001); // Exits at 75% of ray
    }

    // Test 2: Ray starting inside AABB
    {
        let a = AABB { lower_bound: vec3(-1.0, -1.0, -1.0), upper_bound: vec3(1.0, 1.0, 1.0) };
        let p1 = vec3(0.0, 0.0, 0.0);
        let p2 = vec3(2.0, 0.0, 0.0);
        let (mut min_fraction, mut max_fraction) = (0.0f32, 0.0f32);

        let hit = ray_cast_aabb(a, p1, p2, &mut min_fraction, &mut max_fraction);

        ensure!(hit == true);
        ensure!(min_fraction == 0.0); // Starts inside
        ensure!(abs_float(max_fraction - 0.5) < 0.001); // Exits at 50% of ray
    }

    // Test 3: Ray ending inside AABB
    {
        let a = AABB { lower_bound: vec3(-1.0, -1.0, -1.0), upper_bound: vec3(1.0, 1.0, 1.0) };
        let p1 = vec3(-2.0, 0.0, 0.0);
        let p2 = vec3(0.0, 0.0, 0.0);
        let (mut min_fraction, mut max_fraction) = (0.0f32, 0.0f32);

        let hit = ray_cast_aabb(a, p1, p2, &mut min_fraction, &mut max_fraction);

        ensure!(hit == true);
        ensure!(abs_float(min_fraction - 0.5) < 0.001); // Enters at 50% of ray
        ensure!(max_fraction == 1.0); // Ends inside
    }

    // Test 4: Ray completely inside AABB
    {
        let a = AABB { lower_bound: vec3(-2.0, -2.0, -2.0), upper_bound: vec3(2.0, 2.0, 2.0) };
        let p1 = vec3(-1.0, 0.0, 0.0);
        let p2 = vec3(1.0, 0.0, 0.0);
        let (mut min_fraction, mut max_fraction) = (0.0f32, 0.0f32);

        let hit = ray_cast_aabb(a, p1, p2, &mut min_fraction, &mut max_fraction);

        ensure!(hit == true);
        ensure!(min_fraction == 0.0);
        ensure!(max_fraction == 1.0);
    }

    // Test 5: Ray missing AABB
    {
        let a = AABB { lower_bound: vec3(0.0, 0.0, 0.0), upper_bound: vec3(1.0, 1.0, 1.0) };
        let p1 = vec3(-1.0, 2.0, 0.5);
        let p2 = vec3(2.0, 2.0, 0.5);
        let (mut min_fraction, mut max_fraction) = (0.0f32, 0.0f32);

        let hit = ray_cast_aabb(a, p1, p2, &mut min_fraction, &mut max_fraction);

        ensure!(hit == false);
    }

    // Test 6: Ray parallel to AABB face (no intersection)
    {
        let a = AABB { lower_bound: vec3(0.0, 0.0, 0.0), upper_bound: vec3(1.0, 1.0, 1.0) };
        let p1 = vec3(-1.0, 2.0, 0.5);
        let p2 = vec3(2.0, 2.0, 0.5);
        let (mut min_fraction, mut max_fraction) = (0.0f32, 0.0f32);

        let hit = ray_cast_aabb(a, p1, p2, &mut min_fraction, &mut max_fraction);

        ensure!(hit == false);
    }

    // Test 7: Ray parallel to AABB face (within bounds)
    {
        let a = AABB { lower_bound: vec3(0.0, 0.0, 0.0), upper_bound: vec3(1.0, 1.0, 1.0) };
        let p1 = vec3(-1.0, 0.5, 0.5);
        let p2 = vec3(2.0, 0.5, 0.5);
        let (mut min_fraction, mut max_fraction) = (0.0f32, 0.0f32);

        let hit = ray_cast_aabb(a, p1, p2, &mut min_fraction, &mut max_fraction);

        ensure!(hit == true);
        ensure!(abs_float(min_fraction - 1.0 / 3.0) < 0.001);
        ensure!(abs_float(max_fraction - 2.0 / 3.0) < 0.001);
    }

    // Test 8: Degenerate ray (point) inside AABB
    {
        let a = AABB { lower_bound: vec3(0.0, 0.0, 0.0), upper_bound: vec3(1.0, 1.0, 1.0) };
        let p1 = vec3(0.5, 0.5, 0.5);
        let p2 = vec3(0.5, 0.5, 0.5);
        let (mut min_fraction, mut max_fraction) = (0.0f32, 0.0f32);

        let hit = ray_cast_aabb(a, p1, p2, &mut min_fraction, &mut max_fraction);

        ensure!(hit == true);
        ensure!(min_fraction == 0.0);
        ensure!(max_fraction == 0.0);
    }

    // Test 9: Degenerate ray (point) outside AABB
    {
        let a = AABB { lower_bound: vec3(0.0, 0.0, 0.0), upper_bound: vec3(1.0, 1.0, 1.0) };
        let p1 = vec3(2.0, 2.0, 2.0);
        let p2 = vec3(2.0, 2.0, 2.0);
        let (mut min_fraction, mut max_fraction) = (0.0f32, 0.0f32);

        let hit = ray_cast_aabb(a, p1, p2, &mut min_fraction, &mut max_fraction);

        ensure!(hit == false);
    }

    // Test 10: Ray pointing away from AABB
    {
        let a = AABB { lower_bound: vec3(0.0, 0.0, 0.0), upper_bound: vec3(1.0, 1.0, 1.0) };
        let p1 = vec3(-1.0, 0.5, 0.5);
        let p2 = vec3(-2.0, 0.5, 0.5);
        let (mut min_fraction, mut max_fraction) = (0.0f32, 0.0f32);

        let hit = ray_cast_aabb(a, p1, p2, &mut min_fraction, &mut max_fraction);

        ensure!(hit == false);
    }

    // Test 11: Ray hitting corner of AABB
    {
        let a = AABB { lower_bound: vec3(0.0, 0.0, 0.0), upper_bound: vec3(1.0, 1.0, 1.0) };
        let p1 = vec3(-1.0, -1.0, -1.0);
        let p2 = vec3(2.0, 2.0, 2.0);
        let (mut min_fraction, mut max_fraction) = (0.0f32, 0.0f32);

        let hit = ray_cast_aabb(a, p1, p2, &mut min_fraction, &mut max_fraction);

        ensure!(hit == true);
        ensure!(abs_float(min_fraction - 1.0 / 3.0) < 0.001);
        ensure!(abs_float(max_fraction - 2.0 / 3.0) < 0.001);
    }

    // Test 12: Ray grazing edge of AABB
    {
        let a = AABB { lower_bound: vec3(0.0, 0.0, 0.0), upper_bound: vec3(1.0, 1.0, 1.0) };
        let p1 = vec3(-1.0, 0.0, 0.5);
        let p2 = vec3(2.0, 0.0, 0.5);
        let (mut min_fraction, mut max_fraction) = (0.0f32, 0.0f32);

        let hit = ray_cast_aabb(a, p1, p2, &mut min_fraction, &mut max_fraction);

        ensure!(hit == true);
        ensure!(abs_float(min_fraction - 1.0 / 3.0) < 0.001);
        ensure!(abs_float(max_fraction - 2.0 / 3.0) < 0.001);
    }
}

// The narrow phase differences the two world positions then works in frame A, so a
// manifold far from the origin must match the same manifold at the origin.
// (The far-from-origin half is BOX3D_DOUBLE_PRECISION only and is not ported.)
#[test]
fn large_world_manifold_test() {
    let box_a = make_box_hull(0.5, 0.5, 0.5);
    let box_b = make_box_hull(0.5, 0.5, 0.5);

    // Centers 0.9 apart so the cubes overlap by 0.1 along x
    let sep = vec3(0.9, 0.0, 0.0);

    let mut m_origin = LocalManifold::default();

    let xf_ao = WORLD_TRANSFORM_IDENTITY;
    let xf_bo = Transform { p: offset_pos(POS_ZERO, sep), q: Quat::IDENTITY };
    let mut cache_origin = SATCache::default();
    collide_hulls(
        &mut m_origin,
        8,
        &box_a,
        &box_b,
        inv_mul_world_transforms(xf_ao, xf_bo),
        &mut cache_origin,
    );

    // Two cube faces overlap, so the clipped manifold has four points
    ensure!(m_origin.point_count() == 4);
    for i in 0..m_origin.point_count() {
        ensure_small!(m_origin.points[i as usize].separation + 0.1, 0.01);
    }
}

// Broad-phase AABBs must contain the shape and its speculative margin.
// (The far-from-origin half is BOX3D_DOUBLE_PRECISION only and is not ported.)
#[test]
fn large_world_aabb_test() {
    // Unit cube, so the tight extent is 0.5 each way
    let box_hull = make_box_hull(0.5, 0.5, 0.5);
    let mut shape = Shape::default();
    shape.geom = ShapeGeometry::Hull(box_hull);

    let aabb_origin = compute_fat_shape_aabb(&shape, WORLD_TRANSFORM_IDENTITY, 0.0);
    ensure_small!(aabb_origin.lower_bound.x + 0.5, f32::EPSILON);
    ensure_small!(aabb_origin.lower_bound.y + 0.5, f32::EPSILON);
    ensure_small!(aabb_origin.lower_bound.z + 0.5, f32::EPSILON);
    ensure_small!(aabb_origin.upper_bound.x - 0.5, f32::EPSILON);
    ensure_small!(aabb_origin.upper_bound.y - 0.5, f32::EPSILON);
    ensure_small!(aabb_origin.upper_bound.z - 0.5, f32::EPSILON);
}
