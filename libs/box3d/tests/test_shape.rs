// Port of box3d/test/test_shape.c

use std::sync::Arc;

use makepad_box3d::capsule::{compute_capsule_aabb, compute_capsule_mass, ray_cast_capsule};
use makepad_box3d::hull::{compute_hull_aabb, compute_hull_mass, create_hull, make_box_hull, make_transformed_box_hull, ray_cast_hull};
use makepad_box3d::math_functions::*;
use makepad_box3d::sphere::{compute_sphere_aabb, compute_sphere_mass, ray_cast_sphere};
use makepad_box3d::types::{Capsule, CastOutput, HullData, RayCastInput, Sphere};
use makepad_box3d::{ensure, ensure_small};

const N: usize = 4;

fn test_capsule() -> Capsule {
    Capsule { center1: vec3(-1.0, 0.0, 0.0), center2: vec3(1.0, 0.0, 0.0), radius: 1.0 }
}

fn test_sphere() -> Sphere {
    Sphere { center: vec3(1.0, 0.0, 0.0), radius: 1.0 }
}

fn test_box() -> Arc<HullData> {
    make_box_hull(1.0, 1.0, 1.0)
}

#[test]
fn shape_mass_test() {
    let sphere = test_sphere();
    let capsule = test_capsule();
    let box_hull = test_box();

    // Sphere
    {
        let md = compute_sphere_mass(&sphere, 1.0);
        let mass = 4.0 / 3.0 * PI;
        ensure_small!(md.mass - mass, f32::EPSILON);
        ensure!(md.center.x == 1.0 && md.center.y == 0.0);

        // Inertia is now about the shape center of mass, so the offset does not appear.
        let inertia = 2.0 / 5.0 * mass;
        ensure_small!(md.inertia.cx.x - inertia, f32::EPSILON);
        ensure_small!(md.inertia.cy.y - inertia, f32::EPSILON);
        ensure_small!(md.inertia.cz.z - inertia, f32::EPSILON);
    }

    // Analytic box hull
    {
        let md = compute_hull_mass(&box_hull, 1.0);
        let mass = 2.0 * 2.0 * 2.0;
        ensure_small!(md.mass - mass, f32::EPSILON);
        ensure_small!(md.center.x, f32::EPSILON);
        ensure_small!(md.center.y, f32::EPSILON);
        ensure_small!(md.center.z, f32::EPSILON);
        let inertia = (1.0 / 12.0) * mass * (2.0 * 2.0 + 2.0 * 2.0);
        ensure_small!(md.inertia.cx.x - inertia, 2.0 * f32::EPSILON);
        ensure_small!(md.inertia.cy.y - inertia, 2.0 * f32::EPSILON);
        ensure_small!(md.inertia.cz.z - inertia, 2.0 * f32::EPSILON);
    }

    // Translated box
    {
        let offset = vec3(0.4, -0.7, 0.1);
        let transform = Transform { p: offset, q: Quat::IDENTITY };
        let h = vec3(0.25, 0.5, 0.3);
        let b1 = make_box_hull(h.x, h.y, h.z);
        let b2 = make_transformed_box_hull(h.x, h.y, h.z, transform);

        let m1 = compute_hull_mass(&b1, 1.0);
        let m2 = compute_hull_mass(&b2, 1.0);

        ensure_small!(m1.mass - m2.mass, f32::EPSILON);

        let d = sub_mm(b1.central_inertia, b2.central_inertia);
        ensure_small!(d.cx.x, f32::EPSILON);
        ensure_small!(d.cx.y, f32::EPSILON);
        ensure_small!(d.cx.z, f32::EPSILON);
        ensure_small!(d.cy.x, f32::EPSILON);
        ensure_small!(d.cy.y, f32::EPSILON);
        ensure_small!(d.cy.z, f32::EPSILON);
        ensure_small!(d.cz.x, f32::EPSILON);
        ensure_small!(d.cz.y, f32::EPSILON);
        ensure_small!(d.cz.z, f32::EPSILON);

        ensure_small!(m2.center.x - offset.x, f32::EPSILON);
        ensure_small!(m2.center.y - offset.y, f32::EPSILON);
        ensure_small!(m2.center.z - offset.z, f32::EPSILON);
    }

    // Rotated box
    {
        let h1 = vec3(0.25, 0.5, 0.3);
        let h2 = vec3(0.25, 0.3, 0.5);
        let q = compute_quat_between_unit_vectors(Vec3::AXIS_Y, Vec3::AXIS_Z);
        let transform = Transform { p: Vec3::ZERO, q };
        let b1 = make_transformed_box_hull(h1.x, h1.y, h1.z, transform);
        let b2 = make_box_hull(h2.x, h2.y, h2.z);

        let m1 = compute_hull_mass(&b1, 1.0);
        let m2 = compute_hull_mass(&b2, 1.0);

        ensure_small!(m1.mass - m2.mass, f32::EPSILON);

        let d = sub_mm(b1.central_inertia, b2.central_inertia);
        ensure_small!(d.cx.x, f32::EPSILON);
        ensure_small!(d.cx.y, f32::EPSILON);
        ensure_small!(d.cx.z, f32::EPSILON);
        ensure_small!(d.cy.x, f32::EPSILON);
        ensure_small!(d.cy.y, f32::EPSILON);
        ensure_small!(d.cy.z, f32::EPSILON);
        ensure_small!(d.cz.x, f32::EPSILON);
        ensure_small!(d.cz.y, f32::EPSILON);
        ensure_small!(d.cz.z, f32::EPSILON);

        ensure_small!(m1.center.x - m2.center.x, f32::EPSILON);
        ensure_small!(m1.center.y - m2.center.y, f32::EPSILON);
        ensure_small!(m1.center.z - m2.center.z, f32::EPSILON);
    }

    // Transformed box
    {
        let offset = vec3(0.4, -0.7, 0.1);
        let h1 = vec3(0.25, 0.5, 0.3);
        let h2 = vec3(0.25, 0.3, 0.5);
        let q = compute_quat_between_unit_vectors(Vec3::AXIS_Y, Vec3::AXIS_Z);
        let transform = Transform { p: offset, q };
        let b1 = make_transformed_box_hull(h1.x, h1.y, h1.z, transform);
        let b2 = make_box_hull(h2.x, h2.y, h2.z);

        let m1 = compute_hull_mass(&b1, 1.0);
        let m2 = compute_hull_mass(&b2, 1.0);

        ensure_small!(m1.mass - m2.mass, f32::EPSILON);

        let d = sub_mm(b1.central_inertia, b2.central_inertia);
        ensure_small!(d.cx.x, f32::EPSILON);
        ensure_small!(d.cx.y, f32::EPSILON);
        ensure_small!(d.cx.z, f32::EPSILON);
        ensure_small!(d.cy.x, f32::EPSILON);
        ensure_small!(d.cy.y, f32::EPSILON);
        ensure_small!(d.cy.z, f32::EPSILON);
        ensure_small!(d.cz.x, f32::EPSILON);
        ensure_small!(d.cz.y, f32::EPSILON);
        ensure_small!(d.cz.z, f32::EPSILON);

        ensure_small!(m1.center.x - offset.x, f32::EPSILON);
        ensure_small!(m1.center.y - offset.y, f32::EPSILON);
        ensure_small!(m1.center.z - offset.z, f32::EPSILON);
    }

    // Capsule
    {
        let radius = capsule.radius;
        let length = distance(capsule.center1, capsule.center2);

        // Capsule along x-axis
        let md = compute_capsule_mass(&capsule, 1.0);

        // Box that fully contains capsule. Upper bound on capsule mass.
        let r = make_box_hull(radius + 0.5 * length, radius, radius);
        let md_upper = compute_hull_mass(&r, 1.0);

        // Approximate capsule using convex hull. This should be a lower bound on the
        // capsule mass.
        let mut points = [Vec3::ZERO; 2 * N * N];
        let d = PI / (N as f32 - 1.0);
        let mut angle1 = -0.5 * PI;
        let mut index = 0;
        for _i in 0..N {
            let s1 = angle1.sin();
            let c1 = angle1.cos();
            let mut angle2 = -0.5 * PI;
            for _j in 0..N {
                points[index].x = 1.0 + radius * c1;
                points[index].y = radius * s1 * angle2.cos();
                points[index].z = radius * s1 * angle2.sin();
                angle2 += d;
                index += 1;
            }

            angle1 += d;
        }

        let mut angle1 = 0.5 * PI;
        for _i in 0..N {
            let s1 = angle1.sin();
            let c1 = angle1.cos();
            let mut angle2 = -0.5 * PI;
            for _j in 0..N {
                points[index].x = -1.0 + radius * c1;
                points[index].y = radius * s1 * angle2.cos();
                points[index].z = radius * s1 * angle2.sin();
                angle2 += d;
                index += 1;
            }

            angle1 += d;
        }

        ensure!(index == 2 * N * N);

        let hull = create_hull(&points, (2 * N * N) as i32).unwrap();
        let md_lower = compute_hull_mass(&hull, 1.0);

        ensure!(md_lower.mass < md.mass && md.mass < md_upper.mass);
        ensure!(md_lower.inertia.cx.x < md.inertia.cx.x && md.inertia.cx.x < md_upper.inertia.cx.x);
        ensure!(md_lower.inertia.cy.y < md.inertia.cy.y && md.inertia.cy.y < md_upper.inertia.cy.y);
        ensure!(md_lower.inertia.cz.z < md.inertia.cz.z && md.inertia.cz.z < md_upper.inertia.cz.z);
    }
}

