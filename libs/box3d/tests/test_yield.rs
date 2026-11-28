// A surface that gives at a set force: a contact pushes no harder than the
// softer side's yield force allows, so a body drives into it and stops over a
// distance set by its energy.

use makepad_box3d::body::*;
use makepad_box3d::hull::make_box_hull;
use makepad_box3d::physics_world::{create_world, destroy_world, world_step};
use makepad_box3d::shape::*;
use makepad_box3d::*;

const MASS: f32 = 1000.0; // a 1 m cube at water density
const SPEED: f32 = 6.0;
const STEP: f32 = 1.0 / 240.0; // one substep a step, so one step's impulse is one substep's

#[derive(Clone, Copy, PartialEq)]
enum Ground {
    Hull,
    BoxSet,
}

struct Run {
    /// Deepest the cube's bottom face got below the ground surface.
    depth: f32,
    /// Largest momentum change in one step.
    max_step_impulse: f32,
    final_speed_bits: u32,
    final_y_bits: u32,
}

fn drive_cube_into_ground(ground: Ground, ground_force: f32, cube_force: f32) -> Run {
    drive_cube(ground, ground_force, cube_force, None)
}

/// `restore_at`: after that many steps, carry on in a world restored from a snapshot.
fn drive_cube(ground: Ground, ground_force: f32, cube_force: f32, restore_at: Option<usize>) -> Run {
    let mut world_def = default_world_def();
    world_def.gravity = Vec3::ZERO;
    let mut world = create_world(&world_def);

    let ground_body = create_body(&mut world, &default_body_def());
    let ground_def = ShapeDef {
        base_material: SurfaceMaterial { yield_force: ground_force, ..default_surface_material() },
        ..default_shape_def()
    };
    match ground {
        Ground::Hull => {
            let hull = make_box_hull(6.0, 3.0, 6.0);
            create_hull_shape(&mut world, ground_body, &ground_def, &hull);
            // the hull is centred on its body: lower it so its top is the y = 0 surface
            body_set_transform(&mut world, ground_body, pos(0.0, -3.0, 0.0), Quat::IDENTITY);
        }
        Ground::BoxSet => {
            let slab = [SetBox {
                bounds: AABB { lower_bound: vec3(-6.0, -6.0, -6.0), upper_bound: vec3(6.0, 0.0, 6.0) },
                material: 0,
                user_data: 0,
            }];
            create_box_set_shape(&mut world, ground_body, &ground_def, &create_box_set(&slab));
        }
    }

    let cube = create_body(
        &mut world,
        &BodyDef {
            body_type: BodyType::Dynamic,
            position: pos(0.0, 0.8, 0.0),
            linear_velocity: vec3(0.0, -SPEED, 0.0),
            ..default_body_def()
        },
    );
    let cube_def = ShapeDef {
        base_material: SurfaceMaterial { yield_force: cube_force, ..default_surface_material() },
        ..default_shape_def()
    };
    create_hull_shape(&mut world, cube, &cube_def, &make_box_hull(0.5, 0.5, 0.5));
    assert!((body_get_mass(&world, cube) - MASS).abs() < 1.0, "the cube must weigh what the test assumes");

    let mut depth = 0.0f32;
    let mut max_step_impulse = 0.0f32;
    let mut previous = body_get_linear_velocity(&world, cube);
    for step in 0..90 {
        if restore_at == Some(step) {
            let mut snap = recording::RecBuffer::new();
            let mut registry = recording::Recording::new();
            world_snapshot::serialize_world(&world, &mut snap, &mut registry);
            recording::write_registry(&mut registry);
            let reader = recording_replay::load_registry(registry.data()).expect("the geometry registry must be readable");
            let mut shell = create_world(&world_def);
            assert!(world_snapshot::deserialize_into_shell(&snap.data, &mut shell, &reader), "the snapshot must restore");
            destroy_world(std::mem::replace(&mut world, shell));
        }
        world_step(&mut world, STEP, 1);
        let v = body_get_linear_velocity(&world, cube);
        max_step_impulse = max_step_impulse.max(MASS * (v.y - previous.y).abs());
        previous = v;
        depth = depth.max(0.5 - body_get_position(&world, cube).y as f32);
    }
    let run = Run {
        depth,
        max_step_impulse,
        final_speed_bits: previous.y.to_bits(),
        final_y_bits: (body_get_position(&world, cube).y as f32).to_bits(),
    };
    destroy_world(world);
    run
}

