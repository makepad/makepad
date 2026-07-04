// Port of box3d/test/test_compound.c
//
// Deviations from C (see PORTING.md):
// - compound->version / byteCount assertions are skipped: the Rust CompoundData
//   has no blob layout, so those fields don't exist.
// - tree.nodes != NULL becomes !tree.nodes.is_empty().
// - CompoundSerializeRoundtrip / BadVersion / WrongByteCount are skipped:
//   b3ConvertCompoundToBytes/b3ConvertBytesToCompound (in-place blob
//   serialization) are not ported.
// - b3DestroyCompound / b3DestroyMesh are Arc drops.

use std::sync::Arc;

use makepad_box3d::compound::*;
use makepad_box3d::hull::make_box_hull;
use makepad_box3d::math_functions::{make_quat_from_axis_angle, vec3, Quat, Transform, Vec3, AABB, PI};
use makepad_box3d::mesh::{create_box_mesh, get_mesh_material_count};
use makepad_box3d::types::*;
use makepad_box3d::{ensure, ensure_small};

fn make_material(friction: f32, user_id: u64) -> SurfaceMaterial {
    let mut m = default_surface_material();
    m.friction = friction;
    m.user_material_id = user_id;
    m
}

#[test]
fn compound_create_mixed() {
    let mat = default_surface_material();

    let capsules = vec![CompoundCapsuleDef {
        capsule: Capsule { center1: vec3(-1.0, 0.0, 0.0), center2: vec3(1.0, 0.0, 0.0), radius: 0.25 },
        material: mat,
    }];

    let box_hull = make_box_hull(0.5, 0.5, 0.5);
    let hulls = vec![CompoundHullDef {
        hull: Arc::clone(&box_hull),
        transform: Transform::IDENTITY,
        material: mat,
    }];

    let mesh_data = create_box_mesh(Vec3::ZERO, vec3(0.5, 0.5, 0.5), false);
    let meshes = vec![CompoundMeshDef {
        mesh_data: Arc::clone(&mesh_data),
        transform: Transform::IDENTITY,
        scale: vec3(1.0, 1.0, 1.0),
        materials: vec![mat],
    }];

    let spheres = vec![
        CompoundSphereDef { sphere: Sphere { center: vec3(5.0, 0.0, 0.0), radius: 0.5 }, material: mat },
        CompoundSphereDef { sphere: Sphere { center: vec3(-5.0, 0.0, 0.0), radius: 0.5 }, material: mat },
    ];

    let def = CompoundDef { capsules, hulls, meshes, spheres };

    let compound = create_compound(&def);
    // C: compound->version == B3_COMPOUND_VERSION and byteCount checks skipped
    // (no blob layout in the Rust port).

    ensure!(compound.capsules.len() == 1);
    ensure!(compound.hulls.len() == 1);
    ensure!(compound.meshes.len() == 1);
    ensure!(compound.spheres.len() == 2);

    ensure!(compound.materials.len() == 1);
    ensure!(compound.shared_hull_count == 1);
    ensure!(compound.shared_mesh_count == 1);

    ensure!(compound.tree.node_count > 0);
    ensure!(!compound.tree.nodes.is_empty());
}