#[test]
fn shape_aabb_test() {
    let sphere = test_sphere();
    let capsule = test_capsule();
    let box_hull = test_box();

    {
        let b = compute_sphere_aabb(&sphere, Transform::IDENTITY);
        ensure_small!(b.lower_bound.x, f32::EPSILON);
        ensure_small!(b.lower_bound.y + 1.0, f32::EPSILON);
        ensure_small!(b.lower_bound.z + 1.0, f32::EPSILON);
        ensure_small!(b.upper_bound.x - 2.0, f32::EPSILON);
        ensure_small!(b.upper_bound.y - 1.0, f32::EPSILON);
        ensure_small!(b.upper_bound.z - 1.0, f32::EPSILON);
    }

    {
        let b = compute_capsule_aabb(&capsule, Transform::IDENTITY);
        ensure_small!(b.lower_bound.x + 2.0, f32::EPSILON);
        ensure_small!(b.lower_bound.y + 1.0, f32::EPSILON);
        ensure_small!(b.lower_bound.z + 1.0, f32::EPSILON);
        ensure_small!(b.upper_bound.x - 2.0, f32::EPSILON);
        ensure_small!(b.upper_bound.y - 1.0, f32::EPSILON);
        ensure_small!(b.upper_bound.z - 1.0, f32::EPSILON);
    }
    {
        let b = compute_hull_aabb(&box_hull, Transform::IDENTITY);
        ensure_small!(b.lower_bound.x + 1.0, f32::EPSILON);
        ensure_small!(b.lower_bound.y + 1.0, f32::EPSILON);
        ensure_small!(b.lower_bound.z + 1.0, f32::EPSILON);
        ensure_small!(b.upper_bound.x - 1.0, f32::EPSILON);
        ensure_small!(b.upper_bound.y - 1.0, f32::EPSILON);
        ensure_small!(b.upper_bound.z - 1.0, f32::EPSILON);
    }
}

