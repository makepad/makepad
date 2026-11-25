use makepad_box3d::body::*;
use makepad_box3d::physics_world::{create_world, destroy_world, world_cast_ray_closest, world_step};
use makepad_box3d::recording::{world_start_recording, world_stop_recording};
use makepad_box3d::shape::*;
use makepad_box3d::*;
use std::sync::Arc;

fn child(lo: Vec3, hi: Vec3, material: u32) -> SetBox {
    SetBox { bounds: AABB { lower_bound: lo, upper_bound: hi }, material, user_data: material as u64 }
}

#[test]
fn capsule_cast_through_an_edited_set() {
    let mut world = create_world(&default_world_def());
    let position = pos(7.0, 3.0, -4.0);
    // This exact rotation takes the wall's local X,Y,Z axes to world Y,Z,X.
    let rotation = Quat { v: vec3(0.5, 0.5, 0.5), s: 0.5 };
    let body = create_body(&mut world, &BodyDef { position, rotation, ..default_body_def() });
    let def = ShapeDef {
        is_sensor: true,
        enable_sensor_events: true,
        materials: (0..3).map(|i| SurfaceMaterial { user_material_id: 40 + i, ..default_surface_material() }).collect(),
        ..default_shape_def()
    };
    let wall = [0, 1, 2].map(|i| child(vec3(i as f32 - 1.5, -1.0, -0.1), vec3(i as f32 - 0.5, 1.0, 0.1), i));
    let shape = create_box_set_shape(&mut world, body, &def, &create_box_set(&wall));
    let points = [vec3(0.0, 0.0, -0.2), vec3(0.0, 0.0, 0.2)];
    let capsule = ShapeProxy { points: &points, radius: 0.15 };
    let cast = |world: &physics_world::World, y, proxy: &ShapeProxy| {
        let mut hit = None;
        let origin = offset_pos(position, vec3(-2.0, y, 0.0));
        physics_world::world_cast_shape(world, origin, proxy, vec3(4.0, 0.0, 0.0), default_query_filter(),
            &mut |id, _, normal, fraction, material, triangle, slot| {
                assert_eq!(id, shape, "the cast must report the parent shape");
                assert!(normal.x < -0.99, "the wall normal must be in the query frame");
                assert_eq!(triangle, NULL_INDEX, "a box face has no triangle index");
                hit = Some((slot, material, fraction));
                fraction
            });
        hit
    };
    let (slot, material, fraction) = cast(&world, 0.0, &capsule).expect("the intact wall must block the capsule");
    assert_eq!((slot, material), (1, 41), "the cast must report the middle slot and its material");
    assert!((0.43..0.45).contains(&fraction), "the fraction must include the radius and the wall thickness");
    let mut added = Vec::new();
    shape_edit_box_set(&mut world, shape, &[1], &[], &mut added);
    assert!(cast(&world, 0.0, &capsule).is_none(), "the whole capsule must pass through the edited hole");
    let (slot, material, _) = cast(&world, 1.0, &capsule).expect("the side of the opening must still block a cast");
    assert_eq!((slot, material), (2, 42), "the surviving box must retain its slot and material");
    let mut cloud = [Vec3::ZERO; 257];
    cloud[256] = Vec3::AXIS_Y;
    assert!(cast(&world, 0.0, &ShapeProxy { points: &cloud, radius: 0.15 }).is_some(),
        "the last support vertex in a large proxy must participate in the cast");

    let overlaps = |world: &physics_world::World, y| {
        let mut ids = Vec::new();
        physics_world::world_overlap_shape(world, offset_pos(position, vec3(0.0, y, 0.0)), &capsule,
            default_query_filter(), &mut |id| { ids.push(id); true });
        ids
    };
    assert!(overlaps(&world, 0.0).is_empty(), "the opening must also be clear for an overlap query");
    assert_eq!(overlaps(&world, 0.55), [shape], "overlap must report the parent at the exposed lip");
    let center = to_vec3(position);
    assert_eq!(shape_get_closest_point(&world, shape, center), add(center, vec3(0.0, -0.5, 0.0)),
        "the nearest point in the hole must lie on the lower-slot lip");

    let other = create_body(&mut world, &BodyDef { position, ..default_body_def() });
    let sensor = create_capsule_shape(&mut world, other, &def, &Capsule { center1: points[0], center2: points[1], radius: 0.15 });
    world_step(&mut world, 1.0 / 60.0, 1);
    for id in [shape, sensor] {
        assert_eq!(shape_get_sensor_capacity(&world, id), 0, "the hole must be empty in both sensor roles");
    }
    shape_edit_box_set(&mut world, shape, &[], &wall[1..2], &mut added);
    world_step(&mut world, 1.0 / 60.0, 1);
    for (id, visitor) in [(shape, sensor), (sensor, shape)] {
        let mut hits = [ShapeId::default(); 2];
        assert_eq!(shape_get_sensor_data(&world, id, &mut hits), 1, "a filled opening must produce one shape overlap");
        assert_eq!(hits[0], visitor, "the sensor must identify the other shape in either role");
    }
    let half = vec3(0.05, 0.75, 0.05);
    shape_set_box_set(&mut world, sensor, &create_box_set(&[child(neg(half), half, 0)]));
    world_step(&mut world, 1.0 / 60.0, 1);
    for id in [shape, sensor] {
        assert_eq!(shape_get_sensor_capacity(&world, id), 1, "three overlapping child pairs must report one visitor");
    }
    shape_edit_box_set(&mut world, shape, &[0, 1, 2], &[], &mut added);
    world_step(&mut world, 1.0 / 60.0, 1);
    for id in [shape, sensor] {
        assert_eq!(shape_get_sensor_capacity(&world, id), 0, "emptying the set must clear both sensor overlaps");
    }
    assert_eq!(shape_get_closest_point(&world, shape, Vec3::ZERO), center, "an empty shape must return the body origin");
    destroy_world(world);
}

#[test]
fn mover_on_a_set_seam_gets_one_exterior_plane() {
    let mut world = create_world(&default_world_def());
    let body = create_body(&mut world, &default_body_def());
    let tiles = [
        child(vec3(-1.0, -0.25, -1.0), vec3(0.0, 0.0, 1.0), 0),
        child(vec3(0.0, -0.25, -1.0), vec3(1.0, 0.0, 1.0), 0),
    ];
    let shape = create_box_set_shape(&mut world, body, &default_shape_def(), &create_box_set(&tiles));
    // The mover overlaps both tops. The second child's rounded edge alone
    // would give it a sideways normal, since its axis is off the seam.
    let mover = Capsule { center1: vec3(-0.05, 0.25, 0.0), center2: vec3(-0.05, 0.75, 0.0), radius: 0.3 };
    let mut planes = Vec::new();
    physics_world::world_collide_mover(&world, POS_ZERO, &mover, default_query_filter(), &mut |id, found| {
        assert_eq!(id, shape, "the mover must receive planes for the supporting shape");
        planes.extend_from_slice(found);
        true
    });
    assert_eq!(planes.len(), 1, "flush supports must contribute one exterior plane");
    let hit = planes[0];
    assert!(distance(hit.plane.normal, Vec3::AXIS_Y) < 1e-4, "the seam must leave only the upward floor normal");
    assert!(hit.point.y.abs() < 1e-4 && (hit.plane.offset - 0.05).abs() < 1e-4, "the plane must retain the floor depth");
    planes.clear();
    let center = vec3(0.0, -0.1, 0.0);
    let embedded = Capsule { center1: center, center2: center, radius: 0.05 };
    physics_world::world_collide_mover(&world, POS_ZERO, &embedded, default_query_filter(), &mut |_, found| {
        planes.extend_from_slice(found);
        true
    });
    assert_eq!(planes.len(), 1, "an embedded start on the seam must have one exit");
    assert_eq!(planes[0].plane.normal, Vec3::AXIS_Y, "the shared vertical face must not be an exit");
    assert!((planes[0].plane.offset - 0.15).abs() < 1e-4, "the exit must clear the surface by the mover radius");
    let slide = |world: &physics_world::World| physics_world::world_cast_mover(
        world, POS_ZERO, &mover, vec3(2.0, 0.0, 0.0), default_query_filter(), None,
    );
    assert_eq!(slide(&world), 1.0, "a horizontal mover cast must pass the internal edge");
    let curb = child(vec3(1.0, 0.0, -1.0), vec3(1.2, 0.8, 1.0), 0);
    shape_edit_box_set(&mut world, shape, &[], &[curb], &mut Vec::new());
    assert!((0.37..0.39).contains(&slide(&world)), "the same cast must stop at the exposed curb");
    let partly_inside = Capsule { center1: vec3(-0.05, -0.1, 0.0), ..mover };
    let fraction = physics_world::world_cast_mover(
        &world, POS_ZERO, &partly_inside, vec3(2.0, 0.0, 0.0), default_query_filter(), None,
    );
    assert!((0.37..0.39).contains(&fraction), "a child overlapping at fraction zero must not hide the curb");

    // Three separated rings give 72 distinct normals, more than the legacy
    // per-shape mover buffer. All planes belong in the same callback.
    let mut children = Vec::new();
    for y in [-0.7_f32, 0.0, 0.7] {
        for i in 0..24 {
            let angle = std::f32::consts::TAU * i as f32 / 24.0;
            let r = (1.0 - y * y).sqrt();
            let p = mul_sv(0.28, vec3(r * angle.cos(), y, r * angle.sin()));
            let h = vec3(0.002, 0.002, 0.002);
            children.push(child(sub(p, h), add(p, h), 0));
        }
    }
    shape_set_box_set(&mut world, shape, &create_box_set(&children));
    let sphere = Capsule { center1: Vec3::ZERO, center2: Vec3::ZERO, radius: 0.3 };
    let mut batches = Vec::new();
    physics_world::world_collide_mover(&world, POS_ZERO, &sphere, default_query_filter(), &mut |_, found| {
        batches.push(found.len());
        true
    });
    assert_eq!(batches, [72], "all distinct planes must reach the caller in one batch");
    destroy_world(world);
}

#[test]
fn off_center_bounds_extent_and_mass() {
    let boxes = [
        child(vec3(9.0, 19.0, 28.0), vec3(11.0, 21.0, 32.0), 3),
        child(vec3(12.0, 25.0, 21.0), vec3(16.0, 27.0, 23.0), 8),
    ];
    let set = create_box_set(&boxes);
    let transform = Transform { p: vec3(1.0, 2.0, 3.0), q: Quat { v: Vec3::AXIS_Z, s: 0.0 } };
    let world_bounds = compute_box_set_aabb(&set, transform).expect("a nonempty set must have bounds");
    assert_eq!(world_bounds.lower_bound, vec3(-15.0, -25.0, 24.0), "rotated lower bound");
    assert_eq!(world_bounds.upper_bound, vec3(-8.0, -17.0, 35.0), "rotated upper bound");

    // Two masses of 12 at offsets ±(2, 3, -4). Central box inertias add to
    // diag(28, 40, 28) before applying the two parallel-axis terms.
    let mass = compute_box_set_mass(&set, 0.75);
    assert_eq!(mass.mass, 24.0, "mass must scale the sum of both volumes by density");
    assert_eq!(mass.center, vec3(12.0, 23.0, 26.0), "equal box masses must average their centers");
    assert_eq!(mass.inertia.cx, vec3(628.0, -144.0, 192.0), "central inertia x column");
    assert_eq!(mass.inertia.cy, vec3(-144.0, 520.0, 288.0), "central inertia y column");
    assert_eq!(mass.inertia.cz, vec3(192.0, 288.0, 340.0), "central inertia z column");
    let extent = compute_box_set_extent(&set, mass.center);
    assert_eq!(extent.min_extent, 3.5, "the fast threshold must use the smallest half-dimension of the whole set");
    assert_eq!(extent.max_extent, vec3(4.0, 4.0, 6.0), "maximum reach must be relative to the center of mass");

    let mut world = create_world(&default_world_def());
    let body = create_body(&mut world, &BodyDef { body_type: BodyType::Dynamic, ..default_body_def() });
    let def = ShapeDef { update_body_mass: false, ..default_shape_def() };
    let shape = create_box_set_shape(&mut world, body, &def, &set);
    let supplied = MassData { center: vec3(8.0, 17.0, 18.0), mass: 37.0, inertia: Matrix3::IDENTITY };
    body_set_mass_data(&mut world, body, supplied);
    let sim = get_body_sim_from_id(&world, body.index1 - 1);
    assert_eq!(sim.min_extent, 3.5, "moving the supplied centre must preserve the set thickness");
    assert_eq!(sim.max_extent, vec3(8.0, 10.0, 14.0), "cached reach must use the supplied centre");
    body_set_awake(&mut world, body, false);
    assert!(!body_is_awake(&world, body), "the owner must be asleep before its edit");
    let cast = |w: &physics_world::World| {
        world_cast_ray_closest(w, pos(14.0, 24.0, 22.0), vec3(0.0, 4.0, 0.0), default_query_filter())
    };
    let hit = cast(&world);
    assert!(hit.hit && hit.child_index == 1 && hit.fraction == 0.25, "the world ray must hit the second child");
    shape_edit_box_set(&mut world, shape, &[1], &[], &mut Vec::new());
    assert!(!cast(&world).hit, "the world ray must miss the removed child");
    assert!(body_is_awake(&world, body), "a removal must wake the set's sleeping owner");
    let sim = get_body_sim_from_id(&world, body.index1 - 1);
    assert_eq!(sim.max_extent, vec3(3.0, 4.0, 14.0), "an edit must refresh reach about the same centre");
    let got = body_get_mass_data(&world, body);
    assert_eq!(
        (got.mass, got.inertia, got.center),
        (supplied.mass, supplied.inertia, supplied.center),
        "refreshing extents must preserve all supplied mass properties"
    );
    destroy_world(world);
}

#[test]
fn ray_ties_and_import_validation() {
    // Input order opposes spatial order. Both boxes include their common face.
    let boxes = [child(vec3(1.0, 0.0, 0.0), vec3(2.0, 1.0, 1.0), 5), child(Vec3::ZERO, Vec3::ONE, 7)];
    let set = create_box_set(&boxes);
    assert_eq!(box_set_validate(&set), Ok(()), "a shared face must be valid");
    assert!(set.boxes().eq([(0, &boxes[0]), (1, &boxes[1])]), "enumeration must preserve the assigned slots");
    assert!(box_set_get(&set, -1).is_none() && box_set_get(&set, 2).is_none(), "out-of-range slots must be absent");
    let face = AABB { lower_bound: vec3(1.0, 0.25, 0.25), upper_bound: vec3(1.0, 0.75, 0.75) };
    let mut seen = Vec::new();
    box_set_query(&set, face, |slot, value| {
        seen.push((slot, value.material));
        true
    });
    seen.sort_unstable();
    assert_eq!(seen, [(0, 5), (1, 7)], "the local query must visit both boxes sharing the selected face");
    let mut count = 0;
    box_set_query(&set, face, |_, _| {
        count += 1;
        false
    });
    assert_eq!(count, 1, "a visitor returning false must stop the query immediately");
    box_set_query(&set, AABB { lower_bound: vec3(3.0, 0.0, 0.0), upper_bound: vec3(4.0, 1.0, 1.0) }, |_, _| {
        panic!("a disjoint query must never call its visitor")
    });

    let ray = RayCastInput { origin: vec3(1.0, 2.0, 0.5), translation: vec3(0.0, -4.0, 0.0), max_fraction: 0.25 };
    let hit = ray_cast_box_set(&set, &ray);
    assert!(hit.hit, "an entry exactly at max_fraction must count");
    assert_eq!(hit.fraction, 0.25, "ray fraction must use the full input translation");
    assert_eq!(hit.point, vec3(1.0, 1.0, 0.5), "ray entry must be on the upper y face");
    assert_eq!(hit.normal, Vec3::AXIS_Y, "entry normal must point out of the solid");
    assert_eq!(
        (hit.child_index, hit.material_index),
        (0, 5),
        "equal entries must choose the lower slot and its material"
    );
    let short = RayCastInput { max_fraction: 0.125, ..ray };
    assert!(!ray_cast_box_set(&set, &short).hit, "a clipped ray must miss");
    let point = RayCastInput { origin: vec3(1.0, 0.5, 0.5), ..Default::default() };
    let inside = ray_cast_box_set(&set, &point);
    assert!(inside.hit && inside.child_index == 0, "a shared-face point query must choose the lower slot");
    assert_eq!((inside.fraction, inside.normal), (0.0, Vec3::ZERO), "initial overlap must have no entry face");

    let lo = vec3(1.0 + f32::EPSILON, 0.0, 0.0);
    let hi = vec3(1.0 + 2.0 * f32::EPSILON, 1.0, 1.0);
    let thin = create_box_set(&[child(lo, hi, 0)]);
    assert_eq!(
        compute_box_set_aabb(&thin, Transform::IDENTITY),
        thin.bounds(),
        "identity must preserve one-ulp bounds"
    );
    let grazing = RayCastInput { origin: vec3(lo.x, 2.0, 0.5), ..ray };
    assert!(ray_cast_box_set(&thin, &grazing).hit, "rounding the box center must not cull a ray on its face");

    let overlap = child(vec3(0.5, 0.0, 0.0), vec3(1.5, 1.0, 1.0), 0);
    let invalid = create_box_set(&[boxes[1], overlap]);
    assert_eq!(
        box_set_validate(&invalid),
        Err(BoxSetError::OverlappingInteriors { child_a: 0, child_b: 1 }),
        "the importer must be able to identify both overlapping slots"
    );
    for hi in [
        vec3(0.0, 1.0, 1.0),
        vec3(-1.0, -1.0, 1.0),
        vec3(f32::NAN, 1.0, 1.0),
        vec3(f32::INFINITY, 1.0, 1.0),
        vec3(f32::MAX, f32::MAX, f32::MAX),
    ] {
        let invalid = create_box_set(&[boxes[0], child(Vec3::ZERO, hi, 0)]);
        assert_eq!(
            box_set_validate(&invalid),
            Err(BoxSetError::InvalidBounds { child: 1 }),
            "zero, inverted, non-finite or overflowing bounds must identify the bad child: {hi:?}"
        );
    }
}

