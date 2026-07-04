// Port of box3d/test/test_world.c (+ the scene helpers it uses from
// box3d/shared/benchmarks.c, stability.c, overflow_color.c).
//
// Adaptations from C (documented per test):
// - The Rust port has no global world registry: destroy_world consumes the
//   World, so the C `b3World_IsValid(worldId) == false` checks after destroy
//   are not expressible and are dropped.
// - Worker counts are ignored (always serial): world_set_worker_count is a
//   no-op stub and world_get_worker_count always returns 1.
// - user data is u64 instead of void*.

use makepad_box3d::body::*;
use makepad_box3d::compound::create_compound;
use makepad_box3d::constants::GRAPH_COLOR_COUNT;
use makepad_box3d::ensure;
use makepad_box3d::ensure_small;
use makepad_box3d::hull::{create_cylinder, create_rock, make_box_hull, make_cube_hull, make_offset_box_hull};
use makepad_box3d::id::WorldId;
use makepad_box3d::math_functions::{compute_cos_sin, length, vec3, Pos, Quat, Transform, Vec3, WorldTransform, PI};
use makepad_box3d::mesh::create_wave_mesh;
use makepad_box3d::physics_world::*;
use makepad_box3d::shape::*;
use makepad_box3d::test_utils::{random_vec3_uniform, set_random_seed};
use makepad_box3d::types::*;

fn world_id_of(world: &World) -> WorldId {
    WorldId { index1: world.world_id + 1, generation: world.generation }
}

// This is a simple example of building and running a simulation
// using Box3D. Here we create a large ground box and a small dynamic box.
#[test]
fn hello_world() {
    // Construct a world object, which will hold and simulate the rigid bodies.
    let mut world_def = default_world_def();
    world_def.gravity = vec3(0.0, -10.0, 0.0);

    let mut world = create_world(&world_def);
    ensure!(world_is_valid(&world, world_id_of(&world)));

    // Define the ground body.
    let mut ground_body_def = default_body_def();
    ground_body_def.position = vec3(0.0, -10.0, 0.0);

    let ground_id = create_body(&mut world, &ground_body_def);
    ensure!(body_is_valid(&world, ground_id));

    // Define the ground box shape. The extents are the half-widths of the box.
    let ground_box = make_box_hull(50.0, 10.0, 50.0);

    // Add the box shape to the ground body.
    let ground_shape_def = default_shape_def();
    create_hull_shape(&mut world, ground_id, &ground_shape_def, &ground_box);

    // Define the dynamic body. We set its position and call the body factory.
    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    body_def.position = vec3(0.0, 4.0, 0.0);

    let body_id = create_body(&mut world, &body_def);

    // Define another box shape for our dynamic body.
    let dynamic_box = make_cube_hull(1.0);

    // Define the dynamic body shape
    let mut shape_def = default_shape_def();

    // Set the box density to be non-zero, so it will be dynamic.
    shape_def.density = 1.0;

    // Override the default friction.
    shape_def.base_material.friction = 0.3;

    // Add the shape to the body.
    create_hull_shape(&mut world, body_id, &shape_def, &dynamic_box);

    let time_step = 1.0 / 60.0;
    let sub_step_count = 4;

    let mut position = body_get_position(&world, body_id);
    let mut rotation = body_get_rotation(&world, body_id);

    // This is our little game loop.
    for _ in 0..90 {
        world_step(&mut world, time_step, sub_step_count);

        position = body_get_position(&world, body_id);
        rotation = body_get_rotation(&world, body_id);
    }

    destroy_world(world);

    ensure_small!(position.y - 1.00, 0.01);
    ensure_small!(rotation.v.x, 0.01);
    ensure_small!(rotation.v.z, 0.01);
}

#[test]
fn empty_world() {
    let world_def = default_world_def();
    let mut world = create_world(&world_def);
    ensure!(world_is_valid(&world, world_id_of(&world)));

    let time_step = 1.0 / 60.0;
    let sub_step_count = 1;

    for _ in 0..60 {
        world_step(&mut world, time_step, sub_step_count);
    }

    destroy_world(world);
    // C: ENSURE(b3World_IsValid(worldId) == false) — not expressible with an owned World.
}

const BODY_COUNT: usize = 10;

#[test]
fn destroy_all_bodies_world() {
    let world_def = default_world_def();
    let mut world = create_world(&world_def);

    let mut count = 0usize;
    let mut creating = true;

    let mut body_ids = [makepad_box3d::id::NULL_BODY_ID; BODY_COUNT];
    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    let cube = make_cube_hull(0.5);

    for _ in 0..(2 * BODY_COUNT + 10) {
        if creating {
            if count < BODY_COUNT {
                body_ids[count] = create_body(&mut world, &body_def);

                let shape_def = default_shape_def();
                create_hull_shape(&mut world, body_ids[count], &shape_def, &cube);
                count += 1;
            } else {
                creating = false;
            }
        } else if count > 0 {
            destroy_body(&mut world, body_ids[count - 1]);
            body_ids[count - 1] = makepad_box3d::id::NULL_BODY_ID;
            count -= 1;
        }

        world_step(&mut world, 1.0 / 60.0, 3);
    }

    let counters = world_get_counters(&world);
    ensure!(counters.body_count == 0);

    destroy_world(world);
}