// Shared assertions for a surface hit. The normal points outward toward the ray,
// the point sits on the surface, and the point lies on the ray at the reported fraction.
fn check_cast_hit(out: CastOutput, origin: Vec3, translation: Vec3, point: Vec3, normal: Vec3, fraction: f32, tol: f32) {
    ensure!(out.hit);
    ensure_small!(out.fraction - fraction, tol);
    ensure_small!(out.point.x - point.x, tol);
    ensure_small!(out.point.y - point.y, tol);
    ensure_small!(out.point.z - point.z, tol);
    ensure_small!(out.normal.x - normal.x, tol);
    ensure_small!(out.normal.y - normal.y, tol);
    ensure_small!(out.normal.z - normal.z, tol);

    let on_ray = mul_add(origin, out.fraction, translation);
    ensure_small!(distance(out.point, on_ray), tol);
}

// The shared initial overlap convention: a ray starting inside a solid reports the origin
// with zero fraction and no normal.
fn check_initial_overlap(out: CastOutput, origin: Vec3) {
    ensure!(out.hit);
    ensure!(out.fraction == 0.0);
    ensure_small!(distance(out.point, origin), f32::EPSILON);
    ensure!(out.normal.x == 0.0 && out.normal.y == 0.0 && out.normal.z == 0.0);
}

#[test]
fn ray_cast_sphere_hit_test() {
    let s = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 1.0 };

    // Hit along each principal axis. Surface at distance 3 over a length 8 ray.
    {
        let input = RayCastInput { origin: vec3(-4.0, 0.0, 0.0), translation: vec3(8.0, 0.0, 0.0), max_fraction: 1.0 };
        let out = ray_cast_sphere(&s, &input);
        check_cast_hit(out, input.origin, input.translation, vec3(-1.0, 0.0, 0.0), vec3(-1.0, 0.0, 0.0), 3.0 / 8.0, 1e-5);
    }
    {
        let input = RayCastInput { origin: vec3(0.0, 4.0, 0.0), translation: vec3(0.0, -8.0, 0.0), max_fraction: 1.0 };
        let out = ray_cast_sphere(&s, &input);
        check_cast_hit(out, input.origin, input.translation, vec3(0.0, 1.0, 0.0), vec3(0.0, 1.0, 0.0), 3.0 / 8.0, 1e-5);
    }
    {
        let input = RayCastInput { origin: vec3(0.0, 0.0, -4.0), translation: vec3(0.0, 0.0, 8.0), max_fraction: 1.0 };
        let out = ray_cast_sphere(&s, &input);
        check_cast_hit(out, input.origin, input.translation, vec3(0.0, 0.0, -1.0), vec3(0.0, 0.0, -1.0), 3.0 / 8.0, 1e-5);
    }

    // Offset center, hit partway along the ray.
    {
        let s2 = Sphere { center: vec3(5.0, 0.0, 0.0), radius: 2.0 };
        let input = RayCastInput { origin: vec3(0.0, 0.0, 0.0), translation: vec3(10.0, 0.0, 0.0), max_fraction: 1.0 };
        let out = ray_cast_sphere(&s2, &input);
        check_cast_hit(out, input.origin, input.translation, vec3(3.0, 0.0, 0.0), vec3(-1.0, 0.0, 0.0), 0.3, 1e-5);
    }

    // Diagonal ray straight through the center.
    {
        let k = 0.70710678;
        let input = RayCastInput { origin: vec3(-3.0, -3.0, 0.0), translation: vec3(6.0, 6.0, 0.0), max_fraction: 1.0 };
        let out = ray_cast_sphere(&s, &input);
        check_cast_hit(out, input.origin, input.translation, vec3(-k, -k, 0.0), vec3(-k, -k, 0.0), 0.382149, 1e-4);
    }
}