#[test]
fn edits_wake_only_eligible_sleepers_and_keep_one_proxy() {
    let mut world = create_world(&default_world_def());
    let def = default_shape_def();
    let fixed = create_body(&mut world, &BodyDef { position: pos(10.0, 2.0, -3.0), ..default_body_def() });
    let unit = |x| child(vec3(x, 0.0, 0.0), vec3(x + 1.0, 1.0, 1.0), 0);
    let original = create_box_set(&[unit(-6.0), unit(5.0)]);
    let shape = create_box_set_shape(&mut world, fixed, &def, &original);
    let proxy = world.shapes[(shape.index1 - 1) as usize].proxy_key;
    let sleeper =
        BodyDef { body_type: BodyType::Dynamic, position: pos(10.5, 2.0, -2.5), is_awake: false, ..default_body_def() };
    let eligible = create_body(&mut world, &sleeper);
    let filtered = create_body(&mut world, &sleeper);
    let sphere = Sphere { center: Vec3::ZERO, radius: 0.2 };
    let sphere_id = create_sphere_shape(&mut world, eligible, &def, &sphere);
    let mesh_id = create_mesh_shape(&mut world, eligible, &def, &mesh::create_grid_mesh(1, 1, 0.5, 0, true), Vec3::ONE);
    let excluded = ShapeDef { filter: Filter { mask_bits: 0, ..def.filter }, ..def.clone() };
    create_sphere_shape(&mut world, filtered, &excluded, &sphere);
    world_step(&mut world, 0.0, 1);
    assert_eq!(shape_get_contact_capacity(&world, shape), 2, "both unfiltered shapes must have a parent contact");
    assert!(shape_get_contact_data(&world, shape, 2).is_empty(), "both sleeping shapes start in the gap");
    let mut added = Vec::new();
    shape_edit_box_set(&mut world, shape, &[], &[unit(-3.0), unit(3.0)], &mut added);
    assert!(!body_is_awake(&world, eligible), "two disjoint additions must leave the sleeper between them asleep");
    let unique = Arc::as_ptr(shape_get_box_set(&world, shape));
    assert_ne!(unique, Arc::as_ptr(&original), "editing a shared set must detach its geometry");
    shape_edit_box_set(&mut world, shape, &[], &[unit(0.0)], &mut added);
    assert!(body_is_awake(&world, eligible), "growth into a sleeper must wake it before the next step");
    assert!(!body_is_awake(&world, filtered), "the shape filter must also apply to edit wakes");
    assert_eq!(Arc::as_ptr(shape_get_box_set(&world, shape)), unique, "uniquely owned geometry must edit in place");
    assert_eq!(shape_get_box_set(&world, shape).bounds(), original.bounds(), "both edits must keep the outer bounds");
    assert_eq!(world.shapes[(shape.index1 - 1) as usize].proxy_key, proxy, "interior edits must preserve the proxy");
    world_step(&mut world, 0.0, 1);
    let touching = shape_get_contact_data(&world, shape, 2);
    assert_eq!(touching.len(), 2, "interior growth must reach both shapes on the next step");
    for pair in [(mesh_id, shape), (shape, sphere_id)] {
        assert!(
            touching.iter().any(|c| (c.shape_id_a, c.shape_id_b) == pair),
            "each pair must use its registered order"
        );
    }

    assert_eq!(world.task_contexts[0].set_box_visits, 1, "only the sphere's query visits a box for this counter");
    // Rebuild both manifolds at the same pose to exercise the sphere's cached child list.
    let recycle_distance = physics_world::world_get_contact_recycle_distance(&world);
    physics_world::world_set_contact_recycle_distance(&mut world, 0.0);
    world_step(&mut world, 0.0, 1);
    assert_eq!(world.task_contexts[0].set_box_visits, 0, "the next step must count no visits for the cached query");
    physics_world::world_set_contact_recycle_distance(&mut world, recycle_distance);

    let slots: Vec<_> = shape_get_box_set(&world, shape).boxes().map(|(slot, _)| slot).collect();
    shape_edit_box_set(&mut world, shape, &slots, &[], &mut added);
    assert_eq!(shape_get_type(&world, shape), ShapeType::BoxSet, "an empty set must keep its handle");
    assert_eq!(
        world.broad_phase.trees[BodyType::Static as usize].proxy_count,
        0,
        "the final removal must remove the proxy"
    );
    body_disable(&mut world, fixed);
    shape_edit_box_set(&mut world, shape, &[], &[unit(8.0)], &mut added);
    assert_eq!(
        world.broad_phase.trees[BodyType::Static as usize].proxy_count,
        0,
        "a disabled refill must stay unproxied"
    );
    body_enable(&mut world, fixed);
    assert_eq!(
        world.broad_phase.trees[BodyType::Static as usize].proxy_count,
        1,
        "enabling a refilled set must create one proxy"
    );
    shape_set_box_set(&mut world, shape, &create_box_set(&[]));
    shape_edit_box_set(&mut world, shape, &[], &[unit(1.0), unit(2.0)], &mut added);
    assert_eq!(
        world.broad_phase.trees[BodyType::Static as usize].proxy_count,
        1,
        "an enabled refill must create one proxy for both boxes"
    );
    assert_eq!(original.boxes().count(), 2, "the original holder must retain its two boxes");
    destroy_body(&mut world, eligible);
    destroy_body(&mut world, filtered);

    let geometry = shape_get_box_set(&world, shape).clone();
    let dynamic_mass = compute_box_set_mass(&geometry, def.density);
    for kind in [BodyType::Static, BodyType::Dynamic, BodyType::Static] {
        body_set_type(&mut world, fixed, kind);
        assert_eq!(body_get_type(&world, fixed), kind, "the enabled set owner must accept the new type");
        assert_eq!(body_get_shape_count(&world, fixed), 1, "the type change must preserve the shape count");
        assert_eq!(shape_get_body(&world, shape), fixed, "the original shape handle must still name its owner");
        assert!(Arc::ptr_eq(shape_get_box_set(&world, shape), &geometry), "the same set geometry must survive");
        let key = world.shapes[(shape.index1 - 1) as usize].proxy_key;
        assert_ne!(key, NULL_INDEX, "an enabled nonempty set must retain a proxy");
        assert_eq!(broad_phase::proxy_type(key), kind, "the shape proxy must belong to the new tree");
        assert_eq!(
            dynamic_tree::dynamic_tree_get_user_data(
                &world.broad_phase.trees[kind as usize],
                broad_phase::proxy_id(key)
            ),
            (shape.index1 - 1) as u64,
            "the proxy leaf must point back to the original shape"
        );
        for tree in [BodyType::Static, BodyType::Kinematic, BodyType::Dynamic] {
            assert_eq!(
                world.broad_phase.trees[tree as usize].proxy_count,
                i32::from(kind == tree),
                "only the tree for the current body type may contain the proxy"
            );
        }
        let mass = body_get_mass_data(&world, fixed);
        let expected = if kind == BodyType::Dynamic { dynamic_mass } else { MassData::default() };
        assert_eq!(
            (mass.mass, mass.center, mass.inertia),
            (expected.mass, expected.center, expected.inertia),
            "the body type must determine the mass, centre and inertia of the set"
        );
    }

    // A closed mesh gives the moving box six faces in B2's mesh/set pair.
    let half = vec3(0.2, 0.2, 0.2);
    let moving = create_body(
        &mut world,
        &BodyDef { body_type: BodyType::Dynamic, position: pos(11.5, 3.45, -2.5), ..default_body_def() },
    );
    let box_shape =
        create_mesh_shape(&mut world, moving, &def, &mesh::create_box_mesh(Vec3::ZERO, half, true), Vec3::ONE);
    body_set_mass_data(
        &mut world,
        moving,
        MassData { mass: 1.0, center: Vec3::ZERO, inertia: box_inertia(1.0, mul_sv(-1.0, half), half) },
    );
    for _ in 0..120 {
        world_step(&mut world, 1.0 / 60.0, 4);
        if !body_is_awake(&world, moving) {
            break;
        }
    }
    let contacts = shape_get_contact_data(&world, shape, 2);
    assert_eq!(contacts.len(), 1, "the static set must support one dynamic box");
    assert_eq!(
        (contacts[0].shape_id_a, contacts[0].shape_id_b),
        (box_shape, shape),
        "the support contact must connect the moving box and original static set"
    );
    assert!((body_get_position(&world, moving).y - 3.2).abs() < 0.03, "the box must rest on the set's top face");
    assert!(!body_is_awake(&world, moving), "the dynamic box must sleep on the static set");
    destroy_world(world);
}

#[test]
fn recording_refuses_a_set_seen_anywhere_in_the_capture() {
    let mut world = create_world(&default_world_def());
    let body = create_body(&mut world, &default_body_def());
    let def = default_shape_def();
    let set = create_box_set(&[child(Vec3::ZERO, Vec3::ONE, 0)]);
    world_start_recording(&mut world);
    let shape = create_box_set_shape(&mut world, body, &def, &set);
    shape_edit_box_set(&mut world, shape, &[0], &[], &mut Vec::new());
    assert!(shape_get_box_set(&world, shape).bounds().is_none(), "capture must allow the physics edit");
    assert!(world_stop_recording(&mut world).is_none(), "creating a set during capture must invalidate it");

    world_start_recording(&mut world);
    let mut snap = recording::RecBuffer::new();
    let mut registry = recording::Recording::new();
    world_snapshot::serialize_world(&world, &mut snap, &mut registry);
    recording::write_registry(&mut registry);
    let reader = recording_replay::load_registry(registry.data()).expect("the geometry registry must be readable");
    let mut shell = create_world(&default_world_def());
    assert!(
        world_snapshot::deserialize_into_shell(&snap.data, &mut shell, &reader),
        "the independent snapshot must restore an empty live set even during an unsupported capture"
    );
    assert!(shape_get_box_set(&shell, shape).bounds().is_none(), "the restored empty shape must keep its handle");
    destroy_world(shell);
    destroy_shape(&mut world, shape, false);
    assert!(world_stop_recording(&mut world).is_none(), "removing a set from the seed must not rescue the capture");

    let sphere = Sphere { center: Vec3::ZERO, radius: 1.0 };
    let shape = create_sphere_shape(&mut world, body, &def, &sphere);
    world_start_recording(&mut world);
    shape_set_box_set(&mut world, shape, &set);
    shape_set_sphere(&mut world, shape, &sphere);
    assert!(world_stop_recording(&mut world).is_none(), "a transient wholesale replacement must invalidate capture");
    world_start_recording(&mut world);
    assert!(world_stop_recording(&mut world).is_some(), "a later capture with only the sphere must finish normally");
    destroy_world(world);
}

#[test]
fn edited_set_snapshot_continues_bit_for_bit() {
    fn image(world: &physics_world::World) -> (Vec<u8>, Vec<u8>) {
        let mut buf = recording::RecBuffer::new();
        let mut registry = recording::Recording::new();
        world_snapshot::serialize_world(world, &mut buf, &mut registry);
        recording::write_registry(&mut registry);
        (buf.data, registry.buffer.data)
    }

    let rails = [
        child(vec3(-4.0, -0.3, -0.95), vec3(4.0, 0.0, -0.55), 0),
        SetBox { user_data: 0xfedc_ba98_7654_3210, ..child(vec3(-4.0, -0.3, 0.55), vec3(4.0, 0.0, 0.95), 1) },
        child(vec3(5.0, 0.0, 1.0), vec3(5.2, 0.2, 1.2), 0),
        child(vec3(6.0, 0.0, 1.0), vec3(6.2, 0.2, 1.2), 0),
    ];
    let feet = create_box_set(&[-0.6, 0.6].map(|x| child(vec3(x - 0.2, 0.0, -1.2), vec3(x + 0.2, 0.4, 1.2), 0)));
    let def = default_shape_def();
    let floor = mesh::create_grid_mesh(1, 1, 64.0, 0, true);
    for workers in [1, 4] {
        let wd = WorldDef { worker_count: workers, ..default_world_def() };
        let mut original = create_world(&wd);
        let fixed = create_body(&mut original, &default_body_def());
        let mesh = create_mesh_shape(&mut original, fixed, &def, &floor, Vec3::ONE);
        let base = create_body(&mut original, &BodyDef { position: pos(0.0, 0.0, 40.0), ..default_body_def() });
        let rail_def = ShapeDef {
            materials: vec![def.base_material, SurfaceMaterial { friction: 0.25, ..def.base_material }],
            ..default_shape_def()
        };
        let support = create_box_set_shape(&mut original, base, &rail_def, &create_box_set(&rails));
        let mut slots = Vec::new();
        shape_edit_box_set(&mut original, support, &[2, 3, 1], &rails[1..2], &mut slots);
        assert_eq!(slots, [1], "the seed must have a reused rail and two free slots");

        // Nineteen mesh contacts plus set/set and set/sphere span one worker-block boundary.
        let moving = |world: &mut physics_world::World, position| {
            // Refresh the manifolds before the seed so both generations of rows carry features.
            create_body(world, &BodyDef { position, body_type: BodyType::Dynamic, enable_contact_recycling: false, ..default_body_def() })
        };
        for i in -9..=9 {
            let body = moving(&mut original, pos(3.0 * i as f32, 0.0, -2.0));
            create_box_set_shape(&mut original, body, &def, &feet);
        }
        let body = moving(&mut original, pos(0.0, 0.0, 40.0));
        let slider = create_box_set_shape(&mut original, body, &def, &feet);
        let body = moving(&mut original, pos(2.0, 0.3, 40.75));
        let ball = create_sphere_shape(&mut original, body, &def, &Sphere { center: Vec3::ZERO, radius: 0.3 });
        for _ in 0..3 {
            world_step(&mut original, 1.0 / 60.0, 4);
        }
        for shape in [mesh, slider, ball] {
            let contacts = shape_get_contact_data(&original, shape, 21);
            let has_load = contacts.iter().flat_map(|c| c.manifolds.iter()).any(|m| {
                m.points[..m.point_count as usize].iter().any(|p| p.normal_impulse > 0.0)
            });
            assert!(has_load, "shape {shape:?} must carry load before the snapshot");
        }
        assert_eq!(shape_get_contact_data(&original, slider, 1)[0].manifolds.len(), 2,
            "the seed must contain two warm material patches in one set contact");

        let seed = image(&original);
        let reader = recording_replay::load_registry(&seed.1).expect("the edited geometry registry must load");
        assert_eq!(reader.slot_count(), 3, "the mesh, edited rails and shared feet must intern once each");
        let mut restored = create_world(&wd);
        assert!(world_snapshot::deserialize_into_shell(&seed.0, &mut restored, &reader), "edited set seed must restore");
        drop(reader);
        let set = shape_get_box_set(&restored, support);
        assert_eq!(box_set_get(set, 1), Some(&rails[1]), "child material and all numeric tag bits must survive");
        assert!(box_set_get(set, 2).is_none() && box_set_get(set, 3).is_none(), "free slots must remain empty");
        assert!(image(&restored) == seed, "restoring must reproduce every image and registry byte at {workers} workers");
        for tick in 0..4 {
            if tick == 2 {
                for world in [&mut original, &mut restored] {
                    let contacts = shape_get_contact_data(world, slider, 1);
                    let contact = &contacts[0];
                    let id = contact.contact_id;
                    let rail_is_a = contact.shape_id_a == support;
                    let rows: [_; 2] = std::array::from_fn(|m| {
                        contact::contact_point_children(world, id, m as i32, 0).expect("each loaded rail must have a live row")
                    });
                    let rail_slots = rows.map(|pair| if rail_is_a { pair.0 } else { pair.1 });
                    assert!(rail_slots.contains(&0) && rail_slots.contains(&1), "both rail lifetimes must survive restore");
                    // Replace the loaded rail and allocate both pre-snapshot free slots.
                    shape_edit_box_set(world, support, &[1], &[rails[1], rails[3], rails[2]], &mut slots);
                    assert_eq!(slots, [1, 3, 2], "the restored free list must preserve allocation order");
                    for (m, pair) in rows.into_iter().enumerate() {
                        assert_eq!(contact::contact_point_children(world, id, m as i32, 0), (rail_slots[m] == 0).then_some(pair),
                            "only rows for the reused lifetime must become stale");
                    }
                }
                assert!(image(&original) == image(&restored), "edits must reproduce stale warm rows and tree topology");
            }
            world_step(&mut original, 1.0 / 60.0, 4);
            world_step(&mut restored, 1.0 / 60.0, 4);
            assert!(image(&original) == image(&restored), "continuation diverged at tick {tick} with {workers} workers");
        }
        destroy_world(restored);
        destroy_world(original);
    }
}