#[test]
fn yield_force_caps_the_contact_and_sets_the_stopping_distance() {
    let force = 30_000.0f32;
    let ideal = 0.5 * MASS * SPEED * SPEED / force; // 0.6 m
    for ground in [Ground::Hull, Ground::BoxSet] {
        let which = if ground == Ground::Hull { "hull (wide solve)" } else { "box set (scalar solve)" };
        let run = drive_cube_into_ground(ground, force, 0.0);
        assert!(
            run.depth <= ideal + SPEED * STEP,
            "{which}: the cube went {} m in, past the {} m its energy allows",
            run.depth,
            ideal
        );
        assert!(
            run.depth > 0.5 * ideal,
            "{which}: the cube went only {} m in, so the ground did not give (ideal {})",
            run.depth,
            ideal
        );
        assert!(
            run.max_step_impulse <= force * STEP * 1.001,
            "{which}: one substep pushed with {} N s, more than the cap {}",
            run.max_step_impulse,
            force * STEP
        );

        // The softer side decides, whichever side it is on, and a rigid side defers to it.
        let soft_cube = drive_cube_into_ground(ground, 0.0, force);
        let both = drive_cube_into_ground(ground, 4.0 * force, force);
        let both_swapped = drive_cube_into_ground(ground, force, 4.0 * force);
        assert!(
            soft_cube.depth.to_bits() == run.depth.to_bits() && both.depth.to_bits() == run.depth.to_bits()
                && both_swapped.depth.to_bits() == run.depth.to_bits(),
            "{which}: the contact must give at the minimum of the non-zero yield forces"
        );

        // Rigid on both sides is today's contact: it stops at the surface, and it is repeatable.
        // A world restored mid-crush keeps yielding, step for step.
        let restored = drive_cube(ground, force, 0.0, Some(20));
        assert!(
            restored.depth.to_bits() == run.depth.to_bits() && restored.final_y_bits == run.final_y_bits,
            "{which}: a world restored from a snapshot mid-crush must yield exactly as the original"
        );

        let rigid = drive_cube_into_ground(ground, 0.0, 0.0);
        let again = drive_cube_into_ground(ground, 0.0, 0.0);
        assert!(rigid.depth < 0.05, "{which}: a rigid ground let the cube in {} m", rigid.depth);
        assert!(
            rigid.final_speed_bits == again.final_speed_bits && rigid.final_y_bits == again.final_y_bits,
            "{which}: a rigid run must repeat bit for bit"
        );
    }
}

const H: f32 = 1.0 / 256.0;

fn impact_world(workers: u32) -> physics_world::World {
    create_world(&WorldDef {
        gravity: Vec3::ZERO,
        contact_speed: 0.0,
        enable_continuous: false,
        worker_count: workers,
        ..default_world_def()
    })
}

fn material(force: f32, restitution: f32) -> SurfaceMaterial {
    SurfaceMaterial { yield_force: force, restitution, friction: 0.0, ..default_surface_material() }
}

#[test]
fn pitched_multi_manifold_yielding_stop_does_not_rebound_at_contact_speed() {
    let with_wall = || {
        let mut w = create_world(&WorldDef { gravity: Vec3::ZERO, ..default_world_def() });
        let wall = create_body(&mut w, &BodyDef { position: pos(0., 0., -2.), ..default_body_def() });
        create_hull_shape(&mut w, wall, &default_shape_def(), &make_box_hull(5., 5., 2.));
        w
    };
    let mut w = with_wall();
    let body = create_body(&mut w, &BodyDef {
        body_type: BodyType::Dynamic,
        position: pos(0., 0., 1.02),
        rotation: make_quat_from_axis_angle(Vec3::AXIS_X, -0.08),
        linear_velocity: vec3(0., 0., -16.7),
        ..default_body_def()
    });
    // Two stepped front planes (0.10 m apart > linear_slop) sharing material 0 so
    // cluster_for keeps two manifolds and allocate_yield_caps shares one 300 kN cap.
    let nose = [
        SetBox {
            bounds: AABB { lower_bound: vec3(-1.0, -0.50, -1.00), upper_bound: vec3(1.0, -0.05, 1.0) },
            material: 0, user_data: 0,
        },
        SetBox {
            bounds: AABB { lower_bound: vec3(-1.0, 0.05, -0.90), upper_bound: vec3(1.0, 0.50, 1.0) },
            material: 0, user_data: 0,
        },
    ];
    let shape = create_box_set_shape(&mut w, body, &ShapeDef {
        density: 1., base_material: material(300_000., 0.), ..default_shape_def()
    }, &create_box_set(&nose));
    // Hold the -0.08 rad pitch fixed so translational rebound measures contact bias
    // rather than free-floating rotational slap-back (same idiom as line 334).
    body_set_mass_data(&mut w, body, MassData { mass: 1400., ..Default::default() });
    assert!((body_get_mass(&w, body) - 1400.).abs() < 0.01);
    let mut embedded_manifolds = 0;
    let mut rebound = 0.0_f32;
    let mut pos_rebound = 0.0_f32;
    for _ in 0..30 {
        let prev_z = body_get_position(&w, body).z as f32;
        world_step(&mut w, 1. / 60., 4);
        let next_z = body_get_position(&w, body).z as f32;
        rebound = rebound.max(body_get_linear_velocity(&w, body).z);
        pos_rebound = pos_rebound.max((next_z - prev_z) * 60.0);
        for c in shape_get_contact_data(&w, shape, 1) {
            let embedded = c.manifolds.iter().filter(|m| m.points[..m.point_count as usize]
                .iter().any(|p| (0.15..=0.35).contains(&-p.separation))).count();
            embedded_manifolds = embedded_manifolds.max(embedded);
        }
    }
    destroy_world(w);

    // Release two independent bodies from rest with the smaller one embedded
    // 0.20 m in the wall and 0.15 m in the larger body.
    let mut w = with_wall();
    let mut embedded_box = |z: f32, half_z: f32, mass: f32| {
        let body = create_body(&mut w, &BodyDef {
            body_type: BodyType::Dynamic, position: pos(0., 0., z), ..default_body_def()
        });
        let shape = create_hull_shape(&mut w, body, &ShapeDef {
            density: mass / (2. * half_z), base_material: material(300_000., 0.),
            ..default_shape_def()
        }, &make_box_hull(0.5, 0.5, half_z));
        assert!((body_get_mass(&w, body) - mass).abs() < 0.01);
        (body, shape)
    };
    let (parent, _) = embedded_box(0.95, 0.7, 700.);
    let (piece, piece_shape) = embedded_box(0.10, 0.3, 50.);
    let mut contacts = 0;
    let (mut piece_rebound, mut parent_rebound) = (0.0_f32, 0.0_f32);
    for _ in 0..30 {
        world_step(&mut w, 1. / 60., 4);
        contacts = contacts.max(shape_get_contact_data(&w, piece_shape, 2).len());
        piece_rebound = piece_rebound.max(body_get_linear_velocity(&w, piece).z);
        parent_rebound = parent_rebound.max(body_get_linear_velocity(&w, parent).z);
    }
    destroy_world(w);
    assert!(embedded_manifolds >= 2, "multiple manifolds must embed 0.15–0.35 m: {embedded_manifolds}");
    assert_eq!(contacts, 2, "the released piece must contact both the wall and the parent");
    assert!(rebound <= 0.5 && pos_rebound <= 0.5 && piece_rebound <= 0.5 && parent_rebound <= 0.5,
        "rebound in m/s: pitched vel {rebound}, pitched pos {pos_rebound}, released {piece_rebound}, parent {parent_rebound}");
}