#[test]
fn ray_cast_sphere_miss_test() {
    let s = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 1.0 };

    // Pointing away.
    {
        let input = RayCastInput { origin: vec3(-4.0, 0.0, 0.0), translation: vec3(-8.0, 0.0, 0.0), max_fraction: 1.0 };
        ensure!(!ray_cast_sphere(&s, &input).hit);
    }
    // Passes wide of the sphere.
    {
        let input = RayCastInput { origin: vec3(-4.0, 3.0, 0.0), translation: vec3(8.0, 0.0, 0.0), max_fraction: 1.0 };
        ensure!(!ray_cast_sphere(&s, &input).hit);
    }
    // Aimed at the sphere but the translation stops short.
    {
        let input = RayCastInput { origin: vec3(-4.0, 0.0, 0.0), translation: vec3(8.0, 0.0, 0.0), max_fraction: 0.3 };
        ensure!(!ray_cast_sphere(&s, &input).hit);
    }
}

#[test]
fn ray_cast_sphere_clip_test() {
    let s = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 1.0 };

    // The surface is reached at fraction 3/8. Straddle it with max_fraction.
    {
        let input = RayCastInput { origin: vec3(-4.0, 0.0, 0.0), translation: vec3(8.0, 0.0, 0.0), max_fraction: 0.374 };
        ensure!(!ray_cast_sphere(&s, &input).hit);
    }
    {
        let input = RayCastInput { origin: vec3(-4.0, 0.0, 0.0), translation: vec3(8.0, 0.0, 0.0), max_fraction: 0.376 };
        let out = ray_cast_sphere(&s, &input);
        ensure!(out.hit);
        ensure_small!(out.fraction - 3.0 / 8.0, 1e-5);
    }
}

#[test]
fn ray_cast_sphere_interior_test() {
    let s = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 1.0 };

    // Origin inside reports the origin with zero fraction.
    {
        let input = RayCastInput { origin: vec3(0.3, 0.0, 0.0), translation: vec3(8.0, 0.0, 0.0), max_fraction: 1.0 };
        let out = ray_cast_sphere(&s, &input);
        ensure!(out.hit);
        ensure!(out.fraction == 0.0);
        ensure_small!(distance(out.point, input.origin), f32::EPSILON);
    }
    // Zero length ray inside.
    {
        let input = RayCastInput { origin: vec3(0.5, 0.0, 0.0), translation: vec3(0.0, 0.0, 0.0), max_fraction: 1.0 };
        let out = ray_cast_sphere(&s, &input);
        ensure!(out.hit);
        ensure_small!(distance(out.point, input.origin), f32::EPSILON);
    }
    // Zero length ray outside.
    {
        let input = RayCastInput { origin: vec3(3.0, 0.0, 0.0), translation: vec3(0.0, 0.0, 0.0), max_fraction: 1.0 };
        ensure!(!ray_cast_sphere(&s, &input).hit);
    }
}

#[test]
fn ray_cast_sphere_graze_test() {
    let s = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 1.0 };

    // Just inside the radius grazes a hit, just outside misses.
    {
        let input = RayCastInput { origin: vec3(-4.0, 0.999, 0.0), translation: vec3(8.0, 0.0, 0.0), max_fraction: 1.0 };
        ensure!(ray_cast_sphere(&s, &input).hit);
    }
    {
        let input = RayCastInput { origin: vec3(-4.0, 1.001, 0.0), translation: vec3(8.0, 0.0, 0.0), max_fraction: 1.0 };
        ensure!(!ray_cast_sphere(&s, &input).hit);
    }
}

// Capsule along x from -2 to 2, radius 1. Reused by the capsule ray cast subtests.
fn ray_capsule() -> Capsule {
    Capsule { center1: vec3(-2.0, 0.0, 0.0), center2: vec3(2.0, 0.0, 0.0), radius: 1.0 }
}

#[test]
fn ray_cast_capsule_side_test() {
    let ray_capsule = ray_capsule();

    // Perpendicular hit on the cylindrical side. Surface at distance 2 over a length 6 ray.
    {
        let input = RayCastInput { origin: vec3(0.0, 3.0, 0.0), translation: vec3(0.0, -6.0, 0.0), max_fraction: 1.0 };
        let out = ray_cast_capsule(&ray_capsule, &input);
        check_cast_hit(out, input.origin, input.translation, vec3(0.0, 1.0, 0.0), vec3(0.0, 1.0, 0.0), 1.0 / 3.0, 1e-5);
    }
    // Same from +z to exercise the other transverse direction.
    {
        let input = RayCastInput { origin: vec3(0.0, 0.0, 3.0), translation: vec3(0.0, 0.0, -6.0), max_fraction: 1.0 };
        let out = ray_cast_capsule(&ray_capsule, &input);
        check_cast_hit(out, input.origin, input.translation, vec3(0.0, 0.0, 1.0), vec3(0.0, 0.0, 1.0), 1.0 / 3.0, 1e-5);
    }
    // Side hit nearer the c1 end.
    {
        let input = RayCastInput { origin: vec3(-1.0, 3.0, 0.0), translation: vec3(0.0, -6.0, 0.0), max_fraction: 1.0 };
        let out = ray_cast_capsule(&ray_capsule, &input);
        check_cast_hit(out, input.origin, input.translation, vec3(-1.0, 1.0, 0.0), vec3(0.0, 1.0, 0.0), 1.0 / 3.0, 1e-5);
    }
}

