// Port of box3d/test/test_collision.c
// The BOX3D_DOUBLE_PRECISION blocks are not ported (single precision build).

use makepad_box3d::aabb::ray_cast_aabb;
use makepad_box3d::constants::speculative_distance;
use makepad_box3d::convex_manifold::collide_hulls;
use makepad_box3d::hull::{create_hull, find_hull_support_face, make_box_hull};
use makepad_box3d::math_functions::*;
use makepad_box3d::shape::{compute_fat_shape_aabb, Shape, ShapeGeometry};
use makepad_box3d::triangle_manifold::collide_hull_and_triangle;
use makepad_box3d::types::{HullData, LocalManifold, SATCache, SeparatingFeature, TriangleFeature};
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
    let xf_bo = WorldTransform { p: offset_pos(POS_ZERO, sep), q: Quat::IDENTITY };
    let mut cache_origin = SATCache::default();
    collide_hulls(
        &mut m_origin,
        8,
        &box_a,
        &box_b,
        inv_mul_world_transforms(xf_ao, xf_bo),
        &mut cache_origin,
        speculative_distance(),
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

// Port of the BOX3D_DOUBLE_PRECISION half of LargeWorldManifoldTest: the same relative
// configuration shifted far from the origin. The relative pose differences the world
// positions in double, so in double the frame A manifold is preserved to float precision.
// In float it would collapse since the offset is below the ULP.
#[cfg(feature = "double-precision")]
#[test]
fn large_world_manifold_double_precision_test() {
    let box_a = make_box_hull(0.5, 0.5, 0.5);
    let box_b = make_box_hull(0.5, 0.5, 0.5);
    let sep = vec3(0.9, 0.0, 0.0);

    let mut m_origin = LocalManifold::default();
    let xf_ao = WORLD_TRANSFORM_IDENTITY;
    let xf_bo = WorldTransform { p: offset_pos(POS_ZERO, sep), q: Quat::IDENTITY };
    let mut cache_origin = SATCache::default();
    collide_hulls(
        &mut m_origin,
        8,
        &box_a,
        &box_b,
        inv_mul_world_transforms(xf_ao, xf_bo),
        &mut cache_origin,
        speculative_distance(),
    );
    ensure!(m_origin.point_count() == 4);

    let base = offset_pos(POS_ZERO, vec3(1.0e7, 1.0e7, 1.0e7));

    let mut m_large = LocalManifold::default();
    let xf_al = WorldTransform { p: base, q: Quat::IDENTITY };
    let xf_bl = WorldTransform { p: offset_pos(base, sep), q: Quat::IDENTITY };
    let mut cache_large = SATCache::default();
    collide_hulls(
        &mut m_large,
        8,
        &box_a,
        &box_b,
        inv_mul_world_transforms(xf_al, xf_bl),
        &mut cache_large,
        speculative_distance(),
    );

    ensure!(m_large.point_count() == m_origin.point_count());
    ensure_small!(m_large.normal.x - m_origin.normal.x, 1e-4);
    ensure_small!(m_large.normal.y - m_origin.normal.y, 1e-4);
    ensure_small!(m_large.normal.z - m_origin.normal.z, 1e-4);
    for i in 0..m_large.point_count() {
        let i = i as usize;
        ensure_small!(m_large.points[i].separation - m_origin.points[i].separation, 1e-4);
        ensure_small!(m_large.points[i].point.x - m_origin.points[i].point.x, 1e-4);
        ensure_small!(m_large.points[i].point.y - m_origin.points[i].point.y, 1e-4);
        ensure_small!(m_large.points[i].point.z - m_origin.points[i].point.z, 1e-4);
    }
}

// Port of the BOX3D_DOUBLE_PRECISION half of LargeWorldAABBTest: broad-phase AABBs are
// built in double and narrowed to float with directed outward rounding, so a shape and
// its speculative margin stay inside their box far from the origin.
#[cfg(feature = "double-precision")]
#[test]
fn large_world_aabb_double_precision_test() {
    use makepad_box3d::math_functions::Pos;

    let box_hull = make_box_hull(0.5, 0.5, 0.5);
    let mut shape = Shape::default();
    shape.geom = ShapeGeometry::Hull(box_hull);

    let d = 1.0e7f64;
    let xf_large = WorldTransform { p: Pos { x: d, y: d, z: d }, q: Quat::IDENTITY };

    // Tight world AABB still contains the 0.5 m extent
    let tight = compute_fat_shape_aabb(&shape, xf_large, 0.0);
    ensure!((tight.lower_bound.x as f64) <= d - 0.5);
    ensure!((tight.lower_bound.y as f64) <= d - 0.5);
    ensure!((tight.lower_bound.z as f64) <= d - 0.5);
    ensure!((tight.upper_bound.x as f64) >= d + 0.5);
    ensure!((tight.upper_bound.y as f64) >= d + 0.5);
    ensure!((tight.upper_bound.z as f64) >= d + 0.5);

    // The fat helper folds the extra into the double step before the single outward
    // rounding, so a margin smaller than a float ULP at this range survives instead of
    // becoming a no-op subtract.
    let extra = 0.05f32;
    let fat = compute_fat_shape_aabb(&shape, xf_large, extra);
    ensure!((fat.lower_bound.x as f64) <= d - 0.5 - extra as f64);
    ensure!((fat.lower_bound.y as f64) <= d - 0.5 - extra as f64);
    ensure!((fat.lower_bound.z as f64) <= d - 0.5 - extra as f64);
    ensure!((fat.upper_bound.x as f64) >= d + 0.5 + extra as f64);
    ensure!((fat.upper_bound.y as f64) >= d + 0.5 + extra as f64);
    ensure!((fat.upper_bound.z as f64) >= d + 0.5 + extra as f64);
}

fn pyramid_with_regular_base(sides: i32, apex_y: f32, phase: f32) -> std::sync::Arc<HullData> {
    let mut points = Vec::new();
    for i in 0..sides {
        let angle = (i as f32 + phase) * (std::f32::consts::TAU / sides as f32);
        let (s, c) = angle.sin_cos();
        points.push(vec3(c, 0.0, s));
    }
    points.push(vec3(0.0, apex_y, 0.0));
    let hull = create_hull(&points, sides + 1).expect("the pyramid must be a valid convex hull");
    let base = find_hull_support_face(&hull, vec3(0.0, -apex_y, 0.0)) as u8;
    assert_eq!(hull.edges.iter().filter(|e| e.face == base).count(), sides as usize, "the full base must clip");
    hull
}

#[test]
fn many_sided_faces_clip_to_a_valid_manifold() {
    // Alternating the corners of two regular 40-gons gives an 80-vertex
    // intersection before reduction, in either choice of reference frame.
    let lower = pyramid_with_regular_base(40, -1.0, 0.0);
    let upper = pyramid_with_regular_base(40, 1.0, 0.5);
    let mut local = LocalManifold::default();
    collide_hulls(
        &mut local,
        4,
        &lower,
        &upper,
        Transform::IDENTITY,
        &mut SATCache::default(),
        speculative_distance(),
    );
    assert_eq!(local.point_count(), 4, "the clipped base must reduce to four supporting points");
    assert!(local.normal.y > 0.999, "the normal must point from the lower base to the upper one");
    for p in &local.points {
        assert!(
            p.separation.abs() < 1e-5 && p.point.y.abs() < 1e-5 && (length(p.point) - 1.0).abs() < 0.01,
            "every reduced point must lie on the contact perimeter"
        );
    }
}

#[test]
fn triangle_clips_a_many_sided_face_in_both_orders() {
    const SIDES: i32 = 63;
    let hull = pyramid_with_regular_base(SIDES, 1.0, 0.0);
    // Each triangle side cuts one corner of the base, halfway between the
    // extreme vertex and its two neighbours along that side's normal.
    let radius = 1.0 + (std::f32::consts::TAU / SIDES as f32).cos();
    let [a, b, c] = [1.0_f32, -1.0, -3.0].map(|i| {
        let (s, c) = (i * std::f32::consts::FRAC_PI_3).sin_cos();
        vec3(radius * c, 0.0, radius * s)
    });
    let mut local = LocalManifold::default();
    for (axis, feature) in [
        (SeparatingFeature::ManualFaceAxisB, TriangleFeature::HullFace),
        (SeparatingFeature::ManualFaceAxisA, TriangleFeature::TriangleFace),
    ] {
        let mut cache = SATCache { type_: axis as u8, ..Default::default() };
        collide_hull_and_triangle(&mut local, i32::MAX, &hull, a, b, c, 0, &mut cache);
        assert_eq!(local.feature, feature, "the requested face must be the reference");
        assert_eq!(local.point_count(), SIDES + 3, "each cut must replace one corner with two intersections");
        assert!(local.normal.y > 0.999, "both reference choices must support the pyramid from below");
        for p in &local.points {
            assert!(
                p.separation.abs() < 1e-5 && p.point.y.abs() < 1e-5 && (length(p.point) - 1.0).abs() < 0.01,
                "all 66 points must be on the shared perimeter"
            );
        }
        // The mesh caller can request a smaller output even when the full
        // polygon needs more space. Its ordered prefix determines warm starts.
        let mut limited = LocalManifold::default();
        let mut cache = SATCache { type_: axis as u8, ..Default::default() };
        collide_hull_and_triangle(&mut limited, 32, &hull, a, b, c, 0, &mut cache);
        assert_eq!(limited.point_count(), 32, "the public output capacity must be honoured");
        for (p, full) in limited.points.iter().zip(&local.points) {
            assert_eq!(
                (p.point, p.separation, p.pair),
                (full.point, full.separation, full.pair),
                "a bounded output must preserve the full polygon's point and feature order"
            );
        }
    }
}