// The floor body's origin is at y = -0.5, so both forms have their top face at zero.
fn floor_shape(world: &mut physics_world::World, floor: BodyId, ground: Ground, mat: SurfaceMaterial) -> ShapeId {
    let def = ShapeDef { base_material: mat, ..default_shape_def() };
    match ground {
        Ground::Hull => create_hull_shape(world, floor, &def, &make_box_hull(2.0, 0.5, 2.0)),
        Ground::BoxSet => create_box_set_shape(world, floor, &def, &create_box_set(&[SetBox {
            bounds: AABB { lower_bound: vec3(-2.0, -0.5, -2.0), upper_bound: vec3(2.0, 0.5, 2.0) },
            material: 0,
            user_data: 0,
        }])),
    }
}

fn floor_body(world: &mut physics_world::World) -> BodyId {
    create_body(world, &BodyDef { position: pos(0.0, -0.5, 0.0), ..default_body_def() })
}

fn unit_cube(world: &mut physics_world::World, y: f32, speed: f32) -> BodyId {
    let body = create_body(world, &BodyDef {
        body_type: BodyType::Dynamic,
        position: pos(0.0, y, 0.0),
        linear_velocity: vec3(0.0, speed, 0.0),
        enable_contact_recycling: false,
        ..default_body_def()
    });
    create_hull_shape(world, body, &default_shape_def(), &make_box_hull(0.5, 0.5, 0.5));
    // Pure translation makes the change in vertical speed equal to the normal impulse.
    body_set_mass_data(world, body, MassData { mass: 1.0, ..Default::default() });
    body
}

fn cube_shape(world: &physics_world::World, cube: BodyId) -> ShapeId {
    let mut shapes = [NULL_SHAPE_ID];
    assert_eq!(body_get_shapes(world, cube, &mut shapes), 1);
    shapes[0]
}

fn normal_load(m: &Manifold) -> f64 {
    m.points[..m.point_count as usize].iter().map(|p| f64::from(p.normal_impulse)).sum()
}

#[test]
fn yielding_hollow_box_set_against_hull_and_mesh_drops_buried_side_flips() {
    let run = |mesh_wall, workers| {
        let mut world = impact_world(workers);
        let (thickness, angle, ideal, start) = if mesh_wall {
            (0.25, 7.0_f32.to_radians(), 0.20, 0.105)
        } else {
            (0.125, 0.005, 0.50, 0.0)
        };
        let fixed = create_body(&mut world, &default_body_def());
        if mesh_wall {
            let sheet = mesh::create_mesh(&MeshDef {
                vertices: &[vec3(-8.0, 0.0, -8.0), vec3(0.0, 0.0, 8.0), vec3(8.0, 0.0, -8.0)],
                indices: &[0, 1, 2], ..Default::default()
            }, None).unwrap();
            create_mesh_shape(&mut world, fixed, &default_shape_def(), &sheet, Vec3::ONE);
        } else {
            create_hull_shape(&mut world, fixed, &default_shape_def(), &make_box_hull(6.0, 3.0, 6.0));
            body_set_transform(&mut world, fixed, pos(0.0, -3.0, 0.0), Quat::IDENTITY);
        }
        let child = |lower_bound, upper_bound| SetBox {
            bounds: AABB { lower_bound, upper_bound }, material: 0, user_data: 0,
        };
        // The tilt brings the leading rim into the sheet while the plate's centre
        // stays on the front side of the triangle, even at the stopping depth.
        let width = 1.5;
        let rim = 0.125;
        let set = create_box_set(&[
            child(vec3(-width, 0.0, -1.0), vec3(width, thickness, 1.0)),
            child(vec3(-width, thickness, -1.0), vec3(-width + rim, 1.0, 1.0)),
            child(vec3(width - rim, thickness, -1.0), vec3(width, 1.0, 1.0)),
            child(vec3(-width + rim, thickness, -1.0), vec3(width - rim, 1.0, -1.0 + rim)),
            child(vec3(-width + rim, thickness, 1.0 - rim), vec3(width - rim, 1.0, 1.0)),
        ]);
        let body = create_body(&mut world, &BodyDef {
            body_type: BodyType::Dynamic,
            position: pos(0.0, start as _, 0.0),
            rotation: make_quat_from_axis_angle(Vec3::AXIS_Z, angle),
            enable_contact_recycling: false,
            ..default_body_def()
        });
        let force = 0.5 * MASS * SPEED * SPEED / ideal;
        let shape = create_box_set_shape(&mut world, body, &ShapeDef {
            base_material: material(force, 0.0), ..default_shape_def()
        }, &set);
        // Hold the tilt fixed so translation measures work against the normal force.
        body_set_mass_data(&mut world, body, MassData { mass: MASS, ..Default::default() });
        body_set_linear_velocity(&mut world, body, vec3(0.0, -SPEED, 0.0));
        let mut depth = 0.0_f32;
        for _ in 0..64 {
            world_step(&mut world, H, 1);
            for c in shape_get_contact_data(&world, shape, 1) {
                for m in c.manifolds {
                    assert!(m.normal.y.abs() >= std::f32::consts::FRAC_1_SQRT_2,
                        "mesh={mesh_wall}: buried rim supplied normal {:?}", m.normal);
                }
            }
            depth = depth.max(start - body_get_position(&world, body).y as f32);
        }
        assert!(depth - start + width * angle.sin() > thickness * angle.cos(),
            "mesh={mesh_wall}: the leading buried rim must reach the surface");
        assert!(depth <= ideal + SPEED * H, "mesh={mesh_wall}: stopped at {depth}, energy bound {ideal}");
        let speed = body_get_linear_velocity(&world, body).y;
        assert!(speed.abs() < 0.05, "mesh={mesh_wall}: still moving at {speed}");
        destroy_world(world);
        (depth.to_bits(), speed.to_bits())
    };
    for mesh_wall in [false, true] {
        assert_eq!(run(mesh_wall, 1), run(mesh_wall, 4));
    }
}