#[test]
fn compound_create_single_type() {
    let mat = default_surface_material();

    // Capsule only
    {
        let cap = CompoundCapsuleDef {
            capsule: Capsule { center1: vec3(0.0, 0.0, 0.0), center2: vec3(1.0, 0.0, 0.0), radius: 0.5 },
            material: mat,
        };
        let def = CompoundDef { capsules: vec![cap], ..Default::default() };
        let c = create_compound(&def);
        ensure!(c.capsules.len() == 1 && c.hulls.is_empty() && c.meshes.is_empty() && c.spheres.is_empty());
    }

    // Hull only
    {
        let box_hull = make_box_hull(1.0, 1.0, 1.0);
        let h = CompoundHullDef { hull: box_hull, transform: Transform::IDENTITY, material: mat };
        let def = CompoundDef { hulls: vec![h], ..Default::default() };
        let c = create_compound(&def);
        ensure!(c.hulls.len() == 1 && c.shared_hull_count == 1 && c.capsules.is_empty());
    }

    // Mesh only
    {
        let md = create_box_mesh(Vec3::ZERO, vec3(1.0, 1.0, 1.0), false);
        let m = CompoundMeshDef {
            mesh_data: md,
            transform: Transform::IDENTITY,
            scale: vec3(1.0, 1.0, 1.0),
            materials: vec![mat],
        };
        let def = CompoundDef { meshes: vec![m], ..Default::default() };
        let c = create_compound(&def);
        ensure!(c.meshes.len() == 1 && c.shared_mesh_count == 1);
    }

    // Sphere only
    {
        let s = CompoundSphereDef { sphere: Sphere { center: vec3(0.0, 0.0, 0.0), radius: 1.0 }, material: mat };
        let def = CompoundDef { spheres: vec![s], ..Default::default() };
        let c = create_compound(&def);
        ensure!(c.spheres.len() == 1);
    }
}

#[test]
fn compound_material_dedup() {
    let mat = make_material(0.4, 7);

    let mut caps = Vec::new();
    for i in 0..3 {
        caps.push(CompoundCapsuleDef {
            capsule: Capsule {
                center1: vec3(i as f32, 0.0, 0.0),
                center2: vec3(i as f32 + 1.0, 0.0, 0.0),
                radius: 0.25,
            },
            material: mat,
        });
    }

    let def = CompoundDef { capsules: caps, ..Default::default() };
    let c = create_compound(&def);
    ensure!(c.materials.len() == 1);
    for i in 0..3 {
        let cc = get_compound_capsule(&c, i);
        ensure!(cc.material_index == 0);
    }
}

#[test]
fn compound_material_distinct() {
    let mut caps = Vec::new();
    for i in 0..3i32 {
        caps.push(CompoundCapsuleDef {
            capsule: Capsule {
                center1: vec3(i as f32, 0.0, 0.0),
                center2: vec3(i as f32 + 1.0, 0.0, 0.0),
                radius: 0.25,
            },
            material: make_material(0.1 * (i + 1) as f32, (i + 1) as u64),
        });
    }

    let def = CompoundDef { capsules: caps, ..Default::default() };
    let c = create_compound(&def);
    ensure!(c.materials.len() == 3);

    let mats = get_compound_materials(&c);
    ensure!(!mats.is_empty());

    for i in 0..3i32 {
        let cc = get_compound_capsule(&c, i);
        ensure!(cc.material_index >= 0 && cc.material_index < 3);
        ensure!(mats[cc.material_index as usize].user_material_id == (i + 1) as u64);
    }
}

#[test]
fn compound_material_cross_shape() {
    // One material shared across capsule, hull, and sphere -> 1 material slot.
    let mat = make_material(0.5, 99);

    let cap = CompoundCapsuleDef {
        capsule: Capsule { center1: vec3(0.0, 0.0, 0.0), center2: vec3(1.0, 0.0, 0.0), radius: 0.25 },
        material: mat,
    };
    let box_hull = make_box_hull(0.5, 0.5, 0.5);
    let hull = CompoundHullDef { hull: box_hull, transform: Transform::IDENTITY, material: mat };
    let sph = CompoundSphereDef { sphere: Sphere { center: vec3(5.0, 0.0, 0.0), radius: 0.5 }, material: mat };

    let def = CompoundDef {
        capsules: vec![cap],
        hulls: vec![hull],
        spheres: vec![sph],
        ..Default::default()
    };
    let c = create_compound(&def);
    ensure!(c.materials.len() == 1);

    ensure!(get_compound_capsule(&c, 0).material_index == 0);
    ensure!(get_compound_hull(&c, 0).material_index == 0);
    ensure!(get_compound_sphere(&c, 0).material_index == 0);
}

