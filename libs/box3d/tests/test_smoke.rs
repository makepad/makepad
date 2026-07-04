// Not a C test port: a quick end-to-end smoke test of the Rust port.
// A dynamic sphere falls onto a static box and comes to rest on top of it.

use makepad_box3d::body::*;
use makepad_box3d::ensure;
use makepad_box3d::hull::make_box_hull;
#[allow(unused_imports)]
use makepad_box3d::math_functions::{pos, vec3};
use makepad_box3d::physics_world::*;
use makepad_box3d::shape::*;
use makepad_box3d::types::*;

#[test]
fn smoke_falling_sphere() {
    let world_def = default_world_def();
    let mut world = create_world(&world_def);

    // Ground: static box 20 x 1 x 20 centered at origin
    let ground_def = default_body_def();
    let ground_id = create_body(&mut world, &ground_def);
    let ground_hull = make_box_hull(10.0, 0.5, 10.0);
    let shape_def = default_shape_def();
    let _ground_shape = create_hull_shape(&mut world, ground_id, &shape_def, &ground_hull);

    // Falling sphere
    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    body_def.position = pos(0.0, 5.0, 0.0);
    let body_id = create_body(&mut world, &body_def);
    let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.5 };
    let _sphere_shape = create_sphere_shape(&mut world, body_id, &shape_def, &sphere);

    let mut p = body_get_position(&world, body_id);
    ensure!(p.y == 5.0);

    for _ in 0..120 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }

    p = body_get_position(&world, body_id);

    // Rest height: ground top at 0.5, sphere radius 0.5 -> center near 1.0
    println!("final position: {} {} {}", p.x, p.y, p.z);
    assert!((p.y - 1.0).abs() < 0.05, "sphere should rest on the box, y = {}", p.y);
    ensure!(p.x.abs() < 0.01 && p.z.abs() < 0.01);

    // It should eventually fall asleep
    let mut asleep = false;
    for _ in 0..300 {
        world_step(&mut world, 1.0 / 60.0, 4);
        if !body_is_awake(&world, body_id) {
            asleep = true;
            break;
        }
    }
    assert!(asleep, "sphere should fall asleep");

    destroy_world(world);
}