#[test]
fn yield_water_filling_gives_active_manifold_full_budget_beside_unloaded_and_lightly_loaded() {
    let run = |workers, ground, gravity: f32| {
        let mut world = impact_world(workers);
        physics_world::world_set_gravity(&mut world, vec3(0.0, -gravity, 0.0));
        let fixed = floor_body(&mut world);
        let floor = floor_shape(&mut world, fixed, ground, material(0.0, 0.0));
        let wall = create_hull_shape(&mut world, fixed, &default_shape_def(),
            &hull::make_offset_box_hull(1.0, 2.0, 2.0, vec3(1.5, 0.5, 0.0)));
        let side = create_hull_shape(&mut world, fixed, &default_shape_def(),
            &hull::make_offset_box_hull(2.0, 2.0, 1.0, vec3(0.0, 0.5, 1.502)));
        let cube = unit_cube(&mut world, 0.5, 0.0);
        let face = cube_shape(&world, cube);
        shape_set_surface_material(&mut world, face, material(1.0 / H, 0.0));
        body_set_linear_velocity(&mut world, cube, vec3(16.0, 0.0, 0.0));
        let mut trace = Vec::new();
        for _ in 0..8 {
            world_step(&mut world, H, 1);
            let [support, impact, speculative] = [floor, wall, side].map(|shape| {
                let data = shape_get_contact_data(&world, shape, 1);
                assert_eq!(data.len(), 1);
                assert_eq!(data[0].manifolds.len(), 1);
                normal_load(&data[0].manifolds[0])
            });
            let weight = f64::from(gravity * H);
            assert_eq!(speculative, 0.0, "the separated face must take no impulse");
            assert!((support - weight).abs() < 1e-6, "floor took {support} for a load of {weight}");
            assert!(support + impact + speculative <= 1.0, "the group exceeded its substep budget");
            assert!(impact >= 1.0 - weight - 1e-6, "the impact received only {impact}");
            let pos = body_get_position(&world, cube);
            assert!((pos.y - 0.5).abs() < 0.0001, "the light support sagged to {}", pos.y);
            trace.push((support.to_bits(), impact.to_bits(), pos.x.to_bits(), pos.y.to_bits()));
        }
        destroy_world(world);
        trace
    };
    for ground in [Ground::Hull, Ground::BoxSet] {
        for gravity in [0.0_f32, 8.0] {
            let trace = run(1, ground, gravity);
            assert_eq!(trace, run(4, ground, gravity), "worker count changed the impact");
            println!("support trace {gravity} {trace:?}");
        }
    }
}