#[test]
fn compound_material_mesh_shared() {
    // Mesh material entries are routed through the same material map as convex
    // materials, so an identical material is deduped across mesh and convex.
    let mat = make_material(0.3, 11);

    let md = create_box_mesh(Vec3::ZERO, vec3(1.0, 1.0, 1.0), false);
    ensure!(get_mesh_material_count(&md) == 1);

    let sph = CompoundSphereDef { sphere: Sphere { center: vec3(5.0, 0.0, 0.0), radius: 0.5 }, material: mat };
    let mesh = CompoundMeshDef {
        mesh_data: md,
        transform: Transform::IDENTITY,
        scale: vec3(1.0, 1.0, 1.0),
        materials: vec![mat],
    };

    let def = CompoundDef { meshes: vec![mesh], spheres: vec![sph], ..Default::default() };
    let c = create_compound(&def);
    ensure!(c.materials.len() == 1);
}

// ---------------------------------------------------------------------------
// Hull / mesh sharing
// ---------------------------------------------------------------------------

#[test]
fn compound_hull_sharing_pointer() {
    let mat = default_surface_material();
    let box_hull = make_box_hull(1.0, 1.0, 1.0);

    let mut hulls = Vec::new();
    for i in 0..3 {
        let mut transform = Transform::IDENTITY;
        transform.p.x = (4 * i) as f32;
        hulls.push(CompoundHullDef { hull: Arc::clone(&box_hull), transform, material: mat });
    }

    let def = CompoundDef { hulls, ..Default::default() };
    let c = create_compound(&def);
    ensure!(c.hulls.len() == 3);
    ensure!(c.shared_hull_count == 1);
}

#[test]
fn compound_hull_sharing_content() {
    let mat = default_surface_material();

    // Two box hulls built independently with identical args are content-identical
    // (make_box_hull is deterministic).
    let box_a = make_box_hull(1.0, 1.0, 1.0);
    let box_b = make_box_hull(1.0, 1.0, 1.0);
    ensure!(!Arc::ptr_eq(&box_a, &box_b));

    let mut transform_b = Transform::IDENTITY;
    transform_b.p.x = 5.0;

    let hulls = vec![
        CompoundHullDef { hull: box_a, transform: Transform::IDENTITY, material: mat },
        CompoundHullDef { hull: box_b, transform: transform_b, material: mat },
    ];

    let def = CompoundDef { hulls, ..Default::default() };
    let c = create_compound(&def);
    ensure!(c.shared_hull_count == 1);
}

#[test]
fn compound_hull_distinct() {
    let mat = default_surface_material();
    let box_a = make_box_hull(1.0, 1.0, 1.0);
    let box_b = make_box_hull(2.0, 1.0, 1.0);

    let mut transform_b = Transform::IDENTITY;
    transform_b.p.x = 5.0;

    let hulls = vec![
        CompoundHullDef { hull: box_a, transform: Transform::IDENTITY, material: mat },
        CompoundHullDef { hull: box_b, transform: transform_b, material: mat },
    ];

    let def = CompoundDef { hulls, ..Default::default() };
    let c = create_compound(&def);
    ensure!(c.shared_hull_count == 2);
}

#[test]
fn compound_mesh_sharing_pointer() {
    let mat = default_surface_material();
    let md = create_box_mesh(Vec3::ZERO, vec3(1.0, 1.0, 1.0), false);

    let mut meshes = Vec::new();
    for i in 0..3 {
        let mut transform = Transform::IDENTITY;
        transform.p.x = (4 * i) as f32;
        meshes.push(CompoundMeshDef {
            mesh_data: Arc::clone(&md),
            transform,
            scale: vec3(1.0, 1.0, 1.0),
            materials: vec![mat],
        });
    }

    let def = CompoundDef { meshes, ..Default::default() };
    let c = create_compound(&def);
    ensure!(c.meshes.len() == 3);
    ensure!(c.shared_mesh_count == 1);
}

