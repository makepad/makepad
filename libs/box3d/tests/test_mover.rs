// Port of box3d/test/test_mover.c

use makepad_box3d::capsule::collide_mover_and_capsule;
use makepad_box3d::hull::{collide_mover_and_hull, make_box_hull};
use makepad_box3d::math_functions::*;
use makepad_box3d::mover::solve_planes;
use makepad_box3d::sphere::collide_mover_and_sphere;
use makepad_box3d::types::*;
use makepad_box3d::{ensure, ensure_small};

#[test]
fn parallel_planes() {
    let mut planes = [CollisionPlane::default(); 3];
    planes[0].plane.normal = vec3(0.0, 0.0, 1.0);
    planes[0].plane.offset = 0.5;
    planes[0].push_limit = f32::MAX;
    planes[1].plane.normal = vec3(0.0, 0.0, 1.0);
    planes[1].plane.offset = 1.0;
    planes[1].push_limit = f32::MAX;

    let target = vec3(0.0, 0.0, 0.0);
    let result = solve_planes(target, &mut planes[..2]);

    ensure!(result.iteration_count == 2);
    ensure_small!(result.delta.z - 1.0, 0.0055);
}

#[test]
fn game_planes() {
    // This scenario takes many iterations because the target is deep into the plane.
    let mut planes = [CollisionPlane::default(); 3];
    planes[0].plane.normal = vec3(0.0, -0.23941046, 0.970918416);
    planes[0].plane.offset = 0.390724182;
    planes[0].push_limit = f32::MAX;
    planes[1].plane.normal = vec3(0.0, 0.0, 1.0);
    planes[1].plane.offset = 1.49998093;
    planes[1].push_limit = f32::MAX;

    let mut target = vec3(-2.5390625, 0.0, -73.6880798);

    planes[0].plane.offset -= dot(planes[0].plane.normal, target);
    planes[1].plane.offset -= dot(planes[1].plane.normal, target);
    target = Vec3::ZERO;

    let result = solve_planes(target, &mut planes[..2]);

    ensure!(result.iteration_count == 20);
}

// ---------------------------------------------------------------------------
// Mover-collide overlap handling
//
// collide_mover_and_sphere / capsule / hull must never emit a plane with a
// degenerate (zero) normal, even when the mover deeply penetrates the shape.
// On deep overlap the GJK path returns a {0,0,0} normal; these tests guard the
// fix that replaces it with an analytic (sphere/capsule) or dropped (hull) result.
// ---------------------------------------------------------------------------

#[test]
fn mover_sphere_separated() {
    let shape = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.5 };
    let mover = Capsule { center1: vec3(4.0, 3.0, 0.0), center2: vec3(6.0, 3.0, 0.0), radius: 0.2 };

    let mut result = PlaneResult::default();
    let count = collide_mover_and_sphere(&mut result, &shape, &mover);
    ensure!(count == 0);
}

#[test]
fn mover_sphere_touching() {
    let shape = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.5 };

    // Mover core segment runs along X at y = 0.6, leaving it 0.1 inside the
    // 0.7 combined radius.
    let mover = Capsule { center1: vec3(-1.0, 0.6, 0.0), center2: vec3(1.0, 0.6, 0.0), radius: 0.2 };

    let mut result = PlaneResult::default();
    let count = collide_mover_and_sphere(&mut result, &shape, &mover);
    ensure!(count == 1);
    ensure!(is_normalized(result.plane.normal));

    // Push-out points from the sphere straight up toward the mover.
    ensure!(result.plane.normal.y > 0.99);
    ensure_small!(result.plane.offset - 0.1, 1e-5);
}

#[test]
fn mover_sphere_deep_overlap() {
    let shape = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.5 };

    // Mover axis runs straight through the sphere center: the bug case where
    // GJK reports a zero normal.
    let mover = Capsule { center1: vec3(-1.0, 0.0, 0.0), center2: vec3(1.0, 0.0, 0.0), radius: 0.2 };

    let mut result = PlaneResult::default();
    let count = collide_mover_and_sphere(&mut result, &shape, &mover);
    ensure!(count == 1);

    // The normal must still be a valid unit vector.
    ensure!(is_normalized(result.plane.normal));

    // The fallback axis is perpendicular to the mover axis (X).
    ensure_small!(result.plane.normal.x, 1e-5);

    // Deepest possible penetration: the full combined radius.
    ensure_small!(result.plane.offset - 0.7, 1e-5);
}