#[test]
fn four_substeps_share_one_force_across_shapes_and_manifolds() {
    // Replaying the same boundary with 1..=4 substeps and fixed h exposes each substep's
    // stored impulses and net momentum without changing solver timing or using iterate totals.
    let run = |workers, prefix| {
        let mut world = impact_world(workers);
        let floor = floor_body(&mut world);
        let feet = [(-0.5, -0.1), (0.1, 0.5)].map(|(lo, hi)| SetBox {
            bounds: AABB { lower_bound: vec3(lo, -0.5, -1.0), upper_bound: vec3(hi, 0.5, 1.0) },
            material: u32::from(lo > 0.0),
            user_data: 0,
        });
        let rails = create_box_set_shape(&mut world, floor, &ShapeDef {
            materials: vec![material(0.0, 0.0); 2],
            ..default_shape_def()
        }, &create_box_set(&feet));
        let cube = unit_cube(&mut world, 0.5, -16.0);
        let face = cube_shape(&world, cube);
        shape_set_surface_material(&mut world, face, material(256.0, 0.0));
        world_step(&mut world, 4.0 * H, 4);
        let rails_data = shape_get_contact_data(&world, rails, 1);
        assert_eq!(rails_data[0].manifolds.len(), 2, "the two material patches must be separate manifolds");
        for m in rails_data[0].manifolds {
            assert_eq!(normal_load(m), 0.5, "both patches must share the cube material force");
        }

        // Lower F, and add a convex shape to the same static body. Its manifold goes wide.
        shape_set_surface_material(&mut world, face, material(128.0, 0.0));
        let slab = floor_shape(&mut world, floor, Ground::Hull, material(0.0, 0.0));
        let before = body_get_linear_velocity(&world, cube).y;
        world_step(&mut world, prefix as f32 * H, prefix);
        let mut contacts = Vec::new();
        body_get_contact_data(&world, cube, &mut contacts, 4);
        assert_eq!(contacts.len(), 2, "both shape contacts must participate");
        let loads: Vec<_> = contacts.iter().flat_map(|c| c.manifolds).map(normal_load).collect();
        assert_eq!(loads.len(), 3, "the convex face and both set patches must participate");
        let budget = 128.0 * f64::from(H);
        for &load in &loads {
            assert!(load <= budget / 3.0 && load > 0.999 * budget / 3.0, "manifold share {load} at substep {prefix}");
        }
        let total: f64 = loads.iter().sum();
        assert!(total <= budget, "substep {prefix} carried {total} against {budget}");
        let change = body_get_linear_velocity(&world, cube).y - before;
        if prefix == 4 {
            // A rigid patch on the same contact is a backstop and takes no share of F.
            shape_set_surface_material(&mut world, face, material(0.0, 0.0));
            shape_set_mesh_material(&mut world, rails, material(64.0, 0.0), 0);
            shape_set_surface_material(&mut world, slab, material(64.0, 0.0));
            body_set_transform(&mut world, cube, pos(0.0, 0.5, 0.0), Quat::IDENTITY);
            body_set_linear_velocity(&mut world, cube, vec3(0.0, -16.0, 0.0));
            world_step(&mut world, H, 1);
            let mut contacts = Vec::new();
            body_get_contact_data(&world, cube, &mut contacts, 4);
            let mut mixed: Vec<_> = contacts.iter().flat_map(|c| c.manifolds).map(normal_load).collect();
            mixed.sort_by(f64::total_cmp);
            assert_eq!(mixed, [0.25, 0.25, 15.5], "both yielding slots must supply their own Fh beside the rigid backstop");
        }
        destroy_world(world);
        (change, loads)
    };
    let mut previous = 0.0;
    for prefix in 1..=4 {
        let (change, loads) = run(1, prefix);
        assert_eq!(run(4, prefix), (change, loads.clone()), "workers changed substep {prefix}");
        let net = f64::from(change - previous);
        assert!((net - loads.iter().sum::<f64>()).abs() < 4e-6, "momentum and contact loads differ at {prefix}");
        previous = change;
    }
}

#[test]
fn shrinking_a_warm_share_cannot_kick_a_touching_body() {
    for ground in [Ground::Hull, Ground::BoxSet] {
        for grow_count in [false, true] {
            let mut world = impact_world(1);
            let floor = floor_body(&mut world);
            let old = floor_shape(&mut world, floor, ground, material(0.0, 0.0));
            let cube = unit_cube(&mut world, 0.5, -8.0);
            let face = cube_shape(&world, cube);
            shape_set_surface_material(&mut world, face, material(4.0 / H, 0.0));
            body_enable_contact_recycling(&mut world, cube, grow_count);
            let mass = body_get_mass_data(&world, cube);
            // Allow rotation on the loading step so several points acquire different impulses.
            body_set_mass_data(&mut world, cube, MassData { inertia: mul_sm(0.125, Matrix3::IDENTITY), ..mass });
            world_step(&mut world, H, 1);
            body_set_mass_data(&mut world, cube, mass);
            body_set_angular_velocity(&mut world, cube, Vec3::ZERO);
            body_set_transform(&mut world, cube, pos(0.0, 0.5, 0.0), Quat::IDENTITY);
            body_set_linear_velocity(&mut world, cube, vec3(0.0, -3.0, 0.0));
            if grow_count {
                floor_shape(&mut world, floor, ground, material(0.0, 0.0));
            } else {
                shape_set_surface_material(&mut world, face, material(2.0 / H, 0.0));
            }
            let witness = unit_cube(&mut world, 1.5, 0.0);
            world_step(&mut world, 0.0, 1);
            let data = shape_get_contact_data(&world, old, 1);
            let m = &data[0].manifolds[0];
            assert!((3.99999..=4.0).contains(&normal_load(m)), "the floor must still cache its old impulse");
            assert!(m.points[..m.point_count as usize].iter().filter(|p| p.normal_impulse > 0.0).count() > 1,
                "the new allowance must be shared across multiple cached points");

            // Warm starting with 4 would send the cube upwards at 1 m/s. The dynamic/dynamic
            // constraint runs before the floor can retract that impulse in the normal solve.
            world_step(&mut world, H, 1);
            assert_eq!(body_get_position(&world, witness).y, 1.5, "an oversized warm start displaced the witness");
            let data = shape_get_contact_data(&world, old, 1);
            assert!(normal_load(&data[0].manifolds[0]) <= 2.0, "the old manifold must fit its new share");
            destroy_world(world);
        }
    }
}