#[test]
fn compound_mesh_sharing_content() {
    let mat = default_surface_material();
    let md_a = create_box_mesh(Vec3::ZERO, vec3(1.0, 1.0, 1.0), false);
    let md_b = create_box_mesh(Vec3::ZERO, vec3(1.0, 1.0, 1.0), false);
    ensure!(!Arc::ptr_eq(&md_a, &md_b));

    let mut transform_b = Transform::IDENTITY;
    transform_b.p.x = 5.0;

    let meshes = vec![
        CompoundMeshDef {
            mesh_data: md_a,
            transform: Transform::IDENTITY,
            scale: vec3(1.0, 1.0, 1.0),
            materials: vec![mat],
        },
        CompoundMeshDef { mesh_data: md_b, transform: transform_b, scale: vec3(1.0, 1.0, 1.0), materials: vec![mat] },
    ];

    let def = CompoundDef { meshes, ..Default::default() };
    let c = create_compound(&def);
    ensure!(c.shared_mesh_count == 1);
}

#[test]
fn compound_mesh_distinct() {
    let mat = default_surface_material();
    let md_a = create_box_mesh(Vec3::ZERO, vec3(1.0, 1.0, 1.0), false);
    let md_b = create_box_mesh(Vec3::ZERO, vec3(2.0, 1.0, 1.0), false);

    let mut transform_b = Transform::IDENTITY;
    transform_b.p.x = 5.0;

    let meshes = vec![
        CompoundMeshDef {
            mesh_data: md_a,
            transform: Transform::IDENTITY,
            scale: vec3(1.0, 1.0, 1.0),
            materials: vec![mat],
        },
        CompoundMeshDef { mesh_data: md_b, transform: transform_b, scale: vec3(1.0, 1.0, 1.0), materials: vec![mat] },
    ];

    let def = CompoundDef { meshes, ..Default::default() };
    let c = create_compound(&def);
    ensure!(c.shared_mesh_count == 2);
}

#[test]
fn compound_child_dispatch() {
    let mat = default_surface_material();
    let box_hull = make_box_hull(0.5, 0.5, 0.5);
    let md = create_box_mesh(Vec3::ZERO, vec3(0.5, 0.5, 0.5), false);

    let caps = vec![
        CompoundCapsuleDef {
            capsule: Capsule { center1: vec3(0.0, 0.0, 0.0), center2: vec3(1.0, 0.0, 0.0), radius: 0.2 },
            material: mat,
        },
        CompoundCapsuleDef {
            capsule: Capsule { center1: vec3(0.0, 2.0, 0.0), center2: vec3(1.0, 2.0, 0.0), radius: 0.2 },
            material: mat,
        },
    ];
    let mut hull_transform = Transform::IDENTITY;
    hull_transform.p = vec3(5.0, 0.0, 0.0);
    let hulls = vec![CompoundHullDef { hull: box_hull, transform: hull_transform, material: mat }];

    let mut mesh_transform = Transform::IDENTITY;
    mesh_transform.p = vec3(0.0, 0.0, 5.0);
    let meshes = vec![CompoundMeshDef {
        mesh_data: md,
        transform: mesh_transform,
        scale: vec3(1.0, 1.0, 1.0),
        materials: vec![mat],
    }];
    let spheres =
        vec![CompoundSphereDef { sphere: Sphere { center: vec3(-5.0, 0.0, 0.0), radius: 0.5 }, material: mat }];

    let def = CompoundDef { capsules: caps, hulls, meshes, spheres };
    let c = create_compound(&def);

    // Index ordering is capsules -> hulls -> meshes -> spheres.
    ensure!(get_compound_child(&c, 0).shape_type() == ShapeType::Capsule);
    ensure!(get_compound_child(&c, 1).shape_type() == ShapeType::Capsule);
    ensure!(get_compound_child(&c, 2).shape_type() == ShapeType::Hull);
    ensure!(get_compound_child(&c, 3).shape_type() == ShapeType::Mesh);
    ensure!(get_compound_child(&c, 4).shape_type() == ShapeType::Sphere);

    // Capsule and sphere children always report identity transform — the position
    // is encoded in the shape itself (capsule center1/2, sphere center).
    let cap0 = get_compound_child(&c, 0);
    ensure_small!(cap0.transform.p.x, f32::EPSILON);
    ensure_small!(cap0.transform.p.y, f32::EPSILON);
    ensure_small!(cap0.transform.p.z, f32::EPSILON);
    match &cap0.geom {
        ChildShapeGeom::Capsule(capsule) => ensure_small!(capsule.center2.x - 1.0, f32::EPSILON),
        _ => panic!("child 0 is not a capsule"),
    }

    let sph = get_compound_child(&c, 4);
    ensure_small!(sph.transform.p.x, f32::EPSILON);
    match &sph.geom {
        ChildShapeGeom::Sphere(sphere) => ensure_small!(sphere.center.x + 5.0, f32::EPSILON),
        _ => panic!("child 4 is not a sphere"),
    }

    // Hull and mesh children carry their stored transform.
    let hull = get_compound_child(&c, 2);
    ensure_small!(hull.transform.p.x - 5.0, f32::EPSILON);

    let mesh = get_compound_child(&c, 3);
    ensure_small!(mesh.transform.p.z - 5.0, f32::EPSILON);
    match &mesh.geom {
        // C: mesh.mesh.data != NULL
        ChildShapeGeom::Mesh(m) => ensure!(!m.data.triangles.is_empty()),
        _ => panic!("child 3 is not a mesh"),
    }
}