#[test]
fn test_is_valid() {
    let world_def = default_world_def();
    let mut world = create_world(&world_def);
    ensure!(world_is_valid(&world, world_id_of(&world)));

    let body_def = default_body_def();

    let body_id1 = create_body(&mut world, &body_def);
    ensure!(body_is_valid(&world, body_id1) == true);

    let body_id2 = create_body(&mut world, &body_def);
    ensure!(body_is_valid(&world, body_id2) == true);

    destroy_body(&mut world, body_id1);
    ensure!(body_is_valid(&world, body_id1) == false);

    destroy_body(&mut world, body_id2);
    ensure!(body_is_valid(&world, body_id2) == false);

    destroy_world(world);
    // C checks world/body validity after world destroy — not expressible with an owned World.
}

const WORLD_COUNT: usize = 128 / 2; // B3_MAX_WORLDS / 2

#[test]
fn test_world_recycle() {
    let count = 100;

    for _ in 0..count {
        let world_def = default_world_def();
        let mut worlds: Vec<World> = Vec::with_capacity(WORLD_COUNT);
        for _ in 0..WORLD_COUNT {
            let mut world = create_world(&world_def);
            ensure!(world_is_valid(&world, world_id_of(&world)));

            let body_def = default_body_def();
            create_body(&mut world, &body_def);
            worlds.push(world);
        }

        for world in worlds.iter_mut() {
            let time_step = 1.0 / 60.0;
            let sub_step_count = 1;

            for _ in 0..10 {
                world_step(world, time_step, sub_step_count);
            }
        }

        while let Some(world) = worlds.pop() {
            destroy_world(world);
        }
    }
}

// This test is here to ensure all API functions link correctly.
#[test]
fn test_world_coverage() {
    let world_def = default_world_def();

    let mut world = create_world(&world_def);
    ensure!(world_is_valid(&world, world_id_of(&world)));

    world_enable_sleeping(&mut world, true);
    world_enable_sleeping(&mut world, false);
    let mut flag = world_is_sleeping_enabled(&world);
    ensure!(flag == false);

    world_enable_continuous(&mut world, false);
    world_enable_continuous(&mut world, true);
    flag = world_is_continuous_enabled(&world);
    ensure!(flag == true);

    world_set_restitution_threshold(&mut world, 0.0);
    world_set_restitution_threshold(&mut world, 2.0);
    let mut value = world_get_restitution_threshold(&world);
    ensure!(value == 2.0);

    world_set_hit_event_threshold(&mut world, 0.0);
    world_set_hit_event_threshold(&mut world, 100.0);
    value = world_get_hit_event_threshold(&world);
    ensure!(value == 100.0);

    // C passes fn pointers with a NULL context; the port takes closures.
    world_set_custom_filter_callback(&mut world, Some(Box::new(|_a, _b| true)));
    world_set_pre_solve_callback(&mut world, Some(Box::new(|_a, _b, _point, _normal| false)));

    let g = vec3(1.0, 2.0, 0.0);
    world_set_gravity(&mut world, g);
    let v = world_get_gravity(&world);
    ensure!(v.x == g.x);
    ensure!(v.y == g.y);

    let explosion_def = default_explosion_def();
    world_explode(&mut world, &explosion_def);

    world_set_contact_tuning(&mut world, 10.0, 2.0, 4.0);

    world_set_maximum_linear_speed(&mut world, 10.0);
    value = world_get_maximum_linear_speed(&world);
    ensure!(value == 10.0);

    world_enable_warm_starting(&mut world, true);
    flag = world_is_warm_starting_enabled(&world);
    ensure!(flag == true);

    let count = world_get_awake_body_count(&world);
    ensure!(count == 0);

    world_set_user_data(&mut world, 0xDEADBEEF);
    let user_data = world_get_user_data(&world);
    ensure!(user_data == 0xDEADBEEF);

    world_step(&mut world, 1.0, 1);

    destroy_world(world);
}