#[test]
fn restitution_uses_only_the_remaining_impulse() {
    use makepad_box3d::constraint_graph::OVERFLOW_INDEX;
    for (ground, overflow) in [(Ground::Hull, false), (Ground::BoxSet, false), (Ground::Hull, true)] {
        let impact = |force, restitution| {
            let mut world = impact_world(1);
            let floor = floor_body(&mut world);
            floor_shape(&mut world, floor, ground, material(0.0, restitution));
            let cube = unit_cube(&mut world, 0.5, -8.0);
            let face = cube_shape(&world, cube);
            shape_set_surface_material(&mut world, face, material(force, 0.0));

            // This rigid pair shares the static body and, on the convex path, the same SIMD row.
            let rigid = unit_cube(&mut world, 0.5, -8.0);
            body_set_transform(&mut world, rigid, pos(1.25, 0.5, 0.0), Quat::IDENTITY);
            if overflow {
                // Filter joints occupy graph colors without applying any impulses.
                let mut joint = joint::default_filter_joint_def();
                joint.base.body_id_a = floor;
                joint.base.body_id_b = cube;
                joint.base.collide_connected = true;
                for _ in 1..OVERFLOW_INDEX {
                    joint::create_filter_joint(&mut world, &joint);
                }
            }
            world_step(&mut world, H, 1);
            assert_eq!(body_get_linear_velocity(&world, rigid).y, 8.0 * restitution, "rigid neighbor changed");
            let contact = shape_get_contact_data(&world, face, 1);
            let id = contact[0].contact_id.index1 - 1;
            assert_eq!(world.contacts[id as usize].color_index == OVERFLOW_INDEX, overflow, "solver routing");
            let result = (body_get_linear_velocity(&world, cube).y, normal_load(&contact[0].manifolds[0]));
            destroy_world(world);
            result
        };
        assert_eq!(impact(10.0 / H, 0.0), (0.0, 8.0), "the inelastic impact must leave 2 N s unspent");
        assert_eq!(impact(10.0 / H, 1.0), (2.0, 10.0), "only the remaining 2 N s may produce rebound");
        let small = 1.0 / 256.0;
        assert_eq!(impact((8.0 + small) / H, 1.0), (small, 8.0 + f64::from(small)), "even a near-full share can rebound");
        assert_eq!(impact(4.0 / H, 1.0), (-4.0, 4.0), "a saturated manifold must give without bouncing");
        assert_eq!(impact(f32::from_bits(1), 1.0), (-8.0, 0.0), "a subnormal force cannot act as rigid");
        assert_eq!(impact(0.0, 1.0), (8.0, 16.0), "rigid restitution must return the exact incoming speed");
    }
}

#[test]
fn two_material_slots_share_their_load_across_opposing_shapes() {
    let run = |workers, prefix| {
        let mut world = impact_world(workers);
        let floor = floor_body(&mut world);
        let first = floor_shape(&mut world, floor, Ground::Hull, material(0.0, 0.0));
        let body = create_body(&mut world, &BodyDef {
            body_type: BodyType::Dynamic,
            linear_velocity: vec3(0.0, -32.0, 0.0),
            ..default_body_def()
        });
        let feet = [(-1.0, -0.5), (0.5, 1.0)].map(|(lo, hi)| SetBox {
            bounds: AABB { lower_bound: vec3(lo, 0.0, -0.5), upper_bound: vec3(hi, 0.5, 0.5) },
            material: u32::from(lo > 0.0),
            user_data: 0,
        });
        let set = create_box_set_shape(&mut world, body, &ShapeDef {
            materials: vec![material(0.0, 0.0); 2],
            ..default_shape_def()
        }, &create_box_set(&feet));
        body_set_mass_data(&mut world, body, MassData { mass: 1.0, ..Default::default() });
        let second = floor_shape(&mut world, floor, Ground::Hull, material(0.0, 0.0));

        // Identify each load by the supporting hull and the box carrying the material slot.
        let loads = |world: &physics_world::World| {
            let contacts = shape_get_contact_data(world, set, 3);
            assert_eq!(contacts.len(), 2, "both static shapes must touch the set");
            let mut rows = [[None; 2]; 2];
            for c in contacts {
                assert_eq!(c.shape_id_a, set);
                let floor_index = usize::from(c.shape_id_b == second);
                assert_eq!(c.shape_id_b, [first, second][floor_index]);
                assert_eq!(c.manifolds.len(), 2, "each hull must touch both slots");
                for (i, m) in c.manifolds.iter().enumerate() {
                    let (foot, _) = contact::contact_point_children(world, c.contact_id, i as i32, 0).unwrap();
                    rows[floor_index][foot as usize] = Some(normal_load(m));
                }
            }
            rows.map(|row| row.map(Option::unwrap))
        };
        // Cache rigid patches, then enable yielding on their original slots at the same pose.
        world_step(&mut world, 0.0, 1);
        shape_set_mesh_material(&mut world, set, material(256.0, 0.0), 0);
        shape_set_mesh_material(&mut world, set, material(512.0, 0.0), 1);
        world_step(&mut world, prefix as f32 * H, prefix);
        assert_eq!(physics_world::world_get_counters(&world).recycled_contact_count, 2, "both contacts must reuse their patches");
        assert_eq!(loads(&world), [[0.5, 1.0]; 2], "the slots must spend 1 and 2 N s at substep {prefix}");
        let speed = body_get_linear_velocity(&world, body).y;

        // The older hull wins both tied patches. Against the newer hull, each set slot wins one.
        body_set_transform(&mut world, body, pos(0.0, 0.0, 0.0), Quat::IDENTITY);
        body_set_linear_velocity(&mut world, body, vec3(0.0, -32.0, 0.0));
        shape_set_mesh_material(&mut world, set, material(256.0, 0.0), 1);
        shape_set_surface_material(&mut world, first, material(256.0, 0.0));
        shape_set_surface_material(&mut world, second, material(256.0, 0.0));
        world_step(&mut world, H, 1);
        assert_eq!(loads(&world), [[0.5, 0.5], [1.0, 1.0]], "ties must select the lower shape id");
        destroy_world(world);
        speed
    };
    for prefix in 1..=4 {
        for workers in [1, 4] {
            assert_eq!(run(workers, prefix), -32.0 + 3.0 * prefix as f32, "net impulse at substep {prefix}");
        }
    }
}