#[test]
fn mesh_set_drop_sleeps_identically_at_one_and_four_workers() {
    // One more contact than a collide-task block puts this fixture on the
    // parallel narrow-phase route while keeping the individual drops independent.
    const COUNT: usize = 21;
    let floor = mesh::create_grid_mesh(1, 1, 64.0, 0, true);
    let set = create_box_set(&[
        child(vec3(-0.15, 0.3, -0.15), vec3(0.15, 0.5, 0.15), 0),
        child(vec3(-0.4, 0.0, -0.3), vec3(0.4, 0.3, 0.3), 1),
    ]);
    let drop_sets = |workers| {
        let mut world = create_world(&WorldDef { worker_count: workers, ..default_world_def() });
        let ground = create_body(&mut world, &default_body_def());
        let def = default_shape_def();
        let floor_id = create_mesh_shape(&mut world, ground, &def, &floor, Vec3::ONE);
        let set_def = ShapeDef {
            enable_hit_events: true,
            materials: vec![def.base_material, SurfaceMaterial { user_material_id: 17, ..def.base_material }],
            ..def
        };
        let bodies: Vec<_> = (0..COUNT)
            .map(|i| {
                let body = create_body(
                    &mut world,
                    &BodyDef {
                        body_type: BodyType::Dynamic,
                        position: pos(1.0 + i as f32, 0.4, -1.0),
                        ..default_body_def()
                    },
                );
                let shape = create_box_set_shape(&mut world, body, &set_def, &set);
                (body, shape)
            })
            .collect();
        let mut hit = [false; COUNT];
        let mut sleep_steps = [None; COUNT];
        for tick in 1..=120 {
            world_step(&mut world, 1.0 / 60.0, 4);
            for event in &world.contact_hit_events {
                assert_eq!(event.shape_id_a, floor_id, "the terrain must be side A");
                assert_eq!(event.user_material_id_b, 17, "the hit must use the lower child's material");
                let i =
                    bodies.iter().position(|&(_, shape)| shape == event.shape_id_b).expect("the hit must name a set");
                hit[i] = true;
            }
            for (i, &(body, _)) in bodies.iter().enumerate() {
                if sleep_steps[i].is_none() && !body_is_awake(&world, body) {
                    sleep_steps[i] = Some(tick);
                }
            }
            if sleep_steps.iter().all(Option::is_some) {
                break;
            }
        }
        let mut result = Vec::new();
        for (i, &(body, shape)) in bodies.iter().enumerate() {
            assert!(hit[i], "each falling set must produce a hit");
            let pose = body_get_transform(&world, body);
            assert!(pose.p.y.abs() < 0.03, "the set must settle on the mesh floor");
            let contacts = shape_get_contact_data(&world, shape, 2);
            assert_eq!(contacts.len(), 1, "each sleeping set must have one terrain contact");
            assert_eq!(contacts[0].manifolds.len(), 1, "the floor must support a single coplanar patch");
            let point_count = contacts[0].manifolds[0].point_count;
            assert!(point_count >= 3, "the settled foot must retain an area of support");
            result.push((sleep_steps[i].expect("the supported body must sleep"), pose, point_count));
        }
        destroy_world(world);
        result
    };
    assert_eq!(drop_sets(1), drop_sets(4), "sleep steps, poses and support must agree across worker counts");
}

#[test]
fn height_field_kerb_keeps_both_support_planes() {
    let height = 0.25;
    // The last terrace lies beyond triangle 256 in query order. The space
    // between the feet contains terrain but no boxes.
    const LAST_CELL: usize = 129;
    const COLUMNS: usize = LAST_CELL + 2;
    let mut heights = [0.0; 2 * COLUMNS];
    for row in heights.chunks_exact_mut(COLUMNS) {
        row[LAST_CELL..].fill(height);
    }
    let dx = 0.125;
    let field = height_field::create_height_field(&HeightFieldDef {
        heights: &heights,
        count_x: COLUMNS as i32,
        count_z: 2,
        scale: vec3(dx, 1.0, 2.0),
        global_maximum_height: height,
        ..Default::default()
    });
    let mut world = create_world(&default_world_def());
    let ground = create_body(&mut world, &default_body_def());
    let def = default_shape_def();
    create_height_field_shape(&mut world, ground, &def, &field);
    let set = create_box_set(&[
        child(vec3(0.3 * dx, 0.0, 0.25), vec3(0.65 * dx, 0.3, 0.6), 0),
        child(
            vec3((LAST_CELL as f32 + 0.3) * dx, height, 0.25),
            vec3((LAST_CELL as f32 + 0.65) * dx, height + 0.3, 0.6),
            0,
        ),
    ]);
    let body = create_body(&mut world, &BodyDef { body_type: BodyType::Dynamic, ..default_body_def() });
    let shape = create_box_set_shape(&mut world, body, &def, &set);
    for _ in 0..4 {
        world_step(&mut world, 1.0 / 60.0, 4);
        let contacts = shape_get_contact_data(&world, shape, 2);
        assert_eq!(contacts.len(), 1, "both terraces must belong to one shape-pair contact");
        let data = &contacts[0];
        assert_eq!(data.manifolds.len(), 2, "parallel planes at different heights need separate patches");
        let mut supports = [false; 2];
        for (m, manifold) in data.manifolds.iter().enumerate() {
            assert!(
                manifold.normal.y > 0.99 && manifold.point_count > 0,
                "each terrace must support the set from below"
            );
            for (p, point) in manifold.points[..manifold.point_count as usize].iter().enumerate() {
                let (a, slot) = contact::contact_point_children(&world, data.contact_id, m as i32, p as i32)
                    .expect("every support point must have a live row");
                assert_eq!(a, NULL_INDEX, "the height field has no box slot");
                assert!((0..=1).contains(&slot), "each point must name a live foot");
                assert_eq!(
                    point.triangle_index / 2,
                    LAST_CELL as i32 * slot,
                    "each foot must identify its supporting terrain cell"
                );
                supports[slot as usize] = true;
            }
        }
        assert_eq!(supports, [true, true], "both levels must retain support throughout the step sequence");
    }
    destroy_world(world);
}

#[test]
fn edits_under_load_preserve_only_surviving_rows_and_impulses() {
    // Both feet meet the same triangle with identical box topology. Only the
    // child lifetimes distinguish corresponding features on the two feet.
    let material = default_surface_material();
    check_sliding_edits(
        SlidingSurface::Mesh(&mesh::create_grid_mesh(1, 1, 16.0, 0, true)),
        &[SurfaceMaterial { friction: 1.0, tangent_velocity: vec3(0.0, 0.0, 8.0), ..material }],
        &[
            SurfaceMaterial { friction: 0.25, ..material },
            SurfaceMaterial { friction: 1.0, restitution: 0.5, ..material },
        ],
        &BodyDef { body_type: BodyType::Dynamic, position: pos(0.0, 0.0, -3.0), ..default_body_def() },
    );
}

#[test]
fn rotated_set_slides_on_two_mesh_rails() {
    let vertices = [-1.0, 1.0].map(|x| {
        [vec3(x - 0.8, 0.0, -6.0), vec3(x - 0.8, 0.0, 6.0), vec3(x + 0.8, 0.0, -6.0), vec3(x + 0.8, 0.0, 6.0)]
    });
    let mesh_def = MeshDef {
        vertices: vertices.as_flattened(),
        indices: &[0, 1, 2, 2, 1, 3, 4, 5, 6, 6, 5, 7],
        material_indices: &[0, 0, 1, 1],
        identify_edges: true,
        ..Default::default()
    };
    let rails = mesh::create_mesh(&mesh_def, None).expect("both disconnected rails must form one mesh");
    check_rotated_rails(SlidingSurface::Mesh(&rails));
}

#[test]
fn rotated_set_slides_on_two_set_rails() {
    let rails = create_box_set(&[
        child(vec3(-1.8, -0.4, -6.0), vec3(-0.2, 0.0, 6.0), 0),
        child(vec3(0.2, -0.4, -6.0), vec3(1.8, 0.0, 6.0), 1),
    ]);
    check_rotated_rails(SlidingSurface::Set(&rails, make_quat_from_axis_angle(Vec3::AXIS_Y, 0.1)));
}