#[test]
fn test_sensor() {
    let world_def = default_world_def();
    let mut world = create_world(&world_def);

    // Wall from x = 1 to x = 2
    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Static;
    body_def.position = vec3(1.5, 11.0, 0.0);
    let wall_id = create_body(&mut world, &body_def);
    let box_hull = make_box_hull(0.5, 10.0, 1.0);
    let mut shape_def = default_shape_def();
    shape_def.enable_sensor_events = true;
    create_hull_shape(&mut world, wall_id, &shape_def, &box_hull);

    // Bullet fired towards the wall
    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    body_def.is_bullet = true;
    body_def.gravity_scale = 0.0;
    body_def.position = vec3(7.39814, 4.0, 0.0);
    body_def.linear_velocity = vec3(-20.0, 0.0, 0.0);
    let bullet_id = create_body(&mut world, &body_def);
    let mut shape_def = default_shape_def();
    shape_def.is_sensor = true;
    shape_def.enable_sensor_events = true;
    let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.1 };
    create_sphere_shape(&mut world, bullet_id, &shape_def, &sphere);

    let mut begin_count = 0;
    let mut end_count = 0;

    loop {
        let time_step = 1.0 / 60.0;
        let sub_step_count = 4;
        world_step(&mut world, time_step, sub_step_count);

        let bullet_pos = body_get_position(&world, bullet_id);

        let events = world_get_sensor_events(&world);

        if !events.begin_events.is_empty() {
            begin_count += 1;
        }

        if !events.end_events.is_empty() {
            end_count += 1;
        }

        if bullet_pos.x < -1.0 {
            break;
        }
    }

    destroy_world(world);

    ensure!(begin_count == 1);
    ensure!(end_count == 1);
}

#[test]
fn test_contact_events() {
    let world_def = default_world_def();
    let mut world = create_world(&world_def);

    // Static ground
    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Static;
    body_def.position = vec3(0.0, -0.5, 0.0);
    let ground_id = create_body(&mut world, &body_def);
    let ground_box = make_box_hull(10.0, 0.5, 10.0);
    let ground_shape_def = default_shape_def();
    let ground_shape_id = create_hull_shape(&mut world, ground_id, &ground_shape_def, &ground_box);

    // Dynamic sphere dropped onto the ground; restitution causes it to bounce so we get end events
    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    body_def.position = vec3(0.0, 5.0, 0.0);
    let sphere_body_id = create_body(&mut world, &body_def);
    let mut shape_def = default_shape_def();
    shape_def.density = 1.0;
    shape_def.enable_contact_events = true;
    shape_def.base_material.restitution = 0.6;
    let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.5 };
    let sphere_shape_id = create_sphere_shape(&mut world, sphere_body_id, &shape_def, &sphere);

    let mut begin_count = 0;
    let mut end_count = 0;
    let mut ids_checked = false;

    for _ in 0..120 {
        world_step(&mut world, 1.0 / 60.0, 4);

        let (first_begin, begin_len, end_len) = {
            let events = world_get_contact_events(&world);
            (
                events.begin_events.first().copied(),
                events.begin_events.len(),
                events.end_events.len(),
            )
        };

        if begin_len > 0 && ids_checked == false {
            let be = first_begin.unwrap();
            let a_is_sphere = be.shape_id_a == sphere_shape_id;
            let b_is_sphere = be.shape_id_b == sphere_shape_id;
            let a_is_ground = be.shape_id_a == ground_shape_id;
            let b_is_ground = be.shape_id_b == ground_shape_id;
            ensure!((a_is_sphere && b_is_ground) || (a_is_ground && b_is_sphere));
            ensure!(contact_is_valid(&world, be.contact_id));
            ids_checked = true;
        }

        begin_count += begin_len;
        end_count += end_len;
    }

    destroy_world(world);

    ensure!(ids_checked);
    ensure!(begin_count >= 1);
    ensure!(end_count >= 1);
}

#[test]
fn test_hit_events() {
    let mut world_def = default_world_def();
    world_def.hit_event_threshold = 1.0;
    let mut world = create_world(&world_def);

    // Static ground
    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Static;
    body_def.position = vec3(0.0, -0.5, 0.0);
    let ground_id = create_body(&mut world, &body_def);
    let ground_box = make_box_hull(10.0, 0.5, 10.0);
    let ground_shape_def = default_shape_def();
    create_hull_shape(&mut world, ground_id, &ground_shape_def, &ground_box);

    // Sphere driven into the ground fast enough to clear the hit threshold
    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    body_def.gravity_scale = 0.0;
    body_def.position = vec3(0.0, 2.0, 0.0);
    body_def.linear_velocity = vec3(0.0, -30.0, 0.0);
    let sphere_body_id = create_body(&mut world, &body_def);
    let mut shape_def = default_shape_def();
    shape_def.density = 1.0;
    shape_def.enable_hit_events = true;
    shape_def.base_material.user_material_id = 7;
    let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.5 };
    create_sphere_shape(&mut world, sphere_body_id, &shape_def, &sphere);

    let mut hit_count = 0;
    let mut captured_speed = 0.0;
    let mut captured_material_a = 0u64;
    let mut captured_material_b = 0u64;
    let mut captured_normal = vec3(0.0, 0.0, 0.0);

    for _ in 0..30 {
        world_step(&mut world, 1.0 / 60.0, 4);

        let events = world_get_contact_events(&world);
        if !events.hit_events.is_empty() && hit_count == 0 {
            let hit = events.hit_events[0];
            captured_speed = hit.approach_speed;
            captured_normal = hit.normal;
            captured_material_a = hit.user_material_id_a;
            captured_material_b = hit.user_material_id_b;
        }

        hit_count += events.hit_events.len();
    }

    destroy_world(world);

    ensure!(hit_count >= 1);
    ensure!(captured_speed > 1.0);
    // Head-on vertical impact: normal lies along Y
    ensure_small!(captured_normal.x, 0.01);
    ensure_small!(captured_normal.z, 0.01);
    // One side of the contact carries the sphere's user material
    ensure!(captured_material_a == 7 || captured_material_b == 7);
}