#[test]
fn ray_cast_capsule_oblique_test() {
    let ray_capsule = ray_capsule();

    // Oblique ray in the z=0 plane. It crosses y=1 inside the cylinder span, so the
    // normal stays transverse. Exercises the non perpendicular ray/axis solve where
    // dot(axis, rayAxis) != 0.
    let input = RayCastInput { origin: vec3(-3.0, 3.0, 0.0), translation: vec3(4.0, -4.0, 0.0), max_fraction: 1.0 };
    let out = ray_cast_capsule(&ray_capsule, &input);
    check_cast_hit(out, input.origin, input.translation, vec3(-1.0, 1.0, 0.0), vec3(0.0, 1.0, 0.0), 0.5, 1e-4);
}

#[test]
fn ray_cast_capsule_cap_test() {
    let ray_capsule = ray_capsule();
    let k = 0.70710678;

    // Collinear ray hits the c2 hemisphere from beyond the end.
    {
        let input = RayCastInput { origin: vec3(5.0, 0.0, 0.0), translation: vec3(-8.0, 0.0, 0.0), max_fraction: 1.0 };
        let out = ray_cast_capsule(&ray_capsule, &input);
        check_cast_hit(out, input.origin, input.translation, vec3(3.0, 0.0, 0.0), vec3(1.0, 0.0, 0.0), 1.0 / 4.0, 1e-5);
    }
    // Off-axis ray through the c2 cap center, approaching from outside the cylinder.
    {
        let input = RayCastInput { origin: vec3(4.0, 2.0, 0.0), translation: vec3(-4.0, -4.0, 0.0), max_fraction: 1.0 };
        let out = ray_cast_capsule(&ray_capsule, &input);
        check_cast_hit(out, input.origin, input.translation, vec3(2.0 + k, k, 0.0), vec3(k, k, 0.0), 0.323223, 1e-4);
    }
    // Mirror through the c1 cap center.
    {
        let input = RayCastInput { origin: vec3(-4.0, 2.0, 0.0), translation: vec3(4.0, -4.0, 0.0), max_fraction: 1.0 };
        let out = ray_cast_capsule(&ray_capsule, &input);
        check_cast_hit(out, input.origin, input.translation, vec3(-2.0 - k, k, 0.0), vec3(-k, k, 0.0), 0.323223, 1e-4);
    }
}

#[test]
fn ray_cast_capsule_miss_test() {
    let ray_capsule = ray_capsule();

    // Pointing away.
    {
        let input = RayCastInput { origin: vec3(0.0, 3.0, 0.0), translation: vec3(0.0, 4.0, 0.0), max_fraction: 1.0 };
        ensure!(!ray_cast_capsule(&ray_capsule, &input).hit);
    }
    // Crosses above the axis more than a radius away.
    {
        let input = RayCastInput { origin: vec3(0.0, 4.0, 2.0), translation: vec3(0.0, -8.0, 0.0), max_fraction: 1.0 };
        ensure!(!ray_cast_capsule(&ray_capsule, &input).hit);
    }
    // Aimed at the side but the translation stops short.
    {
        let input = RayCastInput { origin: vec3(0.0, 5.0, 0.0), translation: vec3(0.0, -1.0, 0.0), max_fraction: 1.0 };
        ensure!(!ray_cast_capsule(&ray_capsule, &input).hit);
    }
    // Parallel to the axis and outside the cylinder.
    {
        let input = RayCastInput { origin: vec3(0.0, 3.0, 0.0), translation: vec3(8.0, 0.0, 0.0), max_fraction: 1.0 };
        ensure!(!ray_cast_capsule(&ray_capsule, &input).hit);
    }
    // Descends past the rounded c2 end, beyond cap reach.
    {
        let input = RayCastInput { origin: vec3(4.0, 3.0, 0.0), translation: vec3(0.0, -6.0, 0.0), max_fraction: 1.0 };
        ensure!(!ray_cast_capsule(&ray_capsule, &input).hit);
    }
}

