// Port of box3d/test/test_body.c
//
// b3UpdateBodyMassData shifts each shape's inertia to the body center of mass
// with the parallel axis theorem. When shapes sit far from the body origin the
// shift term dwarfs the central inertia, so any error in the per shape framing
// blows up the tensor. Spheres make a clean oracle: the central inertia is
// isotropic and independent of placement, so the shift is the only thing tested.

use makepad_box3d::body::{
    body_apply_mass_from_shapes, body_apply_torque, body_get_angular_velocity, body_get_mass_data,
    body_get_world_inverse_rotational_inertia, body_set_mass_data, body_set_transform, create_body,
};
use makepad_box3d::ensure_small;
use makepad_box3d::math_functions::{make_quat_from_axis_angle, pos, vec3, Matrix3, Vec3, PI};
use makepad_box3d::physics_world::{create_world, destroy_world, world_step};
use makepad_box3d::shape::create_sphere_shape;
use makepad_box3d::types::*;

fn sphere_body_mass(centers: &[Vec3], radius: f32, density: f32) -> MassData {
    let world_def = default_world_def();
    let mut world = create_world(&world_def);

    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    let body_id = create_body(&mut world, &body_def);

    let mut shape_def = default_shape_def();
    shape_def.density = density;

    for center in centers {
        let sphere = Sphere { center: *center, radius };
        create_sphere_shape(&mut world, body_id, &shape_def, &sphere);
    }

    body_apply_mass_from_shapes(&mut world, body_id);
    let mass_data = body_get_mass_data(&world, body_id);

    destroy_world(world);
    mass_data
}

// One sphere far from the body origin. The center of mass lands on the sphere
// and the inertia about it must be the bare central inertia, with no trace of
// the offset.
#[test]
fn far_single_sphere_mass() {
    let radius = 0.5f32;
    let density = 1.0f32;
    let center = vec3(100.0, -50.0, 75.0);
    let md = sphere_body_mass(&[center], radius, density);

    let mass = density * (4.0 / 3.0) * PI * radius * radius * radius;
    let central = 0.4 * mass * radius * radius;

    ensure_small!(md.mass - mass, 1e-4);

    ensure_small!(md.center.x - center.x, 1e-3);
    ensure_small!(md.center.y - center.y, 1e-3);
    ensure_small!(md.center.z - center.z, 1e-3);

    ensure_small!(md.inertia.cx.x - central, 1e-3);
    ensure_small!(md.inertia.cy.y - central, 1e-3);
    ensure_small!(md.inertia.cz.z - central, 1e-3);

    ensure_small!(md.inertia.cy.x, 1e-3);
    ensure_small!(md.inertia.cz.x, 1e-3);
    ensure_small!(md.inertia.cz.y, 1e-3);
}

// Eight equal spheres on the corners of a cube, the whole cube parked far from
// the body origin. The center of mass is the cube center and the products of
// inertia cancel by symmetry, so the tensor stays diagonal no matter how far
// out the cube sits.
#[test]
fn far_cube_sphere_mass() {
    let radius = 0.5f32;
    let density = 1.0f32;
    let h = 1.0f32;
    let p = vec3(100.0, 100.0, 100.0);

    let mut centers = Vec::new();
    for sx in [-1.0f32, 1.0] {
        for sy in [-1.0f32, 1.0] {
            for sz in [-1.0f32, 1.0] {
                centers.push(vec3(p.x + sx * h, p.y + sy * h, p.z + sz * h));
            }
        }
    }

    let md = sphere_body_mass(&centers, radius, density);

    let mass = density * (4.0 / 3.0) * PI * radius * radius * radius;
    let total_mass = 8.0 * mass;

    // Per sphere central inertia summed, plus the parallel axis term for each
    // corner offset (dy^2 + dz^2) = (h^2 + h^2) about every axis.
    let diag = 8.0 * 0.4 * mass * radius * radius + 16.0 * mass * h * h;

    ensure_small!(md.mass - total_mass, 1e-3);

    ensure_small!(md.center.x - p.x, 1e-2);
    ensure_small!(md.center.y - p.y, 1e-2);
    ensure_small!(md.center.z - p.z, 1e-2);

    ensure_small!(md.inertia.cx.x - diag, 1e-2);
    ensure_small!(md.inertia.cy.y - diag, 1e-2);
    ensure_small!(md.inertia.cz.z - diag, 1e-2);

    ensure_small!(md.inertia.cy.x, 1e-2);
    ensure_small!(md.inertia.cz.x, 1e-2);
    ensure_small!(md.inertia.cz.y, 1e-2);
}