// Hit-event material lookup must respect the compound child that participated in the
// contact. Two children with distinct userMaterialIds at separated positions, dropped
// sphere strikes one specifically.
#[test]
fn test_compound_hit_events() {
    const HULL_MATERIAL_A: u64 = 11;
    const HULL_MATERIAL_B: u64 = 22;
    const SPHERE_MATERIAL: u64 = 99;
    const HULL_CENTER_X: f32 = 3.0;

    for side in 0..2 {
        let expected_hull_material = if side == 0 { HULL_MATERIAL_A } else { HULL_MATERIAL_B };
        let spawn_x = if side == 0 { -HULL_CENTER_X } else { HULL_CENTER_X };

        let mut world_def = default_world_def();
        world_def.hit_event_threshold = 1.0;
        let mut world = create_world(&world_def);

        // Build a compound with two hulls at opposite x positions, distinct userMaterialIds
        let box_a = make_box_hull(1.0, 1.0, 1.0);
        let box_b = make_box_hull(1.0, 1.0, 1.0);

        let mut mat_a = default_surface_material();
        mat_a.user_material_id = HULL_MATERIAL_A;

        let mut mat_b = default_surface_material();
        mat_b.user_material_id = HULL_MATERIAL_B;

        let compound_def = CompoundDef {
            hulls: vec![
                CompoundHullDef {
                    hull: box_a.clone(),
                    transform: Transform { p: vec3(-HULL_CENTER_X, 0.0, 0.0), q: Quat::IDENTITY },
                    material: mat_a,
                },
                CompoundHullDef {
                    hull: box_b.clone(),
                    transform: Transform { p: vec3(HULL_CENTER_X, 0.0, 0.0), q: Quat::IDENTITY },
                    material: mat_b,
                },
            ],
            ..Default::default()
        };
        let compound = create_compound(&compound_def);

        // Static body holds the compound
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Static;
        let compound_body_id = create_body(&mut world, &body_def);
        let compound_shape_def = default_shape_def();
        create_compound_shape(&mut world, compound_body_id, &compound_shape_def, &compound);

        // Sphere driven straight down onto the chosen child
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.gravity_scale = 0.0;
        body_def.position = vec3(spawn_x, 3.0, 0.0);
        body_def.linear_velocity = vec3(0.0, -30.0, 0.0);
        let sphere_body_id = create_body(&mut world, &body_def);
        let mut sphere_shape_def = default_shape_def();
        sphere_shape_def.density = 1.0;
        sphere_shape_def.enable_hit_events = true;
        sphere_shape_def.base_material.user_material_id = SPHERE_MATERIAL;
        let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.5 };
        create_sphere_shape(&mut world, sphere_body_id, &sphere_shape_def, &sphere);

        let mut hit_count = 0;
        let mut captured_material_a = 0u64;
        let mut captured_material_b = 0u64;

        for _ in 0..30 {
            world_step(&mut world, 1.0 / 60.0, 4);

            let events = world_get_contact_events(&world);
            if !events.hit_events.is_empty() && hit_count == 0 {
                let hit = events.hit_events[0];
                captured_material_a = hit.user_material_id_a;
                captured_material_b = hit.user_material_id_b;
            }
            hit_count += events.hit_events.len();
        }

        destroy_world(world);

        ensure!(hit_count >= 1);
        // Sphere material on one side
        ensure!(captured_material_a == SPHERE_MATERIAL || captured_material_b == SPHERE_MATERIAL);
        // Struck compound child's material on the other side.
        ensure!(captured_material_a == expected_hull_material || captured_material_b == expected_hull_material);
    }
}

struct JunkyardData {
    pusher_id: makepad_box3d::id::BodyId,
    degrees: f32,
    radius: f32,
}