#[test]
fn compound_aabb_contains_children() {
    let mat = default_surface_material();
    let spheres = vec![
        CompoundSphereDef { sphere: Sphere { center: vec3(-3.0, 0.0, 0.0), radius: 1.0 }, material: mat },
        CompoundSphereDef { sphere: Sphere { center: vec3(4.0, 0.0, 0.0), radius: 0.5 }, material: mat },
    ];
    let def = CompoundDef { spheres, ..Default::default() };
    let c = create_compound(&def);

    let local = compute_compound_aabb(&c, Transform::IDENTITY);
    ensure!(local.lower_bound.x <= -4.0 + 1e-5);
    ensure!(local.upper_bound.x >= 4.5 - 1e-5);
    ensure!(local.lower_bound.y <= -1.0 + 1e-5);
    ensure!(local.upper_bound.y >= 1.0 - 1e-5);

    // Translation commutes through the bounding-box transform.
    let xf = Transform { p: vec3(10.0, 20.0, 30.0), q: Quat::IDENTITY };
    let world = compute_compound_aabb(&c, xf);
    ensure_small!(world.lower_bound.x - (local.lower_bound.x + 10.0), 1e-4);
    ensure_small!(world.upper_bound.y - (local.upper_bound.y + 20.0), 1e-4);
    ensure_small!(world.lower_bound.z - (local.lower_bound.z + 30.0), 1e-4);
}

#[test]
fn compound_ray_cast_miss() {
    let mat = default_surface_material();
    let sph = CompoundSphereDef { sphere: Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.5 }, material: mat };
    let def = CompoundDef { spheres: vec![sph], ..Default::default() };
    let c = create_compound(&def);

    // Ray well above the sphere on a parallel path.
    let input = RayCastInput { origin: vec3(-5.0, 5.0, 0.0), translation: vec3(10.0, 0.0, 0.0), max_fraction: 1.0 };
    let out = ray_cast_compound(&c, &input);
    ensure!(!out.hit);
}