#[test]
fn ray_cast_capsule_interior_test() {
    let ray_capsule = ray_capsule();

    // Origin on the axis between the caps.
    {
        let input = RayCastInput { origin: vec3(0.0, 0.0, 0.0), translation: vec3(0.0, -5.0, 0.0), max_fraction: 1.0 };
        let out = ray_cast_capsule(&ray_capsule, &input);
        ensure!(out.hit);
        ensure!(out.fraction == 0.0);
        ensure_small!(distance(out.point, input.origin), f32::EPSILON);
    }
    // Origin inside the c2 hemisphere, past the cylinder end.
    {
        let input = RayCastInput { origin: vec3(2.5, 0.0, 0.0), translation: vec3(0.0, 0.0, 5.0), max_fraction: 1.0 };
        let out = ray_cast_capsule(&ray_capsule, &input);
        ensure!(out.hit);
        ensure!(out.fraction == 0.0);
        ensure_small!(distance(out.point, input.origin), f32::EPSILON);
    }
    // Zero length ray inside.
    {
        let input = RayCastInput { origin: vec3(0.0, 0.0, 0.0), translation: vec3(0.0, 0.0, 0.0), max_fraction: 1.0 };
        let out = ray_cast_capsule(&ray_capsule, &input);
        ensure!(out.hit);
        ensure_small!(distance(out.point, input.origin), f32::EPSILON);
    }
    // Zero length ray outside.
    {
        let input = RayCastInput { origin: vec3(0.0, 3.0, 0.0), translation: vec3(0.0, 0.0, 0.0), max_fraction: 1.0 };
        ensure!(!ray_cast_capsule(&ray_capsule, &input).hit);
    }
}

#[test]
fn ray_cast_capsule_degenerate_test() {
    // Coincident centers collapse to a sphere.
    let c = Capsule { center1: vec3(0.0, 0.0, 0.0), center2: vec3(0.0, 0.0, 0.0), radius: 1.0 };
    let input = RayCastInput { origin: vec3(-4.0, 0.0, 0.0), translation: vec3(8.0, 0.0, 0.0), max_fraction: 1.0 };
    let out = ray_cast_capsule(&c, &input);
    check_cast_hit(out, input.origin, input.translation, vec3(-1.0, 0.0, 0.0), vec3(-1.0, 0.0, 0.0), 3.0 / 8.0, 1e-5);
}

#[test]
fn ray_cast_capsule_clip_test() {
    let ray_capsule = ray_capsule();

    // The side hit occurs at fraction 1/3. Straddle it with max_fraction.
    {
        let input = RayCastInput { origin: vec3(0.0, 3.0, 0.0), translation: vec3(0.0, -6.0, 0.0), max_fraction: 0.3 };
        ensure!(!ray_cast_capsule(&ray_capsule, &input).hit);
    }
    {
        let input = RayCastInput { origin: vec3(0.0, 3.0, 0.0), translation: vec3(0.0, -6.0, 0.0), max_fraction: 0.5 };
        let out = ray_cast_capsule(&ray_capsule, &input);
        ensure!(out.hit);
        ensure_small!(out.fraction - 1.0 / 3.0, 1e-5);
    }
}

// A ray within a hair of the capsule axis must still hit when it slowly converges onto the
// surface. The closest point solver is ill conditioned in this band, so this guards the near
// parallel fallback that intersects the infinite cylinder directly.
#[test]
fn ray_cast_capsule_parallel_test() {
    let ray_capsule = ray_capsule();

    // Capsule along y. A long ray almost parallel to the axis drifts inward from just outside the
    // cylinder and dips through the far endcap. The naive solve loses this hit to a determinant of zero.
    let axis_y = Capsule { center1: vec3(0.0, 0.0, 0.0), center2: vec3(0.0, 10.0, 0.0), radius: 1.0 };
    {
        let input = RayCastInput { origin: vec3(1.0001, 100.0, 0.0), translation: vec3(-0.001, -200.0, 0.0), max_fraction: 1.0 };
        let out = ray_cast_capsule(&axis_y, &input);
        ensure!(out.hit);

        // The hit lands on the capsule surface and on the ray.
        let on_seg = point_to_segment_distance(axis_y.center1, axis_y.center2, out.point);
        ensure_small!(distance(out.point, on_seg) - axis_y.radius, 1e-3);
        let on_ray = mul_add(input.origin, out.fraction, input.translation);
        ensure_small!(distance(out.point, on_ray), 1e-3);
    }

    // Near parallel ray converging onto the x-axis capsule from far away.
    {
        let input = RayCastInput { origin: vec3(-1000.0, 1.0001, 0.0), translation: vec3(2000.0, -0.001, 0.0), max_fraction: 1.0 };
        let out = ray_cast_capsule(&ray_capsule, &input);
        ensure!(out.hit);
        let on_seg = point_to_segment_distance(ray_capsule.center1, ray_capsule.center2, out.point);
        ensure_small!(distance(out.point, on_seg) - ray_capsule.radius, 1e-3);
    }

    // Exactly parallel and outside the cylinder still misses.
    {
        let input = RayCastInput { origin: vec3(0.0, 3.0, 0.0), translation: vec3(8.0, 0.0, 0.0), max_fraction: 1.0 };
        ensure!(!ray_cast_capsule(&ray_capsule, &input).hit);
    }
}