// Port of CreateJunkyard from box3d/shared/benchmarks.c using the debug body
// count (2 layers instead of 24) — the C test build uses the same reduction.
fn create_junkyard(world: &mut World) -> JunkyardData {
    let ground_id;
    {
        let mut body_def = default_body_def();
        body_def.position.y = -1.0;
        ground_id = create_body(world, &body_def);
    }

    {
        let shape_def = default_shape_def();
        {
            let box_hull = make_box_hull(120.0, 1.0, 120.0);
            create_hull_shape(world, ground_id, &shape_def, &box_hull);
        }
        {
            let box_hull = make_offset_box_hull(1.0, 8.0, 50.0, vec3(-50.0, 8.0, 0.0));
            create_hull_shape(world, ground_id, &shape_def, &box_hull);
        }
        {
            let box_hull = make_offset_box_hull(1.0, 8.0, 50.0, vec3(50.0, 8.0, 0.0));
            create_hull_shape(world, ground_id, &shape_def, &box_hull);
        }
        {
            let box_hull = make_offset_box_hull(50.0, 8.0, 1.0, vec3(0.0, 8.0, -50.0));
            create_hull_shape(world, ground_id, &shape_def, &box_hull);
        }
        {
            let box_hull = make_offset_box_hull(50.0, 8.0, 1.0, vec3(0.0, 8.0, 50.0));
            create_hull_shape(world, ground_id, &shape_def, &box_hull);
        }
    }
    {
        let rock_hull = create_rock(1.5);

        let count = 2; // BENCHMARK_DEBUG variant
        let height = 24.0;
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        let shape_def = default_shape_def();
        for y in 0..count {
            for x in 0..=20 {
                for z in 0..=20 {
                    body_def.position.x = -40.0 + 4.0 * x as f32;
                    body_def.position.y = 4.0 * y as f32 + height + 1.0;
                    body_def.position.z = -40.0 + 4.0 * z as f32;
                    let body_id = create_body(world, &body_def);
                    create_hull_shape(world, body_id, &shape_def, &rock_hull);
                }
            }
        }
    }

    let radius = 35.0;
    let m_height = 24.0;

    let hull = create_cylinder(m_height, 4.0, 0.0, 16);
    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Kinematic;
    body_def.position = vec3(radius, 0.0, 0.0);
    let pusher_id = create_body(world, &body_def);
    let shape_def = default_shape_def();
    create_hull_shape(world, pusher_id, &shape_def, &hull);

    JunkyardData { pusher_id, degrees: 0.0, radius }
}

// Port of StepJunkyard: note the C version only drives the kinematic pusher; it
// does not step the world.
fn step_junkyard(world: &mut World, data: &mut JunkyardData) {
    let time_step = 1.0 / 60.0;
    let omega = -6.0;
    data.degrees += omega * time_step;
    let cs = compute_cos_sin(data.degrees * PI / 180.0);
    let r = data.radius;
    let target_pos: Pos = vec3(r * cs.cosine, 0.0, r * cs.sine);
    let target = WorldTransform { p: target_pos, q: Quat::IDENTITY };
    body_set_target_transform(world, data.pusher_id, target, time_step, false);
}

// Adapted: the Rust port is always serial. world_set_worker_count is a no-op
// stub and world_get_worker_count always returns 1, so the C expectations of
// 4 / B3_MAX_WORKERS collapse to 1.
#[test]
fn test_set_worker_count() {
    let mut world_def = default_world_def();
    world_def.worker_count = 1;
    let mut world = create_world(&world_def);
    ensure!(world_get_worker_count(&world) == 1);

    let mut junkyard = create_junkyard(&mut world);
    step_junkyard(&mut world, &mut junkyard);

    world_set_worker_count(&mut world, 4);
    ensure!(world_get_worker_count(&world) == 1);

    step_junkyard(&mut world, &mut junkyard);

    world_set_worker_count(&mut world, 4);
    ensure!(world_get_worker_count(&world) == 1);

    step_junkyard(&mut world, &mut junkyard);

    world_set_worker_count(&mut world, 0);
    ensure!(world_get_worker_count(&world) == 1);

    step_junkyard(&mut world, &mut junkyard);

    world_set_worker_count(&mut world, -5);
    ensure!(world_get_worker_count(&world) == 1);

    step_junkyard(&mut world, &mut junkyard);

    world_set_worker_count(&mut world, makepad_box3d::constants::MAX_WORKERS as i32 + 10);
    ensure!(world_get_worker_count(&world) == 1);

    step_junkyard(&mut world, &mut junkyard);

    destroy_world(world);
}