#[test]
fn compound_ray_cast_closest() {
    let mat_a = make_material(0.4, 100);
    let mat_b = make_material(0.4, 200);

    // Two unit spheres along +X. Ray from origin must hit the nearer one first.
    let spheres = vec![
        CompoundSphereDef { sphere: Sphere { center: vec3(5.0, 0.0, 0.0), radius: 1.0 }, material: mat_a },
        CompoundSphereDef { sphere: Sphere { center: vec3(10.0, 0.0, 0.0), radius: 1.0 }, material: mat_b },
    ];
    let def = CompoundDef { spheres, ..Default::default() };
    let c = create_compound(&def);

    let input = RayCastInput { origin: vec3(0.0, 0.0, 0.0), translation: vec3(20.0, 0.0, 0.0), max_fraction: 1.0 };
    let out = ray_cast_compound(&c, &input);
    ensure!(out.hit);

    // Front face of the nearer sphere is at x=4 -> fraction 4/20 = 0.2.
    ensure_small!(out.fraction - 0.2, 1e-4);
    ensure_small!(out.normal.x + 1.0, 1e-4);
    ensure!(out.child_index == 0);

    let mats = get_compound_materials(&c);
    ensure!(mats[out.material_index as usize].user_material_id == 100);
}

#[test]
fn compound_ray_cast_hull_normal_rotation() {
    // A unit box rotated 90 degrees about Z, placed at compound +X. The ray hits
    // the face that, in compound space, points back toward -X. Verifies that the
    // normal returned by the cast has been rotated from hull-local space back
    // into compound space.
    let mat = default_surface_material();
    let box_hull = make_box_hull(1.0, 1.0, 1.0);

    let hull = CompoundHullDef {
        hull: box_hull,
        transform: Transform { p: vec3(5.0, 0.0, 0.0), q: make_quat_from_axis_angle(Vec3::AXIS_Z, 0.5 * PI) },
        material: mat,
    };
    let def = CompoundDef { hulls: vec![hull], ..Default::default() };
    let c = create_compound(&def);

    let input = RayCastInput { origin: vec3(0.0, 0.0, 0.0), translation: vec3(20.0, 0.0, 0.0), max_fraction: 1.0 };
    let out = ray_cast_compound(&c, &input);
    ensure!(out.hit);
    // Box has |hx|=1; with the rotation a face still intersects the +X ray at x=4.
    ensure_small!(out.fraction - 0.2, 1e-4);
    ensure_small!(out.normal.x + 1.0, 1e-3);
    ensure_small!(out.normal.y, 1e-3);
    ensure_small!(out.normal.z, 1e-3);
}

#[test]
fn compound_shape_cast_closest() {
    let mat = default_surface_material();
    let spheres = vec![
        CompoundSphereDef { sphere: Sphere { center: vec3(5.0, 0.0, 0.0), radius: 1.0 }, material: mat },
        CompoundSphereDef { sphere: Sphere { center: vec3(10.0, 0.0, 0.0), radius: 1.0 }, material: mat },
    ];
    let def = CompoundDef { spheres, ..Default::default() };
    let c = create_compound(&def);

    let point = [vec3(0.0, 0.0, 0.0)];
    let input = ShapeCastInput {
        proxy: ShapeProxy { points: &point, radius: 0.25 },
        translation: vec3(20.0, 0.0, 0.0),
        max_fraction: 1.0,
        can_encroach: false,
    };
    let out = shape_cast_compound(&c, &input);
    ensure!(out.hit);
    // Closest contact: caster radius 0.25 + sphere radius 1.0 -> first contact at x ~= 3.75.
    ensure_small!(out.fraction - 3.75 / 20.0, 1e-3);
    ensure!(out.child_index == 0);
}

#[test]
fn compound_overlap() {
    let mat = default_surface_material();
    let spheres = vec![
        CompoundSphereDef { sphere: Sphere { center: vec3(-3.0, 0.0, 0.0), radius: 0.5 }, material: mat },
        CompoundSphereDef { sphere: Sphere { center: vec3(3.0, 0.0, 0.0), radius: 0.5 }, material: mat },
    ];
    let def = CompoundDef { spheres, ..Default::default() };
    let c = create_compound(&def);

    // Proxy at the origin lies in the gap between the two spheres.
    let origin = [vec3(0.0, 0.0, 0.0)];
    let gap = ShapeProxy { points: &origin, radius: 0.25 };
    ensure!(!overlap_compound(&c, Transform::IDENTITY, &gap));

    // Proxy at the center of the second sphere overlaps it.
    let on_second = [vec3(3.0, 0.0, 0.0)];
    let hit = ShapeProxy { points: &on_second, radius: 0.1 };
    ensure!(overlap_compound(&c, Transform::IDENTITY, &hit));
}

