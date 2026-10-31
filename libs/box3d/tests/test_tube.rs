use makepad_box3d::body::*;
use makepad_box3d::capsule::compute_capsule_mass;
use makepad_box3d::contact::{SIM_MESH_CONTACT, SIM_TUBE_CONTACT};
use makepad_box3d::hull::make_box_hull;
use makepad_box3d::math_functions::*;
use makepad_box3d::physics_world::*;
use makepad_box3d::shape::*;
use makepad_box3d::types::*;

fn square_xy(half_size: f32, radius: f32) -> Tube {
    Tube {
        points: vec![
            vec3(-half_size, -half_size, 0.0),
            vec3(half_size, -half_size, 0.0),
            vec3(half_size, half_size, 0.0),
            vec3(-half_size, half_size, 0.0),
        ],
        radius,
    }
}

fn square_xz(half_size: f32, radius: f32) -> Tube {
    let mut tube = square_xy(half_size, radius);
    for point in &mut tube.points {
        *point = vec3(point.x, 0.0, point.y);
    }
    tube
}

#[test]
fn interlocked_tubes_hang() {
    let mut def = default_world_def();
    def.gravity = vec3(0.0, 0.0, -10.0);
    let mut world = create_world(&def);
    let static_body = create_body(&mut world, &default_body_def());
    let static_tube = square_xy(1.0, 0.12);
    let shape_def = default_shape_def();
    create_tube_shape(&mut world, static_body, &shape_def, &static_tube);

    // The xz ring crosses the xy plane once inside the static loop and once outside.
    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    body_def.position = pos(0.75, 0.0, 0.0);
    body_def.enable_sleep = false;
    let body = create_body(&mut world, &body_def);
    let shape = create_tube_shape(&mut world, body, &shape_def, &square_xz(0.5, 0.12));

    for _ in 0..120 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    let center = sub_pos(body_get_position(&world, body), POS_ZERO);
    let ring_radius = length(static_tube.points[0]) + static_tube.radius;
    assert!(center.z < -0.1, "the dynamic ring must fall before hanging: {center:?}");
    assert!(length(center) < ring_radius, "the ring escaped: {center:?}");
    assert_eq!(shape_get_contact_capacity(&world, shape), 1);
    let contacts = shape_get_contact_data(&world, shape, 1);
    assert_eq!(contacts.len(), 1);
    assert!(!contacts[0].manifolds.is_empty());
    assert!(contacts[0].manifolds.len() <= 16);
    let contact = &world.contacts[(contacts[0].contact_id.index1 - 1) as usize];
    assert_ne!(contact.flags & SIM_TUBE_CONTACT, 0);
    assert_eq!(contact.flags & SIM_MESH_CONTACT, 0);
    assert!(contact.mesh_contact.triangle_cache.is_empty());
    let color = &world.constraint_graph.colors[contact.color_index as usize];
    assert_eq!(color.contacts[contact.local_index as usize].contact_id, contact.contact_id);
    destroy_world(world);
}

#[test]
fn tube_rests_on_hull() {
    let mut def = default_world_def();
    def.gravity = vec3(0.0, -10.0, 0.0);
    let mut world = create_world(&def);
    let mut ground_def = default_body_def();
    ground_def.position = pos(0.0, -0.5, 0.0);
    let ground = create_body(&mut world, &ground_def);
    let shape_def = default_shape_def();
    create_hull_shape(&mut world, ground, &shape_def, &make_box_hull(5.0, 0.5, 5.0));

    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    body_def.position = pos(0.0, 3.0, 0.0);
    body_def.linear_velocity = vec3(0.0, -20.0, 0.0);
    let body = create_body(&mut world, &body_def);
    let tube = square_xz(0.75, 0.12);
    let shape = create_tube_shape(&mut world, body, &shape_def, &tube);
    for _ in 0..120 {
        world_step(&mut world, 1.0 / 60.0, 4);
        assert!(body_get_position(&world, body).y > 0.0, "tube tunneled through the box");
    }
    let center = sub_pos(body_get_position(&world, body), POS_ZERO);
    assert!((center.y - tube.radius).abs() < 0.04, "tube did not rest on the box: {center:?}");
    assert!(length(body_get_linear_velocity(&world, body)) < 0.1);
    let contacts = shape_get_contact_data(&world, shape, 1);
    assert_eq!(contacts.len(), 1);
    assert!(contacts[0].manifolds.len() > 1);
    destroy_world(world);
}

#[test]
fn ray_hits_tube_at_expected_distance() {
    let mut world = create_world(&default_world_def());
    let mut body_def = default_body_def();
    body_def.position = pos(2.0, 3.0, 4.0);
    let body = create_body(&mut world, &body_def);
    let shape = create_tube_shape(&mut world, body, &default_shape_def(), &square_xy(1.0, 0.2));
    assert_eq!(shape_get_type(&world, shape), ShapeType::Tube);

    // The closing segment is the near side; the far side must not replace its hit.
    let hit = shape_ray_cast(&world, shape, pos(-1.0, 3.0, 4.0), vec3(6.0, 0.0, 0.0));
    assert!(hit.hit);
    assert!((6.0 * hit.fraction - 1.8).abs() < 1.0e-5);
    assert!(length(sub_pos(hit.point, pos(0.8, 3.0, 4.0))) < 1.0e-5);
    assert!(hit.normal.x < -0.99);

    let hole = shape_ray_cast(&world, shape, pos(2.0, 3.0, 6.0), vec3(0.0, 0.0, -4.0));
    assert!(!hole.hit, "the interior of the loop must stay empty");
    destroy_world(world);
}

#[test]
fn tube_mass_sums_segment_capsules() {
    let mut world = create_world(&default_world_def());
    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    let body = create_body(&mut world, &body_def);
    let mut tube = square_xy(1.0, 0.2);
    let offset = vec3(0.4, -0.7, 0.3);
    for point in &mut tube.points {
        *point = add(*point, offset);
    }
    let mut shape_def = default_shape_def();
    shape_def.density = 2.5;
    let shape_id = create_tube_shape(&mut world, body, &shape_def, &tube);
    let shape = &world.shapes[(shape_id.index1 - 1) as usize];
    let mass = compute_shape_mass(shape);
    let mut expected_mass = 0.0;
    let mut expected_inertia = Matrix3::ZERO;
    for i in 0..tube.points.len() {
        let capsule = Capsule {
            center1: tube.points[i],
            center2: tube.points[(i + 1) % tube.points.len()],
            radius: tube.radius,
        };
        let part = compute_capsule_mass(&capsule, shape_def.density);
        expected_mass += part.mass;
        expected_inertia = add_mm(expected_inertia, add_mm(part.inertia, steiner(part.mass, sub(part.center, offset))));
    }
    assert!((mass.mass - expected_mass).abs() < 1.0e-5);
    assert!(length(sub(mass.center, offset)) < 1.0e-5);
    assert!(length(sub(get_shape_centroid(shape), offset)) < 1.0e-5);
    let difference = sub_mm(mass.inertia, expected_inertia);
    assert!(length(difference.cx) + length(difference.cy) + length(difference.cz) < 1.0e-5);
    destroy_world(world);
}