// This tests continuous collision and mesh contact stability.
// Port of CreateMeshDrop from box3d/shared/stability.c.
#[test]
fn test_mesh_drop() {
    let world_def = default_world_def();
    // C requests 4 workers; the port is always serial.

    let mut world = create_world(&world_def);

    // CreateMeshDrop
    {
        let body_def = default_body_def();
        let ground_id = create_body(&mut world, &body_def);

        let grid_count = 40;
        let cell_width = 1.0;
        let row_hz = 0.1;
        let column_hz = 0.2;
        let ground_amplitude = 0.5;

        let mesh = create_wave_mesh(grid_count, grid_count, cell_width, ground_amplitude, row_hz, column_hz);
        let mut shape_def = default_shape_def();
        shape_def.filter.category_bits = 1;
        create_mesh_shape(&mut world, ground_id, &shape_def, &mesh, Vec3::ONE);
    }

    {
        let box_hull = make_box_hull(0.02, 0.2, 0.04);

        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;

        let mut shape_def = default_shape_def();
        shape_def.base_material.rolling_resistance = 0.1;

        // Don't allow shapes to collide with each other.
        shape_def.filter.category_bits = 2;
        shape_def.filter.mask_bits = 1;

        set_random_seed(3963634789);

        let grid_count = 32; // MESH_DROP_GRID_COUNT

        for i in 0..grid_count {
            for j in 0..grid_count {
                let linear_velocity = random_vec3_uniform(-1.0, 1.0);
                let angular_velocity = random_vec3_uniform(-5.0, 5.0);

                body_def.position = vec3(
                    0.5 * (i as f32 - 0.5 * grid_count as f32),
                    5.0,
                    0.5 * (j as f32 - 0.5 * grid_count as f32),
                );
                body_def.linear_velocity = linear_velocity;
                body_def.angular_velocity = angular_velocity;
                let body_id = create_body(&mut world, &body_def);

                create_hull_shape(&mut world, body_id, &shape_def, &box_hull);
            }
        }
    }

    let time_step = 1.0 / 60.0;

    let mut step_index = 0;
    let step_limit = 400;

    while step_index < step_limit {
        let sub_step_count = 4;
        world_step(&mut world, time_step, sub_step_count);

        let move_count = world_get_body_events(&world).move_events.len();
        if move_count == 0 {
            // All bodies sleeping
            break;
        }

        step_index += 1;
    }

    println!("  test_mesh_drop step_index = {}", step_index);

    destroy_world(world);

    ensure!(step_index < step_limit);
}

// Verifies the overflow solver path. The scene puts more dyn-dyn contacts on a
// single hub body than there are dynamic graph colors, so several land in the
// overflow color. Port of CreateOverflowColorPile from box3d/shared/overflow_color.c.
#[test]
fn test_overflow_color_pile() {
    const RING_COUNT: usize = 5;
    const PER_RING: usize = 5;

    let world_def = default_world_def();
    let mut world = create_world(&world_def);

    // Static ground (top surface at y = 0)
    {
        let mut body_def = default_body_def();
        body_def.position = vec3(0.0, -1.0, 0.0);
        let ground_id = create_body(&mut world, &body_def);

        let box_hull = make_box_hull(20.0, 1.0, 20.0);
        let shape_def = default_shape_def();
        create_hull_shape(&mut world, ground_id, &shape_def, &box_hull);
    }

    // Tall, heavy hub.
    let hub_half_x = 0.5f32;
    let hub_half_y = 2.5f32;
    let hub_half_z = 0.5f32;
    {
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.position = vec3(0.0, hub_half_y, 0.0);
        let hub_id = create_body(&mut world, &body_def);

        let box_hull = make_box_hull(hub_half_x, hub_half_y, hub_half_z);
        let mut shape_def = default_shape_def();
        shape_def.density = 50.0;
        create_hull_shape(&mut world, hub_id, &shape_def, &box_hull);
    }

    // Neighbors: vertical rings around the hub, each box slightly overlapping
    // the hub so a contact exists on the very first step.
    let neighbor_half = 0.2f32;
    let ring_radius = hub_half_x + neighbor_half - 0.03;

    let neighbor_box = make_box_hull(neighbor_half, neighbor_half, neighbor_half);
    let neighbor_shape = default_shape_def();

    let ring_spacing = 0.5f32;
    let base_y = neighbor_half + 0.05;

    let _ = hub_half_z;

    for ring in 0..RING_COUNT {
        let y = base_y + ring_spacing * ring as f32;

        // Offset alternate rings by half a slot.
        let theta_offset = if (ring & 1) != 0 { PI / PER_RING as f32 } else { 0.0 };

        for slot in 0..PER_RING {
            let theta = theta_offset + (2.0 * PI * slot as f32) / PER_RING as f32;

            let mut body_def = default_body_def();
            body_def.body_type = BodyType::Dynamic;
            body_def.position = vec3(ring_radius * theta.cos(), y, ring_radius * theta.sin());
            let body_id = create_body(&mut world, &body_def);

            create_hull_shape(&mut world, body_id, &neighbor_shape, &neighbor_box);
        }
    }

    let time_step = 1.0 / 60.0;
    let sub_step_count = 4;

    let step_count = 10;
    for _ in 0..step_count {
        world_step(&mut world, time_step, sub_step_count);
    }

    // Confirm the scene actually populated the overflow color.
    let counters = world_get_counters(&world);
    let overflow_contacts = counters.color_counts[GRAPH_COLOR_COUNT - 1];

    destroy_world(world);

    ensure!(overflow_contacts > 0);
}

// b3Body_EnableSleep must sync bodySim/bodyState flags; the flag-sync assertion
// in validate_solver_sets fires on the next step otherwise.
#[test]
fn enable_sleep_flag_sync_test() {
    let world_def = default_world_def();
    let mut world = create_world(&world_def);

    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    body_def.enable_sleep = false;
    let body_id = create_body(&mut world, &body_def);

    ensure!(body_is_sleep_enabled(&world, body_id) == false);

    body_enable_sleep(&mut world, body_id, true);
    ensure!(body_is_sleep_enabled(&world, body_id) == true);

    world_step(&mut world, 1.0 / 60.0, 4);

    destroy_world(world);
}