#[test]
fn mesh_triangle_forces_without_a_slot_zero_match_share_the_weakest_budget() {
    let mut world = impact_world(1);
    let floor = create_body(&mut world, &default_body_def());
    let centers = [-2.0, 0.0, 2.0];
    let triangles = centers.map(|x| [vec3(x - 0.9, 0.0, -0.9), vec3(x, 0.0, 0.9), vec3(x + 0.9, 0.0, -0.9)]);
    let surface = mesh::create_mesh(&MeshDef {
        vertices: triangles.as_flattened(),
        indices: &[0, 1, 2, 3, 4, 5, 6, 7, 8],
        material_indices: &[0, 1, 2],
        ..Default::default()
    }, None).unwrap();
    create_mesh_shape(&mut world, floor, &ShapeDef {
        materials: vec![material(768.0, 0.0), material(128.0, 0.0), material(384.0, 0.0)],
        ..default_shape_def()
    }, &surface, Vec3::ONE);
    let body = create_body(&mut world, &BodyDef {
        body_type: BodyType::Dynamic,
        linear_velocity: vec3(0.0, -16.0, 0.0),
        ..default_body_def()
    });
    let spheres = centers.map(|x| create_sphere_shape(&mut world, body, &default_shape_def(), &Sphere {
        center: vec3(x, 0.5, 0.0), radius: 0.5,
    }));
    body_set_mass_data(&mut world, body, MassData { mass: 1.0, ..Default::default() });
    world_step(&mut world, H, 1);
    let loads = spheres.map(|id| {
        let data = shape_get_contact_data(&world, id, 2);
        assert_eq!(data.len(), 1, "each sphere must touch its own triangle on the same mesh");
        data[0].manifolds.iter().map(normal_load).sum::<f64>()
    });
    assert_eq!(loads, [3.0, 0.25, 0.25], "material zero is independent of the two fallback contacts");
    destroy_world(world);
}

#[test]
fn touching_box_set_feet_follow_their_material_slots() {
    let base = SurfaceMaterial { friction: 0.25, ..material(8.0, 0.0) };
    for grow in [false, true] {
        let mut world = impact_world(1);
        let floor = floor_body(&mut world);
        let ground = floor_shape(&mut world, floor, Ground::Hull, SurfaceMaterial { friction: 1.0, ..material(0.0, 0.0) });
        let body = create_body(&mut world, &BodyDef { body_type: BodyType::Dynamic, ..default_body_def() });
        let mut feet = [(-1.0, -0.5), (0.5, 1.0)].map(|(lo, hi)| SetBox {
            bounds: AABB { lower_bound: vec3(lo, 0.0, -0.5), upper_bound: vec3(hi, 0.5, 0.5) },
            material: u32::from(lo > 0.0 && !grow),
            user_data: 0,
        });
        let set = create_box_set_shape(&mut world, body, &ShapeDef {
            base_material: base,
            materials: if grow { vec![] } else { vec![base; 2] },
            ..default_shape_def()
        }, &create_box_set(&feet));
        body_set_mass_data(&mut world, body, MassData { mass: 1.0, ..Default::default() });
        world_step(&mut world, H, 1);
        if grow {
            let new_slot = shape_append_mesh_material(&mut world, set, base);
            assert_eq!(new_slot, 1, "growing an inline table must preserve slot zero");
            feet[1].material = new_slot as u32;
            let mut added = Vec::new();
            shape_edit_box_set(&mut world, set, &[1], &feet[1..], &mut added);
            assert_eq!(added, [1], "the re-indexed foot must occupy the same child slot");
            world_step(&mut world, H, 1);
        }
        world_step(&mut world, H, 1);
        assert_eq!(physics_world::world_get_counters(&world).recycled_contact_count, 1, "resting feet must recycle");
        // Slide and push hard enough to saturate both normal and tangential
        // constraints. Read the impulses by child, independent of solver order.
        let push = |world: &mut physics_world::World| {
            body_set_linear_velocity(world, body, vec3(1.0, -0.25, 0.0));
            world_step(world, H, 1);
            let data = shape_get_contact_data(world, set, 1);
            let c = &data[0];
            assert_eq!(c.manifolds.len(), 2);
            let mut loads = [None; 2];
            for (i, m) in c.manifolds.iter().enumerate() {
                let (foot, _) = contact::contact_point_children(world, c.contact_id, i as i32, 0).unwrap();
                loads[foot as usize] = Some((normal_load(m) as f32, length(m.friction_impulse)));
            }
            loads.map(Option::unwrap)
        };
        let unchanged = (8.0 * H, 4.0 * H);
        assert_eq!(push(&mut world), [unchanged; 2], "both original caps and mixed frictions must act");
        let softer = SurfaceMaterial { yield_force: 2.0, ..base };
        shape_set_mesh_material(&mut world, set, softer, 1);
        assert_eq!(push(&mut world), [unchanged, (2.0 * H, H)], "the force write must affect just the right foot");
        shape_set_mesh_material(&mut world, set, SurfaceMaterial { friction: 0.0, ..softer }, 1);
        assert_eq!(push(&mut world), [unchanged, (2.0 * H, 0.0)], "the friction write must affect just the right foot");
        shape_set_friction(&mut world, ground, 0.25);
        shape_set_restitution(&mut world, ground, 0.5);
        assert_eq!(push(&mut world), [(8.0 * H, 2.0 * H), (2.0 * H, 0.0)], "the floor's friction must re-mix");
        let c = shape_get_contact_data(&world, ground, 1)[0].contact_id;
        assert_eq!(world.contacts[(c.index1 - 1) as usize].restitution, 0.5, "the floor's restitution must re-mix");
        destroy_world(world);
    }
}