// body_set_mass_data must refresh the cached world space inverse inertia.
// The solver consumes the cached tensor at the start of the next step (torque
// integration, contact and joint prepare), and only recomputes it at the end
// of the step or in body_set_transform. Before the fix, calling
// body_set_mass_data on a body whose transform was already set left the tensor
// derived from the old (shape) mass data, so the first step ran with a wildly
// wrong rotational response.
//
// Oracle: the same body with the same final mass data and transform, but with
// the calls in the other order (mass data first, then transform — the path
// that always recomputed the cache). Both orders must yield the identical
// cached tensor and the identical angular velocity after one step.
fn spin_with_mass_data(transform_first: bool) -> (Matrix3, Vec3) {
    let world_def = default_world_def();
    let mut world = create_world(&world_def);

    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    let body_id = create_body(&mut world, &body_def);

    // Shape mass data is the stale value the bug leaves behind: a small sphere,
    // orders of magnitude lighter than the mass data set below.
    let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.5 };
    create_sphere_shape(&mut world, body_id, &default_shape_def(), &sphere);

    let position = pos(2.0, 5.0, -1.0);
    let rotation = make_quat_from_axis_angle(vec3(1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0), 0.7);
    let mass_data = MassData {
        mass: 500.0,
        center: vec3(0.0, 0.0, 0.0),
        inertia: Matrix3 {
            cx: vec3(40.0, 0.0, 0.0),
            cy: vec3(0.0, 90.0, 0.0),
            cz: vec3(0.0, 0.0, 160.0),
        },
    };

    if transform_first {
        body_set_transform(&mut world, body_id, position, rotation);
        body_set_mass_data(&mut world, body_id, mass_data);
    } else {
        body_set_mass_data(&mut world, body_id, mass_data);
        body_set_transform(&mut world, body_id, position, rotation);
    }

    let inv_inertia_world = body_get_world_inverse_rotational_inertia(&world, body_id);

    body_apply_torque(&mut world, body_id, vec3(1200.0, -450.0, 800.0), true);
    world_step(&mut world, 1.0 / 60.0, 4);
    let angular_velocity = body_get_angular_velocity(&world, body_id);

    destroy_world(world);
    (inv_inertia_world, angular_velocity)
}

#[test]
fn set_mass_data_refreshes_world_inverse_inertia() {
    let (stale_candidate, spin_a) = spin_with_mass_data(true);
    let (reference, spin_b) = spin_with_mass_data(false);

    // Cached world space inverse inertia must not depend on call order.
    ensure_small!(stale_candidate.cx.x - reference.cx.x, 1e-6);
    ensure_small!(stale_candidate.cx.y - reference.cx.y, 1e-6);
    ensure_small!(stale_candidate.cx.z - reference.cx.z, 1e-6);
    ensure_small!(stale_candidate.cy.x - reference.cy.x, 1e-6);
    ensure_small!(stale_candidate.cy.y - reference.cy.y, 1e-6);
    ensure_small!(stale_candidate.cy.z - reference.cy.z, 1e-6);
    ensure_small!(stale_candidate.cz.x - reference.cz.x, 1e-6);
    ensure_small!(stale_candidate.cz.y - reference.cz.y, 1e-6);
    ensure_small!(stale_candidate.cz.z - reference.cz.z, 1e-6);

    // Sanity: the torque actually spun the body.
    let speed = (spin_b.x * spin_b.x + spin_b.y * spin_b.y + spin_b.z * spin_b.z).sqrt();
    assert!(speed > 0.01, "torque produced no spin: {:?}", spin_b);

    // One step of torque must produce the same angular velocity either way.
    ensure_small!(spin_a.x - spin_b.x, 1e-6);
    ensure_small!(spin_a.y - spin_b.y, 1e-6);
    ensure_small!(spin_a.z - spin_b.z, 1e-6);
}