// b3Body_SetBullet must not drift against b3SyncBodyFlags.
#[test]
fn set_bullet_drift_test() {
    let world_def = default_world_def();
    let mut world = create_world(&world_def);

    {
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.is_bullet = false;
        let body_id = create_body(&mut world, &body_def);

        ensure!(body_is_bullet(&world, body_id) == false);

        body_set_bullet(&mut world, body_id, true);
        ensure!(body_is_bullet(&world, body_id) == true);

        let mut locks = MotionLocks::default();
        locks.linear_x = true;
        body_set_motion_locks(&mut world, body_id, locks);

        ensure!(body_is_bullet(&world, body_id) == true);
    }

    {
        let mut body_def = default_body_def();
        body_def.body_type = BodyType::Dynamic;
        body_def.is_bullet = true;
        let body_id = create_body(&mut world, &body_def);

        ensure!(body_is_bullet(&world, body_id) == true);

        body_set_bullet(&mut world, body_id, false);
        ensure!(body_is_bullet(&world, body_id) == false);

        let mut locks = MotionLocks::default();
        locks.linear_x = true;
        body_set_motion_locks(&mut world, body_id, locks);

        ensure!(body_is_bullet(&world, body_id) == false);
    }

    destroy_world(world);
}

// Regression: b3Body_EnableSleep used to leak the world lock on a no-op change.
#[test]
fn enable_sleep_noop_unlock_test() {
    let world_def = default_world_def();
    let mut world = create_world(&world_def);

    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    body_def.enable_sleep = true;
    let body_id = create_body(&mut world, &body_def);

    // No-op: enableSleep is already true. Must not leak the world lock.
    body_enable_sleep(&mut world, body_id, true);

    // Would assert in the unlocked-world guard if the lock had leaked.
    body_enable_sleep(&mut world, body_id, false);
    ensure!(body_is_sleep_enabled(&world, body_id) == false);

    destroy_world(world);
}

#[test]
fn enable_contact_recycling_test() {
    let world_def = default_world_def();
    let mut world = create_world(&world_def);

    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;

    // Default is enabled
    let body_a = create_body(&mut world, &body_def);
    ensure!(body_is_contact_recycling_enabled(&world, body_a) == true);

    body_enable_contact_recycling(&mut world, body_a, false);
    ensure!(body_is_contact_recycling_enabled(&world, body_a) == false);

    body_enable_contact_recycling(&mut world, body_a, true);
    ensure!(body_is_contact_recycling_enabled(&world, body_a) == true);

    // Per-def opt-out at creation
    body_def.enable_contact_recycling = false;
    let body_b = create_body(&mut world, &body_def);
    ensure!(body_is_contact_recycling_enabled(&world, body_b) == false);

    // Stepping after toggling must not trip the flag-sync validator
    world_step(&mut world, 1.0 / 60.0, 4);

    destroy_world(world);
}

// Identical hull data is shared through a reference counted world database.
#[test]
fn test_hull_database() {
    let world_def = default_world_def();
    let mut world = create_world(&world_def);

    let box_hull = make_box_hull(0.5, 0.5, 0.5);

    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    let body_a = create_body(&mut world, &body_def);
    let body_b = create_body(&mut world, &body_def);

    let shape_def = default_shape_def();

    // Two shapes built from identical data share one owned copy in the world database.
    let shape_a = create_hull_shape(&mut world, body_a, &shape_def, &box_hull);
    let shape_b = create_hull_shape(&mut world, body_b, &shape_def, &box_hull);

    let got_a = shape_get_hull(&world, shape_a);
    let got_b = shape_get_hull(&world, shape_b);

    // Both shapes point at the single shared copy
    ensure!(std::sync::Arc::ptr_eq(&got_a, &got_b));

    // C also checks the shared copy is not the caller's stack hull. The Rust
    // port stores the caller's Arc on first insert by design (Arc handles the
    // lifetime), so that check does not apply.

    // A box built independently must de-duplicate to the same shared copy.
    let box2 = make_box_hull(0.5, 0.5, 0.5);
    let body_c = create_body(&mut world, &body_def);
    let shape_c = create_hull_shape(&mut world, body_c, &shape_def, &box2);
    ensure!(std::sync::Arc::ptr_eq(&shape_get_hull(&world, shape_c), &got_a));
    destroy_shape(&mut world, shape_c, true);

    // Setting a shape's hull to its own sole shared copy must not free it mid update.
    let box3 = make_box_hull(0.3, 0.3, 0.3);
    let body_d = create_body(&mut world, &body_def);
    let shape_d = create_hull_shape(&mut world, body_d, &shape_def, &box3);
    let got_d = shape_get_hull(&world, shape_d);
    shape_set_hull(&mut world, shape_d, &got_d);
    ensure!(std::sync::Arc::ptr_eq(&shape_get_hull(&world, shape_d), &got_d));
    destroy_shape(&mut world, shape_d, true);

    // Releasing one reference keeps the other alive
    destroy_shape(&mut world, shape_a, true);
    let still_b = shape_get_hull(&world, shape_b);
    ensure!(std::sync::Arc::ptr_eq(&still_b, &got_b));

    destroy_shape(&mut world, shape_b, true);

    // World destroy asserts the database drained to zero references
    destroy_world(world);
}