#[test]
fn material_growth_and_writes_replay_and_restore_while_touching() {
    // Triangle and child index one initially fall back to the inline material.
    let mesh = mesh::create_mesh(&MeshDef {
        vertices: &[vec3(-2.0, 0.0, -1.0), vec3(0.0, 0.0, 2.0), vec3(2.0, 0.0, -1.0)],
        indices: &[0, 1, 2],
        material_indices: &[1],
        ..Default::default()
    }, None).unwrap();
    for use_set in [false, true] {
        let mut world = impact_world(1);
        let floor = create_body(&mut world, &default_body_def());
        let ground = create_mesh_shape(&mut world, floor, &default_shape_def(), &mesh, Vec3::ONE);
        let body = create_body(&mut world, &BodyDef { body_type: BodyType::Dynamic, ..default_body_def() });
        let face = if use_set {
            create_box_set_shape(&mut world, body, &default_shape_def(), &create_box_set(&[SetBox {
                bounds: AABB { lower_bound: vec3(-0.25, 0.0, -0.25), upper_bound: vec3(0.25, 0.5, 0.25) },
                material: 1,
                user_data: 0,
            }]))
        } else {
            create_sphere_shape(&mut world, body, &default_shape_def(), &Sphere { center: vec3(0.0, 0.5, 0.0), radius: 0.5 })
        };
        body_set_mass_data(&mut world, body, MassData { mass: 1.0, ..Default::default() });
        world_step(&mut world, H, 1);
        world_step(&mut world, H, 1);
        assert_eq!(physics_world::world_get_counters(&world).recycled_contact_count, 1);

        body_set_awake(&mut world, body, false);
        assert!(!body_is_awake(&world, body), "exercise a static surface supporting a sleeper");
        recording::world_start_recording(&mut world);
        let original = shape_get_mesh_surface_material(&world, ground, 0);
        shape_set_mesh_material(&mut world, ground, original, 0);
        shape_set_friction(&mut world, ground, original.friction);
        shape_set_restitution(&mut world, ground, original.restitution);
        assert!(!body_is_awake(&world, body), "identical material writes must leave the body asleep");
        world_step(&mut world, H, 1);
        assert!(!body_is_awake(&world, body), "the unchanged support must let the body stay asleep");

        // Sleeping contacts skip collision processing. Wake it to observe recycling on its first active step.
        body_set_awake(&mut world, body, true);
        world_step(&mut world, H, 1);
        assert_eq!(physics_world::world_get_counters(&world).recycled_contact_count, 1, "identical writes must preserve the cached contact");
        body_set_awake(&mut world, body, false);
        shape_set_mesh_material(&mut world, ground, SurfaceMaterial { restitution: -0.0, ..original }, 0);
        assert!(body_is_awake(&world, body), "changing even the sign of zero must wake the body");
        assert_eq!(shape_get_restitution(&world, ground).to_bits(), (-0.0f32).to_bits());

        body_set_awake(&mut world, body, false);
        let slot = shape_append_mesh_material(&mut world, ground, material(16.0, 0.0));
        assert_eq!(slot, 1);
        assert!(body_is_awake(&world, body), "a change in support must wake its contact");
        body_set_linear_velocity(&mut world, body, vec3(1.0, -1.0, 0.0));
        world_step(&mut world, H, 1);
        assert_eq!(body_get_linear_velocity(&world, body).y, -1.0 + 16.0 * H, "growth alone must enable the new force");
        if use_set {
            let foot = SurfaceMaterial { friction: 1.0, ..material(8.0, 0.0) };
            assert_eq!(shape_append_mesh_material(&mut world, face, foot), 1);
            world_step(&mut world, H, 1);
            assert_eq!(body_get_linear_velocity(&world, body).y, -1.0 + 24.0 * H, "a grown box slot must cap its patch");
        }
        let speed = body_get_linear_velocity(&world, body).y;
        let changed = SurfaceMaterial { friction: 0.25, ..material(4.0, 0.0) };
        shape_set_mesh_material(&mut world, ground, changed, slot);

        let mut snap = recording::RecBuffer::new();
        let mut registry = recording::Recording::new();
        world_snapshot::serialize_world(&world, &mut snap, &mut registry);
        recording::write_registry(&mut registry);
        let reader = recording_replay::load_registry(registry.data()).unwrap();
        let mut restored = impact_world(1);
        assert!(world_snapshot::deserialize_into_shell(&snap.data, &mut restored, &reader));
        assert_eq!(shape_get_mesh_material_count(&restored, ground), 2);
        assert_eq!(shape_get_mesh_surface_material(&restored, ground, slot), changed);
        for step in 1..=3 {
            world_step(&mut world, H, 1);
            world_step(&mut restored, H, 1);
            assert_eq!(body_get_linear_velocity(&world, body).y, speed + (4 * step) as f32 * H);
            assert!(body_get_linear_velocity(&restored, body).x < 1.0, "the pending friction write must survive restore");
            assert_eq!(recording::hash_world_state(&restored), recording::hash_world_state(&world),
                "restored body state diverged on step {step}");
        }
        let capture = recording::world_stop_recording(&mut world).unwrap();
        assert!(recording_replay::validate_replay(capture.data(), 1), "all recorded body-state hashes must match");
        destroy_world(restored);
        destroy_world(world);
    }
}