// Zero length rays and initial overlap behave the same across the solid shapes. A moving ray
// and a zero length ray that both start inside report the origin with zero fraction, and a zero
// length ray that starts outside misses.
#[test]
fn ray_cast_overlap_convention_test() {
    let ray_capsule = ray_capsule();
    let box_hull = test_box();

    let zero = vec3(0.0, 0.0, 0.0);
    let ray = vec3(8.0, 0.0, 0.0);

    // Sphere
    {
        let s = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 1.0 };
        let inside = vec3(0.2, 0.0, 0.0);
        let outside = vec3(3.0, 0.0, 0.0);
        let moving = RayCastInput { origin: inside, translation: ray, max_fraction: 1.0 };
        let point_inside = RayCastInput { origin: inside, translation: zero, max_fraction: 1.0 };
        let point_outside = RayCastInput { origin: outside, translation: zero, max_fraction: 1.0 };
        check_initial_overlap(ray_cast_sphere(&s, &moving), inside);
        check_initial_overlap(ray_cast_sphere(&s, &point_inside), inside);
        ensure!(!ray_cast_sphere(&s, &point_outside).hit);
    }

    // Capsule
    {
        let inside = vec3(0.0, 0.0, 0.0);
        let outside = vec3(0.0, 3.0, 0.0);
        let moving = RayCastInput { origin: inside, translation: ray, max_fraction: 1.0 };
        let point_inside = RayCastInput { origin: inside, translation: zero, max_fraction: 1.0 };
        let point_outside = RayCastInput { origin: outside, translation: zero, max_fraction: 1.0 };
        check_initial_overlap(ray_cast_capsule(&ray_capsule, &moving), inside);
        check_initial_overlap(ray_cast_capsule(&ray_capsule, &point_inside), inside);
        ensure!(!ray_cast_capsule(&ray_capsule, &point_outside).hit);
    }

    // Hull
    {
        let inside = vec3(0.3, 0.2, 0.1);
        let outside = vec3(3.0, 0.0, 0.0);
        let moving = RayCastInput { origin: inside, translation: ray, max_fraction: 1.0 };
        let point_inside = RayCastInput { origin: inside, translation: zero, max_fraction: 1.0 };
        let point_outside = RayCastInput { origin: outside, translation: zero, max_fraction: 1.0 };
        check_initial_overlap(ray_cast_hull(&box_hull, &moving), inside);
        check_initial_overlap(ray_cast_hull(&box_hull, &point_inside), inside);
        ensure!(!ray_cast_hull(&box_hull, &point_outside).hit);
    }
}

// Distance, in double precision, from a single precision hit point to the analytic first
// ray/sphere intersection of the same float ray. Isolates the single precision method error:
// the reference carries no float rounding, so what remains is purely what the method lost.
fn sphere_hit_error(shape: Sphere, input: &RayCastInput, point: Vec3) -> f64 {
    let r = shape.radius as f64;
    let sx = input.origin.x as f64 - shape.center.x as f64;
    let sy = input.origin.y as f64 - shape.center.y as f64;
    let sz = input.origin.z as f64 - shape.center.z as f64;
    let (tx, ty, tz) = (input.translation.x as f64, input.translation.y as f64, input.translation.z as f64);
    let len = (tx * tx + ty * ty + tz * tz).sqrt();
    let (dx, dy, dz) = (tx / len, ty / len, tz / len);
    let b = sx * dx + sy * dy + sz * dz;
    let c = sx * sx + sy * sy + sz * sz - r * r;
    let t = -b - (b * b - c).sqrt();
    let ex = point.x as f64 - (input.origin.x as f64 + t * dx);
    let ey = point.y as f64 - (input.origin.y as f64 + t * dy);
    let ez = point.z as f64 - (input.origin.z as f64 + t * dz);
    (ex * ex + ey * ey + ez * ez).sqrt()
}

// Same idea for the ray/infinite-cylinder intersection, used where the hit lands on the side.
fn capsule_hit_error(shape: Capsule, input: &RayCastInput, point: Vec3) -> f64 {
    let mut ax = shape.center2.x as f64 - shape.center1.x as f64;
    let mut ay = shape.center2.y as f64 - shape.center1.y as f64;
    let mut az = shape.center2.z as f64 - shape.center1.z as f64;
    let alen = (ax * ax + ay * ay + az * az).sqrt();
    ax /= alen;
    ay /= alen;
    az /= alen;

    let sx = input.origin.x as f64 - shape.center1.x as f64;
    let sy = input.origin.y as f64 - shape.center1.y as f64;
    let sz = input.origin.z as f64 - shape.center1.z as f64;
    let (tx, ty, tz) = (input.translation.x as f64, input.translation.y as f64, input.translation.z as f64);
    let tlen = (tx * tx + ty * ty + tz * tz).sqrt();
    let (dx, dy, dz) = (tx / tlen, ty / tlen, tz / tlen);

    let sa = sx * ax + sy * ay + sz * az;
    let da = dx * ax + dy * ay + dz * az;
    let (spx, spy, spz) = (sx - sa * ax, sy - sa * ay, sz - sa * az);
    let (dpx, dpy, dpz) = (dx - da * ax, dy - da * ay, dz - da * az);
    let a = dpx * dpx + dpy * dpy + dpz * dpz;
    let b = 2.0 * (spx * dpx + spy * dpy + spz * dpz);
    let r = shape.radius as f64;
    let c = spx * spx + spy * spy + spz * spz - r * r;
    let tau = (-b - (b * b - 4.0 * a * c).sqrt()) / (2.0 * a);
    let ex = point.x as f64 - (input.origin.x as f64 + tau * dx);
    let ey = point.y as f64 - (input.origin.y as f64 + tau * dy);
    let ez = point.z as f64 - (input.origin.z as f64 + tau * dz);
    (ex * ex + ey * ey + ez * ez).sqrt()
}