enum SlidingSurface<'a> {
    Mesh(&'a Arc<MeshData>),
    Set(&'a Arc<BoxSetData>, Quat),
}

fn check_rotated_rails(rails: SlidingSurface<'_>) {
    let material = default_surface_material();
    let foot = SurfaceMaterial { friction: 1.0, ..material };
    check_sliding_edits(
        rails,
        &[
            SurfaceMaterial { friction: 0.25, ..material },
            SurfaceMaterial { friction: 1.0, restitution: 0.5, ..material },
        ],
        &[SurfaceMaterial { rolling_resistance: 0.1, ..foot }, foot],
        &BodyDef {
            body_type: BodyType::Dynamic,
            position: pos(0.0, 0.0, -3.0),
            rotation: make_quat_from_axis_angle(Vec3::AXIS_Y, 0.3),
            linear_velocity: vec3(0.0, 0.0, 8.0),
            angular_velocity: Vec3::AXIS_Y,
            ..default_body_def()
        },
    );
}

fn check_sliding_edits(
    rails: SlidingSurface<'_>,
    surface: &[SurfaceMaterial],
    foot: &[SurfaceMaterial],
    body_def: &BodyDef,
) {
    use makepad_box3d::contact::{contact_get_data, contact_point_children};
    let mut world = create_world(&default_world_def());
    let ground = create_body(&mut world, &default_body_def());
    let ground_def = ShapeDef { materials: surface.to_vec(), ..default_shape_def() };
    let rail_slots = match rails {
        SlidingSurface::Mesh(mesh) => {
            create_mesh_shape(&mut world, ground, &ground_def, mesh, Vec3::ONE);
            [NULL_INDEX; 2]
        }
        SlidingSurface::Set(set, rotation) => {
            body_set_transform(&mut world, ground, body_def.position, rotation);
            create_box_set_shape(&mut world, ground, &ground_def, set);
            [0, 1]
        }
    };
    let body = create_body(&mut world, body_def);
    let feet = [
        child(vec3(-1.5, 0.0, -0.5), vec3(-0.5, 0.4, 0.5), 0),
        child(vec3(0.5, 0.0, -0.5), vec3(1.5, 0.4, 0.5), 1),
        child(vec3(-0.1, 1.0, -0.1), vec3(0.1, 1.2, 0.1), 0),
    ];
    let shape_def = ShapeDef { materials: foot.to_vec(), ..default_shape_def() };
    let shape = create_box_set_shape(&mut world, body, &shape_def, &create_box_set(&feet));
    world_step(&mut world, 1.0 / 60.0, 4);
    let contacts = shape_get_contact_data(&world, shape, 2);
    assert_eq!(contacts.len(), 1, "both loaded feet must share a contact id");
    let id = contacts[0].contact_id;
    assert_eq!(contacts[0].manifolds.len(), 2, "coplanar feet with different materials must remain separate");
    for (m, patch) in contacts[0].manifolds.iter().enumerate() {
        let (_, slot) = contact_point_children(&world, id, m as i32, 0).expect("a sliding patch must name its foot");
        let normal: f32 = patch.points[..patch.point_count as usize].iter().map(|p| p.normal_impulse).sum();
        assert!(normal > 0.0, "each sliding foot must carry load");
        // Slip is fast enough to saturate both Coulomb limits. Averaging the
        // two frictions would give the same ratio on both supports.
        let expected = [0.5, 1.0][slot as usize];
        assert!(
            (length(patch.friction_impulse) / normal - expected).abs() < 1e-4,
            "foot {slot} must slide at its own mixed friction {expected}"
        );
        if foot[slot as usize].rolling_resistance == 0.0 {
            assert_eq!(patch.rolling_impulse, Vec3::ZERO, "the free-rolling foot must receive no rolling impulse");
        } else {
            assert!(length_squared(patch.rolling_impulse) > 0.0, "the spinning foot must encounter rolling resistance");
        }
    }
    // Return to the narrow-phase pose. Zero-duration steps refresh contacts
    // without a solve, making the transferred impulses observable exactly.
    body_set_transform(&mut world, body, body_def.position, body_def.rotation);
    world_step(&mut world, 0.0, 1);

    let read = |w: &physics_world::World| {
        let contacts = shape_get_contact_data(w, shape, 2);
        assert_eq!(contacts.len(), 1, "an incremental edit must retain the touching parent pair");
        assert_eq!(contacts[0].contact_id, id, "an edit must preserve contact generation and slot");
        let mut points = Vec::new();
        let mut patches = Vec::new();
        for (m, manifold) in contacts[0].manifolds.iter().enumerate() {
            let (_, slot) = contact_point_children(w, id, m as i32, 0).expect("a patch must have a live foot");
            patches.push((slot, manifold.friction_impulse, manifold.twist_impulse, manifold.rolling_impulse));
            for (p, mp) in manifold.points[..manifold.point_count as usize].iter().enumerate() {
                assert_eq!(
                    contact_point_children(w, id, m as i32, p as i32),
                    Some((rail_slots[slot as usize], slot)),
                    "all points in a material patch must resolve to the same support and foot"
                );
                points.push((slot, mp.triangle_index, mp.feature_id, mp.normal_impulse.to_bits()));
            }
        }
        patches.sort_by_key(|p| p.0);
        points.sort_unstable();
        (points, patches)
    };
    let (loaded, patches) = read(&world);
    let fraction = loaded.iter().filter(|p| p.0 == 1).count() as f32 / loaded.len() as f32;
    let parent = &world.contacts[(id.index1 - 1) as usize];
    assert!(
        (parent.friction - (0.5 + 0.5 * fraction)).abs() < 1e-6,
        "friction must weight both patches by point count"
    );
    assert!((parent.restitution - 0.5 * fraction).abs() < 1e-6, "restitution must use the same point weights");
    for slot in [0, 1] {
        assert!(loaded.iter().any(|p| p.0 == slot && f32::from_bits(p.3) > 0.0), "each original foot must carry load");
    }
    assert!(patches.iter().any(|p| length_squared(p.1) > 0.0), "sliding must load a friction patch");
    let mut slots = Vec::new();
    shape_edit_box_set(&mut world, shape, &[2], &[], &mut slots);
    world_step(&mut world, 0.0, 1);
    assert_eq!(
        read(&world),
        (loaded.clone(), patches.clone()),
        "removing the elevated box must preserve points and impulses"
    );

    shape_edit_box_set(&mut world, shape, &[1], &[], &mut slots);
    world_step(&mut world, 0.0, 1);
    let survivors = (
        loaded.into_iter().filter(|p| p.0 == 0).collect::<Vec<_>>(),
        patches.into_iter().filter(|p| p.0 == 0).collect::<Vec<_>>(),
    );
    assert_eq!(read(&world), survivors, "removing a foot must drop only that foot's rows and impulses");

    shape_edit_box_set(&mut world, shape, &[], &feet[1..2], &mut slots);
    assert_eq!(slots, [1], "the replacement should occupy the freed slot");
    world_step(&mut world, 0.0, 1);
    let (points, patches) = read(&world);
    assert_eq!(
        points.iter().filter(|p| p.0 == 0).copied().collect::<Vec<_>>(),
        survivors.0,
        "slot reuse must preserve the surviving foot's features and exact normal load"
    );
    assert_eq!(patches[0], survivors.1[0], "slot reuse must preserve the surviving patch impulses");
    assert!(points.iter().any(|p| p.0 == 1), "the replacement must acquire support");
    assert!(
        points.iter().filter(|p| p.0 == 1).all(|p| p.3 == 0),
        "the replacement must start with zero normal impulses"
    );
    assert_eq!(patches[1], (1, Vec3::ZERO, 0.0, Vec3::ZERO), "a wholly replaced patch must start cold");
    let m = (0..contact_get_data(&world, id).manifolds.len() as i32)
        .find(|&m| contact_point_children(&world, id, m, 0) == Some((rail_slots[1], 1)))
        .unwrap();
    shape_edit_box_set(&mut world, shape, &[1], &feet[1..2], &mut slots);
    assert_eq!(slots, [1], "the same edit must reuse the removed slot");
    assert_eq!(
        contact_point_children(&world, id, m, 0),
        None,
        "a row from before slot reuse must immediately be stale"
    );
    world_step(&mut world, 0.0, 1);
    assert_eq!(
        contact_point_children(&world, id, m, 0),
        Some((rail_slots[1], 1)),
        "the refreshed row must name the new lifetime"
    );
    assert!(
        !contact_get_data(&world, id).manifolds[m as usize].points[0].persisted,
        "a reused slot must not warm-match the retired box"
    );

    // Replay the same cold impact with restitution first suppressed, then
    // active. Only the foot with nonzero restitution can gain normal impulse.
    let pair = create_box_set(&feet[..2]);
    let mut impact = |threshold| {
        body_set_transform(&mut world, body, body_def.position, body_def.rotation);
        shape_set_box_set(&mut world, shape, &pair);
        body_set_linear_velocity(&mut world, body, vec3(0.0, -4.0, 0.0));
        body_set_angular_velocity(&mut world, body, Vec3::ZERO);
        physics_world::world_set_restitution_threshold(&mut world, threshold);
        world_step(&mut world, 1.0 / 60.0, 1);
        let contacts = shape_get_contact_data(&world, shape, 2);
        assert_eq!(contacts.len(), 1, "the cold impact must produce one shape-pair contact");
        let contact = &contacts[0];
        let mut load = [0.0; 2];
        for (m, patch) in contact.manifolds.iter().enumerate() {
            let (_, slot) = contact_point_children(&world, contact.contact_id, m as i32, 0).expect("a live impact row");
            load[slot as usize] +=
                patch.points[..patch.point_count as usize].iter().map(|p| p.normal_impulse).sum::<f32>();
        }
        load
    };
    let suppressed = impact(8.0);
    let active = impact(1.0);
    assert!(suppressed.iter().all(|&load| load > 0.0), "both feet must absorb the incoming impact");
    assert_eq!(active[0], suppressed[0], "the inelastic patch must receive no restitution impulse");
    assert!(active[1] > suppressed[1], "the elastic patch must receive its own restitution impulse");
    destroy_world(world);
}

#[test]
fn convex_shapes_on_an_edited_set_agree_across_workers() {
    // A gap beneath the hull requires both feet to carry load. The sphere
    // rolls on the two adjacent boxes in the next row.
    let boxes = [
        child(vec3(-1.0, -0.5, -2.0), vec3(-0.25, 0.0, -0.5), 0),
        child(vec3(0.25, -0.5, -2.0), vec3(1.0, 0.0, -0.5), 1),
        child(vec3(-2.0, -0.5, -0.5), vec3(0.0, 0.0, 20.0), 0),
        child(vec3(0.0, -0.5, -0.5), vec3(2.0, 0.0, 20.0), 0),
    ];
    let set = create_box_set(&boxes);
    let run = |workers| {
        let mut world = create_world(&WorldDef { worker_count: workers, ..default_world_def() });
        let fixed = create_body(&mut world, &default_body_def());
        let material = default_surface_material();
        let shape_def = default_shape_def();
        let floor = create_box_set_shape(
            &mut world,
            fixed,
            &ShapeDef { materials: vec![material, SurfaceMaterial { friction: 0.8, ..material }], ..shape_def.clone() },
            &set,
        );
        let hull_def = BodyDef {
            body_type: BodyType::Dynamic,
            position: pos(0.0, 0.25, -1.0),
            rotation: make_quat_from_axis_angle(Vec3::AXIS_Y, 0.12),
            ..default_body_def()
        };
        let hull_body = create_body(&mut world, &hull_def);
        let hull = create_hull_shape(&mut world, hull_body, &shape_def, &make_box_hull(0.75, 0.25, 0.3));
        let ball = create_body(
            &mut world,
            &BodyDef {
                body_type: BodyType::Dynamic,
                position: pos(-0.6, 0.2, 0.0),
                linear_velocity: vec3(2.0, 0.0, 0.0),
                angular_velocity: vec3(0.0, 0.0, -10.0),
                ..default_body_def()
            },
        );
        let sphere = create_sphere_shape(&mut world, ball, &shape_def, &Sphere { center: Vec3::ZERO, radius: 0.2 });

        // Nineteen independent capsules plus the hull and sphere exceed one
        // collide-task block. Each capsule rests on its side along the first box.
        let mut capsules = Vec::new();
        for z in 1..=19 {
            let body = create_body(
                &mut world,
                &BodyDef { body_type: BodyType::Dynamic, position: pos(-1.0, 0.2, z as f32), ..default_body_def() },
            );
            let shape = create_capsule_shape(
                &mut world,
                body,
                &shape_def,
                &Capsule { center1: vec3(-0.25, 0.0, 0.0), center2: vec3(0.25, 0.0, 0.0), radius: 0.2 },
            );
            capsules.push(shape);
        }
        world_step(&mut world, 1.0 / 60.0, 4);
        for capsule in capsules {
            let contacts = shape_get_contact_data(&world, capsule, 2);
            assert_eq!(contacts.len(), 1, "each capsule must have one parent contact");
            let c = &contacts[0];
            assert_eq!((c.shape_id_a, c.shape_id_b), (floor, capsule), "the capsule must be side B");
            assert_eq!(c.manifolds.len(), 1, "a capsule along a single face must have a single patch");
            let m = &c.manifolds[0];
            assert_eq!(m.point_count, 2, "both ends of a horizontal capsule must meet its support");
            for p in 0..m.point_count {
                assert_eq!(
                    contact::contact_point_children(&world, c.contact_id, 0, p),
                    Some((2, NULL_INDEX)),
                    "each capsule point must identify its supporting child"
                );
            }
        }

        let hull_contacts = shape_get_contact_data(&world, hull, 2);
        assert_eq!(hull_contacts.len(), 1, "a hull on two boxes must have one parent contact");
        let id = hull_contacts[0].contact_id;
        let read_hull = |w: &physics_world::World| {
            let contacts = shape_get_contact_data(w, hull, 2);
            assert_eq!(contacts.len(), 1, "editing a support must retain the hull contact");
            let c = &contacts[0];
            assert_eq!(c.contact_id, id, "the hull's contact id must survive the edit");
            assert_eq!((c.shape_id_a, c.shape_id_b), (floor, hull), "only side A owns a child slot");
            assert_eq!(c.manifolds.len(), 2, "differently surfaced halves must yield distinct patches");
            let mut points = [Vec::new(), Vec::new()];
            for (m, patch) in c.manifolds.iter().enumerate() {
                assert!(patch.normal.y > 0.99, "each hull patch must support from below");
                for (p, point) in patch.points[..patch.point_count as usize].iter().enumerate() {
                    let (slot, other) =
                        contact::contact_point_children(w, id, m as i32, p as i32).expect("live hull row");
                    assert!((0..=1).contains(&slot), "each hull point must name a supporting foot");
                    assert_eq!(
                        (other, point.triangle_index),
                        (NULL_INDEX, NULL_INDEX),
                        "only the set child has an index"
                    );
                    points[slot as usize].push((point.feature_id, point.normal_impulse.to_bits(), point.persisted));
                }
            }
            for half in &mut points {
                half.sort_unstable();
            }
            points
        };
        let loaded = read_hull(&world);
        assert!(
            loaded.iter().all(|half| half.iter().any(|p| f32::from_bits(p.1) > 0.0)),
            "the hull must load both supports before replacement"
        );
        // Return to the pose of the loaded manifold. A zero-duration step
        // recomputes after the edit without giving the solver a new impulse.
        body_set_transform(&mut world, hull_body, hull_def.position, hull_def.rotation);
        let retired_patch = (0..2)
            .find(|&m| contact::contact_point_children(&world, id, m, 0) == Some((1, NULL_INDEX)))
            .expect("the retiring support must own a loaded patch");
        let mut slots = Vec::new();
        shape_edit_box_set(&mut world, floor, &[1], &boxes[1..2], &mut slots);
        assert_eq!(slots, [1], "the replacement must reuse the retired slot");
        assert_eq!(
            contact::contact_point_children(&world, id, retired_patch, 0),
            None,
            "the retired side-A row must be stale before the narrow phase runs"
        );
        world_step(&mut world, 0.0, 1);
        let refreshed = read_hull(&world);
        let surviving: Vec<_> = loaded[0].iter().map(|&(feature, load, _)| (feature, load, true)).collect();
        assert_eq!(refreshed[0], surviving, "the survivor must retain every feature and its exact normal impulse");
        assert_eq!(
            refreshed[1].iter().map(|p| p.0).collect::<Vec<_>>(),
            loaded[1].iter().map(|p| p.0).collect::<Vec<_>>(),
            "the same replacement geometry must reproduce the retired features"
        );
        assert!(refreshed[1].iter().all(|p| p.1 == 0 && !p.2), "the new serial must start every point cold");

        let mut trajectory = Vec::new();
        let mut supported = [false; 2];
        let sphere_id = shape_get_contact_data(&world, sphere, 2)
            .first()
            .expect("the sphere must initially rest on the left child")
            .contact_id;
        for _ in 0..36 {
            let start_x = body_get_position(&world, ball).x;
            world_step(&mut world, 1.0 / 60.0, 4);
            let pose = body_get_transform(&world, ball);
            let contacts = shape_get_contact_data(&world, sphere, 2);
            assert_eq!(contacts.len(), 1, "the rolling sphere must keep one parent contact across the joint");
            let c = &contacts[0];
            assert_eq!(c.contact_id, sphere_id, "crossing a box boundary must preserve the sphere contact id");
            let mut step_load = [0.0; 2];
            let mut final_load = [0.0; 2];
            for (m, patch) in c.manifolds.iter().enumerate() {
                for (p, point) in patch.points[..patch.point_count as usize].iter().enumerate() {
                    let (slot, other) = contact::contact_point_children(&world, sphere_id, m as i32, p as i32)
                        .expect("live sphere row");
                    assert_eq!(other, NULL_INDEX, "the sphere must not acquire a child slot");
                    assert!((2..=3).contains(&slot), "each sphere point must name a box on its track");
                    let child = (slot - 2) as usize;
                    // A rolling point can support the ball early in the step and unload by the final substep.
                    step_load[child] += patch.normal.y * point.total_normal_impulse;
                    final_load[child] += patch.normal.y * point.normal_impulse;
                }
            }
            // At 0.25 m from the joint the 0.2 m sphere is wholly over one child.
            supported[0] |= start_x.max(pose.p.x) < -0.25 && step_load[0] > 0.0;
            supported[1] |= start_x.min(pose.p.x) > 0.25 && step_load[1] > 0.0;
            trajectory.push((pose, step_load, final_load));
        }
        assert!(supported[0], "slot 2 must carry the rolling sphere before it reaches the joint");
        assert!(supported[1], "slot 3 must carry the rolling sphere after it clears the joint");
        let pose = body_get_transform(&world, ball);
        assert!(pose.p.x > 0.4 && (pose.p.y - 0.2).abs() < 0.02, "the sphere must roll onto the second box");
        assert!(body_get_angular_velocity(&world, ball).z < -1.0, "the ball must keep rolling after crossing");
        destroy_world(world);
        (loaded, refreshed, trajectory)
    };
    assert_eq!(run(1), run(4), "hull warm starts, sphere poses and loads must agree across worker counts");
}

#[test]
fn a_near_miss_between_sets_does_not_scale_with_their_resolution() {
    // A trunk under a wide slab of 3600 tiles, and a block floating in the open
    // under the slab: inside the slab set's bounds, near no box. The block is
    // built at two resolutions; the set/set search must not follow them.
    let mut tree = vec![child(vec3(-0.2, 0.0, -0.2), vec3(0.2, 3.0, 0.2), 0)];
    // Shared faces come from one formula, so neighbouring tiles never overlap.
    let at = |i: i32, cell: f32, origin: f32| origin + cell * i as f32;
    for i in 0..60 {
        for k in 0..60 {
            let (lo, hi) = ((at(i, 0.1, -3.0), at(k, 0.1, -3.0)), (at(i + 1, 0.1, -3.0), at(k + 1, 0.1, -3.0)));
            tree.push(child(vec3(lo.0, 3.0, lo.1), vec3(hi.0, 3.2, hi.1), 0));
        }
    }
    let tree = create_box_set(&tree);
    let search = |cell: f32| {
        let n = [1.6f32, 0.8, 0.6].map(|extent| (extent / cell).round() as i32);
        let mut boxes = Vec::new();
        for i in 0..n[0] {
            for j in 0..n[1] {
                for k in 0..n[2] {
                    let lo = vec3(at(i, cell, -0.8), at(j, cell, -0.4), at(k, cell, -0.3));
                    let hi = vec3(at(i + 1, cell, -0.8), at(j + 1, cell, -0.4), at(k + 1, cell, -0.3));
                    boxes.push(child(lo, hi, 0));
                }
            }
        }
        let mut world = create_world(&WorldDef { gravity: vec3(0.0, 0.0, 0.0), ..default_world_def() });
        let fixed = create_body(&mut world, &default_body_def());
        let def = default_shape_def();
        create_box_set_shape(&mut world, fixed, &def, &tree);
        let body_def = BodyDef { body_type: BodyType::Dynamic, position: pos(1.6, 1.2, 0.0), ..default_body_def() };
        let body = create_body(&mut world, &body_def);
        create_box_set_shape(&mut world, body, &def, &create_box_set(&boxes));
        // A slow drift keeps the pose changing, so the contact is not recycled.
        body_set_linear_velocity(&mut world, body, vec3(0.0, 0.0, 0.5));
        world_step(&mut world, 1.0 / 60.0, 4);
        let pairs: usize = world.task_contexts.iter().map(|w| w.set_child_pairs).sum();
        let visits: usize = world.task_contexts.iter().map(|w| w.set_box_visits).sum();
        destroy_world(world);
        (boxes.len(), pairs, visits)
    };
    let (coarse, fine) = (search(0.2), search(0.1));
    assert_eq!((coarse.0, fine.0), (96, 768), "the block must be built at 0.2 m and 0.1 m");
    assert_eq!((coarse.1, fine.1), (0, 0), "a near miss must yield no candidate pairs");
    assert!(coarse.2 > 0, "the near miss must reach the set/set search");
    assert!(
        fine.2 <= 2 * coarse.2,
        "the search must not follow the block's resolution: {} at 0.2 m, {} at 0.1 m",
        coarse.2,
        fine.2
    );
}

#[test]
fn crossed_set_bars_keep_surviving_loads_across_workers() {
    // Each moving pair of bars crosses the same two long supports. Twenty-one
    // independent contacts span more than one narrow-phase worker block.
    let rails = [-0.75, 0.75].map(|z| child(vec3(-32.0, -0.3, z - 0.2), vec3(32.0, 0.0, z + 0.2), 0));
    let bars = create_box_set(&[-0.6, 0.6].map(|x| child(vec3(x - 0.2, 0.0, -1.2), vec3(x + 0.2, 0.4, 1.2), 0)));
    let run = |workers| {
        let mut world = create_world(&WorldDef { worker_count: workers, ..default_world_def() });
        let fixed = create_body(&mut world, &default_body_def());
        let def = default_shape_def();
        let ground = create_box_set_shape(&mut world, fixed, &def, &create_box_set(&rails));
        let mut bodies = Vec::new();
        for i in -10..=10 {
            let position = pos(3.0 * i as f32, 0.0, 0.0);
            let body_def = BodyDef { body_type: BodyType::Dynamic, position, ..default_body_def() };
            let body = create_body(&mut world, &body_def);
            let shape = create_box_set_shape(&mut world, body, &def, &bars);
            bodies.push((body, shape, position));
        }
        let read = |world: &physics_world::World, shape| {
            let contacts = shape_get_contact_data(world, shape, 2);
            assert_eq!(contacts.len(), 1, "all four crossings must share a parent contact");
            let c = &contacts[0];
            assert_eq!((c.shape_id_a, c.shape_id_b), (ground, shape), "rows must follow contact A/B order");
            assert_eq!(c.manifolds.len(), 1, "coplanar crossings of the same material must form one patch");
            let patch = &c.manifolds[0];
            assert_eq!(patch.point_count, 4, "the reduced footprint must keep all four corners");
            let rows: [_; 4] = std::array::from_fn(|p| {
                let pair = contact::contact_point_children(world, c.contact_id, 0, p as i32).expect("live bar pair");
                let point = &patch.points[p];
                (pair, point.feature_id, point.normal_impulse.to_bits(), point.persisted)
            });
            let mut pairs = rows.map(|row| row.0);
            pairs.sort_unstable();
            assert_eq!(pairs, [(0, 0), (0, 1), (1, 0), (1, 1)], "every bar crossing must own a reduced point");
            (c.contact_id, rows)
        };

        world_step(&mut world, 1.0 / 60.0, 4);
        let candidates: usize = world.task_contexts.iter().map(|w| w.set_child_pairs).sum();
        assert_eq!(candidates, 4 * bodies.len(), "each two-by-two traversal must find exactly four candidates");
        let visits: usize = world.task_contexts.iter().map(|w| w.set_box_visits).sum();
        // The joint descent tests the root pair, each rail against the bar root and
        // the four leaf pairs (seven), then counts its four candidates.
        assert_eq!(visits, 11 * bodies.len(), "each two-by-two descent must test seven node pairs and emit four");
        let mut loaded = Vec::new();
        for &(body, shape, position) in &bodies {
            loaded.push((read(&world, shape), body_get_transform(&world, body)));
            body_set_transform(&mut world, body, position, Quat::IDENTITY);
        }
        // Replace a bar on A at the original pose. A zero-duration refresh
        // exposes the transferred impulses before another solve can change them.
        let mut added = Vec::new();
        shape_edit_box_set(&mut world, ground, &[1], &rails[1..], &mut added);
        assert_eq!(added, [1], "the replacement support must reuse the retired slot");
        for &((id, rows), _) in &loaded {
            for (p, (pair, ..)) in rows.into_iter().enumerate() {
                assert_eq!(
                    contact::contact_point_children(&world, id, 0, p as i32),
                    (pair.0 == 0).then_some(pair),
                    "only rows for the retired A serial must become stale"
                );
            }
        }
        world_step(&mut world, 0.0, 1);
        let mut results = Vec::new();
        for ((_, shape, _), ((id, before), pose)) in bodies.into_iter().zip(loaded) {
            let (next_id, after) = read(&world, shape);
            assert_eq!(next_id, id, "replacing one bar must preserve the parent contact id");
            for slot in 0..2 {
                assert!(
                    before.iter().any(|(pair, _, bits, _)| pair.0 == slot && f32::from_bits(*bits) > 0.0),
                    "support bar {slot} must carry load before replacement"
                );
            }
            for (pair, feature, impulse, persisted) in after {
                let old = before.iter().find(|old| old.0 == pair).unwrap();
                assert_eq!(feature, old.1, "identical geometry must keep the feature at crossing {pair:?}");
                if pair.0 == 0 {
                    assert!(persisted, "the surviving crossing {pair:?} must warm-start");
                    assert_eq!(impulse, old.2, "the surviving crossing {pair:?} must keep its exact impulse");
                } else {
                    assert!(!persisted && impulse == 0, "the replacement at crossing {pair:?} must start cold");
                }
            }
            results.push((id.index1, id.generation, pose, before, after));
        }
        destroy_world(world);
        results
    };
    assert_eq!(run(1), run(4), "bar contact ids, poses, ordered features and impulses must agree across workers");
}

#[test]
fn set_kerb_keeps_both_support_planes() {
    let mut world = create_world(&default_world_def());
    physics_world::world_set_contact_recycle_distance(&mut world, 0.0);
    // Both bodies can respond. Gravity loads the feet against the initially
    // stationary kerb, whose downward motion then comes from their contact.
    let base_def = BodyDef { body_type: BodyType::Dynamic, gravity_scale: 0.0, ..default_body_def() };
    let base = create_body(&mut world, &base_def);
    let body = create_body(&mut world, &BodyDef { body_type: BodyType::Dynamic, ..default_body_def() });
    let def = default_shape_def();
    let supports = [(-1.0, 0.0), (1.0, 0.25)];
    let kerb = create_box_set(&supports.map(|(x, y)| child(vec3(x - 0.7, -0.5, -1.0), vec3(x + 0.7, y, 1.0), 0)));
    let feet = create_box_set(&supports.map(|(x, y)| child(vec3(x - 0.4, y, -0.4), vec3(x + 0.4, y + 0.3, 0.4), 0)));
    let ground = create_box_set_shape(&mut world, base, &def, &kerb);
    let shape = create_box_set_shape(&mut world, body, &def, &feet);
    let center = body_get_local_center_of_mass(&world, base);
    for _ in 0..4 {
        let rotation = body_get_rotation(&world, base);
        world_step(&mut world, 1.0 / 60.0, 4);
        let contacts = shape_get_contact_data(&world, shape, 2);
        assert_eq!(contacts.len(), 1, "both kerb levels must share a parent contact");
        let c = &contacts[0];
        assert_eq!(c.manifolds.len(), 2, "equal materials and parallel normals must still distinguish the two heights");
        let kerb_is_a = c.shape_id_a == ground;
        let mut loaded = [false; 2];
        for (m, patch) in c.manifolds.iter().enumerate() {
            let upward = if kerb_is_a { patch.normal.y } else { -patch.normal.y };
            assert!(upward > 0.99, "each kerb patch must support from below");
            let pair = contact::contact_point_children(&world, c.contact_id, m as i32, 0).expect("live kerb row");
            assert!(pair.0 == pair.1 && (0..2).contains(&pair.0), "each foot must meet its own kerb level");
            let level = pair.0 as usize;
            for (p, point) in patch.points[..patch.point_count as usize].iter().enumerate() {
                assert_eq!(
                    contact::contact_point_children(&world, c.contact_id, m as i32, p as i32),
                    Some(pair),
                    "a kerb patch must contain only points at its own height"
                );
                let anchor = if kerb_is_a { point.anchor_a } else { point.anchor_b };
                let height = center.y + inv_rotate_vector(rotation, anchor).y;
                assert!((height - supports[level].1).abs() < linear_slop(), "the contact point must lie on its kerb plane");
                loaded[level] |= point.total_normal_impulse > 0.0;
            }
        }
        assert_eq!(loaded, [true; 2], "both kerb levels must keep load-bearing points");
    }
    assert!(body_get_linear_velocity(&world, base).y < 0.0, "the feet must transfer downward momentum to the kerb");
    destroy_world(world);
}

#[test]
fn fast_set_sweeps_walls_and_a_tessellated_crest() {
    const DT: f32 = 1.0 / 60.0;
    const SPEED: f32 = 76.0;
    const CREST: f32 = 10.0;
    let mut world = create_world(&WorldDef { gravity: Vec3::ZERO, ..default_world_def() });
    let fixed = create_body(&mut world, &default_body_def());
    let def = ShapeDef {
        base_material: SurfaceMaterial { friction: 0.0, ..default_surface_material() },
        ..default_shape_def()
    };

    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for x in [0.0, 0.12] {
        let i = vertices.len() as i32;
        vertices.extend([vec3(x, -2.0, -40.0), vec3(x, -2.0, 40.0), vec3(x, 2.0, 40.0), vec3(x, 2.0, -40.0)]);
        indices.extend([i, i + 1, i + 2, i, i + 2, i + 3]);
    }
    // Four quads across a 3 cm crest. A horizontal sweep clears it at 5 cm,
    // but at 2 cm the rising face can intercept a corner within one tick.
    let crest_height = |x: f32| (0.03 - 0.075 * x.abs()).max(0.0);
    for (k, x) in [-0.4, -0.2, 0.0, 0.2, 0.4].into_iter().enumerate() {
        let i = vertices.len() as i32;
        vertices.extend([vec3(CREST + x, crest_height(x), -1.0), vec3(CREST + x, crest_height(x), 1.0)]);
        if k > 0 {
            indices.extend([i - 2, i - 1, i + 1, i - 2, i + 1, i]);
        }
    }
    let terrain = mesh::create_mesh(&MeshDef { vertices: &vertices, indices: &indices, identify_edges: true, ..Default::default() }, None)
        .expect("the wall and crest triangles must be nondegenerate");
    create_mesh_shape(&mut world, fixed, &def, &terrain, Vec3::ONE);
    let set = create_box_set(&[
        child(vec3(-0.2, 0.0, -0.43), vec3(0.2, 0.4, -0.03), 0),
        child(vec3(-0.2, 0.0, 0.03), vec3(0.2, 0.4, 0.43), 0),
    ]);
    let body = create_body(&mut world, &BodyDef { body_type: BodyType::Dynamic, ..default_body_def() });
    create_box_set_shape(&mut world, body, &def, &set);
    let launch = |w: &mut physics_world::World, start, velocity, spin| {
        body_set_transform(w, body, start, Quat::IDENTITY);
        body_set_linear_velocity(w, body, velocity);
        body_set_angular_velocity(w, body, spin);
    };
    let clamped = |w: &physics_world::World| get_body_sim_from_id(w, body.index1 - 1).flags & HAD_TIME_OF_IMPACT != 0;
    for (velocity, spin) in [
        (vec3(SPEED, 0.0, 0.0), Vec3::ZERO),
        (vec3(SPEED * (30.0 * DEG_TO_RAD).cos(), 0.0, SPEED * 0.5), vec3(10.0, 10.0, 10.0)),
    ] {
        launch(&mut world, pos(-0.8, 0.0, 0.0), velocity, spin);
        world_step(&mut world, DT, 4);
        assert!(clamped(&world), "the wall must clamp the fast set with angular velocity {spin:?}");
        let xf = body_get_transform(&world, body);
        if spin != Vec3::ZERO {
            assert!(length(xf.q.v) > 0.0 && xf.p.z > 0.0, "the oblique hit must advance translation and rotation together");
        }
        let mut leading = f32::NEG_INFINITY;
        for (_, child) in set.boxes() {
            let center = transform_world_point(xf, aabb_center(child.bounds));
            assert!(center.x < 0.0, "every box centre must stay in front of the first wall");
            let bounds = aabb_transform(to_relative_transform(xf, POS_ZERO), child.bounds);
            leading = leading.max(bounds.upper_bound.x);
        }
        assert!((-0.02..=0.001).contains(&leading), "the leading corner must reach the first of the two walls, got {leading}");
    }

    launch(&mut world, pos(-0.21, 0.0, -38.0), vec3(0.0, 0.0, SPEED), Vec3::ZERO);
    for _ in 0..60 {
        world_step(&mut world, DT, 4);
        assert!(!clamped(&world), "a set sliding 1 cm clear of a wall must not snag");
    }
    let travel = sub_pos(body_get_position(&world, body), pos(-0.21, 0.0, -38.0));
    assert!((travel.z - SPEED).abs() <= 0.01 * SPEED, "wall-parallel travel must stay within 1% of free travel");

    // Keep the underbody level so its clearance at every intermediate x has an
    // independent geometric oracle, including a tick that spans the whole ridge.
    body_set_motion_locks(&mut world, body, MotionLocks { angular_x: true, angular_y: true, angular_z: true, ..Default::default() });
    for clearance in [0.05, 0.02] {
        let start_x = CREST - 0.5 * DT * SPEED;
        launch(&mut world, pos(start_x, clearance, 0.0), vec3(SPEED, 0.0, 0.0), Vec3::ZERO);
        for tick in 0..4 {
            let old = sub_pos(body_get_position(&world, body), pos(CREST, 0.0, 0.0));
            world_step(&mut world, DT, 4);
            let now = sub_pos(body_get_position(&world, body), pos(CREST, 0.0, 0.0));
            let (candidates, calls) = world.solver_scratch.set_ccd_work();
            if clearance == 0.05 {
                assert!(!clamped(&world) && calls == 0, "5 cm clearance must cross the crest without a clamp or TOI calls");
                assert!((now.x - old.x - SPEED * DT).abs() < 0.001, "the clear sweep must retain free travel");
            } else {
                println!("dipped crest tick={tick} candidate_boxes={candidates} toi_calls={calls}");
                if tick == 0 {
                    assert!(candidates > 0 && calls > 0 && clamped(&world), "the dipped corner must take the per-child path and hit the rising face");
                }
                // Max terrain height under the 40 cm footprint is piecewise
                // linear with breaks at these x's. The endpoint poses alone
                // would also pass if the set went straight through the ridge.
                for x in [old.x, -0.6, -0.2, 0.2, 0.6, now.x] {
                    if x >= old.x && x <= now.x && now.x > old.x {
                        let t = (x - old.x) / (now.x - old.x);
                        let bottom = old.y + t * (now.y - old.y);
                        let road = crest_height((x.abs() - 0.2).max(0.0));
                        assert!(bottom >= road - constants::linear_slop(), "the swept underbody tunneled at x={x} on tick {tick}");
                    }
                }
            }
        }
        assert!(sub_pos(body_get_position(&world, body), pos(CREST, 0.0, 0.0)).x > 2.0, "the set must leave the crest without snagging");
    }
    // Reuse the striker for a vertical sweep through each other static target.
    // All endpoints would be below the surface without CCD.
    for (i, kind) in [ShapeType::Height, ShapeType::Hull, ShapeType::Sphere, ShapeType::Capsule].into_iter().enumerate() {
        let x = 20.0 + 4.0 * i as f32;
        let support = create_body(&mut world, &BodyDef { position: pos(x, 0.0, 0.0), ..default_body_def() });
        let z = 0.25;
        let target = match kind {
            ShapeType::Height => {
                let field = height_field::create_height_field(&HeightFieldDef {
                    heights: &[0.0; 4], count_x: 2, count_z: 2, scale: Vec3::ONE,
                    global_maximum_height: 1.0, ..Default::default()
                });
                create_height_field_shape(&mut world, support, &def, &field)
            }
            ShapeType::Hull => create_hull_shape(&mut world, support, &def, &hull::make_box_hull(0.4, 0.1, 0.5)),
            ShapeType::Sphere => create_sphere_shape(&mut world, support, &def, &Sphere { center: vec3(0.1, 0.0, z), radius: 0.15 }),
            ShapeType::Capsule => create_capsule_shape(&mut world, support, &def, &Capsule {
                center1: vec3(-0.1, 0.0, z), center2: vec3(0.3, 0.0, z), radius: 0.15,
            }),
            _ => unreachable!(),
        };
        assert!(target.is_non_null(), "the {kind:?} target must exist");
        launch(&mut world, pos(x + 0.1, 0.8, 0.0), vec3(0.0, -SPEED, 0.0), Vec3::ZERO);
        world_step(&mut world, DT, 4);
        let height = body_get_position(&world, body).y;
        assert!(clamped(&world) && height > 0.0 && height < 0.2, "a fast set must stop at the {kind:?} surface, got {height}");
    }
    destroy_world(world);
}

#[test]
fn static_set_sweeps_stop_at_posts_and_clear_an_overhang() {
    const DT: f32 = 1.0 / 60.0;
    let mut world = create_world(&WorldDef { gravity: Vec3::ZERO, ..default_world_def() });
    // Work in the target frame. Its nonidentity world pose also exercises the
    // relative sweep in both world precisions.
    let frame = WorldTransform { p: pos(7.0, 3.0, -5.0), q: Quat { v: vec3(0.5, 0.5, 0.5), s: 0.5 } };
    let body = create_body(&mut world, &BodyDef { position: frame.p, rotation: frame.q, ..default_body_def() });
    let def = default_shape_def();
    // Put the rear half in the earlier slot. Both halves lie in a fast
    // striker's swept bounds, but only the front face should stop it.
    let post = |width: f32| create_box_set(&[
        child(vec3(0.5 * width, -0.2, -0.5 * width), vec3(width, 0.4, 0.5 * width), 0),
        child(vec3(0.0, -0.2, -0.5 * width), vec3(0.5 * width, 0.4, 0.5 * width), 0),
        child(vec3(-1.5, 0.4, -1.0), vec3(1.5, 0.5, 1.0), 0),
    ]);
    let target = create_box_set_shape(&mut world, body, &def, &post(0.1));
    let dynamic = BodyDef {
        body_type: BodyType::Dynamic,
        motion_locks: MotionLocks { angular_x: true, angular_y: true, angular_z: true, ..Default::default() },
        ..default_body_def()
    };
    let striker = create_body(&mut world, &dynamic);
    let mut boxes = Vec::new();
    for column in 0..16 {
        for row in 0..4 {
            let x = (column as f32 - 8.0) / 16.0;
            let z = (row as f32 - 2.0) / 8.0;
            boxes.push(child(vec3(x, 0.0, z), vec3(x + 0.0625, 0.1, z + 0.125), 0));
        }
    }
    let shape = create_box_set_shape(&mut world, striker, &def, &create_box_set(&boxes));
    let launch = |w: &mut physics_world::World, id, local, speed| {
        body_set_transform(w, id, transform_world_point(frame, local), frame.q);
        body_set_linear_velocity(w, id, rotate_vector(frame.q, vec3(speed, 0.0, 0.0)));
        body_set_angular_velocity(w, id, Vec3::ZERO);
    };
    let x = |w: &physics_world::World, id| inv_transform_world_point(frame, body_get_position(w, id)).x;
    let flags = |w: &physics_world::World, id: BodyId| get_body_sim_from_id(w, id.index1 - 1).flags;

    for width in [0.10, 0.40] {
        shape_set_box_set(&mut world, target, &post(width));
        for speed in [20.0, 76.0] {
            launch(&mut world, striker, vec3(-0.5 - 0.4 * speed * DT, 0.0, 0.0), speed);
            world_step(&mut world, DT, 4);
            let (candidates, calls) = world.solver_scratch.set_ccd_work();
            println!("static set width={width} speed={speed} candidate_boxes={candidates} toi_calls={calls}");
            assert!(flags(&world, striker) & HAD_TIME_OF_IMPACT != 0, "{speed} m/s into a {width} m post must clamp on the impact tick");
            let gap = -(x(&world, striker) + 0.5);
            assert!((0.0..=2.0 * linear_slop()).contains(&gap), "the leading face must stop in front of the post, gap={gap}");
            assert!((1..=32).contains(&calls), "the impact tick must take at most a few tens of full TOI calls, got {calls}");
            for _ in 0..4 {
                world_step(&mut world, DT, 4);
                assert!(x(&world, striker) + 0.5 <= linear_slop(), "the following contact steps must keep the striker in front");
            }
            assert!(length(body_get_linear_velocity(&world, striker)) < 1.0, "the post must arrest the striker after the clamp");
        }
        // Both roots cover this pass. Every individual target box misses: the
        // upright is 4.5 cm to the side, and the upper box is above all 64 leaves.
        let start = vec3(-1.1, 0.0, 0.25 + 0.5 * width + 0.045);
        launch(&mut world, striker, start, 76.0);
        world_step(&mut world, DT, 4);
        let work = world.solver_scratch.set_ccd_work();
        println!("static set overhang width={width} candidate_boxes={} toi_calls={}", work.0, work.1);
        assert_eq!(flags(&world, striker) & (IS_FAST | HAD_TIME_OF_IMPACT), IS_FAST, "the near miss must be a live, unclamped sweep");
        assert_eq!(work, (0, 0), "the target tree must reject the overhang before enumerating striker leaves");
        assert!((x(&world, striker) - start.x - 76.0 * DT).abs() < 0.001, "the 4.5 cm pass must retain free travel");
    }

    // The centre of this bar stays still. Only the increasing projected
    // width of the rotating child brings it into the upright.
    shape_set_box_set(&mut world, target, &post(0.1));
    shape_set_box_set(&mut world, shape, &create_box_set(&[child(vec3(-0.015, -0.04, -0.65), vec3(0.015, 0.04, 0.65), 0)]));
    body_set_motion_locks(&mut world, striker, MotionLocks::default());
    launch(&mut world, striker, vec3(-0.2, 0.1, -0.4), 0.0);
    body_set_angular_velocity(&mut world, striker, rotate_vector(frame.q, vec3(0.0, 30.0, 0.0)));
    world_step(&mut world, DT, 4);
    assert!(flags(&world, striker) & HAD_TIME_OF_IMPACT != 0, "the interval prefilter must keep a rotating corner hit");
    destroy_body(&mut world, striker);

    for kind in [ShapeType::Sphere, ShapeType::Capsule, ShapeType::Hull] {
        let striker = create_body(&mut world, &dynamic);
        match kind {
            ShapeType::Sphere => create_sphere_shape(&mut world, striker, &def, &Sphere { center: Vec3::ZERO, radius: 0.08 }),
            ShapeType::Capsule => create_capsule_shape(&mut world, striker, &def, &Capsule {
                center1: vec3(0.0, 0.0, -0.04), center2: vec3(0.0, 0.0, 0.04), radius: 0.08,
            }),
            ShapeType::Hull => create_hull_shape(&mut world, striker, &def, &hull::make_box_hull(0.08, 0.08, 0.08)),
            _ => unreachable!(),
        };
        launch(&mut world, striker, vec3(-0.6, 0.1, 0.0), 76.0);
        world_step(&mut world, DT, 4);
        assert!(flags(&world, striker) & HAD_TIME_OF_IMPACT != 0, "a fast {kind:?} must hit the static set");
        assert!((x(&world, striker) + 0.08).abs() <= 2.0 * linear_slop(), "the {kind:?} must stop at the first face");
        assert_eq!(world.solver_scratch.set_ccd_work(), (2, 2), "only the two upright children should reach the convex TOI");
        destroy_body(&mut world, striker);
    }
    destroy_world(world);
}

fn seam_shape(world: &mut physics_world::World, body: BodyId, def: &ShapeDef, boxes: &[SetBox], as_set: bool) {
    if as_set {
        create_box_set_shape(world, body, def, &create_box_set(boxes));
    } else {
        for b in boxes {
            let h = aabb_extents(b.bounds);
            let hull = make_offset_box_hull(h.x, h.y, h.z, aabb_center(b.bounds));
            create_hull_shape(world, body, def, &hull);
        }
    }
}

#[test]
fn seam_two_body_curb_slide() {
    // Same section, mass tensor, preload, drive reversals and observation window
    // as the curb series. Only the longitudinal cuts differ between the bodies.
    const SIZE: Vec3 = Vec3 { x: 4.4, y: 1.4, z: 1.8 };
    const STEPS: i32 = 720;
    const REST_STEPS: i32 = 120;
    const SLIDE_STEPS: usize = (STEPS - REST_STEPS) as usize;
    let make_ground = |section: [(f32, f32); 4]| {
        let vertices = [-4.0 * SIZE.x - 10.0, 4.0 * SIZE.x + 10.0].map(|x| section.map(|(z, y)| vec3(x, y, z)));
        mesh::create_mesh(
            &MeshDef {
                vertices: vertices.as_flattened(),
                indices: &[0, 1, 5, 5, 4, 0, 1, 2, 6, 6, 5, 1, 2, 3, 7, 7, 6, 2],
                identify_edges: true,
                use_median_split: true,
                ..Default::default()
            },
            None,
        )
        .expect("the ground must form a continuous mesh")
    };
    let edge = 4.0 * SIZE.z + 5.0;
    let curb = make_ground([(-4.0, 0.45), (0.0, 0.45), (0.0, 0.0), (edge, 0.0)]);
    // Continue the same pavement past the missing curb for the full run under lateral preload.
    let flat_reach = default_world_def().maximum_linear_speed * (STEPS as f32 / 60.0) + SIZE.z;
    let flat = make_ground([(-flat_reach, 0.0), (-4.0, 0.0), (0.0, 0.0), (edge, 0.0)]);
    let def = ShapeDef { update_body_mass: false, ..default_shape_def() };
    let center = mul_sv(0.5, SIZE);
    let mass = MassData { mass: 1200.0, center, inertia: box_inertia(1200.0, Vec3::ZERO, SIZE) };
    let hull = make_offset_box_hull(center.x, center.y, center.z, center);
    let set = create_box_set(&[(0.0, 1.0), (1.0, 3.0), (3.0, SIZE.x)].map(|(lo, hi)| {
        child(vec3(lo, 0.0, 0.0), vec3(hi, SIZE.y, SIZE.z), 0)
    }));
    let run = |as_set: bool, workers, shift_mm: i32, has_curb: bool| {
        let mut w =
            create_world(&WorldDef { gravity: vec3(0.0, -30.0, 0.0), worker_count: workers, ..default_world_def() });
        let fixed = create_body(&mut w, &default_body_def());
        create_mesh_shape(&mut w, fixed, &def, if has_curb { &curb } else { &flat }, Vec3::ONE);
        let starts = [(0.5 * SIZE.z, 0.0), (1.5 * SIZE.z, shift_mm as f32 / 1000.0)];
        let [base, slider] = starts.map(|(side, dx)| {
            let origin = to_pos(sub(vec3(0.0, 0.5 * SIZE.y, side), center));
            let body = create_body(
                &mut w,
                &BodyDef {
                    body_type: BodyType::Dynamic,
                    position: offset_pos(origin, vec3(dx, 0.0, 0.0)),
                    ..default_body_def()
                },
            );
            if as_set {
                create_box_set_shape(&mut w, body, &def, &set);
            } else {
                create_hull_shape(&mut w, body, &def, &hull);
            }
            body_apply_mass_from_shapes(&mut w, body);
            body_set_mass_data(&mut w, body, mass);
            body
        });
        let relative_x = |w: &physics_world::World| {
            sub_pos(body_get_world_center_of_mass(w, slider), body_get_world_center_of_mass(w, base)).x
        };
        let mut dir = 1.0;
        let mut previous = false;
        let mut losses = Vec::new();
        let mut travel = 0.0f64;
        let mut supported = 0;
        let ground_center = body_get_world_center_of_mass(&w, fixed);
        let mut min_sep = f32::INFINITY;
        let mut min_row = None;
        let mut max_gap: Option<f32> = None;
        let mut tilted_manifolds = 0;
        let mut corrected_rows = 0;
        let mut dropped_rows = 0;
        let mut recovery_contacts = 0;
        let mut slider_friction_x = [0.0f64; SLIDE_STEPS];
        let compare_friction = shift_mm == 0 && workers == 1;
        let mut trace_phantom_until = None;
        let detailed = has_curb && shift_mm == 0 && workers == 1;
        let build_stamp = |c: &contact::Contact| {
            (c.contact_id, c.generation, c.cached_rotation_a, c.cached_rotation_b, c.cached_relative_pose)
        };
        let touches_base = |c: &contact::Contact| c.edges.iter().any(|edge| edge.body_id == base.index1 - 1);
        for tick in 0..STEPS {
            let base_center = body_get_world_center_of_mass(&w, base);
            let slider_center = body_get_world_center_of_mass(&w, slider);
            let start = sub_pos(slider_center, base_center).x;
            let base_speed = body_get_linear_velocity(&w, base).x;
            let before = body_get_linear_velocity(&w, slider).x - base_speed;
            let old_dir = dir;
            if start > 0.30 * SIZE.x {
                dir = -1.0;
            }
            if start < -0.30 * SIZE.x {
                dir = 1.0;
            }
            let push = if tick < REST_STEPS { 0.0 } else { (80.0 * (dir - before)).clamp(-90.0, 90.0) };
            let x = to_vec3(base_center).x;
            let brace = (-12.0 * x - 16.0 * base_speed).clamp(-60.0, 60.0);
            body_apply_force_to_center(&mut w, base, vec3(1200.0 * brace, 0.0, -3600.0), true);
            body_apply_force_to_center(&mut w, slider, vec3(1200.0 * push, 0.0, -27000.0), true);
            // The narrow phase writes a new pose on rebuild and retains the old one on reuse.
            // RELATIVE_TRANSFORM_VALID alone cannot distinguish a fresh ordinary row from a recycled one.
            let mut prior_side = None;
            let mut prior_ground_base = None;
            let mut prior_ground_slider = None;
            if tick >= REST_STEPS {
                let reusable =
                    contact::RELATIVE_TRANSFORM_VALID | contact::CONTACT_RECYCLE_FLAG | contact::CONTACT_TOUCHING_FLAG;
                for c in &w.contacts {
                    if c.contact_id == NULL_INDEX || c.flags & reusable != reusable {
                        continue;
                    }
                    if c.flags & (contact::SIM_SET_CONTACT | contact::SIM_MESH_CONTACT) != 0
                        && c.edges.iter().any(|edge| get_body_sim_from_id(&w, edge.body_id).flags & IS_FAST != 0)
                    {
                        continue;
                    }
                    let stamp = Some(build_stamp(c));
                    if c.flags & contact::CONTACT_STATIC_FLAG == 0 {
                        prior_side = stamp;
                    } else if touches_base(c) {
                        prior_ground_base = stamp;
                    } else {
                        prior_ground_slider = stamp;
                    }
                }
            }
            world_step(&mut w, 1.0 / 60.0, 4);
            let t = tick - REST_STEPS;
            if as_set {
                // Include settling ticks as well as the measured slide. A negative slide tick is in the settle.
                for c in &w.contacts {
                    if c.contact_id == NULL_INDEX || c.flags & contact::CONTACT_TOUCHING_FLAG == 0
                        || c.flags & contact::CONTACT_STATIC_FLAG != 0
                    {
                        continue;
                    }
                    let id = ContactId { index1: c.contact_id + 1, world0: base.world0, generation: c.generation };
                    let pair = if c.edges[0].body_id == base.index1 - 1 { "base/slider" } else { "slider/base" };
                    for (m, patch) in c.manifolds.iter().enumerate().filter(|(_, patch)| patch.normal.z.abs() < 0.9) {
                        eprintln!("tilted has_curb={has_curb} step={tick} slide_tick={t} pair={pair} patch={m} \
                            normal={:?} final_friction_impulse_on_b={:?}", patch.normal, patch.friction_impulse);
                        for (i, p) in patch.points[..patch.point_count as usize].iter().enumerate() {
                            let children = contact::contact_point_children(&w, id, m as i32, i as i32);
                            let (old, row_normal, branch) = match set_contact::debug_set_pair_row(&w, c, m, i) {
                                Some((old, n, side)) => {
                                    let branch = match side {
                                        manifold::FeatureOwner::ShapeA => "kept A",
                                        manifold::FeatureOwner::ShapeB => "A approved B",
                                    };
                                    (Some(old), Some(n), branch)
                                }
                                None if p.feature_id >> 24 == u32::from(u8::MAX) => (None, None, "embedded recovery"),
                                None => (None, None, "source row unavailable"),
                            };
                            eprintln!("  point={i} children_ab={children:?} feature={} old_normal={old:?} row_normal={row_normal:?} branch={branch} \
                                sep={} anchor_a={:?} anchor_b={:?} total_normal_impulse={}",
                                p.feature_id, p.separation, p.anchor_a, p.anchor_b, p.total_normal_impulse);
                        }
                    }
                }
            }
            let touching = |a: BodyId, b: BodyId| {
                w.contacts.iter().any(|c| {
                    c.contact_id != NULL_INDEX
                        && c.flags & contact::CONTACT_TOUCHING_FLAG != 0
                        && [c.edges[0].body_id, c.edges[1].body_id].contains(&(a.index1 - 1))
                        && [c.edges[0].body_id, c.edges[1].body_id].contains(&(b.index1 - 1))
                })
            };
            let seam = touching(base, slider);
            if tick >= REST_STEPS {
                for ctx in &w.task_contexts {
                    corrected_rows += ctx.set_corrected_rows;
                    dropped_rows += ctx.set_dropped_rows;
                    recovery_contacts += ctx.set_recovery_contacts;
                }
                let after = body_get_linear_velocity(&w, slider).x - body_get_linear_velocity(&w, base).x;
                let loss = (seam || previous) && dir == old_dir && before * dir > 0.0 && after.abs() < 0.70 * before.abs();
                if loss {
                    losses.push((t, before, after));
                }
                // Losses are appended in time order. Overlapping three-tick windows share their rows.
                let window = losses.iter().take(3).map(|&(start, _, _)| start).find(|&start| t <= start + 2);
                let print_rows = detailed && (window.is_some() || loss);
                let mut tick_min = f32::INFINITY;
                for c in &w.contacts {
                    if c.contact_id == NULL_INDEX || c.flags & contact::CONTACT_TOUCHING_FLAG == 0 {
                        continue;
                    }
                    let side = c.flags & contact::CONTACT_STATIC_FLAG == 0;
                    if !side && !print_rows {
                        continue;
                    }
                    let prior = if side {
                        prior_side
                    } else if touches_base(c) {
                        prior_ground_base
                    } else {
                        prior_ground_slider
                    };
                    let pose_valid = c.flags & contact::RELATIVE_TRANSFORM_VALID != 0;
                    let fresh = !pose_valid || prior != Some(build_stamp(c));
                    let body_info = |id: i32| {
                        if id == fixed.index1 - 1 {
                            ("ground", ground_center)
                        } else if id == base.index1 - 1 {
                            ("base", base_center)
                        } else {
                            ("slider", slider_center)
                        }
                    };
                    let (a, center_a) = body_info(c.edges[0].body_id);
                    let (b, center_b) = body_info(c.edges[1].body_id);
                    let dc = sub_pos(center_b, center_a);
                    let id = ContactId { index1: c.contact_id + 1, world0: base.world0, generation: c.generation };
                    for (m, patch) in c.manifolds.iter().enumerate() {
                        if side && compare_friction {
                            // The stored world-space vector is applied to B on the final substep.
                            // Sum once per patch, always signed as the impulse on the slider.
                            let jx = f64::from(patch.friction_impulse.x);
                            slider_friction_x[t as usize] += if c.edges[1].body_id == slider.index1 - 1 { jx } else { -jx };
                        }
                        if side && patch.normal.z.abs() < 0.9 {
                            tilted_manifolds += 1;
                        }
                        for (i, p) in patch.points[..patch.point_count as usize].iter().enumerate() {
                            let gap = fresh.then(|| dot(patch.normal, add(dc, sub(p.anchor_b, p.anchor_a))));
                            let slots = || contact::contact_point_children(&w, id, m as i32, i as i32);
                            if side {
                                tick_min = tick_min.min(p.separation);
                                if p.separation < min_sep {
                                    min_sep = p.separation;
                                    min_row = Some((tick, t, a, b, slots()));
                                }
                                if let Some(gap) = gap {
                                    max_gap = Some(max_gap.unwrap_or(0.0).max(gap.abs()));
                                }
                            }
                            if print_rows {
                                eprintln!("curb set={as_set} tick={t} pair={a}/{b} patch={m} point={i} children={:?} \
                                    n={:?} pose_valid={pose_valid} anchor_gap={gap:?} sep={} vn={} total_jn={} final_friction_x={}",
                                    slots(), patch.normal, p.separation, p.normal_velocity, p.total_normal_impulse, patch.friction_impulse.x);
                            }
                        }
                    }
                }
                let phantom = tick_min < -speculative_distance();
                if phantom {
                    trace_phantom_until = Some(t + 2);
                }
                if print_rows || (detailed && trace_phantom_until.is_some_and(|end| t <= end)) {
                    let recoveries: usize = w.task_contexts.iter().map(|ctx| ctx.set_recovery_contacts).sum();
                    eprintln!("curb set={as_set} tick={t} step={tick} first_loss_in_window={window:?} loss={loss} phantom={phantom} \
                        side_touching={seam} side_min_sep={tick_min} x={start:.6} vx={before:.6}->{after:.6} \
                        dir={dir} drive_accel={push} recoveries={recoveries}");
                }
                if seam {
                    travel += f64::from((relative_x(&w) - start).abs());
                }
                if seam && touching(base, fixed) && touching(slider, fixed) {
                    supported += 1;
                }
            }
            previous = seam;
        }
        eprintln!("curb set={as_set} workers={workers} shift_mm={shift_mm} has_curb={has_curb} min_side_sep_m={min_sep} \
            (step,slide_tick,body_a,body_b,children_ab)={min_row:?} max_fresh_abs_anchor_gap_m={max_gap:?} \
            manifolds_abs_normal_z_lt_0.9={tilted_manifolds} corrected_rows={corrected_rows} dropped_rows={dropped_rows} \
            recovery_contacts={recovery_contacts} travel_m={travel:.9} losses={losses:?}");
        if compare_friction {
            eprintln!("friction set={as_set} has_curb={has_curb} shift_mm={shift_mm} sum_final_substep_slider_jx={:.9}",
                slider_friction_x.iter().sum::<f64>());
        }
        assert!(supported > 0 && travel > 0.0, "the fixture must slide with both bodies supported (has_curb={has_curb})");
        destroy_world(w);
        (losses, travel, min_sep, tilted_manifolds, slider_friction_x)
    };
    let (first, unshifted, hull_min, _, hull_curb_jx) = run(false, 1, 0, true);
    let (mut low, mut high) = (unshifted, unshifted);
    for shift_mm in [1, -1, 2, -2] {
        let (_, distance, _, _, _) = run(false, 1, shift_mm, true);
        low = low.min(distance);
        high = high.max(distance);
    }
    let (repeat, _, _, _, _) = run(false, 4, 0, true);
    let (losses, travel, set_min, tilted, set_curb_jx) = run(true, 1, 0, true);
    let (_, hull_flat, _, _, hull_flat_jx) = run(false, 1, 0, false);
    let (_, set_flat, _, _, set_flat_jx) = run(true, 1, 0, false);
    for (has_curb, set_jx, hull_jx) in [(true, &set_curb_jx, &hull_curb_jx), (false, &set_flat_jx, &hull_flat_jx)] {
        let mut ticks: [usize; SLIDE_STEPS] = std::array::from_fn(|t| t);
        let diff = |t: usize| (set_jx[t] - hull_jx[t]).abs();
        ticks.sort_unstable_by(|&a, &b| diff(b).total_cmp(&diff(a)).then(a.cmp(&b)));
        for &t in &ticks[..5] {
            eprintln!("friction has_curb={has_curb} slide_tick={t} set_slider_jx={:.9} hull_slider_jx={:.9} abs_diff={:.9}",
                set_jx[t], hull_jx[t], diff(t));
        }
    }
    // A single flat baseline translates all five hull runs by the same amount, preserving their spread.
    let (low_delta, high_delta) = (low - hull_flat, high - hull_flat);
    let width_mm = 1000.0 * (high - low);
    let set_delta = travel - set_flat;
    // The travel gate (lane ruling, 2026-11-24). Loss side, the property the seam
    // work exists for: the set may lose no more travel to the curb than the hull does
    // at any of the five start shifts. Free side: a set that drops contacts slides too
    // freely, so what it gains over the hull's range on the curb may not exceed what it
    // gains over the hull on flat ground with no curb (a measured bound, no constant).
    // A two-sided band of the hull's start spread alone could not hold: on flat ground
    // the set already travels further than the hull by more than that spread.
    let flat_gain = (set_flat - hull_flat).abs();
    let curb_gain = (set_delta - high_delta).max(0.0);
    let loss_side = set_delta >= low_delta;
    let free_side = curb_gain <= flat_gain;
    let min_allowed = (hull_min - linear_slop()).max(-speculative_distance());
    eprintln!("curb hull_curb_m={unshifted:.9} hull_flat_m={hull_flat:.9} hull_delta_m={:.9} \
        hull_raw_range_m=[{low:.9}, {high:.9}] hull_delta_range_m=[{low_delta:.9}, {high_delta:.9}] \
        width_mm={width_mm:.6} under_1_mm={}", unshifted - hull_flat, width_mm < 1.0);
    eprintln!("curb set_curb_m={travel:.9} set_flat_m={set_flat:.9} set_delta_m={set_delta:.9} loss_side={loss_side} \
        curb_gain_mm={:.6} flat_gain_mm={:.6} free_side={free_side} \
        control_loss_count={} set_loss_count={} hull_min_sep_m={hull_min} set_min_sep_m={set_min} min_allowed_m={min_allowed} \
        manifolds_abs_normal_z_lt_0.9={tilted}", 1000.0 * curb_gain, 1000.0 * flat_gain, first.len(), losses.len());
    assert!(set_min.is_finite() && set_min >= min_allowed,
        "minimum slider/base separation {set_min} fell below {min_allowed} (hull minimum {hull_min})");
    assert_eq!(tilted, 0, "every slider/base curb manifold must have |normal.z| >= 0.9");
    assert_eq!((first.len(), repeat.len()), (1, 1), "the repeated hull control must reproduce its single loss tick");
    assert_eq!(losses.len(), first.len(), "the set's loss count must lie in the curb control envelope");
    assert!(loss_side, "set curb-minus-flat travel {set_delta} lost more than the hull's range {low_delta}..{high_delta}");
    assert!(free_side, "set curb-minus-flat travel {set_delta} exceeds the hull's range top {high_delta} by {curb_gain} m, \
        more than the set's flat-ground gain over the hull {flat_gain} m");
}

#[test]
fn seam_roll_and_slide_on_flat_and_stepped_supports() {
    let run = |as_set: bool, lip: f32| {
        let mut w = create_world(&default_world_def());
        let ground = create_body(&mut w, &default_body_def());
        let def = ShapeDef {
            base_material: SurfaceMaterial { friction: 0.0, ..default_surface_material() },
            ..default_shape_def()
        };
        let supports = [
            child(vec3(-3.0, -1.0, -2.0), vec3(0.0, 0.0, 2.0), 0),
            child(vec3(0.0, -1.0, -2.0), vec3(3.0, lip, 2.0), 0),
        ];
        if !as_set && lip == 0.0 {
            seam_shape(
                &mut w,
                ground,
                &def,
                &[child(supports[0].bounds.lower_bound, supports[1].bounds.upper_bound, 0)],
                false,
            );
        } else {
            seam_shape(&mut w, ground, &def, &supports, as_set);
        }
        let bodies = [false, true].map(|capsule| {
            let body = create_body(
                &mut w,
                &BodyDef {
                    body_type: BodyType::Dynamic,
                    position: pos(-0.6, 0.1, if capsule { 0.75 } else { -0.75 }),
                    linear_velocity: vec3(2.0, 0.0, 0.0),
                    angular_velocity: if capsule { Vec3::ZERO } else { vec3(0.0, 0.0, -20.0) },
                    ..default_body_def()
                },
            );
            let shape = if capsule {
                create_capsule_shape(
                    &mut w,
                    body,
                    &def,
                    &Capsule { center1: vec3(0.0, 0.0, -0.15), center2: vec3(0.0, 0.0, 0.15), radius: 0.1 },
                )
            } else {
                create_sphere_shape(&mut w, body, &def, &Sphere { center: Vec3::ZERO, radius: 0.1 })
            };
            (body, shape)
        });
        let mut facing = [0.0f32; 2];
        let mut floor = [false; 2];
        let mut first_wall = [None; 2];
        for tick in 0..42 {
            let before = bodies.map(|(body, _)| to_vec3(body_get_position(&w, body)));
            world_step(&mut w, 1.0 / 60.0, 4);
            for (i, &(body, shape)) in bodies.iter().enumerate() {
                for c in shape_get_contact_data(&w, shape, 3) {
                    for m in c.manifolds {
                        let n = if c.shape_id_b == shape { m.normal } else { neg(m.normal) };
                        facing[i] = facing[i].max(-n.x);
                        floor[i] |= n.y > 0.99;
                        if n.x < -0.99 && first_wall[i].is_none() {
                            let points = &m.points[..m.point_count as usize];
                            let separation = points.iter().map(|p| p.separation).fold(f32::INFINITY, f32::min);
                            let impulse: f32 = points.iter().map(|p| p.total_normal_impulse).sum();
                            first_wall[i] = Some((tick, before[i], separation, impulse, body_get_linear_velocity(&w, body)));
                        }
                    }
                }
            }
        }
        let result = bodies.map(|(body, _)| to_vec3(body_get_position(&w, body)));
        if lip > 0.0 {
            eprintln!("seam step set={as_set} lip={lip} sphere/capsule first wall (tick,start,sep,total_jn,end_velocity)={first_wall:?} end={result:?}");
        }
        assert!(floor.into_iter().all(|v| v), "both rolling and sliding bodies must have floor support");
        destroy_world(w);
        (result, facing)
    };
    for lip in [0.0, 0.25] {
        let (control, expected_normals) = run(false, lip);
        let (positions, normals) = run(true, lip);
        for (i, name) in ["rolling sphere", "sliding capsule"].into_iter().enumerate() {
            assert!(
                distance(positions[i], control[i]) < 2.0 * linear_slop(),
                "{name} travel and support height must match its control at lip {lip}: set {:?}, control {:?}",
                positions[i], control[i]
            );
            if lip == 0.0 {
                assert!(
                    positions[i].x > 0.5 && normals[i] < 0.01,
                    "{name} must cross the seam without a sideways support normal"
                );
            } else {
                assert!(
                    positions[i].x < 0.0 && normals[i] > 0.99 && expected_normals[i] > 0.99,
                    "{name} must stop at the exposed step face"
                );
            }
        }
    }
}

#[test]
fn seam_lips_corners_and_exposed_edit() {
    let mut w = create_world(&WorldDef { gravity: Vec3::ZERO, ..default_world_def() });
    let fixed = create_body(&mut w, &default_body_def());
    let def = default_shape_def();
    let block = |x1, y1, x2, y2| child(vec3(x1, y1, -1.0), vec3(x2, y2, 1.0), 0);
    let left = block(-1.0, -1.0, 0.0, 1.0);
    let lower = block(0.0, -1.0, 1.0, 0.0);
    let shape = create_box_set_shape(&mut w, fixed, &def, &create_box_set(&[left, lower]));
    let body = create_body(&mut w, &BodyDef { body_type: BodyType::Dynamic, ..default_body_def() });
    let ball = create_sphere_shape(&mut w, body, &def, &Sphere { center: Vec3::ZERO, radius: 0.03 });
    let normals = |w: &mut physics_world::World, x, y| {
        body_set_transform(w, body, pos(x, y, 0.0), Quat::IDENTITY);
        world_step(w, 0.0, 1);
        shape_get_contact_data(w, ball, 1)
            .into_iter()
            .flat_map(|c| c.manifolds.iter().map(|m| m.normal))
            .collect::<Vec<_>>()
    };
    let lip = normals(&mut w, 0.015, 0.5 * linear_slop());
    assert!(lip.iter().any(|n| n.x > 0.99), "half a slop above the ledge the wall must still face outward");
    shape_set_box_set(&mut w, shape, &create_box_set(&[block(-1.0, -1.0, 0.0, 0.0), block(0.0, 0.0, 1.0, 1.0)]));
    let edge = normals(&mut w, 0.015, -0.5 * linear_slop());
    assert!(edge.iter().any(|n| n.x > 0.99), "an edge-only neighbour must leave the first box's wall exposed");
    shape_set_box_set(
        &mut w,
        shape,
        &create_box_set(&[block(-1.0, -1.0, 0.0, 0.0), lower, block(-1.0, 0.0, 0.0, 1.0)]),
    );
    let corner = normals(&mut w, 0.015, 0.015);
    assert!(
        corner.iter().any(|n| n.x > 0.99) && corner.iter().any(|n| n.y > 0.99),
        "both real walls of the three-box inside corner must constrain the sphere"
    );
    assert!(
        corner.iter().all(|n| n.x > 0.99 || n.y > 0.99),
        "the doubly buried corner must emit no diagonal constraint"
    );

    shape_set_box_set(&mut w, shape, &create_box_set(&[block(-1.0, -0.2, 0.0, 0.0), block(0.0, -0.2, 1.0, 0.0)]));
    let center = vec3(-0.018, 0.018, 0.0);
    let witness = mul_sub(center, 0.03, normalize(center));
    // The unfiltered contact is the midpoint of the tile corner at the origin and the sphere witness.
    let contact_point = mul_sv(0.5, witness);
    body_set_transform(&mut w, body, to_pos(center), Quat::IDENTITY);
    world_step(&mut w, 0.0, 1);
    for _ in 0..2 {
        let contacts = shape_get_contact_data(&w, ball, 1);
        let c = &contacts[0];
        let mut found = false;
        for (m, patch) in c.manifolds.iter().enumerate() {
            for (i, p) in patch.points[..patch.point_count as usize].iter().enumerate() {
                if contact::contact_point_children(&w, c.contact_id, m as i32, i as i32) == Some((1, NULL_INDEX)) {
                    let tol = 0.01 * linear_slop();
                    assert!(
                        distance(patch.normal, Vec3::AXIS_Y) < tol,
                        "the right tile edge must retain only the upward normal"
                    );
                    let sphere_anchor = add(center, p.anchor_b);
                    assert!(
                        distance(sphere_anchor, contact_point) < tol,
                        "the sphere anchor must keep the unfiltered contact point"
                    );
                    assert!(
                        distance(p.anchor_a, contact_point) < tol && distance(p.anchor_a, sphere_anchor) < tol,
                        "the set and sphere anchors must coincide at the unfiltered contact point"
                    );
                    assert!(
                        (p.separation - witness.y).abs() < tol,
                        "refreshing at the same pose must preserve the signed plane distance"
                    );
                    found = true;
                }
            }
        }
        assert!(found, "the adjusted edge witness must survive beside the deeper floor support");
        world_step(&mut w, 0.0, 1);
    }

    // Both left children cover the right one's witness at the T-junction.
    // Neither child covers all four neighbours on its own.
    shape_set_box_set(
        &mut w,
        shape,
        &create_box_set(&[
            child(vec3(-1.0, -0.2, -1.0), vec3(0.0, 0.2, 0.0), 0),
            child(vec3(-1.0, -0.2, 0.0), vec3(0.0, 0.2, 1.0), 0),
            block(0.0, -0.2, 1.0, 0.2),
        ]),
    );
    let before = normals(&mut w, 0.006, 0.08);
    assert_eq!(before, [Vec3::AXIS_Y], "the covered internal face must give way to the nearest exterior exit");
    shape_edit_box_set(&mut w, shape, &[2], &[], &mut Vec::new());
    world_step(&mut w, 0.0, 1);
    let contacts = shape_get_contact_data(&w, ball, 1);
    assert_eq!(contacts.len(), 1, "the next narrow phase must see the exposed wall");
    let face = &contacts[0].manifolds[0];
    assert!(
        face.normal.x > 0.99 && face.points[0].separation < 0.0,
        "removing the neighbour must immediately expose a penetrating +X contact"
    );
    destroy_world(w);
}

#[test]
fn seam_embedded_exit_across_workers() {
    let run = |workers| {
        let mut w = create_world(&WorldDef {
            worker_count: workers,
            gravity: Vec3::ZERO,
            contact_speed: 1.0,
            ..default_world_def()
        });
        let fixed = create_body(&mut w, &default_body_def());
        let def = default_shape_def();
        let tiles = [-1.0, 0.0].map(|x| child(vec3(x, -0.4, -1.0), vec3(x + 1.0, 0.12, 23.0), 0));
        create_box_set_shape(&mut w, fixed, &def, &create_box_set(&tiles));
        let mut inside_hit = None;
        let proxy = ShapeProxy { points: &[Vec3::ZERO], radius: 0.02 };
        physics_world::world_cast_shape(
            &w,
            pos(0.0, 0.04, 0.0),
            &proxy,
            Vec3::AXIS_X,
            default_query_filter(),
            &mut |_, point, normal, fraction, _, _, child| {
                inside_hit = Some((point, normal, fraction, child));
                fraction
            },
        );
        let (point, normal, fraction, slot) =
            inside_hit.expect("a world cast starting inside the union must report its initial hit");
        assert_eq!(
            (normal, fraction),
            (Vec3::AXIS_Y, 0.0),
            "an embedded world cast must name the nearest exterior direction at fraction zero"
        );
        assert!(
            (point.y - 0.12).abs() < 1e-5 && (0..=1).contains(&slot),
            "the cast must report the exiting box and its top face"
        );
        let h = vec3(0.02, 0.02, 0.02);
        let embedded = create_box_set(&[child(neg(h), h, 0)]);
        // More than a narrow-phase block, with no contacts between the probes.
        let bodies: Vec<_> = (0..21)
            .map(|z| {
                let body = create_body(
                    &mut w,
                    &BodyDef {
                        body_type: BodyType::Dynamic,
                        position: pos(0.0, 0.04, z as f32),
                        motion_locks: MotionLocks {
                            angular_x: true,
                            angular_y: true,
                            angular_z: true,
                            ..Default::default()
                        },
                        ..default_body_def()
                    },
                );
                (body, create_box_set_shape(&mut w, body, &def, &embedded))
            })
            .collect();
        let mut trace = Vec::new();
        for tick in 0..36 {
            let start = to_vec3(body_get_position(&w, bodies[0].0));
            world_step(&mut w, 1.0 / 60.0, 1);
            if tick == 0 {
                assert_eq!(
                    w.task_contexts.iter().map(|t| t.set_recovery_contacts).sum::<usize>(),
                    bodies.len(),
                    "each fully embedded contact must recover exactly once"
                );
                assert!(
                    w.task_contexts.iter().map(|t| t.set_walk_steps).sum::<usize>() > 6 * bodies.len(),
                    "at least one axis walk per body must cross the buried seam"
                );
            }
            for &(body, shape) in &bodies {
                let contacts = shape_get_contact_data(&w, shape, 1);
                if body == bodies[0].0 && matches!(tick, 0 | 12 | 24 | 35) {
                    eprintln!("seam embedded workers={workers} tick={tick} center={start:?}->{:?} recoveries={}",
                        to_vec3(body_get_position(&w, body)), w.task_contexts.iter().map(|t| t.set_recovery_contacts).sum::<usize>());
                    for c in &contacts {
                        for m in c.manifolds {
                            for p in &m.points[..m.point_count as usize] {
                                let (n, arm) = if c.shape_id_b == shape { (m.normal, p.anchor_b) } else { (neg(m.normal), p.anchor_a) };
                                eprintln!("seam embedded n={n:?} witness_y={:.6} sep={:.6} feature={:#x} total_jn={:.8}",
                                    start.y + arm.y, p.separation, p.feature_id, p.total_normal_impulse);
                            }
                        }
                    }
                }
                if tick == 0 {
                    let c = &contacts[0];
                    assert_eq!(c.manifolds.len(), 1, "only one recovery plane may push an embedded body");
                    let m = &c.manifolds[0];
                    let outward = if c.shape_id_b == shape { m.normal } else { neg(m.normal) };
                    assert_eq!(
                        (outward, m.point_count),
                        (Vec3::AXIS_Y, 1),
                        "the shortest walk must exit upward through one point"
                    );
                    assert_eq!(m.points[0].feature_id >> 24, 0xff, "recovery must own a reserved warm-start feature");
                }
                let limit = body_get_mass(&w, body) * w.contact_speed;
                for c in contacts {
                    for m in c.manifolds {
                        for p in &m.points[..m.point_count as usize] {
                            assert!(
                                p.normal_impulse <= limit * 1.001,
                                "the recovery impulse must respect the push-out speed"
                            );
                            trace.push(p.normal_impulse.to_bits());
                        }
                    }
                }
                let center = to_vec3(body_get_position(&w, body));
                assert!(
                    length(body_get_linear_velocity(&w, body)) <= 1.001 * w.contact_speed,
                    "an embedded body at rest must not acquire more than the permitted push-out speed"
                );
                trace.extend([center.x.to_bits(), center.y.to_bits(), center.z.to_bits()]);
                if tick == 35 {
                    assert!(
                        center.x.abs() < linear_slop() && center.y >= 0.14 - linear_slop(),
                        "the body must clear the nearest exterior without moving along the seam: body {body:?} center {center:?}"
                    );
                }
            }
        }
        destroy_world(w);
        trace
    };
    assert_eq!(run(1), run(4), "embedded exit trajectories and impulses must be identical at one and four workers");
}

#[test]
fn seam_loaded_corner_and_thin_plate() {
    let mut w = create_world(&default_world_def());
    let fixed = create_body(&mut w, &default_body_def());
    let def = ShapeDef {
        base_material: SurfaceMaterial { friction: 0.0, ..default_surface_material() },
        ..default_shape_def()
    };
    let fixtures =
        [child(vec3(-1.0, -0.2, -2.0), vec3(1.0, 0.0, 2.0), 0), child(vec3(-1.0, 0.0, -2.0), vec3(0.0, 0.4, 2.0), 0)];
    create_box_set_shape(&mut w, fixed, &def, &create_box_set(&fixtures));
    let body = |w: &mut physics_world::World, position, half: Vec3, vx| {
        let id = create_body(
            w,
            &BodyDef {
                body_type: BodyType::Dynamic,
                position,
                linear_velocity: vec3(vx, 0.0, 0.0),
                ..default_body_def()
            },
        );
        let shape = create_hull_shape(w, id, &def, &make_box_hull(half.x, half.y, half.z));
        (id, shape)
    };
    let (cube, cube_shape) = body(&mut w, pos(0.098, 0.099, -0.5), vec3(0.1, 0.1, 0.1), 0.0);
    let (plate, _) = body(&mut w, pos(0.35, 0.005, 0.5), vec3(0.1, 0.005, 0.1), -0.8);
    let mut supported_flat = false;
    let mut stopped = false;
    for _ in 0..50 {
        let mut near = false;
        let mut far = false;
        let mass = body_get_mass(&w, cube);
        body_apply_force_to_center(&mut w, cube, vec3(-4.0 * mass, 0.0, 0.0), true);
        world_step(&mut w, 1.0 / 60.0, 4);
        for c in shape_get_contact_data(&w, cube_shape, 1) {
            for m in c.manifolds {
                if m.normal.y < 0.99 {
                    continue;
                }
                for p in &m.points[..m.point_count as usize] {
                    near |= p.anchor_b.x < -0.09 && p.total_normal_impulse > 0.0;
                    far |= p.anchor_b.x > 0.09 && p.total_normal_impulse > 0.0;
                }
            }
        }
        supported_flat |= near && far;
        let x = to_vec3(body_get_position(&w, plate)).x;
        assert!(x >= 0.1 - linear_slop(), "the centimetre plate must never pass under the raised wall");
        stopped |= x < 0.12 && body_get_linear_velocity(&w, plate).x.abs() < 0.03;
    }
    assert!(supported_flat, "the slab must carry both the pressed edge and the free edge in the same step");
    assert!(
        rotate_vector(body_get_rotation(&w, cube), Vec3::AXIS_Y).y > 0.999,
        "the box pressed against the wall must stay flat"
    );
    assert!(stopped, "the moving centimetre plate must reach the wall and stop");
    destroy_world(w);
}

#[test]
fn seam_fast_mesh_rail_scrape() {
    let vertices = [vec3(-10.0, -2.0, 0.0), vec3(30.0, -2.0, 0.0), vec3(30.0, 2.0, 0.0), vec3(-10.0, 2.0, 0.0)];
    let rail = mesh::create_mesh(
        &MeshDef { vertices: &vertices, indices: &[0, 1, 2, 2, 3, 0], identify_edges: true, ..Default::default() },
        None,
    )
    .expect("the straight rail must have two nondegenerate triangles");
    let run = |as_set| {
        let mut w = create_world(&WorldDef { gravity: Vec3::ZERO, ..default_world_def() });
        let fixed = create_body(&mut w, &default_body_def());
        let def = ShapeDef {
            base_material: SurfaceMaterial { friction: 0.0, ..default_surface_material() },
            ..default_shape_def()
        };
        create_mesh_shape(&mut w, fixed, &def, &rail, Vec3::ONE);
        let angle = 10.0f32.to_radians();
        let rotation = make_quat_from_axis_angle(Vec3::AXIS_Y, angle);
        let body = create_body(
            &mut w,
            &BodyDef {
                body_type: BodyType::Dynamic,
                rotation,
                position: pos(0.0, 0.0, angle.sin() + 0.3 * angle.cos() + 0.01),
                linear_velocity: rotate_vector(rotation, vec3(30.0, 0.0, 0.0)),
                ..default_body_def()
            },
        );
        let h = vec3(1.0, 0.2, 0.3);
        let mut loaded_slots = std::collections::BTreeSet::new();
        let mut shape = [ShapeId::default()];
        if as_set {
            let cuts = [-1.0, -0.6, -0.2, 0.2, 0.6, 1.0];
            let boxes: Vec<_> =
                cuts.windows(2).map(|x| child(vec3(x[0], -h.y, -h.z), vec3(x[1], h.y, h.z), 0)).collect();
            seam_shape(&mut w, body, &def, &boxes, true);
        } else {
            seam_shape(&mut w, body, &def, &[child(neg(h), h, 0)], false);
        }
        body_get_shapes(&w, body, &mut shape);
        let mut losses = Vec::new();
        let mut touching = 0;
        for tick in 0..48 {
            let before = body_get_linear_velocity(&w, body).x;
            let mass = body_get_mass(&w, body);
            body_apply_force_to_center(&mut w, body, vec3(0.0, 0.0, -12.0 * mass), true);
            world_step(&mut w, 1.0 / 60.0, 4);
            if as_set {
                for c in shape_get_contact_data(&w, shape[0], 1) {
                    for (m, patch) in c.manifolds.iter().enumerate() {
                        for (i, p) in patch.points[..patch.point_count as usize].iter().enumerate() {
                            if p.total_normal_impulse > 0.0 {
                                let (_, slot) = contact::contact_point_children(&w, c.contact_id, m as i32, i as i32)
                                    .expect("loaded rail point must name a live box");
                                loaded_slots.insert(slot);
                            }
                        }
                    }
                }
            }
            touching += usize::from(
                w.contacts.iter().any(|c| c.contact_id != NULL_INDEX && c.flags & contact::CONTACT_TOUCHING_FLAG != 0),
            );
            if before > 0.0 && body_get_linear_velocity(&w, body).x.abs() < 0.7 * before {
                losses.push(tick);
            }
        }
        assert!(
            touching > 1 && body_get_position(&w, body).x > 20.0,
            "the fast body must sustain contact and travel along the rail"
        );
        assert!(!as_set || loaded_slots.len() >= 2, "the scrape must load more than the first child along the rail");
        destroy_world(w);
        losses
    };
    let control = run(false);
    let set = run(true);
    assert_eq!(
        set.len(),
        control.len(),
        "the 30 m/s, ten-degree rail scrape must match the hull's loss count: set {set:?}, hull {control:?}"
    );
}

#[test]
fn a_long_set_resting_tilted_on_a_mesh_rests_on_its_lowest_corners_and_sleeps() {
    // A felled trunk: two short feet near one end (on different materials,
    // as voxel children are), a deeper-cut foot at the far end and a beam
    // joining them. It rests tilted by about 1.4 degrees, so each near foot's
    // inner corners sit about 4.7 mm above its outer ones. One patch spans
    // both ends; the reduction once kept the widest corners there, the solver
    // rested on the higher ones and every later cull that let a lower corner
    // through pushed it out again, so the trunk gained energy and never slept.
    let floor = mesh::create_grid_mesh(1, 1, 64.0, 0, true);
    let set = create_box_set(&[
        child(vec3(1.6, -0.5, -0.1), vec3(2.4, 0.0, 0.1), 0),
        child(vec3(2.4, -0.5, -0.1), vec3(2.6, 0.0, 0.1), 1),
        child(vec3(15.2, -0.2, -0.1), vec3(15.5, 0.0, 0.1), 1),
        child(vec3(1.6, 0.0, -0.1), vec3(15.5, 0.3, 0.1), 0),
    ]);
    let mut world = create_world(&default_world_def());
    let ground = create_body(&mut world, &default_body_def());
    let def = default_shape_def();
    create_mesh_shape(&mut world, ground, &def, &floor, Vec3::ONE);
    let trunk = create_body(
        &mut world,
        &BodyDef { body_type: BodyType::Dynamic, position: pos(-8.0, 0.52, 0.0), ..default_body_def() },
    );
    let set_def = ShapeDef { materials: vec![def.base_material, def.base_material], ..def };
    let shape = create_box_set_shape(&mut world, trunk, &set_def, &set);
    let asleep_at = (1..=600).find(|_| {
        world_step(&mut world, 1.0 / 60.0, 4);
        !body_is_awake(&world, trunk)
    });
    assert!(asleep_at.is_some(), "a resting trunk must fall asleep within 10 s");
    let height = body_get_position(&world, trunk).y;
    for _ in 0..60 {
        world_step(&mut world, 1.0 / 60.0, 4);
        assert!(!body_is_awake(&world, trunk), "a sleeping trunk must stay asleep");
    }
    assert_eq!(body_get_position(&world, trunk).y, height, "nothing moves a sleeping trunk");
    // The near end rests on its lowest corners: the outer edge of the short foot.
    let contacts = shape_get_contact_data(&world, shape, 4);
    let near_end_points: Vec<f32> = contacts
        .iter()
        .flat_map(|c| c.manifolds.iter())
        .flat_map(|m| m.points[..m.point_count as usize].iter().map(|p| p.anchor_a.x))
        .filter(|x| *x < 0.0)
        .collect();
    let outer = -8.0 + 2.6;
    assert!(
        near_end_points.iter().any(|x| (x - outer).abs() < 0.02),
        "the near end must be supported at its lowest corners (x = {outer}), got {near_end_points:?}"
    );
    destroy_world(world);
}