#[test]
fn compound_query() {
    let mat = default_surface_material();
    let spheres = vec![
        CompoundSphereDef { sphere: Sphere { center: vec3(-10.0, 0.0, 0.0), radius: 0.5 }, material: mat },
        CompoundSphereDef { sphere: Sphere { center: vec3(0.0, 0.0, 0.0), radius: 0.5 }, material: mat },
        CompoundSphereDef { sphere: Sphere { center: vec3(10.0, 0.0, 0.0), radius: 0.5 }, material: mat },
    ];
    let def = CompoundDef { spheres, ..Default::default() };
    let c = create_compound(&def);

    // Tight box around the middle sphere — only it should be reported.
    let middle = AABB { lower_bound: vec3(-1.0, -1.0, -1.0), upper_bound: vec3(1.0, 1.0, 1.0) };
    let mut child_indices: Vec<i32> = Vec::new();
    query_compound(&c, middle, &mut |_compound, child_index| {
        child_indices.push(child_index);
        true
    });
    ensure!(child_indices.len() == 1);
    ensure!(child_indices[0] == 1);

    // Wide box overlapping all three; early-exit after the first reported child.
    let wide = AABB { lower_bound: vec3(-20.0, -1.0, -1.0), upper_bound: vec3(20.0, 1.0, 1.0) };
    let mut stop_count = 0;
    query_compound(&c, wide, &mut |_compound, _child_index| {
        stop_count += 1;
        stop_count < 1
    });
    ensure!(stop_count == 1);

    // Without early-exit, all three are visited.
    let mut all_count = 0;
    query_compound(&c, wide, &mut |_compound, _child_index| {
        all_count += 1;
        true
    });
    ensure!(all_count == 3);
}

#[test]
fn compound_mover() {
    let mat = default_surface_material();
    let box_hull = make_box_hull(0.5, 0.5, 0.5);

    // Two boxes side-by-side along X, gap of 1 between them.
    let hulls = vec![
        CompoundHullDef {
            hull: Arc::clone(&box_hull),
            transform: Transform { p: vec3(-1.0, 0.0, 0.0), q: Quat::IDENTITY },
            material: mat,
        },
        CompoundHullDef {
            hull: Arc::clone(&box_hull),
            transform: Transform { p: vec3(1.0, 0.0, 0.0), q: Quat::IDENTITY },
            material: mat,
        },
    ];
    let def = CompoundDef { hulls, ..Default::default() };
    let c = create_compound(&def);

    // Small capsule mover sitting on top of the boxes, low enough to penetrate both.
    let mover = Capsule { center1: vec3(-1.0, 0.6, 0.0), center2: vec3(1.0, 0.6, 0.0), radius: 0.2 };

    let mut planes = [PlaneResult::default(); 8];
    let plane_count = collide_mover_and_compound(&mut planes, &c, &mover);

    // Both boxes contribute at least one plane each; the +Y face of each box
    // should produce a plane whose normal points roughly +Y in compound space.
    ensure!(plane_count >= 2);

    let mut up_planes = 0;
    for i in 0..plane_count {
        if planes[i as usize].plane.normal.y > 0.9 {
            up_planes += 1;
        }
    }
    ensure!(up_planes >= 2);

    // Capacity cap is honored: ask for 1 and the call must not exceed it.
    let mut one = [PlaneResult::default(); 1];
    let capped = collide_mover_and_compound(&mut one, &c, &mover);
    ensure!(capped <= 1);
}

// C: CompoundSerializeRoundtrip, CompoundSerializeBadVersion, and
// CompoundSerializeWrongByteCount are not ported: the in-place blob
// serialization (b3ConvertCompoundToBytes / b3ConvertBytesToCompound) is not
// part of the Rust port.