// A miss is the worst possible outcome, so fold it into the error as a large sentinel.
const RAY_MISS: f64 = 1.0e30;

#[test]
fn ray_cast_far_origin_test() {
    let s = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 1.0 };
    let c = Capsule { center1: vec3(-2.0, 0.0, 0.0), center2: vec3(2.0, 0.0, 0.0), radius: 1.0 };

    // (0,0,1) lies on the unit sphere and on the capsule side. The ray dives in from a far origin
    // along H + D*u for a fan of directions u skewed off the surface normal, so the capsule solve
    // sees a real perpendicular gap rather than a free exact cancellation.
    let h = vec3(0.0, 0.0, 1.0);
    let offsets = [-0.7f32, 0.0, 0.7];
    let distances = [1e1f32, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7];

    let mut worst_sphere = [0.0f64; 7];
    let mut worst_capsule = [0.0f64; 7];

    println!("    worst hit point error over a fan of skew rays, by origin distance:");
    println!("    {:<9} {:<13} {:<13}", "distance", "sphere", "capsule");
    for i in 0..distances.len() {
        let d = distances[i];
        let mut max_s = 0.0f64;
        let mut max_c = 0.0f64;

        for ia in 0..offsets.len() {
            for ib in 0..offsets.len() {
                let u = normalize(vec3(offsets[ia], offsets[ib], 1.0));
                let origin = mul_add(h, d, u);
                let translation = mul_sv(-2.0 * d, u);
                let input = RayCastInput { origin, translation, max_fraction: 1.0 };

                let os = ray_cast_sphere(&s, &input);
                let oc = ray_cast_capsule(&c, &input);

                let err_s = if os.hit { sphere_hit_error(s, &input, os.point) } else { RAY_MISS };
                let err_c = if oc.hit { capsule_hit_error(c, &input, oc.point) } else { RAY_MISS };

                max_s = if err_s > max_s { err_s } else { max_s };
                max_c = if err_c > max_c { err_c } else { max_c };
            }
        }

        worst_sphere[i] = max_s;
        worst_capsule[i] = max_c;
        println!("    {:<9.0e} {:<13.3e} {:<13.3e}", d, max_s, max_c);
    }

    // The closest point formulation keeps the error at the single precision floor: it grows only
    // linearly with origin distance, error ~ distance * FLT_EPSILON, with no catastrophic loss. This
    // holds out to a million units. At ten million the origin coordinates carry a meter sized ULP,
    // larger than the unit radius, so the ray genuinely drops the hit. That row is printed for insight
    // but left unasserted rather than baking in the breakdown.
    for i in 0..distances.len() {
        if distances[i] < 1.0e7 {
            let floor = 16.0 * distances[i] as f64 * f32::EPSILON as f64 + 2.0e-6;
            ensure!(worst_sphere[i] < floor);
            ensure!(worst_capsule[i] < floor);
        }
    }

    // Still a clean sub-meter hit at a million units out.
    ensure!(worst_sphere[5] < 0.5 && worst_capsule[5] < 0.5);
}

#[test]
fn ray_cast_shape_test() {
    let sphere = test_sphere();
    let capsule = test_capsule();
    let box_hull = test_box();

    let input = RayCastInput {
        origin: vec3(-4.0, 0.0, 0.0),
        translation: vec3(8.0, 0.0, 0.0),
        max_fraction: 1.0,
    };

    {
        let output = ray_cast_sphere(&sphere, &input);
        ensure!(output.hit);
        ensure_small!(output.normal.x + 1.0, f32::EPSILON);
        ensure_small!(output.normal.y, f32::EPSILON);
        ensure_small!(output.normal.z, f32::EPSILON);
        ensure_small!(output.fraction - 0.5, f32::EPSILON);
    }

    {
        let output = ray_cast_capsule(&capsule, &input);
        ensure!(output.hit);
        ensure_small!(output.normal.x + 1.0, f32::EPSILON);
        ensure_small!(output.normal.y, f32::EPSILON);
        ensure_small!(output.normal.z, f32::EPSILON);
        ensure_small!(output.fraction - 1.0 / 4.0, f32::EPSILON);
    }

    {
        let output = ray_cast_hull(&box_hull, &input);
        ensure!(output.hit);
        ensure_small!(output.normal.x + 1.0, f32::EPSILON);
        ensure_small!(output.normal.y, f32::EPSILON);
        ensure_small!(output.normal.z, f32::EPSILON);
        ensure_small!(output.fraction - 3.0 / 8.0, f32::EPSILON);
    }
}