#[test]
fn mover_capsule_separated() {
    let shape = Capsule { center1: vec3(-1.0, 0.0, 0.0), center2: vec3(1.0, 0.0, 0.0), radius: 0.3 };
    let mover = Capsule { center1: vec3(-1.0, 5.0, 0.0), center2: vec3(1.0, 5.0, 0.0), radius: 0.2 };

    let mut result = PlaneResult::default();
    let count = collide_mover_and_capsule(&mut result, &shape, &mover);
    ensure!(count == 0);
}

#[test]
fn mover_capsule_touching() {
    let shape = Capsule { center1: vec3(-1.0, 0.0, 0.0), center2: vec3(1.0, 0.0, 0.0), radius: 0.3 };

    // Parallel mover 0.4 above, leaving it 0.1 inside the 0.5 combined radius.
    let mover = Capsule { center1: vec3(-1.0, 0.4, 0.0), center2: vec3(1.0, 0.4, 0.0), radius: 0.2 };

    let mut result = PlaneResult::default();
    let count = collide_mover_and_capsule(&mut result, &shape, &mover);
    ensure!(count == 1);
    ensure!(is_normalized(result.plane.normal));
    ensure!(result.plane.normal.y > 0.99);
    ensure_small!(result.plane.offset - 0.1, 1e-5);
}

#[test]
fn mover_capsule_deep_overlap() {
    // Shape capsule along X, mover capsule along Z; their core segments cross
    // exactly at the origin, so GJK reports a zero normal.
    let shape = Capsule { center1: vec3(-1.0, 0.0, 0.0), center2: vec3(1.0, 0.0, 0.0), radius: 0.3 };
    let mover = Capsule { center1: vec3(0.0, 0.0, -1.0), center2: vec3(0.0, 0.0, 1.0), radius: 0.2 };

    let mut result = PlaneResult::default();
    let count = collide_mover_and_capsule(&mut result, &shape, &mover);
    ensure!(count == 1);
    ensure!(is_normalized(result.plane.normal));

    // The separating axis of two crossing segments is perpendicular to both.
    ensure_small!(result.plane.normal.x, 1e-5);
    ensure_small!(result.plane.normal.z, 1e-5);
    ensure_small!(result.plane.offset - 0.5, 1e-5);
}

#[test]
fn mover_capsule_parallel_overlap() {
    // Mover core segment coincides with the shape core segment: the cross-product
    // axis degenerates, so a perpendicular of the mover axis is used instead.
    let shape = Capsule { center1: vec3(-1.0, 0.0, 0.0), center2: vec3(1.0, 0.0, 0.0), radius: 0.3 };
    let mover = Capsule { center1: vec3(-1.0, 0.0, 0.0), center2: vec3(1.0, 0.0, 0.0), radius: 0.2 };

    let mut result = PlaneResult::default();
    let count = collide_mover_and_capsule(&mut result, &shape, &mover);
    ensure!(count == 1);
    ensure!(is_normalized(result.plane.normal));

    // The fallback axis is perpendicular to the mover axis (X).
    ensure_small!(result.plane.normal.x, 1e-5);
    ensure_small!(result.plane.offset - 0.5, 1e-5);
}

#[test]
fn mover_hull_separated() {
    let box_hull = make_box_hull(0.5, 0.5, 0.5);
    let mover = Capsule { center1: vec3(-0.3, 5.0, 0.0), center2: vec3(0.3, 5.0, 0.0), radius: 0.2 };

    let mut result = PlaneResult::default();
    let count = collide_mover_and_hull(&mut result, &box_hull, &mover);
    ensure!(count == 0);
}

#[test]
fn mover_hull_touching() {
    let box_hull = make_box_hull(0.5, 0.5, 0.5);

    // Mover core segment above the +Y face; the 0.2 radius reaches 0.1 into it.
    let mover = Capsule { center1: vec3(-0.3, 0.6, 0.0), center2: vec3(0.3, 0.6, 0.0), radius: 0.2 };

    let mut result = PlaneResult::default();
    let count = collide_mover_and_hull(&mut result, &box_hull, &mover);
    ensure!(count == 1);
    ensure!(is_normalized(result.plane.normal));
    ensure!(result.plane.normal.y > 0.99);
    ensure_small!(result.plane.offset - 0.1, 1e-4);
}

#[test]
fn mover_hull_deep_overlap() {
    let box_hull = make_box_hull(0.5, 0.5, 0.5);

    // Mover core segment lies entirely inside the box, so GJK reports overlap.
    let mover = Capsule { center1: vec3(-0.2, 0.0, 0.0), center2: vec3(0.2, 0.0, 0.0), radius: 0.1 };

    let mut result = PlaneResult::default();
    let count = collide_mover_and_hull(&mut result, &box_hull, &mover);

    // The overlap guard drops the plane rather than emit a zero normal.
    // todo replace with SAT once collide_mover_and_hull resolves overlaps.
    ensure!(count == 0);
}