struct ExplosionResult {
    linear_velocity: Vec3,
    angular_velocity: Vec3,
}

// Explode just off the +x side of a centered sphere and capture the impulse it
// receives. The result must not depend on how far the body sits from the origin.
fn run_explosion(base: Pos) -> ExplosionResult {
    let mut world_def = default_world_def();
    world_def.gravity = Vec3::ZERO;
    let mut world = create_world(&world_def);

    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    body_def.position = base;
    let body_id = create_body(&mut world, &body_def);

    let sphere = Sphere { center: Vec3::ZERO, radius: 1.0 };
    let shape_def = default_shape_def();
    create_sphere_shape(&mut world, body_id, &shape_def, &sphere);

    // Blast sits 3 units along +x, so the body is pushed back along -x
    let mut explosion_def = default_explosion_def();
    explosion_def.position = makepad_box3d::math_functions::offset_pos(base, vec3(3.0, 0.0, 0.0));
    explosion_def.radius = 5.0;
    explosion_def.falloff = 0.0;
    explosion_def.impulse_per_area = 10.0;
    world_explode(&mut world, &explosion_def);

    let result = ExplosionResult {
        linear_velocity: body_get_linear_velocity(&world, body_id),
        angular_velocity: body_get_angular_velocity(&world, body_id),
    };

    destroy_world(world);
    result
}

#[test]
fn test_explosion() {
    let origin = run_explosion(Vec3::ZERO);

    // Pushed away from the blast along -x. A centered sphere has no transverse
    // or angular component.
    ensure!(origin.linear_velocity.x < -1.0e-4);
    ensure_small!(origin.linear_velocity.y, 1.0e-6);
    ensure_small!(origin.linear_velocity.z, 1.0e-6);
    ensure_small!(length(origin.angular_velocity), 1.0e-6);

    // The same blast far from the origin must produce the same impulse.
    let far = run_explosion(vec3(1.0e7, 1.0e7, 1.0e7));
    ensure_small!(far.linear_velocity.x - origin.linear_velocity.x, 1.0e-5);
    ensure_small!(far.linear_velocity.y - origin.linear_velocity.y, 1.0e-5);
    ensure_small!(far.linear_velocity.z - origin.linear_velocity.z, 1.0e-5);
}

// Ensure correct move events from bodies involved in CCD.
#[test]
fn test_continuous_move_event() {
    let world_def = default_world_def();
    let mut world = create_world(&world_def);
    world_enable_continuous(&mut world, true);

    // Thin static wall, near face at x = 0.1
    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Static;
    body_def.position = vec3(0.0, 0.0, 0.0);
    let wall_id = create_body(&mut world, &body_def);
    let wall_box = make_box_hull(0.1, 5.0, 5.0);
    let shape_def = default_shape_def();
    create_hull_shape(&mut world, wall_id, &shape_def, &wall_box);

    // Fast dynamic sphere fired at the wall.
    let mut body_def = default_body_def();
    body_def.body_type = BodyType::Dynamic;
    body_def.gravity_scale = 0.0;
    body_def.position = vec3(3.0, 0.0, 0.0);
    body_def.linear_velocity = vec3(-30.0, 0.0, 0.0);
    let ball_id = create_body(&mut world, &body_def);
    let mut shape_def = default_shape_def();
    shape_def.density = 1.0;
    let sphere = Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.25 };
    create_sphere_shape(&mut world, ball_id, &shape_def, &sphere);

    let time_step = 1.0 / 60.0;
    let sub_step_count = 4;
    let mut have_move = false;

    for _ in 0..30 {
        world_step(&mut world, time_step, sub_step_count);

        let xf = body_get_transform(&world, ball_id);

        let events = world_get_body_events(&world);
        for event in events.move_events {
            if event.body_id != ball_id {
                continue;
            }

            have_move = true;

            // The move event must carry the same pose the body reports, CCD rewind included
            ensure!(event.transform.p.x == xf.p.x);
            ensure!(event.transform.p.y == xf.p.y);
            ensure!(event.transform.p.z == xf.p.z);
            ensure!(event.transform.q.v.x == xf.q.v.x);
            ensure!(event.transform.q.v.y == xf.q.v.y);
            ensure!(event.transform.q.v.z == xf.q.v.z);
            ensure!(event.transform.q.s == xf.q.s);
        }
    }

    ensure!(have_move == true);

    // Tunnel check
    let final_pos = body_get_position(&world, ball_id);
    ensure!(0.2 < final_pos.x && final_pos.x < 0.8);

    destroy_world(world);
}
